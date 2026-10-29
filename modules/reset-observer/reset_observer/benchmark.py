"""Freeze prequential trials on an hourly UTC clock; settle only mature windows."""
from __future__ import annotations

from dataclasses import asdict
import json
import math
import time

from . import SETTLEMENT_VERSION
from .contracts import AUTOMATIC, HORIZONS, Forecast
from .evidence import coverage, reconcile


def initialize(store, now, policy):
    saved = store.get("policy")
    if saved and saved["hash"] != policy.hash:
        raise ValueError("policy changed: use a separate evaluation database")
    if not saved:
        store.put("policy", {"hash": policy.hash, **asdict(policy)})
        store.put("started_at", now)
        store.put("last_grid", math.floor(now / policy.grid_seconds) * policy.grid_seconds)


def matching(event, target):
    return target != AUTOMATIC or event["kind"] in ("automatic", "unknown")


def cycle_exclusion(forecast, origin, events, policy):
    # A source's explicit completed-reset observation closes no-event
    # conditioning promptly. Formal outcome settlement still needs confirmation.
    latest = max((e["to"] for e in events if matching(e, forecast.target)
                  and e.get("known_at", origin) <= origin and e.get("to") is not None
                  and e["to"] <= origin and (e["status"] == "confirmed" or (
                      e["status"] == "pending" and e["reason"] == "insufficient_primary_confirmation"
                      and any(c["provider"] == "codex_reset_status" and c["stage"] == "completed"
                              for c in e.get("claims", []))))), default=None)
    if latest is not None:
        if latest > forecast.origin:
            return "event_since_forecast"
        last_reset = forecast.metadata.get("last_reset_at")
        if last_reset is not None and last_reset < latest - policy.completion_time_tolerance_seconds:
            return "source_missed_known_event"
    return None


def trial_probability(forecast, origin, hours, events, policy):
    if forecast.issues:
        return None, "source_quality_exclusion"
    if forecast.available_at > origin:
        return None, "not_received_at_origin"
    if forecast.origin > origin:
        return None, "forecast_not_started"
    if forecast.expires_at <= origin:
        return None, "stale_forecast"
    # Check information actually known at the trial origin, never a later label.
    reason = cycle_exclusion(forecast, origin, events, policy)
    if reason:
        return None, reason
    p = forecast.align(origin, hours)
    return (p, None) if p is not None else (None, "outside_original_support")


def freeze_grid(store, now, policy, *, maximum_hours=None, maximum_seconds=None,
                should_stop=None, progress=None):
    initialize(store, now, policy)
    grid = policy.grid_seconds
    origin = store.get("last_grid") + grid
    count = 0
    completed = 0
    started = time.monotonic()
    while origin <= now:
        if (should_stop and should_stop()) or (
            maximum_hours is not None and completed >= maximum_hours
        ) or (completed and maximum_seconds is not None
              and time.monotonic() - started >= maximum_seconds):
            break
        latest = {}
        for f in store.forecasts(origin, latest=True):
            # Retire an old version when its successor is observed. Its past
            # trials remain separate; it is not charged for future missing rows.
            key = f.source, f.target, f.track
            previous = latest.get(key)
            if previous is None or (f.origin, f.available_at) >= (previous.origin, previous.available_at):
                latest[key] = f
        evidence = reconcile(store, origin, policy)
        for f in latest.values():
            for hours in HORIZONS:
                p, reason = trial_probability(f, origin, hours, evidence["events"], policy)
                store.freeze({"source": f.source, "version": f.version, "target": f.target, "track": f.track,
                              "origin": origin, "hours": hours, "p": p, "frozen_at": now,
                              "policy_hash": policy.hash, "forecast_id": f.id, "private": f.private,
                              "missing_reason": reason, "received_at": f.available_at,
                              "publication_origin": f.origin, "conditioning": "piecewise_constant_hazard_no_tail",
                              "headline": int(origin) % (hours * 3600) == 0})
                count += 1
        # Persist after each complete grid. Retrying a partially written grid is
        # idempotent, and never replaces already frozen probabilities.
        store.put("last_grid", origin)
        completed += 1
        if progress:
            progress(origin)
        origin += grid
    return count


def label_window(trial, events, now, policy, covered):
    end = trial["origin"] + trial["hours"] * 3600
    result = {"label": None, "status": "pending", "events": [], "reason": "window_not_mature",
              "deadline": end, "matures_at": end + policy.reporting_grace_seconds,
              "settlement_version": SETTLEMENT_VERSION, "coverage_complete": bool(covered),
              "score_eligible": False, "event_score_eligible": False}
    if now < result["matures_at"]:
        return result
    certain, uncertain = [], []
    for event in events:
        if not matching(event, trial["target"]) or event["status"] == "targeted" or event["from"] is None:
            continue
        if event["to"] <= trial["origin"] or event["from"] > end:
            continue
        if event["status"] == "confirmed" and event["from"] > trial["origin"] and event["to"] <= end:
            certain.append(event["id"])
        else:
            uncertain.append(event["id"])
    result["events"] = sorted(certain or uncertain)
    if certain:
        # An observed occurrence establishes the label despite a monitoring gap.
        # Keep partial-coverage positives out of calibration/ranking samples;
        # otherwise missing negative windows would select an optimistic sample.
        result.update(label=1, status="settled", reason="completion_observed")
    elif uncertain:
        result.update(status="unresolved", reason="boundary_or_evidence_conflict")
    elif not covered:
        result.update(status="unresolved", reason="incomplete_outcome_coverage")
    else:
        result.update(label=int(bool(certain)), status="settled", reason="completion_observed" if certain else "monitored_without_completion")
    if trial["p"] is None:
        result["score_eligible"] = False
    else:
        result["score_eligible"] = result["status"] == "settled" and bool(covered)
        result["event_score_eligible"] = result["label"] == 1
    return result


def revise_trial_eligibility(store, now, policy, evidence, truth_hash):
    """Append corrections to event exclusions using the original received forecast.

    No probability model is rerun and no frozen row is replaced. The correction
    is visible only from its own knowledge time; earlier as-of reads retain the
    original exclusion and old settlements. Every source/track is treated alike.
    """
    context = store.get("trial_revision_context")
    # Catch-up can append more trials after this truth was already evaluated.
    # Count as well as the last row: a preloaded future trial can become visible
    # below the current maximum rowid. Capture this before processing so a
    # concurrent collector's later rows remain eligible for the next pass.
    watermark = list(store.db.execute(
        "SELECT COUNT(*),COALESCE(MAX(rowid),0) FROM trials WHERE frozen_at<=?",
        (now,),
    ).fetchone())
    revision_from = context.get("revision_from") if isinstance(context, dict) else None
    next_context = {"truth_hash": truth_hash, "trials": watermark, "revision_from": revision_from}
    if context == next_context:
        return 0
    prior = store.trial_revisions(now)
    times = [e["from"] for e in evidence["events"]
             if e.get("reason") == "completion_withdrawn" and e.get("from") is not None]
    # A reclassification can arrive while the first catch-up batch still
    # predates all affected trials. Retain its lower bound even when this pass
    # has no corrections, so later batches can use the same current evidence.
    if revision_from is not None:
        times.append(revision_from)
    old_hash = (context.get("truth_hash") if isinstance(context, dict) else context)
    old_hash = old_hash or (store.get("settlement_context") or {}).get("truth_hash")
    if old_hash:
        previous = store.db.execute("SELECT payload FROM truth_revisions WHERE hash=?", (old_hash,)).fetchone()
        if previous:
            for old in json.loads(previous[0])["events"]:
                if old.get("from") is None:
                    continue
                matches = [e for e in evidence["events"] if set(e["references"]) & set(old["references"])]
                if not matches or any(tuple(e.get(k) for k in ("status", "reason", "from", "to")) !=
                                      tuple(old.get(k) for k in ("status", "reason", "from", "to")) for e in matches):
                    times.append(old["from"])
    if prior:
        row = store.db.execute("""SELECT MIN(t.origin) FROM trials t JOIN
          (SELECT DISTINCT trial FROM trial_revisions WHERE at<=?) r ON r.trial=t.id""", (now,)).fetchone()
        if row[0] is not None:
            times.append(row[0])
    next_context["revision_from"] = min(times) if times else None
    if not times:
        store.put("trial_revision_context", next_context)
        return 0
    rows = store.db.execute("""SELECT id,payload FROM trials WHERE frozen_at<=? AND origin>=? AND rowid<=?
      AND (p IS NULL OR id IN (SELECT trial FROM trial_revisions WHERE at<=?)) ORDER BY origin,id""",
      (now, min(times), watermark[1], now)).fetchall()
    # Finish the read cursor before appending revisions. A concurrent collector
    # or maintenance batch can commit while probabilities are reconstructed;
    # upgrading its older WAL snapshot would otherwise fail with SQLITE_BUSY_SNAPSHOT.
    history, forecasts, count = {}, {}, 0
    with store.db:
        for row in rows:
            original = json.loads(row["payload"])
            if original.get("missing_reason") not in ("source_missed_known_event", "event_since_forecast"):
                continue
            identifier = original.get("forecast_id")
            if not identifier:
                continue
            if identifier not in forecasts:
                saved = store.db.execute("SELECT payload FROM forecasts WHERE id=? AND available_at<=?", (identifier, original["origin"])).fetchone()
                forecasts[identifier] = Forecast(**json.loads(saved[0])) if saved else None
            forecast = forecasts[identifier]
            if forecast is None:
                continue
            origin = original["origin"]
            if origin not in history:
                history[origin] = reconcile(store, origin, policy)["events"]
            corrected = []
            for event in evidence["events"]:
                # Reinstatement of the same previously known event must also
                # reinstate its exclusion. A newly discovered unrelated event
                # must never be backdated into the original information set.
                old = [e for e in history[origin] if set(e["references"]) & set(event["references"])
                       and (e["status"] == "confirmed" or (e["status"] == "pending"
                            and e.get("reason") == "insufficient_primary_confirmation"
                            and any(c["provider"] == "codex_reset_status" and c["stage"] == "completed"
                                    for c in e.get("claims", []))))]
                known = min([event.get("known_at", now)] + [e.get("known_at", origin) for e in old])
                corrected.append({**event, "known_at": known})
            p, reason = trial_probability(forecast, origin, original["hours"], corrected, policy)
            current = prior.get(row["id"], original)
            if p == current.get("p") and reason == current.get("missing_reason"):
                continue
            revision = {"p": p, "missing_reason": reason, "eligibility_revision": {
                "at": now, "reason": "completion_evidence_revision", "truth_hash": truth_hash,
                "forecast_id": identifier, "method": "original_received_forecast_with_revised_evidence"}}
            count += store.revise_trial(row["id"], now, revision)
    store.put("trial_revision_context", next_context)
    return count


def settle_all(store, now, policy, evidence):
    truth_hash = store.truth(now, evidence)
    revised = revise_trial_eligibility(store, now, policy, evidence, truth_hash)
    trials = store.trials(now, compact=True)
    previous = store.settled(now, compact=True)
    context = {"truth_hash": truth_hash, "settlement_version": SETTLEMENT_VERSION}
    unchanged_evidence = store.get("settlement_context") == context and not revised
    changes = 0
    cached_coverage = {}
    with store.db:
        for trial in trials:
            old = previous.get(trial["id"])
            if unchanged_evidence and old and old.get("settlement_version") == SETTLEMENT_VERSION:
                if old["status"] != "pending" or now < old["matures_at"]:
                    continue
            key = trial["origin"], trial["hours"]
            end = trial["origin"] + trial["hours"] * 3600
            if key not in cached_coverage:
                cached_coverage[key] = now >= end + policy.reporting_grace_seconds and coverage(store, trial["origin"], end, now, policy)
            result = label_window(trial, evidence["events"], now, policy, cached_coverage[key])
            result["truth_hash"] = truth_hash
            if trial.get("eligibility_revision"):
                result["eligibility_revision"] = trial["eligibility_revision"]
            changes += store.settle(trial["id"], now, result, commit=False)
    store.put("settlement_context", context)
    return changes
