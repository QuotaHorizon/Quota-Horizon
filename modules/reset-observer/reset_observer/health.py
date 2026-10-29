"""Progress-based worker health and bounded recovery for owned bundled workers."""
from __future__ import annotations

import ctypes
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

from .contracts import digest, iso
from .store import Store

CAPTURE_STALL_SECONDS = 600
EVALUATION_STALL_SECONDS = 900
MAINTENANCE_STALL_SECONDS = 1800
RECOVERY_CONFIRM_SECONDS = 90
RECOVERY_COOLDOWN_SECONDS = 900
LOG_LIMIT_BYTES = 5 * 1024 * 1024


def mark_progress(store, kind, state, *, success=False, error=None, now=None):
    now = time.time() if now is None else now
    key = kind + "_progress"
    previous = store.get(key) or {}
    value = {"pid": os.getpid(), "state": state, "at": now,
             "last_success_at": now if success else previous.get("last_success_at"),
             "error": error}
    store.put(key, value)
    return value


def process_identity(pid):
    """A birth identity, not just a reusable PID; unavailable means no forced stop."""
    try:
        if sys.platform == "win32":
            from ctypes import wintypes
            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
            kernel.OpenProcess.restype = wintypes.HANDLE
            kernel.GetProcessTimes.argtypes = (wintypes.HANDLE, *([ctypes.POINTER(wintypes.FILETIME)] * 4))
            kernel.GetProcessTimes.restype = wintypes.BOOL
            kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
            handle = kernel.OpenProcess(0x1000, False, pid)
            if not handle:
                return None
            try:
                values = [wintypes.FILETIME() for _ in range(4)]
                if not kernel.GetProcessTimes(handle, *(ctypes.byref(v) for v in values)):
                    return None
                return f"win:{values[0].dwHighDateTime}:{values[0].dwLowDateTime}"
            finally:
                kernel.CloseHandle(handle)
        value = subprocess.run(["ps", "-p", str(pid), "-o", "lstart="],
                               capture_output=True, text=True, timeout=2, check=False)
        return "posix:" + value.stdout.strip() if value.returncode == 0 and value.stdout.strip() else None
    except (OSError, ValueError, subprocess.SubprocessError):
        return None


def database_bytes(path):
    if not path:
        return None
    total = 0
    for item in (Path(path), Path(str(path) + "-wal")):
        try:
            total += item.stat().st_size
        except FileNotFoundError:  # A checkpoint can remove the WAL between reads.
            pass
    return total


def storage_status(db, producer_db=None, maintenance=None):
    log = Path(str(Path(db).resolve()) + ".log")
    states = []
    for module, value in (maintenance or {}).items():
        if module not in ("observer", "producer") or not isinstance(value, dict):
            continue
        states.append({"module": module, "phase": value.get("phase", "unknown"),
                       "state": value.get("state"), "error": value.get("error")})
    return {"observer_bytes": database_bytes(db), "producer_bytes": database_bytes(producer_db),
            "log_bytes": log.stat().st_size if log.is_file() else 0,
            "log_limit_bytes": LOG_LIMIT_BYTES, "maintenance": states}


def worker_health(db, producer_db=None, *, now=None):
    now = time.time() if now is None else now
    if not Path(db).is_file():
        return {"health": "starting", "last_capture_at": None, "last_report_at": None,
                "storage": storage_status(db, producer_db)}
    store = Store(db, read_only=True)
    try:
        heartbeat = store.get("heartbeat") or {}
        capture = store.get("capture_progress") or {}
        evaluation = store.get("evaluation_progress") or {}
        captured = capture.get("last_success_at") or heartbeat.get("captured_at")
        published = evaluation.get("last_success_at")
        if published is None:
            published = store.db.execute("SELECT MAX(at) FROM reports WHERE at<=?", (now,)).fetchone()[0]
        capture_clock = capture.get("at") or heartbeat.get("at")
        evaluation_clock = evaluation.get("at") or published
        health, issue = "healthy", None
        if capture_clock is None:
            health = "starting"
        elif now < capture_clock or (evaluation_clock is not None and now < evaluation_clock):
            health, issue = "degraded", "clock_changed"
        elif now-capture_clock > (MAINTENANCE_STALL_SECONDS if capture.get("state") == "maintaining" else CAPTURE_STALL_SECONDS):
            health, issue = "stalled", "capture_stalled"
        elif capture.get("state") == "failed" or heartbeat.get("error"):
            health, issue = "degraded", "capture_failed"
        elif evaluation.get("state") == "failed":
            health, issue = "degraded", "evaluation_failed"
        elif capture.get("state") != "maintaining" and evaluation_clock is not None and now-evaluation_clock > EVALUATION_STALL_SECONDS:
            health, issue = "stalled", "evaluation_stalled"
        elif heartbeat.get("outcomes", {}).get("state") == "failed" or heartbeat.get("refresh_state") == "failed":
            health, issue = "degraded", "source_updates_failed"
        elif capture.get("state") == "maintaining":
            health = "maintaining"
        elif capture.get("state") == "collecting":
            health = "collecting"
        elif evaluation.get("state") == "evaluating":
            health = "evaluating"
        return {"health": health, "issue": issue,
                "last_capture_at": iso(captured) if captured is not None else None,
                "last_report_at": iso(published) if published is not None else None,
                # Captures can continue while the evaluator is stuck; only the
                # stalled stage must remain unchanged across recovery probes.
                "progress_signature": digest([evaluation, published] if issue == "evaluation_stalled"
                                             else [capture, heartbeat.get("at")]),
                "storage": storage_status(db, producer_db, store.get("storage_maintenance"))}
    finally:
        store.close()


def public_health(value):
    return {key: value.get(key) for key in ("health", "issue", "last_capture_at", "last_report_at", "storage")}


def stop_owned_worker(db, owner):
    pid = owner.get("pid")
    if not isinstance(pid, int) or pid <= 1 or pid == os.getpid():
        return False
    current = json.loads(Path(str(Path(db).resolve()) + ".worker.json").read_text())
    if (not owner.get("owner_nonce") or not owner.get("process_identity")
            or current.get("owner_nonce") != owner.get("owner_nonce")
            or process_identity(pid) != owner.get("process_identity")):
        return False
    if sys.platform == "win32":
        result = subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"],
                                capture_output=True, timeout=3, check=False,
                                creationflags=subprocess.CREATE_NO_WINDOW)
        return result.returncode == 0
    if os.getpgid(pid) != pid:  # Only our detached session and its own producer.
        return False
    os.killpg(pid, signal.SIGTERM)
    return True


def consider_recovery(args, owner, health, expected_signature, revision, *, now=None, monotonic=None):
    """Require unchanged stalled progress on two checks, then rate-limit recovery.

    The first check after wake only records a probe. Source failures with fresh
    progress never qualify. External/legacy workers without the ownership token
    are observed but never terminated by this path.
    """
    if (health.get("health") != "stalled" or revision is None or not owner.get("managed")
            or owner.get("launch_signature") != expected_signature
            or not owner.get("owner_nonce") or not owner.get("process_identity")):
        return None
    now = time.time() if now is None else now
    monotonic = time.monotonic() if monotonic is None else monotonic
    # Recovery must not need a write lock in the database whose owner may be
    # stalled while holding that very lock. A tiny separately locked receipt
    # also serializes concurrent supervisors without touching any ledger data.
    from .runtime import atomic_write, exclusive
    path = Path(str(Path(args.db).resolve()) + ".recovery.json")
    try:
        guard = exclusive(str(Path(args.db).resolve()) + ".recovery")
        guard.__enter__()
    except (BlockingIOError, PermissionError):
        return "busy"
    try:
        try:
            with path.open(encoding="utf-8") as stream:
                saved = json.loads(stream.read(4096))
        except (OSError, ValueError):
            saved = {}
        if not isinstance(saved, dict):
            saved = {}
        def save(value):
            atomic_write(path, json.dumps(value))
        signature = digest([owner["owner_nonce"], health.get("progress_signature")])
        attempts = [at for at in saved.get("attempts", []) if 0 <= now-at < 3600]
        if saved.get("signature") != signature or not 0 <= monotonic-saved.get("monotonic", monotonic) < 3600:
            save({"signature": signature, "monotonic": monotonic, "attempts": attempts})
            return "probing"
        if monotonic-saved["monotonic"] < RECOVERY_CONFIRM_SECONDS:
            return "probing"
        if len(attempts) >= 2 or attempts and now-attempts[-1] < RECOVERY_COOLDOWN_SECONDS:
            return "cooldown"
        # Reserve the attempt before stopping so concurrent supervisors cannot
        # independently restart the same stalled owner.
        saved = {**saved, "attempts": [*attempts, now]}
        save(saved)
        try:
            stopped = stop_owned_worker(args.db, owner)
        except (OSError, ValueError, subprocess.SubprocessError):
            stopped = False
        save({**saved, "last_result": {"at": now, "issue": health.get("issue"),
                                      "state": "stop_requested" if stopped else "owner_unavailable"}})
        return "recovering" if stopped else "owner_unavailable"
    finally:
        guard.__exit__(None, None, None)
