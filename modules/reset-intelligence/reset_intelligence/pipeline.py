"""Independent forecasting pipeline and allowlisted consumer contract."""
from __future__ import annotations

from dataclasses import asdict, replace
import numpy as np

from . import MODEL_VERSION, VERSION
from .community import ballot_curve, ballot_rejection, count_curve, count_rejection, recent_ballot
from .contracts import TARGETS, digest, iso
from .evaluation import canonical_events
from .quant import ModelConfig, Survival
from .increments import combine, counterfactuals
from .response import announcement_context
from .signals import apply_comment_estimates
from .lifecycle import cycle_issues, live_cycle
from .sources import normalization_outdated
from .sentiment import calendar_overlay
from .community_diagnostics import initialize_community_diagnostics, record_community_diagnostic, public_community_diagnostics

HOURS = list(range(1, 73))


def align_curve(curve, now: float, hours: list[int], baseline: Survival) -> tuple[list[float], dict] | None:
    if (
        normalization_outdated(curve)
        or curve.issues
        or now < curve.origin
        or now >= curve.expires_at
    ):
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
             private_source_ids: set[str] | None = None, informed_floor: list[float] | None = None,
             counterfactual_floors: dict | None = None) -> dict:
    if target not in TARGETS:
        raise ValueError("unknown target")
    config = config or ModelConfig()
    if config.apply_learned_weights:
        raise ValueError("increment model weights require a separately reviewed forward fit")
    config_hash = config_hash or digest(asdict(config))
    observations = {k: v for k, v in store.observations(now).items() if source_ids is None or k in source_ids or k == "completion_evidence"}
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
        if name == "completion_evidence":
            continue
        obs = observations.get(name)
        result["source_health"].append({"id": name, "last_attempt": attempts.get(name, {}).get("observed_at"),
                                        "error": attempts.get(name, {}).get("error"),
                                        "last_success": obs.observed_at if obs else None,
                                        "generated_at": obs.generated_at if obs else None,
                                        "issues": list(obs.issues) if obs else [],
                                        "private": name in (private_source_ids or set()) or (any(p.get("private") for p in obs.polls) if obs else False)})
        if obs:
            result["facts"].extend(f for f in obs.facts if f.get("kind") in ("statement", "structured_schedule") and f["at"] <= now)
    initialize_community_diagnostics(result, observations, config, source_ids, private_source_ids)
    if not ledger or len(events) < 3 or not ledger.complete or ledger.issues:
        result["reason"] = "event_history_unavailable_or_incomplete"
        return result
    if ledger.generated_at is None or now - ledger.generated_at > 3600:
        result["reason"] = "event_history_stale"
        result["status"] = "stale"
        return result
    result["status"] = "current"
    cycle = live_cycle(observations, events[-1].at, now, events, target)
    last_event_at = cycle["at"]
    result["last_event_at"] = last_event_at
    result["cycle"] = cycle
    event_times = sorted({*(e.at for e in events if e.at <= last_event_at), last_event_at})
    result["event_audit"] = {"time_basis": "public_notice_or_observed_event",
                             "excluded_targeted_events": [e.id for e in ledger.events if e.population == "targeted"],
                             "classification_corrections": [asdict(e) for e in events if e.classification_basis != "source_label"],
                             "after_live_boundary": [e.id for e in events if e.at > last_event_at]}
    historical_times = sorted({e.at for e in events if e.at <= last_event_at})
    if len(historical_times) < 3:
        # A live occurrence can predate catalogue publication proxies. Keep
        # those records for audit, without training on post-boundary history or
        # turning an insufficient baseline into a failed refresh.
        result["status"] = "unavailable"
        result["reason"] = "event_history_before_live_boundary_insufficient"
        result["event_audit"]["usable_distinct_times"] = len(historical_times)
        return result
    baseline = Survival([e.at for e in events], now, config, renewal_at=last_event_at)
    base = [baseline.p(h) for h in HOURS]
    components = [{"id": "survival_baseline", "family": "baseline", "dependency": "public_event_history",
                   "probabilities": base, "metadata": baseline.summary(), "private": False}]
    seen, seen_polls = set(), set()
    for obs in observations.values():
        for c in obs.curves:
            if c.target != target:
                continue
            health = next(h for h in result["source_health"] if h["id"] == c.source)
            reason = list(c.issues)
            # Clock and event failures are independent diagnostics. Collect both
            # before deciding admission, even when another issue already rejects it.
            if now >= c.expires_at:
                reason.append("expired")
            if now < c.origin:
                reason.append("horizon_not_started")
            if c.source == "codex_reset" and c.metadata.get("estimate_kind") != "published_complete":
                reason.append("source_normalization_outdated")
            if normalization_outdated(c):
                reason.append("source_normalization_outdated")
            reason.extend(cycle_issues(c, cycle))
            aligned = align_curve(c, now, HOURS, baseline) if not reason else None
            if aligned is None:
                health["issues"].extend(reason or ["expired_or_horizon_not_started"])
                continue
            # Exact syndicated curves share a contribution even across hostnames.
            fingerprint = digest({"target": c.target, "points": c.points, "origin": c.origin,
                                  "dependency": c.dependency, "baseline_points": c.metadata.get("baseline_points")})
            if fingerprint in seen:
                health["issues"].append("duplicate_forecast")
                continue
            seen.add(fingerprint)
            values, alignment = aligned
            signal_baseline, role = None, "comparison"
            if c.metadata.get("baseline_points"):
                paired = replace(c, points=c.metadata["baseline_points"])
                try:
                    paired.validate()
                    aligned_base = align_curve(paired, now, HOURS, baseline)
                except (ValueError, TypeError, IndexError):
                    health["issues"].append("invalid_source_baseline")
                    aligned_base = None
                if aligned_base:
                    signal_baseline = aligned_base[0]
                    # Reviewing a statement does not turn an unchanged cadence
                    # forecast into a zero-effect news vote. Keep actual signed
                    # revisions, including small ones; ignore only numerical
                    # noise or an explicitly identified rounding-only change.
                    active = (not c.metadata.get("rounding_only", False)
                              and any(abs(p-b) > 1e-12 for p,b in zip(values, signal_baseline)))
                    role = "information" if active else "background"
            components.append({"id": c.source, "family": "radars", "dependency": c.dependency,
                               "role": role, "signal_baseline": signal_baseline,
                               "probabilities": values, "metadata": {**c.metadata, **alignment},
                               "source_points": c.points, "private": c.private})
    apply_comment_estimates(components, result["facts"], now, last_event_at, HOURS, target=target)
    result["response"] = announcement_context(components, result["facts"], now, last_event_at)
    informed = combine(components, config, informed_floor=informed_floor)["radar_probability"]
    for obs in observations.values():
        for poll in obs.polls:
            if poll["target"] != target:
                record_community_diagnostic(result, obs, poll, "target_mismatch")
                continue
            health = next(h for h in result["source_health"] if h["id"] == obs.source)
            if now - obs.observed_at > 1800 or obs.issues:
                health["issues"].append("poll_observation_stale_or_excluded")
                record_community_diagnostic(result, obs, poll, "observation_stale" if now - obs.observed_at > 1800 else "source_quality")
                continue
            sampling = {}
            if poll["kind"] != "count_distribution":
                poll, sampling = recent_ballot(store, obs.source, poll, now, half_life_hours=config.poll_half_life_hours)
                if not poll["total"]:
                    health["issues"].append("empty_poll" if sampling["total_votes"] == 0 else sampling["sampling_state"])
                    record_community_diagnostic(result, obs, poll, "empty_poll" if sampling["total_votes"] == 0 else sampling["sampling_state"], sampling)
                    continue
                if poll["total"] < config.minimum_recent_votes:
                    health["issues"].append("insufficient_recent_votes")
                    record_community_diagnostic(result, obs, poll, "insufficient_recent_votes", sampling)
                    continue
            if poll["kind"] == "calendar_preference":
                converted, reason = calendar_overlay(poll, sampling, now, cycle.get("occurred_at", last_event_at),
                                                     informed, HOURS, config, cycle.get("confirmation_at"))
            elif poll["kind"] == "count_distribution":
                reason = count_rejection(poll, now, event_times)
                converted = count_curve(poll, now, HOURS, event_times)
            else:
                reason = ballot_rejection(poll, now, last_event_at)
                converted = ballot_curve(poll, now, HOURS, baseline, last_event_at, config)
            if converted is None:
                health["issues"].append(reason or "no_usable_current_poll")
                record_community_diagnostic(result, obs, poll, reason or "no_usable_current_poll", sampling)
                continue
            fingerprint = digest({k: poll.get(k) for k in ("kind", "target", "population", "id", "start", "expires", "bins", "rows")})
            if fingerprint in seen_polls:
                health["issues"].append("duplicate_poll_round")
                record_community_diagnostic(result, obs, poll, "duplicate_poll_round", sampling)
                continue
            seen_polls.add(fingerprint)
            terminal = (converted["coverage_hours"], converted["terminal_probability"]) if "terminal_probability" in converted else None
            values = complete_tail(converted.pop("probabilities"), HOURS, baseline, terminal)
            converted.update(sampling)
            converted["fresh_until"] = min(obs.observed_at + 1800, poll["expires"])
            components.append({"id": obs.source, "family": "community", "dependency": poll["population"],
                               "probabilities": values, "metadata": converted, "private": poll.get("private", False)})
            record_community_diagnostic(result, obs, poll, None, sampling)

    mixed = combine(components, config, informed_floor=informed_floor)
    result["combination_method"] = "baseline_plus_revisions"
    result["features"] = {str(h): {"baseline": mixed["baseline"][i], "radars": mixed["radar_probability"][i],
                                    "community": mixed["community"][i]} for i,h in enumerate(HOURS)}
    # No fitted parameters are used here. Forward scoring belongs to the
    # independent observer, not an archive scan on every forecast publication.
    result["training"] = {"state": "cold_start_prior", "windows": 0,
                          "events": 0, "evaluation": "independent_observer", "minimum_windows": config.minimum_training_windows,
                          "weights": {}, "increment_groups": mixed["groups"],
                          "prior": "one_anchor_paired_revisions_community_residual",
                          "community_overlap": config.community_overlap, "community_weight_rule": "quality_odds_by_dependency",
                          "candidate_state": "increment_model_requires_forward_validation", "apply_learned_weights": False}
    result["components"] = components
    result["community_weight"] = mixed["community_weight"]
    result["counterfactuals"] = counterfactuals(components, config, informed_floor=informed_floor,
                                               counterfactual_floors=counterfactual_floors)
    if informed_floor is not None:
        result["informed_floor"] = informed_floor
    for c in components:
        c["weight"] = mixed["weights"][c["id"]]
        c["baseline_weight"] = mixed["baseline_weights"].get(c["id"], 0.)
    for i,h in enumerate(HOURS):
        values = [c["probabilities"][i] for c in components]
        result["curve"].append({"hours": h, "probability": mixed["probabilities"][i], "baseline": mixed["baseline"][i],
            "radar_adjustment": mixed["radar_adjustment"][i], "community_adjustment": mixed["community_adjustment"][i],
            "comment_adjustment": mixed["comment_adjustment"][i],
            "coherence_adjustment": mixed["shape_adjustment"][i], "baseline_parameter_interval_80": None,
            "component_range": [min(values),max(values)]})
    result["valid_until"] = min([now + 900, ledger.generated_at + 3600,
                                 (result.get("response") or {}).get("deadline") or now + 900,
                                 *(c["metadata"].get("expires_at", now + 900) for c in components),
                                 *(c["metadata"].get("fresh_until", now + 900) for c in components)])
    return result


def forecast_all(store, now: float, config: ModelConfig | None = None, config_hash: str | None = None,
                 source_ids: set[str] | None = None, private_source_ids: set[str] | None = None) -> list[dict]:
    # The broad event contains an automatic reset. Establish its compatible
    # information background before measuring a community innovation against it.
    automatic = forecast(store, now, TARGETS[1], config, config_hash, source_ids, private_source_ids)
    floor = [p["probability"] for p in automatic["curve"]] if automatic["status"] == "current" else None
    broad = forecast(store, now, TARGETS[0], config, config_hash, source_ids, private_source_ids,
                     floor, automatic.get("counterfactuals"))
    if floor is not None and broad["status"] == "current":
        broad["valid_until"] = min(broad["valid_until"], automatic["valid_until"])
    results = [broad, automatic]
    if all(r["status"] == "current" for r in results):
        for a, b in zip(broad["curve"], automatic["curve"]):
            # A forecast of the containing event inherits the narrower event's
            # probability floor. Do not average a narrow forecast down merely
            # because a separately estimated broader curve is lower.
            if a["probability"] < b["probability"]:
                a["unconstrained_probability"] = a["probability"]
                a["coherence_adjustment"] += b["probability"] - a["probability"]
                a["probability"] = b["probability"]
        for r in results:
            r["coherence"] = "broad_inherits_automatic_floor"
    return results


def consumer_export(results: list[dict]) -> dict:
    """Construct, rather than redact, the product payload. No raw market paths."""
    forecasts = []
    for result in results:
        without_community = result.get("counterfactuals", {}).get("without_community", [])
        item = {"target": result["target"], "status": result["status"],
                "issued_at": iso(result["issued_at"]), "model_version": result["model_version"],
                "evaluation_version": result.get("evaluation_version") or result["model_version"] + ":" + result["config_hash"],
                "last_event_at": result.get("last_event_at"),
                "cycle": {k: result["cycle"].get(k) for k in
                          ("time_basis", "occurred_at", "confirmation_at", "invalidation_at", "references", "identity")}
                         if result.get("cycle") else None,
                "response": {k: result["response"].get(k) for k in ("at", "url", "reviewed", "changed", "kind", "deadline")}
                            if result.get("response") else None,
                "statements_checked_at": max((h["last_success"] for h in result["source_health"] if h["id"] in ("statements", "tibo_posts") and h.get("last_success")), default=None),
                "curve": [{**point, "without_community_probability": without_community[i]
                           if i < len(without_community) else None}
                          for i, point in enumerate(result["curve"])], "reason": result.get("reason"),
                "valid_until": iso(result["valid_until"]) if result.get("valid_until") else None,
                "training_state": result.get("training", {}).get("state"),
                "coherence": result.get("coherence"),
                "combination_method": result.get("combination_method"),
                "evaluation_windows": result.get("training", {}).get("windows", 0),
                "event_count": result["event_count"], "sources": [], "community": None,
                "family_weights": result.get("training", {}).get("weights", {}),
                "community_inputs": [],
                "community_diagnostics": public_community_diagnostics(result),
                "changes": result.get("changes"),
                "source_health": [{k: h.get(k) for k in ("id", "last_success", "generated_at", "error", "issues")}
                                  for h in result["source_health"] if not h["private"]],
                "statements": [{**{k: f.get(k) for k in ("id", "at", "url", "original", "translation_zh", "stage", "event_type", "scope_note", "context", "relevance", "is_reply", "parent")},
                                "collector": "Codex Reset" if f.get("source") == "tibo_posts" else "Codex Reset FM"}
                               for f in result["facts"] if f.get("kind") == "statement"]}
        for c in result["components"]:
            if not c["private"] and c["family"] in ("baseline", "radars", "history"):
                item["sources"].append({"id": c["id"], "probabilities": c["probabilities"],
                                        "weight": c.get("weight"), "family": c["family"],
                                        "role": c.get("role", "background"), "signal_baseline": c.get("signal_baseline"),
                                        "response_state": c["metadata"].get("response_state"),
                                        "comment": {k: c["metadata"]["comment"].get(k, [] if k == "timing_evidence" else None) for k in
                                            ("probability", "url", "at", "included", "state", "deadline", "timing_sources", "timing_evidence", "adjustment_24h", "adjustment_48h")}
                                            if c["metadata"].get("comment") else None,
                                        "baseline_weight": c.get("baseline_weight"),
                                        "baseline_points": c["metadata"].get("baseline_points"),
                                        "source_points": c.get("source_points"),
                                        "origin": c["metadata"].get("origin"),
                                        "expires_at": c["metadata"].get("expires_at"),
                                        "source_coverage_hours": c["metadata"].get("source_coverage_hours"),
                                        "alignment": c["metadata"].get("alignment")})
        if result.get("community_weight", 0.) > 0:
            item["community"] = {"probabilities": [result["features"][str(h)]["community"] for h in HOURS],
                                 "weight": result["community_weight"]}
        for c in result["components"]:
            if c["family"] == "community" and not c["private"]:
                m = c["metadata"]
                item["community_inputs"].append({"id": c["id"], "state": "included", "weight": c.get("weight", 0.),
                    "votes": m.get("votes"), "effective_votes": m.get("effective_votes"),
                    "total_votes": m.get("total_votes"), "recent_votes": m.get("recent_votes"),
                    "observation_window_hours": m.get("observation_window_hours"),
                    "latest_vote_at": m.get("latest_vote_at"), "freshness_weight": m.get("freshness_weight"),
                    "mapping_kind": m.get("mapping_kind"), "mapping_version": m.get("mapping_version"),
                    "reference_share": m.get("reference_share"), "recent_share": m.get("recent_share"),
                    "log_odds_change": m.get("log_odds_change"), "repeat_sensitivity_units": m.get("repeat_sensitivity_units"),
                    "terminal_probability": m.get("terminal_probability"),
                    "window_start": m.get("window_start"), "window_end": m.get("window_end"),
                    "coverage_hours": m.get("coverage_hours"), "probability_24h": c["probabilities"][23],
                    "probability_48h": c["probabilities"][47]})
        # Only an aggregate availability count crosses the public boundary;
        # private source names, identifiers, rules and venues stay in the audit.
        private = [c for c in result["components"] if c["family"] == "community" and c["private"]]
        private_health = [h for h in result["source_health"] if h["private"] and result["target"] == "broad_quota_relief"]
        item["community_coverage"] = {"included": len([c for c in result["components"] if c["family"] == "community"]),
            "unavailable": max(0, len(private_health) + len([h for h in result["source_health"] if h["id"] in
                              (("date_poll", "didcodex_poll") if result["target"] == "broad_quota_relief" else ("community_poll",))])
                           - len([c for c in result["components"] if c["family"] == "community"])),
            "supplementary_included": len(private), "supplementary_checked": len(private_health),
            "supplementary_weight": sum(c.get("weight", 0.) for c in private)}
        forecasts.append(item)
    return {"schema_version": "reset-intelligence-consumer/1", "horizons_hours": HOURS, "forecasts": forecasts}
