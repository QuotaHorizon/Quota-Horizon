from dataclasses import replace
import json
from pathlib import Path
import tempfile
import unittest

from reset_intelligence.completions import ingest
from reset_intelligence.contracts import AUTOMATIC, Observation
from reset_intelligence.lifecycle import live_cycle, cycle_issues
from reset_intelligence.sources import scheduled_status_facts, reset_anchor
import test_lifecycle as fixture
from test_engine import NOW, history


class CompletionInputTests(unittest.TestCase):
    setUp = fixture.LiveCycleTests.setUp
    tearDown = fixture.LiveCycleTests.tearDown
    save = fixture.LiveCycleTests.save
    def test_fresh_forecast_anchored_to_previous_reset_is_retired_on_confirmation(self):
        ref = "https://x.com/i/status/789"
        obs = Observation("didcodex_status", NOW, NOW, facts=[{
            "kind": "structured_completion", "stage": "completed", "at": NOW-100,
            "target": AUTOMATIC, "population": "broad", "references": [ref], "url": ref,
            "expires_at": NOW+3600}])
        cycle = live_cycle({obs.source: obs}, self.historical, NOW, history())
        old = replace(self.codex, last_reset_at=self.historical, origin=NOW)
        self.assertIn("missed_latest_event", cycle_issues(old, cycle))
        updated = replace(old, metadata={"reset_anchor": reset_anchor(NOW-100, "announcement_proxy", [ref])})
        self.assertEqual(cycle_issues(updated, cycle), [])

    def test_observer_revision_withdraws_direct_completion_and_later_discovery(self):
        ref = "https://x.com/i/status/789"
        payload = {"schema_version": "reset-completions/1", "generated_at": NOW,
            "events": [{"id": "event", "status": "confirmed", "kind": "automatic", "at": NOW-300,
                        "known_at": NOW-100, "references": [ref, "https://example.com/#event-one"]},
                       {"id": "other", "status": "pending", "kind": "automatic", "at": NOW-400,
                        "known_at": NOW-50, "references": ["https://example.com/#event-two"]}]}
        path = Path(self.tmp.name) / "completion.json"
        path.write_text(json.dumps(payload))
        ingest(self.store, path, NOW)
        curve = replace(self.codex, last_reset_at=NOW-100, metadata={"reset_anchor": reset_anchor(NOW-100, "source_clock", [ref])})
        self.save(curve)
        cycle = live_cycle(self.store.observations(NOW), self.historical, NOW, history())
        self.assertEqual(cycle["at"], NOW-300)
        self.assertIsNone(cycle["occurred_at"])
        payload["events"][0]["status"] = "pending"
        path.write_text(json.dumps(payload))
        ingest(self.store, path, NOW+1)
        cycle = live_cycle(self.store.observations(NOW+1), self.historical, NOW+1, history())
        self.assertEqual(cycle["at"], self.historical)
        self.assertEqual(len(list(self.store.db.execute("SELECT id FROM observations WHERE source='completion_evidence'"))), 2)

    def test_standalone_status_completion_preserves_scope_and_explicit_links(self):
        from reset_intelligence.contracts import iso
        raw = {"schemaVersion": 1, "generatedAt": iso(NOW), "lastSuccessfulCheckAt": iso(NOW),
            "monitor": {"status": "ok"}, "events": [{"kind": "reset_completed", "resetType": "global",
                "source": {"postId": "789", "url": "https://x.com/thsottiaux/status/789"}, "announcedAt": iso(NOW-100),
                "scope": {"plans": ["all"]}}], "resetTimeline": {"nextSchedule": None,
                "fulfilledSchedules": [{"completionPostId": "789", "schedule": {"resetType": "global",
                    "source": {"postId": "456", "url": "https://x.com/thsottiaux/status/456"}}}]}}
        _, facts, issues = scheduled_status_facts(raw, "didcodex_status", NOW)
        self.assertEqual(issues, [])
        self.assertEqual(facts[0]["references"], ["https://x.com/i/status/789", "https://x.com/i/status/456"])
        raw["events"][0]["scope"] = {"plans": ["pro"]}
        self.assertEqual(scheduled_status_facts(raw, "didcodex_status", NOW)[1], [])

    def test_linked_later_completion_and_discovery_do_not_restart_cycle(self):
        ref = "https://x.com/i/status/789"
        facts = [{"kind": "structured_completion", "stage": "completed", "at": at,
                  "target": AUTOMATIC, "population": "broad", "url": ref, "references": [ref],
                  "expires_at": NOW+300} for at in (NOW-300, NOW-200)]
        observations = {"completion_evidence": Observation("completion_evidence", NOW, NOW, facts=facts)}
        self.assertEqual(live_cycle(observations, self.historical, NOW, history())["at"], NOW-300)
        curves = [replace(self.codex, last_reset_at=at, metadata={"reset_anchor": reset_anchor(at, "source_clock", [ref])}) for at in (NOW-400, NOW-100)]
        observations["codex_reset"] = Observation("codex_reset", NOW, NOW, curves=curves)
        self.assertEqual(live_cycle(observations, self.historical, NOW, history())["at"], NOW-400)
