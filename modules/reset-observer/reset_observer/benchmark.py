"""Freeze prequential trials on an hourly UTC clock; settle only mature windows."""
from __future__ import annotations

from dataclasses import asdict
import math

from .contracts import AUTOMATIC, HORIZONS
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
    latest = max((e["to"] for e in events if e["status"] == "confirmed" and matching(e, forecast.target)
                  and e["to"] <= origin), default=None)
    if latest is not None:
        if latest > forecast.origin:
            return None, "event_since_forecast"
        last_reset = forecast.metadata.get("last_reset_at")
        if last_reset is not None and last_reset < latest - policy.completion_time_tolerance_seconds:
            return None, "source_missed_known_event"
    p = forecast.align(origin, hours)
    return (p, None) if p is not None else (None, "outside_original_support")


def freeze_grid(store, now, policy):
    initialize(store, now, policy)
    grid = policy.grid_seconds
    origin = store.get("last_grid") + grid
    count = 0
    while origin <= now:
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
        origin += grid
    return count


def label_window(trial, events, now, policy, covered):
    end = trial["origin"] + trial["hours"] * 3600
    result = {"label": None, "status": "pending", "events": [], "reason": "window_not_mature",
              "deadline": end, "matures_at": end + policy.reporting_grace_seconds}
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
    if not certain and uncertain:
        result.update(status="unresolved", reason="boundary_or_evidence_conflict")
    elif not covered:
        result.update(label=1 if certain else None, status="unresolved", reason="incomplete_outcome_coverage")
    else:
        result.update(label=int(bool(certain)), status="settled", reason="completion_observed" if certain else "monitored_without_completion")
    if trial["p"] is None:
        result["score_eligible"] = False
    else:
        result["score_eligible"] = result["status"] == "settled"
    return result


def settle_all(store, now, policy, evidence):
    truth_hash = store.truth(now, evidence)
    changes = 0
    cached_coverage = {}
    for trial in store.trials(now):
        key = trial["origin"], trial["hours"]
        end = trial["origin"] + trial["hours"] * 3600
        if key not in cached_coverage:
            cached_coverage[key] = now >= end + policy.reporting_grace_seconds and coverage(store, trial["origin"], end, now, policy)
        result = label_window(trial, evidence["events"], now, policy, cached_coverage[key])
        result["truth_hash"] = truth_hash
        changes += store.settle(trial["id"], now, result)
    return changes
