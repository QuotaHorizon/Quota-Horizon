"""Explain evidence coverage without changing the frozen settlement policy."""
from __future__ import annotations

from collections import Counter, defaultdict
import json
import math

from .contracts import digest, iso
from .evidence import FEEDS


def coverage_details(store, start, end, known_by, policy):
    providers = []
    gap = policy.maximum_coverage_gap_seconds
    for provider in FEEDS:
        rows = [dict(r) for r in store.db.execute(
            "SELECT observed_at,generated_at,healthy,error FROM observations WHERE provider=? AND observed_at BETWEEN ? AND ? ORDER BY observed_at,id",
            (provider, start - gap, min(end + gap, known_by)))]
        good = [r["observed_at"] for r in rows if r["healthy"]]
        left = max((t for t in good if t <= start), default=None)
        right = min((t for t in good if t >= end), default=None)
        reasons, spans = [], []
        if left is None:
            reasons.append("missing_start_evidence")
        if right is None:
            reasons.append("missing_end_evidence")
        # Ignore observations beyond the closest boundaries of this window.
        bounded = [t for t in good if (left if left is not None else start) <= t <= (right if right is not None else end)]
        health_bounds = ([start] if left is None else []) + bounded + ([end] if right is None else [])
        for a, b in zip(health_bounds, health_bounds[1:]):
            if b - a > gap:
                spans.append({"from": iso(a), "to": iso(b), "seconds": b - a, "kind": "healthy_evidence_gap"})
        if spans:
            reasons.append("healthy_evidence_gap")
        # Keep absence of attempts separate from requests that failed or read
        # stale content. Include a gap reaching a missing window boundary.
        attempts = [start, *[r["observed_at"] for r in rows if start < r["observed_at"] < end], end]
        for a, b in zip(attempts, attempts[1:]):
            if b - a > gap:
                spans.append({"from": iso(a), "to": iso(b), "seconds": b - a, "kind": "collection_gap"})
                if "collection_gap" not in reasons:
                    reasons.append("collection_gap")
        failed = [r for r in rows if start <= r["observed_at"] <= end and not r["healthy"]]
        counts = Counter("request_or_decode_failure" if r["error"] else
                         "stale_source_content" if r["generated_at"] is not None and r["observed_at"] - r["generated_at"] > 3600 else
                         "incomplete_source_evidence" for r in failed)
        # A failed request between timely healthy observations does not erase
        # that coverage. Preserve failures as diagnostics, not a second gate.
        qualifies = not reasons and end <= known_by
        reasons.extend(sorted(counts))
        providers.append({"provider": provider, "qualifies": qualifies,
                          "reasons": reasons, "gaps": spans, "failed_attempts": dict(counts),
                          "first_failure": iso(failed[0]["observed_at"]) if failed else None,
                          "last_failure": iso(failed[-1]["observed_at"]) if failed else None})
    healthy = sum(p["qualifies"] for p in providers)
    return {"covered": end <= known_by and healthy >= policy.minimum_coverage_providers,
            "qualified_providers": healthy, "required_providers": policy.minimum_coverage_providers,
            "providers": providers}


def provider_health(store, now, policy):
    output = []
    for row in store.observations(now):
        previous = store.db.execute("SELECT claims FROM observations WHERE provider=? AND observed_at<? AND claims IS NOT NULL ORDER BY observed_at DESC,id DESC LIMIT 1",
                                    (row["provider"], row["observed_at"])).fetchone()
        def fingerprint(value):
            return digest([{k: v for k, v in c.items() if k != "observed_at"} for c in json.loads(value)]) if value else None
        unchanged = bool(previous and row["claims"] is not None and fingerprint(previous[0]) == fingerprint(row["claims"]))
        state = "healthy"
        if now - row["observed_at"] > policy.maximum_coverage_gap_seconds:
            state = "collector_overdue"
        elif row["error"]:
            state = "request_or_decode_failure"
        elif not row["healthy"]:
            state = "stale_source_content" if row["generated_at"] is not None and row["observed_at"] - row["generated_at"] > 3600 else "incomplete_source_evidence"
        output.append({"provider": row["provider"], "last_attempt": iso(row["observed_at"]),
                       "generated_at": iso(row["generated_at"]) if row["generated_at"] is not None else None,
                       "healthy": bool(row["healthy"]), "error": row["error"], "state": state,
                       "content_unchanged": unchanged})
    return output


def runtime_gap(previous_wall, previous_mono, wall, mono):
    elapsed, active = wall - previous_wall, mono - previous_mono
    if elapsed <= 180:
        return None
    return {"kind": "suspend_or_clock_jump" if elapsed - active > 60 else "loop_delay",
            "from": previous_wall, "to": wall, "wall_seconds": elapsed, "monotonic_seconds": active}


def window_diagnostics(store, now, policy, trials, settlements):
    grouped = defaultdict(list)
    for trial in trials:
        grouped[(trial["target"], trial["origin"], trial["hours"])].append(trial)
    windows, cache = [], {}
    for (target, origin, hours), members in sorted(grouped.items(), key=lambda x: (x[0][1], x[0][2], x[0][0]), reverse=True):
        label = settlements.get(members[0]["id"], {"status": "pending", "reason": "window_not_mature", "label": None})
        end = origin + hours * 3600
        detail = None
        if label["status"] == "unresolved" and len(windows) < 200:
            key = (origin, hours)
            if key not in cache:
                cache[key] = coverage_details(store, origin, end, now, policy)
            detail = cache[key]
        action = "await_maturity" if label["status"] == "pending" else "complete"
        repairable = None
        if label["reason"] == "incomplete_outcome_coverage":
            # Later current snapshots cannot establish past negative coverage.
            action, repairable = "retain_unscored_historical_gap", False
        elif label["status"] == "unresolved":
            action, repairable = "await_primary_evidence_revision", True
        windows.append({"target": target, "origin": iso(origin), "hours": hours, "deadline": iso(end),
                        "matures_at": iso(end + policy.reporting_grace_seconds), "status": label["status"],
                        "reason": label["reason"], "known_label": label.get("label"), "forecasts": len(members),
                        "missing_forecasts": sum(t["p"] is None for t in members), "action": action,
                        "repairable_from_later_evidence": repairable, "coverage": detail})
    runtime = store.runtime_events(now)
    return {"unique_windows": len(windows), "states": dict(Counter(w["status"] for w in windows)),
            "unresolved_reasons": dict(Counter(w["reason"] for w in windows if w["status"] == "unresolved")),
            "windows": windows[:200], "omitted_windows": max(0, len(windows) - 200),
            "runtime_events": runtime[-30:], "historical_gap_cause": "unknown_without_runtime_telemetry"}


def dependence(trials, safe):
    """Descriptive same-clock correlations; neither independence nor skill proof."""
    groups = defaultdict(dict)
    for t in trials:
        if t["hours"] != 24 or t["track"] not in ("native", "model", "community") or t["p"] is None:
            continue
        groups[(t["source"], t["version"], t["target"], t["track"])][t["origin"]] = t["p"]
    def correlation(a, b):
        am, bm = sum(a) / len(a), sum(b) / len(b)
        av, bv = [x - am for x in a], [x - bm for x in b]
        scale = math.sqrt(sum(x*x for x in av) * sum(x*x for x in bv))
        return sum(x*y for x, y in zip(av, bv)) / scale if scale > 1e-12 else None
    output = []
    keys = sorted(groups)
    for i, left in enumerate(keys):
        for right in keys[i+1:]:
            if left[2] != right[2] or left[0] == right[0]:
                continue
            common = sorted(groups[left].keys() & groups[right].keys())
            if len(common) < 8:
                continue
            changes = [(a, b) for a, b in zip(common, common[1:]) if b - a == 3600]
            output.append({"left": safe(left[0]), "left_version": left[1], "right": safe(right[0]), "right_version": right[1],
                           "target": left[2], "hours": 24, "matched_origins": len(common),
                           "level_correlation": correlation([groups[left][t] for t in common], [groups[right][t] for t in common]),
                           "consecutive_changes": len(changes),
                           "change_correlation": correlation([groups[left][b]-groups[left][a] for a,b in changes],
                                                             [groups[right][b]-groups[right][a] for a,b in changes]) if len(changes) >= 8 else None,
                           "interpretation": "descriptive_dependence_not_predictive_skill"})
    return output
