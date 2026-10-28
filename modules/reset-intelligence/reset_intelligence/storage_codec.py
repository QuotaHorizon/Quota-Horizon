"""Append-only, content-addressed JSON bodies with legacy SQL row compatibility.

Sampling rows keep their original IDs and clocks. Large fields refer to a compressed
body; repeated subtrees share immutable bodies. JSON references live in a separate
path manifest, so ordinary source objects cannot impersonate storage instructions.
"""
from __future__ import annotations

from collections import OrderedDict
import hashlib
import json
import sqlite3
import zlib


_PREFIX = b"\x00QHJSON2\x00"
_INLINE = b"\x00QHJSON3\x00"
RETIRED_RAW = b"\x00QHRAWEXPIRED1\x00"
_FAMILY = b"\x00QHJSON"
_THRESHOLD = 2048
_CACHE_BYTES = 16 * 1024 * 1024
_INTERN_CACHE_ENTRIES = 4096


def _packed(value):
    return json.dumps(value, ensure_ascii=False, allow_nan=False, sort_keys=True, separators=(",", ":"))


def pack_inline(value):
    """Compress small records and disposable reports without allocating shared bodies."""
    text = _packed(value)
    raw = text.encode("utf-8")
    compressed = _INLINE + zlib.compress(raw)
    return compressed if len(compressed) + 16 < len(raw) else text


def retain_raw_cache(db, now, *, maximum_rows=256):
    """Keep 48h of full responses, daily snapshots for 30d, and latest inputs.

    The content hash and every normalized receipt remain permanent. An explicit
    binary marker retires only the diagnostic response body, never a forecast,
    ballot, coverage clock or event correction. Blobs are self-contained so
    retirement releases pages without a shared-body garbage-collection pass.
    """
    columns = {r[1] for r in db.execute("PRAGMA table_info(observations)")}
    if {"source", "normalized"} <= columns:
        source, success = "source", "normalized"
    elif {"provider", "claims"} <= columns:
        source, success = "provider", "claims"
    else:
        raise ValueError("unrecognized raw response cache")
    if maximum_rows < 1:
        raise ValueError("cache batch limit must be positive")
    # Names above are fixed schema identifiers, never caller-controlled SQL.
    with db:
        cursor = db.execute(f"""UPDATE payloads SET body=? WHERE hash IN (
            SELECT hash FROM payloads WHERE body!=? AND hash IN (
                SELECT payload_hash FROM observations WHERE observed_at<?
            ) AND hash NOT IN (
                SELECT payload_hash FROM observations WHERE observed_at>=? AND payload_hash IS NOT NULL
                UNION SELECT payload_hash FROM (
                    SELECT payload_hash,ROW_NUMBER() OVER(PARTITION BY {source},CAST(observed_at/86400 AS INTEGER)
                        ORDER BY observed_at DESC,id DESC) rank
                    FROM observations WHERE observed_at>=? AND payload_hash IS NOT NULL
                ) WHERE rank=1
                UNION SELECT payload_hash FROM (
                    SELECT payload_hash,ROW_NUMBER() OVER(PARTITION BY {source}
                        ORDER BY observed_at DESC,id DESC) rank FROM observations WHERE payload_hash IS NOT NULL
                ) WHERE rank=1
                UNION SELECT payload_hash FROM (
                    SELECT payload_hash,ROW_NUMBER() OVER(PARTITION BY {source}
                        ORDER BY observed_at DESC,id DESC) rank FROM observations
                        WHERE payload_hash IS NOT NULL AND {success} IS NOT NULL
                ) WHERE rank=1
            ) LIMIT ?
        )""", (RETIRED_RAW, RETIRED_RAW, now - 2 * 86400, now - 2 * 86400, now - 30 * 86400, maximum_rows))
    return cursor.rowcount


def maintain_raw_cache(db, now):
    """Check at most hourly; catch up older installations in bounded batches."""
    db.execute("CREATE TABLE IF NOT EXISTS qh_storage_cache(key TEXT PRIMARY KEY,at REAL NOT NULL)")
    previous = db.execute("SELECT at FROM qh_storage_cache WHERE key='raw_retention'").fetchone()
    if previous and now - previous[0] < 3600:
        return 0
    removed = retain_raw_cache(db, now, maximum_rows=1024)
    if removed < 1024:
        with db:
            db.execute("INSERT OR REPLACE INTO qh_storage_cache VALUES ('raw_retention',?)", (now,))
    reclaim_pages(db)
    return removed


def reclaim_pages(db, *, minimum_bytes=16 * 1024 * 1024, maximum_pages=256):
    if db.in_transaction or db.execute("PRAGMA auto_vacuum").fetchone()[0] != 2:
        return 0
    page_size = db.execute("PRAGMA page_size").fetchone()[0]
    before = db.execute("PRAGMA freelist_count").fetchone()[0]
    if before * page_size < minimum_bytes:
        return 0
    db.execute(f"PRAGMA incremental_vacuum({int(maximum_pages)})").fetchall()
    after = db.execute("PRAGMA freelist_count").fetchone()[0]
    return (before - after) * page_size


class JSONStorage:
    def __init__(self, db, read_only=False):
        self.db = db
        self.cache = OrderedDict()
        self.cache_bytes = 0
        # Content digests, not another copy of large evidence trees. Repeated
        # report subtrees can reuse a persisted body without encoding it again.
        self.intern_cache = OrderedDict()
        if not read_only:
            db.execute("CREATE TABLE IF NOT EXISTS json_bodies(hash TEXT PRIMARY KEY,body BLOB NOT NULL)")

    def _remember(self, key, text):
        size = len(text.encode("utf-8"))
        if size > _CACHE_BYTES:
            return
        previous = self.cache.pop(key, None)
        if previous:
            self.cache_bytes -= previous[1]
        self.cache[key] = text, size
        self.cache_bytes += size
        while self.cache_bytes > _CACHE_BYTES:
            _, (_, old_size) = self.cache.popitem(last=False)
            self.cache_bytes -= old_size

    def _intern(self, value, text=None):
        text = text if text is not None else _packed(value)
        content_hash = hashlib.sha256(text.encode("utf-8")).hexdigest()
        existing = self.intern_cache.get(content_hash)
        if existing is not None and self.db.execute("SELECT 1 FROM json_bodies WHERE hash=?", (existing,)).fetchone():
            # An earlier transaction may have rolled back. Only the actual body
            # table establishes persistence; a cache entry alone never does.
            self.intern_cache.move_to_end(content_hash)
            self._remember(existing, text)
            return existing
        refs = []
        if isinstance(value, (dict, list, tuple)):
            is_object = isinstance(value, dict)
            template = {} if is_object else []
            for key, child in (value.items() if is_object else enumerate(value)):
                child_text = _packed(child)
                if len(child_text.encode("utf-8")) >= _THRESHOLD:
                    refs.append([key, self._intern(child, child_text)])
                    child = None
                if is_object:
                    template[key] = child
                else:
                    template.append(child)
        else:
            template = value
        key = self._store({"version": 1, "value": template, "refs": refs}, text)
        self.intern_cache[content_hash] = key
        self.intern_cache.move_to_end(content_hash)
        while len(self.intern_cache) > _INTERN_CACHE_ENTRIES:
            self.intern_cache.popitem(last=False)
        return key

    def _store(self, envelope, text):
        encoded = _packed(envelope).encode("utf-8")
        key = hashlib.sha256(encoded).hexdigest()
        # A previous caller may have rolled back after packing. A memory cache
        # proves decoding, not persistence in the current SQL transaction.
        if key not in self.cache or not self.db.execute("SELECT 1 FROM json_bodies WHERE hash=?", (key,)).fetchone():
            self.db.execute("INSERT OR IGNORE INTO json_bodies(hash,body) VALUES (?,?)", (key, zlib.compress(encoded)))
        self._remember(key, text)
        return key

    def pack_json(self, value, volatile_fields=()):
        text = _packed(value)
        size = len(text.encode("utf-8"))
        if isinstance(value, dict) and volatile_fields and size >= 512:
            value = json.loads(text)
            overrides = {k: value[k] for k in volatile_fields if k in value}
            stable = {k: v for k, v in value.items() if k not in overrides}
            base = self._intern(stable)
            key = self._store({"version": 1, "value": overrides, "refs": [], "base": base}, text)
            return _PREFIX + key.encode("ascii")
        if size < _THRESHOLD:
            return pack_inline(value)
        return _PREFIX + self._intern(json.loads(text), text).encode("ascii")

    def _body(self, key, ancestors):
        if not isinstance(key, str) or len(key) != 64 or any(c not in "0123456789abcdef" for c in key):
            raise ValueError("invalid JSON storage body key")
        if key in ancestors:
            raise ValueError("cyclic JSON storage reference")
        if key in self.cache:
            self.cache.move_to_end(key)
            return self.cache[key][0]
        cursor = self.db.cursor()
        # Bodies are binary evidence, not sampling rows; avoid recursive row decoding.
        cursor.row_factory = None
        try:
            row = cursor.execute("SELECT body FROM json_bodies WHERE hash=?", (key,)).fetchone()
        except sqlite3.OperationalError as error:
            raise ValueError("JSON storage body table is missing") from error
        if row is None:
            raise ValueError("JSON storage body is missing")
        try:
            encoded = zlib.decompress(row[0])
        except (TypeError, zlib.error) as error:
            raise ValueError("invalid compressed JSON storage body") from error
        if hashlib.sha256(encoded).hexdigest() != key:
            raise ValueError("JSON storage body hash mismatch")
        envelope = json.loads(encoded)
        if (not isinstance(envelope, dict) or set(envelope) not in
                ({"version", "value", "refs"}, {"version", "value", "refs", "base"})
                or type(envelope["version"]) is not int or envelope["version"] != 1
                or not isinstance(envelope["refs"], list)):
            raise ValueError("unsupported JSON storage body")
        value = envelope["value"]
        if "base" in envelope:
            if not isinstance(value, dict) or not isinstance(envelope["base"], str):
                raise ValueError("invalid JSON storage overlay")
            base = json.loads(self._body(envelope["base"], ancestors | {key}))
            if not isinstance(base, dict):
                raise ValueError("invalid JSON storage overlay base")
            value = {**base, **value}
        seen = set()
        for reference in envelope["refs"]:
            if not isinstance(reference, list) or len(reference) != 2:
                raise ValueError("invalid JSON storage reference manifest")
            path, child_key = reference
            valid_path = ((isinstance(value, dict) and isinstance(path, str) and path in value)
                          or (isinstance(value, list) and type(path) is int and 0 <= path < len(value)))
            if not valid_path or path in seen or not isinstance(child_key, str) or value[path] is not None:
                raise ValueError("invalid JSON storage reference manifest")
            seen.add(path)
            value[path] = json.loads(self._body(child_key, ancestors | {key}))
        text = _packed(value)
        self._remember(key, text)
        return text

    def expand_json(self, value):
        if isinstance(value, bytes) and value.startswith(_FAMILY):
            if value.startswith(_INLINE):
                try:
                    return zlib.decompress(value[len(_INLINE):]).decode("utf-8")
                except (zlib.error, UnicodeDecodeError) as error:
                    raise ValueError("invalid compressed inline JSON") from error
            if not value.startswith(_PREFIX) or len(value) != len(_PREFIX) + 64:
                raise ValueError("unsupported JSON storage reference")
            try:
                key = value[len(_PREFIX):].decode("ascii")
            except UnicodeDecodeError as error:
                raise ValueError("invalid JSON storage reference") from error
            if any(c not in "0123456789abcdef" for c in key):
                raise ValueError("invalid JSON storage reference")
            return self._body(key, set())
        return value

    def row_factory(self, cursor, row):
        # Match the binary storage envelope, not column names or JSON keys. Raw
        # source JSON and ordinary BLOBs remain unchanged, including aliases.
        return sqlite3.Row(cursor, tuple(self.expand_json(value) for value in row))


def configure_storage(db, read_only=False):
    codec = JSONStorage(db, read_only)
    db.row_factory = codec.row_factory
    db.create_function("qh_json", 1, codec.expand_json, deterministic=True)
    return codec
