import json
from pathlib import Path
from types import SimpleNamespace
import tempfile
import subprocess
import sys
import time
import unittest
from unittest.mock import patch

from reset_observer.runtime import (
    launch_signature,
    start,
    worker_path,
    stop_path,
    watch,
    atomic_write,
    exclusive,
    running,
    read_config,
)
from reset_observer.evidence import text_stage, reported_stage


class BundledUpgradeTests(unittest.TestCase):
    def test_utf8_exports_and_configuration_round_trip_on_every_platform(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "radar.json"
            value = {"statement": "额度更新 · 📈", "path": str(Path(d) / "中文目录")}
            atomic_write(path, json.dumps(value, ensure_ascii=False))
            self.assertEqual(json.loads(path.read_bytes().decode("utf-8")), value)
            self.assertEqual(read_config(path), value)

    @unittest.skipUnless(sys.platform == "win32", "Windows byte-range lock")
    def test_lock_probes_do_not_grow_the_lock_file_or_release_the_worker(self):
        with tempfile.TemporaryDirectory() as d:
            db = Path(d) / "state.sqlite"
            with exclusive(db):
                for _ in range(3):
                    self.assertTrue(running(db))
                self.assertEqual(Path(str(db) + ".lock").stat().st_size, 1)
            self.assertFalse(running(db))
            self.assertEqual(Path(str(db) + ".lock").stat().st_size, 1)

    def test_conditional_pledge_does_not_inherit_a_tracker_completion_label(self):
        text = "For the next 28 days, each day we either ship improvements or ship a full reset."
        self.assertEqual(text_stage(text), "conditional")
        self.assertEqual(reported_stage("completed", text), "conditional")

    def test_upgrade_requests_graceful_stop_and_reuses_matching_worker(self):
        with tempfile.TemporaryDirectory() as d:
            args = SimpleNamespace(db=str(Path(d) / "state.sqlite"))
            with (
                patch("reset_observer.runtime.running", return_value=True),
                patch("reset_observer.runtime.bundled_revision", return_value="v2"),
            ):
                worker_path(args.db).write_text(json.dumps({"revision": "v1"}))
                self.assertEqual(start(args)["state"], "restarting")
                self.assertTrue(stop_path(args.db).exists())
                stop_path(args.db).unlink()
                worker_path(args.db).write_text(
                    json.dumps(
                        {
                            "revision": "v2",
                            "launch_signature": launch_signature(args, {}),
                        }
                    )
                )
                self.assertEqual(start(args)["state"], "already_running")
                self.assertFalse(stop_path(args.db).exists())

    def test_external_worker_retains_its_own_lifecycle(self):
        with (
            patch("reset_observer.runtime.running", return_value=True),
            patch("reset_observer.runtime.bundled_revision", return_value=None),
        ):
            self.assertEqual(
                start(SimpleNamespace(db="unused"))["state"], "already_running"
            )

    def test_matching_runtime_restarts_when_launch_paths_or_configuration_change(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            config = root / "private-config.json"
            settings = {"producer_command": ["existing-python", "private-argument"]}
            config.write_text(json.dumps(settings), encoding="utf-8")
            original = SimpleNamespace(
                db=str(root / "state.sqlite"),
                config=str(config),
                output=str(root / "report.json"),
                html=None,
                ingest=None,
                producer_db=str(root / "producer.sqlite"),
            )
            worker = {
                "revision": "v2",
                "launch_signature": launch_signature(original, settings),
            }
            worker_path(original.db).write_text(json.dumps(worker), encoding="utf-8")
            for revision in ("v2", None):
                with (
                    patch("reset_observer.runtime.running", return_value=True),
                    patch(
                        "reset_observer.runtime.bundled_revision", return_value=revision
                    ),
                ):
                    self.assertEqual(start(original)["state"], "already_running")
                    for name in ("output", "html", "producer_db", "ingest", "config"):
                        changed = SimpleNamespace(
                            **{**vars(original), name: str(root / (name + ".json"))}
                        )
                        if name == "config":
                            Path(changed.config).write_text(
                                json.dumps(settings), encoding="utf-8"
                            )
                        self.assertEqual(start(changed)["state"], "restarting")
                        self.assertTrue(stop_path(original.db).exists())
                        stop_path(original.db).unlink()
            with (
                patch("reset_observer.runtime.running", return_value=True),
                patch("reset_observer.runtime.bundled_revision", return_value="v2"),
            ):
                config.write_text(
                    json.dumps(
                        {
                            "producer_command": [
                                "existing-python",
                                "updated-private-argument",
                            ]
                        }
                    ),
                    encoding="utf-8",
                )
                self.assertEqual(start(original)["state"], "restarting")
            self.assertNotIn("private-argument", json.dumps(worker))

    def test_watch_records_only_the_signature_for_its_actual_loaded_configuration(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            args = SimpleNamespace(
                db=str(root / "state.sqlite"),
                config=None,
                output=str(root / "report.json"),
            )
            config = {"producer_command": ["private-python", "private-argument"]}

            def one_tick(*_, **__):
                stop_path(args.db).touch()
                return {"state": "test_complete"}

            with (
                patch("reset_observer.runtime.bundled_revision", return_value="v2"),
                patch("reset_observer.runtime.run_once", side_effect=one_tick),
            ):
                watch(args, config)
            worker = json.loads(worker_path(args.db).read_text(encoding="utf-8"))
            self.assertEqual(worker["launch_signature"], launch_signature(args, config))
            self.assertNotIn("private-argument", json.dumps(worker))

    def test_real_worker_releases_lock_and_replacement_uses_new_output(self):
        script = """
import json, sys
from types import SimpleNamespace
from unittest.mock import patch
from reset_observer.runtime import atomic_write, watch
args = SimpleNamespace(**json.loads(sys.argv[1]))
def tick(args, config, **kwargs):
    atomic_write(args.output, json.dumps({'fixture': config['fixture']}))
    return {'state': 'fixture_ready'}
with patch('reset_observer.runtime.run_once', side_effect=tick):
    watch(args, {'fixture': 'handoff'})
"""
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            config = root / "config.json"
            config.write_text(json.dumps({"fixture": "handoff"}), encoding="utf-8")
            args = SimpleNamespace(
                db=str(root / "observer.sqlite"),
                config=str(config),
                output=str(root / "first.json"),
                producer_db=None,
                html=None,
                ingest=None,
            )
            processes = []

            def launch(arguments):
                child = subprocess.Popen(
                    [sys.executable, "-B", "-c", script, json.dumps(vars(arguments))],
                    cwd=Path(__file__).resolve().parents[1],
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
                processes.append(child)
                deadline = time.monotonic() + 5
                while not Path(arguments.output).exists():
                    if child.poll() is not None or time.monotonic() > deadline:
                        self.fail("Isolated worker did not become ready")
                    time.sleep(0.02)
                return child

            try:
                old = launch(args)
                updated = SimpleNamespace(
                    **{**vars(args), "output": str(root / "second.json")}
                )
                self.assertEqual(start(updated)["state"], "restarting")
                self.assertEqual(old.wait(timeout=5), 0)
                replacement = launch(updated)
                self.assertEqual(
                    json.loads(Path(updated.output).read_text()), {"fixture": "handoff"}
                )
                self.assertEqual(start(updated)["state"], "already_running")
                stop_path(args.db).touch()
                self.assertEqual(replacement.wait(timeout=5), 0)
            finally:
                for child in processes:
                    if child.poll() is None:
                        child.terminate()
                        child.wait(timeout=5)
