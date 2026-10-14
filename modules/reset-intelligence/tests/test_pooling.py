from copy import deepcopy
import json
import unittest

import test_engine as fixtures
from test_engine import NOW
from reset_intelligence.community import parse_count_market, count_rejection
from reset_intelligence.contracts import AUTOMATIC, RELIEF, Curve, Observation, stamp
from reset_intelligence.pipeline import consumer_export, forecast
from reset_intelligence.rounds import reviewed_window, select_round
from reset_intelligence.sources import parse
from unittest.mock import patch


class PoolingTests(unittest.TestCase):
    setUp, tearDown = fixtures.LedgerTests.setUp, fixtures.LedgerTests.tearDown

    def test_complete_source_forecast_drives_pool_and_keeps_baseline_separate(self):
        raw = {"updated_at": NOW, "last_reset_at": NOW - 96*3600,
               "probabilities": {"raw_24h": .15, "raw_48h": .29, "rounded_24h": 45, "rounded_48h": 70},
               "model": {"version": "upstream-v3"}}
        obs = parse({"id": "codex_reset", "adapter": "codex_reset"}, raw, NOW)
        self.store.save("codex_reset", NOW, raw, obs)
        first = forecast(self.store, NOW, AUTOMATIC)
        radar = next(c for c in first["components"] if c["id"] == "codex_reset")
        self.assertEqual(radar["source_points"], [[24, .45], [48, .7]])
        self.assertEqual(radar["metadata"]["baseline_points"], [[24, .15], [48, .29]])
        self.assertAlmostEqual(radar["weight"], 1.)
        self.assertAlmostEqual(first["curve"][23]["probability"], .45)
        self.assertIn("published-v2", obs.curves[0].metadata["source_model"])
        raw["probabilities"]["rounded_24h"] = 55
        self.store.save("codex_reset", NOW, raw, parse({"id":"codex_reset", "adapter":"codex_reset"}, raw, NOW))
        second = forecast(self.store, NOW, AUTOMATIC)
        self.assertAlmostEqual(second["curve"][23]["probability"]-first["curve"][23]["probability"], .1)

    def test_statements_never_add_a_separate_probability_boost(self):
        before = forecast(self.store, NOW, RELIEF)
        self.store.save("statements", NOW, {}, Observation("statements", NOW, NOW, facts=[
            {"kind":"statement", "id":"123", "at":NOW, "original":"Reset tomorrow! Or maybe next month."}]))
        after = forecast(self.store, NOW, RELIEF)
        self.assertEqual(before["curve"], after["curve"])
        self.assertEqual(before["training"]["weights"], after["training"]["weights"])

    def test_upgrade_refreshes_legacy_baseline_cache_and_never_labels_it_complete(self):
        from reset_intelligence.sources import collect
        c = Curve("codex_reset", AUTOMATIC, NOW, NOW, NOW, NOW+3600, [(24,.15),(48,.29)])
        legacy = Observation("codex_reset",NOW,NOW,curves=[c])
        self.store.save("codex_reset",NOW,{},legacy)
        f = forecast(self.store,NOW,AUTOMATIC)
        self.assertNotIn("codex_reset",[c["id"] for c in f["components"]])
        self.assertIn("source_normalization_outdated",next(h for h in f["source_health"] if h["id"]=="codex_reset")["issues"])
        spec = {"id":"codex_reset","adapter":"codex_reset"}
        with patch("reset_intelligence.sources.time.time",return_value=NOW), patch("reset_intelligence.sources.fetch",return_value=(spec,NOW,None,None,"URLError")) as fetch:
            collect(self.store,[spec],minimum_interval=900)
            fetch.assert_called_once()
            collect(self.store,[spec],minimum_interval=900)
            fetch.assert_called_once()  # failed refresh keeps the minute backoff

    def test_eligible_votes_have_bounded_mass_and_deadline_reduces_it(self):
        curve = Curve("lunar", RELIEF, NOW, NOW, NOW, NOW+3600, [(24,.4),(48,.6)])
        self.store.save("lunar", NOW, {}, Observation("lunar", NOW, NOW, curves=[curve]))
        poll = {"kind":"probability_votes", "target":RELIEF, "population":"voters", "id":"r",
                "start":NOW, "expires":NOW+86400, "total":50, "bins":[{"p":.9,"votes":50}]}
        self.store.save("date_poll", NOW, {}, Observation("date_poll", NOW, NOW, polls=[poll]))
        a = forecast(self.store, NOW, RELIEF)
        c = next(c for c in a["components"] if c["family"] == "community")
        self.assertAlmostEqual(c["probabilities"][23], .9)
        self.assertLess(c["weight"], .6)
        poll["expires"] = NOW+6*3600
        self.store.save("date_poll", NOW, {}, Observation("date_poll", NOW, NOW, polls=[poll]))
        b = forecast(self.store, NOW, RELIEF)
        self.assertLess(b["community_weight"], a["community_weight"] / 4)
        self.assertAlmostEqual(sum(c["baseline_weight"] for c in b["components"]), 1.)
        exported = consumer_export([b])["forecasts"][0]
        self.assertEqual(exported["community_inputs"][0]["votes"], 50)
        self.assertAlmostEqual(exported["community_inputs"][0]["weight"], exported["community"]["weight"])

    def test_mirrored_radars_cannot_increase_weight_or_change_forecast(self):
        c = Curve("lunar", RELIEF, NOW, NOW, NOW, NOW+3600, [(24,.6),(48,.8)])
        self.store.save("lunar", NOW, {}, Observation("lunar", NOW, NOW, curves=[c]))
        a = forecast(self.store, NOW, RELIEF)
        clone = deepcopy(c); clone.source = "mirror"
        self.store.save("mirror", NOW, {}, Observation("mirror", NOW, NOW, curves=[clone]))
        b = forecast(self.store, NOW, RELIEF)
        self.assertEqual(a["curve"], b["curve"])
        self.assertEqual(a["training"], b["training"])

    def test_transient_collection_failure_retries_without_waiting_full_poll_interval(self):
        from reset_intelligence.sources import collect
        self.store.save("failing", NOW-61, None, None, "URLError")
        self.store.save("healthy", NOW-61, {}, Observation("healthy", NOW-61, NOW-61))
        specs = [{"id":name} for name in ("failing", "healthy")]
        result = (specs[0], NOW, {}, Observation("failing", NOW, NOW), None)
        with patch("reset_intelligence.sources.time.time", return_value=NOW), patch("reset_intelligence.sources.fetch", return_value=result) as fetch:
            collect(self.store, specs, minimum_interval=900)
            fetch.assert_called_once_with(specs[0])


class RolloverTests(unittest.TestCase):
    def example(self, start="October 5, 2026", end="October 11, 2026"):
        description = ("This market will resolve according to the number of qualifying resets announced or initiated "
                       f"between 12:00 a.m. ET on {start}, and 11:59 p.m. ET on {end}.\n\nReviewed settlement rules.")
        a, b, fingerprint = reviewed_window(description, "America/New_York")
        spec = {"id":"internal_private", "rules_reviewed":True, "target":RELIEF, "timezone":"America/New_York",
                "window_start":"2026-09-28T00:00:00-04:00", "rule_reference":"https://private.invalid/rules",
                "discovery":{"series_id":"reviewed", "rules_template_hash":fingerprint},
                "outcomes":{"0":{"count":0}, "1":{"count":1}}}
        event = {"id":"new", "description":description, "active":True, "closed":False,
                 "series":[{"id":"reviewed"}], "endDate":b, "updatedAt":a+86400, "liquidity":1000,
                 "markets":[{"groupItemTitle":str(i), "description":description, "outcomes":'["Yes","No"]',
                             "outcomePrices":'["0.5","0.5"]', "bestBid":.49, "bestAsk":.51,
                             "closed":False, "updatedAt":a+86400, "liquidity":500} for i in (0,1)]}
        return spec, event, a+86400

    def test_same_reviewed_series_rolls_forward_using_rule_window_not_publication_time(self):
        spec, current, now = self.example()
        _, old, _ = self.example("September 28, 2026", "October 4, 2026")
        old["id"] = "expired"
        current["startDate"] = now
        poll = parse_count_market(spec, [old,current], now).polls[0]
        self.assertEqual(poll["id"], "new")
        self.assertEqual(poll["start"], stamp("2026-10-05T00:00:00-04:00"))
        self.assertTrue(poll["private"])

    def test_changed_rules_series_outcomes_closed_and_ambiguous_rounds_do_not_roll_over(self):
        for change in (lambda e: e.update(description=e["description"]+" Changed rule."),
                       lambda e: e["markets"][0].update(description=e["description"]+" Different outcome rule."),
                       lambda e: e.update(series=[{"id":"other"}]),
                       lambda e: e.update(closed=True), lambda e: e["markets"].pop(),
                       lambda e: e.update(endDate=e["endDate"]+86400)):
            spec, event, now = self.example(); change(event)
            self.assertIsNone(select_round(spec, [event], now))
            self.assertEqual(parse_count_market(spec, [event], now).issues, ["no_unique_reviewed_active_round"])
        spec, event, now = self.example()
        self.assertIsNone(select_round(spec, [event,dict(event,id="overlap")], now))

    def test_rollover_retains_bad_quotes_as_excluded_observations(self):
        spec, event, now = self.example()
        event["markets"][0].update(bestAsk=.9)
        poll = parse_count_market(spec, [event], now).polls[0]
        self.assertEqual(count_rejection(poll, now, []), "unusable_quoted_spread")
        event["markets"][0].update(bestAsk=.51)
        event["liquidity"] = 100
        self.assertEqual(count_rejection(parse_count_market(spec,[event],now).polls[0],now,[]), "insufficient_quote_liquidity")


if __name__ == "__main__":
    unittest.main()
