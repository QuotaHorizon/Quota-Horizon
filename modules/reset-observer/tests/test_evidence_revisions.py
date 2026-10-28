"""Completion withdrawal uses source revisions, never text probability scoring."""
from dataclasses import replace
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from reset_observer.benchmark import freeze_grid, initialize, label_window, settle_all, trial_probability
from reset_observer.contracts import AUTOMATIC, RELIEF, Forecast, Policy, iso
from reset_observer.evidence import parse, reconcile, reinterpret_latest
from reset_observer.reaction import record_forecast, report as reaction_report
from reset_observer.reporting import build_report
from reset_observer.store import Store


# Public 2026-10-28 case: the status inventory associated "has landed" with a
# reset promise, then restored the promise to pending and removed that association.
ANNOUNCED = 1793155373.
COMPLETED = 1793161102.
FIRST = 1793161198.702778
REMOVED = 1793161803.6644669
ORIGIN = 1793163600.  # 05:00 UTC
PROMISE = "2115273064481218829"
POST = "2115297093879472332"


def inventory(at=FIRST, completed=True):
    schedule = {"kind": "reset_scheduled", "resetType": "global", "announcedAt": iso(ANNOUNCED),
        "effectiveAt": "2026-10-28T08:00:00.000Z", "scope": {"plans": ["all"]},
        "source": {"handle": "thsottiaux", "postId": PROMISE, "url": f"https://x.com/thsottiaux/status/{PROMISE}"},
        "text": "Day 23/ Just kidding, today is a reset."}
    done = {"kind": "reset_completed", "resetType": "global", "announcedAt": iso(COMPLETED),
        "effectiveAt": None, "scope": {"plans": ["unknown"]},
        "source": {"handle": "thsottiaux", "postId": POST, "url": f"https://x.com/thsottiaux/status/{POST}"},
        "text": "https://t.co/Fgy0dBAHsJ has landed"}
    item = done if completed else schedule
    return {"schemaVersion": 1, "generatedAt": iso(at-30), "lastSuccessfulCheckAt": iso(at-30),
        "monitor": {"status": "ok"}, "events": [item],
        "heatmap": {"timezone": "UTC", "total": 1, "weeks": 53, "days": [{"date": "2026-10-28", "count": 1,
            "sources": [{**{k:v for k,v in item.items() if k != "source"},
                         "postId": item["source"]["postId"], "url": item["source"]["url"]}]}]},
        "resetTimeline": {"fulfilledSchedules": [{"completionPostId": POST, "completedAt": iso(COMPLETED), "schedule": schedule}] if completed else [],
            "manualCompletions": [], "suppressedPostIds": [], "nextSchedule": None if completed else schedule,
            "recentNonCompletedPostId": None if completed else PROMISE}}


class EvidenceRevisionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.store = Store(Path(self.directory.name)/"observer.sqlite")
        self.policy = replace(Policy(), reporting_grace_seconds=0)

    def tearDown(self):
        self.store.close()
        self.directory.cleanup()

    def capture(self, raw, at, provider="didcodex_status"):
        generated, healthy, claims = parse(provider, raw, at)
        self.store.observation(provider, at, generated, healthy, claims, raw)
        return reconcile(self.store, at, self.policy)

    def forecast(self, source="horizon", target=RELIEF, origin=ORIGIN-60):
        return Forecast(source=source, version="fixed-before-revision", target=target,
            track="model" if source == "horizon" else "native", origin=origin,
            available_at=origin+1, generated_at=origin, expires_at=origin+72*3600,
            points=[[6,.4],[12,.6],[24,.8],[48,.95],[72,.98]],
            metadata={"last_reset_at": ANNOUNCED-86400,
                      "cycle": {"time_basis": "observed_reset", "occurred_at": ANNOUNCED-86400}})

    def test_retained_day_withdrawal_preserves_history_and_uncertainty(self):
        before = self.capture(inventory(), FIRST)
        original = self.store.db.execute("SELECT payload FROM claims WHERE record=?", (POST,)).fetchone()[0]
        after = self.capture(inventory(REMOVED, False), REMOVED)
        event = next(e for e in after["events"] if f"https://x.com/i/status/{POST}" in e["references"])
        self.assertEqual((event["status"], event["reason"], event["completion_reports"]), ("pending", "completion_withdrawn", 0))
        self.assertEqual((event["from"], event["to"]), (COMPLETED, COMPLETED))
        self.assertEqual(reconcile(self.store, FIRST, self.policy), before)
        self.assertEqual(self.store.db.execute("SELECT payload FROM claims WHERE record=? ORDER BY id LIMIT 1", (POST,)).fetchone()[0], original)
        self.assertEqual(self.store.db.execute("SELECT count(*) FROM claims WHERE record=?", (POST,)).fetchone()[0], 2)
        t = {"origin": COMPLETED-3600, "hours": 6, "target": RELIEF, "p": .8}
        settled = label_window(t, after["events"], COMPLETED+86400, self.policy, covered=True)
        self.assertEqual((settled["status"], settled["label"]), ("unresolved", None))
        f = self.forecast()
        self.assertEqual(trial_probability(f, ORIGIN, 24, before["events"], self.policy)[1], "source_missed_known_event")
        self.assertIsNone(trial_probability(f, ORIGIN, 24, after["events"], self.policy)[1])
        self.store.add_forecast(f)
        record_forecast(self.store, f, source_received_at=f.available_at)
        self.assertEqual(reaction_report(self.store, ORIGIN, after, self.policy)["events"], [])

    def test_failed_stale_truncated_and_out_of_scope_inventory_cannot_withdraw(self):
        self.capture(inventory(), FIRST)
        for case in ("failed", "stale", "truncated", "missing_clock", "other_day", "rolling_feed", "future", "regressed"):
            raw = inventory(REMOVED, False)
            provider = "didcodex_status"
            if case == "failed": raw["monitor"]["status"] = "error"
            if case == "stale": raw["lastSuccessfulCheckAt"] = iso(REMOVED-7200)
            if case == "truncated": raw["heatmap"]["total"] = 99
            if case == "missing_clock": raw.pop("generatedAt")
            if case == "other_day": raw["heatmap"]["days"][0]["date"] = "2026-10-29"
            if case == "rolling_feed": provider = "completion_page"
            if case == "future": raw["generatedAt"] = iso(REMOVED+3600)
            if case == "regressed": raw["generatedAt"] = iso(FIRST-60)
            with self.subTest(case=case):
                # Deliberately exercise the storage boundary, including an
                # upstream flag claiming health for a malformed inventory.
                self.store.observation(provider, REMOVED, REMOVED-30, case != "failed", [], raw)
                self.assertEqual(reconcile(self.store, REMOVED, self.policy)["events"][0]["status"], "confirmed")

    def test_retained_post_reclassified_as_non_reset_withdraws_completion(self):
        self.capture(inventory(), FIRST)
        raw = inventory(REMOVED, True)
        raw["events"][0]["kind"] = "feature"
        raw["heatmap"]["days"][0]["sources"][0]["kind"] = "feature"
        raw["resetTimeline"]["fulfilledSchedules"] = []
        # The post still exists in the full inventory; only its structured
        # classification changed. No interpretation of "has landed" is needed.
        self.assertFalse(parse("didcodex_status", raw, REMOVED)[1])
        # An empty reset inventory cannot establish a negative outcome, but its
        # explicit reclassification still withdraws its own previous claim.
        result = self.capture(raw, REMOVED)
        event = next(e for e in result["events"] if POST in str(e["references"]))
        self.assertEqual((event["status"], event["reason"], event["completion_reports"]), ("pending", "completion_withdrawn", 0))

    def test_explicit_suppression_does_not_require_a_retained_day(self):
        self.capture(inventory(), FIRST)
        raw = inventory(REMOVED, False)
        raw["heatmap"].update(days=[], total=0)
        raw["resetTimeline"]["suppressedPostIds"] = [POST]
        event = self.capture(raw, REMOVED)["events"][0]
        self.assertEqual(event["reason"], "completion_withdrawn")

    def test_reinstatement_and_independent_confirmation_are_not_poisoned(self):
        before = self.capture(inventory(), FIRST)
        self.capture(inventory(REMOVED, False), REMOVED)
        restored = self.capture(inventory(REMOVED+100, True), REMOVED+100)
        self.assertEqual(restored["events"][0]["status"], "confirmed")
        self.assertEqual(self.store.db.execute("SELECT count(*) FROM claims WHERE record=?", (POST,)).fetchone()[0], 3)
        independent = {**before["events"][0]["claims"][0], "provider": "completion_page", "record": "independent", "observed_at": REMOVED+200}
        self.store.record_claims([independent], REMOVED+200)
        result = self.capture(inventory(REMOVED+300, False), REMOVED+300)
        self.assertEqual((result["events"][0]["status"], result["events"][0]["completion_reports"]), ("confirmed", 1))

    def test_later_completion_of_same_promise_keeps_withdrawn_time_out_of_event_range(self):
        self.capture(inventory(), FIRST)
        self.capture(inventory(REMOVED, False), REMOVED)
        # Later observations of the same real promise reference a different
        # completion post. The withdrawn post remains part of the audit trail.
        later_post, completed_at, received_at = "2115330127886463244", 1793168978., 1793169081.688229
        raw = inventory(received_at, True)
        raw["events"][0].update(announcedAt=iso(completed_at), text="")
        raw["events"][0]["source"].update(postId=later_post, url=f"https://x.com/thsottiaux/status/{later_post}")
        raw["heatmap"]["days"][0]["sources"][0].update(
            postId=later_post, url=f"https://x.com/thsottiaux/status/{later_post}", announcedAt=iso(completed_at), text="")
        raw["resetTimeline"]["fulfilledSchedules"][0].update(
            completionPostId=later_post, completedAt=iso(completed_at))
        current = self.capture(raw, received_at)["events"][0]
        self.assertEqual((current["status"], current["from"], current["to"], current["completion_reports"]),
                         ("confirmed", completed_at, completed_at, 1))
        self.assertEqual(next(c["stage"] for c in current["claims"] if c["record"] == POST), "withdrawn")
        self.assertIn(f"https://x.com/i/status/{POST}", current["references"])
        earlier = reconcile(self.store, received_at-1, self.policy)["events"][0]
        self.assertEqual((earlier["reason"], earlier["from"], earlier["completion_reports"]),
                         ("completion_withdrawn", COMPLETED, 0))
        completion = next(c for c in current["claims"] if c["stage"] == "completed")
        self.store.record_claims([{**completion, "provider": "completion_page",
            "record": f"{PROMISE}:{later_post}", "observed_at": received_at+1}], received_at+1)
        corroborated = reconcile(self.store, received_at+1, self.policy)["events"][0]
        self.assertEqual((corroborated["status"], corroborated["from"], corroborated["to"], corroborated["completion_reports"]),
                         ("confirmed", completed_at, completed_at, 2))

    def test_reclassification_stays_withdrawn_without_repeated_revisions(self):
        self.capture(inventory(), FIRST)
        raw = inventory(REMOVED, False)
        raw["events"][0]["source"].update(postId=POST, url=f"https://x.com/thsottiaux/status/{POST}")
        result = self.capture(raw, REMOVED)
        self.assertEqual(result["events"][0]["reason"], "completion_withdrawn")
        count = self.store.db.execute("SELECT count(*) FROM claims").fetchone()[0]
        self.capture(raw, REMOVED+60)
        self.assertEqual(self.store.db.execute("SELECT count(*) FROM claims").fetchone()[0], count)
        self.assertEqual(reconcile(self.store, REMOVED+60, self.policy)["events"][0]["reason"], "completion_withdrawn")

    def test_upgrade_revises_existing_claim_at_new_knowledge_time(self):
        self.capture(inventory(), FIRST)
        with patch("reset_observer.evidence.inventory_revisions", return_value=[]):
            self.capture(inventory(REMOVED, False), REMOVED)
        self.assertEqual(reconcile(self.store, REMOVED, self.policy)["events"][0]["status"], "confirmed")
        reinterpret_latest(self.store, REMOVED+10)
        self.assertEqual(reconcile(self.store, REMOVED+10, self.policy)["events"][0]["reason"], "completion_withdrawn")
        self.assertEqual(reconcile(self.store, REMOVED, self.policy)["events"][0]["status"], "confirmed")
        count = self.store.db.execute("SELECT count(*) FROM claims").fetchone()[0]
        reinterpret_latest(self.store, REMOVED+20)
        self.assertEqual(self.store.db.execute("SELECT count(*) FROM claims").fetchone()[0], count)

    def test_original_trials_get_append_only_eligibility_corrections_for_every_source(self):
        self.capture(inventory(), FIRST)
        for source in ("horizon", "codex_reset", "monitor"):
            for target in (AUTOMATIC, RELIEF):
                self.store.add_forecast(self.forecast(source, target))
        initialize(self.store, ORIGIN-3600, self.policy)
        freeze_grid(self.store, ORIGIN+10, self.policy)
        before = list(self.store.db.execute("SELECT id,payload FROM trials ORDER BY id"))
        self.assertEqual(len(before), 24)
        self.assertTrue(all(t["p"] is None for t in self.store.trials(ORIGIN+10)))
        at = ORIGIN+600
        current = self.capture(inventory(at, False), at)
        settle_all(self.store, at, self.policy, current)
        trials = self.store.trials(at)
        self.assertTrue(all(t["p"] is not None and t["original_probability"] is None for t in trials))
        self.assertTrue(all(t["eligibility_revision"]["at"] == at for t in trials))
        self.assertTrue(all(t["p"] is None for t in self.store.trials(at-1)))
        self.assertEqual([tuple(r) for r in self.store.db.execute("SELECT id,payload FROM trials ORDER BY id")], [tuple(r) for r in before])
        self.assertEqual(self.store.db.execute("SELECT count(*) FROM trial_revisions").fetchone()[0], 24)
        compact = self.store.trials(at, compact=True)
        self.assertEqual([(t["id"],t["p"]) for t in compact], [(t["id"],t["p"]) for t in trials])
        settle_all(self.store, at+1, self.policy, current)
        self.assertEqual(self.store.db.execute("SELECT count(*) FROM trial_revisions").fetchone()[0], 24)
        restored = self.capture(inventory(at+60, True), at+60)
        settle_all(self.store, at+60, self.policy, restored)
        self.assertTrue(all(t["p"] is None for t in self.store.trials(at+60)))
        self.assertTrue(all(t["p"] is not None for t in self.store.trials(at)))
        self.assertEqual(self.store.db.execute("SELECT count(*) FROM trial_revisions").fetchone()[0], 48)

    def test_removed_scope_association_restores_eligibility_without_denying_completion(self):
        before = self.capture(inventory(), FIRST)
        announcement = {**before["events"][0]["claims"][0], "record": "unrelated",
            "keys": ["https://x.com/i/status/999"], "references": ["https://x.com/i/status/999"],
            "scope_references": [], "stage": "announced", "completed_at": None, "observed_at": FIRST}
        self.store.record_claims([announcement], FIRST)
        self.store.add_forecast(self.forecast())
        initialize(self.store, ORIGIN-3600, self.policy)
        freeze_grid(self.store, ORIGIN+10, self.policy)
        settle_all(self.store, ORIGIN+10, self.policy, before)
        at = ORIGIN+600
        raw = inventory(at, True)
        raw["resetTimeline"]["fulfilledSchedules"] = []
        after = self.capture(raw, at)
        event = next(e for e in after["events"] if POST in str(e["references"]))
        self.assertEqual((event["status"], event["reason"], event["completion_reports"]), ("pending", "population_unspecified", 1))
        settle_all(self.store, at, self.policy, after)
        self.assertTrue(all(t["p"] is not None for t in self.store.trials(at)))
        self.assertTrue(all(t["p"] is None for t in self.store.trials(at-1)))
        # A newly discovered unrelated event is not backdated into the trial's
        # original information set merely because its alleged time is earlier.
        new = {**announcement, "stage": "completed", "completed_at": COMPLETED, "observed_at": at+60}
        self.store.record_claims([new], at+60)
        settle_all(self.store, at+60, self.policy, reconcile(self.store, at+60, self.policy))
        self.assertTrue(all(t["p"] is not None for t in self.store.trials(at+60)))

    def test_restored_eligibility_keeps_original_source_support_limits(self):
        self.capture(inventory(), FIRST)
        f = self.forecast()
        f.points = [[6,.4],[12,.6],[24,.8]]
        self.store.add_forecast(f)
        initialize(self.store, ORIGIN-3600, self.policy)
        freeze_grid(self.store, ORIGIN+10, self.policy)
        at = ORIGIN+600
        evidence = self.capture(inventory(at, False), at)
        settle_all(self.store, at, self.policy, evidence)
        current = {t["hours"]:t for t in self.store.trials(at)}
        self.assertIsNotNone(current[6]["p"])
        self.assertIsNotNone(current[12]["p"])
        for hours in (24,48):
            self.assertIsNone(current[hours]["p"])
            self.assertEqual(current[hours]["missing_reason"], "outside_original_support")

    def test_settlement_revision_never_turns_removed_completion_into_a_negative(self):
        before = self.capture(inventory(), FIRST)
        origin = COMPLETED-3600
        f = self.forecast(origin=origin)
        self.store.add_forecast(f)
        initialize(self.store, origin, self.policy)
        trial = {"source": f.source, "version": f.version, "target": f.target, "track": f.track,
            "origin": origin, "hours": 6, "p": .8, "frozen_at": origin+1,
            "policy_hash": self.policy.hash, "forecast_id": f.id, "private": False,
            "missing_reason": None, "received_at": f.available_at, "publication_origin": f.origin, "headline": True}
        identifier = self.store.freeze(trial)
        mature = origin+6*3600
        with patch("reset_observer.benchmark.coverage", return_value=True):
            settle_all(self.store, mature, self.policy, before)
            raw = inventory(mature+60, False)
            after = self.capture(raw, mature+60)
            settle_all(self.store, mature+60, self.policy, after)
        self.assertEqual(self.store.settled(mature)[identifier]["label"], 1)
        current = self.store.settled(mature+60)[identifier]
        self.assertEqual((current["status"], current["label"], current["score_eligible"]), ("unresolved", None, False))
        self.assertEqual(self.store.db.execute("SELECT count(*) FROM settlements WHERE trial=?", (identifier,)).fetchone()[0], 2)
        report = build_report(self.store, mature+60, self.policy, after)
        outcome = next(e for e in report["outcomes"] if POST in str(e["references"]))
        self.assertEqual((outcome["status"], outcome["reason"], outcome["completion_reports"]), ("pending", "completion_withdrawn", 0))
        self.assertEqual(next(s for s in report["scorecards"] if s["source"] == "horizon" and s["hours"] == 6)["scores"]["n"], 0)


if __name__ == "__main__":
    unittest.main()
