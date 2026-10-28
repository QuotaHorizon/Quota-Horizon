"""Resumable, lossless storage maintenance for the two reset-module databases.

Run explicitly with ``python -m reset_intelligence.maintenance``. Compaction
rewrites representations, never evidence rows or their clocks. Each committed
batch includes a cold-read semantic checksum and its restart checkpoint. Writers
may append between batches after all readers understand QHJSON2. VACUUM is a
separate operation requiring stopped writers and enough temporary disk space.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import sqlite3
import sys
import time
import zlib

from .storage_codec import JSONStorage, pack_inline, RETIRED_RAW


VERSION = "lossless-json-2"
PREFIX = b"\x00QHJSON2\x00"
LEGACY_SOURCE = b"QHJSON\x01ZLIB\x00"
ZERO = "0" * 64
MODULUS = 1 << 256
_PROGRESS = "qh_storage_progress"
_JOBS = "qh_storage_jobs"


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, allow_nan=False,
                      separators=(",", ":")).encode("utf-8")


def quote(name):
    return '"' + name.replace('"', '""') + '"'


def connect(path, read_only=False):
    path = Path(path).resolve()
    if not path.is_file():
        raise ValueError("maintenance requires an existing database")
    db = sqlite3.connect(path.as_uri() + ("?mode=ro" if read_only else "?mode=rw"),
                         uri=True, timeout=5)
    db.row_factory = sqlite3.Row
    # Bound maintenance cache usage independently from the daily worker.
    db.execute("PRAGMA cache_size=-8192")
    if read_only:
        db.execute("PRAGMA query_only=ON")
    return db


def begin_batch(db, emit=None, maximum_wait=180):
    """Retry a daily writer's short reservation before starting any mutation."""
    started = time.monotonic()
    attempt = 0
    while True:
        try:
            db.execute("BEGIN IMMEDIATE")
            return
        except sqlite3.OperationalError as error:
            code = getattr(error, "sqlite_errorcode", None)
            locked = code is not None and (code & 0xff) in (sqlite3.SQLITE_BUSY, sqlite3.SQLITE_LOCKED)
            if not locked or time.monotonic() - started >= maximum_wait:
                raise
            attempt += 1
            if emit:
                emit({"event": "waiting_for_writer", "attempt": attempt,
                      "waited_seconds": round(time.monotonic() - started, 3)})
            time.sleep(min(0.1 * 2 ** min(attempt, 5), 2))


def tables(db):
    return {r[0]: {c[1] for c in db.execute(f"PRAGMA table_info({quote(r[0])})")}
            for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")
            if not r[0].startswith(("sqlite_", "qh_storage_"))}


def targets(db):
    schema = tables(db)
    observation = schema.get("observations", set())
    if {"source", "normalized", "payload_hash"} <= observation and "source_config" in schema:
        kind = "producer"
        wanted = [("payloads", "body", "source", ()),
                  ("observations", "normalized", "json", ("observed_at", "generated_at")),
                  ("forecasts", "payload", "json", ()),
                  ("source_config", "payload", "json", ())]
    elif {"provider", "claims", "healthy"} <= observation and "trials" in schema:
        kind = "observer"
        wanted = [("payloads", "body", "keep_zlib", ()),
                  ("observations", "claims", "json", ()),
                  ("reports", "payload", "report", ())]
        wanted += [(name, "payload", "json", ()) for name in (
            "claims", "forecasts", "indicators", "truth_revisions", "trials",
            "settlements", "runtime_events", "preference_samples", "preference_trials",
            "preference_settlements", "reaction_forecasts", "trial_revisions")]
    else:
        raise ValueError("not a recognized reset producer/observer database")
    return kind, [(t, c, m, v) for t, c, m, v in wanted if c in schema.get(t, ())]


def unpack(codec, value, mode):
    if value is None:
        return None
    if mode in ("source", "keep_zlib") and value == RETIRED_RAW:
        return {"expired_raw_response": True}
    value = codec.expand_json(value)
    if isinstance(value, bytes):
        if mode == "source" and value.startswith(LEGACY_SOURCE):
            value = zlib.decompress(value[len(LEGACY_SOURCE):])
        elif mode in ("keep_zlib", "report"):
            value = zlib.decompress(value)
    return json.loads(value)


def encode(codec, original, value, mode, volatile):
    if original is None:
        return original
    if mode in ("source", "keep_zlib"):
        if original == RETIRED_RAW:
            return original
        return pack_inline(value)
    if mode == "report":
        return pack_inline(value)
    if isinstance(original, bytes) and original.startswith(PREFIX):
        return original
    return codec.pack_json(value, volatile)


def value_size(value):
    return len(value.encode("utf-8") if isinstance(value, str) else value or b"")


def row_digest(row, column, value):
    # Include every application column, including identifiers, clocks and NULLs.
    # Internal rowids are traversal cursors, not identity: VACUUM may renumber them.
    content = []
    for key in row.keys():
        if key == "_maintenance_rowid":
            continue
        item = row[key]
        if key == column:
            item = ["json", value] if item is not None else ["sql-null"]
        elif isinstance(item, bytes):
            item = ["blob", item.hex()]
        else:
            item = [type(item).__name__, item]
        content.append([key, item])
    return hashlib.sha256(canonical(content)).hexdigest()


def add_digest(total, item):
    # Order-independent multiset checksum plus an exact row count. It remains
    # comparable after VACUUM; repeated rows still contribute repeatedly.
    return f"{(int(total, 16) + int(item, 16)) % MODULUS:064x}"


def inventory(db):
    return {name: db.execute(f"SELECT COUNT(*) FROM {quote(name)}").fetchone()[0]
            for name in tables(db) if name != "json_bodies"}


def rowid_identities(db):
    """Small-column fingerprints protect importer cursors and equal-time ordering."""
    result = {}
    for table in tables(db):
        if table == "json_bodies":
            continue
        columns = db.execute(f"PRAGMA table_info({quote(table)})").fetchall()
        keys = [c[1] for c in sorted(columns, key=lambda c: c[5]) if c[5]]
        if not keys:
            keys = [c[1] for c in columns if c[1] not in ("payload", "body", "normalized", "claims", "value")]
        digest = hashlib.sha256()
        selection = ",".join(["rowid", *(quote(k) for k in keys)])
        for row in db.execute(f"SELECT {selection} FROM {quote(table)} ORDER BY rowid"):
            digest.update(canonical(list(row)))
            digest.update(b"\n")
        result[table] = digest.hexdigest()
    return result


def database_info(db):
    page_size = db.execute("PRAGMA page_size").fetchone()[0]
    pages = db.execute("PRAGMA page_count").fetchone()[0]
    free = db.execute("PRAGMA freelist_count").fetchone()[0]
    return {"page_size": page_size, "page_count": pages, "free_pages": free,
            "logical_bytes": pages * page_size, "live_page_bytes": (pages - free) * page_size}


def _journal(db):
    db.execute(f"""CREATE TABLE IF NOT EXISTS {_JOBS}(
        id INTEGER PRIMARY KEY,version TEXT NOT NULL,kind TEXT NOT NULL,
        started_at REAL NOT NULL,status TEXT NOT NULL,inventory TEXT NOT NULL)""")
    db.execute(f"""CREATE TABLE IF NOT EXISTS {_PROGRESS}(
        job INTEGER NOT NULL,table_name TEXT NOT NULL,column_name TEXT NOT NULL,
        mode TEXT NOT NULL,volatile TEXT NOT NULL,high_water INTEGER NOT NULL,
        last_rowid INTEGER NOT NULL,expected_rows INTEGER NOT NULL,
        verified_rows INTEGER NOT NULL DEFAULT 0,changed_rows INTEGER NOT NULL DEFAULT 0,
        before_bytes INTEGER NOT NULL DEFAULT 0,after_bytes INTEGER NOT NULL DEFAULT 0,
        semantic_hash TEXT NOT NULL,status TEXT NOT NULL,
        PRIMARY KEY(job,table_name))""")
    db.execute("""CREATE TABLE IF NOT EXISTS qh_storage_vacuum(
        job INTEGER PRIMARY KEY,counts TEXT NOT NULL,rowid_identities TEXT NOT NULL)""")


def _job(db, kind, selected, new_pass):
    previous = db.execute(f"SELECT * FROM {_JOBS} ORDER BY id DESC LIMIT 1").fetchone()
    if previous and not new_pass:
        if previous["version"] != VERSION or previous["kind"] != kind:
            raise ValueError("unsupported maintenance checkpoint")
        if previous["status"] == "vacuum_started":
            raise ValueError("resume interrupted VACUUM before compaction; rowids are not yet verified")
        return previous["id"]
    if previous and previous["status"] in ("running", "vacuum_started"):
        raise ValueError("resume the unfinished maintenance pass before starting another")
    cursor = db.execute(f"INSERT INTO {_JOBS}(version,kind,started_at,status,inventory) VALUES (?,?,?,'running',?)",
                        (VERSION, kind, time.time(), json.dumps(inventory(db), sort_keys=True)))
    job = cursor.lastrowid
    for table, column, mode, volatile in selected:
        high = db.execute(f"SELECT MAX(rowid) FROM {quote(table)}").fetchone()[0]
        high = high if high is not None else 0
        count = db.execute(f"SELECT COUNT(*) FROM {quote(table)} WHERE rowid<=?", (high,)).fetchone()[0]
        db.execute(f"""INSERT INTO {_PROGRESS}(job,table_name,column_name,mode,volatile,
            high_water,last_rowid,expected_rows,semantic_hash,status) VALUES (?,?,?,?,?,?,?, ?,?,'pending')""",
                   (job, table, column, mode, json.dumps(volatile), high, -(1 << 63), count, ZERO))
    return job


def progress(db, job):
    status = dict(db.execute(f"SELECT id,version,kind,status FROM {_JOBS} WHERE id=?", (job,)).fetchone())
    status["tables"] = [dict(r) for r in db.execute(f"SELECT * FROM {_PROGRESS} WHERE job=? ORDER BY table_name", (job,))]
    return status


def compact(path, *, batch_rows=64, batch_bytes=8 * 1024 * 1024,
            batch_seconds=0.5, max_batches=None, new_pass=False, catch_up=False, emit=None):
    if batch_rows < 1 or batch_bytes < 1 or batch_seconds <= 0:
        raise ValueError("batch limits must be positive")
    if max_batches is not None and max_batches < 1:
        raise ValueError("max_batches must be positive")
    db = connect(path)
    try:
        kind, selected = targets(db)
        with db:
            begin_batch(db, emit)
            _journal(db)
            job = _job(db, kind, selected, new_pass)
            codec = JSONStorage(db)
            if catch_up:
                status = db.execute(f"SELECT status FROM {_JOBS} WHERE id=?", (job,)).fetchone()[0]
                if status == "vacuumed":
                    raise ValueError("start a new pass after VACUUM; rowids may have changed")
                for table, column, mode, volatile in selected:
                    state = db.execute(f"SELECT * FROM {_PROGRESS} WHERE job=? AND table_name=?", (job, table)).fetchone()
                    if state is None:
                        raise ValueError("schema changed; finish this pass, then use --new-pass")
                    high = db.execute(f"SELECT MAX(rowid) FROM {quote(table)}").fetchone()[0] or 0
                    count = db.execute(f"SELECT COUNT(*) FROM {quote(table)} WHERE rowid<=?", (high,)).fetchone()[0]
                    if high > state["high_water"]:
                        db.execute(f"UPDATE {_PROGRESS} SET high_water=?,expected_rows=?,status='pending' WHERE job=? AND table_name=?",
                                   (high, count, job, table))
                db.execute(f"UPDATE {_JOBS} SET status='running' WHERE id=?", (job,))
        batches = 0
        for table, column, mode, volatile in selected:
            while True:
                with db:
                    begin_batch(db, emit)
                    state = db.execute(f"SELECT * FROM {_PROGRESS} WHERE job=? AND table_name=?", (job, table)).fetchone()
                    if state is None:
                        # An upgraded schema may append a new table after the
                        # pass began. It belongs to the next explicitly new pass.
                        break
                    if state["status"] == "complete":
                        break
                    ids = [r[0] for r in db.execute(f"SELECT rowid FROM {quote(table)} WHERE rowid>? AND rowid<=? ORDER BY rowid LIMIT ?",
                                                    (state["last_rowid"], state["high_water"], batch_rows))]
                    if not ids:
                        actual = db.execute(f"SELECT COUNT(*) FROM {quote(table)} WHERE rowid<=?", (state["high_water"],)).fetchone()[0]
                        if actual != state["expected_rows"] or state["verified_rows"] != actual:
                            raise ValueError(f"row count changed during maintenance: {table}")
                        db.execute(f"UPDATE {_PROGRESS} SET status='complete' WHERE job=? AND table_name=?", (job, table))
                        break
                    verifier = JSONStorage(db, read_only=True)  # Never trust the packing cache.
                    before_sum = after_sum = state["semantic_hash"]
                    read_bytes = written_bytes = changed = verified = 0
                    started = time.monotonic()
                    for rowid in ids:
                        sql = f"SELECT rowid AS _maintenance_rowid,* FROM {quote(table)} WHERE rowid=?"
                        row = db.execute(sql, (rowid,)).fetchone()
                        value = unpack(verifier, row[column], mode)
                        before = row_digest(row, column, value)
                        replacement = encode(codec, row[column], value, mode, volatile)
                        if replacement != row[column]:
                            db.execute(f"UPDATE {quote(table)} SET {quote(column)}=? WHERE rowid=?", (replacement, rowid))
                            changed += 1
                            stored = db.execute(sql, (rowid,)).fetchone()
                            restored = unpack(verifier, stored[column], mode)
                            after = row_digest(stored, column, restored)
                        else:
                            # Existing references still receive a cold body/hash
                            # check and full semantic/identity digest above. No
                            # UPDATE occurred while we hold the writer lock, so
                            # decoding the same immutable row twice adds no proof.
                            stored, after = row, before
                        if before != after:
                            raise ValueError(f"semantic verification failed: {table} row {rowid}")
                        before_sum, after_sum = add_digest(before_sum, before), add_digest(after_sum, after)
                        verified += 1
                        read_bytes += value_size(row[column])
                        written_bytes += value_size(stored[column])
                        # Limits are soft for a single row, and never split a JSON
                        # value or leave bodies outside its committing transaction.
                        if read_bytes >= batch_bytes or time.monotonic() - started >= batch_seconds:
                            break
                    if before_sum != after_sum:
                        raise ValueError("batch semantic checksum mismatch")
                    db.execute(f"""UPDATE {_PROGRESS} SET last_rowid=?,verified_rows=verified_rows+?,
                        changed_rows=changed_rows+?,before_bytes=before_bytes+?,after_bytes=after_bytes+?,
                        semantic_hash=?,status='running' WHERE job=? AND table_name=?""",
                               (rowid, verified, changed, read_bytes, written_bytes, after_sum, job, table))
                batches += 1
                if emit:
                    emit({"event": "batch_committed", "job": job, "table": table,
                          "last_rowid": rowid, "high_water": state["high_water"],
                          "verified_rows": state["verified_rows"] + verified,
                          "expected_rows": state["expected_rows"], "changed_rows": changed,
                          "elapsed_seconds": round(time.monotonic() - started, 3),
                          "semantic_hash": after_sum})
                if max_batches is not None and batches >= max_batches:
                    return progress(db, job)
                # Give the daily writer an opportunity between bounded batches.
                time.sleep(0.02)
        with db:
            pending = db.execute(f"SELECT COUNT(*) FROM {_PROGRESS} WHERE job=? AND status!='complete'", (job,)).fetchone()[0]
            if not pending:
                db.execute(f"UPDATE {_JOBS} SET status='complete' WHERE id=? AND status='running'", (job,))
        return progress(db, job)
    finally:
        db.close()


def verify(path, emit=None):
    """Read-only full semantic verification against the most recent completed pass.

    New appended rows beyond the recorded high-water mark are outside that pass.
    A mismatch fails visibly; no evidence or baseline is repaired automatically.
    """
    db = connect(path, read_only=True)
    try:
        job = db.execute(f"SELECT * FROM {_JOBS} ORDER BY id DESC LIMIT 1").fetchone()
        if job is None or job["status"] not in ("complete", "vacuumed", "vacuum_started"):
            raise ValueError("complete a compaction pass before verification")
        result = []
        codec = JSONStorage(db, read_only=True)
        for state in db.execute(f"SELECT * FROM {_PROGRESS} WHERE job=? ORDER BY table_name", (job["id"],)).fetchall():
            total, count = ZERO, 0
            if job["status"] in ("vacuumed", "vacuum_started"):
                rows = db.execute(f"SELECT COUNT(*) FROM {quote(state['table_name'])}").fetchone()[0]
                if rows != state["verified_rows"]:
                    raise ValueError("rows were appended after VACUUM; use a new compaction pass for a new full baseline")
                query, params = f"SELECT rowid AS _maintenance_rowid,* FROM {quote(state['table_name'])} ORDER BY rowid", ()
            else:
                query, params = f"SELECT rowid AS _maintenance_rowid,* FROM {quote(state['table_name'])} WHERE rowid<=? ORDER BY rowid", (state["high_water"],)
            for row in db.execute(query, params):
                total = add_digest(total, row_digest(row, state["column_name"], unpack(codec, row[state["column_name"]], state["mode"])))
                count += 1
            if count != state["verified_rows"] or total != state["semantic_hash"]:
                raise ValueError(f"semantic baseline mismatch: {state['table_name']}")
            item = {"table": state["table_name"], "verified_rows": count, "semantic_hash": total}
            result.append(item)
            if emit:
                emit(item)
        return {"job": job["id"], "verified": True, "tables": result}
    finally:
        db.close()


def collect_unused_bodies(path, *, writers_stopped=False, emit=None):
    """One-time sweep after converting legacy report caches.

    All application roots and their complete reference closure are marked under
    one write reservation. Only unreachable immutable bodies are removed. New
    reports are inline blobs, so routine archive rotation needs no global sweep.
    """
    if not writers_stopped:
        raise ValueError("body collection requires explicitly stopped module writers")
    db = connect(path)
    try:
        _, selected = targets(db)
        with db:
            begin_batch(db, emit)
            db.execute("CREATE TEMP TABLE live_bodies(hash TEXT PRIMARY KEY,visited INTEGER NOT NULL DEFAULT 0) WITHOUT ROWID")
            db.execute("CREATE INDEX temp.live_bodies_pending ON live_bodies(visited)")
            for table, column, _, _ in selected:
                db.execute(f"""INSERT OR IGNORE INTO live_bodies(hash)
                    SELECT CAST(substr({quote(column)},10) AS TEXT) FROM {quote(table)}
                    WHERE typeof({quote(column)})='blob' AND substr({quote(column)},1,9)=?""", (PREFIX,))
            count = 0
            while True:
                pending = db.execute("""SELECT l.hash,b.body FROM live_bodies l LEFT JOIN json_bodies b ON b.hash=l.hash
                    WHERE l.visited=0 LIMIT 512""").fetchall()
                if not pending:
                    break
                for key, body in pending:
                    if body is None:
                        raise ValueError("reachable JSON storage body is missing")
                    encoded = zlib.decompress(body)
                    if hashlib.sha256(encoded).hexdigest() != key:
                        raise ValueError("reachable JSON storage body hash mismatch")
                    envelope = json.loads(encoded)
                    if not isinstance(envelope, dict) or envelope.get("version") != 1 or not isinstance(envelope.get("refs"), list):
                        raise ValueError("unsupported JSON storage body")
                    children = []
                    for ref in envelope["refs"]:
                        if not isinstance(ref, list) or len(ref) != 2:
                            raise ValueError("invalid JSON storage reference")
                        children.append(ref[1])
                    if "base" in envelope:
                        children.append(envelope["base"])
                    if any(not isinstance(child, str) or len(child) != 64 or any(c not in "0123456789abcdef" for c in child) for child in children):
                        raise ValueError("invalid JSON storage reference")
                    db.executemany("INSERT OR IGNORE INTO live_bodies(hash) VALUES (?)", ((child,) for child in children))
                    db.execute("UPDATE live_bodies SET visited=1 WHERE hash=?", (key,))
                    count += 1
                if emit and count % 8192 < 512:
                    emit({"event": "body_marking", "visited": count})
            before = db.execute("SELECT COUNT(*) FROM json_bodies").fetchone()[0]
            db.execute("DELETE FROM json_bodies WHERE hash NOT IN (SELECT hash FROM live_bodies)")
            after = db.execute("SELECT COUNT(*) FROM json_bodies").fetchone()[0]
        return {"event": "unused_bodies_removed", "before": before, "after": after, "removed": before - after}
    finally:
        db.close()


def vacuum(path, *, writers_stopped=False, emit=None):
    if not writers_stopped:
        raise ValueError("VACUUM requires explicitly stopped module writers")
    db = connect(path)
    try:
        job = db.execute(f"SELECT * FROM {_JOBS} ORDER BY id DESC LIMIT 1").fetchone()
        if job is None or job["status"] not in ("complete", "vacuumed", "vacuum_started"):
            raise ValueError("finish compaction before reclaiming space")
        for state in db.execute(f"SELECT * FROM {_PROGRESS} WHERE job=?", (job["id"],)).fetchall():
            count = db.execute(f"SELECT COUNT(*) FROM {quote(state['table_name'])}").fetchone()[0]
            if count != state["verified_rows"]:
                raise ValueError("writers appended rows; stop them and run compact --catch-up before VACUUM")
        checkpoint = db.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()
        if checkpoint[0]:
            raise ValueError("database is busy; stop readers/writers before VACUUM")
        before, counts = database_info(db), inventory(db)
        identities = rowid_identities(db)
        previous = db.execute("SELECT * FROM qh_storage_vacuum WHERE job=?", (job["id"],)).fetchone()
        if previous:
            if counts != json.loads(previous["counts"]) or identities != json.loads(previous["rowid_identities"]):
                raise ValueError("post-VACUUM row identity changed; keep writers stopped and inspect importer cursors")
        if job["status"] == "vacuumed":
            return {"event": "already_vacuumed", "job": job["id"], "after": before, "rows_preserved": counts}
        required = before["live_page_bytes"] * 2 + 64 * 1024 * 1024
        available = shutil.disk_usage(Path(path).resolve().parent).free
        if available < required:
            raise ValueError(f"insufficient temporary disk space: need {required}, available {available}")
        if emit:
            emit({"event": "vacuum_started", "before": before, "temporary_space_required": required})
        with db:
            db.execute("INSERT OR IGNORE INTO qh_storage_vacuum VALUES (?,?,?)",
                       (job["id"], json.dumps(counts, sort_keys=True), json.dumps(identities, sort_keys=True)))
            db.execute(f"UPDATE {_JOBS} SET status='vacuum_started' WHERE id=?", (job["id"],))
        # SQLite's transaction/journal provides interruption recovery. No backup
        # copy or evidence deletion is used. VACUUM owns its temporary work file.
        db.execute("PRAGMA auto_vacuum=INCREMENTAL")
        db.execute("VACUUM")
        if emit:
            emit({"event": "vacuum_sql_complete", "job": job["id"]})
        if inventory(db) != counts:
            raise ValueError("row counts changed during VACUUM")
        if rowid_identities(db) != identities:
            raise ValueError("VACUUM changed rowid ordering; keep writers stopped and inspect importer cursors")
        check = db.execute("PRAGMA quick_check").fetchall()
        if len(check) != 1 or check[0][0] != "ok":
            raise ValueError("SQLite integrity check failed after VACUUM")
        with db:
            db.execute(f"UPDATE {_JOBS} SET status='vacuumed' WHERE id=?", (job["id"],))
        checkpoint = db.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()
        if checkpoint[0]:
            raise ValueError("space reclaim checkpoint was blocked")
        return {"event": "vacuum_complete", "job": job["id"], "before": before,
                "after": database_info(db), "rows_preserved": counts}
    finally:
        db.close()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("inspect", "compact", "verify", "vacuum", "collect-bodies"))
    parser.add_argument("--db", required=True)
    parser.add_argument("--batch-rows", type=int, default=64)
    parser.add_argument("--batch-mib", type=float, default=8)
    parser.add_argument("--batch-seconds", type=float, default=0.5)
    parser.add_argument("--max-batches", type=int)
    parser.add_argument("--new-pass", action="store_true")
    parser.add_argument("--catch-up", action="store_true", help="Extend this pass to appended rows before final offline VACUUM")
    parser.add_argument("--writers-stopped", action="store_true")
    args = parser.parse_args(argv)
    def output(value):
        print(json.dumps(value, ensure_ascii=False, allow_nan=False), flush=True)
    try:
        if args.command == "compact":
            result = compact(args.db, batch_rows=args.batch_rows, batch_bytes=int(args.batch_mib * 1024 * 1024),
                             batch_seconds=args.batch_seconds, max_batches=args.max_batches,
                             new_pass=args.new_pass, catch_up=args.catch_up, emit=output)
        elif args.command == "verify":
            result = verify(args.db, emit=output)
        elif args.command == "vacuum":
            result = vacuum(args.db, writers_stopped=args.writers_stopped, emit=output)
        elif args.command == "collect-bodies":
            result = collect_unused_bodies(args.db, writers_stopped=args.writers_stopped, emit=output)
        else:
            db = connect(args.db, read_only=True)
            try:
                kind, selected = targets(db)
                result = {"kind": kind, "database": database_info(db), "targets": [t[0] for t in selected]}
                if db.execute("SELECT 1 FROM sqlite_master WHERE name=?", (_JOBS,)).fetchone():
                    job = db.execute(f"SELECT MAX(id) FROM {_JOBS}").fetchone()[0]
                    if job:
                        result["maintenance"] = progress(db, job)
            finally:
                db.close()
        output(result)
        return 0
    except KeyboardInterrupt:
        output({"status": "interrupted", "message": "Committed batches are preserved; run compact again to resume."})
        return 130
    except (ValueError, sqlite3.Error, zlib.error) as error:
        output({"status": "failed", "error": str(error), "resumable": True})
        return 1


if __name__ == "__main__":
    sys.exit(main())
