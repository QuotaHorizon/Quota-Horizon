from copy import deepcopy
import unittest
from unittest.mock import patch

import test_engine as fixtures
from test_engine import NOW
from reset_intelligence.contracts import AUTOMATIC, RELIEF, Curve, Observation
from reset_intelligence.pipeline import forecast
from reset_intelligence.sources import collect, parse


class PublishedRevisionTests(unittest.TestCase):
    setUp, tearDown = fixtures.LedgerTests.setUp, fixtures.LedgerTests.tearDown

    def payload(self):
        return {"updated_at": NOW, "last_reset_at": NOW - 96*3600,
                "model": {"version": "rate-v3"},
                "context": {"effect": "informational_only", "multiplier": 1},
                "probabilities": {"raw_24h": .21831990584656782, "raw_48h": .3889762304042814,
                    "rounded_24h": 22, "rounded_48h": 39, "model_24h": 22, "model_48h": 39,
                    "tease_tier": None, "tease_floor_24h": None, "tease_floor_48h": None}}

    def save(self, raw):
        obs = parse({"id": "codex_reset", "adapter": "codex_reset"}, raw, NOW)
        self.store.save("codex_reset", NOW, raw, obs)
        return obs

    def codex(self, result):
        return next(c for c in result["components"] if c["id"] == "codex_reset")

    def test_rounding_preserves_public_values_without_diluting_news(self):
        raw = self.payload()
        saved = deepcopy(raw)
        self.save(raw)
        signal = Curve("measured_news", AUTOMATIC, NOW, NOW, NOW, NOW+3600,
                       [(24, .7), (48, .8)], metadata={"baseline_points": [(24, .2), (48, .35)]})
        self.store.save(signal.source, NOW, {}, Observation(signal.source, NOW, NOW, curves=[signal]))
        result = forecast(self.store, NOW, AUTOMATIC)
        source = self.codex(result)
        self.assertEqual(source["role"], "background")
        self.assertEqual(source["weight"], 0.)
        self.assertEqual(source["baseline_weight"], .5)
        self.assertEqual(source["source_points"], [[24, .22], [48, .39]])
        self.assertEqual(source["metadata"]["baseline_points"], [[24, raw["probabilities"]["raw_24h"]], [48, raw["probabilities"]["raw_48h"]]])
        self.assertAlmostEqual(result["curve"][23]["radar_adjustment"], .5)
        self.assertEqual(raw, saved)

    def test_explicit_adjustments_and_incomplete_metadata_remain_information(self):
        for change in ("positive", "negative", "tiny_declared", "tease_floor", "tease_tier", "missing_context", "missing_model", "noninteger_model"):
            with self.subTest(change=change):
                raw = self.payload()
                if change == "positive": raw["probabilities"]["rounded_24h"] = 30
                if change == "negative": raw["probabilities"]["rounded_24h"] = 15
                if change == "tiny_declared": raw["context"].update(effect="increase", multiplier=1.001)
                if change == "tease_floor": raw["probabilities"]["tease_floor_24h"] = 22
                if change == "tease_tier": raw["probabilities"]["tease_tier"] = "weak"
                if change == "missing_context": raw.pop("context")
                if change == "missing_model": raw["probabilities"].pop("model_24h")
                if change == "noninteger_model": raw["probabilities"].update(rounded_24h=21.9, model_24h=21.9)
                self.save(raw)
                result = forecast(self.store, NOW, AUTOMATIC)
                source = self.codex(result)
                self.assertEqual(source["role"], "information")
                if change in ("positive", "negative"):
                    self.assertAlmostEqual(result["curve"][23]["radar_adjustment"], raw["probabilities"]["rounded_24h"]/100-raw["probabilities"]["raw_24h"])

    def test_commitment_still_promotes_same_source_above_rounding_background(self):
        raw = self.payload()
        url = "https://x.com/i/status/1234"
        raw.update(mode="announced", official_signal={"url": url, "at": NOW-3600},
                   latest_alert={"state": "active", "url": url})
        raw["probabilities"].update(commitment_floor_percent=93, commitment=.93)
        self.save(raw)
        signal = Curve("measured_news", AUTOMATIC, NOW, NOW, NOW, NOW+3600,
                       [(24, .96), (48, .98)], metadata={"baseline_points": [(24, .2), (48, .35)],
                         "announcement": {"url": url, "at": NOW-3600, "deadline": NOW+4*3600}})
        self.store.save(signal.source, NOW, {}, Observation(signal.source, NOW, NOW, curves=[signal]))
        source = self.codex(forecast(self.store, NOW, AUTOMATIC))
        self.assertEqual(source["role"], "information")
        self.assertEqual(source["metadata"]["pre_comment_role"], "background")
        self.assertEqual(source["probabilities"][23], .93)
        self.assertEqual(source["weight"], .5)
        self.assertEqual(source["source_points"], [[24, .22], [48, .39]])

    def test_old_normalized_rounding_contract_refreshes_without_rewriting_history(self):
        raw = self.payload()
        obs = self.save(raw)
        obs.curves[0].metadata.pop("adapter_contract")
        self.store.save("codex_reset", NOW, raw, obs)
        before = [tuple(r) for r in self.store.db.execute("SELECT * FROM observations ORDER BY id")]
        self.assertNotIn("codex_reset", [c["id"] for c in forecast(self.store, NOW, AUTOMATIC)["components"]])
        spec = {"id": "codex_reset", "adapter": "codex_reset"}
        fresh = parse(spec, raw, NOW+1)
        with patch("reset_intelligence.sources.time.time", return_value=NOW+1), patch(
            "reset_intelligence.sources.fetch", return_value=(spec, NOW+1, raw, fresh, None)
        ) as fetch:
            collect(self.store, [spec])
            fetch.assert_called_once_with(spec)
        self.assertEqual(self.codex(forecast(self.store, NOW+1, AUTOMATIC))["role"], "background")
        self.assertEqual([tuple(r) for r in self.store.db.execute("SELECT * FROM observations ORDER BY id LIMIT ?", (len(before),))], before)


class LunarDeadlineCollectionTests(unittest.TestCase):
    setUp, tearDown = fixtures.LedgerTests.setUp, fixtures.LedgerTests.tearDown

    def set_lunar(self, generated_age, source="lunar"):
        generated = NOW-generated_age
        self.spec = {"id": source, "adapter": "lunar"}
        self.curve = Curve(source, RELIEF, generated, generated, NOW, generated+600, [(6, .08), (24, .38)],
                           metadata={"adapter": "lunar", "adapter_contract": 1, "decomposition_version": 1,
                                     "response_version": 2, "baseline_points": [(6, .08), (24, .38)]})
        self.obs = Observation(source, NOW, generated, curves=[self.curve])
        self.store.save(source, NOW, {}, self.obs)

    def attempt(self, elapsed, interval=None, error=None):
        with patch("reset_intelligence.sources.time.time", return_value=NOW+elapsed), patch(
            "reset_intelligence.sources.fetch",
            return_value=(self.spec, NOW+elapsed, {} if not error else None,
                          self.obs if not error else None, error)
        ) as fetch:
            outcome = collect(self.store, [self.spec], minimum_interval=interval)[0]
            return outcome["status"], fetch.call_count

    def test_aged_receipt_fetches_before_deadline_without_extending_it(self):
        self.set_lunar(487)
        self.assertEqual(self.attempt(59), ("cached", 0))
        self.assertEqual(self.attempt(60), ("collected", 1))
        self.assertEqual(self.curve.expires_at, NOW+113)
        # Even if the server returns the same old content, never spin or keep
        # its expired numerical contribution alive by refreshing its receipt.
        self.assertEqual(self.attempt(119), ("cached", 0))
        self.assertNotIn("lunar", [c["id"] for c in forecast(self.store, NOW+119, RELIEF)["components"]])
        self.assertEqual(self.attempt(120), ("collected", 1))

    def test_fresh_receipt_uses_normal_interval_and_custom_source_uses_same_deadline(self):
        self.set_lunar(0)
        self.assertEqual(self.attempt(299), ("cached", 0))
        self.assertEqual(self.attempt(300), ("collected", 1))
        self.set_lunar(420, source="custom_lunar")
        self.assertEqual(self.attempt(119), ("cached", 0))
        self.assertEqual(self.attempt(120), ("collected", 1))

    def test_manual_refresh_guard_and_failure_backoff_take_precedence(self):
        self.set_lunar(487)
        self.assertEqual(self.attempt(4, interval=5), ("cached", 0))
        self.assertEqual(self.attempt(5, interval=5), ("collected", 1))
        self.assertEqual(self.attempt(65, error="http_429"), ("error", 1))
        self.assertEqual(self.attempt(964), ("cached", 0))
        self.assertEqual(self.attempt(965, error="URLError"), ("error", 1))
        self.assertEqual(self.attempt(1084), ("cached", 0))
        self.assertEqual(self.attempt(1085), ("collected", 1))
