"""Module-owned append-only evidence, frozen forecasts and settlement revisions."""
from __future__ import annotations

import json
from pathlib import Path
import sqlite3
import zlib

from .contracts import Forecast, digest, packed


class Store:
    def __init__(self, path, read_only=False):
        self.path = Path(path).resolve()
        if read_only:
            self.db = sqlite3.connect(self.path.as_uri() + "?mode=ro", uri=True, timeout=30)
        else:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            self.db = sqlite3.connect(self.path, timeout=30)
            self.db.execute("PRAGMA journal_mode=WAL")
        self.db.row_factory = sqlite3.Row
        if read_only:
            return
        self.db.executescript("""
        CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS payloads(hash TEXT PRIMARY KEY,body BLOB NOT NULL);
        CREATE TABLE IF NOT EXISTS observations(
          id INTEGER PRIMARY KEY,provider TEXT NOT NULL,observed_at REAL NOT NULL,
          generated_at REAL,healthy INTEGER NOT NULL,claims TEXT,payload_hash TEXT,error TEXT);
        CREATE INDEX IF NOT EXISTS observations_time ON observations(provider,observed_at);
        CREATE TABLE IF NOT EXISTS claims(
          id INTEGER PRIMARY KEY,provider TEXT NOT NULL,record TEXT NOT NULL,
          observed_at REAL NOT NULL,signature TEXT NOT NULL,payload TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS claims_identity ON claims(provider,record,observed_at);
        CREATE TABLE IF NOT EXISTS forecasts(
          id TEXT PRIMARY KEY,source TEXT NOT NULL,version TEXT NOT NULL,target TEXT NOT NULL,
          track TEXT NOT NULL,origin REAL NOT NULL,available_at REAL NOT NULL,
          expires_at REAL NOT NULL,payload TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS forecasts_time ON forecasts(available_at,origin);
        CREATE INDEX IF NOT EXISTS forecasts_latest ON forecasts(source,target,track,origin DESC,available_at DESC,id DESC);
        CREATE TABLE IF NOT EXISTS imports(kind TEXT NOT NULL,id TEXT NOT NULL,PRIMARY KEY(kind,id));
        CREATE TABLE IF NOT EXISTS indicators(id TEXT PRIMARY KEY,source TEXT NOT NULL,observed_at REAL NOT NULL,payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS truth_revisions(hash TEXT PRIMARY KEY,at REAL NOT NULL,payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS trials(
          id TEXT PRIMARY KEY,source TEXT NOT NULL,version TEXT NOT NULL,target TEXT NOT NULL,
          track TEXT NOT NULL,origin REAL NOT NULL,hours INTEGER NOT NULL,p REAL,
          frozen_at REAL NOT NULL,policy_hash TEXT NOT NULL,payload TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS trials_time ON trials(origin,hours);
        CREATE TABLE IF NOT EXISTS settlements(
          id INTEGER PRIMARY KEY,trial TEXT NOT NULL,at REAL NOT NULL,signature TEXT NOT NULL,
          label INTEGER,status TEXT NOT NULL,payload TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS settlements_trial ON settlements(trial,at);
        CREATE TABLE IF NOT EXISTS reports(id INTEGER PRIMARY KEY,at REAL NOT NULL,payload BLOB NOT NULL);
        """)

    def close(self):
        self.db.close()

    def get(self, key):
        row = self.db.execute("SELECT value FROM metadata WHERE key=?", (key,)).fetchone()
        return json.loads(row[0]) if row else None

    def put(self, key, value):
        with self.db:
            self.db.execute("INSERT OR REPLACE INTO metadata VALUES (?,?)", (key, packed(value)))

    def observation(self, provider, now, generated, healthy, claims, raw, error=None):
        h = digest(raw) if raw is not None else None
        with self.db:
            if raw is not None:
                self.db.execute("INSERT OR IGNORE INTO payloads VALUES (?,?)", (h, zlib.compress(packed(raw).encode())))
            self.db.execute("INSERT INTO observations(provider,observed_at,generated_at,healthy,claims,payload_hash,error) VALUES (?,?,?,?,?,?,?)",
                            (provider, now, generated, int(healthy), packed(claims) if claims is not None else None, h, error))
            for claim in claims or []:
                signature = digest({k: v for k, v in claim.items() if k != "observed_at"})
                last = self.db.execute("SELECT signature FROM claims WHERE provider=? AND record=? ORDER BY observed_at DESC,id DESC LIMIT 1",
                                       (provider, claim["record"])).fetchone()
                if last is None or last[0] != signature:
                    self.db.execute("INSERT INTO claims(provider,record,observed_at,signature,payload) VALUES (?,?,?,?,?)",
                                    (provider, claim["record"], now, signature, packed(claim)))

    def claims(self, now):
        # A feed dropping an old item is not a retraction. Explicit revisions
        # replace that provider's claim while the preceding versions survive.
        return [json.loads(r[0]) for r in self.db.execute("""
          SELECT payload FROM (SELECT *,ROW_NUMBER() OVER
          (PARTITION BY provider,record ORDER BY observed_at DESC,id DESC) rank
          FROM claims WHERE observed_at<=?) WHERE rank=1""", (now,))]

    def observations(self, now, successful=False):
        extra = " AND claims IS NOT NULL" if successful else ""
        rows = self.db.execute(f"""SELECT * FROM (SELECT *, ROW_NUMBER() OVER
          (PARTITION BY provider ORDER BY observed_at DESC,id DESC) rank FROM observations
          WHERE observed_at<=? {extra}) WHERE rank=1""", (now,))
        return [dict(r) for r in rows]

    def add_forecast(self, f: Forecast):
        f.validate()
        with self.db:
            self.db.execute("INSERT OR IGNORE INTO forecasts VALUES (?,?,?,?,?,?,?,?,?)",
                            (f.id, f.source, f.version, f.target, f.track, f.origin, f.available_at, f.expires_at, packed(f.__dict__)))

    def forecasts(self, now, latest=False):
        query = """SELECT f.payload FROM forecasts f JOIN (SELECT id,ROW_NUMBER() OVER
          (PARTITION BY source,target,track ORDER BY origin DESC,available_at DESC,id DESC) rank
          FROM forecasts WHERE available_at<=?) latest ON f.id=latest.id WHERE latest.rank=1""" if latest else \
            "SELECT payload FROM forecasts WHERE available_at<=? ORDER BY available_at,origin,id"
        return [Forecast(**json.loads(r[0])) for r in self.db.execute(query, (now,))]

    def seen(self, kind, id):
        return self.db.execute("SELECT 1 FROM imports WHERE kind=? AND id=?", (kind, str(id))).fetchone() is not None

    def mark(self, kind, id):
        with self.db:
            self.db.execute("INSERT OR IGNORE INTO imports VALUES (?,?)", (kind, str(id)))

    def indicator(self, source, now, payload):
        with self.db:
            self.db.execute("INSERT OR IGNORE INTO indicators VALUES (?,?,?,?)", (digest([source, payload]), source, now, packed(payload)))

    def truth(self, now, payload):
        h = digest(payload)
        with self.db:
            self.db.execute("INSERT OR IGNORE INTO truth_revisions VALUES (?,?,?)", (h, now, packed(payload)))
        return h

    def freeze(self, trial):
        id = digest([trial[k] for k in ("source", "version", "target", "track", "origin", "hours", "policy_hash")])
        with self.db:
            self.db.execute("INSERT OR IGNORE INTO trials VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                            (id, trial["source"], trial["version"], trial["target"], trial["track"], trial["origin"],
                             trial["hours"], trial["p"], trial["frozen_at"], trial["policy_hash"], packed(trial)))
        return id

    def trials(self, now):
        return [{"id": r["id"], **json.loads(r["payload"])} for r in self.db.execute(
            "SELECT id,payload FROM trials WHERE frozen_at<=? ORDER BY origin,source", (now,))]

    def settle(self, id, now, result):
        signature = digest({k: result[k] for k in ("label", "status", "events", "reason")})
        last = self.db.execute("SELECT signature FROM settlements WHERE trial=? ORDER BY at DESC,id DESC LIMIT 1", (id,)).fetchone()
        if last and last[0] == signature:
            return False
        with self.db:
            self.db.execute("INSERT INTO settlements(trial,at,signature,label,status,payload) VALUES (?,?,?,?,?,?)",
                            (id, now, signature, result["label"], result["status"], packed(result)))
        return True

    def settled(self, now):
        return {r["trial"]: {"at": r["at"], **json.loads(r["payload"])} for r in self.db.execute("""
          SELECT * FROM (SELECT *,ROW_NUMBER() OVER (PARTITION BY trial ORDER BY at DESC,id DESC) rank
          FROM settlements WHERE at<=?) WHERE rank=1""", (now,))}

    def report(self, now, payload):
        with self.db:
            self.db.execute("INSERT INTO reports(at,payload) VALUES (?,?)", (now, zlib.compress(packed(payload).encode())))

    def report_at(self, now):
        row = self.db.execute("SELECT payload FROM reports WHERE at<=? ORDER BY at DESC,id DESC LIMIT 1", (now,)).fetchone()
        return json.loads(zlib.decompress(row[0])) if row else None
