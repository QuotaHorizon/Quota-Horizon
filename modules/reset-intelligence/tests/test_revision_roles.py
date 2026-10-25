import unittest

from reset_intelligence.contracts import AUTOMATIC, Curve, Observation
from reset_intelligence.pipeline import forecast
import test_engine
from test_engine import NOW


class RevisionRoleTests(unittest.TestCase):
    setUp = test_engine.LedgerTests.setUp
    tearDown = test_engine.LedgerTests.tearDown

    def source(self, name, points, **metadata):
        adapter = "monitor" if name == "monitor" else "codex_reset"
        curve = Curve(name, AUTOMATIC, NOW, NOW, NOW, NOW + 3600, points,
                      dependency=name, metadata={
                          "adapter": adapter, "adapter_contract": 1,
                          "estimate_kind": "published_complete",
                          "baseline_points": [(24, .2), (48, .35)],
                          **metadata,
                      })
        self.store.save(name, NOW, {}, Observation(name, NOW, NOW, curves=[curve]))

    def test_reviewed_zero_revision_does_not_dilute_informed_source(self):
        self.source("codex_reset", [(24, .83), (48, .9)])
        without = forecast(self.store, NOW, AUTOMATIC)
        self.source("monitor", [(24, .2), (48, .35)], revision_reviewed=True)
        result = forecast(self.store, NOW, AUTOMATIC)
        monitor = next(c for c in result["components"] if c["id"] == "monitor")
        self.assertEqual(monitor["role"], "background")
        self.assertEqual(monitor["weight"], 0)
        self.assertGreater(monitor["baseline_weight"], 0)
        for actual, expected in zip(result["curve"], without["curve"]):
            self.assertAlmostEqual(actual["probability"], expected["probability"])
        self.assertAlmostEqual(result["curve"][23]["probability"], .83)

    def test_small_positive_and_negative_revisions_remain_information(self):
        for delta in (1e-8, -1e-8, -.1):
            with self.subTest(delta=delta):
                self.source("monitor", [(24, .2 + delta), (48, .35 + delta)],
                            revision_reviewed=True)
                result = forecast(self.store, NOW, AUTOMATIC)
                monitor = next(c for c in result["components"] if c["id"] == "monitor")
                self.assertEqual(monitor["role"], "information")
                self.assertEqual(monitor["weight"], 1)
                self.assertAlmostEqual(result["curve"][23]["radar_adjustment"], delta)

    def test_review_metadata_does_not_override_explicit_rounding_only(self):
        self.source("monitor", [(24, .201), (48, .351)],
                    revision_reviewed=True, rounding_only=True)
        result = forecast(self.store, NOW, AUTOMATIC)
        monitor = next(c for c in result["components"] if c["id"] == "monitor")
        self.assertEqual(monitor["role"], "background")
        self.assertEqual(result["curve"][23]["radar_adjustment"], 0)

    def test_published_commitment_promotes_background_after_actual_supplement(self):
        url = "https://x.com/i/status/1234"
        self.source("codex_reset", [(24, .2), (48, .35)],
                    comment_estimate={"url": url, "at": NOW - 60, "probability": .83,
                                      "kind": "published_commitment_floor"})
        self.source("monitor", [(24, .2), (48, .35)], revision_reviewed=True,
                    announcement={"url": url, "at": NOW - 60, "deadline": NOW + 12 * 3600})
        result = forecast(self.store, NOW, AUTOMATIC)
        source = next(c for c in result["components"] if c["id"] == "codex_reset")
        monitor = next(c for c in result["components"] if c["id"] == "monitor")
        self.assertEqual(source["metadata"]["pre_comment_role"], "background")
        self.assertEqual(source["role"], "information")
        self.assertTrue(source["metadata"]["comment"]["included"])
        self.assertEqual(source["weight"], 1)
        self.assertEqual(monitor["weight"], 0)
        self.assertAlmostEqual(result["curve"][23]["probability"], .83)


if __name__ == "__main__":
    unittest.main()
