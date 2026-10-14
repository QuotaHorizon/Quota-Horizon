from __future__ import annotations

from dataclasses import replace
import json
from contextlib import closing
import math
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import numpy as np

from reset_observer.benchmark import (
    freeze_grid,
    initialize,
    label_window,
    settle_all,
    trial_probability,
)
from reset_observer.contracts import (
    AUTOMATIC,
    RELIEF,
    Claim,
    Forecast,
    Policy,
    iso,
    url_key,
)
from reset_observer.evidence import coverage, parse, population, reconcile, text_stage
from reset_observer.importers import (
    fixed_weight_variants,
    intelligence_database,
    json_lines,
    paired_variants,
)
from reset_observer.reporting import build_report, html_report, publication_curves
from reset_observer.runtime import (
    atomic_write,
    exclusive,
    producer_due,
    run_once,
    running,
    started_worker,
)
from reset_observer.statistics import alert_metrics, block_interval, paired, scores
from reset_observer.store import Store

T = 1_728_000_000.0


def forecast(**kwargs):
    value = dict(
        source="radar",
        version="v1",
        target=RELIEF,
        track="native",
        origin=T,
        available_at=T,
        generated_at=T,
        expires_at=T + 72 * 3600,
        points=[[24, 0.2], [48, 0.36], [72, 0.488]],
    )
    value.update(kwargs)
    return Forecast(**value)


def claim(provider="quota_events", at=T + 3600, **kwargs):
    value = dict(
        provider=provider,
        record="event",
        keys=["https://x.com/i/status/123"],
        references=["https://x.com/i/status/123"],
        kind="automatic",
        stage="completed",
        population="broad",
        announced_at=at - 600,
        completed_at=at,
        observed_at=at + 60,
        text="Reset all propagated.",
        authority="official_relay",
    )
    value.update(kwargs)
    return Claim(**value).to_dict()


def event(**kwargs):
    value = dict(
        id="event",
        kind="automatic",
        status="confirmed",
        reason="public_completion_notice",
        announced_at=T + 3600,
        **{"from": T + 7200, "to": T + 7200},
    )
    value.update(kwargs)
    return value


def trial(**kwargs):
    value = dict(
        source="radar",
        version="v1",
        track="native",
        target=RELIEF,
        origin=T,
        hours=6,
        p=0.4,
        frozen_at=T,
        policy_hash=Policy().hash,
        private=False,
        missing_reason=None,
        headline=True,
    )
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
        self.store.observation(
            c["provider"],
            c["observed_at"],
            c["observed_at"],
            healthy,
            [c],
            {"claims": [c]},
        )

    def cover(self, start, end, providers=("completion_feed", "public_catalogue")):
        for provider in providers:
            for t in np.arange(start, end + 901, 900):
                self.store.observation(provider, float(t), float(t), True, [], {})


class ContractTests(unittest.TestCase):
    def test_conditional_hazard_alignment_and_strict_tail(self):
        f = forecast()
        self.assertAlmostEqual(f.cdf(12), 1 - math.sqrt(0.8))
        self.assertAlmostEqual(f.align(T + 3600, 24), 0.2)
        self.assertIsNone(f.align(T + 3600, 72))
        self.assertIsNone(replace(f, available_at=T + 100).align(T, 6))
        self.assertIsNone(replace(f, points=[[24, 0.2]]).align(T + 1, 24))

    def test_rejects_bad_probabilities_and_clock(self):
        for points in (
            [[1, -0.01]],
            [[1, 0.5], [2, 0.4]],
            [[0, 0.2]],
            [[1, float("nan")]],
            [[1, 1.1]],
        ):
            with self.assertRaises(ValueError):
                forecast(points=points).validate()
        with self.assertRaises(ValueError):
            forecast(generated_at=T + 1).validate()

    def test_extreme_probabilities_and_version_policy(self):
        self.assertEqual(forecast(points=[[1, 0], [2, 1]]).cdf(2), 1)
        self.assertIsNone(forecast(points=[[1, 1], [2, 1]]).align(T + 3600, 1))
        self.assertNotEqual(Policy().hash, Policy(alert_threshold=0.8).hash)
        with self.assertRaises(ValueError):
            Policy(collection_seconds=2000)

    def test_reference_identity_and_future_announcements(self):
        self.assertEqual(
            url_key("https://twitter.com/a/status/123?s=20"),
            url_key("https://x.com/b/status/123"),
        )
        self.assertNotEqual(
            url_key("https://example.com/usage#date1"),
            url_key("https://example.com/usage#date2"),
        )
        self.assertEqual(text_stage("We will reset all limits tomorrow."), "announced")
        self.assertEqual(text_stage("Reset all propagated. Enjoy."), "completed")
        self.assertEqual(text_stage("We are resetting all limits."), "started")
        self.assertEqual(
            population("Only affected users, not all accounts"), "targeted"
        )


class EvidenceTests(TemporaryStore):
    def test_relays_share_one_event_and_one_primary_reference(self):
        self.observe(claim())
        self.observe(claim("completion_feed"))
        result = reconcile(self.store, T + 4000, self.policy)["events"]
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0]["status"], "confirmed")
        self.assertEqual(result[0]["primary_references"], 1)
        self.assertEqual(result[0]["completion_reports"], 2)

    def test_two_relays_of_one_community_rumor_do_not_confirm(self):
        self.observe(claim(authority="community_report"))
        self.observe(claim("completion_feed", authority="community_report"))
        self.assertEqual(
            reconcile(self.store, T + 4000, self.policy)["events"][0]["status"],
            "pending",
        )

    def test_disappearance_does_not_retract_and_corrections_are_versioned(self):
        self.observe(claim())
        self.store.observation("quota_events", T + 4000, T + 4000, True, [], {})
        self.assertEqual(
            reconcile(self.store, T + 4100, self.policy)["events"][0]["status"],
            "confirmed",
        )
        self.observe(claim(stage="retracted", observed_at=T + 5000))
        self.assertEqual(
            reconcile(self.store, T + 4500, self.policy)["events"][0]["status"],
            "confirmed",
        )
        self.assertEqual(
            reconcile(self.store, T + 5100, self.policy)["events"][0]["status"],
            "disputed",
        )
        self.assertEqual(
            self.store.db.execute("SELECT COUNT(*) FROM claims").fetchone()[0], 2
        )

    def test_repeated_claim_preserves_first_knowledge_time(self):
        self.observe(claim())
        self.observe(claim(observed_at=T + 6000))
        self.assertEqual(
            reconcile(self.store, T + 7000, self.policy)["events"][0]["known_at"],
            T + 3660,
        )

    def test_conflicts_scope_and_banked_target(self):
        self.observe(claim())
        self.observe(claim("completion_feed", at=T + 10000, observed_at=T + 11000))
        self.assertEqual(
            reconcile(self.store, T + 12000, self.policy)["events"][0]["reason"],
            "completion_time_conflict",
        )
        self.observe(
            claim(
                "completion_feed",
                at=T + 3600,
                observed_at=T + 12000,
                population="targeted",
            )
        )
        self.assertEqual(
            reconcile(self.store, T + 13000, self.policy)["events"][0]["status"],
            "targeted",
        )
        self.assertEqual(
            label_window(trial(), [event(kind="banked")], T + 40000, self.policy, True)[
                "label"
            ],
            1,
        )
        self.assertEqual(
            label_window(
                trial(target=AUTOMATIC),
                [event(kind="banked")],
                T + 40000,
                self.policy,
                True,
            )["label"],
            0,
        )

    def test_quota_missing_optional_fields_and_announcements(self):
        row = {
            "slug": "a",
            "provider": "openai",
            "type": "hard_reset",
            "state": "confirmed",
            "announcedAt": iso(T),
            "confirmedAt": iso(T),
            "source": {
                "url": "https://x.com/person/status/123",
                "excerpt": "We will reset all limits tomorrow.",
                "kind": "authorized_social",
            },
        }
        _, healthy, claims = parse(
            "quota_events", {"meta": {"generatedAt": iso(T)}, "data": [row]}, T + 100
        )
        self.assertTrue(healthy)
        self.assertEqual(claims[0]["stage"], "announced")
        row.update(
            type="banked_reset",
            state="confirmed",
            confirmationSource={
                "url": "https://x.com/person/status/124",
                "excerpt": "We have added a banked reset for all paid users.",
            },
        )
        self.assertEqual(
            parse(
                "quota_events",
                {"meta": {"generatedAt": iso(T)}, "data": [row]},
                T + 100,
            )[2][0]["kind"],
            "banked",
        )

    def test_catalogue_misclassified_banked_grant_is_corrected(self):
        raw = {
            "meta": {"generated_at": iso(T)},
            "pagination": {"has_more": False},
            "data": [
                {
                    "id": "a",
                    "reset_type": "regular",
                    "announced_at": iso(T),
                    "text": "We have added a banked reset for all paid users.",
                    "source": {
                        "url": "https://x.com/person/status/123",
                        "author": "thsottiaux",
                    },
                }
            ],
        }
        result = parse("public_catalogue", raw, T + 10)[2][0]
        self.assertEqual((result["kind"], result["stage"]), ("banked", "completed"))

    def test_coverage_requires_two_continuous_fresh_providers(self):
        self.cover(T, T + 7200, ("completion_feed",))
        self.assertFalse(coverage(self.store, T, T + 7200, T + 10000, self.policy))
        self.cover(T, T + 7200, ("public_catalogue",))
        self.assertTrue(coverage(self.store, T, T + 7200, T + 10000, self.policy))
        self.assertFalse(coverage(self.store, T - 1, T + 7200, T + 10000, self.policy))
        self.store.observation(
            "public_catalogue", T + 1000, None, False, None, None, "timeout"
        )
        self.assertTrue(coverage(self.store, T, T + 7200, T + 10000, self.policy))


class BenchmarkTests(TemporaryStore):
    def test_successor_version_retires_future_trials_for_old_version(self):
        initialize(self.store, T, self.policy)
        self.store.add_forecast(forecast())
        self.store.add_forecast(
            forecast(
                version="v2", origin=T + 100, generated_at=T + 100, available_at=T + 101
            )
        )
        freeze_grid(self.store, T + 3600, self.policy)
        self.assertEqual({r["version"] for r in self.store.trials(T + 3600)}, {"v2"})

    def test_grid_never_imports_future_information_or_backfills_arrival(self):
        initialize(self.store, T, self.policy)
        self.store.add_forecast(forecast(available_at=T + 1))
        self.store.add_forecast(
            forecast(
                origin=T + 3600,
                generated_at=T + 3600,
                available_at=T + 4000,
                points=[[24, 0.9], [48, 0.95]],
            )
        )
        freeze_grid(self.store, T + 4000, self.policy)
        rows = self.store.trials(T + 4000)
        self.assertEqual(len(rows), 4)
        self.assertAlmostEqual(next(r["p"] for r in rows if r["hours"] == 24), 0.2)
        self.assertEqual({r["origin"] for r in rows}, {T + 3600})
        self.assertEqual(len(self.store.forecasts(T + 3600)), 1)

    def test_freeze_is_immutable_and_versions_separate(self):
        first = trial()
        id = self.store.freeze(first)
        self.assertEqual(id, self.store.freeze({**first, "p": 0.9}))
        self.assertEqual(self.store.trials(T)[0]["p"], 0.4)
        self.store.freeze({**first, "version": "v2", "p": 0.6})
        self.assertEqual(len(self.store.trials(T)), 2)

    def test_event_after_source_publication_invalidates_alignment(self):
        self.assertEqual(
            trial_probability(forecast(), T + 3 * 3600, 6, [event()], self.policy)[1],
            "event_since_forecast",
        )
        self.assertIsNotNone(
            trial_probability(forecast(), T + 3 * 3600, 6, [], self.policy)[0]
        )

    def test_maturity_boundary_coverage_and_conflict(self):
        self.assertEqual(
            label_window(trial(), [], T + 6 * 3600, self.policy, True)["status"],
            "pending",
        )
        self.assertEqual(
            label_window(trial(), [], T + 9 * 3600, self.policy, True)["label"], 0
        )
        self.assertIsNone(
            label_window(trial(), [], T + 9 * 3600, self.policy, False)["label"]
        )
        incomplete = label_window(trial(), [event()], T + 9 * 3600, self.policy, False)
        self.assertEqual(incomplete["label"], 1)
        self.assertFalse(incomplete["score_eligible"])
        boundary = event(**{"from": T - 60, "to": T + 60})
        self.assertEqual(
            label_window(trial(), [boundary], T + 9 * 3600, self.policy, True)[
                "reason"
            ],
            "boundary_or_evidence_conflict",
        )
        self.assertEqual(
            label_window(
                trial(),
                [event(**{"from": T + 6 * 3600, "to": T + 6 * 3600})],
                T + 9 * 3600,
                self.policy,
                True,
            )["label"],
            1,
        )

    def test_late_correction_preserves_old_score_and_report(self):
        initialize(self.store, T, self.policy)
        id = self.store.freeze(trial())
        self.cover(T, T + 6 * 3600)
        evidence = {"events": [], "estimand": "test"}
        settle_all(self.store, T + 9 * 3600, self.policy, evidence)
        self.assertEqual(self.store.settled(T + 9 * 3600)[id]["label"], 0)
        store_report = {"score": 0.16}
        self.store.report(T + 9 * 3600, store_report)
        evidence["events"] = [event()]
        settle_all(self.store, T + 10 * 3600, self.policy, evidence)
        self.assertEqual(self.store.settled(T + 10 * 3600)[id]["label"], 1)
        self.assertEqual(self.store.settled(T + 9 * 3600)[id]["label"], 0)
        self.assertEqual(self.store.report_at(T + 9 * 3600), store_report)
        self.assertEqual(
            settle_all(self.store, T + 11 * 3600, self.policy, evidence), 0
        )

    def test_missing_forecast_does_not_become_zero(self):
        value = label_window(trial(p=None), [], T + 9 * 3600, self.policy, True)
        self.assertEqual(value["label"], 0)
        self.assertFalse(value["score_eligible"])


class StatisticalTests(unittest.TestCase):
    def setUp(self):
        self.policy = Policy()

    def rows(self):
        return [
            dict(
                trial(origin=T + i * 86400), p=p, label=y, events=[str(i)] if y else []
            )
            for i, (p, y) in enumerate(zip([0.1, 0.3, 0.8, 0.9], [0, 0, 1, 1]))
        ]

    def test_proper_scores_and_calibration(self):
        result = scores(self.rows(), self.policy)
        self.assertAlmostEqual(result["brier"], 0.0375)
        self.assertAlmostEqual(
            result["log_loss"], -np.mean(np.log([0.9, 0.7, 0.8, 0.9]))
        )
        self.assertEqual(sum(r["n"] for r in result["calibration"]), 4)
        self.assertEqual(result["unique_events"], 2)
        self.assertEqual(result["state"], "insufficient_events_or_windows")

    def test_log_loss_penalizes_certainty_and_remains_finite(self):
        result = scores([{**trial(), "p": 1.0, "label": 0, "events": []}], self.policy)
        self.assertTrue(math.isfinite(result["log_loss"]))
        self.assertGreater(result["log_loss"], 13)
        self.assertEqual(result["clipped_predictions"], 1)

    def test_paired_skill_requires_identical_windows_and_targets(self):
        left = self.rows()
        right = [{**r, "p": 0.5} for r in left]
        result = paired(left, right, self.policy)
        self.assertEqual(result["n"], 4)
        self.assertAlmostEqual(result["brier_skill"], 0.85)
        self.assertLess(result["loss_difference"]["mean"], 0)
        self.assertEqual(
            paired(
                left, [{**r, "origin": r["origin"] + 1} for r in right], self.policy
            )["n"],
            0,
        )
        self.assertEqual(
            paired(left, [{**r, "target": AUTOMATIC} for r in right], self.policy)["n"],
            0,
        )

    def test_calendar_blocks_not_repeated_minutes_control_uncertainty(self):
        self.assertIsNone(block_interval([0.1] * 500, [T] * 500)["ci95"])
        values = np.linspace(-0.2, -0.1, 80)
        origins = T + np.arange(80) * 86400
        a, b = block_interval(values, origins), block_interval(values, origins)
        self.assertEqual(a, b)
        self.assertLess(a["ci95"][1], 0)

    def test_alerts_count_episodes_and_events_once(self):
        rows = [
            {**trial(origin=T + i * 3600), "p": 0.8, "label": 1, "events": ["event"]}
            for i in range(2)
        ]
        result = alert_metrics(rows, [event()], self.policy)
        self.assertEqual(result["episodes"], 1)
        self.assertEqual(result["detected_events"], 1)
        self.assertEqual(result["median_lead_hours"], 2)


class ImportAndReportTests(TemporaryStore):
    def test_published_24h_trend_survives_late_receipt_without_relaxing_scoring(self):
        f = forecast(points=[[6,.1],[24,.4]], available_at=T+60, expires_at=T+3600)
        self.store.add_forecast(f)
        curves = publication_curves(self.store,T+120)
        short = next(c for c in curves if c["hours"] == 24)
        self.assertEqual(short["points"], [{"at":iso(T+60),"p":.4}])
        self.assertEqual(next(c for c in curves if c["hours"] == 48)["points"][0]["p"], None)
        self.assertIsNone(f.align(T+60,24))
        self.assertEqual(self.store.trials(T+120), [])

    def test_publication_trends_keep_expiry_gaps_versions_and_private_inputs_out(self):
        self.store.add_forecast(forecast(available_at=T+60,expires_at=T+300))
        self.store.add_forecast(forecast(origin=T+3600,generated_at=T+3600,available_at=T+3660,expires_at=T+7200))
        self.store.add_forecast(forecast(source="secret",private=True,available_at=T+3660))
        self.store.add_forecast(forecast(version="v2",origin=T+7200,generated_at=T+7200,available_at=T+7260))
        curves = publication_curves(self.store,T+7270)
        self.assertNotIn("secret",json.dumps(curves))
        v1 = next(c for c in curves if c["version"]=="v1" and c["hours"]==24)
        self.assertEqual(v1["points"][1],{"at":iso(T+300),"p":None})
        self.assertEqual({c["version"] for c in curves},{"v1","v2"})
    def test_scoring_progress_separates_rolling_windows_from_headline_scores(self):
        initialize(self.store, T, self.policy)
        for source in ("radar", "comparison"):
            self.store.freeze(trial(source=source, origin=T + 3600, headline=False))
        self.store.freeze(
            trial(
                source="missing",
                origin=T + 3600,
                p=None,
                headline=False,
                missing_reason="stale_forecast",
            )
        )
        self.cover(T, T + 13 * 3600)
        evidence = {"events": [], "estimand": "test"}
        first_at = T + 10 * 3600
        settle_all(self.store, first_at, self.policy, evidence)
        first = build_report(self.store, first_at, self.policy, evidence)
        self.assertEqual(first["summary"]["scored_nonoverlapping_trials"], 0)
        six = first["summary"]["scoring_by_horizon"][0]
        self.assertEqual(
            six,
            {
                "hours": 6,
                "settled_windows": 1,
                "scored_windows": 0,
                "scored_trials": 0,
                "eligible_rolling_trials": 2,
                "missing_forecast_trials": 1,
            },
        )
        self.assertIn("已有滚动窗口结算", html_report(first))
        self.store.report(first_at, first)
        # The next predeclared six-hour origin becomes eligible naturally.
        for target in (AUTOMATIC, RELIEF):
            for source in ("radar", "comparison"):
                self.store.freeze(
                    trial(
                        source=source, target=target, origin=T + 6 * 3600, headline=True
                    )
                )
        settle_all(self.store, T + 15 * 3600, self.policy, evidence)
        final = build_report(self.store, T + 15 * 3600, self.policy, evidence)
        six = final["summary"]["scoring_by_horizon"][0]
        self.assertEqual(
            six,
            {
                "hours": 6,
                "settled_windows": 3,
                "scored_windows": 2,
                "scored_trials": 4,
                "eligible_rolling_trials": 6,
                "missing_forecast_trials": 1,
            },
        )
        self.assertEqual(final["summary"]["scored_nonoverlapping_trials"], 4)
        self.assertIn("4 条评分，覆盖 2 个目标时间窗", html_report(final))
        old_report = dict(
            first,
            summary={
                k: v for k, v in first["summary"].items() if k != "scoring_by_horizon"
            },
        )
        self.assertIn("正在积累未来窗口", html_report(old_report))
        self.assertEqual(
            [r["hours"] for r in final["summary"]["scoring_by_horizon"]],
            [6, 12, 24, 48],
        )
        self.assertEqual(self.store.report_at(first_at), first)
        replay = build_report(self.store, first_at, self.policy, evidence)
        self.assertEqual(
            replay["summary"]["scoring_by_horizon"],
            first["summary"]["scoring_by_horizon"],
        )

    def test_incremental_import_skips_already_captured_payload_bodies(self):
        producer = self.root / "producer.sqlite"

        def payload(at):
            return json.dumps(
                {
                    "schema_version": "reset-intelligence/1",
                    "issued_at": at,
                    "config_hash": "config",
                    "model_version": "v1",
                    "target": RELIEF,
                    "status": "unavailable",
                    "curve": [],
                }
            )

        with closing(sqlite3.connect(producer)) as db, db:
            db.executescript(
                "CREATE TABLE observations(id INTEGER PRIMARY KEY,source TEXT,observed_at REAL,normalized TEXT); CREATE TABLE forecasts(id TEXT,issued_at REAL,payload TEXT);"
            )
            db.execute("INSERT INTO forecasts VALUES ('a',?,?)", (T, payload(T)))
        intelligence_database(self.store, producer, T + 1)
        with closing(sqlite3.connect(producer)) as db, db:
            db.execute(
                "UPDATE forecasts SET payload='body no longer needed' WHERE id='a'"
            )
            db.execute(
                "INSERT INTO forecasts VALUES ('b',?,?)", (T + 100, payload(T + 100))
            )
        result = intelligence_database(self.store, producer, T + 101)
        self.assertEqual(result["rejected"], 0)

    def test_short_upstream_expiry_triggers_producer_before_next_collection(self):
        self.store.put("last_producer", {"at": T, "result": {"status": "current"}})
        self.store.add_forecast(
            forecast(source="horizon", track="model", expires_at=T + 120)
        )
        self.assertFalse(producer_due(self.store, T + 30, 900))
        self.assertTrue(producer_due(self.store, T + 60, 900))
        self.assertTrue(producer_due(self.store, T + 130, 900))

    def test_dropped_source_is_refreshed_even_when_reduced_model_is_current(self):
        self.store.put("last_producer", {"at": T, "result": {"status": "current"}})
        self.store.add_forecast(
            forecast(source="horizon", track="model", expires_at=T + 900)
        )
        self.store.add_forecast(
            forecast(expires_at=T + 120, issues=["scope_under_review"])
        )
        self.assertFalse(producer_due(self.store, T + 60, 900))
        self.store.add_forecast(forecast(expires_at=T + 120, available_at=T + 1))
        self.assertTrue(producer_due(self.store, T + 60, 900))

    def test_complete_realtime_pipeline_to_mature_score(self):
        path = self.root / "input.jsonl"
        path.write_text(json.dumps(forecast().__dict__) + "\n")
        args = SimpleNamespace(
            db=self.store.path,
            producer_db=None,
            ingest=path,
            output=self.root / "report.json",
            html=self.root / "report.html",
        )

        def collect_initial(store, policy):
            for provider in ("completion_feed", "public_catalogue"):
                store.observation(provider, T, T, True, [], {})
            return []

        with (
            patch("reset_observer.runtime.time.time", return_value=T),
            patch("reset_observer.runtime.collect", side_effect=collect_initial),
        ):
            run_once(args, {})
        self.cover(T + 900, T + 15 * 3600)
        self.observe(claim(at=T + 8 * 3600, observed_at=T + 8 * 3600 + 60))
        with (
            patch("reset_observer.runtime.time.time", return_value=T + 15 * 3600),
            patch("reset_observer.runtime.collect", return_value=[]),
        ):
            run_once(args, {})
        report = json.loads(args.output.read_text())
        score = next(r for r in report["scorecards"] if r["hours"] == 6)["scores"]
        self.assertEqual(score["n"], 1)
        self.assertEqual(score["unique_events"], 1)
        self.assertGreater(score["brier"], 0.8)
        self.assertIn("<!doctype html>", args.html.read_text())
        self.assertEqual(report["state"], "scoring")

    def test_producer_failure_is_reported_without_corrupting_outcomes(self):
        args = SimpleNamespace(
            db=self.store.path,
            producer_db=None,
            ingest=None,
            output=self.root / "report.json",
            html=None,
        )
        config = {"producer_command": ["unavailable-executable"]}
        with (
            patch("reset_observer.runtime.time.time", return_value=T),
            patch("reset_observer.runtime.collect", return_value=[]),
            patch(
                "reset_observer.runtime.subprocess.run",
                side_effect=FileNotFoundError("private command details"),
            ),
        ):
            run_once(args, config)
        report = json.loads(args.output.read_text())
        self.assertEqual(report["runtime"]["producer"]["status"], "unavailable")
        self.assertNotIn("private command details", args.output.read_text())
        self.assertEqual(report["summary"]["scored_nonoverlapping_trials"], 0)

    def test_jsonl_receipt_clock_cannot_be_backdated(self):
        path = self.root / "input.jsonl"
        path.write_text(json.dumps(forecast().__dict__) + "\n")
        json_lines(self.store, path, T + 50)
        json_lines(self.store, path, T + 500)
        self.assertEqual(len(self.store.forecasts(T + 1000)), 1)
        self.assertEqual(self.store.forecasts(T + 1000)[0].available_at, T + 50)
        self.assertEqual(self.store.forecasts(T + 49), [])

    def test_readonly_producer_import_preserves_raw_curve(self):
        producer = self.root / "producer.sqlite"
        with closing(sqlite3.connect(producer)) as db, db:
            db.executescript(
                "CREATE TABLE observations(id INTEGER PRIMARY KEY,source TEXT,observed_at REAL,normalized TEXT); CREATE TABLE forecasts(id TEXT,issued_at REAL,payload TEXT);"
            )
            c = {**forecast().__dict__, "dependency": "shared_history"}
            db.execute(
                "INSERT INTO observations VALUES (1,?,?,?)",
                ("radar", T, json.dumps({"curves": [c]})),
            )
        before = producer.read_bytes()
        self.assertEqual(
            intelligence_database(self.store, producer, T + 20)["native"], 1
        )
        self.assertEqual(self.store.forecasts(T + 30)[0].points, forecast().points)
        self.assertEqual(self.store.forecasts(T + 30)[0].available_at, T + 20)
        self.assertEqual(
            intelligence_database(self.store, producer, T + 30)["native"], 0
        )
        self.assertEqual(before, producer.read_bytes())

    def test_fixed_ablation_excludes_community_and_renormalizes(self):
        payload = {
            "components": [
                {"id": "history", "family": "history", "probabilities": [0.2, 0.4]},
                {"id": "votes", "family": "community", "probabilities": [0.8, 0.9]},
            ],
            "training": {
                "weights": {"history": 0.7, "community": 0.3},
                "within_groups": [
                    {"family": "history", "weights": {"history": 1.0}},
                    {"family": "community", "weights": {"votes": 1.0}},
                ],
            },
        }
        variants = fixed_weight_variants(payload)
        np.testing.assert_allclose(variants["without_community"], [0.2, 0.4])
        np.testing.assert_allclose(variants["without:history"], [0.8, 0.9])
        np.testing.assert_allclose(variants["equal_components"], [0.5, 0.65])

    def test_ablations_preserve_deployed_cross_target_projection(self):
        def model(target, p, community=None):
            components = [
                {"id": "survival_baseline", "family": "history", "probabilities": [p]}
            ]
            groups = [{"family": "history", "weights": {"survival_baseline": 1.0}}]
            weights = {"history": 1.0}
            if community is not None:
                components.append(
                    {"id": "votes", "family": "community", "probabilities": [community]}
                )
                groups.append({"family": "community", "weights": {"votes": 1.0}})
                weights = {"history": 0.5, "community": 0.5}
            raw = p if community is None else (p + community) / 2
            return {
                "target": target,
                "status": "current",
                "valid_until": T + 1000,
                "components": components,
                "training": {"within_groups": groups, "weights": weights},
                "curve": [{"hours": 1, "probability": raw}],
            }

        result = paired_variants([model(RELIEF, 0.2), model(AUTOMATIC, 0.3, 0.9)])
        self.assertEqual(result[RELIEF]["without_community"]["points"], [0.25])
        self.assertEqual(result[AUTOMATIC]["without_community"]["points"], [0.25])

    def test_three_family_ablations_preserve_group_mass_and_relative_budgets(self):
        payload = {"components": [
            {"id":"survival_baseline", "family":"baseline", "probabilities":[.1,.2]},
            {"id":"a", "family":"radars", "probabilities":[.4,.6]},
            {"id":"b", "family":"radars", "probabilities":[.8,.9]},
            {"id":"votes", "family":"community", "probabilities":[.7,.8]}],
            "training": {"weights":{"baseline":.15,"radars":.65,"community":.2}, "within_groups":[
                {"family":"baseline", "weights":{"survival_baseline":1.}, "group_weight":1.},
                {"family":"radars", "weights":{"a":1.}, "group_weight":.75},
                {"family":"radars", "weights":{"b":1.}, "group_weight":.25},
                {"family":"community", "weights":{"votes":1.}, "group_weight":1.}]}}
        variants = fixed_weight_variants(payload)
        radar = .75*np.array([.4,.6]) + .25*np.array([.8,.9])
        without = (.15*np.array([.1,.2])+.65*radar)/.8
        np.testing.assert_allclose(variants["without_community"], without)
        np.testing.assert_allclose(variants["without_radars"], (.15*np.array([.1,.2])+.2*np.array([.7,.8]))/.35)
        np.testing.assert_allclose(variants["community_weight_50pct"], .5*without+.5*np.array([.7,.8]))

    def test_increment_ablations_use_the_deployed_floor_and_supplied_recomputation(self):
        def payload(target, full, without):
            return {"target":target,"status":"current","valid_until":T+1000,"components":[],
                    "combination_method":"baseline_plus_revisions","coherence":"broad_inherits_automatic_floor",
                    "curve":[{"hours":1,"probability":full}], "counterfactuals":{"without_community":[without]}}
        rows=paired_variants([payload(RELIEF,.3,.32),payload(AUTOMATIC,.5,.55)])
        self.assertEqual(rows[RELIEF]["without_community"]["points"],[.55])
        self.assertEqual(rows[AUTOMATIC]["without_community"]["points"],[.55])

    def test_private_provenance_does_not_leave_report(self):
        initialize(self.store, T, self.policy)
        self.store.add_forecast(
            forecast(
                source="SECRET_PLATFORM",
                private=True,
                track="community",
                metadata={"rule_reference": "https://private.example/contract"},
            )
        )
        self.store.indicator(
            "SECRET_PLATFORM",
            T,
            {
                "private": True,
                "kind": "count_distribution",
                "total": None,
                "expires": T + 10000,
                "rule_reference": "https://private.example/contract",
            },
        )
        report = build_report(
            self.store, T + 10, self.policy, {"events": [], "estimand": "test"}
        )
        content = json.dumps(report) + html_report(report)
        self.assertNotIn("SECRET_PLATFORM", content)
        self.assertNotIn("private.example", content)
        self.assertEqual(report["scorecards"][0]["scores"]["n"], 0)

    def test_writer_lock_prevents_duplicate_watchers(self):
        self.assertFalse(running(self.store.path))
        with exclusive(self.store.path):
            self.assertTrue(running(self.store.path))
        self.assertFalse(running(self.store.path))

    def test_startup_confirms_lock_and_reports_early_exit(self):
        process = Mock(pid=123)
        process.poll.return_value = 1
        with patch("reset_observer.runtime.running", return_value=False):
            self.assertEqual(
                started_worker(process, self.store.path, "test.log")["error"],
                "worker_exited",
            )
        # A concurrent winner is reused even if this launch lost the lock.
        with patch("reset_observer.runtime.running", return_value=True):
            self.assertEqual(
                started_worker(process, self.store.path, "test.log")["state"],
                "starting",
            )
        process.terminate.assert_not_called()

    def test_startup_timeout_terminates_only_its_own_unready_child(self):
        process = Mock()
        process.poll.return_value = None
        with patch("reset_observer.runtime.running", return_value=False):
            self.assertEqual(
                started_worker(process, self.store.path, "test.log", timeout=0)[
                    "error"
                ],
                "startup_timeout",
            )
        process.terminate.assert_called_once()
        process.wait.assert_called_once_with(timeout=2)

    def test_atomic_report_replacement_and_policy_isolation(self):
        output = self.root / "report.json"
        atomic_write(output, '{"value":1}')
        atomic_write(output, '{"value":2}')
        self.assertEqual(json.loads(output.read_text())["value"], 2)
        self.assertEqual(list(self.root.glob(".observer-*")), [])
        initialize(self.store, T, self.policy)
        with self.assertRaises(ValueError):
            initialize(self.store, T + 1, replace(self.policy, alert_threshold=0.8))


if __name__ == "__main__":
    unittest.main()
