//! Public-only collection and an append-only local evidence journal. No account
//! input, credentials, quota observations or private dashboard URLs are fetched.

use capacity_domain::public_reset::{
    PublicEvidenceDisposition, PublicEvidenceReview, PublicEvidenceScope, PublicResetLedger,
    PublicResetSignal, PublicSourceClass,
};
use capacity_domain::UtcTimestamp;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager, Runtime};

use crate::public_reset_sources::{
    canonical_public_url, decode_public_timeline, PublicCandidateBatch, PublicTimelineSource,
};

const DB_NAME: &str = "public-reset-ledger.sqlite3";
const MAX_DB_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SIGNAL_BYTES: usize = 32 * 1024;
const MAX_JOURNAL_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
const SOURCE_INTERVAL_SECONDS: i64 = 15 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceIssue {
    RequestFailed,
    HttpError,
    InvalidResponse,
    SchemaChanged,
    ResponseTooLarge,
    UpstreamStale,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceStatus {
    source_id: String,
    source_url: String,
    last_attempt_at: Option<UtcTimestamp>,
    last_success_at: Option<UtcTimestamp>,
    issue: Option<SourceIssue>,
    accepted_records: usize,
    rejected_records: usize,
    skipped_records: usize,
}

impl SourceStatus {
    fn empty(source: PublicTimelineSource) -> Self {
        Self {
            source_id: source.id().into(),
            source_url: source.url().into(),
            last_attempt_at: None,
            last_success_at: None,
            issue: None,
            accepted_records: 0,
            rejected_records: 0,
            skipped_records: 0,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveEntry {
    signal: PublicResetSignal,
    disposition: PublicEvidenceDisposition,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicResetTimeline {
    sources: Vec<SourceStatus>,
    entries: Vec<LiveEntry>,
    revision_count: usize,
    evidence_family_count: usize,
    insights: insights::PublicInsights,
    changes: changes::PublicRevisionHistory,
}

struct StoredLedger {
    ledger: PublicResetLedger,
    latest: HashMap<String, PublicResetSignal>,
    count: usize,
    bytes: u64,
    changes: changes::RecentChanges,
}

fn now() -> UtcTimestamp {
    UtcTimestamp::parse(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)).expect("UTC clock")
}

fn db_error(_: impl std::fmt::Display) -> String {
    "本地公开资料暂不可读或无法保存，请稍后重试；不会自动清空已有资料。".into()
}

fn validate_db_path(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() || meta.len() > MAX_DB_BYTES => Err(db_error("invalid file")),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(db_error(error)),
    }
}

fn open_store(path: &Path, write: bool) -> Result<Connection, String> {
    validate_db_path(path)?;
    if write {
        fs::create_dir_all(path.parent().ok_or_else(|| db_error("missing parent"))?)
            .map_err(db_error)?;
    }
    let flags = if write {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let mut conn = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(db_error)?;
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(db_error)?;
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(db_error)?;
    if version == 0 && write {
        let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'", [], |row| row.get(0)).map_err(db_error)?;
        if tables != 0 {
            return Err(db_error("unrecognized database"));
        }
        conn.execute_batch("BEGIN IMMEDIATE;
            CREATE TABLE public_signals (signal_id TEXT NOT NULL, revision INTEGER NOT NULL,
                source_id TEXT NOT NULL, signal_json TEXT NOT NULL, PRIMARY KEY(signal_id, revision));
            CREATE TABLE public_sources (source_id TEXT PRIMARY KEY, last_attempt_at TEXT NOT NULL,
                last_success_at TEXT, issue_json TEXT, accepted_records INTEGER NOT NULL,
                rejected_records INTEGER NOT NULL, skipped_records INTEGER NOT NULL);
            CREATE TABLE public_insights (id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL);
            PRAGMA user_version=2; COMMIT;").map_err(db_error)?;
    } else if version == 1 && write {
        let backup = path.with_extension("before-insights.sqlite3");
        if !backup.exists() {
            conn.execute(
                "VACUUM main INTO ?1",
                [backup.to_str().ok_or_else(|| db_error("backup path"))?],
            )
            .map_err(db_error)?;
        }
        conn.execute_batch("BEGIN IMMEDIATE; CREATE TABLE public_insights (id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL); PRAGMA user_version=2; COMMIT;").map_err(db_error)?;
    } else if !(1..=3).contains(&version) {
        return Err(db_error("unsupported schema"));
    }
    if write && version < 3 {
        if version == 2 {
            let backup = path.with_extension("before-reading-state.sqlite3");
            validate_db_path(&backup)?;
            if !backup.exists() {
                conn.execute(
                    "VACUUM main INTO ?1",
                    [backup.to_str().ok_or_else(|| db_error("backup path"))?],
                )
                .map_err(db_error)?;
            }
            // An existing unrelated/corrupt file must not be treated as a
            // usable restore point, nor overwritten by this migration.
            load_timeline(&backup, &now())?;
        }
        read_state::migrate(&mut conn)?;
    }
    Ok(conn)
}

fn validate_collected_signal(source: PublicTimelineSource, signal: &PublicResetSignal) -> bool {
    let Ok((canonical, class)) = canonical_public_url(&signal.source.canonical_url) else {
        return false;
    };
    signal.validate().is_ok()
        && canonical == signal.source.canonical_url
        && signal.source.review == PublicEvidenceReview::Indirect
        && signal.source.discovered_via == [source.url()]
        && signal.announcement_timing.as_ref().is_none_or(|timing| {
            source == PublicTimelineSource::QuotaResets && timing.source_url == source.url()
        })
        && (signal.source.source_class == class
            || (signal.source.source_class == PublicSourceClass::ProviderUiObservation
                && class == PublicSourceClass::IndependentTracker))
        && signal.scope
            == if signal.source.source_class == PublicSourceClass::ProviderUiObservation {
                PublicEvidenceScope::Personal
            } else {
                PublicEvidenceScope::Unknown
            }
}

fn load_ledger(
    conn: &Connection,
    history_at: Option<&UtcTimestamp>,
) -> Result<StoredLedger, String> {
    let (count, largest, bytes): (i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(length(CAST(signal_json AS BLOB))), 0), COALESCE(SUM(length(CAST(signal_json AS BLOB))), 0) FROM public_signals", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).map_err(db_error)?;
    if !(0..=PublicResetLedger::MAX_REVISIONS as i64).contains(&count)
        || !(0..=MAX_SIGNAL_BYTES as i64).contains(&largest)
        || !(0..=MAX_JOURNAL_PAYLOAD_BYTES as i64).contains(&bytes)
    {
        return Err(db_error("ledger bounds"));
    }
    let mut result = StoredLedger {
        ledger: PublicResetLedger::default(),
        latest: HashMap::new(),
        count: count as usize,
        bytes: bytes as u64,
        changes: changes::RecentChanges::default(),
    };
    let mut statement = conn.prepare("SELECT signal_id, revision, source_id, signal_json FROM public_signals ORDER BY signal_id, revision").map_err(db_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(db_error)?;
    for row in rows {
        let (id, revision, source_id, json) = row.map_err(db_error)?;
        let source =
            PublicTimelineSource::from_id(&source_id).ok_or_else(|| db_error("unknown source"))?;
        let signal: PublicResetSignal = serde_json::from_str(&json).map_err(db_error)?;
        if id != signal.signal_id
            || revision != signal.revision
            || !validate_collected_signal(source, &signal)
        {
            return Err(db_error("invalid evidence"));
        }
        result.ledger.append(signal.clone()).map_err(db_error)?;
        if history_at.is_some_and(|at| at.is_not_before(&signal.recorded_at)) {
            result.changes.record(result.latest.get(&id), &signal);
        }
        result.latest.insert(id, signal);
    }
    Ok(result)
}

fn read_sources(conn: &Connection) -> Result<Vec<SourceStatus>, String> {
    let mut sources: Vec<_> = PublicTimelineSource::ALL
        .into_iter()
        .map(SourceStatus::empty)
        .collect();
    let mut statement = conn.prepare("SELECT source_id, last_attempt_at, last_success_at, issue_json, accepted_records, rejected_records, skipped_records FROM public_sources").map_err(db_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, u32>(4)?,
                row.get::<_, u32>(5)?,
                row.get::<_, u32>(6)?,
            ))
        })
        .map_err(db_error)?;
    for row in rows {
        let (id, attempted, succeeded, issue, accepted, rejected, skipped) =
            row.map_err(db_error)?;
        let source = sources
            .iter_mut()
            .find(|source| source.source_id == id)
            .ok_or_else(|| db_error("unknown source"))?;
        source.last_attempt_at = Some(UtcTimestamp::parse(attempted).map_err(db_error)?);
        source.last_success_at = succeeded
            .map(UtcTimestamp::parse)
            .transpose()
            .map_err(db_error)?;
        source.issue = issue
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(db_error)?;
        source.accepted_records = accepted as usize;
        source.rejected_records = rejected as usize;
        source.skipped_records = skipped as usize;
    }
    Ok(sources)
}

fn load_timeline(path: &Path, at: &UtcTimestamp) -> Result<PublicResetTimeline, String> {
    validate_db_path(path)?;
    if !path.exists() {
        return Ok(PublicResetTimeline {
            sources: PublicTimelineSource::ALL
                .into_iter()
                .map(SourceStatus::empty)
                .collect(),
            entries: Vec::new(),
            revision_count: 0,
            evidence_family_count: 0,
            insights: insights::PublicInsights::default(),
            changes: changes::PublicRevisionHistory::default(),
        });
    }
    let mut conn = open_store(path, false)?;
    // Bounds, revisions and per-source freshness must describe the same SQLite
    // snapshot even when another command commits a refresh concurrently.
    let tx = conn.transaction().map_err(db_error)?;
    let store = load_ledger(&tx, Some(at))?;
    let mut entries: Vec<_> = store
        .ledger
        .as_of(at)
        .into_iter()
        .map(|signal| LiveEntry {
            signal: signal.clone(),
            disposition: signal.disposition(),
        })
        .collect();
    entries.sort_by_key(|entry| {
        std::cmp::Reverse(
            DateTime::parse_from_rfc3339(
                entry
                    .signal
                    .source
                    .published_at
                    .as_ref()
                    .unwrap_or(&entry.signal.recorded_at)
                    .as_str(),
            )
            .ok(),
        )
    });
    let mut changes = store.changes.finish();
    read_state::annotate(&tx, &mut changes)?;
    Ok(PublicResetTimeline {
        sources: read_sources(&tx)?,
        entries,
        revision_count: store.count,
        evidence_family_count: store.ledger.evidence_family_count_at(at),
        insights: insights::read_cache(&tx)?,
        changes,
    })
}

include!("collector.rs");
mod background;
mod changes;
pub(crate) mod insights;
pub(crate) mod read_state;
pub(crate) use background::{setup, shutdown};
include!("tests.rs");
