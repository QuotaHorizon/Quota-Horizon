"""Background evaluator lifecycle; no OS service installation or app dependency."""
from __future__ import annotations

from contextlib import contextmanager
import json
import os
from pathlib import Path
import subprocess
import sqlite3
import sys
import tempfile
import time

from .benchmark import freeze_grid, initialize, settle_all
from .contracts import Policy, digest, iso
from .evidence import collect, reconcile
from .importers import intelligence_database, json_lines
from .reporting import build_report, html_report
from .store import Store


def atomic_write(path, text):
    path = Path(path).resolve()
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".observer-", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            stream.write(text)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


@contextmanager
def exclusive(db):
    lock = Path(str(Path(db).resolve()) + ".lock")
    lock.parent.mkdir(parents=True, exist_ok=True)
    with lock.open("a+b") as handle:
        if sys.platform == "win32":
            import msvcrt
            handle.seek(0)
            handle.write(b"0")
            handle.flush()
            handle.seek(0)
            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            yield
        finally:
            if sys.platform == "win32":
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def running(db):
    if not Path(str(Path(db).resolve()) + ".lock").exists():
        return False
    try:
        with exclusive(db):
            return False
    except (BlockingIOError, PermissionError):
        return True


def read_config(path):
    return json.loads(Path(path).read_text()) if path else {}


def producer_due(store, now, interval):
    last = store.get("last_producer")
    if last is None:
        return True
    elapsed = now - last["at"]
    if elapsed < 60:
        return False
    latest = store.forecasts(now, latest=True)
    models = [f for f in latest if f.track == "model"]
    expiring = [f for f in latest if f.track in ("model", "native") and not f.issues]
    # A dropped, expired source still needs a refresh even if the reduced
    # ensemble gave itself a later expiry. The producer throttles source reads.
    return elapsed >= interval or last["result"]["status"] != "current" or not models or any(f.expires_at <= now + 60 for f in expiring)


def run_once(args, config):
    policy = Policy(**config.get("policy", {}))
    store = Store(args.db)
    try:
        initialize(store, time.time(), policy)
        producer = {"status": "external"}
        last_producer = store.get("last_producer")
        if last_producer:
            producer = {**last_producer["result"], "last_attempt": iso(last_producer["at"])}
        if config.get("producer_command") and producer_due(store, time.time(), policy.collection_seconds):
            command = config["producer_command"]
            if not isinstance(command, list) or not all(isinstance(s, str) for s in command):
                raise ValueError("producer_command must be an argument array")
            try:
                result = subprocess.run(command, cwd=config.get("producer_cwd"), stdin=subprocess.DEVNULL,
                                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=90, check=False)
                producer = {"status": "current" if result.returncode == 0 else "unavailable", "exit_code": result.returncode}
            except (OSError, subprocess.TimeoutExpired) as exc:
                producer = {"status": "unavailable", "error": type(exc).__name__}
            store.put("last_producer", {"at": time.time(), "result": producer})
        collected = collect(store, policy)
        now = round(time.time(), 3)
        imported = {}
        if args.producer_db:
            try:
                imported = intelligence_database(store, args.producer_db, now)
            except (OSError, ValueError, sqlite3.Error) as exc:
                imported = {"error": type(exc).__name__}
        if args.ingest:
            imported["jsonl"] = json_lines(store, args.ingest, now)
        freeze_grid(store, now, policy)
        evidence = reconcile(store, now, policy)
        revisions = settle_all(store, now, policy, evidence)
        report = build_report(store, now, policy, evidence)
        report["runtime"] = {"producer": producer, "import": imported, "pid": os.getpid()}
        semantic = {k: v for k, v in report.items() if k not in ("generated_at", "runtime", "latest")}
        semantic["latest"] = [{k: v for k, v in row.items() if k != "age_seconds"} for row in report["latest"]]
        report_hash = digest(semantic)
        if store.get("last_report_hash") != report_hash:
            store.report(now, report)
            store.put("last_report_hash", report_hash)
        if args.output:
            atomic_write(args.output, json.dumps(report, ensure_ascii=False, allow_nan=False, indent=2) + "\n")
        if args.html:
            atomic_write(args.html, html_report(report))
        store.put("heartbeat", {"at": now, "pid": os.getpid(), "state": "running", "error": None})
        return {"at": iso(now), "state": report["state"], "collected": collected, "import": imported,
                "settlement_updates": revisions, "summary": report["summary"]}
    finally:
        store.close()


def stop_path(db):
    return Path(str(Path(db).resolve()) + ".stop")


def watch(args, config):
    with exclusive(args.db):
        marker = stop_path(args.db)
        marker.unlink(missing_ok=True)
        try:
            while not marker.exists():
                began = time.monotonic()
                try:
                    print(json.dumps(run_once(args, config), ensure_ascii=False), flush=True)
                except Exception as exc:
                    # Keep collecting after a transient provider/parse failure.
                    # Logs contain categories only, not responses or private URLs.
                    with_store = Store(args.db)
                    with_store.put("heartbeat", {"at": time.time(), "pid": os.getpid(), "state": "error", "error": type(exc).__name__})
                    with_store.close()
                    print(json.dumps({"at": iso(time.time()), "error": type(exc).__name__}), flush=True)
                remaining = max(0., 60 - (time.monotonic() - began))
                until = time.monotonic() + remaining
                while not marker.exists() and time.monotonic() < until:
                    time.sleep(min(2., max(0., until - time.monotonic())))
        except KeyboardInterrupt:
            pass
        finally:
            store = Store(args.db)
            store.put("heartbeat", {"at": time.time(), "pid": os.getpid(), "state": "stopped", "error": None})
            store.close()
            marker.unlink(missing_ok=True)


def start(args):
    if running(args.db):
        return {"state": "already_running"}
    command = [sys.executable, "-m", "reset_observer", "watch", "--db", str(Path(args.db).resolve())]
    for name in ("producer_db", "config", "ingest", "output", "html"):
        value = getattr(args, name)
        if value:
            command.extend(["--" + name.replace("_", "-"), str(Path(value).resolve())])
    log_path = Path(str(Path(args.db).resolve()) + ".log")
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("ab") as log:
        process = subprocess.Popen(command, cwd=Path(__file__).resolve().parent.parent, stdin=subprocess.DEVNULL,
                                   stdout=log, stderr=log, start_new_session=True, close_fds=True)
    return {"state": "starting", "pid": process.pid, "log": str(log_path)}


def status(db):
    if not Path(db).is_file():
        return {"state": "not_started"}
    store = Store(db, read_only=True)
    try:
        heartbeat = store.get("heartbeat")
        active = running(db)
        report = store.report_at(time.time())
        return {"state": "running" if active else "stopped", "heartbeat": heartbeat,
                "heartbeat_age_seconds": time.time() - heartbeat["at"] if heartbeat else None,
                "summary": report["summary"] if report else None}
    finally:
        store.close()
