"""Bound the disposable report archive while keeping the prediction ledger intact.

The live JSON report is still published on every evaluation. Saved reports retain
the latest publication in each UTC hour for seven days, then each UTC day, plus
the first and last report of every implementation/policy revision. Forecasts,
ballots, coverage receipts, trials and outcome revisions are retained. Expired
unfrozen auxiliary computations have a separate cache policy below.
"""
from __future__ import annotations

import json
import zlib

from .contracts import digest

HOUR = 3600
DAY = 24 * HOUR
HOURLY_DAYS = 7


def report_identity(store, raw):
    # Old shared reports can be megabytes after expansion. The identity lives
    # in the small root manifest; only its policy subtree needs decoding.
    if isinstance(raw, bytes) and raw.startswith(b"\0QHJSON2\0"):
        key = raw[9:].decode("ascii")
        cursor = store.db.cursor()
        cursor.row_factory = None
        body = cursor.execute("SELECT body FROM json_bodies WHERE hash=?", (key,)).fetchone()
        if body is None:
            raise ValueError("report storage body is missing")
        import hashlib
        encoded = zlib.decompress(body[0])
        if hashlib.sha256(encoded).hexdigest() != key:
            raise ValueError("report storage body hash mismatch")
        envelope = json.loads(encoded)
        value = envelope.get("value")
        if isinstance(value, dict) and not envelope.get("base"):
            for field, child in envelope.get("refs", []):
                if field in ("policy", "implementation_hash", "module_version"):
                    value[field] = json.loads(store.json_storage.expand_json(b"\0QHJSON2\0" + child.encode("ascii")))
        else:
            value = json.loads(store.json_storage.expand_json(raw))
    else:
        raw = store.json_storage.expand_json(raw)
        value = json.loads(zlib.decompress(raw) if isinstance(raw, bytes) else raw)
    return digest([value.get("implementation_hash"), value.get("module_version"), value.get("policy", {}).get("hash")])


def retain_reports(store, now):
    """Idempotent UTC roll-up, including a one-time legacy identity backfill."""
    from .store import retention_ready
    if not retention_ready(store.db):
        return 0
    db = store.db
    with db:
        db.execute("CREATE TABLE IF NOT EXISTS report_versions(signature TEXT PRIMARY KEY,first_id INTEGER NOT NULL,last_id INTEGER NOT NULL)")
        last = store.get("report_archive_cursor") or 0
        cursor = db.cursor()
        cursor.row_factory = None
        rows = cursor.execute("SELECT id,payload FROM reports WHERE id>? ORDER BY id", (last,)).fetchall()
        for id, raw in rows:
            signature = report_identity(store, raw)
            db.execute("""INSERT INTO report_versions VALUES (?,?,?) ON CONFLICT(signature)
                DO UPDATE SET last_id=excluded.last_id""", (signature, id, id))
        if rows:
            db.execute("INSERT OR REPLACE INTO metadata VALUES ('report_archive_cursor',?)", (json.dumps(rows[-1][0]),))
        before = db.total_changes
        db.execute("""DELETE FROM reports WHERE at<=? AND id NOT IN (
            SELECT id FROM (
                SELECT id,ROW_NUMBER() OVER (PARTITION BY
                    CASE WHEN at>=? THEN 'hour' ELSE 'day' END,
                    CAST(at / CASE WHEN at>=? THEN 3600 ELSE 86400 END AS INTEGER)
                    ORDER BY at DESC,id DESC) AS rank FROM reports WHERE at<=?
            ) WHERE rank=1
            UNION SELECT first_id FROM report_versions
            UNION SELECT last_id FROM report_versions
            UNION SELECT MIN(id) FROM reports
        )""", (now, now - HOURLY_DAYS * DAY, now - HOURLY_DAYS * DAY, now))
        removed = db.total_changes - before
    return removed


def reclaim_cache_pages(store, *, minimum_bytes=16 * 1024 * 1024, maximum_pages=256):
    """Small incremental reclamation, never a periodic full database rewrite."""
    from .store import reclaim_pages
    return reclaim_pages(store.db, minimum_bytes=minimum_bytes, maximum_pages=maximum_pages)


def _pin_trial_forecasts(store):
    """Index immutable trial references in small resumable batches."""
    db = store.db
    db.execute("CREATE TABLE IF NOT EXISTS forecast_pins(id TEXT PRIMARY KEY)")
    for table in ("trials", "preference_trials"):
        key = "forecast_pins:" + table
        last = store.get(key) or 0
        while True:
            rows = db.execute(f"SELECT rowid,payload FROM {table} WHERE rowid>? ORDER BY rowid LIMIT 1024", (last,)).fetchall()
            if not rows:
                break
            pins = set()
            for rowid, text in rows:
                value = json.loads(text)
                if value.get("forecast_id"):
                    pins.add(value["forecast_id"])
                pins.update(c["forecast_id"] for c in value.get("comparisons", []) if c.get("forecast_id"))
            with db:
                db.executemany("INSERT OR IGNORE INTO forecast_pins VALUES (?)", ((id,) for id in pins))
                last = rows[-1][0]
                db.execute("INSERT OR REPLACE INTO metadata VALUES (?,?)", (key, json.dumps(last)))


def retain_auxiliary_forecasts(store, now, *, maximum_rows=2048):
    """Retire only expired, unfrozen shadow/adapted intermediates after 48h.

    Published model/native/community forecasts remain complete. Every formal or
    community trial reference is pinned, including missing/excluded trials that
    may be revised later. Latest series and version endpoints remain readable.
    IDs and first receipt clocks survive for deduplication and cumulative counts.
    """
    grid = store.get("last_grid")
    if grid is None or grid < now - 2 * DAY:
        return 0  # Catch up hourly freezing before considering old intermediates.
    if maximum_rows < 1:
        raise ValueError("auxiliary batch limit must be positive")
    _pin_trial_forecasts(store)
    db = store.db
    with db:
        # Take a fresh write reservation before selecting deletable rows. A
        # concurrent capture can append new trials while the pin index is built.
        db.execute("BEGIN IMMEDIATE")
        for table in ("trials", "preference_trials"):
            high = db.execute(f"SELECT MAX(rowid) FROM {table}").fetchone()[0] or 0
            if high > (store.get("forecast_pins:" + table) or 0):
                return 0
        db.execute("CREATE TEMP TABLE IF NOT EXISTS auxiliary_expiry(id TEXT PRIMARY KEY) WITHOUT ROWID")
        db.execute("DELETE FROM auxiliary_expiry")
        db.execute("""INSERT INTO auxiliary_expiry
            SELECT id FROM forecasts WHERE track IN ('shadow','adapted')
                AND available_at<? AND expires_at<=?
                AND id NOT IN (SELECT id FROM forecast_pins)
                AND id NOT IN (
                    SELECT id FROM (SELECT id,
                        ROW_NUMBER() OVER (PARTITION BY source,version,target,track ORDER BY origin,available_at,id) first,
                        ROW_NUMBER() OVER (PARTITION BY source,version,target,track ORDER BY origin DESC,available_at DESC,id DESC) last
                        FROM forecasts WHERE track IN ('shadow','adapted')) WHERE first=1 OR last=1
                ) ORDER BY available_at,id LIMIT ?""", (now - 2 * DAY, now, maximum_rows))
        count = db.execute("SELECT COUNT(*) FROM auxiliary_expiry").fetchone()[0]
        db.execute("INSERT OR IGNORE INTO retired_forecasts SELECT id,available_at FROM forecasts WHERE id IN (SELECT id FROM auxiliary_expiry)")
        db.execute("DELETE FROM forecasts WHERE id IN (SELECT id FROM auxiliary_expiry)")
        db.execute("DELETE FROM auxiliary_expiry")
    return count


def maintain_auxiliary_forecasts(store, now):
    from .store import retention_ready
    if not retention_ready(store.db):
        return 0
    previous = store.get("auxiliary_retention_at")
    if previous is not None and now - previous < HOUR:
        return 0
    removed = retain_auxiliary_forecasts(store, now)
    if removed < 2048:
        store.put("auxiliary_retention_at", now)
    return removed
