import json
from unittest.mock import patch

from reset_observer.benchmark import revise_trial_eligibility, trial_probability
from reset_observer.contracts import packed
from reset_observer.preferences import settle as settle_preferences
from reset_observer.store import Store
from test_observer import T, TemporaryStore, forecast, trial


class ConcurrentSettlementTests(TemporaryStore):
    def test_calendar_settlement_releases_read_cursor_before_another_writer_commits(self):
        with self.store.db:
            for identifier in ("first", "second"):
                self.store.db.execute("INSERT INTO preference_trials VALUES (?,?,?)",
                    (identifier, T, packed(trial(candidates={}, comparisons=[]))))
        other = Store(self.store.path)
        commits = []
        def coverage(*args):
            other.runtime_event(T + len(commits), {"kind": "concurrent_capture"})
            commits.append(True)
            return {"covered": False}
        try:
            with patch("reset_observer.preferences.coverage_details", side_effect=coverage):
                self.assertEqual(settle_preferences(self.store, T+60, self.policy, {"events": []}), 2)
            self.assertEqual(len(commits), 2)
            self.assertEqual(self.store.db.execute("SELECT COUNT(*) FROM preference_trials").fetchone()[0], 2)
        finally:
            other.close()

    def test_eligibility_revision_does_not_upgrade_an_outdated_wal_snapshot(self):
        f = forecast(origin=T-60, available_at=T-60, generated_at=T-60)
        self.store.add_forecast(f)
        for hours in (6, 12):
            self.store.freeze(trial(hours=hours, p=None, missing_reason="source_missed_known_event",
                forecast_id=f.id, received_at=f.available_at, publication_origin=f.origin))
        evidence = {"events": [{"id": "withdrawn", "status": "pending", "reason": "completion_withdrawn",
            "kind": "automatic", "from": T-300, "to": T-300, "known_at": T+1,
            "references": ["https://x.com/i/status/123"], "claims": []}]}
        other = Store(self.store.path)
        committed = False
        def probability(*args):
            nonlocal committed
            if not committed:
                other.runtime_event(T+1, {"kind": "concurrent_maintenance"})
                committed = True
            return trial_probability(*args)
        try:
            with patch("reset_observer.benchmark.trial_probability", side_effect=probability):
                self.assertEqual(revise_trial_eligibility(self.store, T+60, self.policy, evidence, "new-truth"), 2)
            self.assertTrue(committed)
            self.assertTrue(all(row["p"] is not None for row in self.store.trials(T+60)))
            self.assertTrue(all(json.loads(row[0])["p"] is None for row in self.store.db.execute("SELECT payload FROM trials")))
        finally:
            other.close()
