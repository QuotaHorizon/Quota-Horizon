import json
from pathlib import Path
import unittest

from test_observer import T, TemporaryStore, claim, forecast, trial
from reset_observer.benchmark import initialize, settle_all
from reset_observer.contracts import AUTOMATIC, RELIEF, iso
from reset_observer.diagnostics import coverage_details, dependence, provider_health, runtime_gap, window_diagnostics
from reset_observer.evidence import parse, population, reconcile, reported_stage, retry_delay, text_stage, reinterpret_latest
from reset_observer.importers import fixed_weight_variants, import_model
from reset_observer.reporting import build_report, html_report
from reset_observer.statistics import paired
from reset_observer.store import Store


class DiagnosticsTests(TemporaryStore):
    def test_request_failure_stale_content_and_unchanged_are_distinct(self):
        c = claim(observed_at=T)
        for at in (T, T+900):
            self.store.observation("quota_events", at, T-7200, False, [dict(c, observed_at=at)], {})
            self.store.observation("completion_feed", at, at, True, [dict(c, observed_at=at)], {})
        self.store.observation("public_catalogue", T+900, None, False, None, None, "TimeoutError")
        rows = {r["provider"]: r for r in provider_health(self.store, T+901, self.policy)}
        self.assertEqual(rows["quota_events"]["state"], "stale_source_content")
        self.assertEqual(rows["public_catalogue"]["state"], "request_or_decode_failure")
        self.assertEqual(rows["completion_feed"]["state"], "healthy")
        self.assertTrue(rows["completion_feed"]["content_unchanged"])

    def test_retry_backoff_recovers_and_respects_rate_limit(self):
        for i, delay in enumerate((60,120,240,480,900,900)):
            self.store.observation("completion_feed", T+i, None, False, None, None, "TimeoutError")
            self.assertEqual(retry_delay(self.store, "completion_feed", T+i, self.policy), delay)
        self.store.observation("completion_feed", T+10, T-7200, False, [], {})
        self.assertEqual(retry_delay(self.store, "completion_feed", T+10, self.policy), 900)
        self.store.observation("completion_feed", T+11, None, False, None, None, "http_429")
        self.assertEqual(retry_delay(self.store, "completion_feed", T+11, self.policy), 900)

    def test_coverage_identifies_gap_and_preserves_failed_attempt(self):
        self.cover(T, T+7200, ("completion_feed",))
        for at in (T, T+7200):
            self.store.observation("public_catalogue", at, at, True, [], {})
        detail = coverage_details(self.store, T, T+7200, T+10800, self.policy)
        self.assertFalse(detail["covered"])
        self.assertEqual(detail["qualified_providers"], 1)
        source = next(p for p in detail["providers"] if p["provider"] == "public_catalogue")
        self.assertEqual(source["gaps"][0]["seconds"], 7200)
        self.store.observation("completion_feed", T+100, None, False, None, None, "TimeoutError")
        self.assertEqual(coverage_details(self.store, T, T+7200, T+10800, self.policy)["qualified_providers"], 0)

    def test_windows_deduplicate_forecast_rows_and_mark_historical_gap(self):
        initialize(self.store, T, self.policy)
        for name in ("a", "b", "c"):
            self.store.freeze(trial(source=name))
        settle_all(self.store, T+10*3600, self.policy, {"events": [], "estimand": "test"})
        out = window_diagnostics(self.store, T+10*3600, self.policy, self.store.trials(T+10*3600), self.store.settled(T+10*3600))
        self.assertEqual(out["unique_windows"], 1)
        self.assertEqual(out["windows"][0]["forecasts"], 3)
        self.assertFalse(out["windows"][0]["repairable_from_later_evidence"])

    def test_boundary_gap_and_failed_requests_are_not_both_called_no_collection(self):
        self.cover(T, T+1800, ("completion_feed",))
        for at in range(int(T), int(T+7201), 900):
            self.store.observation("public_catalogue", at, None, False, None, None, "TimeoutError")
        detail = coverage_details(self.store, T, T+7200, T+10800, self.policy)
        by_id = {p["provider"]:p for p in detail["providers"]}
        self.assertIn("collection_gap", by_id["completion_feed"]["reasons"])
        self.assertNotIn("collection_gap", by_id["public_catalogue"]["reasons"])
        self.assertIn("request_or_decode_failure", by_id["public_catalogue"]["reasons"])
        self.assertEqual(by_id["completion_feed"]["gaps"][-1]["to"], iso(T+7200))

    def test_runtime_gap_does_not_claim_certain_sleep(self):
        self.assertIsNone(runtime_gap(T, 100, T+60, 160))
        self.assertEqual(runtime_gap(T, 100, T+7200, 160)["kind"], "suspend_or_clock_jump")
        self.assertEqual(runtime_gap(T, 100, T+7200, 7300)["kind"], "loop_delay")
        self.store.runtime_event(T, {"kind": "process_start", "at": T})
        self.assertEqual(len(self.store.runtime_events(T)), 1)
        self.assertEqual(self.store.runtime_events(T-1), [])

    def test_readonly_old_database_does_not_require_new_table(self):
        self.store.db.execute("DROP TABLE runtime_events")
        self.store.db.commit()
        other = Store(self.store.path, read_only=True)
        try:
            self.assertEqual(other.runtime_events(T), [])
        finally:
            other.close()

    def test_weight_candidates_are_predeclared_and_share_issued_inputs(self):
        payload = {"components": [{"id":"base", "family":"history", "probabilities":[.2,.4]},
                                  {"id":"votes", "family":"community", "probabilities":[.8,.9]}],
                   "training":{"weights":{"history":.5,"community":.5}, "within_groups":[
                       {"family":"history", "weights":{"base":1}}, {"family":"community", "weights":{"votes":1}}]}}
        variants = fixed_weight_variants(payload)
        self.assertAlmostEqual(variants["community_weight_25pct"][0], .35)
        self.assertAlmostEqual(variants["community_weight_75pct"][1], .775)
        self.assertEqual(variants["community_weight_50pct"], variants["equal_components"])

    def test_dependence_pairs_time_target_version_and_preserves_gaps(self):
        trials = []
        for i in range(12):
            for name, private in (("A", False), ("PRIVATE_VENDOR", True)):
                if i == 5 and private:
                    continue
                trials.append(trial(source=name, origin=T+i*3600, hours=24, p=i/20, private=private))
        out = dependence(trials, lambda x: "internal_signal" if x == "PRIVATE_VENDOR" else x)
        self.assertEqual(out[0]["matched_origins"], 11)
        self.assertEqual(out[0]["consecutive_changes"], 9)
        self.assertAlmostEqual(out[0]["level_correlation"], 1)
        self.assertNotIn("PRIVATE_VENDOR", json.dumps(out))


class EventScopeTests(TemporaryStore):
    def test_parser_revision_preserves_observation_and_original_knowledge(self):
        raw = {"generatedAt": iso(T), "posts": [{"eventType":"reset_card", "url":"https://x.com/i/status/123", "id":"1",
                "original":"Added a banked reset to 500k users.", "stage":"completed", "at":iso(T), "handle":"thsottiaux"}]}
        _, _, claims = parse("completion_feed", raw, T)
        wrong = dict(claims[0], population="broad")
        self.store.observation("completion_feed", T, T, True, [wrong], raw)
        reinterpret_latest(self.store, T+7200)
        self.assertEqual(self.store.claims(T)[0]["population"], "broad")
        self.assertEqual(self.store.claims(T+7200)[0]["population"], "targeted")
        self.assertEqual(self.store.observations(T+7200)[0]["observed_at"], T)
        self.assertEqual(self.store.db.execute("SELECT COUNT(*) FROM observations").fetchone()[0], 1)
        reinterpret_latest(self.store, T+10800)
        self.assertEqual(self.store.claims(T+10800)[0]["observed_at"], T+7200)

    def test_mixed_completed_compensation_and_future_global_grant(self):
        text = "Added a banked reset to 500k users. Tomorrow we will grant a banked reset to all users."
        self.assertEqual(text_stage(text), "completed")
        self.assertEqual(population(text), "targeted")
        self.assertEqual(population("Added a banked reset to everyone who pressed the reset button."), "targeted")
        self.assertEqual(population("We have reset usage limits across Codex and ChatGPT Work."), "broad")
        self.assertEqual(population("I reset my own limits for Codex."), "unspecified")
        self.assertEqual(reported_stage("completed", "We are loading a banked reset into all accounts."), "started")
        self.assertEqual(reported_stage("completed", "Tomorrow we will grant a banked reset to all users."), "announced")

    def test_editorial_reason_cannot_enlarge_original_population(self):
        raw = {"generatedAt": iso(T), "posts": [{"eventType":"reset_card", "url":"https://x.com/i/status/123", "id":"1",
                "original":"Added a banked reset.", "reason":"all paid users", "stage":"completed", "at":iso(T), "handle":"thsottiaux"}]}
        _, _, claims = parse("completion_feed", raw, T)
        self.assertEqual(claims[0]["population"], "unspecified")

    def test_unconfirmed_completion_has_accurate_reason(self):
        self.observe(claim(authority="community_report"))
        result = reconcile(self.store, T+7200, self.policy)["events"][0]
        self.assertEqual(result["reason"], "insufficient_primary_confirmation")

    def test_grant_settles_broad_only_and_late_scope_revision_is_append_only(self):
        initialize(self.store, T, self.policy)
        broad = self.store.freeze(trial())
        auto = self.store.freeze(trial(target=AUTOMATIC))
        original = claim(kind="banked", at=T+7200)
        self.observe(original)
        self.cover(T, T+6*3600)
        now = T+10*3600
        evidence = reconcile(self.store, now, self.policy)
        settle_all(self.store, now, self.policy, evidence)
        self.assertEqual(self.store.settled(now)[broad]["label"], 1)
        self.assertEqual(self.store.settled(now)[auto]["label"], 0)
        report = build_report(self.store, now, self.policy, evidence)
        self.store.report(now, report)
        self.observe(dict(original, population="targeted", observed_at=now+60))
        settle_all(self.store, now+60, self.policy, reconcile(self.store, now+60, self.policy))
        self.assertEqual(self.store.settled(now+60)[broad]["label"], 0)
        self.assertEqual(self.store.settled(now)[broad]["label"], 1)
        self.assertEqual(self.store.report_at(now), report)
        self.assertIn("观测与结算诊断", html_report(report))


if __name__ == "__main__":
    unittest.main()
