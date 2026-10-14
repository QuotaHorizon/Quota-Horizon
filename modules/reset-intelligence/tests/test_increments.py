from copy import deepcopy
import unittest

import test_engine as fixtures
from test_engine import NOW
from reset_intelligence.community import recent_ballot
from reset_intelligence.contracts import AUTOMATIC, RELIEF, Curve, Observation
from reset_intelligence.increments import combine
from reset_intelligence.pipeline import forecast_all
from reset_intelligence.quant import ModelConfig
import numpy as np


def component(name, base, full, role="information"):
    return {"id": name, "family": "radars", "dependency": "shared_news", "role": role,
            "signal_baseline": [base]*72, "probabilities": [full]*72, "metadata": {}, "private": False}


class IncrementTests(unittest.TestCase):
    def setUp(self):
        self.local = {"id": "survival_baseline", "family": "baseline", "dependency": "history",
                      "probabilities": [.12]*72, "metadata": {}, "private": False}

    def test_twenty_plus_ten_then_thirty_retains_the_full_news_response(self):
        for full in (.3, .5, .7):
            items = [self.local, component("active", .2, full), component("quiet", .2, .2, "background")]
            result = combine(items, ModelConfig())
            self.assertAlmostEqual(result["baseline"][23], .2)
            self.assertAlmostEqual(result["radar_adjustment"][23], full-.2)
            self.assertAlmostEqual(result["probabilities"][23], full)
            self.assertEqual(result["weights"]["quiet"], 0)

    def test_background_is_stable_when_a_source_starts_responding(self):
        quiet = component("quiet", .1, .1, "background")
        inactive = component("active", .3, .3, "background")
        before = combine([self.local, quiet, inactive], ModelConfig())
        after = combine([self.local, quiet, component("active", .3, .5)], ModelConfig())
        self.assertEqual(before["baseline"], after["baseline"])
        self.assertAlmostEqual(after["probabilities"][23]-before["probabilities"][23], .2)

    def test_signed_corrections_and_neutral_reviews_are_retained(self):
        result = combine([self.local, component("negative", .5, .3)], ModelConfig())
        self.assertAlmostEqual(result["radar_adjustment"][23], -.2)
        self.assertAlmostEqual(result["probabilities"][23], .3)
        result = combine([self.local, component("positive", .2, .6), component("reviewed_neutral", .2, .2)], ModelConfig())
        self.assertAlmostEqual(result["probabilities"][23], .4)

    def test_opaque_high_and_low_forecasts_are_comparisons_symmetrically(self):
        for p in (.01, .99):
            result = combine([self.local, component("active", .2, .5), component("opaque", .2, p, "comparison")], ModelConfig())
            self.assertAlmostEqual(result["probabilities"][23], .5)

    def test_community_is_a_signed_residual_not_a_second_copy_of_news(self):
        for p in (.2, .5, .9):
            poll = {"id":"poll", "family":"community", "dependency":"voters", "metadata":{"quality":1.}, "probabilities":[p]*72}
            out = combine([self.local, component("active", .2, .5), poll], ModelConfig())
            self.assertAlmostEqual(out["probabilities"][23], .5 + out["community_weight"]*(p-.5))
            self.assertAlmostEqual(out["baseline"][23]+out["radar_adjustment"][23]+out["community_adjustment"][23], out["probabilities"][23])

    def test_conflicting_source_curves_remain_valid_and_every_adjustment_is_accounted_for(self):
        rng = np.random.default_rng(48)
        for _ in range(100):
            a, b = component("active", .2, .5), component("quiet", .2, .2, "background")
            a["signal_baseline"] = np.sort(rng.random(72)).tolist()
            a["probabilities"] = np.sort(rng.random(72)).tolist()
            b["signal_baseline"] = b["probabilities"] = np.sort(rng.random(72)).tolist()
            result = combine([self.local,a,b],ModelConfig())
            self.assertTrue(all(0 <= p <= 1 for p in result["probabilities"]))
            self.assertTrue(np.all(np.diff(result["probabilities"]) >= -1e-9))
            np.testing.assert_allclose(np.array(result["baseline"])+result["radar_adjustment"]+np.array(result["community_adjustment"])+result["shape_adjustment"],result["probabilities"],atol=1e-9)


class IncrementIntegrationTests(unittest.TestCase):
    setUp, tearDown = fixtures.LedgerTests.setUp, fixtures.LedgerTests.tearDown

    def test_broad_floor_never_halves_automatic_information(self):
        curve = Curve("signal", AUTOMATIC, NOW, NOW, NOW, NOW+3600, [(24,.5),(48,.75)], metadata={"baseline_points":[(24,.2),(48,.36)]})
        self.store.save("signal", NOW, {}, Observation("signal", NOW, NOW, curves=[curve]))
        broad, auto = forecast_all(self.store, NOW)
        self.assertAlmostEqual(auto["curve"][23]["probability"], .5)
        self.assertGreaterEqual(broad["curve"][23]["probability"], .5)
        for f in (broad, auto):
            for p in f["curve"]:
                self.assertAlmostEqual(p["baseline"]+p["radar_adjustment"]+p["community_adjustment"]+p["coherence_adjustment"], p["probability"])

    def test_recent_ballots_do_not_turn_old_totals_into_current_sentiment(self):
        poll = {"id":"r", "kind":"probability_votes", "target":RELIEF, "population":"voters", "start":NOW-86400,
                "expires":NOW+86400, "total":70, "bins":[{"p":.3,"votes":60},{"p":.9,"votes":10}]}
        self.store.save("poll", NOW-4*3600, {}, Observation("poll", NOW-4*3600, polls=[poll]))
        current, meta = recent_ballot(self.store, "poll", poll, NOW)
        self.assertEqual(current["total"], 0)
        updated = deepcopy(poll); updated["total"] += 4; updated["bins"][1]["votes"] += 4
        current, meta = recent_ballot(self.store, "poll", updated, NOW)
        self.assertEqual(current["total"], 4)
        self.assertEqual(meta["total_votes"], 74)
        self.assertEqual(current["bins"][0]["votes"], 0)
        self.assertEqual(current["bins"][1]["votes"], 4)
        # A tally correction cannot manufacture a new optimistic sample.
        updated["bins"][0]["votes"] -= 1
        self.assertEqual(recent_ballot(self.store,"poll",updated,NOW)[0]["total"], 0)
