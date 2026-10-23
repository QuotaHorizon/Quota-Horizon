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
_FAMILY = b"\x00QHJSON"
_THRESHOLD = 2048
_CACHE_BYTES = 16 * 1024 * 1024


def _packed(value):
    return json.dumps(value, ensure_ascii=False, allow_nan=False, sort_keys=True, separators=(",", ":"))


class JSONStorage:
    def __init__(self, db, read_only=False):
        self.db = db
        self.cache = OrderedDict()
        self.cache_bytes = 0
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
        return self._store({"version": 1, "value": template, "refs": refs},
                           text if text is not None else _packed(value))

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
            return text
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
