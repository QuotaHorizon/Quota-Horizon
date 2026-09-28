use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{Manager, Runtime};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

use crate::storage::{replace_file, resolve_paths, write_text_atomic};

const INDEX_NAME: &str = "session_index.jsonl";
const ROLLOUT_FOLDERS: [&str; 2] = ["sessions", "archived_sessions"];
const BUNDLE_KIND: &str = "codex-session-export";
const BUNDLE_REVISION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadEntry {
    session_id: String,
    session_kind: String,
    status: String,
    title: String,
    cwd: String,
    updated_at: Option<i64>,
    size_bytes: u64,
    rollout_count: usize,
    match_excerpt: Option<String>,
    match_timestamp: Option<String>,
    account_id: Option<String>,
    account_email: Option<String>,
    account_active: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadTokenTotals {
    session_id: String,
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadResumeResult {
    session_id: String,
    cwd: String,
    mode: String,
    resume_command: String,
    launched: bool,
    message: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ThreadRebindProtectedTarget {
    RolloutMetadata,
    CanonicalState,
    LegacyState,
    ThreadCatalog,
    SessionIndexGuard,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadRebindPreview {
    confirm_token: String,
    expires_at: String,
    session_id: String,
    title: String,
    old_cwd: String,
    new_cwd: String,
    rollout_file_count: usize,
    state_database_row_count: usize,
    catalog_row_count: usize,
    protected_targets: Vec<ThreadRebindProtectedTarget>,
    session_index_unchanged: bool,
    automatic_rollback: bool,
    crash_recovery: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadRebindReport {
    session_id: String,
    old_cwd: String,
    new_cwd: String,
    rollout_file_count: usize,
    state_database_row_count: usize,
    catalog_row_count: usize,
    message: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ThreadVisibilityRepairProtectedTarget {
    RolloutSource,
    CanonicalState,
    LegacyState,
    ThreadCatalog,
    SessionIndex,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadVisibilityRepairPreview {
    confirm_token: String,
    expires_at: String,
    mode: String,
    scope: String,
    requested_count: usize,
    scanned_count: usize,
    rollout_guard_count: usize,
    state_update_count: usize,
    state_insert_count: usize,
    catalog_update_count: usize,
    index_add_count: usize,
    index_update_count: usize,
    protected_targets: Vec<ThreadVisibilityRepairProtectedTarget>,
    rollout_source_unchanged: bool,
    automatic_rollback: bool,
    crash_recovery: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadVisibilityRepairReport {
    mode: String,
    repaired_session_count: usize,
    state_update_count: usize,
    state_insert_count: usize,
    catalog_update_count: usize,
    index_add_count: usize,
    index_update_count: usize,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MutationReport {
    requested_count: usize,
    affected_count: usize,
    released_bytes: u64,
    message: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ThreadTrashProtectedTarget {
    RolloutFiles,
    SessionIndex,
    StateVisibility,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadTrashPreviewItem {
    session_id: String,
    title: String,
    cwd: String,
    size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadTrashPreview {
    confirm_token: String,
    expires_at: String,
    requested_count: usize,
    affected_count: usize,
    total_size_bytes: u64,
    items: Vec<ThreadTrashPreviewItem>,
    protected_targets: Vec<ThreadTrashProtectedTarget>,
    creates_restore_point: bool,
    automatic_rollback: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ThreadRestoreProtectedTarget {
    RolloutFiles,
    SessionIndex,
    StateVisibility,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadRestorePreviewItem {
    session_id: String,
    title: String,
    cwd: String,
    size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadRestorePreview {
    confirm_token: String,
    expires_at: String,
    requested_count: usize,
    affected_count: usize,
    total_size_bytes: u64,
    items: Vec<ThreadRestorePreviewItem>,
    protected_targets: Vec<ThreadRestoreProtectedTarget>,
    conflicts_checked: bool,
    automatic_rollback: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadPurgePreviewItem {
    session_id: String,
    title: String,
    cwd: String,
    size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadPurgePreview {
    confirm_token: String,
    expires_at: String,
    empty_bin: bool,
    requested_count: usize,
    affected_count: usize,
    total_size_bytes: u64,
    items: Vec<ThreadPurgePreviewItem>,
    creates_temporary_restore_point: bool,
    automatic_rollback: bool,
    permanently_deletes: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadPurgeOutcome {
    session_id: String,
    status: String,
    released_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadPurgeReport {
    requested_count: usize,
    affected_count: usize,
    released_bytes: u64,
    outcomes: Vec<ThreadPurgeOutcome>,
    restore_point_removed: bool,
    message: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ThreadArchiveProtectedTarget {
    RolloutFiles,
    SessionIndex,
    StateVisibility,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadArchivePreviewItem {
    session_id: String,
    title: String,
    cwd: String,
    size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadArchivePreview {
    confirm_token: String,
    expires_at: String,
    requested_count: usize,
    affected_count: usize,
    total_size_bytes: u64,
    items: Vec<ThreadArchivePreviewItem>,
    protected_targets: Vec<ThreadArchiveProtectedTarget>,
    conflicts_checked: bool,
    preserves_session_index: bool,
    automatic_rollback: bool,
    crash_recovery: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MigrationReport {
    requested_count: usize,
    migrated_count: usize,
    skipped_count: usize,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BinEntry {
    session_id: String,
    title: String,
    cwd: String,
    deleted_at: Option<i64>,
    size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BundlePreview {
    package_version: u32,
    exported_at: Option<String>,
    total_count: usize,
    ready_count: usize,
    total_size_bytes: u64,
    items: Vec<BundlePreviewItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BundlePreviewItem {
    session_id: String,
    title: String,
    cwd: String,
    updated_at: Option<i64>,
    size_bytes: u64,
    status: String,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BundleResult {
    requested_count: usize,
    completed_count: usize,
    skipped_count: usize,
    path: String,
    message: String,
}

#[derive(Debug, Clone)]
struct RolloutSnapshot {
    session_id: String,
    title: String,
    cwd: String,
    updated_at: Option<i64>,
    started_at: Option<i64>,
    path: PathBuf,
    physical_paths: Vec<PathBuf>,
    relative_path: PathBuf,
    index_value: Value,
    size_bytes: u64,
    history_base_thread_id: Option<String>,
    parent_thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageManifest {
    kind: String,
    package_version: u32,
    exported_at: String,
    sessions: Vec<PackageItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageItem {
    session_id: String,
    title: String,
    cwd: String,
    updated_at: Option<i64>,
    relative_rollout_path: String,
    file_entry: String,
    size_bytes: u64,
    sha256: String,
    session_index_entry: Value,
    #[serde(default)]
    source_instance: Option<Value>,
    #[serde(default)]
    state_row: Option<SqliteRowSnapshot>,
    #[serde(default)]
    related_state: Vec<SqliteTableSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SqliteRowSnapshot {
    columns: Vec<String>,
    values: Vec<SqliteCell>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SqliteTableSnapshot {
    database: String,
    table: String,
    rows: Vec<SqliteRowSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum SqliteCell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinManifest {
    session_id: String,
    title: String,
    cwd: String,
    relative_rollout_path: String,
    session_index_entry: Value,
    deleted_at: String,
    #[serde(default)]
    state_visibility: Option<StateVisibilitySnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StateVisibilitySnapshot {
    rollout_path: String,
    archived: i64,
    archived_at: Option<i64>,
    preview: String,
}

#[derive(Debug, Clone)]
struct BinSnapshot {
    folder: PathBuf,
    manifest: BinManifest,
    manifest_revision: String,
    rollouts: Vec<PathBuf>,
}

include!("discovery.rs");
include!("read_model.rs");
include!("timeline.rs");
include!("timeline_sources.rs");
include!("search.rs");
include!("search_batches.rs");
include!("resume.rs");
include!("rebind.rs");
include!("ownership.rs");
include!("state_storage.rs");
include!("state_restore.rs");
include!("discard.rs");
include!("bin.rs");
include!("purge.rs");
include!("archive.rs");
include!("transfer.rs");
include!("migration.rs");
include!("visibility.rs");
include!("commands.rs");
include!("tests.rs");
include!("timeline_sources_tests.rs");
include!("search_tests.rs");
