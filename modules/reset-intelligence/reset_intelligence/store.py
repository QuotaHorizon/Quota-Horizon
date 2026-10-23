"""Append-only source revisions and issued forecasts, in a module-owned database."""
from __future__ import annotations

import json
from pathlib import Path
import sqlite3
import zlib

from .contracts import Observation, digest
from .storage_codec import configure_storage


_COMPRESSED_JSON = b"QHJSON\x01ZLIB\x00"


def _json_text(value: object) -> str:
    # These columns are also read directly by the observer and replay tools.
    # Keep them ordinary JSON, without the per-row whitespace/ASCII overhead.
    return json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(",", ":"))


def _payload_body(value: object) -> str | bytes:
    text = _json_text(value)
    raw = text.encode("utf-8")
    compressed = _COMPRESSED_JSON + zlib.compress(raw)
    # Tiny payloads cost less as text; the marker identifies compressed rows
    # without changing content hashes or rewriting any historical evidence.
    return compressed if len(compressed) < len(raw) else text


def _read_payload(body: str | bytes) -> object:
    if isinstance(body, bytes) and body.startswith(_COMPRESSED_JSON):
        try:
            body = zlib.decompress(body[len(_COMPRESSED_JSON):])
        except zlib.error as error:
            raise ValueError("invalid compressed source payload") from error
    return json.loads(body)


class Store:
    def __init__(self, path: str | Path, read_only: bool = False):
        self.path = Path(path)
        if read_only:
            self.db = sqlite3.connect(self.path.resolve().as_uri() + "?mode=ro", uri=True, timeout=30)
            self.json_storage = configure_storage(self.db, read_only=True)
            return
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.db = sqlite3.connect(self.path, timeout=30)
        self.db.row_factory = sqlite3.Row
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.executescript("""
          CREATE TABLE IF NOT EXISTS payloads(hash TEXT PRIMARY KEY, body BLOB NOT NULL);
          CREATE TABLE IF NOT EXISTS observations(
            id INTEGER PRIMARY KEY, source TEXT NOT NULL, observed_at REAL NOT NULL,
            generated_at REAL, payload_hash TEXT, normalized TEXT, error TEXT,
            FOREIGN KEY(payload_hash) REFERENCES payloads(hash));
          CREATE INDEX IF NOT EXISTS observations_time ON observations(source, observed_at);
          CREATE INDEX IF NOT EXISTS observations_audit ON observations(
            source, observed_at, (normalized IS NOT NULL), (error IS NOT NULL));
          CREATE TABLE IF NOT EXISTS forecasts(
            id TEXT PRIMARY KEY, issued_at REAL NOT NULL, target TEXT NOT NULL,
            model_version TEXT NOT NULL, payload TEXT NOT NULL);
          CREATE INDEX IF NOT EXISTS forecasts_time ON forecasts(issued_at);
          CREATE TABLE IF NOT EXISTS source_config(
            hash TEXT PRIMARY KEY, first_seen_at REAL NOT NULL, payload TEXT NOT NULL);
        """)
        self.json_storage = configure_storage(self.db)

    def close(self):
        self.db.close()

    def pack_json(self, value, volatile_fields=()):
        return self.json_storage.pack_json(value, volatile_fields)

    def save(self, source: str, at: float, raw: object | None, observation: Observation | None,
             error: str | None = None) -> None:
        with self.db:
            h = digest(raw) if raw is not None else None
            if raw is not None:
                self.db.execute("INSERT OR IGNORE INTO payloads VALUES (?,?)", (h, _payload_body(raw)))
            self.db.execute("INSERT INTO observations(source,observed_at,generated_at,payload_hash,normalized,error) VALUES (?,?,?,?,?,?)",
                            (source, at, observation.generated_at if observation else None, h,
                             self.pack_json(observation.to_dict(), ("observed_at", "generated_at")) if observation else None, error))

    def raw_payload(self, payload_hash: str) -> object | None:
        """Read original source content from either legacy JSON or compressed rows."""
        row = self.db.execute("SELECT body FROM payloads WHERE hash=?", (payload_hash,)).fetchone()
        return _read_payload(row[0]) if row else None

    def observations(self, as_of: float) -> dict[str, Observation]:
        # Select the last successfully decoded observation as it was known then.
        rows = self.db.execute("""SELECT o.* FROM
            (SELECT DISTINCT source FROM observations) sources JOIN observations o ON o.id=(
            SELECT id FROM observations WHERE source=sources.source AND observed_at<=? AND normalized IS NOT NULL
            ORDER BY observed_at DESC,id DESC LIMIT 1)""", (as_of,))
        return {r["source"]: Observation.from_dict(json.loads(r["normalized"])) for r in rows}

    def attempts(self, as_of: float) -> list[dict]:
        return [dict(r) for r in self.db.execute("""SELECT o.source,o.observed_at,o.error FROM
          (SELECT DISTINCT source FROM observations) sources JOIN observations o ON o.id=(
          SELECT id FROM observations WHERE source=sources.source AND observed_at<=?
          ORDER BY observed_at DESC,id DESC LIMIT 1)""", (as_of,))]

    def record_config(self, config: dict, at: float) -> str:
        h = digest(config)
        with self.db:
            self.db.execute("INSERT OR IGNORE INTO source_config VALUES (?,?,?)", (h, at, _json_text(config)))
        return h

    def config_at(self, at: float) -> dict | None:
        # Prefer the actual issued forecast's configuration, including a return
        # to a previously used profile, over the profile's first-seen timestamp.
        row = self.db.execute("SELECT payload FROM forecasts WHERE issued_at<=? ORDER BY issued_at DESC LIMIT 1", (at,)).fetchone()
        if row:
            h = json.loads(row[0])["config_hash"]
            config = self.db.execute("SELECT payload FROM source_config WHERE hash=? AND first_seen_at<=?", (h, at)).fetchone()
            if config:
                return json.loads(config[0])
        row = self.db.execute("SELECT payload FROM source_config WHERE first_seen_at<=? ORDER BY first_seen_at DESC LIMIT 1", (at,)).fetchone()
        return json.loads(row[0]) if row else None

    def forecast(self, result: dict) -> str:
        h = digest(result)
        with self.db:
            self.db.execute("INSERT OR IGNORE INTO forecasts VALUES (?,?,?,?,?)",
                            (h, result["issued_at"], result["target"], result["model_version"], self.pack_json(result)))
        return h

    def forecasts(self, before: float, target: str) -> list[dict]:
        return [json.loads(r[0]) for r in self.db.execute(
            "SELECT payload FROM forecasts WHERE issued_at<=? AND target=? ORDER BY issued_at", (before, target))]

    def coverage(self, source: str, start: float, end: float, known_by: float, max_gap: float = 3600) -> bool:
        rows = self.db.execute("""SELECT observed_at,normalized FROM observations
            WHERE source=? AND observed_at BETWEEN ? AND ? AND normalized IS NOT NULL ORDER BY observed_at""",
                               (source, start - max_gap, min(end + max_gap, known_by)))
        times = []
        for row in rows:
            observation = json.loads(row["normalized"])
            generated = observation.get("generated_at")
            if observation.get("complete") and not observation.get("issues") and generated is not None and 0 <= row["observed_at"] - generated < max_gap:
                times.append(row["observed_at"])
        if not times or times[0] > start or times[-1] < end:
            return False
        return all(b - a <= max_gap for a, b in zip(times, times[1:]))
