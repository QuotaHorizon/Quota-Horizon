from __future__ import annotations

from dataclasses import replace
import json
import math
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import numpy as np

from reset_observer.benchmark import freeze_grid, initialize, label_window, settle_all, trial_probability
from reset_observer.contracts import AUTOMATIC, RELIEF, Claim, Forecast, Policy, iso, url_key
from reset_observer.evidence import coverage, parse, population, reconcile, text_stage
from reset_observer.importers import fixed_weight_variants, intelligence_database, json_lines, paired_variants
from reset_observer.reporting import build_report, html_report
from reset_observer.runtime import atomic_write, exclusive, producer_due, run_once, running
from reset_observer.statistics import alert_metrics, block_interval, paired, scores
from reset_observer.store import Store

T = 1_728_000_000.


def forecast(**kwargs):
    value = dict(source="radar", version="v1", target=RELIEF, track="native", origin=T, available_at=T,
                 generated_at=T, expires_at=T + 72*3600, points=[[24, .2], [48, .36], [72, .488]])
    value.update(kwargs)
    return Forecast(**value)


def claim(provider="quota_events", at=T+3600, **kwargs):
    value = dict(provider=provider, record="event", keys=["https://x.com/i/status/123"], references=["https://x.com/i/status/123"],
                 kind="automatic", stage="completed", population="broad", announced_at=at-600,
                 completed_at=at, observed_at=at+60, text="Reset all propagated.", authority="official_relay")
    value.update(kwargs)
    return Claim(**value).to_dict()


def event(**kwargs):
    value = dict(id="event", kind="automatic", status="confirmed", reason="public_completion_notice",
                 announced_at=T+3600, **{"from": T+7200, "to": T+7200})
    value.update(kwargs)
    return value


def trial(**kwargs):
    value = dict(source="radar", version="v1", track="native", target=RELIEF, origin=T, hours=6, p=.4,
                 frozen_at=T, policy_hash=Policy().hash, private=False, missing_reason=None, headline=True)
    value.update(kwargs)
    return value


class TemporaryStore(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.store = Store(self.root / "observer.sqlite")
        self.policy = Policy()

    def tearDown(self):
        self.store.close()
        self.directory.cleanup()

    def observe(self, c, healthy=True):
        self.store.observation(c["provider"], c["observed_at"], c["observed_at"], healthy, [c], {"claims": [c]})

    def cover(self, start, end, providers=("completion_feed", "public_catalogue")):
        for provider in providers:
            for t in np.arange(start, end + 901, 900):
                self.store.observation(provider, float(t), float(t), True, [], {})


class ContractTests(unittest.TestCase):
    def test_conditional_hazard_alignment_and_strict_tail(self):
        f = forecast()
        self.assertAlmostEqual(f.cdf(12), 1 - math.sqrt(.8))
        self.assertAlmostEqual(f.align(T+3600, 24), .2)
        self.assertIsNone(f.align(T+3600, 72))
        self.assertIsNone(replace(f, available_at=T+100).align(T, 6))
        self.assertIsNone(replace(f, points=[[24, .2]]).align(T+1, 24))

    def test_rejects_bad_probabilities_and_clock(self):
        for points in ([[1, -.01]], [[1, .5], [2, .4]], [[0, .2]], [[1, float("nan")]], [[1, 1.1]]):
            with self.assertRaises(ValueError):
                forecast(points=points).validate()
        with self.assertRaises(ValueError):
            forecast(generated_at=T+1).validate()

    def test_extreme_probabilities_and_version_policy(self):
        self.assertEqual(forecast(points=[[1, 0], [2, 1]]).cdf(2), 1)
        self.assertIsNone(forecast(points=[[1, 1], [2, 1]]).align(T+3600, 1))
        self.assertNotEqual(Policy().hash, Policy(alert_threshold=.8).hash)
        with self.assertRaises(ValueError):
            Policy(collection_seconds=2000)

    def test_reference_identity_and_future_announcements(self):
        self.assertEqual(url_key("https://twitter.com/a/status/123?s=20"), url_key("https://x.com/b/status/123"))
        self.assertNotEqual(url_key("https://example.com/usage#date1"), url_key("https://example.com/usage#date2"))
        self.assertEqual(text_stage("We will reset all limits tomorrow."), "announced")
        self.assertEqual(text_stage("Reset all propagated. Enjoy."), "completed")
        self.assertEqual(text_stage("We are resetting all limits."), "started")
        self.assertEqual(population("Only affected users, not all accounts"), "targeted")


class EvidenceTests(TemporaryStore):
    def test_relays_share_one_event_and_one_primary_reference(self):
        self.observe(claim())
        self.observe(claim("completion_feed"))
        result = reconcile(self.store, T+4000, self.policy)["events"]
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0]["status"], "confirmed")
        self.assertEqual(result[0]["primary_references"], 1)
        self.assertEqual(result[0]["completion_reports"], 2)

    def test_two_relays_of_one_community_rumor_do_not_confirm(self):
        self.observe(claim(authority="community_report"))
        self.observe(claim("completion_feed", authority="community_report"))
        self.assertEqual(reconcile(self.store, T+4000, self.policy)["events"][0]["status"], "pending")

    def test_disappearance_does_not_retract_and_corrections_are_versioned(self):
        self.observe(claim())
        self.store.observation("quota_events", T+4000, T+4000, True, [], {})
        self.assertEqual(reconcile(self.store, T+4100, self.policy)["events"][0]["status"], "confirmed")
        self.observe(claim(stage="retracted", observed_at=T+5000))
        self.assertEqual(reconcile(self.store, T+4500, self.policy)["events"][0]["status"], "confirmed")
        self.assertEqual(reconcile(self.store, T+5100, self.policy)["events"][0]["status"], "disputed")
        self.assertEqual(self.store.db.execute("SELECT COUNT(*) FROM claims").fetchone()[0], 2)

    def test_repeated_claim_preserves_first_knowledge_time(self):
        self.observe(claim())
        self.observe(claim(observed_at=T+6000))
        self.assertEqual(reconcile(self.store, T+7000, self.policy)["events"][0]["known_at"], T+3660)

    def test_conflicts_scope_and_banked_target(self):
        self.observe(claim())
        self.observe(claim("completion_feed", at=T+10000, observed_at=T+11000))
        self.assertEqual(reconcile(self.store, T+12000, self.policy)["events"][0]["reason"], "completion_time_conflict")
        self.observe(claim("completion_feed", at=T+3600, observed_at=T+12000, population="targeted"))
        self.assertEqual(reconcile(self.store, T+13000, self.policy)["events"][0]["status"], "targeted")
        self.assertEqual(label_window(trial(), [event(kind="banked")], T+40000, self.policy, True)["label"], 1)
        self.assertEqual(label_window(trial(target=AUTOMATIC), [event(kind="banked")], T+40000, self.policy, True)["label"], 0)

    def test_quota_missing_optional_fields_and_announcements(self):
        row = {"slug": "a", "provider": "openai", "type": "hard_reset", "state": "confirmed", "announcedAt": iso(T),
               "confirmedAt": iso(T), "source": {"url": "https://x.com/person/status/123", "excerpt": "We will reset all limits tomorrow.", "kind": "authorized_social"}}
        _, healthy, claims = parse("quota_events", {"meta": {"generatedAt": iso(T)}, "data": [row]}, T+100)
        self.assertTrue(healthy)
        self.assertEqual(claims[0]["stage"], "announced")
        row.update(type="banked_reset", state="confirmed", confirmationSource={"url": "https://x.com/person/status/124", "excerpt": "We have added a banked reset for all paid users."})
        self.assertEqual(parse("quota_events", {"meta": {"generatedAt": iso(T)}, "data": [row]}, T+100)[2][0]["kind"], "banked")

    def test_catalogue_misclassified_banked_grant_is_corrected(self):
        raw = {"meta": {"generated_at": iso(T)}, "pagination": {"has_more": False}, "data": [
            {"id": "a", "reset_type": "regular", "announced_at": iso(T), "text": "We have added a banked reset for all paid users.",
             "source": {"url": "https://x.com/person/status/123", "author": "thsottiaux"}}]}
        result = parse("public_catalogue", raw, T+10)[2][0]
        self.assertEqual((result["kind"], result["stage"]), ("banked", "completed"))

    def test_coverage_requires_two_continuous_fresh_providers(self):
        self.cover(T, T+7200, ("completion_feed",))
        self.assertFalse(coverage(self.store, T, T+7200, T+10000, self.policy))
        self.cover(T, T+7200, ("public_catalogue",))
        self.assertTrue(coverage(self.store, T, T+7200, T+10000, self.policy))
        self.assertFalse(coverage(self.store, T-1, T+7200, T+10000, self.policy))
        self.store.observation("public_catalogue", T+1000, None, False, None, None, "timeout")
        self.assertFalse(coverage(self.store, T, T+7200, T+10000, self.policy))


class BenchmarkTests(TemporaryStore):
    def test_successor_version_retires_future_trials_for_old_version(self):
        initialize(self.store, T, self.policy)
        self.store.add_forecast(forecast())
        self.store.add_forecast(forecast(version="v2", origin=T+100, generated_at=T+100, available_at=T+101))
        freeze_grid(self.store, T+3600, self.policy)
        self.assertEqual({r["version"] for r in self.store.trials(T+3600)}, {"v2"})

    def test_grid_never_imports_future_information_or_backfills_arrival(self):
        initialize(self.store, T, self.policy)
        self.store.add_forecast(forecast(available_at=T+1))
        self.store.add_forecast(forecast(origin=T+3600, generated_at=T+3600, available_at=T+4000, points=[[24, .9], [48, .95]]))
        freeze_grid(self.store, T+4000, self.policy)
        rows = self.store.trials(T+4000)
        self.assertEqual(len(rows), 4)
        self.assertAlmostEqual(next(r["p"] for r in rows if r["hours"] == 24), .2)
        self.assertEqual({r["origin"] for r in rows}, {T+3600})
        self.assertEqual(len(self.store.forecasts(T+3600)), 1)

    def test_freeze_is_immutable_and_versions_separate(self):
        first = trial()
        id = self.store.freeze(first)
        self.assertEqual(id, self.store.freeze({**first, "p": .9}))
        self.assertEqual(self.store.trials(T)[0]["p"], .4)
        self.store.freeze({**first, "version": "v2", "p": .6})
        self.assertEqual(len(self.store.trials(T)), 2)

    def test_event_after_source_publication_invalidates_alignment(self):
        self.assertEqual(trial_probability(forecast(), T+3*3600, 6, [event()], self.policy)[1], "event_since_forecast")
        self.assertIsNotNone(trial_probability(forecast(), T+3*3600, 6, [], self.policy)[0])

    def test_maturity_boundary_coverage_and_conflict(self):
        self.assertEqual(label_window(trial(), [], T+6*3600, self.policy, True)["status"], "pending")
        self.assertEqual(label_window(trial(), [], T+9*3600, self.policy, True)["label"], 0)
        self.assertIsNone(label_window(trial(), [], T+9*3600, self.policy, False)["label"])
        incomplete = label_window(trial(), [event()], T+9*3600, self.policy, False)
        self.assertEqual(incomplete["label"], 1)
        self.assertFalse(incomplete["score_eligible"])
        boundary = event(**{"from": T-60, "to": T+60})
        self.assertEqual(label_window(trial(), [boundary], T+9*3600, self.policy, True)["reason"], "boundary_or_evidence_conflict")
        self.assertEqual(label_window(trial(), [event(**{"from": T+6*3600, "to": T+6*3600})], T+9*3600, self.policy, True)["label"], 1)

    def test_late_correction_preserves_old_score_and_report(self):
        initialize(self.store, T, self.policy)
        id = self.store.freeze(trial())
        self.cover(T, T+6*3600)
        evidence = {"events": [], "estimand": "test"}
        settle_all(self.store, T+9*3600, self.policy, evidence)
        self.assertEqual(self.store.settled(T+9*3600)[id]["label"], 0)
        store_report = {"score": .16}
        self.store.report(T+9*3600, store_report)
        evidence["events"] = [event()]
        settle_all(self.store, T+10*3600, self.policy, evidence)
        self.assertEqual(self.store.settled(T+10*3600)[id]["label"], 1)
        self.assertEqual(self.store.settled(T+9*3600)[id]["label"], 0)
        self.assertEqual(self.store.report_at(T+9*3600), store_report)
        self.assertEqual(settle_all(self.store, T+11*3600, self.policy, evidence), 0)

    def test_missing_forecast_does_not_become_zero(self):
        value = label_window(trial(p=None), [], T+9*3600, self.policy, True)
        self.assertEqual(value["label"], 0)
        self.assertFalse(value["score_eligible"])


class StatisticalTests(unittest.TestCase):
    def setUp(self):
        self.policy = Policy()

    def rows(self):
        return [dict(trial(origin=T+i*86400), p=p, label=y, events=[str(i)] if y else []) for i,(p,y) in enumerate(zip([.1,.3,.8,.9],[0,0,1,1]))]

    def test_proper_scores_and_calibration(self):
        result = scores(self.rows(), self.policy)
        self.assertAlmostEqual(result["brier"], .0375)
        self.assertAlmostEqual(result["log_loss"], -np.mean(np.log([.9,.7,.8,.9])))
        self.assertEqual(sum(r["n"] for r in result["calibration"]), 4)
        self.assertEqual(result["unique_events"], 2)
        self.assertEqual(result["state"], "insufficient_events_or_windows")

    def test_log_loss_penalizes_certainty_and_remains_finite(self):
        result = scores([{**trial(), "p": 1., "label": 0, "events": []}], self.policy)
        self.assertTrue(math.isfinite(result["log_loss"]))
        self.assertGreater(result["log_loss"], 13)
        self.assertEqual(result["clipped_predictions"], 1)

    def test_paired_skill_requires_identical_windows_and_targets(self):
        left = self.rows()
        right = [{**r, "p": .5} for r in left]
        result = paired(left, right, self.policy)
        self.assertEqual(result["n"], 4)
        self.assertAlmostEqual(result["brier_skill"], .85)
        self.assertLess(result["loss_difference"]["mean"], 0)
        self.assertEqual(paired(left, [{**r, "origin": r["origin"]+1} for r in right], self.policy)["n"], 0)
        self.assertEqual(paired(left, [{**r, "target": AUTOMATIC} for r in right], self.policy)["n"], 0)

    def test_calendar_blocks_not_repeated_minutes_control_uncertainty(self):
        self.assertIsNone(block_interval([.1]*500, [T]*500)["ci95"])
        values = np.linspace(-.2,-.1,80)
        origins = T+np.arange(80)*86400
        a, b = block_interval(values, origins), block_interval(values, origins)
        self.assertEqual(a, b)
        self.assertLess(a["ci95"][1], 0)

    def test_alerts_count_episodes_and_events_once(self):
        rows = [{**trial(origin=T+i*3600), "p": .8, "label": 1, "events": ["event"]} for i in range(2)]
        result = alert_metrics(rows, [event()], self.policy)
        self.assertEqual(result["episodes"], 1)
        self.assertEqual(result["detected_events"], 1)
        self.assertEqual(result["median_lead_hours"], 2)


class ImportAndReportTests(TemporaryStore):
    def test_incremental_import_skips_already_captured_payload_bodies(self):
        producer = self.root / "producer.sqlite"
        def payload(at):
            return json.dumps({"schema_version":"reset-intelligence/1", "issued_at":at, "config_hash":"config", "model_version":"v1",
                               "target":RELIEF,"status":"unavailable","curve":[]})
        with sqlite3.connect(producer) as db:
            db.executescript("CREATE TABLE observations(id INTEGER PRIMARY KEY,source TEXT,observed_at REAL,normalized TEXT); CREATE TABLE forecasts(id TEXT,issued_at REAL,payload TEXT);")
            db.execute("INSERT INTO forecasts VALUES ('a',?,?)", (T,payload(T)))
        intelligence_database(self.store, producer, T+1)
        with sqlite3.connect(producer) as db:
            db.execute("UPDATE forecasts SET payload='body no longer needed' WHERE id='a'")
            db.execute("INSERT INTO forecasts VALUES ('b',?,?)", (T+100,payload(T+100)))
        result = intelligence_database(self.store, producer, T+101)
        self.assertEqual(result["rejected"], 0)

    def test_short_upstream_expiry_triggers_producer_before_next_collection(self):
        self.store.put("last_producer", {"at":T,"result":{"status":"current"}})
        self.store.add_forecast(forecast(source="horizon", track="model", expires_at=T+120))
        self.assertFalse(producer_due(self.store, T+30, 900))
        self.assertTrue(producer_due(self.store, T+60, 900))
        self.assertTrue(producer_due(self.store, T+130, 900))

    def test_dropped_source_is_refreshed_even_when_reduced_model_is_current(self):
        self.store.put("last_producer", {"at":T,"result":{"status":"current"}})
        self.store.add_forecast(forecast(source="horizon", track="model", expires_at=T+900))
        self.store.add_forecast(forecast(expires_at=T+120, issues=["scope_under_review"]))
        self.assertFalse(producer_due(self.store, T+60, 900))
        self.store.add_forecast(forecast(expires_at=T+120, available_at=T+1))
        self.assertTrue(producer_due(self.store, T+60, 900))

    def test_complete_realtime_pipeline_to_mature_score(self):
        path = self.root / "input.jsonl"
        path.write_text(json.dumps(forecast().__dict__) + "\n")
        args = SimpleNamespace(db=self.store.path, producer_db=None, ingest=path,
                               output=self.root/"report.json", html=self.root/"report.html")
        def collect_initial(store, policy):
            for provider in ("completion_feed", "public_catalogue"):
                store.observation(provider, T, T, True, [], {})
            return []
        with patch("reset_observer.runtime.time.time", return_value=T), patch("reset_observer.runtime.collect", side_effect=collect_initial):
            run_once(args, {})
        self.cover(T+900, T+15*3600)
        self.observe(claim(at=T+8*3600, observed_at=T+8*3600+60))
        with patch("reset_observer.runtime.time.time", return_value=T+15*3600), patch("reset_observer.runtime.collect", return_value=[]):
            run_once(args, {})
        report = json.loads(args.output.read_text())
        score = next(r for r in report["scorecards"] if r["hours"] == 6)["scores"]
        self.assertEqual(score["n"], 1)
        self.assertEqual(score["unique_events"], 1)
        self.assertGreater(score["brier"], .8)
        self.assertIn("<!doctype html>", args.html.read_text())
        self.assertEqual(report["state"], "scoring")

    def test_producer_failure_is_reported_without_corrupting_outcomes(self):
        args = SimpleNamespace(db=self.store.path, producer_db=None, ingest=None, output=self.root/"report.json", html=None)
        config = {"producer_command":["unavailable-executable"]}
        with patch("reset_observer.runtime.time.time", return_value=T), patch("reset_observer.runtime.collect", return_value=[]), \
             patch("reset_observer.runtime.subprocess.run", side_effect=FileNotFoundError("private command details")):
            run_once(args, config)
        report = json.loads(args.output.read_text())
        self.assertEqual(report["runtime"]["producer"]["status"], "unavailable")
        self.assertNotIn("private command details", args.output.read_text())
        self.assertEqual(report["summary"]["scored_nonoverlapping_trials"], 0)

    def test_jsonl_receipt_clock_cannot_be_backdated(self):
        path = self.root / "input.jsonl"
        path.write_text(json.dumps(forecast().__dict__) + "\n")
        json_lines(self.store, path, T+50)
        json_lines(self.store, path, T+500)
        self.assertEqual(len(self.store.forecasts(T+1000)), 1)
        self.assertEqual(self.store.forecasts(T+1000)[0].available_at, T+50)
        self.assertEqual(self.store.forecasts(T+49), [])

    def test_readonly_producer_import_preserves_raw_curve(self):
        producer = self.root / "producer.sqlite"
        with sqlite3.connect(producer) as db:
            db.executescript("CREATE TABLE observations(id INTEGER PRIMARY KEY,source TEXT,observed_at REAL,normalized TEXT); CREATE TABLE forecasts(id TEXT,issued_at REAL,payload TEXT);")
            c = {**forecast().__dict__, "dependency": "shared_history"}
            db.execute("INSERT INTO observations VALUES (1,?,?,?)", ("radar", T, json.dumps({"curves": [c]})))
        before = producer.read_bytes()
        self.assertEqual(intelligence_database(self.store, producer, T+20)["native"], 1)
        self.assertEqual(self.store.forecasts(T+30)[0].points, forecast().points)
        self.assertEqual(self.store.forecasts(T+30)[0].available_at, T+20)
        self.assertEqual(intelligence_database(self.store, producer, T+30)["native"], 0)
        self.assertEqual(before, producer.read_bytes())

    def test_fixed_ablation_excludes_community_and_renormalizes(self):
        payload = {"components": [{"id":"history", "family":"history", "probabilities":[.2,.4]},
                                   {"id":"votes", "family":"community", "probabilities":[.8,.9]}],
                   "training":{"weights":{"history":.7,"community":.3}, "within_groups":[
                       {"family":"history", "weights":{"history":1.}}, {"family":"community", "weights":{"votes":1.}}]}}
        variants = fixed_weight_variants(payload)
        np.testing.assert_allclose(variants["without_community"], [.2,.4])
        np.testing.assert_allclose(variants["without:history"], [.8,.9])
        np.testing.assert_allclose(variants["equal_components"], [.5,.65])

    def test_ablations_preserve_deployed_cross_target_projection(self):
        def model(target, p, community=None):
            components = [{"id":"survival_baseline", "family":"history", "probabilities":[p]}]
            groups = [{"family":"history", "weights":{"survival_baseline":1.}}]
            weights = {"history":1.}
            if community is not None:
                components.append({"id":"votes", "family":"community", "probabilities":[community]})
                groups.append({"family":"community", "weights":{"votes":1.}})
                weights = {"history":.5,"community":.5}
            raw = p if community is None else (p+community)/2
            return {"target":target,"status":"current","valid_until":T+1000,"components":components,
                    "training":{"within_groups":groups,"weights":weights}, "curve":[{"hours":1,"probability":raw}]}
        result = paired_variants([model(RELIEF,.2),model(AUTOMATIC,.3,.9)])
        self.assertEqual(result[RELIEF]["without_community"]["points"], [.25])
        self.assertEqual(result[AUTOMATIC]["without_community"]["points"], [.25])

    def test_private_provenance_does_not_leave_report(self):
        initialize(self.store, T, self.policy)
        self.store.add_forecast(forecast(source="SECRET_PLATFORM", private=True, track="community",
                                          metadata={"rule_reference":"https://private.example/contract"}))
        self.store.indicator("SECRET_PLATFORM", T, {"private":True,"kind":"count_distribution","total":None,"expires":T+10000,
                                                   "rule_reference":"https://private.example/contract"})
        report = build_report(self.store, T+10, self.policy, {"events":[], "estimand":"test"})
        content = json.dumps(report) + html_report(report)
        self.assertNotIn("SECRET_PLATFORM", content)
        self.assertNotIn("private.example", content)
        self.assertEqual(report["scorecards"][0]["scores"]["n"], 0)

    def test_writer_lock_prevents_duplicate_watchers(self):
        self.assertFalse(running(self.store.path))
        with exclusive(self.store.path):
            self.assertTrue(running(self.store.path))
        self.assertFalse(running(self.store.path))

    def test_atomic_report_replacement_and_policy_isolation(self):
        output = self.root / "report.json"
        atomic_write(output, '{"value":1}')
        atomic_write(output, '{"value":2}')
        self.assertEqual(json.loads(output.read_text())["value"], 2)
        self.assertEqual(list(self.root.glob(".observer-*")), [])
        initialize(self.store, T, self.policy)
        with self.assertRaises(ValueError):
            initialize(self.store, T+1, replace(self.policy, alert_threshold=.8))


if __name__ == "__main__":
    unittest.main()
