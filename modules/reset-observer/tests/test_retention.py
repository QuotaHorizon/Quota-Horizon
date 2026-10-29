import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
import zlib
from unittest.mock import patch

from reset_observer.retention import retain_reports, reclaim_cache_pages, retain_auxiliary_forecasts, DAY, HOUR
from reset_observer.store import Store
from reset_observer.contracts import Forecast, RELIEF, packed


class ReportRetentionTests(unittest.TestCase):
    def test_worker_automatically_upgrades_legacy_observer_without_touching_ledger(self):
        from types import SimpleNamespace
        from reset_observer.runtime import maintain_storage
        original = {"frozen_prediction": .91234567, "evidence": "public source record " * 2000}
        with self.store.db:
            for i in range(12):
                self.store.db.execute("INSERT INTO runtime_events(id,at,payload) VALUES (?,?,?)", (i*3+1, i, json.dumps(original)))
                self.store.db.execute("INSERT INTO reports(at,payload) VALUES (?,?)", (i, zlib.compress(json.dumps({"policy":{}, "n":i, "details":"archive "*10000}).encode())))
            self.store.db.execute("DELETE FROM qh_storage_upgrade")
        self.store.close()
        db = sqlite3.connect(self.path)
        db.execute("PRAGMA auto_vacuum=NONE")
        db.execute("VACUUM")
        db.close()
        for _ in range(40):
            result = maintain_storage(SimpleNamespace(db=self.path, producer_db=None))["observer"]
            self.assertNotEqual(result.get("state"), "deferred", result)
            if result["phase"] == "done":
                break
        self.assertEqual(result["phase"], "done")
        self.store = Store(self.path)
        self.assertEqual(self.store.db.execute("PRAGMA auto_vacuum").fetchone()[0], 2)
        rows = self.store.db.execute("SELECT id,payload FROM runtime_events ORDER BY id").fetchall()
        self.assertEqual([r[0] for r in rows], [i*3+1 for i in range(12)])
        self.assertTrue(all(json.loads(r[1]) == original for r in rows))
        self.assertEqual(self.store.db.execute("SELECT COUNT(*) FROM reports").fetchone()[0], 2)

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.path = Path(self.directory.name) / "observer.sqlite"
        self.store = Store(self.path)

    def tearDown(self):
        self.store.close()
        self.directory.cleanup()

    def test_rollup_preserves_daily_hourly_latest_versions_and_the_entire_ledger(self):
        original = {}
        for day in range(10):
            for hour in (0, 6, 12, 18):
                for minute in (0, 30):
                    at = day * DAY + hour * HOUR + minute * 60
                    value = {"implementation_hash": "new" if day == 9 and hour == 12 else "old",
                             "policy": {"hash": "p"}, "generated_at": at, "scores": [day, hour, minute]}
                    self.store.report(at, value)
                    original[at] = value
        now = 10 * DAY
        with self.store.db:
            self.store.db.execute("INSERT INTO runtime_events(at,payload) VALUES (?,?)", (0, '{"historical":"event"}'))
            self.store.db.execute("INSERT INTO trial_revisions(trial,at,signature,payload) VALUES ('original',0,'original','{}')")
        # Earlier snapshots still exist before the automatic archive policy.
        before = self.store.db.execute("SELECT COUNT(*) FROM reports").fetchone()[0]
        self.assertGreater(retain_reports(self.store, now), 0)
        kept = self.store.db.execute("SELECT at,payload FROM reports ORDER BY at").fetchall()
        self.assertLess(len(kept), before / 2)
        for at, text in kept:
            self.assertEqual(json.loads(text), original[at])
        required = {0, max(original), 9 * DAY + 12 * HOUR, 9 * DAY + 12 * HOUR + 1800}
        required |= {d * DAY + 18 * HOUR + 1800 for d in range(3)}
        required |= {d * DAY + h * HOUR + 1800 for d in range(3, 10) for h in (0, 6, 12, 18)}
        self.assertEqual({r[0] for r in kept}, required)
        self.assertEqual(retain_reports(self.store, now), 0)
        self.assertEqual(self.store.db.execute("SELECT COUNT(*) FROM runtime_events").fetchone()[0], 1)
        self.assertEqual(self.store.db.execute("SELECT payload FROM trial_revisions").fetchone()[0], '{}')
        self.assertEqual(self.store.report_at(now), original[max(original)])
        self.assertLessEqual(self.store.report_at(2 * DAY)["generated_at"], 2 * DAY)

    def test_legacy_shared_and_zlib_reports_keep_revision_anchors(self):
        history = [{"text": "complete earlier evaluation" * 200} for _ in range(20)]
        reports = [{"implementation_hash": revision, "policy": {"hash": "p"}, "history": history, "at": at}
                   for at, revision in enumerate(("a", "a", "b", "b", "c", "c"))]
        with self.store.db:
            for at, report in enumerate(reports):
                value = zlib.compress(json.dumps(report).encode()) if at == 0 else self.store.pack_json(report)
                self.store.db.execute("INSERT INTO reports(at,payload) VALUES (?,?)", (at, value))
        self.assertEqual(retain_reports(self.store, 100), 0)
        for at, value in enumerate(reports):
            self.assertEqual(self.store.report_at(at), value)

    def test_minute_updates_rotate_reports_without_allocating_shared_garbage(self):
        for at in range(120):
            self.store.report(at * 60, {"implementation_hash": "v", "policy": {"hash": "p"},
                                       "time": at, "history": ["large report" * 1000]}, archive=True)
        self.assertEqual(self.store.db.execute("SELECT COUNT(*) FROM reports").fetchone()[0], 3)
        self.assertEqual(self.store.db.execute("SELECT COUNT(*) FROM json_bodies").fetchone()[0], 0)
        self.assertEqual(self.store.report_at(120 * 60)["time"], 119)
        self.assertEqual(self.store.db.execute("PRAGMA auto_vacuum").fetchone()[0], 2)

    def test_page_reclaim_is_bounded_and_preserves_retained_reports(self):
        import random
        rng = random.Random(3)
        with self.store.db:
            self.store.db.execute("CREATE TABLE discardable_cache(payload BLOB)")
            self.store.db.executemany("INSERT INTO discardable_cache VALUES (?)", [(rng.randbytes(100_000),) for _ in range(30)])
        self.store.report(1, {"kept": True})
        with self.store.db:
            self.store.db.execute("DELETE FROM discardable_cache")
        self.assertEqual(reclaim_cache_pages(self.store, minimum_bytes=100_000_000), 0)
        pages = self.store.db.execute("PRAGMA page_count").fetchone()[0]
        saved = reclaim_cache_pages(self.store, minimum_bytes=0, maximum_pages=10)
        self.assertGreater(saved, 0)
        self.assertLessEqual(saved, 10 * 4096)
        self.assertLess(self.store.db.execute("PRAGMA page_count").fetchone()[0], pages)
        self.assertEqual(self.store.report_at(10), {"kept": True})

    def test_raw_cache_keeps_latest_success_errors_and_recent_duplicate_hashes(self):
        from reset_observer.store import maintain_raw_cache
        now = 60 * DAY
        values = [(DAY, 'offline', {"valid": "old"}, []),
                  (DAY+10, 'offline', {"bad": "response"}, None),
                  (DAY, 'active', {"repeated": True}, []),
                  (DAY+1, 'active', {"temporary": True}, []),
                  (now, 'active', {"repeated": True}, [])]
        for at, source, raw, claims in values:
            self.store.observation(source, at, at, False, claims, raw)
        hashes = [r[0] for r in self.store.db.execute("SELECT payload_hash FROM observations ORDER BY id")]
        self.assertEqual(maintain_raw_cache(self.store.db, now), 1)
        for index in (0, 1, 2, 4):
            self.assertEqual(self.store.raw_payload(hashes[index]), values[index][2])
        self.assertIsNone(self.store.raw_payload(hashes[3]))
        self.assertEqual(maintain_raw_cache(self.store.db, now+60), 0)
        self.store.observation('active', now+61, now+61, False, [], values[3][2])
        self.assertEqual(self.store.raw_payload(hashes[3]), values[3][2])

    def auxiliary(self, at, track='shadow', expires=None):
        value = Forecast('comparison', 'v1', RELIEF, track, at, at, expires or at+HOUR,
                         [[24, .9], [48, .95]], at, metadata={'audit': 'original information' * 200})
        self.store.add_forecast(value)
        return value

    def pin(self, forecast, name):
        with self.store.db:
            self.store.db.execute("""INSERT INTO trials VALUES (?,?,?,?,?,?,?,?,?,?,?)""",
                (name, forecast.source, forecast.version, forecast.target, forecast.track,
                 forecast.origin, 24, None, forecast.origin, 'policy',
                 self.store.pack_json({'forecast_id':forecast.id,'missing_reason':'event_since_forecast'})))

    def test_only_expired_unfrozen_auxiliary_computations_retire_counts_and_evidence_survive(self):
        now = 10 * DAY
        records = [self.auxiliary(at) for at in range(10)]
        native = [self.auxiliary(at, 'native') for at in range(3)]
        models = [self.auxiliary(at, 'model') for at in range(3)]
        community = [self.auxiliary(at, 'community') for at in range(3)]
        recent = self.auxiliary(now-HOUR)
        active = self.auxiliary(10, expires=now+DAY)
        self.pin(records[3], 'excluded-formal-trial')
        with self.store.db:
            self.store.db.execute("INSERT INTO preference_trials VALUES ('community',1,?)",
                                 (packed({'comparisons':[{'forecast_id':records[4].id}]}),))
        self.store.put('last_grid', now)
        counts = {at:self.store.forecast_count(at) for at in (4, 100, now)}
        latest = self.store.forecasts(now, latest=True)
        original_trial = self.store.db.execute('SELECT payload FROM trials').fetchone()[0]
        self.assertEqual(retain_auxiliary_forecasts(self.store, now, maximum_rows=100), 7)
        remaining = {f.id for f in self.store.forecasts(now)}
        self.assertEqual(remaining, {f.id for f in [records[0],records[3],records[4],recent,active,*native,*models,*community]})
        self.assertEqual(self.store.forecasts(now, latest=True), latest)
        self.assertEqual({at:self.store.forecast_count(at) for at in counts}, counts)
        self.assertEqual(self.store.db.execute('SELECT payload FROM trials').fetchone()[0], original_trial)
        self.assertEqual(retain_auxiliary_forecasts(self.store, now), 0)
        self.store.add_forecast(records[1])
        self.assertEqual(self.store.forecast_count(now), counts[now])
        self.assertNotIn(records[1].id, {f.id for f in self.store.forecasts(now)})

    def test_unfinished_freezing_and_a_concurrent_new_trial_prevent_retirement(self):
        import reset_observer.retention as retention
        records = [self.auxiliary(at) for at in range(5)]
        now = 10 * DAY
        self.assertEqual(retain_auxiliary_forecasts(self.store, now), 0)
        self.store.put('last_grid', DAY)
        self.assertEqual(retain_auxiliary_forecasts(self.store, now), 0)
        self.store.put('last_grid', now)
        pin = retention._pin_trial_forecasts
        def race(store):
            pin(store)
            self.pin(records[2], 'new-trial')
        with patch.object(retention, '_pin_trial_forecasts', side_effect=race):
            self.assertEqual(retain_auxiliary_forecasts(self.store, now), 0)
        self.assertEqual(retain_auxiliary_forecasts(self.store, now), 2)
        self.assertIn(records[2].id, {f.id for f in self.store.forecasts(now)})

    def test_auxiliary_frames_do_not_allocate_shared_bodies_that_expiry_cannot_reclaim(self):
        self.auxiliary(1)
        self.auxiliary(2, 'adapted')
        self.assertEqual(self.store.db.execute('SELECT COUNT(*) FROM json_bodies').fetchone()[0], 0)


if __name__ == "__main__":
    unittest.main()
