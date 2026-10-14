"""Chronological evaluation: retrospective catalogues and prospective scores stay distinct."""
from __future__ import annotations

from . import MODEL_VERSION
from .contracts import Event
from .quant import ModelConfig, Survival, metrics, paired_block_interval


def canonical_events(observation, target: str, now: float) -> list[Event]:
    if observation is None:
        return []
    # One primary catalogue defines outcomes; corroborating sites are not extra
    # events. Merge aliases by primary reference/id, never by proximity alone.
    groups: list[list[Event]] = []
    for event in sorted(observation.events, key=lambda e: (e.at, e.id)):
        if event.at > now or event.known_at > now or not event.matches(target):
            continue
        matches = [g for g in groups if any(e.id == event.id or set(e.references) & set(event.references) for e in g)]
        if matches:
            merged = [event]
            for group in matches:
                merged.extend(group)
                groups.remove(group)
            groups.append(merged)
        else:
            groups.append([event])
    return sorted((min(g, key=lambda e: (e.at, e.id)) for g in groups), key=lambda e: e.at)


def matured(store, now: float, target: str, hours: int, config_hash: str | None = None) -> list[dict]:
    obs = store.observations(now).get("event_ledger")
    events = canonical_events(obs, target, now)
    out, last_end = [], float("-inf")
    for f in store.forecasts(now, target):
        start, end = f["issued_at"], f["issued_at"] + hours * 3600
        if f["model_version"] != MODEL_VERSION or (config_hash is not None and f["config_hash"] != config_hash):
            continue
        # A reporting grace period and monitored coverage are required for zero
        # labels. Late discoveries can revise scores without rewriting forecasts.
        if end + 3 * 3600 > now or start < last_end or f.get("status") != "current":
            continue
        hit = any(start < e.at <= end for e in events)
        # Apply the SAME coverage gate to positive and negative windows; keeping
        # only identifiable positives during outages would bias every score.
        if not store.coverage("event_ledger", start, end + 3 * 3600, now):
            continue
        point = next((p for p in f["curve"] if p["hours"] == hours), None)
        if point and point["probability"] is not None:
            out.append({"issued_at": start, "hours": hours, "label": int(hit),
                        "p": point["probability"], "baseline": point["baseline"],
                        "features": f["features"].get(str(hours), {}), "forecast": f})
            last_end = end
    return out


def forward_report(store, now: float, target: str, config_hash: str | None = None) -> dict:
    results = {}
    for h in (24, 48):
        rows = matured(store, now, target, h, config_hash)
        p, y = [r["p"] for r in rows], [r["label"] for r in rows]
        b = [r["baseline"] for r in rows]
        source_scores = {}
        for name in sorted({c["id"] for r in rows for c in r["forecast"]["components"]}):
            pairs = [(r, next((c for c in r["forecast"]["components"] if c["id"] == name), None)) for r in rows]
            pairs = [(r, c) for r, c in pairs if c is not None]
            ps = [c["probabilities"][h - 1] for r, c in pairs]
            ys = [r["label"] for r, c in pairs]
            source_scores[name] = {"score": metrics(ps, ys),
                                   "baseline_on_same_windows": metrics([r["baseline"] for r, c in pairs], ys)}
        results[str(h)] = {"ensemble": metrics(p, y), "baseline": metrics(b, y), "components": source_scores,
                           "paired_brier_difference_95": paired_block_interval([(a - c) ** 2 - (v - c) ** 2 for a, v, c in zip(p, b, y)])}
    return {"mode": "prospective_nonoverlapping", "target": target, "as_of": now,
            "config_hash": config_hash,
            "label_reporting_grace_hours": 3, "windows": results}


def retrospective(events: list[Event], now: float, config: ModelConfig | None = None) -> dict:
    config = config or ModelConfig()
    times = sorted(e.at for e in events if e.at <= now)
    if len(times) < 10:
        return {"mode": "retrospective_catalogue", "status": "insufficient_events"}
    results = {}
    for h in (24, 48):
        start = (int(times[7] / 86400) + 1) * 86400
        predictions, constants, labels = [], [], []
        while start + h * 3600 <= now:
            past = [t for t in times if t <= start]
            predictions.append(Survival(past, start, config).p(h))
            constants.append(Survival(past, start, config, constant=True).p(h))
            labels.append(int(any(start < t <= start + h * 3600 for t in times)))
            start += h * 3600
        score, comparator = metrics(predictions, labels), metrics(constants, labels)
        delta = [(p - y) ** 2 - (b - y) ** 2 for p, b, y in zip(predictions, constants, labels)]
        results[str(h)] = {"piecewise_survival": score, "constant_rate": comparator,
                           "paired_brier_difference_95": paired_block_interval(delta, max(1, 168 // h))}
    return {"mode": "retrospective_catalogue", "event_count": len(times),
            "assumption": "event_catalogue_known_at_event_time_historical_discovery_delays_unavailable",
            "source_forecasts_backfilled": False, "windows": results}
