from copy import deepcopy
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch

import test_engine
from test_engine import NOW
from reset_intelligence.community import recent_ballot, ballot_curve
from reset_intelligence.contracts import Observation, RELIEF
from reset_intelligence.pipeline import forecast
from reset_intelligence.quant import ModelConfig, Survival


class BallotFreshnessTests(unittest.TestCase):
    setUp = test_engine.LedgerTests.setUp
    tearDown = test_engine.LedgerTests.tearDown

    def sample(self, total=0):
        return {"kind": "probability_votes", "target": RELIEF, "population": "voters", "id": "r",
                "start": NOW-86400, "expires": NOW+86400, "total": total,
                "bins": [{"p": .1, "votes": total}, {"p": .9, "votes": 0}]}

    def save_poll(self, at, poll):
        self.store.save("votes", at, {}, Observation("votes", at, polls=[deepcopy(poll)]))

    def test_repeated_fetches_do_not_freshen_an_old_vote(self):
        poll = self.sample()
        self.save_poll(NOW-5*3600, poll)
        poll["total"] = poll["bins"][0]["votes"] = 1
        self.save_poll(NOW-4*3600, poll)
        self.save_poll(NOW-60, poll)
        first, meta = recent_ballot(self.store, "votes", poll, NOW)
        self.assertEqual(meta["latest_vote_at"], NOW-4*3600)
        self.assertEqual(first["total"], 1)
        self.save_poll(NOW+60, poll)
        later, updated = recent_ballot(self.store, "votes", poll, NOW+120)
        self.assertEqual(updated["latest_vote_at"], meta["latest_vote_at"])
        self.assertLess(updated["freshness_weight"], meta["freshness_weight"])
        self.assertLess(later["weighted_bins"][0]["votes"], first["weighted_bins"][0]["votes"])

    def test_new_votes_can_move_both_directions_without_a_selected_message(self):
        for old_p, new_p in ((.1, .9), (.9, .1)):
            with self.subTest(old=old_p):
                source = str(old_p)
                poll = self.sample()
                poll["bins"] = [{"p": old_p, "votes": 0}, {"p": new_p, "votes": 0}]
                for at, counts in ((NOW-5*3600, [0,0]), (NOW-4*3600,[10,0]), (NOW-600,[10,0]), (NOW-60,[10,10])):
                    poll["total"] = sum(counts)
                    for item, n in zip(poll["bins"], counts): item["votes"] = n
                    self.store.save(source, at, {}, Observation(source, at, polls=[deepcopy(poll)]))
                recent, meta = recent_ballot(self.store, source, poll, NOW)
                self.assertEqual(recent["total"], 20)
                self.assertEqual(meta["latest_vote_at"], NOW-60)
                weights = recent["weighted_bins"]
                average = sum(x["p"]*x["votes"] for x in weights)/sum(x["votes"] for x in weights)
                self.assertLess(abs(average-new_p), abs(average-old_p))

    def test_new_round_cumulative_votes_have_conservative_age(self):
        poll = self.sample(20);poll["start"] = NOW-4*3600
        _, meta = recent_ballot(self.store, "new", poll, NOW)
        self.assertEqual(meta["recent_votes"], 20)
        self.assertAlmostEqual(meta["freshness_weight"], .25)

    def test_forecast_publication_does_not_scan_historical_predictions(self):
        with patch.object(self.store, "forecasts", side_effect=AssertionError("archive scan on live path")):
            result = forecast(self.store, NOW, RELIEF)
        self.assertEqual(result["status"], "current")
        self.assertEqual(result["training"]["windows"], 0)
        self.assertEqual(result["training"]["evaluation"], "independent_observer")

    def test_consumer_is_published_before_diagnostic_history_is_read(self):
        from reset_intelligence.__main__ import main
        output = Path(self.tmp.name) / "consumer.json"
        audit = Path(self.tmp.name) / "audit.json"
        def diagnostic(*args):
            saved = json.loads(output.read_text())
            self.assertEqual(len(saved["forecasts"]), 2)
            raise RuntimeError("isolated diagnostic failure")
        with patch("reset_intelligence.__main__.time.time", return_value=NOW), patch("reset_intelligence.__main__.source_audit", side_effect=diagnostic), redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, "isolated diagnostic"):
                main(["forecast", "--db", str(self.store.path), "--output", str(output), "--audit-output", str(audit)])
        self.assertFalse(audit.exists())
