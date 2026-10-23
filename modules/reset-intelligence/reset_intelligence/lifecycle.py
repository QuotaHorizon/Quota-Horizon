"""Live renewal boundaries, separate from publication and discovery clocks.

Historical catalogue dates remain statistical inputs. A publication proxy alone
cannot establish that a later reset occurred, or expel forecasts which already
incorporated an observed reset. Explicit completion confirmations can retire old
forecasts while retaining an unknown occurrence time.
"""
import math

from .contracts import AUTOMATIC, RELIEF, canonical_url, digest
from .sources import CATALOG


def adapter_for(curve):
    return curve.metadata.get("adapter") or next(
        (s["adapter"] for s in CATALOG if s["id"] == curve.source), None)


def references(values):
    result = []
    for value in values:
        try:
            url = canonical_url(value)
        except (TypeError, ValueError, AttributeError):
            continue
        if url not in result:
            result.append(url)
    return sorted(result)


def valid_clock(value, limit):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and 0 < value <= limit


def curve_anchor(curve):
    """Read new provenance or conservatively interpret an older normalized row."""
    anchor = curve.metadata.get("reset_anchor")
    if isinstance(anchor, dict):
        return {**anchor, "references": references(anchor.get("references", []))}
    adapter = adapter_for(curve)
    # Legacy Monitor history used post timestamps, unlike the two operator/feed
    # reset clocks. Do not wait for another successful HTTP fetch to fix this.
    basis = "announcement_proxy" if adapter == "monitor" else "source_clock"
    return {"occurred_at": curve.last_reset_at if basis == "source_clock" else None,
            "announced_at": curve.last_reset_at if basis == "announcement_proxy" else None,
            "discovered_at": None, "time_basis": basis, "references": [], "source_event_id": None}


def live_cycle(observations, historical_at, now, historical_events=None, target=AUTOMATIC):
    occurrences, proxies, confirmations = [], [], []
    for event in historical_events or []:
        if not valid_clock(event.at, now) or event.known_at > now:
            continue
        observed = event.precision == "observed"
        anchor = {"source": "event_ledger", "occurred_at": event.at if observed else None,
                  "announced_at": None if observed else event.at, "discovered_at": None,
                  "received_at": event.known_at, "time_basis": "observed" if observed else "announcement_proxy",
                  "references": references(event.references), "source_event_id": event.id}
        (occurrences if observed else proxies).append(anchor)
    facts = []
    for obs in observations.values():
        if obs.observed_at > now or obs.issues:
            continue
        facts.extend((obs, f) for f in obs.facts if f.get("kind") == "statement")
        for curve in obs.curves:
            if (adapter_for(curve) not in ("codex_reset", "monitor", "nextreset")
                    or curve.target != AUTOMATIC or curve.private or curve.issues
                    or curve.generated_at > obs.observed_at + 60):
                continue
            anchor = {**curve_anchor(curve), "source": curve.source, "received_at": obs.observed_at}
            discovered = anchor.get("discovered_at")
            if discovered is not None and not valid_clock(discovered, obs.observed_at + 60):
                continue
            if anchor.get("time_basis") in ("observed", "source_clock"):
                if valid_clock(anchor.get("occurred_at"), min(curve.origin, now)):
                    occurrences.append(anchor)
            elif valid_clock(anchor.get("announced_at"), min(curve.origin, now)):
                proxies.append(anchor)

    # A structured completed stage is a confirmation, not an exact event time.
    # Only the reviewed statement feed supplies this state. Display selection,
    # message text, siblings and temporal proximity never establish identity.
    explicit_stages = {}
    for _, fact in facts:
        if valid_clock(fact.get("at"), now) and fact.get("stage"):
            for url in references([fact.get("url")]):
                explicit_stages.setdefault(url, set()).add(fact["stage"])
    for obs, fact in facts:
        if (obs.source != "statements" or fact.get("stage") != "completed"
                or fact.get("event_type") not in (("usage_reset", "reset_card") if target == RELIEF else ("usage_reset",))
                or not valid_clock(fact.get("at"), now)):
            continue
        refs = references([fact.get("url")])
        if not refs or any(explicit_stages[url] != {"completed"} for url in refs):
            continue
        # A later completion post for an already identified observation adds
        # evidence; it must not restart that event's waiting clock.
        if any(set(refs) & set(a["references"]) for a in occurrences):
            continue
        confirmations.append({"source": obs.source, "occurred_at": None,
            "announced_at": fact["at"], "confirmation_at": fact["at"], "discovered_at": None,
            "received_at": obs.observed_at, "time_basis": "completion_confirmation",
            "references": refs, "source_event_id": str(fact.get("id", "")) or None})

    latest_occurrence = max((a["occurred_at"] for a in occurrences), default=None)
    latest_confirmation = max((a["confirmation_at"] for a in confirmations), default=None)
    boundary = max((at for at in (latest_occurrence, latest_confirmation) if at is not None), default=None)
    selected = ([a for a in occurrences if a["occurred_at"] == boundary]
                + [a for a in confirmations if a["confirmation_at"] == boundary]) if boundary is not None else []
    occurred_at = boundary if any(a.get("occurred_at") == boundary for a in selected) else None
    basis = ("observed_reset" if occurred_at is not None else "completion_confirmation"
             if boundary is not None else "historical_background")
    refs = sorted({r for a in selected for r in a["references"]})
    # The fallback is a background age only. It has no authority to reject a
    # source as preceding/missing a completion which has not been established.
    at = boundary if boundary is not None else historical_at
    return {"at": at, "historical_at": historical_at, "basis": basis,
            "occurred_at": occurred_at, "confirmation_at": boundary if basis == "completion_confirmation" else None,
            "invalidation_at": boundary, "time_basis": basis, "references": refs,
            "identity": digest(["reset_references", refs]) if refs else
                        digest(["reset_clock", at, [(a["source"], a.get("source_event_id")) for a in selected]]),
            "observations": selected,
            "unresolved_proxies": [a for a in proxies if a["announced_at"] > (boundary or 0)
                                   and not set(a["references"]) & set(refs)]}


def cycle_issues(curve, cycle):
    """Compare occurrence clocks only; confirmations provide an upper bound."""
    boundary = cycle.get("invalidation_at")
    if boundary is None:
        return []
    issues = []
    anchor = curve_anchor(curve)
    same_event = bool(set(anchor["references"]) & set(cycle["references"]))
    # A proxy earlier than a measured occurrence proves this provider's last
    # known event is old, unless an explicit shared identity links that event.
    # A later proxy says nothing about a new occurrence, and cannot exclude peers.
    if (cycle.get("occurred_at") is not None and curve.last_reset_at is not None
            and curve.last_reset_at < boundary - 3600 and not same_event):
        issues.append("missed_latest_event")
    if boundary > curve.origin:
        issues.append("forecast_precedes_latest_event")
    return issues
