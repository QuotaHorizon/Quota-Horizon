"""Proper scores, paired time-block inference, calibration and fixed alerts."""
from __future__ import annotations

import math
import numpy as np


def block_interval(values, origins, block_days=7, draws=2000):
    """Cluster bootstrap of UTC calendar blocks. No iid-minute confidence claims."""
    values, origins = np.asarray(values, dtype=float), np.asarray(origins)
    if len(values) == 0:
        return {"mean": None, "ci95": None, "blocks": 0}
    keys = (origins // (block_days * 86400)).astype(int)
    clusters = [values[keys == key] for key in np.unique(keys)]
    result = {"mean": float(values.mean()), "ci95": None, "blocks": len(clusters)}
    if len(clusters) < 8:
        return result
    rng = np.random.default_rng(74219)
    sums = np.array([c.sum() for c in clusters])
    counts = np.array([len(c) for c in clusters])
    indices = rng.integers(0, len(clusters), (draws, len(clusters)))
    means = sums[indices].sum(axis=1) / counts[indices].sum(axis=1)
    result["ci95"] = np.quantile(means, [.025, .975]).tolist()
    return result


def wilson(successes, total):
    if not total:
        return None
    p, z = successes / total, 1.959963984540054
    center = (p + z * z / (2 * total)) / (1 + z * z / total)
    half = z * math.sqrt(p * (1 - p) / total + z * z / (4 * total * total)) / (1 + z * z / total)
    return [max(0., center - half), min(1., center + half)]


def scores(rows, policy):
    ps, ys = np.array([r["p"] for r in rows]), np.array([r["label"] for r in rows])
    n = len(rows)
    if not n:
        return {"n": 0, "positive_windows": 0, "unique_events": 0, "brier": None, "log_loss": None,
                "brier_interval": {"mean": None, "ci95": None, "blocks": 0}, "calibration": [], "state": "awaiting_mature_windows"}
    eps = policy.probability_epsilon
    clipped = np.clip(ps, eps, 1 - eps)
    loss = -ys * np.log(clipped) - (1 - ys) * np.log1p(-clipped)
    brier = (ps - ys) ** 2
    events = {event for row in rows for event in row.get("events", []) if row["label"] == 1}
    calibration = []
    for low in np.arange(0., 1., .2):
        idx = (ps >= low - 1e-12) & ((ps < low + .2 - 1e-12) if low < .79 else (ps <= 1))
        size = int(idx.sum())
        if size:
            bin_rows = [row for row, ok in zip(rows, idx) if ok]
            calibration.append({"from": round(float(low), 1), "to": round(float(low + .2), 1), "n": size,
                                "mean_probability": float(ps[idx].mean()), "event_frequency": float(ys[idx].mean()),
                                "frequency_wilson95_descriptive": wilson(int(ys[idx].sum()), size),
                                "calibration_gap_block": block_interval(ps[idx] - ys[idx], [r["origin"] for r in bin_rows], policy.bootstrap_block_days)})
    enough = n >= policy.minimum_rank_windows and len(events) >= policy.minimum_rank_events
    return {"n": n, "positive_windows": int(ys.sum()), "unique_events": len(events),
            "brier": float(brier.mean()), "log_loss": float(loss.mean()),
            "clipped_predictions": int(((ps < eps) | (ps > 1 - eps)).sum()),
            "brier_interval": block_interval(brier, [r["origin"] for r in rows], policy.bootstrap_block_days),
            "calibration": calibration, "state": "accumulating_evidence" if enough else "insufficient_events_or_windows"}


def paired(left, right, policy):
    """Negative loss difference favors left. Pair absolute windows and labels."""
    index = {(r["origin"], r["hours"], r["target"]): r for r in right}
    pairs = [(a, index[key]) for a in left if (key := (a["origin"], a["hours"], a["target"])) in index
             and a["label"] == index[key]["label"]]
    if not pairs:
        return {"n": 0, "brier_skill": None, "loss_difference": block_interval([], [], policy.bootstrap_block_days),
                "error_correlation": None, "interpretation": "no_common_mature_windows"}
    a = np.array([r[0]["p"] - r[0]["label"] for r in pairs])
    b = np.array([r[1]["p"] - r[1]["label"] for r in pairs])
    interval = block_interval(a * a - b * b, [r[0]["origin"] for r in pairs], policy.bootstrap_block_days)
    correlation = float(np.corrcoef(a, b)[0, 1]) if len(pairs) >= 3 and a.std() > 1e-12 and b.std() > 1e-12 else None
    unique_events = len({event for r, _ in pairs for event in r.get("events", []) if r["label"] == 1})
    state = "insufficient_evidence"
    if len(pairs) >= policy.minimum_rank_windows and unique_events >= policy.minimum_rank_events and interval["ci95"]:
        state = "left_lower_loss" if interval["ci95"][1] < 0 else "right_lower_loss" if interval["ci95"][0] > 0 else "inconclusive"
    return {"n": len(pairs), "unique_events": unique_events, "brier_skill": float(1 - np.mean(a*a) / np.mean(b*b)) if np.mean(b*b) > 0 else None,
            "loss_difference": interval, "error_correlation": correlation, "interpretation": state,
            "inference": "exploratory_pairwise_no_multiple_comparison_adjustment"}


def alert_metrics(rows, events, policy):
    """Predeclared threshold; alerts merge into episodes until their window ends."""
    ordered = sorted(rows, key=lambda r: r["origin"])
    episodes = []
    for row in ordered:
        if row["p"] < policy.alert_threshold or (episodes and row["origin"] < episodes[-1]["deadline"]):
            continue
        episodes.append({"origin": row["origin"], "deadline": row["origin"] + row["hours"] * 3600,
                         "hit": bool(row["label"]), "events": row.get("events", [])})
    event_index = {e["id"]: e for e in events if e["status"] == "confirmed"}
    detected = {}
    for episode in episodes:
        for event in episode["events"] if episode["hit"] else []:
            if event in event_index:
                detected.setdefault(event, max(0., (event_index[event]["from"] - episode["origin"]) / 3600))
    observable = {event for row in rows for event in row.get("events", []) if row["label"] == 1}
    return {"threshold": policy.alert_threshold, "episodes": len(episodes),
            "false_alerts": sum(not e["hit"] for e in episodes),
            "precision": sum(e["hit"] for e in episodes) / len(episodes) if episodes else None,
            "event_recall": len(detected) / len(observable) if observable else None,
            "detected_events": len(detected), "observable_events": len(observable),
            "median_lead_hours": float(np.median(list(detected.values()))) if detected else None,
            "event_leads": detected}
