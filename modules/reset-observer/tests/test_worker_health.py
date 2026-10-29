from pathlib import Path
from types import SimpleNamespace
import json
import os
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

from reset_observer.health import (worker_health, mark_progress, consider_recovery,
    stop_owned_worker, process_identity, public_health, LOG_LIMIT_BYTES)
from reset_observer.bounded_log import BoundedLog, compact_log
from reset_observer.runtime import evaluate, run_once
from reset_observer.importers import import_model
from test_observer import TemporaryStore, T


class WorkerHealthTests(TemporaryStore):
    def progress(self, capture=T, evaluation=T, state="idle"):
        mark_progress(self.store, "capture", state, success=state == "idle", now=capture)
        mark_progress(self.store, "evaluation", "idle", success=True, now=evaluation)

    def test_distinguishes_real_progress_from_running_lock_and_old_report(self):
        self.progress()
        self.assertEqual(worker_health(self.store.path, now=T+60)["health"], "healthy")
        self.assertEqual(worker_health(self.store.path, now=T+601)["issue"], "capture_stalled")
        self.progress(capture=T+1200)
        value = worker_health(self.store.path, now=T+1201)
        self.assertEqual(value["issue"], "evaluation_stalled")
        self.progress(capture=T+1260)
        later = worker_health(self.store.path, now=T+1261)
        self.assertEqual(value["progress_signature"], later["progress_signature"])
        self.assertNotIn("progress_signature", public_health(value))

    def test_slow_requests_maintenance_and_failed_live_sources_do_not_trigger_recovery(self):
        self.progress(state="collecting")
        self.assertEqual(worker_health(self.store.path, now=T+120)["health"], "collecting")
        self.store.put("heartbeat", {"at":T, "outcomes":{"state":"failed"}})
        self.assertEqual(worker_health(self.store.path, now=T+121)["health"], "degraded")
        self.store.put("heartbeat", {})
        self.progress(capture=T, evaluation=T, state="maintaining")
        self.assertEqual(worker_health(self.store.path, now=T+1200)["health"], "maintaining")

    def test_previous_process_evaluation_does_not_stall_a_new_capture(self):
        self.progress(capture=T, evaluation=T-86400, state="collecting")
        old = self.store.get("evaluation_progress")
        self.store.put("evaluation_progress", {**old, "pid": -1})
        health = worker_health(self.store.path, now=T+120)
        self.assertEqual(health["health"], "collecting")
        self.assertIsNone(health["issue"])
        self.assertEqual(health["last_report_at"], public_health(worker_health(self.store.path, now=T))["last_report_at"])
        # The waiting marker belongs to the new process and keeps old report
        # freshness honest without hiding a subsequently stuck live evaluator.
        mark_progress(self.store, "evaluation", "waiting", now=T)
        self.assertEqual(self.store.get("evaluation_progress")["last_success_at"], T-86400)
        self.assertEqual(worker_health(self.store.path, now=T+120)["health"], "collecting")
        mark_progress(self.store, "evaluation", "evaluating", now=T+120)
        mark_progress(self.store, "capture", "idle", success=True, now=T+1100)
        self.assertEqual(worker_health(self.store.path, now=T+1101)["issue"], "evaluation_stalled")

    def test_pending_backlog_is_visible_without_overriding_active_stages(self):
        self.progress()
        self.store.put("catch_up", {"state": "pending", "remaining_hours": 20})
        self.assertEqual(worker_health(self.store.path, now=T+1)["health"], "catching_up")
        mark_progress(self.store, "capture", "collecting", now=T+2)
        self.assertEqual(worker_health(self.store.path, now=T+3)["health"], "collecting")

    def test_capture_and_evaluation_report_actual_success_and_failure(self):
        args = SimpleNamespace(db=self.store.path, producer_db=None, ingest=None, output=None, html=None)
        with patch("reset_observer.runtime.collect", return_value=[]):
            run_once(args, {})
        self.assertIsNotNone(self.store.get("capture_progress")["last_success_at"])
        prior = self.store.get("evaluation_progress")["last_success_at"]
        with patch("reset_observer.runtime.settle_all", side_effect=ValueError("fixture")):
            with self.assertRaises(ValueError):
                evaluate(args, self.policy, T, {}, {"phase_seconds":{}})
        failed = self.store.get("evaluation_progress")
        self.assertEqual(failed["state"], "failed")
        self.assertEqual(failed["last_success_at"], prior)

    def test_persistent_capture_errors_do_not_restart_a_responsive_worker(self):
        self.progress()
        for now in (T+1200, T+1260, T+1320):
            mark_progress(self.store, "capture", "failed", error="TimeoutError", now=now)
            self.store.put("heartbeat", {"at":now, "error":"TimeoutError"})
            health = worker_health(self.store.path, now=now+1)
            self.assertEqual(health["health"], "degraded")
            self.assertEqual(health["issue"], "capture_failed")

    def test_recovery_requires_persistent_stall_ownership_and_cooldown(self):
        args = SimpleNamespace(db=self.store.path)
        owner = {"managed":True,"launch_signature":"s","owner_nonce":"nonce","process_identity":"born"}
        health = {"health":"stalled","issue":"evaluation_stalled","progress_signature":"p"}
        with patch("reset_observer.health.stop_owned_worker", return_value=True) as stop:
            for bad in ({**owner,"managed":False}, {**owner,"launch_signature":"other"}, {**owner,"owner_nonce":None}):
                self.assertIsNone(consider_recovery(args,bad,health,"s","build",now=T,monotonic=100))
            self.assertIsNone(consider_recovery(args,owner,health,"s",None,now=T,monotonic=100))
            self.assertEqual(consider_recovery(args,owner,health,"s","build",now=T,monotonic=100), "probing")
            self.assertEqual(consider_recovery(args,owner,health,"s","build",now=T+60,monotonic=160), "probing")
            stop.assert_not_called()
            self.assertEqual(consider_recovery(args,owner,health,"s","build",now=T+120,monotonic=220), "recovering")
            self.assertEqual(consider_recovery(args,owner,health,"s","build",now=T+180,monotonic=280), "cooldown")
            self.assertEqual(stop.call_count, 1)
            self.assertEqual(consider_recovery(args,owner,{**health,"progress_signature":"new"},"s","build",now=T+180,monotonic=280), "probing")
            # A restarted monotonic clock (machine reboot) starts a new probe.
            self.assertEqual(consider_recovery(args,owner,health,"s","build",now=T+200,monotonic=1), "probing")

    def test_pid_reuse_and_changed_owner_cannot_terminate_another_process(self):
        owner = {"pid":999999,"owner_nonce":"ours","process_identity":"original"}
        Path(str(self.store.path)+".worker.json").write_text(json.dumps(owner))
        with patch("reset_observer.health.process_identity",return_value="reused"):
            self.assertFalse(stop_owned_worker(self.store.path, owner))
        self.assertIsNotNone(process_identity(os.getpid()))

    def test_recovery_probe_does_not_depend_on_the_stalled_database_write_lock(self):
        args = SimpleNamespace(db=self.store.path)
        owner = {"managed":True,"launch_signature":"s","owner_nonce":"nonce","process_identity":"born"}
        health = {"health":"stalled","issue":"evaluation_stalled","progress_signature":"p"}
        writer = sqlite3.connect(self.store.path)
        try:
            writer.execute("BEGIN IMMEDIATE")
            with patch("reset_observer.health.stop_owned_worker", return_value=True) as stop:
                self.assertEqual(consider_recovery(args,owner,health,"s","build",now=T,monotonic=100), "probing")
                self.assertEqual(consider_recovery(args,owner,health,"s","build",now=T+120,monotonic=220), "recovering")
                stop.assert_called_once()
        finally:
            writer.rollback()
            writer.close()

    def test_storage_reports_actual_bytes_and_compaction_status_without_paths(self):
        self.store.put("storage_maintenance", {"observer":{"phase":"done"},"producer":{"phase":"vacuum","state":"waiting_for_disk_space"}})
        value = worker_health(self.store.path, now=T)["storage"]
        expected = sum(p.stat().st_size for p in [self.store.path, Path(str(self.store.path)+"-wal")] if p.exists())
        self.assertEqual(value["observer_bytes"], expected)
        self.assertEqual(value["log_limit_bytes"], LOG_LIMIT_BYTES)
        self.assertNotIn(str(self.store.path), json.dumps(value))

    def test_import_preserves_build_provenance_without_relabelling_old_forecasts(self):
        payload = {"schema_version":"reset-intelligence/1", "model_version":"m", "config_hash":"a", "target":"quota_relief",
                   "issued_at":T,"valid_until":T+3600,"status":"current","inputs_hash":"inputs", "training":{"state":"prior"},
                   "curve":[{"hours":24,"probability":.5}],"components":[]}
        # Use the product event key from this store's fixture.
        from reset_observer.contracts import RELIEF
        payload["target"] = RELIEF
        import_model(self.store,payload,T)
        payload.update(evaluation_version="m:rules", config_hash="b", issued_at=T+60)
        import_model(self.store,payload,T+60)
        payload.update(config_hash="c", issued_at=T+120)
        import_model(self.store,payload,T+120)
        rows = self.store.forecasts(T+120)
        self.assertEqual([r.version for r in rows], ["m:a","m:rules","m:rules"])
        self.assertEqual([r.metadata["build_config_hash"] for r in rows], ["a","b","c"])


class LogLimitTests(unittest.TestCase):
    def test_existing_oversized_log_and_future_writes_stay_bounded_without_backups(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/"worker.log"
            path.write_bytes(b"old line\n" * 2000)
            with BoundedLog(path, limit=1024) as log:
                self.assertLessEqual(path.stat().st_size, 1024)
                for _ in range(100):
                    log.write("new receipt 数据\n")
                    self.assertLessEqual(path.stat().st_size, 1024)
                log.write("large"*10000)
                self.assertLessEqual(path.stat().st_size, 1024)
                log.write("\nlatest receipt\n")
            self.assertTrue(path.read_text().endswith("latest receipt\n"))
            self.assertEqual(list(Path(directory).iterdir()), [path])
