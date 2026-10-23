from contextlib import closing
from dataclasses import replace
from pathlib import Path
import tempfile
import unittest

from reset_observer.contracts import Claim, Forecast, Policy, RELIEF
from reset_observer.evidence import reconcile
from reset_observer.reaction import initialize, record_forecast, report
from reset_observer.reporting import reaction_html
from reset_observer.store import Store

T = 1_792_000_000.


def observed(at):
    return {"last_reset_at": at, "reset_anchor": {"occurred_at": at, "time_basis": "observed"}}


def forecast(**changes):
    values = dict(source="radar", version="v1", target=RELIEF, track="native", origin=T-60,
                  generated_at=T-60, available_at=T-30, expires_at=T+7200,
                  points=[[24,.95],[48,.98]], metadata=observed(T-86400))
    values.update(changes)
    return Forecast(**values)


def completion():
    return Claim("public_catalogue", "event", ["https://x.com/i/status/123"], ["https://x.com/i/status/123"],
                 "automatic", "completed", "broad", T, T, T+60, "All limits reset", "official_relay",
                 "public_completion_notice", T).to_dict()


class ReactionClocks(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.store = Store(Path(self.directory.name)/"observer.sqlite")
        c = completion()
        self.store.observation(c["provider"], c["observed_at"], T, True, [c], {})

    def tearDown(self):
        self.store.close()
        self.directory.cleanup()

    def capture(self, value, received=None):
        self.store.add_forecast(value)
        record_forecast(self.store, value, received)
        self.store.db.commit()

    def result(self, now=T+600):
        return report(self.store, now, reconcile(self.store, now, Policy()), Policy())["events"][0]

    def test_event_capture_model_update_and_collection_clocks_remain_separate(self):
        self.capture(forecast(), T-40)
        self.capture(forecast(origin=T+100, generated_at=T+100, available_at=T+120), T+110)
        self.capture(forecast(origin=T+300, generated_at=T+300, available_at=T+360,
                              metadata=observed(T), points=[[24,.4],[48,.6]]), T+330)
        event = self.result()
        self.assertEqual(event["first_received_at"], T+60)
        self.assertEqual(event["evidence"][0]["published_to_capture_seconds"], 60)
        row = event["series"][0]
        self.assertEqual(row["state"], "updated")
        self.assertEqual(row["publication_to_capture_seconds"], 30)
        self.assertEqual(row["completion_to_update_seconds"], 300)
        self.assertEqual(row["known_to_update_seconds"], 240)
        self.assertEqual(row["old_cycle_residual_seconds"], 300)
        self.assertEqual((row["before_probability_24h"], row["after_probability_24h"]), (.95,.4))
        self.assertIsNone(row["ui_received_at"])
        self.assertIsNone(row["ui_delay_seconds"])

    def test_probability_drop_and_announcement_clock_do_not_fake_a_cycle_response(self):
        self.capture(forecast())
        self.capture(forecast(origin=T+100, generated_at=T+100, available_at=T+120,
            points=[[24,.01],[48,.02]], metadata={"last_reset_at": T,
                "reset_anchor": {"occurred_at": None, "announced_at": T, "time_basis": "announcement_proxy"}}))
        row = self.result()["series"][0]
        self.assertEqual(row["state"], "anchor_unknown")
        self.assertIsNone(row["completion_to_update_seconds"])
        self.assertIsNone(row["after_probability_24h"])

    def test_active_old_cycle_residual_is_censored_until_response(self):
        self.capture(forecast())
        self.capture(forecast(origin=T+100, generated_at=T+100, available_at=T+120))
        row = self.result()["series"][0]
        self.assertEqual(row["state"], "awaiting_update")
        self.assertEqual(row["old_cycle_residual_seconds"], 600)
        self.assertTrue(row["residual_censored"])
        self.assertIsNone(row["completion_to_update_seconds"])

    def test_historical_index_keeps_receipt_and_unknown_upstream_capture(self):
        self.store.add_forecast(forecast())
        self.store.add_forecast(forecast(origin=T+300, generated_at=T+300, available_at=T+360,
                                         metadata=observed(T)))
        initialize(self.store, T+600)
        initialize(self.store, T+1200)
        row = self.result()["series"][0]
        self.assertEqual(row["observer_received_at"], T+360)
        self.assertIsNone(row["source_received_at"])
        self.assertIsNone(row["publication_to_capture_seconds"])
        self.assertEqual(self.store.db.execute("SELECT COUNT(*) FROM reaction_forecasts").fetchone()[0],2)
        self.assertEqual(self.store.get("reset-reaction-1")["initialized_at"],T+600)

    def test_private_sources_and_shadow_variants_do_not_cross_report_boundary(self):
        self.capture(forecast(private=True))
        self.capture(forecast(source="without_community",track="shadow"))
        self.assertEqual(self.result()["series"],[])

    def test_confirmation_can_match_explicit_identity_without_inventing_occurrence_time(self):
        self.capture(forecast(source="horizon",track="model", origin=T+300, generated_at=T+300,
            available_at=T+360, metadata={"cycle":{"occurred_at":None,"confirmation_at":T,
            "time_basis":"completion_confirmation","references":["https://twitter.com/thsottiaux/status/123?ref=source"]}}))
        row=self.result()["series"][0]
        self.assertEqual(row["state"],"updated")
        self.assertIsNone(row["old_cycle_residual_seconds"])

    def test_legacy_reset_clock_cannot_create_a_retrospective_occurrence_measurement(self):
        self.store.add_forecast(forecast(source="monitor", metadata={"last_reset_at":T-86400}))
        self.store.add_forecast(forecast(source="monitor", origin=T+300, generated_at=T+300,
            available_at=T+360, metadata={"last_reset_at":T}))
        initialize(self.store, T+600)
        row = self.result()["series"][0]
        self.assertEqual(row["state"], "anchor_unknown")
        self.assertIsNone(row["completion_to_update_seconds"])
        self.assertIsNone(row["old_cycle_residual_seconds"])
        self.assertIsNone(row["after_probability_24h"])

    def test_unknown_population_does_not_become_a_measured_actual_event(self):
        event = reconcile(self.store,T+600,Policy())["events"][0]
        event.update(status="pending",reason="population_unspecified")
        self.assertEqual(report(self.store,T+600,{"events":[event]},Policy())["events"],[])

    def test_new_instrumentation_does_not_date_response_from_unknown_legacy_cycle(self):
        self.capture(forecast(metadata={"last_reset_at": T-86400}))
        self.capture(forecast(origin=T+300, generated_at=T+300, available_at=T+360,
                              metadata=observed(T)), T+330)
        row = self.result()["series"][0]
        self.assertEqual(row["state"], "updated")
        self.assertEqual(row["publication_to_capture_seconds"], 30)
        self.assertIsNone(row["known_to_update_seconds"])
        self.assertIsNone(row["completion_to_update_seconds"])
        self.assertIsNone(row["old_cycle_residual_seconds"])

    def test_unknown_intermediate_cycle_leaves_response_time_unmeasured(self):
        self.capture(forecast())
        self.capture(forecast(origin=T+100, generated_at=T+100, available_at=T+120,
                              metadata={"last_reset_at": T}))
        self.capture(forecast(origin=T+300, generated_at=T+300, available_at=T+360,
                              metadata=observed(T)))
        row = self.result()["series"][0]
        self.assertEqual(row["state"], "updated")
        self.assertIsNone(row["known_to_update_seconds"])
        self.assertIsNone(row["old_cycle_residual_seconds"])

    def test_html_keeps_missing_clocks_and_censored_residual_visible(self):
        self.capture(forecast())
        self.capture(forecast(origin=T+100, generated_at=T+100, available_at=T+120))
        html = reaction_html({"events":[self.result()]})
        self.assertIn("事件后的更新速度",html)
        self.assertIn("至少 600 秒",html)
        self.assertIn("未记录",html)
        self.assertIn("仍在旧周期",html)
        self.assertEqual(reaction_html(None),"")


if __name__ == "__main__": unittest.main()
