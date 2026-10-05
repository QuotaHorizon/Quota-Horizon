"""Read-only source admission and issued-weight audit; no trading or new fitting."""
from __future__ import annotations

import json

from .contracts import TARGETS, digest, iso


def source_audit(store, now, results=None):
    if results is None:
        results = []
        for target in TARGETS:
            row = store.db.execute("SELECT payload FROM forecasts WHERE issued_at<=? AND target=? ORDER BY issued_at DESC,rowid DESC LIMIT 1", (now, target)).fetchone()
            if row:
                results.append(json.loads(row[0]))
    observations = store.observations(now)
    rows, targets = [], []
    for result in results:
        components = {c["id"]: c for c in result["components"]}
        training = result.get("training", {})
        weights, groups = training.get("weights", {}), training.get("within_groups", [])
        health_rows = list(result["source_health"])
        if "survival_baseline" in components:
            health_rows.append({"id": "survival_baseline", "private": False, "issues": [], "error": None,
                                "last_attempt": None, "generated_at": result["issued_at"]})
        for health in health_rows:
            source = health["id"]
            obs = observations.get(source)
            component = components.get(source)
            attempts = store.db.execute("SELECT COUNT(*) n,SUM(normalized IS NOT NULL) decoded,SUM(error IS NOT NULL) failed FROM observations WHERE source=? AND observed_at BETWEEN ? AND ?", (source, now - 7*86400, now)).fetchone()
            details = {}
            if component:
                family = component["family"]
                matching = [g for g in groups if g["family"] == family]
                mass = sum(weights.get(family, 0) * g["weights"].get(source, 0) / max(1, len(matching)) for g in matching)
                metadata = component.get("metadata", {})
                strength = metadata.get("shrinkage_strength")
                details = {"family": family, "component_weight_before_projection": mass,
                           "weight_state": training.get("state"), "dependency": component["dependency"] if not health["private"] else "internal_community_group",
                           "votes": metadata.get("votes"), "effective_votes": metadata.get("effective_votes"),
                           "shrinkage_strength": strength,
                           "ballot_weight_before_projection": mass * strength if strength is not None else None,
                           "assumption": metadata.get("assumption", metadata.get("alignment")),
                           "probability_24h": component["probabilities"][23], "probability_48h": component["probabilities"][47]}
            curves = [c for c in obs.curves if c.target == result["target"]] if obs else []
            rows.append({"source": "internal_signal_" + digest(source)[:8] if health["private"] else source,
                         "private": health["private"], "target": result["target"],
                         "admission": "included" if component else "excluded" if health["issues"] or health["error"] else
                                      "supporting_evidence" if obs and (obs.events or obs.facts) else "different_target_or_no_probability",
                         "has_supporting_evidence": bool(obs and (obs.events or obs.facts)),
                         "reasons": sorted(set(health["issues"])), "last_error": health["error"],
                         "last_attempt": iso(health["last_attempt"]) if health["last_attempt"] is not None else None,
                         "generated_at": iso(health["generated_at"]) if health["generated_at"] is not None else None,
                         "seven_day_attempts": attempts["n"], "decoded_attempts": attempts["decoded"] or 0,
                         "failed_attempts": attempts["failed"] or 0,
                         "decode_rate": attempts["decoded"] / attempts["n"] if attempts["n"] else None,
                         "native_support": [{"published_hours": c.points[-1][0], "remaining_hours": max(0, c.points[-1][0] - (now-c.origin)/3600),
                                             "valid_until": iso(c.expires_at), "current": not c.issues and c.origin <= now < c.expires_at} for c in curves],
                         "issued_contribution": details})
        targets.append({"target": result["target"], "issued_at": iso(result["issued_at"]), "status": result["status"],
                        "config_hash": result["config_hash"], "model_version": result["model_version"],
                        "family_weights": weights, "training_state": training.get("state"),
                        "training_windows": training.get("windows", 0), "coherence": result.get("coherence"),
                        "learned_weights_applied": training.get("apply_learned_weights", False),
                        "candidate_state": training.get("candidate_state"),
                        "community_dependency_groups": len([g for g in groups if g["family"] == "community"]),
                        "excluded_targeted_events": result.get("event_audit", {}).get("excluded_targeted_events", []),
                        "projection_points": sum("unconstrained_probability" in p for p in result["curve"])})
    return {"schema_version": "reset-intelligence-audit/1", "generated_at": iso(now), "targets": targets, "sources": rows,
            "interpretation": "issued_weights_and_source_admission_not_validated_skill",
            "dependence_assumptions": {"history": "shared_event_history_one_family_budget",
                                       "community": "exact_round_duplicates_removed_population_groups_share_budget",
                                       "unknown_cross_population_overlap": "unmeasured"},
            "evaluation": "use_independent_observer_same_window_scores_and_ablations"}
