"""Independent forecasting pipeline and allowlisted consumer contract."""
from __future__ import annotations

from dataclasses import asdict
import numpy as np

from . import MODEL_VERSION, VERSION
from .community import ballot_curve, count_curve
from .contracts import TARGETS, digest, iso
from .evaluation import canonical_events, matured
from .quant import ModelConfig, Survival, fit_weights

HOURS = list(range(1, 73))


def align_curve(curve, now: float, hours: list[int], baseline: Survival) -> tuple[list[float], dict] | None:
    if curve.issues or now < curve.origin or now >= curve.expires_at:
        return None
    age = (now - curve.origin) / 3600
    start = curve.cdf(age)
    if start is None or start >= 1 - 1e-9:
        return None
    coverage = curve.points[-1][0] - age
    values = []
    for h in hours:
        covered = min(h, coverage)
        end = curve.cdf(age + covered)
        # Complete the explicitly unsupported tail with our baseline conditional
        # survival. Never present those tail values as source-issued forecasts.
        survival = (1 - end) / (1 - start)
        if h > covered:
            survival *= (1 - baseline.p(h)) / (1 - baseline.p(covered))
        values.append(float(1 - survival))
    return values, {"source_coverage_hours": coverage,
                    "alignment": "condition_on_no_event_then_baseline_tail",
                    "origin": curve.origin, "generated_at": curve.generated_at,
                    "expires_at": curve.expires_at}


def complete_tail(values: list[float | None], hours: list[int], baseline: Survival,
                  terminal: tuple[float, float] | None = None):
    out, last_h, last_p = [], 0., 0.
    for h, p in zip(hours, values):
        if p is None:
            if terminal is not None and last_h < terminal[0] < h:
                last_h, last_p = terminal
            p = 1 - (1 - last_p) * (1 - baseline.p(h)) / (1 - baseline.p(last_h))
        out.append(p)
        last_h, last_p = h, p
    return out


def forecast(store, now: float, target: str, config: ModelConfig | None = None,
             config_hash: str | None = None, source_ids: set[str] | None = None,
             private_source_ids: set[str] | None = None) -> dict:
    if target not in TARGETS:
        raise ValueError("unknown target")
    config = config or ModelConfig()
    config_hash = config_hash or digest(asdict(config))
    observations = {k: v for k, v in store.observations(now).items() if source_ids is None or k in source_ids}
    ledger = observations.get("event_ledger")
    events = canonical_events(ledger, target, now)
    result = {"schema_version": "reset-intelligence/1", "module_version": VERSION,
              "model_version": MODEL_VERSION, "config_hash": config_hash,
              "issued_at": now, "target": target, "status": "unavailable", "curve": [],
              "components": [], "features": {}, "source_health": [], "facts": [],
              "event_count": len(events), "events_hash": digest([asdict(e) for e in events]),
              "inputs_hash": digest({k: v.to_dict() for k, v in observations.items()})}
    attempts = {a["source"]: a for a in store.attempts(now) if source_ids is None or a["source"] in source_ids}
    for name in sorted(set(observations) | set(attempts)):
        obs = observations.get(name)
        result["source_health"].append({"id": name, "last_attempt": attempts.get(name, {}).get("observed_at"),
                                        "error": attempts.get(name, {}).get("error"),
                                        "last_success": obs.observed_at if obs else None,
                                        "generated_at": obs.generated_at if obs else None,
                                        "issues": list(obs.issues) if obs else [],
                                        "private": name in (private_source_ids or set()) or (any(p.get("private") for p in obs.polls) if obs else False)})
        if obs:
            result["facts"].extend(f for f in obs.facts if f.get("kind") == "statement" and f["at"] <= now)
    if not ledger or len(events) < 3 or not ledger.complete or ledger.issues:
        result["reason"] = "event_history_unavailable_or_incomplete"
        return result
    if ledger.generated_at is None or now - ledger.generated_at > 3600:
        result["reason"] = "event_history_stale"
        result["status"] = "stale"
        return result
    result["status"] = "current"
    result["last_event_at"] = events[-1].at
    result["event_audit"] = {"time_basis": "public_notice_or_observed_event",
                             "classification_corrections": [asdict(e) for e in events if e.classification_basis != "source_label"]}
    baseline = Survival([e.at for e in events], now, config)
    base = [baseline.p(h) for h in HOURS]
    components = [{"id": "survival_baseline", "family": "history", "dependency": "public_event_history",
                   "probabilities": base, "metadata": baseline.summary(), "private": False}]
    seen = set()
    for obs in observations.values():
        for c in obs.curves:
            if c.target != target:
                continue
            health = next(h for h in result["source_health"] if h["id"] == c.source)
            reason = list(c.issues)
            if c.last_reset_at is not None and c.last_reset_at < events[-1].at - 3600:
                reason.append("missed_latest_event")
            if events[-1].at > c.origin:
                reason.append("forecast_precedes_latest_event")
            aligned = align_curve(c, now, HOURS, baseline) if not reason else None
            if aligned is None:
                health["issues"].extend(reason or ["expired_or_horizon_not_started"])
                continue
            # Exact syndicated curves share a contribution even across hostnames.
            fingerprint = digest({"target": c.target, "points": c.points, "origin": c.origin,
                                  "dependency": c.dependency})
            if fingerprint in seen:
                health["issues"].append("duplicate_forecast")
                continue
            seen.add(fingerprint)
            values, alignment = aligned
            components.append({"id": c.source, "family": "history", "dependency": c.dependency,
                               "probabilities": values, "metadata": {**c.metadata, **alignment},
                               "source_points": c.points, "private": c.private})
        for poll in obs.polls:
            if poll["target"] != target or now - obs.observed_at > 1800:
                continue
            if poll["kind"] == "count_distribution":
                converted = count_curve(poll, now, HOURS, [e.at for e in events])
            else:
                converted = ballot_curve(poll, now, HOURS, baseline, events[-1].at, config)
            if converted is None:
                next(h for h in result["source_health"] if h["id"] == obs.source)["issues"].append("no_usable_current_poll")
                continue
            terminal = (converted["coverage_hours"], converted["terminal_probability"]) if "terminal_probability" in converted else None
            values = complete_tail(converted.pop("probabilities"), HOURS, baseline, terminal)
            converted["fresh_until"] = min(obs.observed_at + 1800, poll["expires"])
            components.append({"id": obs.source, "family": "community", "dependency": poll["population"],
                               "probabilities": values, "metadata": converted, "private": poll.get("private", False)})

    # Hierarchical budget: duplicates/populations share a dependency group;
    # history-based sites together get one family vote, regardless of site count.
    training_rows = matured(store, now, target, 24, config_hash)
    families = sorted(set(c["family"] for c in components))
    features = {}
    source_training = []
    for family in families:
        members = [c for c in components if c["family"] == family]
        # Our baseline and third-party historical models depend on the same
        # underlying events. Pool that entire family as one feature.
        dependencies = {c["dependency"] for c in members} if family == "community" else {"shared_history"}
        groups = []
        for dep in sorted(dependencies):
            group = [c for c in members if family == "history" or c["dependency"] == dep]
            names = [c["id"] for c in group]
            available = []
            for row in training_rows:
                lookup = {c["id"]: c["probabilities"][23] for c in row["forecast"]["components"]}
                if set(names).issubset(lookup):
                    available.append(([lookup[n] for n in names], row["label"]))
            ys = np.array([r[1] for r in available])
            prior = np.full(len(group), 1 / len(group))
            learned = len(ys) >= config.minimum_training_windows and ys.sum() >= 5 and (1 - ys).sum() >= 5
            weight = fit_weights(np.array([r[0] for r in available]), ys, prior, config.stacking_regularization) if learned else prior
            groups.append(weight @ np.array([c["probabilities"] for c in group]))
            source_training.append({"family": family, "dependency": dep, "weights": dict(zip(names, weight.tolist())),
                                    "state": "learned" if learned else "cold_start_prior", "windows": len(available)})
        features[family] = np.mean(groups, axis=0)
    result["features"] = {str(h): {f: float(features[f][i]) for f in families} for i, h in enumerate(HOURS)}
    training = [r for r in training_rows
                if set(families).issubset(r["features"])]
    prior = np.full(len(families), 1 / len(families))
    y = np.array([r["label"] for r in training])
    learned = len(y) >= config.minimum_training_windows and y.sum() >= 5 and (1 - y).sum() >= 5
    if learned:
        x = np.array([[r["features"][f] for f in families] for r in training])
        weights = fit_weights(x, y, prior, config.stacking_regularization)
    else:
        weights = prior
    matrix = np.array([features[f] for f in families])
    combined = weights @ matrix
    if np.any(np.diff(combined) < -1e-9):
        raise ValueError("nonmonotone component conversion")
    result["components"] = components
    result["training"] = {"state": "learned" if learned else "cold_start_prior", "windows": len(training),
                          "events": int(y.sum()), "minimum_windows": config.minimum_training_windows,
                          "objective": "regularized_brier", "weights": dict(zip(families, weights.tolist())),
                          "prior": "equal_mass_per_available_family", "horizon_trained_hours": 24}
    result["training"]["within_groups"] = source_training
    for i, h in enumerate(HOURS):
        lo, hi = baseline.interval(h) if h in (6, 12, 24, 48, 72) else (None, None)
        source_values = [c["probabilities"][i] for c in components]
        result["curve"].append({"hours": h, "probability": float(combined[i]), "baseline": base[i],
                                "baseline_parameter_interval_80": [lo, hi] if lo is not None else None,
                                "component_range": [min(source_values), max(source_values)]})
    result["valid_until"] = min([now + 900, ledger.generated_at + 3600,
                                 *(c["metadata"].get("expires_at", now + 900) for c in components),
                                 *(c["metadata"].get("fresh_until", now + 900) for c in components)])
    return result


def forecast_all(store, now: float, config: ModelConfig | None = None, config_hash: str | None = None,
                 source_ids: set[str] | None = None, private_source_ids: set[str] | None = None) -> list[dict]:
    results = [forecast(store, now, t, config, config_hash, source_ids, private_source_ids) for t in TARGETS]
    broad, automatic = results
    if all(r["status"] == "current" for r in results):
        for a, b in zip(broad["curve"], automatic["curve"]):
            # Euclidean projection onto P(automatic) <= P(any relief). The two
            # original curves are monotone, so this preserves both time orders.
            if a["probability"] < b["probability"]:
                mean = (a["probability"] + b["probability"]) / 2
                for p in (a, b):
                    p["unconstrained_probability"] = p["probability"]
                    p["probability"] = mean
        for r in results:
            r["coherence"] = "projection_automatic_subset_of_broad_relief"
    return results


def consumer_export(results: list[dict]) -> dict:
    """Construct, rather than redact, the product payload. No raw market paths."""
    forecasts = []
    for result in results:
        item = {"target": result["target"], "status": result["status"],
                "issued_at": iso(result["issued_at"]), "model_version": result["model_version"],
                "curve": result["curve"], "reason": result.get("reason"),
                "valid_until": iso(result["valid_until"]) if result.get("valid_until") else None,
                "training_state": result.get("training", {}).get("state"),
                "coherence": result.get("coherence"),
                "evaluation_windows": result.get("training", {}).get("windows", 0),
                "event_count": result["event_count"], "sources": [], "community": None,
                "source_health": [{k: h.get(k) for k in ("id", "last_success", "generated_at", "error", "issues")}
                                  for h in result["source_health"] if not h["private"]],
                "statements": [{k: f.get(k) for k in ("id", "at", "url", "original", "translation_zh", "stage", "event_type", "scope_note", "context")}
                               for f in result["facts"]]}
        for c in result["components"]:
            if not c["private"] and c["family"] == "history":
                item["sources"].append({"id": c["id"], "probabilities": c["probabilities"],
                                        "source_points": c.get("source_points"),
                                        "origin": c["metadata"].get("origin"),
                                        "source_coverage_hours": c["metadata"].get("source_coverage_hours"),
                                        "alignment": c["metadata"].get("alignment")})
        if "community" in result.get("training", {}).get("weights", {}):
            item["community"] = {"probabilities": [result["features"][str(h)]["community"] for h in HOURS],
                                 "weight": result["training"]["weights"]["community"]}
        forecasts.append(item)
    return {"schema_version": "reset-intelligence-consumer/1", "horizons_hours": HOURS, "forecasts": forecasts}
