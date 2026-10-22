"""Live renewal boundaries, independent of the slower historical catalogue.

A source's completed-reset anchor invalidates forecasts of the previous cycle.
It does not add an outcome to the training history or settle evaluator labels.
"""
from .contracts import AUTOMATIC
from .sources import CATALOG


def live_cycle(observations, historical_at, now):
    anchors = []
    for obs in observations.values():
        if obs.observed_at > now or obs.issues:
            continue
        for curve in obs.curves:
            # These adapters define this field as the last automatic reset:
            # Codex Reset's global-reset clock, Monitor's forced-reset history,
            # and NextReset's automatic-reset history. Other dates are ineligible.
            adapter = curve.metadata.get("adapter") or next(
                (s["adapter"] for s in CATALOG if s["id"] == curve.source), None
            )
            if (adapter not in ("codex_reset", "monitor", "nextreset")
                    or curve.target != AUTOMATIC or curve.private or curve.issues):
                continue
            at = curve.last_reset_at
            if at is None or at <= historical_at or at > min(curve.origin, now):
                continue
            if curve.generated_at > obs.observed_at + 60:
                continue
            # A known occurrence does not expire when its forecast TTL expires.
            # A corrected last-reset field in the next successful observation
            # replaces it naturally; it is never kept as an immutable override.
            anchors.append({"source": curve.source, "at": at,
                            "known_at": obs.observed_at, "basis": "source_last_reset"})
    at = max([historical_at, *(a["at"] for a in anchors)])
    return {"at": at, "historical_at": historical_at,
            "basis": "source_last_reset" if at > historical_at else "historical_catalogue",
            "observations": [a for a in anchors if a["at"] == at]}
