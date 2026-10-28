import json
from pathlib import Path
import sqlite3
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
import zlib

from reset_intelligence.maintenance import compact, begin_batch, vacuum, verify, collect_unused_bodies, LEGACY_SOURCE
from reset_intelligence.storage_codec import configure_storage, JSONStorage
from reset_intelligence.store import Store


class StorageMaintenanceTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.path = Path(self.directory.name) / "producer.sqlite"
        self.values = [{"time": i, "facts": [{"n": n, "text": "公共证据" * 120} for n in range(18)],
                        "precise": 0.123456789, "large_integer": 9007199254740993,
                        "nullable": None, "boolean": False} for i in range(12)]
        store = Store(self.path)
        with store.db:
            for i, value in enumerate(self.values):
                text = json.dumps(value, ensure_ascii=False)
                body = text if i % 3 == 0 else text.encode() if i % 3 == 1 else LEGACY_SOURCE + zlib.compress(text.encode())
                store.db.execute("INSERT INTO payloads VALUES (?,?)", (str(i), body))
                store.db.execute("INSERT INTO observations(source,observed_at,generated_at,payload_hash,normalized,error) VALUES (?,?,?,?,?,?)",
                                 ("source", i, i - 1, str(i), text, None))
                store.db.execute("INSERT INTO forecasts VALUES (?,?,?,?,?)", (str(i), i, "target", "frozen-v1", text))
            store.db.execute("INSERT INTO observations(source,observed_at,normalized) VALUES ('null',99,NULL)")
        store.close()

    def tearDown(self):
        self.directory.cleanup()

    def assert_original_values(self):
        store = Store(self.path, read_only=True)
        try:
            for i, value in enumerate(self.values):
                self.assertEqual(store.raw_payload(str(i)), value)
                row = store.db.execute("SELECT * FROM forecasts WHERE id=?", (str(i),)).fetchone()
                self.assertEqual(json.loads(row["payload"]), value)
                self.assertEqual((row["issued_at"], row["target"], row["model_version"]), (i, "target", "frozen-v1"))
            self.assertIsNone(store.db.execute("SELECT normalized FROM observations WHERE source='null'").fetchone()[0])
        finally:
            store.close()

    def test_interrupted_batches_resume_and_semantics_survive_vacuum(self):
        before = self.path.stat().st_size
        emitted = []
        def interrupt(event):
            emitted.append(event)
            raise KeyboardInterrupt()
        with self.assertRaises(KeyboardInterrupt):
            compact(self.path, batch_rows=3, emit=interrupt)
        self.assertEqual(emitted[0]["verified_rows"], 3)
        result = compact(self.path, batch_rows=5)
        self.assertEqual(result["status"], "complete")
        self.assertEqual(sum(t["verified_rows"] for t in result["tables"]), 37)
        self.assert_original_values()
        self.assertTrue(verify(self.path)["verified"])
        # Re-running does not allocate another pass or reencode immutable bodies.
        repeated = compact(self.path)
        self.assertEqual(repeated, result)
        with self.assertRaisesRegex(ValueError, "stopped"):
            vacuum(self.path)
        report = vacuum(self.path, writers_stopped=True)
        self.assertEqual(report["event"], "vacuum_complete")
        self.assertLess(self.path.stat().st_size, before * .6)
        self.assert_original_values()
        self.assertTrue(verify(self.path)["verified"])

    def test_failed_row_verification_rolls_back_row_bodies_and_checkpoint(self):
        with sqlite3.connect(self.path) as db:
            db.execute("CREATE TRIGGER corrupt_clock AFTER UPDATE OF normalized ON observations BEGIN UPDATE observations SET observed_at=999 WHERE id=NEW.id; END")
            original = db.execute("SELECT normalized,observed_at FROM observations WHERE id=1").fetchone()
        with self.assertRaisesRegex(ValueError, "semantic verification failed"):
            compact(self.path, batch_rows=1)
        with sqlite3.connect(self.path) as db:
            self.assertEqual(db.execute("SELECT normalized,observed_at FROM observations WHERE id=1").fetchone(), original)
            self.assertEqual(db.execute("SELECT verified_rows FROM qh_storage_progress WHERE table_name='observations'").fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT COUNT(*) FROM json_bodies").fetchone()[0], 0)
            db.execute("DROP TRIGGER corrupt_clock")
        self.assertEqual(compact(self.path)["status"], "complete")
        self.assert_original_values()

    def test_corrupt_json_fails_without_skipping_evidence_and_can_resume_after_repair(self):
        with sqlite3.connect(self.path) as db:
            original = db.execute("SELECT body FROM payloads WHERE hash='0'").fetchone()[0]
            db.execute("UPDATE payloads SET body='invalid json' WHERE hash='0'")
        with self.assertRaises(json.JSONDecodeError):
            compact(self.path)
        with sqlite3.connect(self.path) as db:
            self.assertEqual(db.execute("SELECT verified_rows FROM qh_storage_progress WHERE table_name='payloads'").fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT COUNT(*) FROM payloads").fetchone()[0], 12)
            db.execute("UPDATE payloads SET body=? WHERE hash='0'", (original,))
        self.assertEqual(compact(self.path)["status"], "complete")

    def test_concurrent_appends_are_preserved_and_explicitly_caught_up(self):
        appended = False
        def append(event):
            nonlocal appended
            if not appended:
                appended = True
                with sqlite3.connect(self.path) as db:
                    db.execute("INSERT INTO forecasts VALUES ('appended',100,'target','new',?)", (json.dumps(self.values[0]),))
        compact(self.path, batch_rows=4, emit=append)
        with self.assertRaisesRegex(ValueError, "appended"):
            vacuum(self.path, writers_stopped=True)
        caught = compact(self.path, catch_up=True)
        self.assertEqual(next(t for t in caught["tables"] if t["table_name"] == "forecasts")["verified_rows"], 13)
        vacuum(self.path, writers_stopped=True)
        self.assertTrue(verify(self.path)["verified"])
        with sqlite3.connect(self.path) as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM forecasts WHERE id='appended'").fetchone()[0], 1)

    def test_full_verification_detects_changed_metadata_without_repairing_it(self):
        compact(self.path)
        with sqlite3.connect(self.path) as db:
            db.execute("UPDATE forecasts SET model_version='tampered' WHERE id='0'")
        with self.assertRaisesRegex(ValueError, "baseline mismatch"):
            verify(self.path)
        with sqlite3.connect(self.path) as db:
            self.assertEqual(db.execute("SELECT model_version FROM forecasts WHERE id='0'").fetchone()[0], "tampered")

    def test_space_guard_leaves_database_usable(self):
        compact(self.path)
        with patch("reset_intelligence.maintenance.shutil.disk_usage") as disk:
            disk.return_value.free = 0
            with self.assertRaisesRegex(ValueError, "insufficient temporary"):
                vacuum(self.path, writers_stopped=True)
        self.assert_original_values()

    def test_vacuum_interruption_does_not_resume_with_unverified_rowids(self):
        compact(self.path)
        def stop(event):
            if event["event"] == "vacuum_sql_complete":
                raise KeyboardInterrupt()
        with self.assertRaises(KeyboardInterrupt):
            vacuum(self.path, writers_stopped=True, emit=stop)
        with self.assertRaisesRegex(ValueError, "interrupted VACUUM"):
            compact(self.path, catch_up=True)
        self.assertTrue(verify(self.path)["verified"])
        self.assertEqual(vacuum(self.path, writers_stopped=True)["event"], "vacuum_complete")
        self.assertEqual(vacuum(self.path, writers_stopped=True)["event"], "already_vacuumed")
        self.assert_original_values()

    def test_vacuum_detects_changed_importer_rowid_mapping(self):
        compact(self.path)
        def alter(event):
            if event["event"] == "vacuum_sql_complete":
                with sqlite3.connect(self.path) as other:
                    other.execute("UPDATE forecasts SET rowid=500 WHERE id='0'")
        with self.assertRaisesRegex(ValueError, "rowid ordering"):
            vacuum(self.path, writers_stopped=True, emit=alter)
        with sqlite3.connect(self.path) as db:
            self.assertEqual(db.execute("SELECT status FROM qh_storage_jobs").fetchone()[0], "vacuum_started")

    def test_report_zlib_and_text_are_deduplicated_without_losing_any_revision(self):
        path = Path(self.directory.name) / "observer.sqlite"
        with sqlite3.connect(path) as db:
            db.executescript("""CREATE TABLE observations(id INTEGER PRIMARY KEY,provider TEXT,claims TEXT,healthy INTEGER);
                CREATE TABLE trials(id TEXT PRIMARY KEY,payload TEXT);
                CREATE TABLE reports(id INTEGER PRIMARY KEY,at REAL,payload BLOB);
                CREATE TABLE payloads(hash TEXT PRIMARY KEY,body BLOB);
                CREATE TABLE forecasts(id TEXT PRIMARY KEY,payload TEXT);""")
            for at, value in enumerate(self.values):
                db.execute("INSERT INTO reports(at,payload) VALUES (?,?)", (at, zlib.compress(json.dumps(value).encode())))
            # Legacy source evidence remains JSON-readable after inline compression.
            raw = zlib.compress(json.dumps(self.values[0]).encode())
            db.execute("INSERT INTO payloads VALUES ('raw',?)", (raw,))
        result = compact(path, batch_rows=2)
        self.assertEqual(result["status"], "complete")
        db = sqlite3.connect(path)
        configure_storage(db, read_only=True)
        self.assertEqual(json.loads(db.execute("SELECT body FROM payloads").fetchone()[0]), self.values[0])
        self.assertEqual([json.loads(r[0]) for r in db.execute("SELECT payload FROM reports ORDER BY at")], self.values)
        db.close()
        self.assertTrue(verify(path)["verified"])

    def test_shared_body_sweep_preserves_nested_roots_and_rolls_back_on_missing_body(self):
        compact(self.path)
        db = sqlite3.connect(self.path)
        codec = JSONStorage(db)
        with db:
            codec.pack_json({"orphan": "unused disposable report" * 200})
        db.close()
        with self.assertRaisesRegex(ValueError, "stopped"):
            collect_unused_bodies(self.path)
        result = collect_unused_bodies(self.path, writers_stopped=True)
        self.assertGreater(result["removed"], 0)
        self.assert_original_values()
        self.assertTrue(verify(self.path)["verified"])
        with sqlite3.connect(self.path) as db:
            before = db.execute("SELECT COUNT(*) FROM json_bodies").fetchone()[0]
            db.execute("UPDATE forecasts SET payload=? WHERE id='0'", (b'\0QHJSON2\0' + b'0' * 64,))
        with self.assertRaisesRegex(ValueError, "missing"):
            collect_unused_bodies(self.path, writers_stopped=True)
        with sqlite3.connect(self.path) as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM json_bodies").fetchone()[0], before)

    def test_unknown_database_and_missing_path_do_not_create_evidence(self):
        missing = Path(self.directory.name) / "missing.sqlite"
        with self.assertRaisesRegex(ValueError, "existing"):
            compact(missing)
        self.assertFalse(missing.exists())
        unknown = Path(self.directory.name) / "unknown.sqlite"
        with sqlite3.connect(unknown) as db:
            db.execute("CREATE TABLE unrelated(value TEXT)")
        with self.assertRaisesRegex(ValueError, "recognized"):
            compact(unknown)
        with sqlite3.connect(unknown) as db:
            self.assertEqual(db.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall(), [("unrelated",)])

    def test_short_live_writer_lock_is_retried_without_advancing_checkpoint(self):
        ready = threading.Event()
        def writer():
            with sqlite3.connect(self.path) as db:
                db.execute("BEGIN IMMEDIATE")
                ready.set()
                time.sleep(.12)
        thread = threading.Thread(target=writer)
        thread.start()
        ready.wait(2)
        db = sqlite3.connect(self.path, timeout=.01)
        events = []
        try:
            begin_batch(db, events.append, maximum_wait=2)
            self.assertTrue(db.in_transaction)
            self.assertEqual(events[0]["event"], "waiting_for_writer")
            db.rollback()
        finally:
            db.close()
            thread.join()


if __name__ == "__main__":
    unittest.main()
