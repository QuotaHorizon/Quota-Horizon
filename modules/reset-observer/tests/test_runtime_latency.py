import json
import threading
import time
from types import SimpleNamespace
from unittest.mock import patch

from test_observer import T, TemporaryStore, claim, trial, event
from reset_observer.benchmark import initialize, label_window, settle_all
from reset_observer.evidence import reconcile
from reset_observer.runtime import BackgroundEvaluation, run_once
from reset_observer.reporting import build_report
from reset_observer import SETTLEMENT_VERSION


class RuntimeLatencyTests(TemporaryStore):
    def test_capture_and_manual_refresh_continue_during_slow_evaluation(self):
        args=SimpleNamespace(db=self.store.path,producer_db=None,ingest=None,output=None,html=None)
        started, release = threading.Event(), threading.Event()
        calls=[]
        def slow(*args):
            calls.append(args[2]);started.set()
            if not release.wait(5):raise AssertionError("evaluation did not release")
            return {}
        worker=BackgroundEvaluation()
        try:
            with patch("reset_observer.runtime.collect", return_value=[]), patch("reset_observer.runtime.evaluate", side_effect=slow):
                first=run_once(args,{},evaluator=worker)
                self.assertTrue(started.wait(2))
                second=run_once(args,{},refresh=True,evaluator=worker)
                self.assertEqual(second["refresh_state"],"refreshed")
                self.assertEqual(first["evaluation"],"background")
                self.assertFalse(worker.future.done())
                self.assertEqual(len(calls),1)
                release.set();worker.future.result(timeout=2)
                worker.poll();worker.future.result(timeout=2)
                self.assertEqual(len(calls),2)
        finally:
            release.set();worker.close()

    def test_transient_failure_does_not_erase_valid_24h_and_48h_coverage(self):
        initialize(self.store,T,self.policy)
        self.cover(T,T+48*3600)
        self.store.observation("completion_feed",T+100,None,False,None,None,"TimeoutError")
        self.observe(claim(at=T+12*3600))
        ids=[self.store.freeze(trial(hours=h)) for h in (24,48)]
        now=T+52*3600;evidence=reconcile(self.store,now,self.policy)
        settle_all(self.store,now,self.policy,evidence)
        for id in ids:
            row=self.store.settled(now)[id]
            self.assertEqual(row["label"],1)
            self.assertTrue(row["score_eligible"])
            self.assertEqual(row["settlement_version"],SETTLEMENT_VERSION)
        with patch("reset_observer.benchmark.coverage",side_effect=AssertionError("repeated settled coverage scan")):
            self.assertEqual(settle_all(self.store,now+60,self.policy,evidence),0)

    def test_observed_event_with_gap_is_separate_from_calibration(self):
        initialize(self.store,T,self.policy)
        self.observe(claim(at=T+12*3600))
        id=self.store.freeze(trial(hours=24))
        now=T+28*3600;evidence=reconcile(self.store,now,self.policy)
        settle_all(self.store,now,self.policy,evidence)
        row=self.store.settled(now)[id]
        self.assertEqual(row["status"],"settled")
        self.assertEqual(row["label"],1)
        self.assertTrue(row["event_score_eligible"])
        self.assertFalse(row["score_eligible"])
        report=build_report(self.store,now,self.policy,evidence)
        score=report["scorecards"][0]
        self.assertEqual(score["scores"]["n"],0)
        self.assertEqual(score["observed_event_scores"]["n"],1)
        self.assertAlmostEqual(score["observed_event_scores"]["brier"],.36)
        self.assertFalse(report["comparisons"])

    def test_old_labels_and_frozen_predictions_survive_rule_revision(self):
        initialize(self.store,T,self.policy)
        id=self.store.freeze(trial(hours=24));self.cover(T,T+24*3600)
        now=T+28*3600
        old={"label":None,"status":"unresolved","events":[],"reason":"incomplete_outcome_coverage",
             "deadline":T+86400,"matures_at":T+27*3600,"score_eligible":False}
        self.store.settle(id,now-1,old)
        before=self.store.trials(now)
        settle_all(self.store,now,self.policy,{"events":[],"estimand":"test"})
        self.assertEqual(self.store.trials(now),before)
        self.assertEqual(self.store.settled(now-1)[id]["status"],"unresolved")
        self.assertEqual(self.store.settled(now)[id]["label"],0)
