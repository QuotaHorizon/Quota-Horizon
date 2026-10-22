"""Normalize source-published comment estimates without inventing text scores.

A site may expose a cadence/tease curve and a separate commitment floor. Resolve
that floor's event window from matching public structured references, then fill
only the gap in that site's curve. The same site retains one numerical vote;
other radars already incorporating the comment are not boosted a second time.
"""
from __future__ import annotations

from .response import reference_key


def direct_reply_links(facts, now, last_event):
    """Keep verified direct references; a thread is not one forecast event.

    Facts are normalized public statements, not text classifications. Conflicting
    parents and cycles cannot establish identity. We inspect ancestors only to
    reject cycles, never to join cousins or transitive replies for scoring.
    """
    links = {}
    for fact in facts:
        parent = fact.get("parent")
        if (fact.get("kind") != "statement" or not isinstance(parent, dict)
                or not last_event < fact["at"] <= now or fact.get("is_reply") is False
                or not parent.get("url")):
            continue
        child_key, parent_key = reference_key(fact["url"]), reference_key(parent["url"])
        if (not child_key.startswith("x:") or not parent_key.startswith("x:")
                or (fact.get("id") is not None and str(fact["id"]) != child_key[2:])
                or (parent.get("id") is not None and str(parent["id"]) != parent_key[2:])):
            continue
        key = (child_key, parent_key)
        evidence = {"child_url": fact["url"], "parent_url": parent["url"],
                    "at": fact["at"], "source": fact.get("source")}
        links.setdefault(key, [])
        if evidence not in links[key]:
            links[key].append(evidence)
    parents = {}
    for child, parent in links:
        parents.setdefault(child, set()).add(parent)
    # A conflicting record is unusable even if one candidate matches our event.
    direct = {child: next(iter(values)) for child, values in parents.items() if len(values) == 1}
    cyclic = set()
    for child in direct:
        path, positions, current = [], {}, child
        while current in direct and current not in positions:
            positions[current] = len(path)
            path.append(current)
            current = direct[current]
        if current in positions:
            cyclic.update(path[positions[current]:])
    return {child: {"parent": parent, "evidence": links[child, parent]}
            for child, parent in direct.items() if child not in cyclic}


def timing_link(estimate, announcement, links):
    estimate_key, timing_key = reference_key(estimate["url"]), reference_key(announcement["url"])
    if estimate_key == timing_key:
        return {"relationship": "same_reference", "estimate_url": estimate["url"],
                "timing_url": announcement["url"], "links": []}
    for child_key, parent_key, child_at, parent_at in (
        (timing_key, estimate_key, announcement["at"], estimate["at"]),
        (estimate_key, timing_key, estimate["at"], announcement["at"]),
    ):
        link = links.get(child_key)
        if link and link["parent"] == parent_key and parent_at <= child_at:
            # The direct reply must be the same dated statement, allowing the
            # minute precision used by some providers. An unrelated/stale row
            # with a reused reference must not supply the relationship.
            evidence = [e for e in link["evidence"] if abs(e["at"] - child_at) <= 60]
            if evidence:
                return {"relationship": "direct_reply", "estimate_url": estimate["url"],
                        "timing_url": announcement["url"], "links": evidence}
    return None


def apply_comment_estimates(components, facts, now, last_event, hours):
    radars = [c for c in components if c["family"] == "radars" and not c.get("private")]
    links = direct_reply_links(facts, now, last_event)
    closed = {reference_key(f["url"]) for f in facts if f["at"] <= now
              and f.get("stage") in ("completed", "cancelled", "retracted")}
    for c in radars:
        estimate = c["metadata"].get("comment_estimate")
        if not estimate:
            continue
        key = reference_key(estimate["url"])
        result = {**estimate, "included": False, "deadline": None, "timing_sources": [], "timing_evidence": [],
                  "adjustment_24h": 0., "adjustment_48h": 0.}
        c["metadata"]["comment"] = result
        if not last_event < estimate["at"] <= now or key in closed:
            result["state"] = "closed_or_superseded"
            continue
        timed = [(other, a, link) for other in radars if (a := other["metadata"].get("announcement"))
                 and last_event < a["at"] <= now < a["deadline"]
                 and (link := timing_link(estimate, a, links)) is not None]
        # Closure applies to this matched pair, not arbitrary siblings in a
        # conversation. A cancelled timing reply also closes its matched root.
        if any(reference_key(a["url"]) in closed for _, a, _ in timed):
            result.update(state="closed_or_superseded",
                          timing_evidence=[{"source": other["id"], "deadline": a["deadline"], **link}
                                           for other, a, link in timed])
            continue
        if not timed:
            result["state"] = "awaiting_event_window"
            continue
        # A later explicit deadline contains every supplied window. No guessed
        # timezone or sentence parsing; no independent vote for a copied window.
        deadline = max(a["deadline"] for _, a, _ in timed)
        result.update(deadline=deadline, timing_sources=sorted({other["id"] for other, _, _ in timed}),
                      timing_evidence=[{"source": other["id"], "deadline": a["deadline"], **link}
                                       for other, a, link in timed])
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
