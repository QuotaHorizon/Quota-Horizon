from dataclasses import replace
from unittest.mock import patch

from test_observer import T, TemporaryStore, forecast, trial
from test_preferences import poll
from reset_observer.benchmark import initialize
from reset_observer.preferences import capture
from reset_observer.reporting import ReportCache, build_report
from reset_observer.store import Store


class ReportCacheTests(TemporaryStore):
    def setUp(self):
        super().setUp()
        initialize(self.store, T, self.policy)
        self.now = T + 20 * 3600
        self.evidence = {"events": [], "estimand": "test"}
        self.cache = ReportCache()
        self.ids = []
        for source, p in (("radar", .4), ("other", .6)):
            self.store.add_forecast(forecast(source=source))
            id = self.store.freeze(trial(source=source, p=p))
            self.ids.append(id)
            self.store.settle(id, T+10*3600, self.label())

    def label(self, **changes):
        return {"label": 0, "status": "settled", "events": [],
                "reason": "covered_without_completion", "score_eligible": True,
                "event_score_eligible": False, "coverage_complete": True,
                "deadline": T+21600, "matures_at": T+32400, **changes}

    def report(self, now=None, cache=None, evidence=None, policy=None):
        return build_report(self.store, self.now if now is None else now,
                            policy or self.policy, evidence or self.evidence, cache=cache)

    def assert_current(self, now=None, evidence=None, policy=None):
        expected = self.report(now, evidence=evidence, policy=policy)
        actual = self.report(now, self.cache, evidence, policy)
        self.assertEqual(actual, expected)
        return actual

    def test_warm_report_skips_old_ledger_and_still_updates_clocks(self):
        first = self.assert_current()
        expected = self.report(self.now+60)
        with patch.object(self.store, "trials", side_effect=AssertionError("old trials reread")), \
             patch.object(self.store, "settled", side_effect=AssertionError("old labels reread")):
            actual = self.report(self.now+60, self.cache)
        self.assertEqual(actual, expected)
        self.assertNotEqual(first["generated_at"], actual["generated_at"])
        self.assertNotEqual(first["latest"][0]["age_seconds"], actual["latest"][0]["age_seconds"])

    def test_new_forecast_curve_and_ballots_do_not_wait_for_scoring_refresh(self):
        first = self.assert_current()
        self.store.add_forecast(forecast(origin=self.now, available_at=self.now,
            generated_at=self.now, points=[[24, .91], [48, .95]], expires_at=self.now+3600))
        capture(self.store, "didcodex_poll", self.now-60, self.now-60, poll())
        capture(self.store, "didcodex_poll", self.now, self.now, poll(17100))
        expected = self.report(self.now+1)
        with patch.object(self.store, "trials", side_effect=AssertionError("unrelated scoring read")):
            actual = self.report(self.now+1, self.cache)
        self.assertEqual(actual, expected)
        self.assertNotEqual(actual["curves"], first["curves"])
        self.assertTrue(actual["calendar_sentiment"]["latest"])
        self.assertEqual(actual["scorecards"], first["scorecards"])

    def test_settlement_and_eligibility_corrections_invalidate_at_same_clock(self):
        before = self.assert_current()
        self.store.settle(self.ids[0], self.now, self.label(label=1, events=["event"]))
        after = self.assert_current()
        self.assertNotEqual(before["comparisons"], after["comparisons"])
        self.store.revise_trial(self.ids[0], self.now, {"p": .8, "missing_reason": None})
        self.store.db.commit()
        changed = self.assert_current()
        self.assertNotEqual(changed["scorecards"], after["scorecards"])
        # A withdrawal can append another revision at the same timestamp.
        self.store.settle(self.ids[0], self.now, self.label(label=None, status="unresolved",
            score_eligible=False, reason="boundary_or_evidence_conflict"))
        withdrawn = self.assert_current()
        self.assertNotEqual(withdrawn["scorecards"], changed["scorecards"])

    def test_event_time_and_status_change_alerts_without_new_trial_rows(self):
        event = {"id": "event", "status": "confirmed", "reason": "completion_observed",
                 "kind": "automatic", "from": T+3600, "to": T+3600,
                 "announced_at": T, "known_at": T+3600, "references": [],
                 "relay_providers": [], "primary_references": [], "completion_reports": 1,
                 "time_basis": "reported_completion"}
        evidence = {"events": [event], "estimand": "test"}
        self.store.revise_trial(self.ids[0], self.now, {"p": .8, "missing_reason": None})
        self.store.db.commit()
        self.store.settle(self.ids[0], self.now, self.label(label=1, events=["event"]))
        first = self.assert_current(evidence=evidence)
        evidence = {**evidence, "events": [{**event, "from": T+7200, "to": T+7200}]}
        changed = self.assert_current(evidence=evidence)
        self.assertNotEqual(first["scorecards"], changed["scorecards"])
        evidence = {**evidence, "events": [{**event, "status": "pending"}]}
        pending = self.assert_current(evidence=evidence)
        self.assertNotEqual(changed["scorecards"], pending["scorecards"])

    def test_future_backfill_visibility_and_backward_replay_do_not_reuse_counts(self):
        self.store.freeze(trial(source="future", frozen_at=self.now+100))
        self.store.freeze(trial(source="earlier", frozen_at=self.now+10))
        first = self.assert_current(self.now+50)
        second = self.assert_current(self.now+150)
        self.assertEqual(second["summary"]["frozen_trials"], first["summary"]["frozen_trials"]+1)
        self.assertEqual(self.assert_current(self.now+50), first)

    def test_live_coverage_and_runtime_events_refresh_without_rescoring_history(self):
        self.assert_current()
        self.store.observation("quota_events", self.now, self.now, True, [], {})
        self.store.runtime_event(self.now, {"kind": "loop_delay", "from": self.now-600, "to": self.now})
        expected = self.report()
        with patch.object(self.store, "trials", side_effect=AssertionError("live receipt rescored history")):
            result = self.report(cache=self.cache)
        self.assertEqual(result, expected)
        self.assertTrue(result["diagnostics"]["runtime_events"])
        expected = self.report(self.now+1900)
        with patch.object(self.store, "trials", side_effect=AssertionError("unchanged ledger")):
            result = self.report(self.now+1900, self.cache)
        self.assertEqual(result, expected)
        self.assertEqual(result["outcome_providers"][0]["state"], "collector_overdue")

    def test_backfilled_coverage_inside_unresolved_window_invalidates_details(self):
        for id in self.ids:
            self.store.settle(id, self.now, self.label(label=None, status="unresolved",
                score_eligible=False, reason="incomplete_outcome_coverage"))
        first = self.assert_current()
        # A current receipt cannot certify this old window. A genuine historical
        # backfill must still invalidate its exact missing-boundary diagnosis.
        self.store.observation("quota_events", self.now, self.now, True, [], {})
        expected = self.report()
        with patch.object(self.store, "trials", side_effect=AssertionError("unrelated receipt")):
            self.assertEqual(self.report(cache=self.cache), expected)
        self.store.observation("quota_events", T, T, True, [], {})
        changed = self.assert_current()
        self.assertNotEqual(first["diagnostics"], changed["diagnostics"])

    def test_crossing_window_end_expires_predeadline_projection(self):
        self.store.freeze(trial(source="upcoming", origin=self.now-21600+30))
        self.assert_current()
        with patch.object(self.store, "trials", wraps=self.store.trials) as reads:
            actual = self.report(self.now+31, self.cache)
        self.assertEqual(reads.call_count, 1)
        self.assertEqual(actual, self.report(self.now+31))

    def test_policy_version_and_private_identity_are_part_of_cache_key(self):
        first = self.assert_current()
        other_policy = replace(self.policy, alert_threshold=.3)
        changed = self.assert_current(policy=other_policy)
        self.assertNotEqual(first["scorecards"], changed["scorecards"])
        self.store.add_forecast(forecast(source="radar", private=True, version="v2",
                                         origin=self.now, available_at=self.now))
        private = self.assert_current()
        self.assertNotIn('"source": "radar"', __import__("json").dumps(private["scorecards"]))
        self.assertTrue(any(r["version"] == "v2" for r in private["scorecards"]))

    def test_cached_result_is_isolated_and_oversized_snapshot_is_not_retained(self):
        expected = self.assert_current()
        expected["scorecards"][0]["scheduled_windows"] = -1
        self.assert_current()
        tiny = ReportCache(maximum_bytes=1)
        self.report(cache=tiny)
        self.assertIsNone(tiny.payload)
        with patch.object(self.store, "trials", wraps=self.store.trials) as reads:
            self.report(cache=tiny)
        self.assertEqual(reads.call_count, 1)

    def test_capture_append_during_key_read_is_seen_on_next_consistent_snapshot(self):
        writer = Store(self.store.path)
        revision = self.store.report_revision
        def append(now):
            result = revision(now)
            writer.freeze(trial(source="concurrent"))
            return result
        try:
            expected = self.report()
            with patch.object(self.store, "report_revision", side_effect=append):
                first = self.report(cache=self.cache)
            self.assertEqual(first["scorecards"], expected["scorecards"])
            second = self.assert_current()
            self.assertEqual(second["summary"]["frozen_trials"], first["summary"]["frozen_trials"]+1)
        finally:
            writer.close()
