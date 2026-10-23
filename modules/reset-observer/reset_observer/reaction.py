"""Observed event-to-forecast response clocks, independent of probability scores.

Only explicit cycle anchors establish a response. A falling percentage alone
does not establish that a source noticed a reset. UI receipt is measured by the
desktop, so the evaluator leaves those clocks empty.
"""
from __future__ import annotations

from collections import defaultdict
import json
import math

from .contracts import AUTOMATIC, Forecast, packed, url_key
from .evidence import SOURCE_FAMILIES

VERSION = "reset-reaction-1"
HISTORY_SECONDS = 7 * 86400
MAX_EVENTS = 8


def _number(value):
    return float(value) if isinstance(value, (float, int)) and not isinstance(value, bool) and math.isfinite(value) else None


def record_forecast(store, forecast, source_received_at=None, history=False):
    if forecast.private or forecast.track not in ("native", "model"):
        return
    metadata = forecast.metadata
    anchor = metadata.get("reset_anchor") or metadata.get("cycle") or {}
    if not isinstance(anchor, dict):
        anchor = {}
    payload = {
        "source": forecast.source, "version": forecast.version, "target": forecast.target,
        "track": forecast.track, "origin": forecast.origin, "generated_at": forecast.generated_at,
        "source_received_at": _number(source_received_at), "available_at": forecast.available_at,
        "expires_at": forecast.expires_at, "last_reset_at": _number(metadata.get("last_reset_at")),
        "anchor": anchor, "issues": forecast.issues, "p24": forecast.cdf(24), "p48": forecast.cdf(48),
        "capture_basis": "existing_frozen_receipt" if history else "prospective_import",
    }
    store.db.execute("INSERT OR IGNORE INTO reaction_forecasts VALUES (?,?,?,?,?,?)",
                     (forecast.id, forecast.source, forecast.target, forecast.track,
                      forecast.available_at, packed(payload)))


def initialize(store, now):
    """Build a compact index once from existing receipts, without backdating them."""
    if store.get(VERSION):
        return
    start = now - HISTORY_SECONDS
    with store.db:
        rows = store.db.execute("""SELECT source,target,track,available_at,
            json_extract(qh_json(payload),'$.version','$.origin','$.generated_at','$.expires_at',
                         '$.private','$.issues','$.metadata','$.points') fields
            FROM forecasts WHERE available_at BETWEEN ? AND ? AND track IN ('native','model')
            ORDER BY available_at,origin,id""", (start, now))
        for row in rows:
            version, origin, generated, expires, private, issues, metadata, points = json.loads(row["fields"])
            forecast = Forecast(source=row["source"], target=row["target"], track=row["track"],
                available_at=row["available_at"], version=version, origin=origin, generated_at=generated,
                expires_at=expires, private=bool(private), issues=issues or [], metadata=metadata or {}, points=points)
            record_forecast(store, forecast, history=True)
    store.put(VERSION, {"initialized_at": now, "history_since": start})


def _anchor_time(row):
    anchor = row.get("anchor") or {}
    # Legacy last_reset_at mixed occurrence, announcement and discovery clocks.
    # A source name or later adapter fix cannot retroactively establish which
    # clock a frozen record meant.
    if anchor.get("time_basis") not in ("observed", "source_clock", "observed_reset"):
        return None
    return _number(anchor.get("occurred_at"))


def _reference_keys(values):
    keys = set()
    for value in values:
        try:
            keys.add(url_key(value))
        except (TypeError, ValueError, AttributeError):
            continue
    return keys


def _matches(row, event, tolerance):
    at = _anchor_time(row)
    if at is not None:
        return event["from"] - tolerance <= at <= event["to"] + tolerance
    anchor = row.get("anchor") or {}
    if anchor.get("time_basis") == "completion_confirmation":
        keys = _reference_keys(key for claim in event.get("claims", []) for key in claim["keys"])
        return bool(keys.intersection(_reference_keys(anchor.get("references", []))))
    return False


def _first_received(store, claim, now):
    # Announcements earlier in the same record are not completion observations.
    row = store.db.execute("""SELECT observed_at FROM claims WHERE provider=? AND record=?
        AND observed_at<=? AND json_extract(qh_json(payload),'$.stage')='completed'
        ORDER BY observed_at,id LIMIT 1""", (claim["provider"], claim["record"], now)).fetchone()
    return row[0] if row else claim["observed_at"]


def _series(rows, event, first_known, now, tolerance):
    rows = sorted(rows, key=lambda row: (row["available_at"], row["generated_at"], row["origin"]))
    before = [row for row in rows if row["available_at"] <= event["to"] and not row["issues"]]
    previous = before[-1] if before else None
    after = [row for row in rows if row["available_at"] >= event["from"] and not row["issues"]]
    updated = next((row for row in after if _matches(row, event, tolerance)), None)
    old = [row for row in after if _anchor_time(row) is not None and _anchor_time(row) < event["from"] - tolerance]
    identity = updated or rows[-1]
    published = updated["generated_at"] if updated else None
    # Existing archives know the observer receipt, but not always the producer's
    # earlier HTTP receipt. Preserve that missing stage instead of guessing it.
    captured = updated["source_received_at"] if updated else None
    prior_known = (previous is not None and _anchor_time(previous) is not None
                   and previous["expires_at"] >= event["from"])
    transition_known = prior_known and updated is not None and all(
        _anchor_time(row) is not None or _matches(row, event, tolerance)
        for row in after if row["available_at"] <= updated["available_at"])
    # A newly instrumented record can prove the current cycle, but it cannot
    # date an earlier response through legacy records with unknown anchors.
    # Measuring from the event to deployment would manufacture a long delay.
    residual = None
    if prior_known and _anchor_time(previous) < event["from"] - tolerance:
        if transition_known:
            residual = max(0., published - event["to"])
        elif updated is None and old and old[-1]["expires_at"] > now and all(_anchor_time(row) is not None for row in after):
            residual = max(0., now - event["to"])
    state = ("updated" if updated else "awaiting_update" if old else
             "anchor_unknown" if any(_anchor_time(row) is None for row in after) else "no_prior_observation")
    return {
        "source": identity["source"], "version": identity["version"], "target": identity["target"], "track": identity["track"],
        "state": state, "published_at": published, "source_received_at": captured,
        "observer_received_at": updated["available_at"] if updated else None,
        "publication_to_capture_seconds": captured - published if captured is not None and published is not None and captured >= published else None,
        "completion_to_update_seconds": max(0., published - event["to"]) if transition_known else None,
        "known_to_update_seconds": max(0., published - first_known) if transition_known and first_known is not None else None,
        "old_cycle_residual_seconds": residual, "residual_censored": residual is not None and updated is None,
        "before_probability_24h": previous["p24"] if previous else None,
        "after_probability_24h": updated["p24"] if updated else None,
        "ui_received_at": None, "ui_presented_at": None, "ui_delay_seconds": None,
        "missing_fields": (["source_received_at"] if captured is None else [])
                          + ([] if transition_known else ["historical_cycle_transition"]) + ["ui_received_at"],
    }


def report(store, now, evidence, policy):
    initialized = store.get(VERSION) or {}
    result = {"version": VERSION, "generated_at": now, **initialized, "events": []}
    exists = store.db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='reaction_forecasts'").fetchone()
    if not exists:
        return result
    events = [event for event in evidence["events"] if event.get("from") is not None
              and event.get("to") is not None and event["to"] <= now
              and event["to"] >= now - HISTORY_SECONDS
              and (event["status"] == "confirmed" or (event["status"] == "pending"
                   and event.get("reason") == "insufficient_primary_confirmation"))
              and any(claim["stage"] == "completed" for claim in event.get("claims", []))][-MAX_EVENTS:]
    if not events:
        return result
    groups = defaultdict(list)
    for row in store.db.execute("SELECT payload FROM reaction_forecasts WHERE available_at BETWEEN ? AND ? ORDER BY available_at,id",
                               (min(event["from"] for event in events)-86400, now)):
        value = json.loads(row[0])
        groups[(value["source"], value["target"], value["track"])].append(value)
    for event in reversed(events):
        captures = []
        for claim in event["claims"]:
            if claim["stage"] != "completed":
                continue
            received = _first_received(store, claim, now)
            published = _number(claim.get("published_at"))
            if published is None and claim["authority"] == "official_relay" and claim["completed_at"] == claim["announced_at"]:
                published = claim["announced_at"]
            captures.append({"provider": claim["provider"], "source_family": SOURCE_FAMILIES.get(claim["provider"], claim["provider"]),
                "published_at": published, "first_received_at": received,
                "published_to_capture_seconds": received-published if published is not None and received >= published else None})
        first_known = min((capture["first_received_at"] for capture in captures), default=None)
        series = []
        for (source, target, track), rows in sorted(groups.items()):
            if target == AUTOMATIC and event["kind"] not in ("automatic", "unknown"):
                continue
            bounded = [row for row in rows if event["from"]-86400 <= row["available_at"] <= min(now, event["to"]+2*86400)]
            if bounded:
                series.append(_series(bounded, event, first_known, now, policy.completion_time_tolerance_seconds))
        result["events"].append({"id": event["id"], "kind": event["kind"], "status": event["status"],
            "time_basis": event["time_basis"], "completed_from": event["from"], "completed_to": event["to"],
            "first_received_at": first_known, "evidence": captures, "series": series[:16]})
    return result
