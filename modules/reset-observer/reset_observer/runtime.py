"""Background evaluator lifecycle; no OS service installation or app dependency."""

from __future__ import annotations

from contextlib import contextmanager
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import subprocess
import sqlite3
import sys
import tempfile
import time
import uuid

from .benchmark import freeze_grid, initialize, settle_all
from .contracts import Policy, digest, iso
from .evidence import ACTIVE_FEEDS, FEEDS, collect, reconcile, reinterpret_latest, completion_export
from .diagnostics import runtime_gap
from .importers import intelligence_database, json_lines
from .reporting import ReportCache, build_report, html_report
from .store import Store, storage_upgrade_step
from .health import mark_progress, process_identity, worker_health, public_health, consider_recovery
from .bounded_log import compact_log, managed_log


def atomic_write(path, text):
    path = Path(path).resolve()
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".observer-", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
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

            handle.seek(0, os.SEEK_END)
            if handle.tell() == 0:
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
    return json.loads(Path(path).read_text(encoding="utf-8")) if path else {}


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
    return (
        elapsed >= interval
        or last["result"]["status"] != "current"
        or not models
        or any(f.expires_at <= now + 60 for f in expiring)
    )


def outcome_status(store, policy, now, collected):
    # Latest attempts, never older successes hidden behind a failed/backed-off read.
    latest = {o["provider"]: o for o in store.observations(now)}
    healthy = {
        p
        for p in ACTIVE_FEEDS
        if p in latest
        and latest[p]["healthy"]
        and not latest[p]["error"]
        and 0 <= now - latest[p]["observed_at"] <= policy.maximum_coverage_gap_seconds
    }
    available = healthy | {
        p for p in ACTIVE_FEEDS if p == "completion_page" and p in latest
        and not latest[p]["error"] and latest[p]["claims"] is not None
        and 0 <= now - latest[p]["observed_at"] <= policy.maximum_coverage_gap_seconds
    }
    refreshed = available & {
        o["provider"] for o in collected if (o["healthy"] or o["provider"] == "completion_page") and not o["error"]
    }
    state = (
        "failed"
        if not available
        else "cached"
        if not refreshed
        else "refreshed"
        if len(available) == len(ACTIVE_FEEDS)
        else "partial"
    )
    return {
        "state": state,
        "refreshed_count": len(refreshed),
        "healthy_count": len(healthy),
        "total": len(ACTIVE_FEEDS),
    }


CATCH_UP_HOURS = 24
CATCH_UP_SECONDS = 2.0


def catch_up_status(store, now, policy):
    target = int(now // policy.grid_seconds) * policy.grid_seconds
    through = store.get("last_grid")
    remaining = max(0, int((target - through) // policy.grid_seconds))
    return {"state": "pending" if remaining else "current", "through": iso(through),
            "target": iso(target), "remaining_hours": remaining}


def freeze_capture_batch(store, now, policy, should_stop=None):
    count = freeze_grid(store, now, policy, maximum_hours=CATCH_UP_HOURS,
                        maximum_seconds=CATCH_UP_SECONDS, should_stop=should_stop,
                        progress=lambda _: mark_progress(store, "capture", "collecting"))
    store.put("catch_up", catch_up_status(store, now, policy))
    return count


def run_once(args, config, refresh=False, evaluator=None):
    began = time.monotonic()
    timings = {}
    policy = Policy(**config.get("policy", {}))
    store = Store(args.db)
    try:
        mark_progress(store, "capture", "collecting")
        initialize(store, time.time(), policy)
        from .reaction import initialize as initialize_reaction
        initialize_reaction(store, time.time())
        phase = time.monotonic()
        collected = collect(store, policy, refresh=True) if refresh else collect(store, policy)
        now = round(time.time(), 3)
        reinterpret_latest(store, now)
        evidence = reconcile(store, now, policy)
        completion_path = str(Path(args.db).resolve()) + ".completions.json"
        atomic_write(completion_path, json.dumps(completion_export(evidence, now)))
        timings["outcome_collection"] = time.monotonic() - phase
        phase = time.monotonic()
        producer = {"status": "external"}
        last_producer = store.get("last_producer")
        if last_producer:
            producer = {
                **last_producer["result"],
                "last_attempt": iso(last_producer["at"]),
            }
        producer_signature = digest(
            {
                "command": config.get("producer_command"),
                "cwd": config.get("producer_cwd"),
                "runtime": bundled_revision(),
            }
        )
        if config.get("producer_command") and (
            refresh
            or store.get("producer_signature") != producer_signature
            or producer_due(store, time.time(), 60)
        ):
            command = config["producer_command"]
            if not isinstance(command, list) or not all(
                isinstance(s, str) for s in command
            ):
                raise ValueError("producer_command must be an argument array")
            if refresh and any(
                command[i : i + 2] == ["-m", "reset_intelligence"]
                for i in range(len(command) - 1)
            ):
                command = [*command, "--refresh"]
            if any(command[i : i + 2] == ["-m", "reset_intelligence"] for i in range(len(command) - 1)):
                command = [*command, "--completion-input", completion_path]
            try:
                result = subprocess.run(
                    command,
                    cwd=config.get("producer_cwd"),
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=90,
                    check=False,
                    creationflags=subprocess.CREATE_NO_WINDOW
                    if sys.platform == "win32"
                    else 0,
                )
                producer = {
                    "status": "current" if result.returncode == 0 else "unavailable",
                    "exit_code": result.returncode,
                }
            except (OSError, subprocess.TimeoutExpired) as exc:
                producer = {"status": "unavailable", "error": type(exc).__name__}
            store.put("last_producer", {"at": time.time(), "result": producer})
            store.put("producer_signature", producer_signature)
        timings["producer"] = time.monotonic() - phase
        phase = time.monotonic()
        now = round(time.time(), 3)
        outcomes = outcome_status(store, policy, time.time(), collected)
        refresh_state = (
            "failed"
            if producer["status"] == "unavailable" or outcomes["state"] == "failed"
            else "cached"
            if outcomes["state"] == "cached"
            else "refreshed"
        )
        imported = {}
        if args.producer_db:
            try:
                imported = intelligence_database(store, args.producer_db, now)
            except (OSError, ValueError, sqlite3.Error) as exc:
                imported = {"error": type(exc).__name__}
        if args.ingest:
            imported["jsonl"] = json_lines(store, args.ingest, now)
        timings["import"] = time.monotonic() - phase
        phase = time.monotonic()
        if evaluator is None:
            freeze_grid(store, now, policy)
            store.put("catch_up", catch_up_status(store, now, policy))
        else:
            freeze_capture_batch(store, now, policy, lambda: stop_path(args.db).exists())
        from .preferences import current_features, freeze as freeze_preferences

        # Both consumers use this exact capture cutoff. Carry the small feature
        # projection to evaluation instead of decoding twelve hours of ballot
        # receipts a second time. New captures always compute their own input.
        preference_features = current_features(store, now)
        freeze_preferences(store, now, policy, evidence, current=preference_features)
        timings["freeze_and_reconcile"] = time.monotonic() - phase
        timings["capture_total"] = time.monotonic() - began
        runtime = {
            "producer": producer,
            "outcomes": outcomes,
            "import": imported,
            "pid": os.getpid(),
            "phase_seconds": timings,
            "catch_up": catch_up_status(store, now, policy),
        }
        store.put(
            "heartbeat",
            {
                "at": time.time(),
                "captured_at": now,
                "pid": os.getpid(),
                "state": "running",
                "outcomes": outcomes,
                "refresh_state": refresh_state,
                "error": None,
                "phase_seconds": timings,
            },
        )
        mark_progress(store, "capture", "idle", success=True)
        if evaluator is None:
            result = evaluate(args, policy, now, evidence, runtime, preference_features)
        elif not stop_path(args.db).exists():
            evaluator.submit(args, policy, now, evidence, runtime, preference_features)
            result = {"state": "captured", "evaluation": "background"}
        else:
            result = {"state": "stopping", "evaluation": "deferred"}
        return {
            "at": iso(now),
            "collected": collected,
            "import": imported,
            "phase_seconds": timings,
            "refresh_state": refresh_state,
            "outcomes": outcomes,
            **result,
        }
    finally:
        store.close()


def evaluate(args, policy, now, evidence, runtime, preference_features=None, report_cache=None):
    """One evaluator per locked worker; collection owns a separate connection.

    Capture and forecast publication continue while historical scoring runs.
    Evaluation only appends outcome revisions/reports, never capture records.
    """
    store = Store(args.db)
    try:
        mark_progress(store, "evaluation", "evaluating")
        phase = time.monotonic()
        revisions = settle_all(store, now, policy, evidence)
        from .preferences import settle as settle_preferences

        settle_preferences(store, now, policy, evidence)
        from .retention import maintain_auxiliary_forecasts
        if (store.get("catch_up") or {}).get("state") != "pending":
            maintain_auxiliary_forecasts(store, now)
        timings = dict(runtime["phase_seconds"])
        timings["settlement"] = time.monotonic() - phase
        phase = time.monotonic()
        report = build_report(store, now, policy, evidence,
                              preference_features=preference_features, cache=report_cache)
        timings["report"] = time.monotonic() - phase
        report["runtime"] = {
            **runtime,
            "phase_seconds": timings,
            "published_at": iso(time.time()),
        }
        semantic = {
            k: v
            for k, v in report.items()
            if k not in ("generated_at", "runtime", "latest")
        }
        semantic["latest"] = [
            {k: v for k, v in row.items() if k != "age_seconds"}
            for row in report["latest"]
        ]
        report_hash = digest(semantic)
        if store.get("last_report_hash") != report_hash:
            store.report(now, report, archive=True)
            store.put("last_report_hash", report_hash)
        if args.output:
            atomic_write(
                args.output,
                json.dumps(
                    report, ensure_ascii=False, allow_nan=False, separators=(",", ":")
                )
                + "\n",
            )
        if args.html:
            atomic_write(args.html, html_report(report))
        from .store import maintain_raw_cache
        from .retention import reclaim_cache_pages
        maintain_raw_cache(store.db, now)
        reclaim_cache_pages(store)
        mark_progress(store, "evaluation", "idle", success=True)
        return {
            "state": report["state"],
            "settlement_updates": revisions,
            "summary": report["summary"],
        }
    except Exception as exc:
        mark_progress(store, "evaluation", "failed", error=type(exc).__name__)
        raise
    finally:
        store.close()


class BackgroundEvaluation:
    """Coalesce requests without running competing historical evaluations."""

    def __init__(self):
        self.pool = ThreadPoolExecutor(
            max_workers=1, thread_name_prefix="reset-evaluation"
        )
        self.future = None
        self.pending = None
        self.database = None
        self.latest_job = None
        self.report_cache = ReportCache()

    def submit(self, *job):
        self.latest_job = job
        self.pending = job
        self.database = job[0].db
        self.poll()

    def poll(self):
        if self.future is not None:
            if not self.future.done():
                return
            try:
                self.future.result()
            except Exception as exc:
                store = Store(self.database)
                try:
                    store.runtime_event(
                        time.time(),
                        {"kind": "evaluation_failed", "error": type(exc).__name__},
                    )
                finally:
                    store.close()
            self.future = None
        if self.pending is not None:
            self.future = self.pool.submit(evaluate, *self.pending, report_cache=self.report_cache)
            self.pending = None

    def resume_catch_up(self, should_stop):
        """Advance a bounded batch using the capture's original knowledge cutoff.

        Maintenance runs before this method while both connections are idle.
        Each batch keeps every missing window and yields before refreshing or
        stopping; a partial report explicitly retains its catch-up status.
        """
        if self.future is not None or self.pending is not None or self.latest_job is None or should_stop():
            return False
        args, policy, now, evidence, runtime, features = self.latest_job
        store = Store(args.db)
        try:
            if catch_up_status(store, now, policy)["state"] == "current":
                return False
            mark_progress(store, "capture", "collecting")
            try:
                freeze_capture_batch(store, now, policy, should_stop)
            finally:
                mark_progress(store, "capture", "idle")
            runtime = {**runtime, "catch_up": catch_up_status(store, now, policy)}
        except Exception as exc:
            mark_progress(store, "capture", "failed", error=type(exc).__name__)
            store.runtime_event(time.time(), {"kind": "catch_up_failed", "error": type(exc).__name__})
            self.latest_job = None  # The next normal capture retries the saved cursor.
            return False
        finally:
            store.close()
        if not should_stop():
            self.submit(args, policy, now, evidence, runtime, features)
        return True

    def close(self):
        self.pending = None
        self.pool.shutdown(wait=True)
        self.poll()


def maintain_storage(args):
    """The capture and evaluation connections are idle during this step."""
    store = Store(args.db)
    try:
        mark_progress(store, "capture", "maintaining")
    finally:
        store.close()
    def retire():
        from .retention import retain_reports, retain_auxiliary_forecasts
        store = Store(args.db)
        try:
            now = time.time()
            # A partial catch-up within the old 48h guard can still need an
            # unfrozen intermediate. Retire auxiliaries only after all grids.
            auxiliaries = (retain_auxiliary_forecasts(store, now)
                           if (store.get("catch_up") or {}).get("state") != "pending" else 0)
            return (retain_reports(store, now) or 0) + auxiliaries
        finally:
            store.close()
    result = {"observer": storage_upgrade_step(args.db, retire=retire)}
    if getattr(args, "producer_db", None):
        result["producer"] = storage_upgrade_step(args.producer_db)
    store = Store(args.db)
    try:
        if store.get("storage_maintenance") != result:
            store.put("storage_maintenance", result)
        mark_progress(store, "capture", "idle")
    finally:
        store.close()
    return result


def stop_path(db):
    return Path(str(Path(db).resolve()) + ".stop")


def refresh_path(db):
    return Path(str(Path(db).resolve()) + ".refresh")


def refresh_request(db):
    try:
        return refresh_path(db).read_text()[:128]
    except FileNotFoundError:
        return None


def request_refresh(args):
    """Wake the existing single writer; never launch a competing evaluation."""
    import uuid

    request = refresh_request(args.db) or uuid.uuid4().hex
    atomic_write(refresh_path(args.db), request)
    if not running(args.db):
        start(args)
    end = time.monotonic() + 40
    while time.monotonic() < end:
        ack = Path(str(refresh_path(args.db)) + ".result")
        try:
            result = json.loads(ack.read_text())
            if result.get("id") == request:
                return {k: result[k] for k in ("state", "outcomes") if k in result}
        except (FileNotFoundError, ValueError):
            pass
        time.sleep(0.2)
    return {"state": "refreshing"}


def bundled_revision():
    manifest = Path(sys.prefix) / "reset-runtime-manifest.json"
    if not manifest.exists():
        return None
    return json.loads(manifest.read_text())["revision"]


def worker_path(db):
    return Path(str(Path(db).resolve()) + ".worker.json")


def launch_signature(args, config):
    paths = {}
    for name in ("db", "producer_db", "config", "ingest", "output", "html"):
        value = getattr(args, name, None)
        paths[name] = str(Path(value).resolve()) if value else None
    # Worker metadata contains no private command arguments or configuration.
    return digest({"paths": paths, "config": config})


def watch(args, config):
    with exclusive(args.db), managed_log(args.db, getattr(args, "managed_log", False)):
        revision = bundled_revision()
        atomic_write(
            worker_path(args.db),
            json.dumps(
                {
                    "revision": revision,
                    "pid": os.getpid(),
                    "launch_signature": launch_signature(args, config),
                    "managed": revision is not None,
                    "owner_nonce": uuid.uuid4().hex,
                    "process_identity": process_identity(os.getpid()),
                }
            ),
        )
        marker = stop_path(args.db)
        marker.unlink(missing_ok=True)
        store = Store(args.db)
        previous = store.get("heartbeat")
        now = time.time()
        # Old reports remain visibly old, but an evaluator from a previous
        # process must not make this process's first capture look stalled.
        mark_progress(store, "evaluation", "waiting", now=now)
        store.runtime_event(
            now, {"kind": "process_start", "at": now, "pid": os.getpid()}
        )
        if previous and now - previous["at"] > 180:
            store.runtime_event(
                now, {"kind": "process_unobserved", "from": previous["at"], "to": now}
            )
        store.close()
        previous_wall, previous_mono = now, time.monotonic()
        evaluator = BackgroundEvaluation()
        next_maintenance = 0.0
        try:
            while not marker.exists():
                # An app replacement can change the runtime on disk while this
                # detached process still has the previous modules in memory.
                # Finish the current tick and release the lock before restart.
                if revision is not None and bundled_revision() != revision:
                    break
                began = time.monotonic()
                wall = time.time()
                gap = runtime_gap(previous_wall, previous_mono, wall, began)
                if gap:
                    store = Store(args.db)
                    store.runtime_event(wall, gap)
                    store.close()
                previous_wall, previous_mono = wall, began
                request = refresh_request(args.db)
                refresh_state = "failed"
                outcomes = None
                try:
                    result = (
                        run_once(args, config, refresh=True, evaluator=evaluator)
                        if request
                        else run_once(args, config, evaluator=evaluator)
                    )
                    print(json.dumps(result, ensure_ascii=False), flush=True)
                    refresh_state = result.get("refresh_state", "refreshed")
                    outcomes = result.get("outcomes")
                except Exception as exc:
                    # Keep collecting after a transient provider/parse failure.
                    # Logs contain categories only, not responses or private URLs.
                    with_store = Store(args.db)
                    mark_progress(with_store, "capture", "failed", error=type(exc).__name__)
                    with_store.put(
                        "heartbeat",
                        {
                            "at": time.time(),
                            "pid": os.getpid(),
                            "state": "error",
                            "error": type(exc).__name__,
                        },
                    )
                    with_store.close()
                    print(
                        json.dumps(
                            {"at": iso(time.time()), "error": type(exc).__name__}
                        ),
                        flush=True,
                    )
                finally:
                    if request:
                        ack = {"id": request, "state": refresh_state}
                        if outcomes is not None:
                            ack["outcomes"] = outcomes
                        atomic_write(
                            str(refresh_path(args.db)) + ".result", json.dumps(ack)
                        )
                        if refresh_request(args.db) == request:
                            refresh_path(args.db).unlink(missing_ok=True)
                # Source intervals start when collection finishes. Starting
                # this timer before the work skips the next collection tick.
                # Manual requests still wake this wait immediately.
                until = time.monotonic() + 60
                while (
                    not marker.exists()
                    and time.monotonic() < until
                    and not refresh_request(args.db)
                ):
                    evaluator.poll()
                    if evaluator.future is None and evaluator.pending is None and time.monotonic() >= next_maintenance:
                        maintenance = maintain_storage(args)
                        quiet = all(r["phase"] == "done" or r.get("state") in ("deferred", "retry_later", "waiting_for_disk_space") for r in maintenance.values())
                        next_maintenance = time.monotonic() + (3600 if quiet else 2)
                    if time.monotonic() < until:
                        evaluator.resume_catch_up(lambda: marker.exists() or bool(refresh_request(args.db)))
                    time.sleep(min(2.0, max(0.0, until - time.monotonic())))
        except KeyboardInterrupt:
            pass
        finally:
            evaluator.close()
            store = Store(args.db)
            store.put(
                "heartbeat",
                {
                    "at": time.time(),
                    "pid": os.getpid(),
                    "state": "stopped",
                    "error": None,
                },
            )
            store.close()
            marker.unlink(missing_ok=True)


def start(args):
    if running(args.db):
        revision = bundled_revision()
        try:
            previous = json.loads(worker_path(args.db).read_text())
        except (OSError, ValueError):
            previous = {}
        expected = launch_signature(args, read_config(getattr(args, "config", None)))
        revision_changed = revision is not None and revision != previous.get("revision")
        configuration_changed = previous.get("launch_signature") != expected
        # Older external workers have not opted into managed configuration handoff.
        if revision_changed or (
            configuration_changed
            and (revision is not None or "launch_signature" in previous)
        ):
            stop_path(args.db).touch()
            return {"state": "restarting"}
        health = worker_health(args.db, getattr(args, "producer_db", None))
        recovery = consider_recovery(args, previous, health, expected, revision)
        if recovery == "recovering":
            health["health"] = "recovering"
        if revision is None or not previous.get("managed"):
            health["health"] = "external" if health["health"] == "healthy" else health["health"]
        return {"state": "restarting" if recovery == "recovering" else "already_running", **public_health(health)}
    command = [
        sys.executable,
        *(["-I", "-B", "-X", "utf8"] if bundled_revision() else []),
        "-m",
        "reset_observer",
        "watch",
        "--managed-log",
        "--db",
        str(Path(args.db).resolve()),
    ]
    for name in ("producer_db", "config", "ingest", "output", "html"):
        value = getattr(args, name)
        if value:
            command.extend(["--" + name.replace("_", "-"), str(Path(value).resolve())])
    log_path = Path(str(Path(args.db).resolve()) + ".log")
    log_path.parent.mkdir(parents=True, exist_ok=True)
    compact_log(log_path)
    with log_path.open("ab") as log:
        process = subprocess.Popen(
            command,
            cwd=Path(__file__).resolve().parent.parent,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=log,
            start_new_session=True,
            close_fds=True,
            creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0,
        )
    result = started_worker(process, args.db, log_path)
    return {**result, **public_health(worker_health(args.db, getattr(args, "producer_db", None))),
            "health": "starting"}


def started_worker(process, db, log_path, timeout=3.0):
    # Confirm that a worker actually acquired the singleton lock. This also
    # handles concurrent starts: the losing launcher can reuse the winner.
    deadline = time.monotonic() + timeout
    while True:
        if running(db):
            return {"state": "starting", "pid": process.pid, "log": str(log_path)}
        if process.poll() is not None:
            return {"state": "unavailable", "error": "worker_exited"}
        if time.monotonic() >= deadline:
            process.terminate()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            return {"state": "unavailable", "error": "startup_timeout"}
        time.sleep(0.05)


def status(db, producer_db=None):
    if not Path(db).is_file():
        return {"state": "not_started"}
    store = Store(db, read_only=True)
    try:
        heartbeat = store.get("heartbeat")
        active = running(db)
        report = store.report_at(time.time())
        return {
            "state": "running" if active else "stopped",
            **public_health(worker_health(db, producer_db)),
            "heartbeat": heartbeat,
            "heartbeat_age_seconds": time.time() - heartbeat["at"]
            if heartbeat
            else None,
            "summary": report["summary"] if report else None,
        }
    finally:
        store.close()
