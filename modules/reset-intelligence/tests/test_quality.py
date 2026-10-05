from copy import deepcopy
import json
import unittest

import test_engine as fixtures
from test_engine import NOW, history
from reset_intelligence.audit import source_audit
from reset_intelligence.community import ballot_curve, count_curve, count_rejection, parse_count_market
from reset_intelligence.contracts import RELIEF, Observation
from reset_intelligence.evaluation import canonical_events
from reset_intelligence.pipeline import complete_tail, forecast
from reset_intelligence.quant import ModelConfig, Survival
from reset_intelligence.sources import parse


class SourceQualityTests(unittest.TestCase):
    setUp = fixtures.LedgerTests.setUp
    tearDown = fixtures.LedgerTests.tearDown

    def test_same_poll_round_mirror_does_not_add_weight(self):
        poll = {"kind":"probability_votes", "target":RELIEF, "population":"same_voters", "id":"round",
                "start":NOW-3600, "expires":NOW+86400, "total":10, "bins":[{"p":.9,"votes":10}]}
        for source in ("a", "mirror"):
            self.store.save(source, NOW, {}, Observation(source, NOW, NOW, polls=[poll]))
        f = forecast(self.store, NOW, RELIEF)
        self.assertEqual(sum(c["family"] == "community" for c in f["components"]), 1)
        self.assertIn("duplicate_poll_round", [issue for h in f["source_health"] for issue in h["issues"]])
        self.assertAlmostEqual(f["training"]["weights"]["community"], .5)

    def test_empty_poll_has_specific_exclusion_and_audit_preserves_actual_weight(self):
        poll = {"kind":"probability_votes", "target":RELIEF, "population":"sample", "id":"round",
                "start":NOW-3600, "expires":NOW+86400, "total":1, "bins":[{"p":.9,"votes":1}]}
        self.store.save("votes", NOW, {}, Observation("votes", NOW, NOW, polls=[poll]))
        self.store.save("private_name", NOW, {}, Observation("private_name", NOW, NOW, polls=[dict(poll, total=0, private=True)]))
        f = forecast(self.store, NOW, RELIEF, private_source_ids={"private_name"})
        before = self.store.db.total_changes
        audit = source_audit(self.store, NOW, [f])
        self.assertEqual(self.store.db.total_changes, before)
        row = next(r for r in audit["sources"] if r["source"] == "votes")
        self.assertAlmostEqual(row["issued_contribution"]["ballot_weight_before_projection"], .1)
        self.assertEqual(row["issued_contribution"]["weight_state"], "cold_start_prior")
        private = next(r for r in audit["sources"] if r["private"])
        self.assertEqual(private["reasons"], ["empty_poll"])
        self.assertEqual(private["admission"], "excluded")
        self.assertAlmostEqual(sum(r["issued_contribution"].get("component_weight_before_projection", 0) for r in audit["sources"]), 1)
        self.assertNotIn("private_name", json.dumps(audit))

    def test_targeted_grant_stays_in_evidence_but_not_broad_baseline(self):
        raw = {"meta":{"generated_at":NOW},"pagination":{"has_more":False},"data":[
            {"id":"compensation", "reset_type":"banked", "announced_at":NOW-3600,
             "text":"Added a banked reset to 500k users. Tomorrow we will grant one to all users.",
             "source":{"url":"https://example.test/compensation", "type":"social"}}]}
        obs = parse({"id":"event_ledger", "adapter":"ledger"}, raw, NOW)
        self.assertEqual(len(obs.events), 1)
        self.assertEqual(obs.events[0].population, "targeted")
        self.assertEqual(canonical_events(obs, RELIEF, NOW), [])


class QuoteQualityTests(unittest.TestCase):
    def quote(self):
        spec = {"id":"private", "rules_reviewed":True, "target":RELIEF, "window_start":NOW-86400,
                "timezone":"UTC", "rule_reference":"https://private.invalid/rules", "outcomes":{"0":{"count":0}, "1":{"count":1}}}
        raw = {"id":"round", "updatedAt":NOW, "endDate":NOW+86400, "liquidity":1000,
               "markets":[{"groupItemTitle":str(i), "outcomes":'["Yes","No"]', "outcomePrices":'["0.5","0.5"]',
                           "bestBid":.49, "bestAsk":.51, "closed":False, "updatedAt":NOW, "liquidity":500} for i in (0,1)]}
        return spec, raw

    def test_missing_quotes_future_clock_and_duplicate_outcomes_rejected(self):
        for change in (lambda d: d["markets"][0].pop("bestAsk"),
                       lambda d: d["markets"][0].update(updatedAt=NOW+3600),
                       lambda d: d["markets"].append(deepcopy(d["markets"][0])),
                       lambda d: d["markets"].pop(),
                       lambda d: d.update(liquidity=float("nan"))):
            spec, raw = self.quote()
            change(raw)
            with self.assertRaises(ValueError):
                parse_count_market(spec, raw, NOW)

    def test_zero_bid_is_valid_but_unresolved_closed_quote_is_not(self):
        spec, raw = self.quote()
        raw["markets"][0].update(bestBid=0, bestAsk=.1)
        self.assertEqual(parse_count_market(spec, raw, NOW).polls[0]["rows"][0]["bid"], 0)
        raw["markets"][0]["closed"] = True
        with self.assertRaises(ValueError):
            parse_count_market(spec, raw, NOW)

    def test_impossible_count_has_specific_rejection_not_zero_probability(self):
        spec, raw = self.quote()
        poll = parse_count_market(spec, raw, NOW).polls[0]
        self.assertEqual(count_rejection(poll, NOW, [NOW-10]), "quote_conflicts_with_event_count_or_remaining_days")
        self.assertIsNone(count_curve(poll, NOW, [6], [NOW-10]))

    def test_fractional_ballot_deadline_is_used_for_tail_completion(self):
        base = Survival([e.at for e in history()], NOW)
        poll = {"kind":"interval_votes", "target":RELIEF, "population":"sample", "id":"round",
                "start":NOW, "expires":NOW+1.5*3600, "total":10,
                "bins":[{"start":NOW,"end":NOW+1.5*3600,"votes":8},
                        {"start":NOW+1.5*3600,"end":None,"votes":2}]}
        out = ballot_curve(poll, NOW, [1,2], base, NOW-10000, ModelConfig())
        self.assertEqual(out["coverage_hours"], 1.5)
        curve = complete_tail(out["probabilities"], [1,2], base, (out["coverage_hours"],out["terminal_probability"]))
        self.assertAlmostEqual(curve[1], 1-(1-out["terminal_probability"])*(1-base.p(2))/(1-base.p(1.5)))


if __name__ == "__main__":
    unittest.main()
