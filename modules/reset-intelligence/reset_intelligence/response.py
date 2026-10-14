"""Source-provided announcement context for reading, never numerical weighting."""
from __future__ import annotations

from urllib.parse import urlsplit


def reference_key(url):
    parts = urlsplit(url).path.strip("/").split("/")
    if urlsplit(url).hostname in ("x.com", "twitter.com") and len(parts) == 3 and parts[1] == "status" and parts[2].isdigit():
        return "x:" + parts[2]
    return url


def announcement_context(components, facts, now, last_event):
    """Keep a live upstream promise readable without selecting probability inputs.

    No component, numerical weight, vote window or source probability is changed.
    The deadline and classification come from the source's structured contract;
    this module neither recognizes sentence patterns nor assigns prose a score.
    """
    radars = [c for c in components if c["family"] == "radars" and not c.get("private")]
    closed = {reference_key(f["url"]) for f in facts if f["at"] <= now
              and f.get("stage") in ("completed", "cancelled", "retracted")}
    events = [a for c in radars if (a := c["metadata"].get("announcement"))
              and last_event < a["at"] <= now < a["deadline"] and reference_key(a["url"]) not in closed]
    if not events:
        return None
    primary = max(events, key=lambda e: e["at"])
    reviewed = [c["id"] for c in radars if reference_key(primary["url"]) in
                {reference_key(r) for r in c["metadata"].get("reviewed_references", [])}]
    return {**primary, "kind": "announcement", "reviewed": sorted(reviewed), "changed": []}
