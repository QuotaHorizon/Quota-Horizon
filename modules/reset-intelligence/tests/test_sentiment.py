from copy import deepcopy
import math
import unittest

from reset_intelligence.contracts import AUTOMATIC, Curve, Observation, RELIEF
from reset_intelligence.increments import combine, counterfactuals
from reset_intelligence.pipeline import forecast, forecast_all, consumer_export
from reset_intelligence.quant import ModelConfig
from reset_intelligence.sentiment import calendar_overlay, shift_curve
import test_engine
from test_engine import NOW


def poll(reset=1000, feature=1000):
    return {"kind": "calendar_preference", "id": "day", "target": RELIEF,
            "start": NOW-8*3600, "expires": NOW+16*3600,
            "population": "calendar_clicks", "unit": "clicks", "max_votes_per_ip_24h": 100,
            "question": "reset_or_improvement_preference_and_prediction",
            "voting_open": True, "upstream_outcome": None, "observed_only": True,
            "total": reset+feature, "bins": [{"p": 1., "votes": reset}, {"p": 0., "votes": feature}]}


class SentimentMappingTests(unittest.TestCase):
    def setUp(self):
        self.hours = list(range(1, 73))
        self.informed = [1-math.exp(-h/40) for h in self.hours]
        self.config = ModelConfig()

    def mapped(self, raw=(1000, 1000), weighted=(900, 100), **changes):
        p = poll(*raw)
        p["weighted_bins"] = [{"p": 1., "votes": weighted[0]}, {"p": 0., "votes": weighted[1]}]
        p.update(changes)
        return calendar_overlay(p, {}, NOW, NOW-86400, self.informed, self.hours, self.config)

    def test_steady_optimism_is_not_recounted_as_new_information(self):
        for level in (0, .1, .5, .9, 1):
            result, reason = self.mapped((round(level*1000), round((1-level)*1000)), (level*100, (1-level)*100))
            self.assertIsNone(reason)
            self.assertAlmostEqual(result["log_odds_change"], 0)
            for actual, prior in zip(result["probabilities"], self.informed):
                self.assertAlmostEqual(actual, prior)

    def test_change_is_signed_and_does_not_claim_click_share_is_probability(self):
        up, _ = self.mapped()
        down, _ = self.mapped(weighted=(100, 900))
        self.assertGreater(up["probabilities"][23], self.informed[23])
        self.assertLess(down["probabilities"][23], self.informed[23])
        self.assertNotEqual(up["probabilities"][23], up["recent_share"])
        self.assertIsNone(up["effective_votes"])
        self.assertEqual(up["repeat_sensitivity_units"], 10)

    def test_original_deadline_preserves_conditional_tail(self):
        mapped, _ = self.mapped()
        p = mapped["probabilities"]
        self.assertAlmostEqual((1-p[47])/(1-p[23]), (1-self.informed[47])/(1-self.informed[23]))
        self.assertTrue(all(0 <= a <= b <= 1 for a, b in zip(p, p[1:])))
        for deadline in (.01, .5, 1, 1.5, 25, 72):
            curve = shift_curve(self.informed, self.hours, deadline, -.7)
            self.assertTrue(all(0 <= a <= b <= 1 for a, b in zip(curve, curve[1:])))

    def test_closed_upcoming_answered_and_small_samples_cannot_contribute(self):
        for change in ({"voting_open": False}, {"upstream_outcome": "reset"},
                       {"start": NOW+1}, {"expires": NOW}, {"total": 49}):
            result, _ = self.mapped(**change)
            self.assertIsNone(result)
        p = poll(); p["weighted_bins"] = deepcopy(p["bins"])
        self.assertIsNone(calendar_overlay(p, {}, NOW, NOW-60, self.informed, self.hours, self.config)[0])
        self.assertIsNone(calendar_overlay(p, {"largest_gap_seconds": 1801}, NOW, NOW-86400, self.informed, self.hours, self.config)[0])

    def test_ablation_rebases_innovation_without_leaking_removed_news(self):
        local = {"id": "survival_baseline", "family": "baseline", "dependency": "history", "metadata": {}, "probabilities": [.2]*72}
        source = {"id": "radar", "family": "radars", "role": "information", "dependency": "radar", "metadata": {},
                  "signal_baseline": [.2]*72, "probabilities": [.8]*72}
        m, _ = self.mapped()
        c = {"id": "calendar", "family": "community", "dependency": "clicks", "metadata": {k:v for k,v in m.items() if k != "probabilities"}, "probabilities": m["probabilities"]}
        variants = counterfactuals([local, source, c], self.config)
        self.assertGreater(variants["without_information"][23], .2)
        self.assertLess(variants["without_information"][23], .5)
        self.assertEqual(variants["without_community"], [.8]*72)
        with_floor = combine([local, source, c], self.config, informed_floor=[.9]*72)
        self.assertGreater(with_floor["probabilities"][23], .9)


class SentimentPipelineTests(unittest.TestCase):
    setUp = test_engine.LedgerTests.setUp
    tearDown = test_engine.LedgerTests.tearDown

    def automatic_source(self, expiry=NOW+3600):
        curve = Curve("automatic_only", AUTOMATIC, NOW, NOW, NOW, expiry,
                      [(24, .85), (48, .95)], metadata={
                          "baseline_points": [(24, .2), (48, .4)],
                          "revision_reviewed": True,
                      })
        self.store.save(curve.source, NOW, {}, Observation(
            curve.source, NOW, NOW, curves=[curve]))

    def test_containing_forecast_inherits_minimum_input_expiry(self):
        self.automatic_source(NOW+45)
        broad, automatic = forecast_all(self.store, NOW)
        self.assertGreater(broad["curve"][23]["probability"], .8)
        self.assertEqual(broad["valid_until"], NOW+45)
        self.assertEqual(automatic["valid_until"], NOW+45)
        after, _ = forecast_all(self.store, NOW+46)
        self.assertLess(after["curve"][23]["probability"], .5)

    def test_automatic_only_source_ablation_removes_inherited_floor_and_rebases_community(self):
        self.automatic_source()
        for minute in range(361):
            reset = 1000 + max(0, minute-180)*10
            feature = 1000 + min(minute, 180)*10
            at = NOW-(360-minute)*60
            self.store.save("didcodex_poll", at, {}, Observation(
                "didcodex_poll", at, polls=[poll(reset, feature)]))
        broad, automatic = forecast_all(self.store, NOW)
        self.assertGreater(broad["curve"][23]["probability"], automatic["curve"][23]["probability"])
        without, _ = forecast_all(self.store, NOW, source_ids={"event_ledger", "didcodex_poll"})
        variant = broad["counterfactuals"]["without:automatic_only"]
        self.assertLess(variant[23], .5)
        for actual, expected in zip(variant, without["curve"]):
            self.assertAlmostEqual(actual, expected["probability"])
        for actual, expected in zip(broad["counterfactuals"]["without_community"], automatic["curve"]):
            self.assertAlmostEqual(actual, expected["probability"])

    def test_actual_new_clicks_enter_export_and_have_matched_control(self):
        samples = [(NOW-6*3600, 1000, 1000)]
        for minute in range(1, 361):
            # Early pessimistic and later optimistic batches, never cumulative
            # totals masquerading as a recent sample.
            reset = 1000 + max(0, minute-180)*10
            feature = 1000 + min(minute, 180)*10
            samples.append((NOW-(360-minute)*60, reset, feature))
        for at, reset, feature in samples:
            self.store.save("didcodex_poll", at, {}, Observation("didcodex_poll", at, polls=[poll(reset, feature)]))
        result = forecast(self.store, NOW, RELIEF)
        c = next(c for c in result["components"] if c["id"] == "didcodex_poll")
        self.assertEqual(c["metadata"]["recent_votes"], 3600)
        self.assertGreater(result["curve"][23]["community_adjustment"], 0)
        self.assertGreater(result["curve"][23]["probability"], result["counterfactuals"]["without_community"][23])
        exported = consumer_export([result])["forecasts"][0]["community_inputs"][0]
        self.assertEqual(exported["mapping_kind"], "sentiment_innovation")
        self.assertEqual(exported["recent_votes"], 3600)
        self.assertIsNone(exported["effective_votes"])
        pair = forecast_all(self.store, NOW)
        self.assertTrue(all(a["probability"] >= b["probability"] for a,b in zip(pair[0]["curve"], pair[1]["curve"])))

    def test_first_snapshot_and_unknown_gap_do_not_invent_new_sentiment(self):
        self.store.save("didcodex_poll", NOW, {}, Observation("didcodex_poll", NOW, polls=[poll()]))
        self.assertFalse(any(c["family"] == "community" for c in forecast(self.store, NOW, RELIEF)["components"]))
        self.store.save("didcodex_poll", NOW-3600, {}, Observation("didcodex_poll", NOW-3600, polls=[poll(0, 0)]))
        self.assertFalse(any(c["family"] == "community" for c in forecast(self.store, NOW, RELIEF)["components"]))
