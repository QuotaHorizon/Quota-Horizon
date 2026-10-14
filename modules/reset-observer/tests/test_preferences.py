from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from reset_observer.contracts import RELIEF, Policy
from reset_observer.preferences import capture, current_features, freeze, report, settle
from reset_observer.store import Store
from reset_observer.reporting import build_report, html_report
from reset_observer.benchmark import initialize
from test_observer import T, event, forecast


def poll(reset=17000, feature=5000, **kwargs):
    return {"id": "didcodex:2026-10-13", "date": "2026-10-13", "kind": "calendar_preference",
            "target": RELIEF, "unit": "clicks", "start": T-3600, "expires": T+3600,
            "total": reset+feature, "bins": [{"p": 1., "votes": reset}, {"p": 0., "votes": feature}],
            "max_votes_per_ip_24h": 100, "unique_participants": None,
            "voting_open": True, "upstream_outcome": None,
            "question": "reset_or_improvement_preference_and_prediction", **kwargs}


class CalendarPreferenceTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.store = Store(Path(self.tmp.name)/"observer.sqlite")
        self.policy = Policy()

    def tearDown(self):
        self.store.close()
        self.tmp.cleanup()

    def put(self, at, p, received=None):
        capture(self.store, "didcodex_poll", at, at if received is None else received, p)

    def test_initial_23000_clicks_are_not_new_votes(self):
        self.put(T, poll(18000,5000))
        self.put(T+60, poll(18000,5000))
        f = current_features(self.store,T+60)[0]
        self.assertEqual(f["total_clicks"],23000)
        self.assertEqual(f["recent_votes"],0)
        self.assertIsNone(f["reset_share"])
        self.assertIsNone(f["repeat_sensitive_candidate"])
        self.assertEqual(f["state"],"insufficient_recent_votes")

    def test_threshold_repeat_sensitivity_and_received_clock(self):
        self.put(T-60,poll())
        self.put(T,poll(17049,5000),received=T+10)
        self.assertEqual(current_features(self.store,T)[0]["recent_votes"],0)
        self.assertEqual(current_features(self.store,T+10)[0]["state"],"insufficient_recent_votes")
        self.assertIsNone(current_features(self.store,T+10)[0]["reset_share"])
        self.put(T+60,poll(17049,5001))
        f = current_features(self.store,T+60)[0]
        self.assertEqual(f["recent_votes"],50)
        self.assertEqual(f["state"],"ready_for_evaluation")
        self.assertLess(f["repeat_adjusted_units"],.5)
        self.assertLess(f["repeat_sensitive_candidate"],.7)
        self.assertIsNone(f["unique_participants"])
        self.assertEqual(f["minimum_ip_units_under_published_limit"],1)
        self.assertEqual(f["production_weight"],0)

    def test_correction_and_large_gap_rebase_without_counting_unknown_batch(self):
        self.put(T-120,poll())
        self.put(T-60,poll(17090,5000))
        self.put(T,poll(17010,5000))
        self.put(T+60,poll(17011,5000))
        f = current_features(self.store,T+60)[0]
        self.assertEqual((f["recent_votes"],f["corrections"]),(1,1))
        self.put(T+3600,poll(17500,5000,expires=T+7200))
        f = current_features(self.store,T+3600)[0]
        self.assertEqual((f["recent_votes"],f["observation_gaps"]),(0,1))

    def test_closed_or_future_day_does_not_freeze(self):
        for state in ({"voting_open":False},{"upstream_outcome":"reset"},{"start":T+3600,"expires":T+86400}):
            with self.subTest(state=state):
                round_id=str(state)
                self.put(T-60,poll(id=round_id,**state))
                self.put(T,poll(17050,5000,id=round_id,**state))
        self.assertEqual(freeze(self.store,T,self.policy,{"events":[]}),0)

    def eligible(self):
        self.put(T-60,poll())
        self.put(T,poll(17050,5000))
        self.store.add_forecast(forecast(source="horizon",track="model"))
        self.assertEqual(freeze(self.store,T,self.policy,{"events":[]}),1)

    def test_freeze_once_with_contemporary_comparator_and_preserve_values(self):
        self.eligible()
        saved=report(self.store,T)["trials"][0]
        self.assertEqual(saved["comparisons"][0]["source"],"horizon")
        self.put(T+60,poll(17050,5050))
        self.assertEqual(freeze(self.store,T+60,self.policy,{"events":[]}),0)
        self.assertEqual(report(self.store,T+60)["trials"][0]["candidates"],saved["candidates"])
        self.assertEqual(len(self.store.trials(T+60)),0) # No preference value contaminates main scoring.

    def test_known_completion_cannot_be_predicted_by_later_votes(self):
        self.put(T-60,poll())
        self.put(T,poll(17050,5000))
        evidence={"events":[event(**{"from":T-30,"to":T-30})]}
        self.assertEqual(freeze(self.store,T,self.policy,evidence),0)

    def test_prior_day_reset_excludes_old_cycle_comparator(self):
        self.put(T-60,poll(start=T-30))
        self.put(T,poll(17050,5000,start=T-30))
        self.store.add_forecast(forecast(origin=T-120,generated_at=T-120,available_at=T-120))
        evidence={"events":[event(**{"from":T-90,"to":T-90})]}
        self.assertEqual(freeze(self.store,T,self.policy,evidence),1)
        self.assertEqual(report(self.store,T)["trials"][0]["comparisons"],[])

    def test_positive_negative_and_partial_coverage_use_same_outcome_policy(self):
        self.eligible()
        end=T+3600
        with patch("reset_observer.preferences.coverage_details",return_value={"covered":True}):
            settle(self.store,end+self.policy.reporting_grace_seconds-1,self.policy,{"events":[]})
            self.assertEqual(report(self.store,end+10800)["trials"][0]["settlement"]["status"],"pending")
            settle(self.store,end+10800,self.policy,{"events":[]})
            zero=report(self.store,end+10800)["trials"][0]["settlement"]
            self.assertEqual(zero["label"],0)
            self.assertTrue(zero["score_eligible"])
            self.assertEqual(zero["candidate_losses"]["raw_click_share"]["brier"],1.)
        evidence={"events":[event(**{"from":T+1800,"to":T+1800})]}
        with patch("reset_observer.preferences.coverage_details",return_value={"covered":False}):
            settle(self.store,end+10801,self.policy,evidence)
            one=report(self.store,end+10801)["trials"][0]["settlement"]
            self.assertEqual(one["label"],1)
            self.assertFalse(one["score_eligible"])
            self.assertTrue(one["event_score_eligible"])
            self.assertEqual(one["candidate_losses"]["raw_click_share"]["brier"],0.)
            self.assertEqual(settle(self.store,end+10802,self.policy,evidence),0)
        self.assertEqual(self.store.db.execute('SELECT COUNT(*) FROM preference_settlements').fetchone()[0],3)

    def test_actual_report_includes_collection_and_evaluation_section(self):
        self.eligible()
        initialize(self.store,T,self.policy)
        evidence={"events":[],"estimand":"public_quota_relief"}
        r=build_report(self.store,T,self.policy,evidence)
        self.assertEqual(r["calendar_sentiment"]["sample_observations"],2)
        html=html_report(r)
        self.assertIn("Did Codex Reset",html)
        self.assertIn("实际新增",html)
        self.assertIn("重复折算单位",html)
