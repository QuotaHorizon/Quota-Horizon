"""Prospective evaluation of calendar click preferences, separate from forecasts.

Only observed increments qualify. The first eligible sample in each local day is
frozen once, and matched forecasts are captured then, before learning its outcome.
Neither candidate is automatically admitted to the production probability model.
"""
from __future__ import annotations

import json
import math

from .benchmark import label_window, matching, trial_probability
from .contracts import RELIEF, digest, iso, packed
from .diagnostics import coverage_details

VERSION = "calendar-preference-increments-1"
MINIMUM_VOTES = 50
WINDOW_SECONDS = 6 * 3600
MAX_GAP_SECONDS = 1800


def capture(store, source, observed_at, received_at, poll):
    if observed_at > received_at or poll.get("kind") != "calendar_preference":
        raise ValueError("invalid preference observation")
    if poll["target"] != RELIEF or poll["unit"] != "clicks" or poll["max_votes_per_ip_24h"] != 100:
        raise ValueError("unreviewed preference units")
    bins = {b["p"]: b["votes"] for b in poll["bins"]}
    if set(bins) != {0., 1.} or len(poll["bins"]) != 2 or any(isinstance(n, bool) or not isinstance(n, int) or n < 0 for n in bins.values()):
        raise ValueError("invalid preference counts")
    if sum(bins.values()) != poll["total"] or poll["expires"] <= poll["start"]:
        raise ValueError("invalid preference total or clock")
    record = {**poll, "source": source, "observed_at": observed_at, "received_at": received_at}
    key = digest([source, poll["id"], observed_at])
    with store.db:
        store.db.execute("INSERT OR IGNORE INTO preference_samples VALUES (?,?,?,?,?,?)",
                         (key, source, poll["id"], observed_at, received_at, packed(record)))


def recent_samples(store, source, round_id, now):
    rows = store.db.execute("""SELECT payload FROM preference_samples
        WHERE source=? AND round_id=? AND received_at<=? AND observed_at BETWEEN ? AND ?
        ORDER BY observed_at,id""", (source, round_id, now, now-WINDOW_SECONDS, now))
    return [json.loads(r[0]) for r in rows]


def features(samples, now):
    latest = samples[-1]
    reset = feature = 0
    weighted_reset = weighted_feature = 0.
    last_vote = None
    corrections = gaps = 0
    first_at = latest["observed_at"]
    for before, after in zip(samples, samples[1:]):
        a, b = ({x["p"]: x["votes"] for x in p["bins"]} for p in (before, after))
        left, right = before["observed_at"], after["observed_at"]
        dr, df = b[1.]-a[1.], b[0.]-a[0.]
        if dr < 0 or df < 0 or right-left > MAX_GAP_SECONDS:
            # Rebase after corrections/gaps; an unknown batch cannot contribute.
            corrections += int(dr < 0 or df < 0)
            gaps += int(right-left > MAX_GAP_SECONDS)
            reset = feature = 0
            weighted_reset = weighted_feature = 0.
            last_vote = None
            first_at = right
            continue
        first_at = min(first_at, left)
        weight = 2 ** (-(now-left)/(2*3600))
        reset += dr
        feature += df
        weighted_reset += weight*dr
        weighted_feature += weight*df
        if dr+df:
            last_vote = right
    n = reset+feature
    weighted_n = weighted_reset+weighted_feature
    # This is a sensitivity model using 100 perfectly correlated clicks per
    # voting unit, not an estimate of unique people or an asserted sample size.
    repeat_units = weighted_n/latest["max_votes_per_ip_24h"]
    state = "ready_for_evaluation"
    if now >= latest["expires"] or not latest["voting_open"] or latest["upstream_outcome"] is not None:
        state = "closed"
    elif now-latest["observed_at"] > MAX_GAP_SECONDS:
        state = "stale"
    elif n < MINIMUM_VOTES:
        state = "insufficient_recent_votes"
    elif now < latest["start"]:
        state = "upcoming"
    return {
        "source": latest["source"], "round_id": latest["id"], "date": latest["date"],
        "observed_at": latest["observed_at"], "received_at": latest["received_at"],
        "window_start": latest["start"], "window_end": latest["expires"],
        "state": state, "question": latest["question"], "unit": "clicks",
        "total_clicks": latest["total"], "recent_votes": n,
        "reset_clicks": reset, "feature_clicks": feature,
        "reset_share": reset/n if n >= MINIMUM_VOTES else None,
        "weighted_reset_share": weighted_reset/weighted_n if n >= MINIMUM_VOTES else None,
        "repeat_adjusted_units": repeat_units,
        "repeat_sensitive_candidate": (weighted_reset/100+.5)/(repeat_units+1) if n >= MINIMUM_VOTES else None,
        "unique_participants": None, "max_votes_per_ip_24h": 100,
        "minimum_ip_units_under_published_limit": math.ceil(n/100),
        "sampling_since": first_at, "latest_vote_at": last_vote,
        "corrections": corrections, "observation_gaps": gaps,
        "upstream_outcome": latest["upstream_outcome"],
        "production_weight": 0., "admission": "independent_evaluation",
    }


def current_features(store, now):
    keys = store.db.execute("""SELECT source,round_id FROM preference_samples
        WHERE received_at<=? GROUP BY source,round_id""", (now,)).fetchall()
    output = []
    for source, round_id in keys:
        samples = recent_samples(store, source, round_id, now)
        if samples:
            output.append(features(samples, now))
    return output


def freeze(store, now, policy, evidence):
    created = 0
    for f in current_features(store, now):
        if f["state"] != "ready_for_evaluation":
            continue
        # The calendar question is already answered once an independently
        # confirmed reset occurred in its day; later votes cannot predict it.
        if any(e["status"] == "confirmed" and matching(e, RELIEF) and e.get("to") is not None
               and f["window_start"] <= e["to"] <= now
               for e in evidence["events"]):
            continue
        key = digest([f["source"], f["round_id"], VERSION, policy.hash])
        if store.db.execute("SELECT 1 FROM preference_trials WHERE id=?", (key,)).fetchone():
            continue
        hours = (f["window_end"]-now)/3600
        candidates = {"raw_click_share": f["weighted_reset_share"],
                      "repeat_sensitive_share": f["repeat_sensitive_candidate"]}
        comparisons = []
        for source in store.forecasts(now, latest=True):
            if source.target != RELIEF or source.private or source.track not in ("model", "native"):
                continue
            probability, _ = trial_probability(source, now, hours, evidence["events"], policy)
            if probability is not None:
                comparisons.append({"source": source.source, "version": source.version,
                                    "forecast_id": source.id, "probability": probability})
        trial = {"source": f["source"], "round_id": f["round_id"], "version": VERSION,
                 "target": RELIEF, "origin": now, "hours": hours, "frozen_at": now,
                 "p": f["weighted_reset_share"],
                 "policy_hash": policy.hash, "features": f, "candidates": candidates,
                 "comparisons": comparisons}
        with store.db:
            cursor = store.db.execute("INSERT OR IGNORE INTO preference_trials VALUES (?,?,?)", (key, now, packed(trial)))
        created += cursor.rowcount
    return created


def settle(store, now, policy, evidence):
    changed = 0
    for row in store.db.execute("SELECT id,payload FROM preference_trials WHERE frozen_at<=?", (now,)):
        trial = json.loads(row["payload"])
        coverage = coverage_details(store, trial["origin"], trial["origin"]+trial["hours"]*3600, now, policy)
        result = label_window(trial, evidence["events"], now, policy, coverage["covered"])
        # Preserve the same outcome/missingness policy as the main evaluator.
        result["truth_hash"] = digest(evidence)
        result["coverage"] = coverage
        losses = {}
        if result.get("label") is not None:
            values = {**trial["candidates"], **{c["source"]+":"+c["version"]: c["probability"] for c in trial["comparisons"]}}
            for name, p in values.items():
                clipped = max(1e-6, min(1-1e-6, p)); y = result["label"]
                losses[name] = {"probability": p, "brier": (p-y)**2,
                                "log_loss": -(y*math.log(clipped)+(1-y)*math.log1p(-clipped))}
        result["candidate_losses"] = losses
        signature = digest(result)
        last = store.db.execute("SELECT signature FROM preference_settlements WHERE trial=? ORDER BY at DESC,rowid DESC LIMIT 1", (row["id"],)).fetchone()
        if last and last[0] == signature:
            continue
        with store.db:
            store.db.execute("INSERT INTO preference_settlements VALUES (?,?,?,?)", (row["id"], now, signature, packed(result)))
        changed += 1
    return changed


def report(store, now):
    latest = current_features(store, now)
    trials = []
    for row in store.db.execute("SELECT id,payload FROM preference_trials WHERE frozen_at<=? ORDER BY frozen_at DESC", (now,)):
        trial = json.loads(row["payload"])
        last = store.db.execute("SELECT payload FROM preference_settlements WHERE trial=? AND at<=? ORDER BY at DESC,rowid DESC LIMIT 1", (row["id"], now)).fetchone()
        trials.append({"id": row["id"], **trial, "settlement": json.loads(last[0]) if last else None})
    return {"version": VERSION, "minimum_recent_votes": MINIMUM_VOTES,
            "sampling_window_hours": WINDOW_SECONDS/3600,
            "vote_units": "clicks_not_unique_people", "repeat_sensitivity": "100_clicks_per_unit_Jeffreys_prior",
            "question_scope": "preference_or_prediction_for_one_Pacific_calendar_day",
            "production_weight": 0., "automatic_promotion": False,
            "latest": latest, "trials": trials,
            "sample_observations": store.db.execute("SELECT COUNT(*) FROM preference_samples WHERE received_at<=?", (now,)).fetchone()[0],
            "scored_days": sum(bool(t["settlement"] and t["settlement"].get("score_eligible")) for t in trials)}
