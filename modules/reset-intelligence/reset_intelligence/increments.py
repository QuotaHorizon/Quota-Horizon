"""One historical anchor, source-declared revisions, then a community residual.

No statement text is scored here. A source with an unchanged historical forecast
is background; it is not a zero-valued observation of the current news effect.
Sources lacking a published decomposition remain visible comparisons. Within a
shared evidence group we average revisions, rather than add repeated reports.
"""
from __future__ import annotations

import numpy as np
from .sentiment import shift_curve


def project_cdf(values):
    """Least-squares isotonic projection, then probability bounds."""
    blocks = []
    for value in values:
        blocks.append([float(value), 1])
        while len(blocks) > 1 and blocks[-2][0] > blocks[-1][0]:
            b, a = blocks.pop(), blocks.pop()
            blocks.append([(a[0]*a[1]+b[0]*b[1])/(a[1]+b[1]), a[1]+b[1]])
    return np.clip(np.array([v for v,n in blocks for _ in range(n)]), 0, 1)


def grouped_weights(members):
    groups = {}
    for c in members:
        groups.setdefault(c["dependency"], []).append(c)
    if not groups:
        return {}, []
    quality = {k: max(c["metadata"].get("quality", 1.) for c in group) for k, group in groups.items()}
    total = sum(quality.values())
    if total <= 0:
        return {}, []
    weights, audit = {}, []
    for name, group in sorted(groups.items()):
        mass = quality[name] / total
        denom = sum(c["metadata"].get("quality", 1.) for c in group)
        if denom <= 0:
            continue
        local = {c["id"]: c["metadata"].get("quality", 1.) / denom for c in group}
        weights.update({k: mass * w for k, w in local.items()})
        audit.append({"dependency": name, "weights": local, "group_weight": mass,
                      "state": "cold_start_prior", "windows": 0, "candidate_weights": None})
    return weights, audit


def combine(components, config, exclude=frozenset(), community_weight=None, remove_signals=False, informed_floor=None):
    items = [c for c in components if c["id"] not in exclude]
    local = next(c for c in components if c["id"] == "survival_baseline")
    radars = [c for c in items if c["family"] == "radars"]
    active = [c for c in radars if c.get("role") == "information"]
    backgrounds = [c for c in radars if c.get("role") == "background"]
    # A decomposition is required to attribute a revision. An opaque full
    # forecast never silently becomes a negative news adjustment.
    backgrounds = active + backgrounds or [local]
    anchor_weights, anchor_groups = grouped_weights(backgrounds)
    revision_weights, revision_groups = grouped_weights(active)
    base = sum(np.asarray(c.get("signal_baseline", c["probabilities"])) * anchor_weights[c["id"]] for c in backgrounds)
    delta = sum(((np.asarray(c["probabilities"]) - np.asarray(c["signal_baseline"])) * revision_weights[c["id"]] for c in active), np.zeros_like(base))
    comment_delta = sum(((np.asarray(c["probabilities"]) - np.asarray(c["metadata"].get("pre_comment_probabilities", c["probabilities"])))
                         * revision_weights[c["id"]] for c in active), np.zeros_like(base))
    if remove_signals:
        delta = np.zeros_like(base)
        comment_delta = np.zeros_like(base)
    # The sum of monotone CDFs and signed revisions need not itself be a CDF.
    # Preserve the raw requested increment, expose the necessary probability
    # bounds/time-order repair instead of silently attributing it to news.
    informed_raw = base + delta
    informed = project_cdf(informed_raw)
    if informed_floor is not None:
        informed = np.maximum(informed, informed_floor)
    community = [c for c in items if c["family"] == "community" and c["metadata"].get("quality", 1.) > 0]
    # Rebase innovations for every ablation. Reusing an overlay calculated on
    # the full news forecast would leak removed information into its control.
    community = [{**c, "probabilities": shift_curve(informed, range(1, len(informed)+1),
                  c["metadata"]["coverage_hours"], c["metadata"]["log_odds_change"])}
                 if c["metadata"].get("mapping_kind") == "sentiment_innovation" else c for c in community]
    precision = [{**c, "metadata": {**c["metadata"], "quality": min(1-1e-6, c["metadata"].get("quality", 0.)) / max(1e-6, 1-c["metadata"].get("quality", 0.))}} for c in community]
    community_weights, community_groups = grouped_weights(precision)
    community_curve = sum(np.asarray(c["probabilities"]) * community_weights[c["id"]] for c in community) if community else informed.copy()
    # Convert sample reliability q=n_eff/(n_eff+prior) back into relative
    # information mass. Shared populations contribute only their maximum mass.
    # This removes the old fixed 10% ceiling while retaining sample/clock and
    # overlap adjustments. These are explicit priors, not fitted precision.
    masses = {}
    for c in community:
        q = min(1-1e-6, max(0., c["metadata"].get("quality", 0.)))
        masses[c["dependency"]] = max(masses.get(c["dependency"], 0.), q/(1-q))
    community_mass = sum(masses.values()) * (1-config.community_overlap)
    alpha = community_mass / (1+community_mass)
    if community_weight is not None and community:
        alpha = community_weight
    residual = alpha * (community_curve - informed)
    combined = informed + residual
    if informed_floor is not None:
        combined = np.maximum(combined, informed_floor)
        residual = combined - informed
    weights = {c["id"]: revision_weights.get(c["id"], 0.) if c["family"] == "radars" else alpha*community_weights.get(c["id"], 0.) for c in components}
    if np.any(np.diff(combined) < -1e-8) or np.any(combined < -1e-8) or np.any(combined > 1+1e-8):
        raise ValueError("invalid increment combination")
    return {"probabilities": np.clip(combined, 0, 1).tolist(), "baseline": base.tolist(),
            "radar_adjustment": delta.tolist(), "community_adjustment": residual.tolist(),
            "comment_adjustment": comment_delta.tolist(),
            "radar_probability": informed.tolist(), "community": community_curve.tolist(),
            "shape_adjustment": (informed-informed_raw).tolist(),
            "community_weight": alpha, "weights": weights, "baseline_weights": anchor_weights,
            "groups": {"background": anchor_groups, "revisions": revision_groups, "community": community_groups}}


def counterfactuals(components, config, informed_floor=None, counterfactual_floors=None):
    def mix(name, items, config, *args, **kwargs):
        floor = (counterfactual_floors or {}).get(name, informed_floor)
        return combine(items, config, *args, informed_floor=floor, **kwargs)
    community = {c["id"] for c in components if c["family"] == "community"}
    variants = {"without_community": mix("without_community", components, config, community)["probabilities"],
                "without_information": mix("without_information", components, config, remove_signals=True)["probabilities"],
                "equal_components": np.mean([c["probabilities"] for c in components], axis=0).tolist()}
    if any(c["metadata"].get("pre_comment_probabilities") for c in components):
        before = [{**c, "probabilities": c["metadata"].get("pre_comment_probabilities", c["probabilities"]),
                   "role": c["metadata"].get("pre_comment_role", c.get("role"))} for c in components]
        variants["without_comment_estimates"] = mix("without_comment_estimates", before, config)["probabilities"]
    for weight in (.25, .5, .75):
        if community:
            name = f"community_weight_{round(weight*100)}pct"
            variants[name] = mix(name, components, config, community_weight=weight)["probabilities"]
    for c in components:
        if c["id"] != "survival_baseline":
            name = "without:"+c["id"]
            variants[name] = mix(name, components, config, {c["id"]})["probabilities"]
    for name in counterfactual_floors or {}:
        if name not in variants:
            variants[name] = mix(name, components, config)["probabilities"]
    if informed_floor is not None:
        variants["equal_components"] = np.maximum(variants["equal_components"],
            (counterfactual_floors or {}).get("equal_components", informed_floor)).tolist()
    return variants
