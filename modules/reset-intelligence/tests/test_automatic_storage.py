import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

from reset_intelligence.automatic_storage import step, capture_rowids, recover_rowids
from reset_intelligence.store import Store
from reset_intelligence.maintenance import rowid_identities


class AutomaticStorageTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = Path(self.tmp.name) / "legacy.sqlite"
        store = Store(self.path)
        self.values = [{"n": n, "evidence": "真实原始记录" * 2000, "p": .923456789} for n in range(40)]
        with store.db:
            for i, payload in enumerate(self.values):
                store.db.execute("INSERT INTO forecasts(rowid,id,issued_at,target,model_version,payload) VALUES (?,?,?,?,?,?)",
                    (3*i+1, str(i), i, "automatic_reset", "original", json.dumps(payload)))
            store.db.execute("DROP TABLE qh_storage_upgrade")
        store.close()
        with sqlite3.connect(self.path) as db:
            db.execute("PRAGMA auto_vacuum=NONE")
            db.execute("VACUUM")
        db.close()
        self.before = self.path.stat().st_size

    def tearDown(self):
        self.tmp.cleanup()

    def test_normal_entry_resumes_compression_and_reclaims_legacy_database(self):
        # Store startup detects the old file; no maintenance CLI or backup.
        store = Store(self.path)
        identity = rowid_identities(store.db)
        self.assertEqual(store.db.execute("SELECT value FROM qh_storage_upgrade WHERE key='phase'").fetchone()[0], "compress")
        store.close()
        phases = []
        for _ in range(100):
            result = step(self.path, batch_rows=3, max_batches=1)
            phases.append(result["phase"])
            self.assertNotEqual(result.get("state"), "deferred", result)
            if result["phase"] == "done":
                break
        self.assertEqual(phases[-1], "done")
        self.assertGreater(phases.count("compress"), 1)
        store = Store(self.path)
        self.assertEqual([json.loads(r[0]) for r in store.db.execute("SELECT payload FROM forecasts ORDER BY rowid")], self.values)
        self.assertEqual(rowid_identities(store.db), identity)
        self.assertEqual(store.db.execute("PRAGMA auto_vacuum").fetchone()[0], 2)
        self.assertLess(self.path.stat().st_size, self.before / 3)
        store.close()

    def test_interrupted_rowid_restore_recovers_original_import_cursors(self):
        store = Store(self.path)
        expected = rowid_identities(store.db)
        capture_rowids(store.db)
        with store.db:
            store.db.execute("UPDATE forecasts SET rowid=rowid+1000")
        store.close()
        # Recovery runs before ordinary writer operations on restart.
        store = Store(self.path)
        self.assertEqual(rowid_identities(store.db), expected)
        self.assertIsNone(store.db.execute("SELECT 1 FROM sqlite_master WHERE name='qh_storage_rowids'").fetchone())
        store.close()

    def test_disk_pressure_defers_final_reclaim_without_losing_rows(self):
        with patch("reset_intelligence.automatic_storage.shutil.disk_usage") as usage:
            usage.return_value.free = 0
            for _ in range(30):
                result = step(self.path)
                if result.get("state") == "waiting_for_disk_space":
                    break
        self.assertEqual(result.get("state"), "waiting_for_disk_space")
        self.assertEqual(step(self.path)["phase"], "done")
        store = Store(self.path)
        self.assertEqual(len(store.forecasts(100, "automatic_reset")), len(self.values))
        store.close()
