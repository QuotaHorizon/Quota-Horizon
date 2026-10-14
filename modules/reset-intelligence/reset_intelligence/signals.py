"""Normalize source-published comment estimates without inventing text scores.

A site may expose a cadence/tease curve and a separate commitment floor. Resolve
that floor's event window from matching public structured references, then fill
only the gap in that site's curve. The same site retains one numerical vote;
other radars already incorporating the comment are not boosted a second time.
"""
from __future__ import annotations

from .response import reference_key


def apply_comment_estimates(components, facts, now, last_event, hours):
    radars = [c for c in components if c["family"] == "radars" and not c.get("private")]
    closed = {reference_key(f["url"]) for f in facts if f["at"] <= now
              and f.get("stage") in ("completed", "cancelled", "retracted")}
    for c in radars:
        estimate = c["metadata"].get("comment_estimate")
        if not estimate:
            continue
        key = reference_key(estimate["url"])
        result = {**estimate, "included": False, "deadline": None, "timing_sources": [],
                  "adjustment_24h": 0., "adjustment_48h": 0.}
        c["metadata"]["comment"] = result
        if not last_event < estimate["at"] <= now or key in closed:
            result["state"] = "closed_or_superseded"
            continue
        timed = [(other, a) for other in radars if (a := other["metadata"].get("announcement"))
                 and reference_key(a["url"]) == key and last_event < a["at"] <= now < a["deadline"]]
        if not timed:
            result["state"] = "awaiting_event_window"
            continue
        # A later explicit deadline contains every supplied window. No guessed
        # timezone or sentence parsing; no independent vote for a copied window.
        deadline = max(a["deadline"] for _, a in timed)
        result.update(deadline=deadline, timing_sources=sorted({other["id"] for other, _ in timed}))
        if c.get("signal_baseline") is None:
            result["state"] = "awaiting_source_baseline"
            continue
        before = c["probabilities"]
        # With no within-window timing distribution, the published score is a
        # floor only for horizons containing the complete event window.
        after = [max(p, estimate["probability"]) if now+h*3600 >= deadline else p
                 for h, p in zip(hours, before)]
        c["metadata"]["pre_comment_probabilities"] = list(before)
        c["metadata"]["pre_comment_role"] = c.get("role")
        c["probabilities"] = after
        if after != before:
            c["role"] = "information"
        c["metadata"]["fresh_until"] = min(deadline, c["metadata"].get("fresh_until", deadline))
        result.update(included=True, state="included", adjustment_24h=after[23]-before[23],
                      adjustment_48h=after[47]-before[47])
