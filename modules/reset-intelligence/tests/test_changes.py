from copy import deepcopy
import json
import unittest

import test_engine as fixtures
from test_engine import NOW
from reset_intelligence.changes import explain_change, previous_forecast
from reset_intelligence.contracts import RELIEF, Curve, Observation
from reset_intelligence.pipeline import forecast
from reset_intelligence.sources import parse


class ChangesTests(unittest.TestCase):
    setUp = fixtures.LedgerTests.setUp
    tearDown = fixtures.LedgerTests.tearDown

    def test_contributions_sum_to_actual_change_including_projection_and_entry(self):
        before = forecast(self.store, NOW, RELIEF)
        curve = Curve("lunar", RELIEF, NOW, NOW, NOW, NOW + 3600, [(24,.8),(48,.9)], metadata={"baseline_points":[(24,.2),(48,.4)]})
        curve.metadata.update(adapter="lunar", adapter_contract=1)
        poll = {"kind":"probability_votes", "target":RELIEF, "population":"private", "id":"round",
                "start":NOW-3600, "expires":NOW+86400, "total":20, "bins":[{"p":.9,"votes":20}], "private":True}
        self.store.save("lunar", NOW, {}, Observation("lunar", NOW, NOW, curves=[curve]))
        self.store.save("private_identity", NOW, {}, Observation("private_identity", NOW, NOW, polls=[poll]))
        after = forecast(self.store, NOW + 60, RELIEF)
        after["curve"][23]["probability"] += .01
        change = explain_change(after, before)
        self.assertNotIn("private_identity", json.dumps(change))
        for w in change["windows"]:
            self.assertAlmostEqual(sum(d["effect"] for d in w["drivers"]) + w["coherence_effect"], w["after"] - w["before"])
            entered = next(d for d in w["drivers"] if d["id"] == "radar_revision")
            self.assertEqual(entered["before"], 0)
            self.assertGreater(entered["input_effect"], 0)
            self.assertEqual(entered["weight_effect"], 0)
        self.assertAlmostEqual(change["windows"][0]["coherence_effect"], .01)

    def test_version_boundaries_and_future_comparisons_are_excluded(self):
        before = forecast(self.store, NOW, RELIEF)
        self.store.forecast(before)
        after = deepcopy(before); after["issued_at"] += 60
        self.assertEqual(previous_forecast(self.store, after)["issued_at"], NOW)
        after["config_hash"] = "new version"
        self.assertIsNone(previous_forecast(self.store, after))
        self.assertIsNone(explain_change(after, before))
        self.assertIsNone(explain_change(before, before))

    def test_conditional_daily_pledge_is_collected_without_becoming_an_outcome(self):
        obs = parse({"id":"tibo_posts", "adapter":"tibo_posts"}, {"fetched_at": NOW, "tweets":[
            {"id":"123", "url":"https://x.com/thsottiaux/status/123", "at":NOW,
             "text":"For the next 28 days, each day we either ship improvements or ship a full reset.",
             "translation_status":"translated", "localized_text":"未来 28 天，每天发布改进或完整重置。"}]}, NOW)
        self.assertEqual(obs.facts[0]["relevance"], "conditional_pledge")
        self.assertEqual(obs.facts[0]["stage"], "conditional")
        self.assertEqual(obs.events, [])
        ledger = parse({"id":"event_ledger", "adapter":"ledger"}, {"meta":{"generated_at":NOW},
            "pagination":{"has_more":False}, "data":[{"id":"123", "text":obs.facts[0]["original"],
              "reset_type":"regular","announced_at":NOW,"source":{"url":"https://x.com/i/status/123","type":"social"}}]}, NOW)
        self.assertEqual(ledger.events, [])
        self.assertEqual(ledger.facts[0]["reason"], "conditional_pledge")
