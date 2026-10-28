"""Module-owned append-only evidence, frozen forecasts and settlement revisions."""
from __future__ import annotations

import json
from pathlib import Path
import sqlite3
import zlib

from .contracts import Forecast, digest, packed
try:
    from reset_intelligence.storage_codec import configure_storage, pack_inline, RETIRED_RAW, maintain_raw_cache, reclaim_pages
except ModuleNotFoundError as error:
    if error.name != "reset_intelligence":
        raise
    # The documented standalone observer command starts in this module's root.
    # Resolve the sibling shared codec without changing sys.path or requiring an
    # environment install; bundled distributions import the package above.
    import importlib.util
    codec_path = Path(__file__).resolve().parents[2] / "reset-intelligence" / "reset_intelligence" / "storage_codec.py"
    spec = importlib.util.spec_from_file_location("_quota_horizon_storage_codec", codec_path)
    if spec is None or spec.loader is None:
        raise ImportError("shared reset storage codec is unavailable") from error
    codec = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(codec)
    configure_storage = codec.configure_storage
    pack_inline = codec.pack_inline
    RETIRED_RAW = codec.RETIRED_RAW
    maintain_raw_cache = codec.maintain_raw_cache
    reclaim_pages = codec.reclaim_pages


class Store:
    def __init__(self, path, read_only=False):
        self.path = Path(path).resolve()
        if read_only:
            self.db = sqlite3.connect(self.path.as_uri() + "?mode=ro", uri=True, timeout=30)
        else:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            new_database = not self.path.exists()
            self.db = sqlite3.connect(self.path, timeout=30)
            if new_database:
                self.db.execute("PRAGMA auto_vacuum=INCREMENTAL")
            self.db.execute("PRAGMA journal_mode=WAL")
        self.db.row_factory = sqlite3.Row
        if read_only:
            self.json_storage = configure_storage(self.db, read_only=True)
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
        CREATE INDEX IF NOT EXISTS forecasts_publication_curve ON forecasts(available_at,origin,id)
          WHERE track IN ('native','model');
        CREATE INDEX IF NOT EXISTS forecasts_latest ON forecasts(source,target,track,origin DESC,available_at DESC,id DESC);
        CREATE TABLE IF NOT EXISTS retired_forecasts(id TEXT PRIMARY KEY,available_at REAL NOT NULL);
        CREATE TABLE IF NOT EXISTS imports(kind TEXT NOT NULL,id TEXT NOT NULL,PRIMARY KEY(kind,id));
        CREATE TABLE IF NOT EXISTS indicators(id TEXT PRIMARY KEY,source TEXT NOT NULL,observed_at REAL NOT NULL,payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS truth_revisions(hash TEXT PRIMARY KEY,at REAL NOT NULL,payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS trials(
          id TEXT PRIMARY KEY,source TEXT NOT NULL,version TEXT NOT NULL,target TEXT NOT NULL,
          track TEXT NOT NULL,origin REAL NOT NULL,hours INTEGER NOT NULL,p REAL,
          frozen_at REAL NOT NULL,policy_hash TEXT NOT NULL,payload TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS trials_time ON trials(origin,hours);
        CREATE TABLE IF NOT EXISTS trial_revisions(
          id INTEGER PRIMARY KEY,trial TEXT NOT NULL,at REAL NOT NULL,
          signature TEXT NOT NULL,payload TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS trial_revisions_trial ON trial_revisions(trial,at);
        CREATE TABLE IF NOT EXISTS settlements(
          id INTEGER PRIMARY KEY,trial TEXT NOT NULL,at REAL NOT NULL,signature TEXT NOT NULL,
          label INTEGER,status TEXT NOT NULL,payload TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS settlements_trial ON settlements(trial,at);
        CREATE TABLE IF NOT EXISTS reports(id INTEGER PRIMARY KEY,at REAL NOT NULL,payload BLOB NOT NULL);
        CREATE TABLE IF NOT EXISTS runtime_events(id INTEGER PRIMARY KEY,at REAL NOT NULL,payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS preference_samples(
          id TEXT PRIMARY KEY,source TEXT NOT NULL,round_id TEXT NOT NULL,
          observed_at REAL NOT NULL,received_at REAL NOT NULL,payload TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS preference_samples_time ON preference_samples(source,round_id,observed_at);
        CREATE INDEX IF NOT EXISTS preference_samples_received ON preference_samples(received_at);
        CREATE TABLE IF NOT EXISTS preference_trials(id TEXT PRIMARY KEY,frozen_at REAL NOT NULL,payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS preference_settlements(trial TEXT NOT NULL,at REAL NOT NULL,signature TEXT NOT NULL,payload TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS preference_settlements_trial ON preference_settlements(trial,at);
        CREATE TABLE IF NOT EXISTS reaction_forecasts(
          id TEXT PRIMARY KEY,source TEXT NOT NULL,target TEXT NOT NULL,track TEXT NOT NULL,
          available_at REAL NOT NULL,payload TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS reaction_forecasts_time ON reaction_forecasts(available_at);
        CREATE INDEX IF NOT EXISTS reaction_forecasts_series ON reaction_forecasts(source,target,track,available_at);
        """)
        self.json_storage = configure_storage(self.db)

    def close(self):
        self.db.close()

    def pack_json(self, value, volatile_fields=()):
        return self.json_storage.pack_json(value, volatile_fields)

    def runtime_event(self, now, payload):
        with self.db:
            self.db.execute("INSERT INTO runtime_events(at,payload) VALUES (?,?)", (now, packed(payload)))

    def runtime_events(self, now):
        if not self.db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='runtime_events'").fetchone():
            return []
        return [json.loads(r[0]) for r in self.db.execute("SELECT payload FROM runtime_events WHERE at<=? ORDER BY at,id", (now,))]

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
                saved = self.db.execute("SELECT body=? FROM payloads WHERE hash=?", (RETIRED_RAW, h)).fetchone()
                if saved is None:
                    self.db.execute("INSERT OR IGNORE INTO payloads VALUES (?,?)", (h, pack_inline(raw)))
                elif saved[0]:
                    self.db.execute("UPDATE payloads SET body=? WHERE hash=? AND body=?", (pack_inline(raw), h, RETIRED_RAW))
            self.db.execute("INSERT INTO observations(provider,observed_at,generated_at,healthy,claims,payload_hash,error) VALUES (?,?,?,?,?,?,?)",
                            (provider, now, generated, int(healthy), self.pack_json(claims) if claims is not None else None, h, error))
            from .evidence import inventory_revisions
            revisions = inventory_revisions(self, provider, raw, now, error is None and claims is not None, claims or [])
            revised = {(c["provider"], c["record"]) for c in revisions}
            self.record_claims([c for c in claims or [] if (c["provider"], c["record"]) not in revised] + revisions, now)

    def record_claims(self, claims, now):
        # Reinterpretation appends knowledge revisions, never monitoring samples.
        for claim in claims:
            signature = digest({k: v for k, v in claim.items() if k != "observed_at"})
            last = self.db.execute("SELECT signature FROM claims WHERE provider=? AND record=? ORDER BY observed_at DESC,id DESC LIMIT 1",
                                   (claim["provider"], claim["record"])).fetchone()
            if last is None or last[0] != signature:
                self.db.execute("INSERT INTO claims(provider,record,observed_at,signature,payload) VALUES (?,?,?,?,?)",
                                (claim["provider"], claim["record"], now, signature, packed(claim)))

    def claims(self, now):
        # A feed dropping an old item is not a retraction. Explicit revisions
        # replace that provider's claim while the preceding versions survive.
        return [json.loads(r[0]) for r in self.db.execute("""
          SELECT payload FROM (SELECT *,ROW_NUMBER() OVER
          (PARTITION BY provider,record ORDER BY observed_at DESC,id DESC) rank
          FROM claims WHERE observed_at<=?) WHERE rank=1""", (now,))]

    def observations(self, now, successful=False):
        extra = " AND claims IS NOT NULL" if successful else ""
        rows = self.db.execute(f"""SELECT o.* FROM
          (SELECT DISTINCT provider FROM observations) providers JOIN observations o ON o.id=(
          SELECT id FROM observations WHERE provider=providers.provider AND observed_at<=? {extra}
          ORDER BY observed_at DESC,id DESC LIMIT 1)""", (now,))
        return [dict(r) for r in rows]

    def add_forecast(self, f: Forecast):
        f.validate()
        key = f.id
        if self.db.execute("SELECT 1 FROM forecasts WHERE id=? UNION ALL SELECT 1 FROM retired_forecasts WHERE id=?", (key, key)).fetchone():
            return
        with self.db:
            self.db.execute("INSERT OR IGNORE INTO forecasts VALUES (?,?,?,?,?,?,?,?,?)",
                            (key, f.source, f.version, f.target, f.track, f.origin, f.available_at, f.expires_at,
                             pack_inline(f.__dict__) if f.track in ("shadow", "adapted") else self.pack_json(f.__dict__)))

    def forecast_count(self, now):
        count = self.db.execute("SELECT COUNT(*) FROM forecasts WHERE available_at<=?", (now,)).fetchone()[0]
        if self.db.execute("SELECT 1 FROM sqlite_master WHERE name='retired_forecasts'").fetchone():
            count += self.db.execute("SELECT COUNT(*) FROM retired_forecasts WHERE available_at<=?", (now,)).fetchone()[0]
        return count

    def forecasts(self, now, latest=False):
        # Discover series through the covering index, then seek its latest known
        # publication. Ranking every historical payload makes this read grow
        # with the archive even though only one row per series is needed.
        query = """SELECT f.payload FROM
          (SELECT DISTINCT source,target,track FROM forecasts) series
          JOIN forecasts f ON f.id=(SELECT id FROM forecasts
            WHERE source=series.source AND target=series.target AND track=series.track
              AND available_at<=?
            ORDER BY origin DESC,available_at DESC,id DESC LIMIT 1)
          ORDER BY f.source,f.target,f.track""" if latest else \
            "SELECT payload FROM forecasts WHERE available_at<=? ORDER BY available_at,origin,id"
        return [Forecast(**json.loads(r[0])) for r in self.db.execute(query, (now,))]

    def seen(self, kind, id):
        return self.db.execute("SELECT 1 FROM imports WHERE kind=? AND id=?", (kind, str(id))).fetchone() is not None

    def mark(self, kind, id):
        with self.db:
            self.db.execute("INSERT OR IGNORE INTO imports VALUES (?,?)", (kind, str(id)))

    def indicator(self, source, now, payload):
        with self.db:
            self.db.execute("INSERT OR IGNORE INTO indicators VALUES (?,?,?,?)", (digest([source, payload]), source, now, self.pack_json(payload)))

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

    def trial_revisions(self, now):
        if not self.db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='trial_revisions'").fetchone():
            return {}
        return {r["trial"]: json.loads(r["payload"]) for r in self.db.execute("""
          SELECT r.trial,r.payload FROM (SELECT DISTINCT trial FROM trial_revisions) ids
          JOIN trial_revisions r ON r.id=(SELECT id FROM trial_revisions
            WHERE trial=ids.trial AND at<=? ORDER BY at DESC,id DESC LIMIT 1)""", (now,))}

    def revise_trial(self, trial, now, revision):
        signature = digest(revision)
        last = self.db.execute("SELECT signature FROM trial_revisions WHERE trial=? ORDER BY at DESC,id DESC LIMIT 1", (trial,)).fetchone()
        if last and last[0] == signature:
            return False
        self.db.execute("INSERT INTO trial_revisions(trial,at,signature,payload) VALUES (?,?,?,?)",
                        (trial, now, signature, packed(revision)))
        return True

    def trials(self, now, compact=False):
        revisions = self.trial_revisions(now)
        if compact:
            rows = self.db.execute("""SELECT id,source,version,target,track,origin,hours,p,frozen_at,policy_hash,
              json_extract(qh_json(payload),'$.private','$.headline','$.missing_reason','$.forecast_id',
                           '$.received_at','$.publication_origin') fields
              FROM trials WHERE frozen_at<=? ORDER BY origin,source""", (now,))
        else:
            rows = self.db.execute("SELECT id,payload FROM trials WHERE frozen_at<=? ORDER BY origin,source", (now,))
        result = []
        symbols = {}
        for row in rows:
            if compact:
                value = {key: row[key] for key in row.keys() if key != "fields"}
                value.update(zip(("private", "headline", "missing_reason", "forecast_id", "received_at", "publication_origin"), json.loads(row["fields"])))
                for key in ("source", "version", "target", "track", "policy_hash", "missing_reason"):
                    if isinstance(value.get(key), str):
                        value[key] = symbols.setdefault(value[key], value[key])
            else:
                value = {"id": row["id"], **json.loads(row["payload"])}
            if revision := revisions.get(row["id"]):
                value.update(original_probability=value["p"], original_missing_reason=value.get("missing_reason"))
                value.update(revision)
            result.append(value)
        return result

    def settle(self, id, now, result, commit=True):
        content = {k: result.get(k) for k in ("label", "status", "events", "reason",
            "score_eligible", "event_score_eligible", "coverage_complete", "settlement_version")}
        if result.get("eligibility_revision") is not None:
            content["eligibility_revision"] = result["eligibility_revision"]
        signature = digest(content)
        last = self.db.execute("SELECT signature FROM settlements WHERE trial=? ORDER BY at DESC,id DESC LIMIT 1", (id,)).fetchone()
        if last and last[0] == signature:
            return False
        # The first eligibility-aware writer included an empty field in every
        # signature. Accept those already-written rows as unchanged too, while
        # preserving actual revision changes and every historical settlement.
        if last and "eligibility_revision" not in content and last[0] == digest({**content, "eligibility_revision": None}):
            return False
        self.db.execute("INSERT INTO settlements(trial,at,signature,label,status,payload) VALUES (?,?,?,?,?,?)",
                        (id, now, signature, result["label"], result["status"], packed(result)))
        if commit:
            self.db.commit()
        return True

    def settled(self, now, compact=False):
        # Seek each trial's latest revision through settlements_trial instead
        # of sorting all historical payloads on every background report.
        fields = ("label", "status", "events", "reason", "deadline", "matures_at",
                  "score_eligible", "event_score_eligible", "coverage_complete",
                  "settlement_version")
        projection = ("json_extract(qh_json(s.payload)," +
                      ",".join("'$." + key + "'" for key in fields) + ")") if compact else "s.payload"
        rows = self.db.execute(f"""SELECT s.trial,s.at,{projection} AS payload FROM
          (SELECT DISTINCT trial FROM settlements) trials JOIN settlements s ON s.id=(
            SELECT id FROM settlements WHERE trial=trials.trial AND at<=?
            ORDER BY at DESC,id DESC LIMIT 1) ORDER BY s.trial""", (now,))
        output, symbols = {}, {}
        for row in rows:
            value = json.loads(row["payload"])
            if compact:
                value = dict(zip(fields, value))
                for key in ("status", "reason", "settlement_version"):
                    if isinstance(value.get(key), str):
                        value[key] = symbols.setdefault(value[key], value[key])
            output[row["trial"]] = {"at": row["at"], **value}
        return output

    def raw_payload(self, payload_hash):
        row = self.db.execute("SELECT body FROM payloads WHERE hash=?", (payload_hash,)).fetchone()
        if row and row[0] == RETIRED_RAW:
            return None
        return json.loads(zlib.decompress(row[0]) if isinstance(row[0], bytes) else row[0]) if row else None

    def report(self, now, payload, *, archive=False):
        with self.db:
            self.db.execute("INSERT INTO reports(at,payload) VALUES (?,?)", (now, pack_inline(payload)))
        if archive:
            from .retention import retain_reports
            retain_reports(self, now)

    def report_at(self, now):
        row = self.db.execute("SELECT payload FROM reports WHERE id=(SELECT id FROM reports WHERE at<=? ORDER BY at DESC,id DESC LIMIT 1)", (now,)).fetchone()
        # QHJSON2 is expanded by the shared row factory. Legacy reports retain
        # their original zlib representation until explicit maintenance.
        return json.loads(zlib.decompress(row[0]) if isinstance(row[0], bytes) else row[0]) if row else None
