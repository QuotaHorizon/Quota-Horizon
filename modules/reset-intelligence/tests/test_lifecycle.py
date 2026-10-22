from dataclasses import replace
from pathlib import Path
import tempfile
import unittest

import numpy as np

from reset_intelligence.contracts import AUTOMATIC, RELIEF, Curve, Observation
from reset_intelligence.community import ballot_curve
from reset_intelligence.lifecycle import live_cycle
from reset_intelligence.pipeline import forecast_all
from reset_intelligence.quant import ModelConfig, Survival
from reset_intelligence.store import Store
from test_engine import NOW, history


class LiveCycleTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.store = Store(Path(self.tmp.name) / "state.sqlite")
        self.historical = history()[-1].at
        self.reset = NOW - 600
        self.store.save("event_ledger", NOW, {}, Observation(
            "event_ledger", NOW, NOW, events=history()))
        self.codex = Curve("codex_reset", AUTOMATIC, NOW, NOW, NOW, NOW + 3600,
            [(24, .32), (48, .53)], last_reset_at=self.reset,
            metadata={"adapter": "codex_reset", "adapter_contract": 1,
                      "estimate_kind": "published_complete", "rounding_only": True,
                      "baseline_points": [(24, .32), (48, .53)]})

    def tearDown(self):
        self.store.close()
        self.tmp.cleanup()

    def save(self, curve, at=NOW):
        self.store.save(curve.source, at, {}, Observation(
            curve.source, at, curve.generated_at, curves=[curve]))

    def test_operator_reset_retires_old_high_forecasts_for_both_targets(self):
        monitor = Curve("monitor", AUTOMATIC, NOW - 7200, NOW - 7200, NOW,
            NOW + 3600, [(24, .94), (48, .97)], last_reset_at=self.historical,
            metadata={"adapter": "monitor", "adapter_contract": 1,
                      "revision_reviewed": True, "baseline_points": [(24, .2), (48, .4)]})
        self.save(monitor)
        before = forecast_all(self.store, NOW)
        self.assertGreater(before[1]["curve"][23]["probability"], .9)
        self.save(self.codex)
        after = forecast_all(self.store, NOW)
        for f in after:
            self.assertEqual(f["status"], "current")
            self.assertEqual(f["last_event_at"], self.reset)
            self.assertEqual(f["event_count"], 20)
            self.assertNotIn("monitor", [c["id"] for c in f["components"]])
            self.assertLess(f["curve"][23]["probability"], .5)
            self.assertEqual(f["components"][0]["metadata"]["intervals"], 19)
        health = next(h for h in after[1]["source_health"] if h["id"] == "monitor")
        self.assertIn("forecast_precedes_latest_event", health["issues"])
        self.assertIn("missed_latest_event", health["issues"])
        # A provider that has incorporated the new event is admitted again.
        self.save(replace(monitor, origin=NOW, generated_at=NOW,
                          last_reset_at=self.reset, points=[(24, .3), (48, .5)]), NOW + 1)
        self.assertIn("monitor", [c["id"] for c in forecast_all(self.store, NOW+1)[1]["components"]])

    def test_new_receipt_or_publication_cannot_freshen_old_reset_anchor(self):
        self.save(self.codex)
        old = replace(self.codex, source="stale", points=[(24, .94), (48, .97)],
                      last_reset_at=self.historical, metadata={})
        self.save(old)
        f = forecast_all(self.store, NOW)[1]
        self.assertNotIn("stale", [c["id"] for c in f["components"]])
        self.assertIn("missed_latest_event", next(h for h in f["source_health"] if h["id"] == "stale")["issues"])

    def test_known_occurrence_survives_forecast_expiry_and_request_failure(self):
        self.save(replace(self.codex, origin=NOW-600, generated_at=NOW-600, expires_at=NOW-1))
        self.store.save("codex_reset", NOW+1, None, None, "http_503")
        f = forecast_all(self.store, NOW+1)[1]
        self.assertEqual(f["last_event_at"], self.reset)
        self.assertNotIn("codex_reset", [c["id"] for c in f["components"]])

    def test_replay_uses_known_at_and_allows_source_corrections(self):
        before = forecast_all(self.store, NOW)
        self.save(self.codex, NOW+1)
        self.assertEqual(forecast_all(self.store, NOW), before)
        self.assertEqual(forecast_all(self.store, NOW+1)[1]["last_event_at"], self.reset)
        self.save(replace(self.codex, last_reset_at=self.historical), NOW+2)
        self.assertEqual(forecast_all(self.store, NOW+2)[1]["last_event_at"], self.historical)

    def test_only_trusted_completed_automatic_anchors_renew_the_cycle(self):
        for c in (
            replace(self.codex, last_reset_at=NOW+10),
            replace(self.codex, generated_at=NOW+120),
            replace(self.codex, private=True),
            replace(self.codex, target=RELIEF),
            replace(self.codex, issues=["unhealthy_outcome_history"]),
            replace(self.codex, source="custom", metadata={}),
        ):
            with self.subTest(curve=c):
                obs = Observation(c.source, NOW, NOW, curves=[c])
                self.assertEqual(live_cycle({c.source: obs}, self.historical, NOW)["at"], self.historical)

    def test_reset_changes_waiting_age_not_historical_training_events(self):
        times = [e.at for e in history()]
        old = Survival(times, NOW)
        new = Survival(times, NOW, renewal_at=self.reset)
        self.assertEqual(new.intervals, old.intervals)
        np.testing.assert_array_equal(new.alpha, old.alpha)
        self.assertAlmostEqual(new.age, 1/6)
        with self.assertRaises(ValueError):
            Survival(times, NOW, renewal_at=NOW+1)

    def test_ballot_opened_before_recent_completion_does_not_predict_next_reset(self):
        p = {"kind": "probability_votes", "start": self.reset-60,
             "expires": NOW+86400, "id": "today", "total": 100,
             "bins": [{"p": .95, "votes": 100}]}
        self.assertIsNone(ballot_curve(p, NOW, [24], Survival([e.at for e in history()], NOW),
                                      self.reset, ModelConfig()))


if __name__ == "__main__":
    unittest.main()
