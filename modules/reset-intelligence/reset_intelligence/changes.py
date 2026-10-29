"""Exact additive explanations between two actually issued model snapshots."""
from __future__ import annotations

from .contracts import iso
from .identity import evaluation_identity


def previous_forecast(store, current):
    now = current["issued_at"]
    # Prefer a six-hour comparison, then the oldest observation in this model
    # revision. Never fabricate a pre-launch value or mix implementation versions.
    rows = store.db.execute("""SELECT payload FROM forecasts WHERE target=? AND
        issued_at BETWEEN ? AND ? ORDER BY ABS(issued_at-?)""",
        (current["target"], now - 6.5 * 3600, now - 60, now - 6 * 3600))
    import json
    for row in rows:
        value = json.loads(row[0])
        if value["status"] == "current" and evaluation_identity(value) == evaluation_identity(current):
            return value
    return None


def terms(forecast, hours):
    if forecast.get("combination_method") == "baseline_plus_revisions":
        p = forecast["curve"][hours-1]
        return {name: {"probability": p[key], "weight": 1., "raw": None, "origin": None}
                for name,key in (("survival_baseline","baseline"), ("radar_revision","radar_adjustment"),
                                 ("community","community_adjustment"))}
    training = forecast.get("training", {})
    result = {}
    for group in training.get("within_groups", []):
        if group["family"] not in ("history", "radars", "baseline"):
            continue
        for name, weight in group["weights"].items():
            component = next(c for c in forecast["components"] if c["id"] == name)
            if component.get("private"):
                continue
            result[name] = {"probability": component["probabilities"][hours - 1],
                            "weight": weight * training["weights"][group["family"]] * group.get("group_weight", 1.),
                            "raw": dict(component.get("source_points", [])).get(hours),
                            "origin": component.get("metadata", {}).get("origin")}
    if "community" in training.get("weights", {}):
        result["community"] = {"probability": forecast["features"][str(hours)]["community"],
                               "weight": training["weights"]["community"], "raw": None, "origin": None}
    return result


def explain_change(current, previous):
    if current["status"] != "current" or previous is None:
        return None
    if (evaluation_identity(current), current["target"]) != (
            evaluation_identity(previous), previous["target"]):
        return None
    if not 60 <= current["issued_at"] - previous["issued_at"] <= 6.5 * 3600:
        return None
    windows = []
    for hours in (24, 48):
        before, after = terms(previous, hours), terms(current, hours)
        drivers = []
        for name in sorted(set(before) | set(after)):
            a, b = before.get(name), after.get(name)
            # A source entering/leaving the pool changes allocation. Do not
            # manufacture a 0% source forecast when it is unavailable.
            p0, p1 = (a or b)["probability"], (b or a)["probability"]
            w0, w1 = a["weight"] if a else 0., b["weight"] if b else 0.
            input_effect = (w0 + w1) / 2 * (p1 - p0)
            weight_effect = (p0 + p1) / 2 * (w1 - w0)
            drivers.append({"id": name, "before": a["probability"] if a else None,
                            "after": b["probability"] if b else None, "weight_before": w0, "weight_after": w1,
                            "raw_before": a["raw"] if a else None, "raw_after": b["raw"] if b else None,
                            "input_effect": input_effect, "weight_effect": weight_effect,
                            "effect": input_effect + weight_effect})
        p0 = previous["curve"][hours - 1]["probability"]
        p1 = current["curve"][hours - 1]["probability"]
        # The last projection enforces automatic <= any relief. Its change is
        # an explicit residual, not misattributed to a source or a statement.
        projection = p1 - p0 - sum(d["effect"] for d in drivers)
        windows.append({"hours": hours, "before": p0, "after": p1, "delta": p1 - p0,
                        "drivers": sorted(drivers, key=lambda d: -abs(d["effect"])),
                        "coherence_effect": projection})
    known = {f["id"] for f in previous.get("facts", [])}
    fresh = {f["id"]: f for f in current.get("facts", []) if f.get("id") not in known
             and f.get("relevance") in ("reset_related", "conditional_pledge")
             and current["issued_at"] - 7 * 86400 <= f["at"] <= current["issued_at"]}
    return {"since": iso(previous["issued_at"]), "until": iso(current["issued_at"]),
            "basis": "increment_terms" if current.get("combination_method") == "baseline_plus_revisions" else "weighted_probabilities",
            "windows": windows, "new_statement_ids": [f["id"] for f in sorted(fresh.values(), key=lambda f: -f["at"])[:5]]}
