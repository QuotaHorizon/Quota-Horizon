from dataclasses import replace

from reset_observer.benchmark import initialize, label_window, trial_probability
from reset_observer.contracts import iso
from reset_observer.diagnostics import coverage_details
from reset_observer.evidence import parse, reconcile
from reset_observer.reporting import build_report
from test_observer import T, TemporaryStore, forecast, trial


class LiveCompletionTests(TemporaryStore):
    def raw(self, at=T+3600):
        return {"updated_at": iso(at+60), "last_reset_at": iso(at), "latest_alert": {
            "id": "123", "kind": "reset", "state": "confirmed", "url": "https://x.com/thsottiaux/status/123",
            "confirmation_source": "operator_observed", "observed_at": iso(at),
            "confirmed_at": None, "confirmation_url": None,
            "observation_message": "Our checks observed a reset; awaiting the author's confirmation."}}

    def collect(self, raw=None, now=T+3660):
        raw = raw or self.raw()
        generated, healthy, claims = parse("codex_reset_status", raw, now)
        self.store.observation("codex_reset_status", now, generated, healthy, claims, raw)
        return reconcile(self.store, now, self.policy)["events"]

    def test_operator_observation_closes_old_cycle_without_faking_official_confirmation(self):
        events = self.collect()
        self.assertEqual(len(events), 1)
        e = events[0]
        self.assertEqual(e["status"], "pending")
        self.assertEqual(e["reason"], "insufficient_primary_confirmation")
        self.assertEqual(e["primary_references"], 1)
        self.assertNotIn("x.com", e["references"][0])
        f = forecast()
        self.assertEqual(trial_probability(f, T+7200, 24, events, self.policy), (None, "event_since_forecast"))
        # The same report cannot label a trial positive before corroboration.
        label = label_window(trial(), events, T+86400, self.policy, covered=True)
        self.assertIsNone(label["label"])
        self.assertEqual(label["status"], "unresolved")
        self.store.add_forecast(f)
        initialize(self.store, T, self.policy)
        report = build_report(self.store, T+7200, self.policy, reconcile(self.store, T+7200, self.policy))
        self.assertEqual(report["latest"][0]["state"], "cycle_closed")

    def test_occurrence_is_used_only_after_collection_and_retraction_is_versioned(self):
        events = self.collect()
        self.assertEqual(reconcile(self.store, T+3599, self.policy)["events"], [])
        self.assertIsNone(trial_probability(forecast(), T+3601, 24, events, self.policy)[1])
        raw = self.raw(); raw["latest_alert"]["state"] = "retracted"
        retracted = self.collect(raw, T+7300)
        self.assertEqual(retracted[0]["status"], "disputed")
        self.assertIsNone(trial_probability(forecast(), T+7300, 24, retracted, self.policy)[1])
        self.assertEqual(reconcile(self.store, T+3660, self.policy)["events"][0]["status"], "pending")

    def test_single_status_endpoint_never_proves_absence_or_reuses_old_promise(self):
        for offset in (0, 900, 1800):
            self.collect(self.raw(T+offset), T+offset+60)
        events = reconcile(self.store, T+1860, self.policy)["events"]
        self.assertEqual(len(events), 3)
        detail = coverage_details(self.store, T+60, T+1860, T+3600, self.policy)
        self.assertEqual(detail["qualified_providers"], 0)
        self.assertNotIn("codex_reset_status", [p["provider"] for p in detail["providers"]])

    def test_announcement_or_future_observation_cannot_close_cycle(self):
        raw = self.raw(); raw["latest_alert"]["state"] = "active"
        self.assertEqual(self.collect(raw), [])
        raw = self.raw(T+7200)
        with self.assertRaisesRegex(ValueError, "future completion"):
            parse("codex_reset_status", raw, T+3660)

    def test_updated_source_uses_new_cycle_and_stale_anchor_stays_excluded(self):
        events = self.collect()
        fresh = forecast(origin=T+3700, generated_at=T+3700, available_at=T+3700,
                         metadata={"last_reset_at": T+3600})
        self.assertIsNone(trial_probability(fresh, T+7200, 24, events, self.policy)[1])
        old = replace(fresh, metadata={"last_reset_at": T-86400})
        self.assertEqual(trial_probability(old, T+7200, 24, events, self.policy)[1], "source_missed_known_event")
