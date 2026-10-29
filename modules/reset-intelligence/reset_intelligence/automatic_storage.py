"""Resume old-cache upgrades between collection ticks, without data backups.

Compression runs in checked, short batches. Retirement keeps the existing
evidence policy. One final SQLite compaction enables incremental page reclaim
on old databases. A durable identity map protects import cursors across VACUUM.
"""
import json
from pathlib import Path
import shutil
import sqlite3
import time

from .maintenance import (compact, collect_unused_bodies, connect, database_info,
                          quote, rowid_identities, tables)
from .storage_codec import configure_storage, prepare_upgrade, retain_raw_cache, reclaim_pages


def put(db, key, value):
    with db:
        db.execute("INSERT OR REPLACE INTO qh_storage_upgrade VALUES (?,?)", (key, str(value)))


def get(db, key):
    row = db.execute("SELECT value FROM qh_storage_upgrade WHERE key=?", (key,)).fetchone()
    return row[0] if row else None


def identity_columns(db, table):
    columns = db.execute(f"PRAGMA table_info({quote(table)})").fetchall()
    keys = [c[1] for c in sorted(columns, key=lambda c: c[5]) if c[5]]
    return keys or [c[1] for c in columns if c[1] not in ("payload", "body", "normalized", "claims", "value")]


def capture_rowids(db):
    """Saved keys are cursors, not a second copy of application data."""
    with db:
        db.execute("CREATE TABLE IF NOT EXISTS qh_storage_rowids(table_name TEXT,identity TEXT,old_rowid INTEGER,PRIMARY KEY(table_name,identity))")
        db.execute("DELETE FROM qh_storage_rowids")
        identities = rowid_identities(db)
        for table in tables(db):
            if table == "json_bodies":
                continue
            columns = identity_columns(db, table)
            expression = "json_array(" + ",".join(quote(k) for k in columns) + ")"
            db.execute(f"INSERT INTO qh_storage_rowids SELECT ?,{expression},rowid FROM {quote(table)}", (table,))
        put(db, "rowid_fingerprints", json.dumps(identities, sort_keys=True))


def recover_rowids(db):
    """Also runs at writer startup after an interrupted final compaction."""
    if not db.execute("SELECT 1 FROM sqlite_master WHERE name='qh_storage_upgrade'").fetchone():
        return
    saved = get(db, "rowid_fingerprints")
    if not saved:
        return
    expected = json.loads(saved)
    current = rowid_identities(db)
    with db:
        for table, fingerprint in expected.items():
            if current.get(table) == fingerprint:
                continue
            keys = identity_columns(db, table)
            expression = "json_array(" + ",".join("t." + quote(k) for k in keys) + ")"
            count, minimum, maximum = db.execute(f"SELECT COUNT(*),MIN(rowid),MAX(rowid) FROM {quote(table)}").fetchone()
            saved_count, saved_max = db.execute("SELECT COUNT(*),MAX(old_rowid) FROM qh_storage_rowids WHERE table_name=?", (table,)).fetchone()
            if count != saved_count or (minimum is not None and minimum < 0):
                raise ValueError("storage upgrade identity inventory changed")
            missing = db.execute(f"SELECT COUNT(*) FROM {quote(table)} t WHERE NOT EXISTS (SELECT 1 FROM qh_storage_rowids m WHERE m.table_name=? AND m.identity={expression})", (table,)).fetchone()[0]
            if missing:
                raise ValueError("storage upgrade identity key changed")
            # Move outside both old and current ranges, then restore exact IDs.
            # Both updates share one transaction, so an interrupted restore rolls
            # back and can be repeated on the next launch.
            offset = max(maximum or 0, saved_max or 0) + 1
            db.execute(f"UPDATE {quote(table)} SET rowid=rowid+?", (offset,))
            db.execute(f"UPDATE {quote(table)} AS t SET rowid=(SELECT old_rowid FROM qh_storage_rowids m WHERE m.table_name=? AND m.identity={expression})", (table,))
        if rowid_identities(db) != expected:
            raise ValueError("storage upgrade row identity verification failed")
        db.execute("DELETE FROM qh_storage_upgrade WHERE key='rowid_fingerprints'")
        db.execute("DROP TABLE qh_storage_rowids")


def finish(path):
    db = connect(path)
    try:
        # The runtime calls this only between ticks with its evaluator idle.
        # SQLite also excludes any accidental second writer throughout the
        # map/VACUUM/restore sequence, including transaction boundaries.
        db.execute("PRAGMA locking_mode=EXCLUSIVE")
        db.execute("BEGIN EXCLUSIVE")
        db.commit()
        recover_rowids(db)
        before = database_info(db)
        required = before["live_page_bytes"] * 2 + 64 * 1024 * 1024
        if shutil.disk_usage(Path(path).resolve().parent).free < required:
            return {"phase": "vacuum", "state": "waiting_for_disk_space"}
        needs_vacuum = db.execute("PRAGMA auto_vacuum").fetchone()[0] != 2 or before["free_pages"] * before["page_size"] >= 16 * 1024 * 1024
        if needs_vacuum:
            capture_rowids(db)
            db.execute("PRAGMA auto_vacuum=INCREMENTAL")
            db.execute("VACUUM")
            recover_rowids(db)
            if db.execute("PRAGMA quick_check").fetchone()[0] != "ok":
                raise ValueError("storage upgrade integrity check failed")
        put(db, "phase", "done")
        put(db, "finished_at", time.time())
        db.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchall()
        return {"phase": "done", "before_bytes": before["logical_bytes"], "after_bytes": database_info(db)["logical_bytes"]}
    finally:
        db.close()


def _step(path, *, retire=None, batch_rows=256, max_batches=4):
    """One resumable step; callers retain ownership of collection scheduling."""
    db = connect(path)
    try:
        prepare_upgrade(db)
        recover_rowids(db)
        phase = get(db, "phase")
        retry = float(get(db, "retry_after") or 0)
        if retry > time.time():
            return {"phase": phase, "state": "retry_later"}
        if phase == "done":
            reclaimed = reclaim_pages(db)
            return {"phase": phase, "reclaimed_bytes": reclaimed}
    finally:
        db.close()
    try:
        if phase == "compress":
            result = compact(path, batch_rows=batch_rows, batch_seconds=0.2, max_batches=max_batches)
            if result["status"] != "complete":
                return {"phase": phase, "state": "progress"}
            phase = "retire"
        if phase == "retire":
            db = connect(path)
            try:
                put(db, "phase", phase)
                configure_storage(db)
                count = retain_raw_cache(db, time.time(), maximum_rows=2048)
            finally:
                db.close()
            if retire:
                count += retire()
            if count:
                return {"phase": phase, "retired": count}
            phase = "bodies"
        if phase == "bodies":
            collect_unused_bodies(path, writers_stopped=True)
            phase = "vacuum"
        db = connect(path)
        try:
            put(db, "phase", phase)
        finally:
            db.close()
        return finish(path)
    except (OSError, ValueError, sqlite3.Error) as error:
        db = connect(path)
        try:
            put(db, "retry_after", time.time() + 3600)
            put(db, "last_error", type(error).__name__)
        finally:
            db.close()
        return {"phase": phase, "state": "deferred", "error": type(error).__name__}


def step(path, *, retire=None, batch_rows=256, max_batches=4):
    # Opening a busy/unavailable database must not terminate the capture worker.
    # Writer startup separately enforces any pending identity recovery.
    try:
        return _step(path, retire=retire, batch_rows=batch_rows, max_batches=max_batches)
    except (OSError, ValueError, sqlite3.Error) as error:
        return {"phase": "unavailable", "state": "deferred", "error": type(error).__name__}
