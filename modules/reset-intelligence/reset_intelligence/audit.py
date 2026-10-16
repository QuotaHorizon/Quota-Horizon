"""Read-only source admission and issued-weight audit; no trading or new fitting."""
from __future__ import annotations

import json

from .contracts import TARGETS, digest, iso
from .sources import normalization_outdated


def native_support(curve, issued_at, result, component):
    """Source deadline and freshness are different clocks; tails are model values."""
    remaining = max(0.0, curve.points[-1][0] - (issued_at - curve.origin) / 3600)
    last_event = result.get("last_event_at")
    current = (
        not normalization_outdated(curve)
        and not (
            curve.source == "codex_reset"
            and curve.metadata.get("estimate_kind") != "published_complete"
        )
        and curve.aligned(issued_at, 0, last_event) is not None
        and not (
            last_event is not None
            and curve.last_reset_at is not None
            and curve.last_reset_at < last_event - 3600
        )
    )
    # Only the issued component can establish that a tail was actually used.
    used = bool(
        component and component.get("metadata", {}).get("origin") == curve.origin
    )
    return {
        "published_hours": curve.points[-1][0],
        "remaining_hours": remaining,
        "valid_until": iso(curve.expires_at),
        "current": current,
        "native_deadline": iso(curve.origin + curve.points[-1][0] * 3600),
        "windows": {
            str(h): {
                "native_supported": current and h <= remaining,
                "aligned_value_basis": "native"
                if used and h <= remaining
                else "baseline_tail"
                if used
                else "not_included",
            }
            for h in (24, 48)
        },
    }

def participation(component):
    role = (
        component.get(
            "role",
            "community" if component.get("family") == "community" else "background",
        )
        if component
        else None
    )
    family = component.get("family") if component else None
    return {
        "role": role,
        "background_anchor": bool(
            component and component.get("baseline_weight", 0) > 0
        ),
        "information_revision": bool(
            family == "radars"
            and role == "information"
            and component.get("weight", 0) > 0
        ),
        "comparison_only": bool(family == "radars" and role == "comparison"),
        "community_residual": bool(
            family == "community" and component.get("weight", 0) > 0
        ),
    }

def source_audit(store, now, results=None):
    if results is None:
        results = []
        for target in TARGETS:
            row = store.db.execute("SELECT payload FROM forecasts WHERE issued_at<=? AND target=? ORDER BY issued_at DESC,rowid DESC LIMIT 1", (now, target)).fetchone()
            if row:
                results.append(json.loads(row[0]))
    rows, targets, attempts_by_source = [], [], {}
    for result in results:
        issued_at = result["issued_at"]
        observations = store.observations(issued_at)
        target_rows = []
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
            attempt_key = (source, issued_at)
            if attempt_key not in attempts_by_source:
                attempts_by_source[attempt_key] = store.db.execute("SELECT COUNT(*) n,SUM(normalized IS NOT NULL) decoded,SUM(error IS NOT NULL) failed FROM observations WHERE source=? AND observed_at BETWEEN ? AND ?", (source, issued_at - 7*86400, issued_at)).fetchone()
            attempts = attempts_by_source[attempt_key]
            details = {}
            if component:
                family = component["family"]
                matching = [g for g in groups if g["family"] == family]
                mass = sum(weights.get(family, 0) * g["weights"].get(source, 0) * g.get("group_weight", 1 / max(1, len(matching))) for g in matching)
                if result.get("combination_method") == "baseline_plus_revisions":
                    mass = component.get("weight", 0.)
                metadata = component.get("metadata", {})
                strength = metadata.get("shrinkage_strength")
                details = {"family": family, "component_weight_before_projection": mass,
                           "weight_state": training.get("state"), "dependency": component["dependency"] if not health["private"] else "internal_community_group",
                           "votes": metadata.get("votes"), "effective_votes": metadata.get("effective_votes"),
                           "latest_vote_at": metadata.get("latest_vote_at"),
                           "freshness_weight": metadata.get("freshness_weight"),
                           "half_life_hours": metadata.get("half_life_hours"),
                           "shrinkage_strength": strength,
                           "sample_reliability": metadata.get("sample_reliability"),
                           "ballot_weight_before_projection": mass * strength if strength is not None else mass if "votes" in metadata else None,
                           "assumption": metadata.get("assumption", metadata.get("alignment")),
                           "role": component.get("role", "background"),
                           "background_anchor_weight": component.get("baseline_weight"),
                           "signal_baseline_24h": (component.get("signal_baseline") or [None]*72)[23],
                           "signal_baseline_48h": (component.get("signal_baseline") or [None]*72)[47],
                           'revision_24h_before_projection': mass * (component['probabilities'][23] - component['signal_baseline'][23]) if component.get('role') == 'information' and component.get('signal_baseline') else None,
                           'revision_48h_before_projection': mass * (component['probabilities'][47] - component['signal_baseline'][47]) if component.get('role') == 'information' and component.get('signal_baseline') else None,
                           "comment": metadata.get("comment"),
                           "probability_24h": component["probabilities"][23], "probability_48h": component["probabilities"][47]}
            curves = [c for c in obs.curves if c.target == result["target"]] if obs else []
            target_rows.append({"source": "internal_signal_" + digest(source)[:8] if health["private"] else source,
                         "private": health["private"], "target": result["target"],
                         "issued_at": iso(issued_at), "participation": participation(component),
                         "admission": "included" if component else "excluded" if health["issues"] or health["error"] else
                                      "supporting_evidence" if obs and (obs.events or obs.facts) else "different_target_or_no_probability",
                         "has_supporting_evidence": bool(obs and (obs.events or obs.facts)),
                         "reasons": sorted(set(health["issues"])), "last_error": health["error"],
                         "last_attempt": iso(health["last_attempt"]) if health["last_attempt"] is not None else None,
                         "generated_at": iso(health["generated_at"]) if health["generated_at"] is not None else None,
                         "seven_day_attempts": attempts["n"], "decoded_attempts": attempts["decoded"] or 0,
                         "failed_attempts": attempts["failed"] or 0,
                         "decode_rate": attempts["decoded"] / attempts["n"] if attempts["n"] else None,
                         "native_support": [native_support(c, issued_at, result, component) for c in curves],
                         "issued_contribution": details})
        rows.extend(target_rows)
        radar_rows = [r for r in target_rows if not r["private"] and r["issued_contribution"].get("family") == "radars"]
        targets.append({"target": result["target"], "issued_at": iso(result["issued_at"]), "status": result["status"],
                        "config_hash": result["config_hash"], "model_version": result["model_version"],
                        "family_weights": weights, "training_state": training.get("state"),
                        "training_windows": training.get("windows", 0), "coherence": result.get("coherence"),
                        "learned_weights_applied": training.get("apply_learned_weights", False),
                        "candidate_state": training.get("candidate_state"),
                        "radar_coverage": {'included': len(radar_rows), **{key: sum((r['participation'][key] for r in radar_rows)) for key in ('background_anchor', 'information_revision', 'comparison_only')}, 'windows': {str(h): {basis: sum((any((n['windows'][str(h)]['aligned_value_basis'] == basis for n in r['native_support'])) for r in radar_rows)) for basis in ('native', 'baseline_tail')} for h in (24, 48)}},
                        "community_dependency_groups": len(training.get("increment_groups", {}).get("community", []))
                            if result.get("combination_method") == "baseline_plus_revisions" else len([g for g in groups if g["family"] == "community"]),
                        "excluded_targeted_events": result.get("event_audit", {}).get("excluded_targeted_events", []),
                        "projection_points": sum("unconstrained_probability" in p for p in result["curve"])})
    return {"schema_version": "reset-intelligence-audit/1", "generated_at": iso(now), "targets": targets, "sources": rows,
            "interpretation": "issued_weights_and_source_admission_not_validated_skill",
            "dependence_assumptions": {"baseline": "source_declared_historical_anchor_with_survival_fallback",
                                       "radars": "one_historical_anchor_plus_source_declared_revisions",
                                       "community": "recent_votes_residual_with_explicit_overlap_discount",
                                       "unknown_cross_population_overlap": "unmeasured"},
            "evaluation": "use_independent_observer_same_window_scores_and_ablations"}
