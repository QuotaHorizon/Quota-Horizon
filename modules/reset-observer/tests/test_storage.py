from contextlib import closing
from dataclasses import replace
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
import zlib

from reset_observer.contracts import AUTOMATIC, Forecast
from reset_observer.store import Store


class ObserverStorageTests(unittest.TestCase):
    def test_shared_reports_preserve_legacy_as_of_reads_and_every_revision(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "observer.sqlite"
            store = Store(path)
            detail = [{"id": str(i), "text": "完整历史报告与来源依据" * 150} for i in range(20)]
            values = [{"at": at, "history": detail, "unknown": None} for at in range(12)]
            with store.db:
                store.db.execute("INSERT INTO reports(at,payload) VALUES (?,?)",
                                 (0, zlib.compress(json.dumps(values[0]).encode())))
            for at, value in enumerate(values[1:], 1):
                store.report(at, value)
            store.close()
            store = Store(path, read_only=True)
            for at, value in enumerate(values):
                self.assertEqual(store.report_at(at), value)
            self.assertIsNone(store.report_at(-1))
            self.assertEqual(store.db.execute("SELECT COUNT(*) FROM reports").fetchone()[0], len(values))
            with closing(sqlite3.connect(path)) as raw, raw:
                self.assertLess(raw.execute("SELECT MAX(length(payload)) FROM reports WHERE at>0").fetchone()[0], 2048)
                self.assertEqual(raw.execute("SELECT COUNT(*) FROM json_bodies").fetchone()[0], 0)
            store.close()

    def test_standalone_observer_loads_shared_codec_without_pythonpath(self):
        environment = dict(os.environ)
        environment.pop("PYTHONPATH", None)
        result = subprocess.run([sys.executable, "-c", "from reset_observer.store import Store; from reset_observer import importers; print('ready')"],
                                cwd=Path(__file__).resolve().parents[1], env=environment,
                                capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "ready")

    def test_duplicate_forecast_arrivals_do_not_allocate_unused_storage_bodies(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/"observer.sqlite"
            store = Store(path)
            first = Forecast("source", "test", AUTOMATIC, "native", 100, 101, 10000,
                             [[24, .7], [48, .9]], 100,
                             metadata={"evidence": ["public source evidence"*300]})
            store.add_forecast(first)
            original_bodies = store.db.execute("SELECT COUNT(*) FROM json_bodies").fetchone()[0]
            self.assertGreater(original_bodies, 0)
            for at in range(102, 162):
                store.add_forecast(replace(first, available_at=at))
            self.assertEqual(store.db.execute("SELECT COUNT(*) FROM json_bodies").fetchone()[0], original_bodies)
            self.assertEqual(store.forecasts(1000), [first])
            store.close()
            store = Store(path, read_only=True)
            self.assertEqual(store.forecasts(1000, latest=True), [first])
            self.assertEqual(json.loads(store.db.execute("SELECT json_extract(qh_json(payload),'$.points') FROM forecasts").fetchone()[0]), first.points)
            store.close()

    def test_sample_body_deduplication_retains_received_and_observed_time_boundaries(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/"observer.sqlite"
            store = Store(path)
            stable = {"round": "day-1", "source": "poll", "question": "source question"*40,
                      "total": 5000, "bins": [{"p": 1, "votes": 4000}, {"p": 0, "votes": 1000}]}
            for at in range(60):
                value = {**stable, "observed_at": at, "received_at": at+3}
                with store.db:
                    store.db.execute("INSERT INTO preference_samples VALUES (?,?,?,?,?,?)",
                                     (str(at), "poll", "day-1", at, at+3,
                                      store.pack_json(value, ("observed_at", "received_at"))))
            store.close()
            store = Store(path, read_only=True)
            rows = store.db.execute("SELECT payload FROM preference_samples WHERE received_at<=? AND observed_at<=? ORDER BY observed_at", (10, 10)).fetchall()
            self.assertEqual(len(rows), 8)
            for at, row in enumerate(rows):
                self.assertEqual(json.loads(row[0]), {**stable, "observed_at": at, "received_at": at+3})
            self.assertEqual(store.db.execute("SELECT COUNT(*) FROM json_bodies").fetchone()[0], 61)
            # Physical rows remain lightweight references; all 60 samples survive.
            with closing(sqlite3.connect(path)) as raw, raw:
                self.assertEqual(raw.execute("SELECT COUNT(*) FROM preference_samples").fetchone()[0], 60)
                self.assertLess(raw.execute("SELECT MAX(length(payload)) FROM preference_samples").fetchone()[0], 100)
            store.close()


if __name__ == "__main__":
    unittest.main()
