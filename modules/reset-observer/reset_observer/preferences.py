"""Prospective evaluation of calendar click preferences, separate from forecasts.

Only observed increments qualify. The first eligible sample in each local day and
prospective intraday checkpoints freeze their contemporary matched forecasts.
Neither candidate is automatically admitted to the production probability model.
"""
from __future__ import annotations

import json
import math
from datetime import datetime
from zoneinfo import ZoneInfo

from .benchmark import label_window, matching, trial_probability
from .contracts import RELIEF, digest, iso, packed
from .diagnostics import coverage_details

VERSION = "calendar-preference-increments-1"
MINIMUM_VOTES = 50
WINDOW_SECONDS = 6 * 3600
MAX_GAP_SECONDS = 1800
CHECKPOINT_VERSION = "calendar-preference-checkpoints-1"
CHECKPOINT_HOURS = (6, 12, 18)
PACIFIC = "America/Los_Angeles"
MINIMUM_TREND_COVERAGE = .9


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
                         (key, source, poll["id"], observed_at, received_at,
                          store.pack_json(record, volatile_fields=("observed_at", "received_at"))))


def recent_samples(store, source, round_id, now, lookback=WINDOW_SECONDS):
    rows = store.db.execute("""SELECT payload FROM preference_samples
        WHERE source=? AND round_id=? AND received_at<=? AND observed_at BETWEEN ? AND ?
        ORDER BY observed_at,id""", (source, round_id, now, now-lookback, now))
    return [json.loads(r[0]) for r in rows]


def increments(samples, now):
    """Only count batches between observed endpoints, restarting after uncertainty."""
    reset = feature = 0
    weighted_reset = weighted_feature = 0.
    last_vote = None
    corrections = gaps = 0
    first_at = samples[-1]["observed_at"] if samples else now
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
    return {"reset": reset, "feature": feature, "weighted_reset": weighted_reset,
            "weighted_feature": weighted_feature, "first_at": first_at,
            "last_vote": last_vote, "corrections": corrections, "gaps": gaps}


def hourly_trend(samples, now):
    bins = []
    for index in range(6):
        start, end = now-(6-index)*3600, now-(5-index)*3600
        # A batch crossing an edge has no individual ballot timestamps. Leave
        # it unallocated instead of interpolating or assigning it to one hour.
        within = [s for s in samples if start <= s["observed_at"] <= end]
        delta = increments(within, end)
        exposure = max(0., within[-1]["observed_at"]-delta["first_at"]) if within else 0.
        coverage = min(1., exposure/3600)
        votes = delta["reset"]+delta["feature"]
        # Each plotted point uses the same trailing-six-hour qualification as
        # production. A quiet single hour cannot hide a qualified recent sample.
        rolling = [s for s in samples if end-WINDOW_SECONDS <= s["observed_at"] <= end]
        estimate = increments(rolling, end)
        sample_votes = estimate["reset"] + estimate["feature"]
        weighted = estimate["weighted_reset"] + estimate["weighted_feature"]
        sample_coverage = max(0., rolling[-1]["observed_at"]-estimate["first_at"])/WINDOW_SECONDS if rolling else 0.
        current = bool(rolling) and end-rolling[-1]["observed_at"] <= MAX_GAP_SECONDS
        state = ("incomplete_coverage" if not current or (estimate["gaps"] and sample_votes < MINIMUM_VOTES) else
                 "insufficient_recent_votes" if sample_votes < MINIMUM_VOTES else "available")
        bins.append({"start": start, "end": end, "new_clicks": votes,
                     "reset_clicks": delta["reset"], "feature_clicks": delta["feature"],
                     "coverage_fraction": coverage,
                     "sample_votes": sample_votes, "sample_coverage_fraction": min(1., sample_coverage),
                     "reset_share": estimate["weighted_reset"]/weighted if state == "available" and weighted else None,
                     "clicks_per_hour": votes*3600/exposure if coverage >= MINIMUM_TREND_COVERAGE and exposure else None,
                     "state": state})
    previous, current = bins[-2:]
    comparable = previous["state"] == current["state"] == "available"
    return {"window_hours": 6, "bin_hours": 1, "minimum_bin_votes": MINIMUM_VOTES,
            "estimate_window_hours": 6, "estimate_kind": "rolling_weighted_preference",
            "minimum_coverage": MINIMUM_TREND_COVERAGE, "bins": bins,
            "change": {"state": "available" if comparable else "awaiting_comparable_hours",
                       "reset_share_delta": current["reset_share"]-previous["reset_share"] if comparable else None,
                       "click_rate_ratio": current["clicks_per_hour"]/previous["clicks_per_hour"]
                       if current["clicks_per_hour"] is not None and previous["clicks_per_hour"] else None}}


def features(samples, now):
    latest = samples[-1]
    delta = increments([s for s in samples if now-WINDOW_SECONDS <= s["observed_at"] <= now], now)
    reset, feature = delta["reset"], delta["feature"]
    weighted_reset, weighted_feature = delta["weighted_reset"], delta["weighted_feature"]
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
        "sampling_since": delta["first_at"], "latest_vote_at": delta["last_vote"],
        "corrections": delta["corrections"], "observation_gaps": delta["gaps"],
        "upstream_outcome": latest["upstream_outcome"],
        "activity": latest.get("activity"),
        "trend": hourly_trend(samples, now) if latest["start"] <= now < latest["expires"] else None,
        "production_weight": 0., "admission": "independent_evaluation",
    }


def current_features(store, now):
    # The covering index answers this without visiting every payload row.
    # recent_samples applies both clocks; future-only keys produce no features.
    keys = store.db.execute("SELECT DISTINCT source,round_id FROM preference_samples").fetchall()
    output = []
    for source, round_id in keys:
        samples = recent_samples(store, source, round_id, now, lookback=2*WINDOW_SECONDS)
        if samples:
            output.append(features(samples, now))
    return output


def checkpoint(f, now, activated_at, first_origin):
    """Select only the current future-enabled slot, never reconstruct past slots."""
    local_start = datetime.fromtimestamp(f["window_start"], ZoneInfo(PACIFIC))
    boundaries = [(hour, local_start.replace(hour=hour, minute=0, second=0, microsecond=0).timestamp())
                  for hour in CHECKPOINT_HOURS]
    for index, (hour, at) in enumerate(boundaries):
        end = boundaries[index+1][1] if index+1 < len(boundaries) else f["window_end"]
        if (f["window_start"] <= at < f["window_end"] and at >= activated_at
                and first_origin < at <= f["observed_at"] <= now < end):
            return {"mode": "fixed_checkpoint", "version": CHECKPOINT_VERSION,
                    "timezone": PACIFIC, "local_hour": hour, "scheduled_at": at,
                    "slot_end": end, "observation_at": f["observed_at"],
                    "delay_seconds": now-at}
    return None


def frozen_trial(store, f, now, policy, evidence, sampling):
    hours = (f["window_end"]-now)/3600
    candidates = {"raw_click_share": f["weighted_reset_share"],
                  "repeat_sensitive_share": f["repeat_sensitive_candidate"]}
    comparisons = []
    for source in store.forecasts(now, latest=True):
        baseline = source.source == "survival_baseline" and source.track == "shadow"
        community_control = source.source == "without_community" and source.track == "shadow"
        if source.target != RELIEF or source.private or (source.track not in ("model", "native") and not baseline and not community_control):
            continue
        probability, _ = trial_probability(source, now, hours, evidence["events"], policy)
        if probability is not None:
            comparison = {"source": source.source, "version": source.version,
                          "forecast_id": source.id, "probability": probability}
            if baseline:
                comparison["metadata"] = {k: source.metadata[k] for k in ("baseline_kind",) if k in source.metadata}
            comparisons.append(comparison)
    return {"source": f["source"], "round_id": f["round_id"], "version": VERSION,
            "target": RELIEF, "origin": now, "hours": hours, "frozen_at": now,
            "p": f["weighted_reset_share"], "sampling": sampling,
            "activity_day_cluster": digest([f["source"], f["round_id"]]),
            "evaluation_endpoint": "within_day_completion",
            "policy_hash": policy.hash, "features": f, "candidates": candidates,
            "comparisons": comparisons}


def freeze(store, now, policy, evidence):
    activation_key = CHECKPOINT_VERSION + ":activated_at"
    activated_at = store.get(activation_key)
    if activated_at is None:
        store.put(activation_key, now)
        activated_at = now
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
        first = store.db.execute("SELECT payload FROM preference_trials WHERE id=?", (key,)).fetchone()
        if first is None:
            sampling = {"mode": "first_eligible", "timezone": PACIFIC}
        else:
            sampling = checkpoint(f, now, activated_at, json.loads(first[0])["origin"])
            if sampling is None:
                continue
            key = digest([f["source"], f["round_id"], CHECKPOINT_VERSION,
                          sampling["local_hour"], policy.hash])
            if store.db.execute("SELECT 1 FROM preference_trials WHERE id=?", (key,)).fetchone():
                continue
        trial = frozen_trial(store, f, now, policy, evidence, sampling)
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


def activity_endpoint(sample, settlement):
    """The upstream activity classification is a separate research endpoint."""
    outcome = sample.get("upstream_outcome") if sample else None
    label = {"reset": 1, "both": 1, "improvement": 0}.get(outcome)
    completion_label = settlement.get("label") if settlement else None
    mismatch = None
    if label is not None and completion_label is not None and label != completion_label:
        mismatch = ("activity_reset_without_within_day_completion" if label == 1
                    else "within_day_completion_without_activity_reset")
    return {"endpoint": "activity_classification", "scope": "whole_Pacific_activity_day",
            "upstream_outcome": outcome, "label": label,
            "activity": sample.get("activity") if sample else None,
            "classification_basis": "upstream_outcome",
            "source_url": sample.get("question_url") if sample else None,
            "observed_at": sample.get("observed_at") if sample else None,
            "received_at": sample.get("received_at") if sample else None,
            "state": "reported" if label is not None else "awaiting_upstream_classification",
            "comparison_endpoint": "within_day_completion", "completion_label": completion_label,
            "mismatch": mismatch, "used_for_completion_settlement": False}


def report(store, now):
    latest = current_features(store, now)
    trials = []
    activity_samples = {}
    for row in store.db.execute("SELECT id,payload FROM preference_trials WHERE frozen_at<=? ORDER BY frozen_at DESC", (now,)):
        trial = json.loads(row["payload"])
        last = store.db.execute("SELECT payload FROM preference_settlements WHERE trial=? AND at<=? ORDER BY at DESC,rowid DESC LIMIT 1", (row["id"], now)).fetchone()
        settlement = json.loads(last[0]) if last else None
        day = (trial["source"], trial["round_id"])
        if day not in activity_samples:
            sample = store.db.execute("""SELECT payload FROM preference_samples
                WHERE source=? AND round_id=? AND observed_at<=? AND received_at<=?
                ORDER BY observed_at DESC,id DESC LIMIT 1""", (*day, now, now)).fetchone()
            activity_samples[day] = json.loads(sample[0]) if sample else None
        trials.append({"id": row["id"], **trial,
                       "sampling": trial.get("sampling", {"mode": "first_eligible", "timezone": PACIFIC}),
                       "activity_day_cluster": trial.get("activity_day_cluster", digest(list(day))),
                       "evaluation_endpoint": "within_day_completion", "settlement": settlement,
                       "activity_classification": activity_endpoint(activity_samples[day], settlement)})
    scored = [t for t in trials if t["settlement"] and t["settlement"].get("score_eligible")]
    day_keys = lambda rows: {(t["source"], t["round_id"]) for t in rows}
    return {"version": VERSION, "minimum_recent_votes": MINIMUM_VOTES,
            "sampling_window_hours": WINDOW_SECONDS/3600,
            "vote_units": "clicks_not_unique_people", "repeat_sensitivity": "100_clicks_per_unit_Jeffreys_prior",
            "question_scope": "preference_or_prediction_for_one_Pacific_calendar_day",
            "production_weight": 0., "automatic_promotion": False,
            "checkpoint_sampling": {"version": CHECKPOINT_VERSION, "timezone": PACIFIC,
                                    "local_hours": list(CHECKPOINT_HOURS),
                                    "initial": "first_eligible", "missed_slots": "not_backfilled",
                                    "activated_at": store.get(CHECKPOINT_VERSION + ":activated_at"),
                                    "independence_unit": "source_activity_day"},
            "latest": latest, "trials": trials,
            "sample_observations": store.db.execute("SELECT COUNT(*) FROM preference_samples WHERE received_at<=?", (now,)).fetchone()[0],
            "frozen_days": len(day_keys(trials)), "frozen_trials": len(trials),
            "scored_days": len(day_keys(scored)), "scored_trials": len(scored),
            "sampling_groups": [{"mode": mode, "local_hour": hour,
                                 "frozen_days": len(day_keys(group)), "frozen_trials": len(group),
                                 "scored_days": len(day_keys([t for t in group if t in scored]))}
                                for mode, hour in [("first_eligible", None)]+[("fixed_checkpoint", h) for h in CHECKPOINT_HOURS]
                                if (group := [t for t in trials if t["sampling"]["mode"] == mode
                                              and t["sampling"].get("local_hour") == hour])]}
