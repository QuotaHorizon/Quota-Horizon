"""Read-only outcome input from the independent evaluator."""
import json
from pathlib import Path

from .contracts import AUTOMATIC, RELIEF, Observation, event_reference, stamp


def ingest(store, path, now):
    raw = json.loads(Path(path).read_text(encoding="utf-8"))
    generated = stamp(raw["generated_at"])
    if raw.get("schema_version") != "reset-completions/1" or not 0 <= now - generated <= 300:
        raise ValueError("invalid or stale completion inventory")
    facts = []
    for event in raw["events"]:
        target = {"automatic": AUTOMATIC, "banked": RELIEF}.get(event["kind"])
        at, known = stamp(event["at"]), stamp(event["known_at"])
        if max(at, known) > now or target is None:
            continue
        refs = sorted({event_reference(url) for url in event["references"]})
        if not refs:
            continue
        facts.append({"kind": "structured_completion", "id": event["id"], "url": refs[0],
            "references": refs, "at": at, "target": target, "population": "broad",
            "stage": "completed" if event["status"] == "confirmed" else event["status"],
            "source": "completion_evidence", "generated_at": generated, "expires_at": generated + 300})
    observation = Observation("completion_evidence", now, generated, facts=facts)
    store.save(observation.source, now, raw, observation)
