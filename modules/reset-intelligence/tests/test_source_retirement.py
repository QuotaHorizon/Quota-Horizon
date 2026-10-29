import unittest
from unittest.mock import patch

import test_engine as fixtures
from test_engine import NOW
from reset_intelligence.contracts import Observation, RELIEF
from reset_intelligence.pipeline import forecast
from reset_intelligence.sources import CATALOG, collect, configured_sources, parse


LEGACY = {"id": "statements", "adapter": "statements",
          "url": "https://codex-reset.pro/api/reset-feed"}


class SourceRetirementTests(unittest.TestCase):
    setUp = fixtures.LedgerTests.setUp
    tearDown = fixtures.LedgerTests.tearDown

    def test_malformed_custom_url_remains_an_isolated_source_failure(self):
        custom = {**LEGACY, "url": "https://[invalid/statements"}
        self.assertIn(custom, configured_sources([custom]))
        with patch("reset_intelligence.sources.get_json") as request:
            result = collect(self.store, [custom], minimum_interval=0)
        request.assert_not_called()
        self.assertEqual(result[0]["error"], "ValueError")

    def test_default_and_old_config_stop_polling_retired_endpoint(self):
        self.assertNotIn(LEGACY, CATALOG)
        old = {**LEGACY, "url": LEGACY["url"] + "/?locale=zh"}
        self.assertFalse(any(s["id"] == "statements" for s in configured_sources([old])))
        with patch("reset_intelligence.sources.fetch") as fetch:
            self.assertEqual(collect(self.store, [old], minimum_interval=0),
                             [{"source": "statements", "status": "retired"}])
        fetch.assert_not_called()

    def test_retirement_preserves_receipts_and_excludes_obsolete_health(self):
        raw = {"generatedAt": NOW - 10, "posts": [{"id": "saved", "at": NOW - 20,
            "url": "https://example.test/saved", "original": "Preserved source text"}]}
        self.store.save("statements", NOW - 10, raw, parse(LEGACY, raw, NOW - 10))
        self.store.save("statements", NOW - 1, None, None, "http_404")
        before = list(self.store.db.execute("SELECT * FROM observations ORDER BY id"))
        collect(self.store, [LEGACY], minimum_interval=0)
        ids = {s["id"] for s in configured_sources()}
        result = forecast(self.store, NOW, RELIEF, source_ids=ids)
        self.assertNotIn("statements", [h["id"] for h in result["source_health"]])
        self.assertEqual([tuple(r) for r in before], [tuple(r) for r in self.store.db.execute("SELECT * FROM observations ORDER BY id")])
        self.assertEqual(self.store.observations(NOW)["statements"].facts[0]["original"], "Preserved source text")

    def test_custom_statement_feed_and_current_posts_remain_enabled(self):
        custom = {**LEGACY, "url": "https://example.test/statements"}
        specs = configured_sources([custom], {"lunar"})
        self.assertIn(custom, specs)
        self.assertIn("tibo_posts", {s["id"] for s in specs})
        self.assertNotIn("lunar", {s["id"] for s in specs})
        obs = Observation("statements", NOW, NOW)
        with patch("reset_intelligence.sources.fetch", return_value=(custom, NOW, {}, obs, None)) as fetch:
            self.assertEqual(collect(self.store, [custom])[0]["status"], "collected")
        fetch.assert_called_once_with(custom)
