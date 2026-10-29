from copy import deepcopy
import json
import unittest
from unittest.mock import patch

import test_engine as fixtures
from test_engine import NOW
from reset_intelligence.contracts import AUTOMATIC, RELIEF, Observation
from reset_intelligence.pipeline import consumer_export, forecast


class CommunityDiagnosticsTests(unittest.TestCase):
    setUp, tearDown = fixtures.LedgerTests.setUp, fixtures.LedgerTests.tearDown

    def save_poll(self, total, at=NOW, **changes):
        poll = {"kind": "probability_votes", "target": RELIEF, "population": "voters",
                "id": "r", "start": NOW-8*3600, "expires": NOW+86400,
                "total": total, "bins": [{"p": .9, "votes": total}]}
        poll.update(changes)
        self.store.save("date_poll", at, {}, Observation("date_poll", at, at, polls=[poll]))

    def result(self, target=RELIEF, **kwargs):
        result = forecast(self.store, NOW, target, **kwargs)
        exported = consumer_export([result])["forecasts"][0]
        return result, next(d for d in exported["community_diagnostics"] if d["id"] == "date_poll")

    def test_small_recent_increment_never_becomes_large_cumulative_sample(self):
        self.save_poll(1000, NOW-900)
        self.save_poll(1018)
        result, d = self.result()
        self.assertEqual((d["recent_votes"], d["total_votes"]), (18, 1018))
        self.assertEqual(d["reason"], "insufficient_recent_votes")
        self.assertEqual(d["minimum_recent_votes"], 50)
        self.assertFalse(any(c["family"] == "community" for c in result["components"]))
        self.save_poll(1050)
        result, d = self.result()
        self.assertEqual((d["state"], d["recent_votes"], d["reason"]), ("included", 50, None))
        self.assertEqual(d["window_end"], NOW+86400)
        with patch("reset_intelligence.pipeline.initialize_community_diagnostics"), patch("reset_intelligence.pipeline.record_community_diagnostic"):
            without = forecast(self.store, NOW, RELIEF)
        for key in ("curve", "components", "training", "counterfactuals", "community_weight"):
            self.assertEqual(result[key], without[key])

    def test_scope_failure_and_stale_clocks_remain_explicit(self):
        self.save_poll(50, start=NOW)
        _, d = self.result(AUTOMATIC)
        self.assertEqual((d["reason"], d["target"]), ("target_mismatch", RELIEF))
        self.store.save("date_poll", NOW+1, None, None, "URLError")
        current = forecast(self.store, NOW+2, RELIEF)
        d = current["community_diagnostics"]["date_poll"]
        self.assertTrue(d["read_failed"])
        self.assertEqual(d["observed_at"], NOW)
        later = forecast(self.store, NOW+1801, RELIEF)["community_diagnostics"]["date_poll"]
        self.assertEqual(later["reason"], "observation_stale")
        self.assertIsNone(later["recent_votes"])

    def test_disabled_and_private_inputs_never_cross_public_contract(self):
        empty = forecast(self.store, NOW, RELIEF, source_ids={"date_poll"}, private_source_ids={"date_poll"})
        self.assertEqual(consumer_export([empty])["forecasts"][0]["community_diagnostics"], [])
        self.save_poll(50, start=NOW)
        disabled = forecast(self.store, NOW, RELIEF, source_ids={"event_ledger"})
        self.assertEqual(consumer_export([disabled])["forecasts"][0]["community_diagnostics"], [])
        private = forecast(self.store, NOW, RELIEF, private_source_ids={"date_poll"})
        public = consumer_export([private])["forecasts"][0]["community_diagnostics"]
        self.assertNotIn("date_poll", json.dumps(public))
        self.save_poll(50, start=NOW, private=True)
        self.assertNotIn("date_poll", json.dumps(consumer_export([forecast(self.store, NOW, RELIEF)])["forecasts"][0]["community_diagnostics"]))

    def test_unknown_arrival_time_and_no_source_are_not_zero_vote_claims(self):
        _, missing = self.result()
        self.assertEqual(missing["reason"], "source_unavailable")
        self.assertIsNone(missing["recent_votes"])
        self.save_poll(999)
        _, unknown = self.result()
        self.assertEqual(unknown["reason"], "vote_timing_unavailable")
        self.assertEqual(unknown["total_votes"], 999)
        self.assertIsNone(unknown["recent_votes"])

    def test_active_round_wins_over_closed_or_future_rounds(self):
        self.save_poll(50, start=NOW)
        obs = self.store.observations(NOW)["date_poll"]
        current = deepcopy(obs.polls[0])
        obs.polls.extend([{**current, "id": "old", "start": NOW-86400, "expires": NOW},
                          {**current, "id": "next", "start": NOW+86400, "expires": NOW+172800}])
        self.store.save("date_poll", NOW, {}, obs)
        _, d = self.result()
        self.assertEqual(d["state"], "included")
        self.assertEqual(d["window_start"], NOW)
