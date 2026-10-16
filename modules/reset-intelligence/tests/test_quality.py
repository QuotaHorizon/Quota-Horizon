from copy import deepcopy
import json
import unittest
from unittest.mock import patch

import test_engine as fixtures
from test_engine import NOW, history
from reset_intelligence.audit import source_audit
from reset_intelligence.community import ballot_curve, count_curve, count_rejection, parse_count_market
from reset_intelligence.contracts import AUTOMATIC, RELIEF, Curve, Observation, iso
from reset_intelligence.evaluation import canonical_events
from reset_intelligence.pipeline import align_curve, complete_tail, forecast
from reset_intelligence.quant import ModelConfig, Survival
from reset_intelligence.sources import collect, parse


class SourceQualityTests(unittest.TestCase):
    setUp = fixtures.LedgerTests.setUp
    tearDown = fixtures.LedgerTests.tearDown

    def test_audit_counts_keep_failures_and_the_seven_day_boundary(self):
        self.store.save("event_ledger", NOW-8*86400, None, None, "old_failure")
        self.store.save("event_ledger", NOW-60, None, None, "recent_failure")
        f = forecast(self.store, NOW, RELIEF)
        row = next(r for r in source_audit(self.store, NOW, [f])["sources"] if r["source"] == "event_ledger")
        self.assertEqual((row["seven_day_attempts"], row["decoded_attempts"], row["failed_attempts"]), (2,1,1))

    def test_same_poll_round_mirror_does_not_add_weight(self):
        poll = {"kind":"probability_votes", "target":RELIEF, "population":"same_voters", "id":"round",
                "start":NOW-3600, "expires":NOW+86400, "total":50, "bins":[{"p":.9,"votes":50}]}
        for source in ("a", "mirror"):
            self.store.save(source, NOW, {}, Observation(source, NOW, NOW, polls=[poll]))
        f = forecast(self.store, NOW, RELIEF)
        self.assertEqual(sum(c["family"] == "community" for c in f["components"]), 1)
        self.assertIn("duplicate_poll_round", [issue for h in f["source_health"] for issue in h["issues"]])
        self.assertLess(f["community_weight"], .6)

    def test_empty_poll_has_specific_exclusion_and_audit_preserves_actual_weight(self):
        poll = {"kind":"probability_votes", "target":RELIEF, "population":"sample", "id":"round",
                "start":NOW-3600, "expires":NOW+86400, "total":50, "bins":[{"p":.9,"votes":50}]}
        self.store.save("votes", NOW, {}, Observation("votes", NOW, NOW, polls=[poll]))
        self.store.save("private_name", NOW, {}, Observation("private_name", NOW, NOW, polls=[dict(poll, total=0, private=True)]))
        f = forecast(self.store, NOW, RELIEF, private_source_ids={"private_name"})
        before = self.store.db.total_changes
        audit = source_audit(self.store, NOW, [f])
        self.assertEqual(self.store.db.total_changes, before)
        row = next(r for r in audit["sources"] if r["source"] == "votes")
        self.assertAlmostEqual(row["issued_contribution"]["ballot_weight_before_projection"], f["community_weight"])
        self.assertLess(f["community_weight"], .6)
        self.assertEqual(row["issued_contribution"]["weight_state"], "cold_start_prior")
        self.assertEqual(audit["targets"][0]["community_dependency_groups"], 1)
        private = next(r for r in audit["sources"] if r["private"])
        self.assertEqual(private["reasons"], ["empty_poll"])
        self.assertEqual(private["admission"], "excluded")
        self.assertEqual(next(r for r in audit["sources"] if r["source"] == "survival_baseline")["issued_contribution"]["background_anchor_weight"], 1)
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


class AdmissionAuditTests(unittest.TestCase):
    setUp = fixtures.LedgerTests.setUp
    tearDown = fixtures.LedgerTests.tearDown

    def save_curve(
        self,
        source,
        points=None,
        baseline=None,
        reviewed=False,
        age=0,
        expires=None,
        last_reset=None,
        private=False,
        target=RELIEF,
    ):
        c = Curve(
            source,
            target,
            NOW - age * 3600,
            NOW - age * 3600,
            NOW,
            NOW + 3600 if expires is None else expires,
            points or [(24, 0.2), (48, 0.4)],
            dependency=source,
            last_reset_at=last_reset,
            private=private,
            metadata={"baseline_points": baseline, "revision_reviewed": reviewed},
        )
        self.store.save(source, NOW, {}, Observation(source, NOW, NOW, curves=[c]))
        return c

    def audit(self, f, at=NOW):
        before = deepcopy(f)
        changes = self.store.db.total_changes
        result = source_audit(self.store, at, [f])
        self.assertEqual(f, before)
        self.assertEqual(self.store.db.total_changes, changes)
        return result, {r["source"]: r for r in result["sources"]}

    def test_comparison_is_included_but_has_no_numerical_contribution(self):
        original = forecast(self.store, NOW, RELIEF)
        self.save_curve("comparison")
        f = forecast(self.store, NOW, RELIEF)
        audit, rows = self.audit(f)
        r = rows["comparison"]
        self.assertEqual(r["admission"], "included")
        self.assertEqual(
            r["participation"],
            {
                "role": "comparison",
                "background_anchor": False,
                "information_revision": False,
                "comparison_only": True,
                "community_residual": False,
            },
        )
        self.assertEqual(
            r["issued_contribution"]["component_weight_before_projection"], 0
        )
        self.assertEqual(
            [p["probability"] for p in f["curve"]],
            [p["probability"] for p in original["curve"]],
        )
        self.assertEqual(audit["targets"][0]["radar_coverage"]["comparison_only"], 1)
        self.assertEqual(
            audit["targets"][0]["radar_coverage"]["information_revision"], 0
        )

    def test_background_has_anchor_despite_zero_revision_weight(self):
        self.save_curve("background", baseline=[(24, 0.2), (48, 0.4)])
        f = forecast(self.store, NOW, RELIEF)
        _, rows = self.audit(f)
        r = rows["background"]
        self.assertTrue(r["participation"]["background_anchor"])
        self.assertFalse(r["participation"]["information_revision"])
        self.assertEqual(
            r["issued_contribution"]["component_weight_before_projection"], 0
        )
        self.assertEqual(r["issued_contribution"]["background_anchor_weight"], 1)
        self.assertAlmostEqual(f["curve"][23]["probability"], 0.2)

    def test_negative_and_reviewed_neutral_revisions_keep_eligibility(self):
        self.save_curve("negative", baseline=[(24, 0.3), (48, 0.5)])
        self.save_curve("neutral", baseline=[(24, 0.2), (48, 0.4)], reviewed=True)
        f = forecast(self.store, NOW, RELIEF)
        audit, rows = self.audit(f)
        for name in ("negative", "neutral"):
            self.assertTrue(rows[name]["participation"]["information_revision"])
            self.assertEqual(
                rows[name]["issued_contribution"]["component_weight_before_projection"],
                0.5,
            )
        self.assertAlmostEqual(
            rows["negative"]["issued_contribution"]["revision_24h_before_projection"],
            -0.05,
        )
        self.assertEqual(
            rows["neutral"]["issued_contribution"]["revision_24h_before_projection"], 0
        )
        self.assertEqual(
            audit["targets"][0]["radar_coverage"]["information_revision"], 2
        )
        self.assertAlmostEqual(f["curve"][23]["probability"], 0.2)

    def test_expired_and_missed_event_are_both_reported_without_numerical_change(self):
        original = forecast(self.store, NOW, RELIEF)
        self.save_curve("old", expires=NOW, last_reset=history()[-1].at - 7200)
        f = forecast(self.store, NOW, RELIEF)
        _, rows = self.audit(f)
        self.assertEqual(rows["old"]["reasons"], ["expired", "missed_latest_event"])
        self.assertEqual(rows["old"]["admission"], "excluded")
        self.assertEqual(f["curve"], original["curve"])
        self.assertEqual(f["components"], original["components"])

    def test_native_deadline_distinct_from_freshness_and_baseline_tail(self):
        c = self.save_curve("short", points=[(6, 0.05), (24, 0.2)], age=0.1)
        f = forecast(self.store, NOW, RELIEF)
        audit, rows = self.audit(f)
        native = rows["short"]["native_support"][0]
        self.assertTrue(native["current"])
        self.assertAlmostEqual(native["remaining_hours"], 23.9)
        self.assertEqual(native["native_deadline"], iso(c.origin + 24 * 3600))
        self.assertEqual(native["valid_until"], iso(NOW + 3600))
        for h in (24, 48):
            self.assertEqual(
                native["windows"][str(h)],
                {"native_supported": False, "aligned_value_basis": "baseline_tail"},
            )
            self.assertEqual(
                audit["targets"][0]["radar_coverage"]["windows"][str(h)],
                {"native": 0, "baseline_tail": 1},
            )

    def test_native_boundary_and_excluded_source_do_not_claim_model_tail(self):
        self.save_curve("native")
        self.save_curve("expired", expires=NOW)
        _, rows = self.audit(forecast(self.store, NOW, RELIEF))
        for h in (24, 48):
            self.assertEqual(
                rows["native"]["native_support"][0]["windows"][str(h)],
                {"native_supported": True, "aligned_value_basis": "native"},
            )
            self.assertEqual(
                rows["expired"]["native_support"][0]["windows"][str(h)],
                {"native_supported": False, "aligned_value_basis": "not_included"},
            )

    def test_later_observations_and_failures_do_not_mix_with_issued_snapshot(self):
        self.save_curve("source", points=[(24, 0.2)])
        f = forecast(self.store, NOW, RELIEF)
        first, _ = self.audit(f)
        self.store.forecast(f)
        newer = Curve(
            "source", RELIEF, NOW + 600, NOW + 600, NOW + 600, NOW + 4200, [(72, 0.9)]
        )
        self.store.save(
            "source",
            NOW + 600,
            {},
            Observation("source", NOW + 600, NOW + 600, curves=[newer]),
        )
        self.store.save("source", NOW + 700, None, None, "later_failure")
        later, _ = self.audit(f, NOW + 800)
        self.assertEqual(first["sources"], later["sources"])
        self.assertEqual(first["targets"], later["targets"])
        self.assertEqual(
            source_audit(self.store, NOW + 800)["sources"], first["sources"]
        )

    def test_each_target_uses_its_own_issue_time_and_private_radars_are_not_counted(
        self,
    ):
        self.save_curve("private_fixture", private=True)
        broad = forecast(
            self.store, NOW, RELIEF, private_source_ids={"private_fixture"}
        )
        later = forecast(
            self.store, NOW + 100, AUTOMATIC, private_source_ids={"private_fixture"}
        )
        audit = source_audit(self.store, NOW + 800, [broad, later])
        self.assertEqual(audit["targets"][0]["radar_coverage"]["included"], 0)
        self.assertNotIn("private_fixture", json.dumps(audit))
        for row in audit["sources"]:
            self.assertEqual(
                row["issued_at"], iso(NOW if row["target"] == RELIEF else NOW + 100)
            )


class AdapterContractTests(unittest.TestCase):
    setUp = fixtures.LedgerTests.setUp
    tearDown = fixtures.LedgerTests.tearDown

    def lunar(self):
        return {
            "generatedAt": NOW,
            "lastResetAt": history()[-1].at,
            "nearTerm": {"hours": 6, "probability": 0.059},
            "next24Hours": {"probability": 0.264, "low": 0.2, "high": 0.3},
            "comparisons": {
                "modelVersion": "fixture",
                "withoutHints": {"sixHours": 0.059, "twentyFourHours": 0.264},
            },
        }

    def observatory(self):
        return {
            "checkedAt": NOW,
            "lastRandomResetAt": history()[-1].at,
            "dataHealth": {"stale": False, "overall": "healthy"},
            "viewModel": {
                "probability12h": 0.11,
                "probability24h": 0.35,
                "probability48h": 0.58,
                "probability72h": 0.77,
                "regularResetForecast": {"lastCompletedAt": NOW - 10 * 86400},
            },
        }

    def monitor(self):
        return {
            "updatedAt": NOW,
            "history": [],
            "status": "paused",
            "forecastStatus": "paused",
            "forecast": {
                "score24h": 20,
                "score48h": 38,
                "baseScore24h": 27,
                "baseScore48h": 47,
            },
        }

    def save_source(self, source, raw, at=NOW):
        obs = parse({"id": source, "adapter": source}, raw, at)
        self.store.save(source, at, raw, obs)
        return obs

    def legacy_source(self, source, at=NOW + 600):
        raw = getattr(self, source)()
        if source == "monitor":
            raw.update(forecastStatus="current")
        obs = parse({"id": source, "adapter": source}, raw, at)
        c = obs.curves[0]
        c.metadata.pop("adapter_contract", None)
        c.metadata.pop("adapter", None)
        if source == "lunar":
            c.expires_at = NOW + 3600
        elif source == "observatory":
            c.last_reset_at = raw["viewModel"]["regularResetForecast"][
                "lastCompletedAt"
            ]
        self.store.save(source, at, raw, obs)
        return raw, obs

    def test_upgrade_rejects_old_ttl_and_anchor_without_rewriting_history(self):
        for source in ("lunar", "observatory", "monitor"):
            self.legacy_source(source)
        before = [
            tuple(r)
            for r in self.store.db.execute("SELECT * FROM observations ORDER BY id")
        ]
        for source, target in (
            ("lunar", RELIEF),
            ("observatory", RELIEF),
            ("monitor", AUTOMATIC),
        ):
            with self.subTest(source=source):
                f = forecast(self.store, NOW + 601, target)
                self.assertNotIn(source, [c["id"] for c in f["components"]])
                row = next(
                    r
                    for r in source_audit(self.store, NOW + 601, [f])["sources"]
                    if r["source"] == source
                )
                self.assertIn("source_normalization_outdated", row["reasons"])
                self.assertFalse(row["native_support"][0]["current"])
                self.assertFalse(row["participation"]["background_anchor"])
        self.assertEqual(
            before,
            [
                tuple(r)
                for r in self.store.db.execute("SELECT * FROM observations ORDER BY id")
            ],
        )

    def test_upgrade_first_success_appends_and_restores_current_contract(self):
        for source in ("lunar", "observatory", "monitor"):
            with self.subTest(source=source):
                raw, _ = self.legacy_source(source)
                before = tuple(
                    self.store.db.execute(
                        "SELECT * FROM observations WHERE source=? ORDER BY id LIMIT 1",
                        (source,),
                    ).fetchone()
                )
                raw[
                    {
                        "lunar": "generatedAt",
                        "observatory": "checkedAt",
                        "monitor": "updatedAt",
                    }[source]
                ] = NOW + 601
                spec = {"id": source, "adapter": source}
                fresh = parse(spec, raw, NOW + 601)
                with (
                    patch(
                        "reset_intelligence.sources.time.time", return_value=NOW + 601
                    ),
                    patch(
                        "reset_intelligence.sources.fetch",
                        return_value=(spec, NOW + 601, raw, fresh, None),
                    ) as fetch,
                ):
                    self.assertEqual(
                        collect(self.store, [spec])[0]["status"], "collected"
                    )
                    fetch.assert_called_once_with(spec)
                    self.assertEqual(collect(self.store, [spec])[0]["status"], "cached")
                    fetch.assert_called_once()
                self.assertEqual(
                    before,
                    tuple(
                        self.store.db.execute(
                            "SELECT * FROM observations WHERE source=? ORDER BY id LIMIT 1",
                            (source,),
                        ).fetchone()
                    ),
                )
                target = AUTOMATIC if source == "monitor" else RELIEF
                f = forecast(self.store, NOW + 601, target)
                self.assertIn(source, [c["id"] for c in f["components"]])
                row = next(
                    r
                    for r in source_audit(self.store, NOW + 601, [f])["sources"]
                    if r["source"] == source
                )
                self.assertTrue(row["native_support"][0]["current"])

    def test_upgrade_failure_keeps_backoff_and_old_curve_excluded(self):
        for source, error, delay in (
            ("lunar", "URLError", 60),
            ("observatory", "http_429", 900),
            ("monitor", "http_403", 900),
        ):
            with self.subTest(source=source):
                self.legacy_source(source)
                spec = {"id": source, "adapter": source}
                with (
                    patch(
                        "reset_intelligence.sources.time.time", return_value=NOW + 601
                    ),
                    patch(
                        "reset_intelligence.sources.fetch",
                        return_value=(spec, NOW + 601, None, None, error),
                    ) as fetch,
                ):
                    self.assertEqual(collect(self.store, [spec])[0]["status"], "error")
                    fetch.assert_called_once()
                with (
                    patch(
                        "reset_intelligence.sources.time.time",
                        return_value=NOW + 600 + delay,
                    ),
                    patch("reset_intelligence.sources.fetch") as fetch,
                ):
                    self.assertEqual(collect(self.store, [spec])[0]["status"], "cached")
                    fetch.assert_not_called()
                target = AUTOMATIC if source == "monitor" else RELIEF
                self.assertNotIn(
                    source,
                    [
                        c["id"]
                        for c in forecast(self.store, NOW + 661, target)["components"]
                    ],
                )
                with (
                    patch(
                        "reset_intelligence.sources.time.time",
                        return_value=NOW + 601 + delay,
                    ),
                    patch(
                        "reset_intelligence.sources.fetch",
                        return_value=(spec, NOW + 601 + delay, None, None, error),
                    ) as fetch,
                ):
                    self.assertEqual(collect(self.store, [spec])[0]["status"], "error")
                    fetch.assert_called_once()

    def test_contract_binding_keeps_custom_sources_and_other_adapters(self):
        c = Curve("isolated", RELIEF, NOW, NOW, NOW, NOW + 3600, [(24, 0.2)])
        self.store.save(
            "isolated", NOW, {}, Observation("isolated", NOW, NOW, curves=[c])
        )
        self.assertIn(
            "isolated",
            [c["id"] for c in forecast(self.store, NOW, RELIEF)["components"]],
        )
        raw = self.lunar()
        spec = {"id": "custom_lunar", "adapter": "lunar"}
        obs = parse(spec, raw, NOW)
        self.assertEqual(obs.curves[0].metadata.get("adapter"), "lunar")
        self.assertEqual(obs.curves[0].metadata.get("adapter_contract"), 1)
        obs.curves[0].metadata["adapter_contract"] = 0
        self.store.save(spec["id"], NOW, raw, obs)
        self.assertNotIn(
            spec["id"],
            [c["id"] for c in forecast(self.store, NOW, RELIEF)["components"]],
        )
        with (
            patch("reset_intelligence.sources.time.time", return_value=NOW + 1),
            patch(
                "reset_intelligence.sources.fetch",
                return_value=(spec, NOW + 1, None, None, "URLError"),
            ) as fetch,
        ):
            collect(self.store, [spec])
            fetch.assert_called_once_with(spec)
        # No new contract requirement for adapters outside this upgrade.
        other = {"id": "other", "adapter": "nextreset"}
        c.source = "other"
        c.points = [(24, 0.3)]
        c.metadata.update(decomposition_version=1, response_version=2)
        self.store.save("other", NOW, {}, Observation("other", NOW, NOW, curves=[c]))
        with (
            patch("reset_intelligence.sources.time.time", return_value=NOW + 1),
            patch("reset_intelligence.sources.fetch") as fetch,
        ):
            self.assertEqual(collect(self.store, [other])[0]["status"], "cached")
            fetch.assert_not_called()
        self.assertIn(
            "other", [c["id"] for c in forecast(self.store, NOW, RELIEF)["components"]]
        )

    def test_lunar_native_points_and_complete_paired_baseline_unchanged(self):
        c = self.save_source("lunar", self.lunar()).curves[0]
        self.assertEqual(c.points, [(6.0, 0.059), (24.0, 0.264)])
        self.assertEqual(c.metadata["baseline_points"], c.points)
        f = forecast(self.store, NOW, RELIEF)
        item = next(c for c in f["components"] if c["id"] == "lunar")
        self.assertEqual(item["role"], "background")
        self.assertEqual(item["metadata"]["source_coverage_hours"], 24)
        self.assertEqual(
            item["metadata"]["alignment"], "condition_on_no_event_then_baseline_tail"
        )
        self.assertEqual([tuple(p) for p in item["source_points"]], c.points)

    def test_lunar_null_or_incomplete_baseline_is_comparison(self):
        for comparison in (
            None,
            {},
            {"modelVersion": "fixture", "withoutHints": None},
            {"withoutHints": {"sixHours": 0.059}},
            {"withoutHints": {"twentyFourHours": 0.264}},
            {"withoutHints": {"sixHours": None, "twentyFourHours": 0.264}},
        ):
            with self.subTest(comparison=comparison):
                raw = self.lunar()
                raw["comparisons"] = comparison
                c = self.save_source("lunar", raw).curves[0]
                self.assertEqual(c.points, [(6.0, 0.059), (24.0, 0.264)])
                self.assertIsNone(c.metadata.get("baseline_points"))
                item = next(
                    c
                    for c in forecast(self.store, NOW, RELIEF)["components"]
                    if c["id"] == "lunar"
                )
                self.assertEqual(item["role"], "comparison")
                self.assertIsNone(item["signal_baseline"])

    def test_lunar_six_hour_baseline_never_attaches_to_twelve_hours(self):
        raw = self.lunar()
        raw["nearTerm"].update(hours=12, probability=0.15)
        c = self.save_source("lunar", raw).curves[0]
        self.assertEqual(c.points, [(12.0, 0.15), (24.0, 0.264)])
        self.assertIsNone(c.metadata.get("baseline_points"))
        item = next(
            c
            for c in forecast(self.store, NOW, RELIEF)["components"]
            if c["id"] == "lunar"
        )
        self.assertEqual(item["role"], "comparison")

    def test_lunar_freshness_boundary_uses_generation_not_observation(self):
        c = self.save_source("lunar", self.lunar(), NOW + 590).curves[0]
        self.assertEqual(c.expires_at, NOW + 600)
        self.assertEqual(c.observed_at, NOW + 590)
        base = Survival([e.at for e in history()], NOW)
        for age, usable in ((599, True), (600, False), (601, False)):
            with self.subTest(age=age):
                self.assertEqual(
                    align_curve(c, NOW + age, [6, 24], base) is not None, usable
                )
        self.store.save("lunar", NOW + 599, None, None, "http_503")
        self.assertEqual(
            [
                tuple(p)
                for p in self.store.observations(NOW + 601)["lunar"].curves[0].points
            ],
            c.points,
        )
        for age, usable in ((599, True), (600, False), (601, False)):
            f = forecast(self.store, NOW + age, RELIEF)
            self.assertEqual(any(c["id"] == "lunar" for c in f["components"]), usable)

    def test_observatory_uses_random_anchor_and_preserves_four_points(self):
        raw = self.observatory()
        c = self.save_source("observatory", raw).curves[0]
        self.assertEqual(c.last_reset_at, raw["lastRandomResetAt"])
        self.assertEqual(
            c.points, [(12.0, 0.11), (24.0, 0.35), (48.0, 0.58), (72.0, 0.77)]
        )
        self.assertEqual(c.target, RELIEF)
        item = next(
            c
            for c in forecast(self.store, NOW, RELIEF)["components"]
            if c["id"] == "observatory"
        )
        self.assertEqual(item["role"], "comparison")

    def test_observatory_missing_random_anchor_never_substitutes_regular(self):
        for missing in (False, True):
            raw = self.observatory()
            if missing:
                raw.pop("lastRandomResetAt")
            else:
                raw["lastRandomResetAt"] = None
            obs = self.save_source("observatory", raw)
            self.assertIsNone(obs.curves[0].last_reset_at)
            self.assertIn("missing_random_reset_anchor", obs.issues)
            self.assertEqual(obs.curves[0].issues, [])
            self.assertTrue(
                any(
                    c["id"] == "observatory"
                    for c in forecast(self.store, NOW, RELIEF)["components"]
                )
            )

    def test_observatory_stale_and_expiry_still_exclude(self):
        raw = self.observatory()
        raw["dataHealth"]["stale"] = True
        c = self.save_source("observatory", raw).curves[0]
        base = Survival([e.at for e in history()], NOW)
        self.assertIn("upstream_stale", c.issues)
        self.assertIsNone(align_curve(c, NOW, [24], base))
        raw["dataHealth"]["stale"] = False
        c = self.save_source("observatory", raw).curves[0]
        self.assertIsNone(align_curve(c, NOW + 3600, [24], base))

    def test_monitor_empty_forced_history_retains_points_and_pause_exclusion(self):
        for rows in ([], [{"type": "regular-reset", "dateTime": NOW - 60}]):
            raw = self.monitor()
            raw["history"] = rows
            obs = self.save_source("monitor", raw)
            c = obs.curves[0]
            self.assertIsNone(c.last_reset_at)
            self.assertIn("missing_forced_reset_anchor", obs.issues)
            self.assertEqual(c.points, [(24.0, 0.2), (48.0, 0.38)])
            self.assertEqual(c.metadata["baseline_points"], [(24, 0.27), (48, 0.47)])
            self.assertIn("upstream_forecast_not_current", c.issues)
            self.assertFalse(
                any(
                    c["id"] == "monitor"
                    for c in forecast(self.store, NOW, AUTOMATIC)["components"]
                )
            )

    def test_monitor_empty_history_does_not_renew_expired_forecast(self):
        raw = self.monitor()
        raw.update(updatedAt=NOW - 7 * 3600, forecastStatus="current")
        c = self.save_source("monitor", raw).curves[0]
        self.assertIsNone(c.last_reset_at)
        self.assertEqual(c.expires_at, NOW)
        self.assertFalse(
            any(
                c["id"] == "monitor"
                for c in forecast(self.store, NOW, AUTOMATIC)["components"]
            )
        )


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
                "start":NOW, "expires":NOW+1.5*3600, "total":100,
                "bins":[{"start":NOW,"end":NOW+1.5*3600,"votes":80},
                        {"start":NOW+1.5*3600,"end":None,"votes":20}]}
        out = ballot_curve(poll, NOW, [1,2], base, NOW-10000, ModelConfig())
        self.assertEqual(out["coverage_hours"], 1.5)
        curve = complete_tail(out["probabilities"], [1,2], base, (out["coverage_hours"],out["terminal_probability"]))
        self.assertAlmostEqual(curve[1], 1-(1-out["terminal_probability"])*(1-base.p(2))/(1-base.p(1.5)))


if __name__ == "__main__":
    unittest.main()
