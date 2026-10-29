from dataclasses import replace
from pathlib import Path
import tempfile
import unittest

import numpy as np

from reset_intelligence.contracts import AUTOMATIC, RELIEF, Curve, Event, Observation
from reset_intelligence.community import ballot_curve
from reset_intelligence.audit import native_support, source_audit
from reset_intelligence.lifecycle import cycle_issues, live_cycle
from reset_intelligence.pipeline import forecast_all
from reset_intelligence.quant import ModelConfig, Survival
from reset_intelligence.store import Store
from reset_intelligence.sources import parse, reset_anchor
from test_engine import NOW, history


class LiveCycleTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.store = Store(Path(self.tmp.name) / "state.sqlite")
        self.historical = history()[-1].at
        self.reset = NOW - 600
        self.store.save("event_ledger", NOW, {}, Observation(
            "event_ledger", NOW, NOW, events=history()))
        self.codex = Curve("codex_reset", AUTOMATIC, NOW, NOW, NOW, NOW + 3600,
            [(24, .32), (48, .53)], last_reset_at=self.reset,
            metadata={"adapter": "codex_reset", "adapter_contract": 1,
                      "estimate_kind": "published_complete", "rounding_only": True,
                      "baseline_points": [(24, .32), (48, .53)]})

    def tearDown(self):
        self.store.close()
        self.tmp.cleanup()

    def save(self, curve, at=NOW):
        self.store.save(curve.source, at, {}, Observation(
            curve.source, at, curve.generated_at, curves=[curve]))

    def test_operator_reset_retires_old_high_forecasts_for_both_targets(self):
        monitor = Curve("monitor", AUTOMATIC, NOW - 7200, NOW - 7200, NOW,
            NOW + 3600, [(24, .94), (48, .97)], last_reset_at=self.historical,
            metadata={"adapter": "monitor", "adapter_contract": 1,
                      "revision_reviewed": True, "baseline_points": [(24, .2), (48, .4)]})
        self.save(monitor)
        before = forecast_all(self.store, NOW)
        self.assertGreater(before[1]["curve"][23]["probability"], .9)
        self.save(self.codex)
        after = forecast_all(self.store, NOW)
        for f in after:
            self.assertEqual(f["status"], "current")
            self.assertEqual(f["last_event_at"], self.reset)
            self.assertEqual(f["event_count"], 20)
            self.assertNotIn("monitor", [c["id"] for c in f["components"]])
            self.assertLess(f["curve"][23]["probability"], .5)
            self.assertEqual(f["components"][0]["metadata"]["intervals"], 19)
        health = next(h for h in after[1]["source_health"] if h["id"] == "monitor")
        self.assertIn("forecast_precedes_latest_event", health["issues"])
        self.assertIn("missed_latest_event", health["issues"])
        # A provider that has incorporated the new event is admitted again.
        self.save(replace(monitor, origin=NOW, generated_at=NOW,
                          last_reset_at=self.reset, points=[(24, .3), (48, .5)]), NOW + 1)
        self.assertIn("monitor", [c["id"] for c in forecast_all(self.store, NOW+1)[1]["components"]])

    def test_new_receipt_or_publication_cannot_freshen_old_reset_anchor(self):
        self.save(self.codex)
        old = replace(self.codex, source="stale", points=[(24, .94), (48, .97)],
                      last_reset_at=self.historical, metadata={})
        self.save(old)
        f = forecast_all(self.store, NOW)[1]
        self.assertNotIn("stale", [c["id"] for c in f["components"]])
        self.assertIn("missed_latest_event", next(h for h in f["source_health"] if h["id"] == "stale")["issues"])

    def test_known_occurrence_survives_forecast_expiry_and_request_failure(self):
        self.save(replace(self.codex, origin=NOW-600, generated_at=NOW-600, expires_at=NOW-1))
        self.store.save("codex_reset", NOW+1, None, None, "http_503")
        f = forecast_all(self.store, NOW+1)[1]
        self.assertEqual(f["last_event_at"], self.reset)
        self.assertNotIn("codex_reset", [c["id"] for c in f["components"]])

    def test_replay_uses_known_at_and_allows_source_corrections(self):
        before = forecast_all(self.store, NOW)
        self.save(self.codex, NOW+1)
        self.assertEqual(forecast_all(self.store, NOW), before)
        self.assertEqual(forecast_all(self.store, NOW+1)[1]["last_event_at"], self.reset)
        self.save(replace(self.codex, last_reset_at=self.historical), NOW+2)
        self.assertEqual(forecast_all(self.store, NOW+2)[1]["last_event_at"], self.historical)

    def test_only_trusted_completed_automatic_anchors_renew_the_cycle(self):
        for c in (
            replace(self.codex, last_reset_at=NOW+10),
            replace(self.codex, generated_at=NOW+120),
            replace(self.codex, private=True),
            replace(self.codex, target=RELIEF),
            replace(self.codex, issues=["unhealthy_outcome_history"]),
            replace(self.codex, source="custom", metadata={}),
        ):
            with self.subTest(curve=c):
                obs = Observation(c.source, NOW, NOW, curves=[c])
                self.assertEqual(live_cycle({c.source: obs}, self.historical, NOW)["at"], self.historical)

    def test_reset_changes_waiting_age_not_historical_training_events(self):
        times = [e.at for e in history()]
        old = Survival(times, NOW)
        new = Survival(times, NOW, renewal_at=self.reset)
        self.assertEqual(new.intervals, old.intervals)
        np.testing.assert_array_equal(new.alpha, old.alpha)
        self.assertAlmostEqual(new.age, 1/6)
        with self.assertRaises(ValueError):
            Survival(times, NOW, renewal_at=NOW+1)

    def test_ballot_opened_before_recent_completion_does_not_predict_next_reset(self):
        p = {"kind": "probability_votes", "start": self.reset-60,
             "expires": NOW+86400, "id": "today", "total": 100,
             "bins": [{"p": .95, "votes": 100}]}
        self.assertIsNone(ballot_curve(p, NOW, [24], Survival([e.at for e in history()], NOW),
                                      self.reset, ModelConfig()))

    def test_later_publication_proxies_cannot_displace_an_observed_cycle(self):
        self.save(self.codex)
        monitor = replace(self.codex, source="monitor", last_reset_at=NOW-60,
            metadata={"adapter": "monitor", "adapter_contract": 1,
                      "baseline_points": [(24, .2), (48, .4)], "revision_reviewed": True,
                      "reset_anchor": reset_anchor(NOW-60, "announcement_proxy", ["https://x.com/i/status/456"])},
            points=[(24, .94), (48, .96)])
        nextreset = replace(self.codex, source="nextreset", last_reset_at=self.reset-60,
            metadata={"adapter": "nextreset", "baseline_points": [(24, .3), (48, .5)],
                      "reset_anchor": reset_anchor(self.reset-60, "observed", ["https://x.com/i/status/123"], discovered_at=NOW-10)},
            points=[(24, .93), (48, .97)])
        self.save(monitor)
        self.save(nextreset)
        late = Event("late-post", NOW-30, NOW, "automatic", ["https://x.com/i/status/456"], "announcement_proxy")
        self.store.save("event_ledger", NOW, {}, Observation("event_ledger", NOW, NOW, events=[*history(), late]))
        result = forecast_all(self.store, NOW)[1]
        self.assertEqual(result["last_event_at"], self.reset)
        self.assertEqual(result["cycle"]["occurred_at"], self.reset)
        self.assertEqual({c["id"] for c in result["components"]}, {"survival_baseline", "codex_reset", "nextreset", "monitor"})
        self.assertEqual(result["event_audit"]["after_live_boundary"], ["late-post"])
        self.assertEqual(result["event_count"], 21)
        self.assertEqual(result["components"][0]["metadata"]["intervals"], 19)
        self.assertEqual({p["source"] for p in result["cycle"]["unresolved_proxies"]}, {"event_ledger", "monitor"})

    def test_legacy_monitor_proxy_is_safe_before_the_next_successful_fetch(self):
        self.save(self.codex)
        self.save(replace(self.codex, source="monitor", last_reset_at=NOW-60,
            metadata={"adapter": "monitor", "adapter_contract": 1}, points=[(24, .8), (48, .9)]))
        result = forecast_all(self.store, NOW)[1]
        self.assertEqual(result["last_event_at"], self.reset)
        self.assertIn("codex_reset", [c["id"] for c in result["components"]])

    def completion(self, url, at, stage="completed"):
        return {"kind": "statement", "id": url.rsplit("/", 1)[1], "url": url, "at": at,
                "stage": stage, "event_type": "usage_reset", "original": "No text interpretation"}

    def test_explicit_confirmation_closes_old_forecasts_without_fabricating_occurrence_time(self):
        fact = self.completion("https://x.com/i/status/789", NOW-100)
        observations = {"statements": Observation("statements", NOW, NOW, facts=[fact])}
        cycle = live_cycle(observations, self.historical, NOW, history())
        self.assertEqual(cycle["at"], NOW-100)
        self.assertEqual(cycle["time_basis"], "completion_confirmation")
        self.assertIsNone(cycle["occurred_at"])
        self.assertEqual(cycle["confirmation_at"], NOW-100)
        self.assertEqual(cycle_issues(replace(self.codex, origin=NOW-200), cycle), ["forecast_precedes_latest_event"])
        self.assertEqual(cycle_issues(self.codex, cycle), [])

    def test_exact_event_reference_prevents_late_confirmation_from_resetting_the_clock(self):
        url = "https://x.com/i/status/123"
        curve = replace(self.codex, metadata={**self.codex.metadata,
            "reset_anchor": reset_anchor(self.reset, "source_clock", [url])})
        observations = {"codex_reset": Observation("codex_reset", NOW, NOW, curves=[curve]),
                        "statements": Observation("statements", NOW, NOW, facts=[self.completion(url, NOW-100)])}
        cycle = live_cycle(observations, self.historical, NOW, history())
        self.assertEqual(cycle["at"], self.reset)
        self.assertEqual(cycle["references"], [url])
        corrected = replace(curve, last_reset_at=self.reset+60, metadata={**curve.metadata,
            "reset_anchor": reset_anchor(self.reset+60, "source_clock", [url])})
        corrected_cycle = live_cycle({**observations, "codex_reset": Observation("codex_reset", NOW, NOW, curves=[corrected])},
                                     self.historical, NOW, history())
        self.assertEqual(corrected_cycle["identity"], cycle["identity"])
        self.assertNotEqual(corrected_cycle["at"], cycle["at"])
        # A different event is never aliased merely because it is nearby or
        # shares a parent. Explicit completion then provides its own boundary.
        other = self.completion("https://x.com/i/status/124", NOW-99)
        for f in observations["statements"].facts + [other]:
            f["parent"] = {"id": "100", "url": "https://x.com/i/status/100"}
        observations["statements"].facts.append(other)
        cycle = live_cycle(observations, self.historical, NOW, history())
        self.assertEqual(cycle["at"], NOW-99)
        self.assertIsNone(cycle["occurred_at"])
        self.assertEqual(cycle["references"], [other["url"]])

    def test_proxy_only_history_and_conflicting_or_future_stages_cannot_expel_a_source(self):
        url = "https://x.com/i/status/123"
        for facts in ([], [self.completion(url, NOW+1)],
                      [self.completion(url, NOW-10), self.completion(url, NOW-10, "retracted")]):
            with self.subTest(facts=facts):
                obs = Observation("statements", NOW, NOW, facts=facts)
                proxy = Event("proxy", NOW-60, NOW, "automatic", [url], "announcement_proxy")
                proxies = [replace(event, precision="announcement_proxy") for event in history()]
                cycle = live_cycle({"statements": obs}, proxy.at, NOW, [*proxies, proxy])
                self.assertIsNone(cycle["invalidation_at"])
                self.assertEqual(cycle["time_basis"], "historical_background")
                self.assertEqual(cycle_issues(replace(self.codex, origin=NOW-120, last_reset_at=self.historical), cycle), [])
                current = replace(self.codex, origin=NOW-120, last_reset_at=self.historical)
                self.assertTrue(native_support(current, NOW, {"last_event_at": proxy.at, "cycle": cycle}, None)["current"])

    def test_audit_uses_the_same_explicit_event_identity_as_admission(self):
        url = "https://x.com/i/status/123"
        curve = replace(self.codex, last_reset_at=NOW-7200, metadata={**self.codex.metadata,
            "reset_anchor": reset_anchor(NOW-7200, "announcement_proxy", [url])})
        cycle = {"at": self.reset, "occurred_at": self.reset, "invalidation_at": self.reset, "references": [url]}
        result = {"last_event_at": self.reset, "cycle": cycle}
        self.assertEqual(cycle_issues(curve, cycle), [])
        self.assertTrue(native_support(curve, NOW, result, None)["current"])
        old = {"last_event_at": self.reset}
        self.assertFalse(native_support(curve, NOW, old, None)["current"])

    def test_later_catalogue_proxy_does_not_double_count_waiting_exposure(self):
        times = [e.at for e in history()]
        expected = Survival(times, NOW, renewal_at=self.reset)
        actual = Survival([*times, NOW-60], NOW, renewal_at=self.reset)
        self.assertEqual(actual.intervals, expected.intervals)
        self.assertEqual(actual.age, expected.age)
        np.testing.assert_array_equal(actual.alpha, expected.alpha)
        np.testing.assert_array_equal(actual.beta, expected.beta)

    def test_sparse_pre_boundary_history_returns_unavailable_without_discarding_catalogue(self):
        events = [Event(str(i), NOW-(10-i)*86400, NOW, "automatic", [],
                        precision="announcement_proxy") for i in range(10)]
        self.store.save("event_ledger", NOW, {}, Observation(
            "event_ledger", NOW, NOW, events=events))
        boundary = events[1].at
        self.save(replace(self.codex, last_reset_at=boundary))
        results = forecast_all(self.store, NOW)
        for result in results:
            self.assertEqual(result["status"], "unavailable")
            self.assertEqual(result["reason"], "event_history_before_live_boundary_insufficient")
            self.assertEqual(result["event_count"], 10)
            self.assertEqual(result["event_audit"]["usable_distinct_times"], 2)
            self.assertEqual(result["event_audit"]["after_live_boundary"], [str(i) for i in range(2, 10)])
            self.assertEqual(result["curve"], [])
        self.assertEqual(len(self.store.observations(NOW)["event_ledger"].events), 10)
        self.assertTrue(all(target["reason"] == "event_history_before_live_boundary_insufficient"
                            for target in source_audit(self.store, NOW, results)["targets"]))


class ResetAnchorAdapterTests(unittest.TestCase):
    def test_source_clocks_references_and_discovery_are_preserved_separately(self):
        codex = {"updated_at": NOW, "last_reset_at": NOW-600, "model": {"version": "test"},
                 "probabilities": {"rounded_24h": 30, "rounded_48h": 50, "raw_24h": .3, "raw_48h": .5},
                 "evidence": [{"code": "last_reset", "href": "https://x.com/author/status/123"}]}
        monitor = {"updatedAt": NOW, "status": "ok", "forecastStatus": "current",
                   "history": [{"type": "forced-reset", "dateTime": NOW-60, "id": "reset:456",
                                "sourceUrl": "https://x.com/author/status/456"}],
                   "forecast": {"score24h": 90, "score48h": 95}}
        nextreset = {"asOf": NOW, "expiresAt": NOW+900, "version": "test", "degraded": False,
                     "checks": {"history": {"ok": True}},
                     "windows": [{"hours": 24, "probability": .9}, {"hours": 48, "probability": .95}],
                     "history": {"last": {"date": NOW-600, "type": "automatic", "timeBasis": "feed_observation",
                                          "source": "https://x.com/author/status/123", "discoveredAt": NOW-30}}}
        anchors = {name: parse({"id": name, "adapter": name}, raw, NOW).curves[0].metadata["reset_anchor"]
                   for name, raw in (("codex_reset", codex), ("monitor", monitor), ("nextreset", nextreset))}
        self.assertEqual(anchors["codex_reset"]["occurred_at"], NOW-600)
        self.assertEqual(anchors["codex_reset"]["references"], ["https://x.com/i/status/123"])
        self.assertIsNone(anchors["monitor"]["occurred_at"])
        self.assertEqual(anchors["monitor"]["announced_at"], NOW-60)
        self.assertEqual(anchors["nextreset"]["occurred_at"], NOW-600)
        self.assertEqual(anchors["nextreset"]["discovered_at"], NOW-30)
        for name, raw in (("codex_reset", codex), ("monitor", monitor), ("nextreset", nextreset)):
            self.assertNotIn("received_at", anchors[name])
            self.assertEqual(parse({"id": name, "adapter": name}, raw, NOW+1).curves[0].metadata["reset_anchor"], anchors[name])


if __name__ == "__main__":
    unittest.main()
