use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

use capacity_migration::{
    read_importable_accounts, LegacyAccountAuthMode, LegacyAccountImportBundle,
    LegacyAccountImportRecord,
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use uuid::Uuid;

use crate::{
    auth::{account_fields, canonicalize_chatgpt_auth, validate_auth},
    storage::{ensure_private_directory, harden_private_file},
};

const CONFIRMATION_PHRASE: &str = "IMPORT ACCOUNTS";
const ROLLBACK_PHRASE: &str = "ROLLBACK ACCOUNTS";
const CONFIRMATION_TTL_SECONDS: i64 = 120;
const MIGRATION_DIRECTORY: &str = "legacy-migration-v1";
const OPERATIONS_DIRECTORY: &str = "account-operations";
const LATEST_FILE: &str = "account-latest.json";
const JOURNAL_FILE: &str = "journal.json";
const STAGING_DIRECTORY: &str = "staged";
const MAX_AUTH_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ACCOUNT_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ACCOUNT_DIRECTORY_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ACCOUNT_FILES: usize = 32;
const MAX_JOURNAL_BYTES: u64 = 1024 * 1024;

static PENDING_IMPORTS: LazyLock<Mutex<HashMap<String, PendingAccountImport>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
struct PendingAccountImport {
    source_path: PathBuf,
    source_revision: String,
    accounts_root: PathBuf,
    imports: Vec<PreparedAccount>,
    already_present_count: usize,
    conflict_count: usize,
    unsupported_count: usize,
    expires_at: DateTime<Utc>,
}

#[derive(Clone)]
struct PreparedAccount {
    target_id: String,
    files: Vec<PreparedAccountFile>,
    after_revision: String,
}

#[derive(Clone)]
struct PreparedAccountFile {
    name: &'static str,
    bytes: Vec<u8>,
    revision: String,
}

#[derive(Clone)]
struct NormalizedCandidate {
    ordinal: usize,
    label: String,
    preferred: bool,
    target_id: String,
    email: String,
    auth: Value,
    auth_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LegacyAccountMigrationPreview {
    schema_version: &'static str,
    status: &'static str,
    confirm_token: Option<String>,
    expires_at: Option<String>,
    typed_confirmation: &'static str,
    accounts: Vec<LegacyAccountMigrationPreviewRow>,
    import_count: usize,
    already_present_count: usize,
    conflict_count: usize,
    unsupported_count: usize,
    preserves_current_login: bool,
    imports_quota_cache: bool,
    creates_restore_point: bool,
    automatic_rollback: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LegacyAccountMigrationPreviewRow {
    source_ordinal: usize,
    label: String,
    auth_mode: &'static str,
    preferred: bool,
    disposition: &'static str,
    reason_code: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LegacyAccountMigrationResult {
    schema_version: &'static str,
    status: &'static str,
    operation_id: Option<String>,
    imported_count: usize,
    already_present_count: usize,
    conflict_count: usize,
    unsupported_count: usize,
    source_unchanged: bool,
    current_login_preserved: bool,
    rollback_available: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LegacyAccountMigrationOperationView {
    schema_version: &'static str,
    status: &'static str,
    operation_id: Option<String>,
    imported_count: usize,
    rollback_available: bool,
    recovery_required: bool,
    typed_rollback: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalState {
    Prepared,
    Applying,
    Committed,
    RolledBack,
    NeedsReview,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JournalFile {
    name: String,
    revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JournalAccount {
    target_id: String,
    after_revision: String,
    files: Vec<JournalFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AccountMigrationJournal {
    schema_version: String,
    operation_id: String,
    state: JournalState,
    source_revision: String,
    created_at: String,
    updated_at: String,
    accounts: Vec<JournalAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LatestOperation {
    schema_version: String,
    operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AccountDirectoryObservation {
    Missing,
    Present { revision: String },
}

struct PlannedAccountImport {
    rows: Vec<LegacyAccountMigrationPreviewRow>,
    imports: Vec<PreparedAccount>,
    already_present_count: usize,
    conflict_count: usize,
    unsupported_count: usize,
}

fn app_data_paths<R: Runtime>(app: &AppHandle<R>) -> Result<(PathBuf, PathBuf), String> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|_| "无法定位 QuotaHorizon 数据目录。".to_string())?;
    Ok((
        app_data.join("accounts"),
        app_data.join(MIGRATION_DIRECTORY),
    ))
}

fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn sha256_revision(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

fn valid_sha256_revision(value: &str) -> bool {
    value.len() == 71
        && value.strip_prefix("sha256:").is_some_and(|digest| {
            digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

pub(crate) fn source_revision(bundle: &LegacyAccountImportBundle) -> String {
    let mut hasher = Sha256::new();
    hash_field(
        &mut hasher,
        bundle
            .preferred_source_id
            .as_deref()
            .unwrap_or_default()
            .as_bytes(),
    );
    for account in &bundle.accounts {
        hash_field(&mut hasher, account.source_id.as_bytes());
        hash_field(&mut hasher, account.display_name.as_bytes());
        hash_field(
            &mut hasher,
            match account.auth_mode {
                LegacyAccountAuthMode::Chatgpt => b"chatgpt",
                LegacyAccountAuthMode::ApiKey => b"api_key",
                LegacyAccountAuthMode::Unknown => b"unknown",
            },
        );
        hash_field(&mut hasher, account.created_at.as_bytes());
        hash_field(
            &mut hasher,
            account
                .last_used_at
                .as_deref()
                .unwrap_or_default()
                .as_bytes(),
        );
        hash_field(&mut hasher, &account.auth_json);
        hash_field(
            &mut hasher,
            account.config_toml.as_deref().unwrap_or_default(),
        );
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn safe_target_id(value: &str) -> bool {
    value.len() == 24
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn file_mode(_: &fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn private_directory_mode() -> u32 {
    0o700
}

#[cfg(not(unix))]
fn private_directory_mode() -> u32 {
    0
}

#[cfg(unix)]
fn private_file_mode() -> u32 {
    0o600
}

#[cfg(not(unix))]
fn private_file_mode() -> u32 {
    0
}

#[cfg(unix)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
}

#[cfg(not(unix))]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

pub(crate) fn read_safe_file(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let before =
        fs::symlink_metadata(path).map_err(|_| "账号迁移目标文件不可读取。".to_string())?;
    if before.file_type().is_symlink() || !before.is_file() || before.len() > maximum {
        return Err("账号迁移目标文件不安全或超过大小上限。".to_string());
    }
    let mut file = File::open(path).map_err(|_| "账号迁移目标文件不可读取。".to_string())?;
    let opened = file
        .metadata()
        .map_err(|_| "账号迁移目标文件无法验证。".to_string())?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| "账号迁移目标文件不可读取。".to_string())?;
    let after =
        fs::symlink_metadata(path).map_err(|_| "账号迁移目标文件在读取时发生变化。".to_string())?;
    if bytes.len() as u64 > maximum
        || after.file_type().is_symlink()
        || !after.is_file()
        || !same_file_identity(&opened, &after)
    {
        return Err("账号迁移目标文件在读取时发生变化。".to_string());
    }
    Ok(bytes)
}

fn account_revision_from_parts(directory_mode: u32, files: &[(String, u32, String)]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(directory_mode.to_be_bytes());
    for (name, mode, revision) in files {
        hash_field(&mut hasher, name.as_bytes());
        hasher.update(mode.to_be_bytes());
        hash_field(&mut hasher, revision.as_bytes());
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn observe_account_directory(path: &Path) -> Result<AccountDirectoryObservation, String> {
    let directory = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AccountDirectoryObservation::Missing);
        }
        Err(_) => return Err("无法检查账号迁移目标。".to_string()),
    };
    if directory.file_type().is_symlink() || !directory.is_dir() {
        return Err("账号迁移目标不是安全目录。".to_string());
    }
    let mut entries = fs::read_dir(path)
        .map_err(|_| "无法检查账号迁移目标。".to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "无法检查账号迁移目标。".to_string())?;
    if entries.len() > MAX_ACCOUNT_FILES {
        return Err("账号迁移目标文件数量超过安全上限。".to_string());
    }
    entries.sort_by_key(|entry| entry.file_name());
    let mut total = 0_u64;
    let mut files = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "账号迁移目标包含无效文件名。".to_string())?;
        if name.is_empty()
            || name.len() > 128
            || name.chars().any(char::is_control)
            || matches!(name.as_str(), "." | "..")
        {
            return Err("账号迁移目标包含无效文件名。".to_string());
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| "无法检查账号迁移目标文件。".to_string())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() > MAX_ACCOUNT_FILE_BYTES
        {
            return Err("账号迁移目标包含不安全文件。".to_string());
        }
        total = total.saturating_add(metadata.len());
        if total > MAX_ACCOUNT_DIRECTORY_BYTES {
            return Err("账号迁移目标大小超过安全上限。".to_string());
        }
        let bytes = read_safe_file(&entry.path(), MAX_ACCOUNT_FILE_BYTES)?;
        files.push((name, file_mode(&metadata), sha256_revision(&bytes)));
    }
    Ok(AccountDirectoryObservation::Present {
        revision: account_revision_from_parts(file_mode(&directory), &files),
    })
}

fn auth_mode_name(mode: LegacyAccountAuthMode) -> &'static str {
    match mode {
        LegacyAccountAuthMode::Chatgpt => "chatgpt",
        LegacyAccountAuthMode::ApiKey => "api_key",
        LegacyAccountAuthMode::Unknown => "unknown",
    }
}

fn preview_row(
    ordinal: usize,
    record: &LegacyAccountImportRecord,
    preferred: bool,
    disposition: &'static str,
    reason_code: &'static str,
) -> LegacyAccountMigrationPreviewRow {
    LegacyAccountMigrationPreviewRow {
        source_ordinal: ordinal + 1,
        label: record.display_name.clone(),
        auth_mode: auth_mode_name(record.auth_mode),
        preferred,
        disposition,
        reason_code,
    }
}

fn normalize_candidate(
    ordinal: usize,
    record: &LegacyAccountImportRecord,
    preferred_source_id: Option<&str>,
) -> Result<NormalizedCandidate, &'static str> {
    if record.auth_mode != LegacyAccountAuthMode::Chatgpt {
        return Err("legacy_account_requires_provider_review");
    }
    let mut auth = serde_json::from_slice::<Value>(&record.auth_json)
        .map_err(|_| "legacy_account_auth_invalid")?;
    canonicalize_chatgpt_auth(&mut auth).map_err(|_| "legacy_account_auth_unsupported")?;
    validate_auth(&auth).map_err(|_| "legacy_account_auth_unsupported")?;
    let (email, _, _, target_id) =
        account_fields(&auth).map_err(|_| "legacy_account_identity_unavailable")?;
    if !safe_target_id(&target_id) {
        return Err("legacy_account_identity_invalid");
    }
    let auth_bytes =
        serde_json::to_vec_pretty(&auth).map_err(|_| "legacy_account_auth_unsupported")?;
    Ok(NormalizedCandidate {
        ordinal,
        label: record.display_name.clone(),
        preferred: preferred_source_id == Some(record.source_id.as_str()),
        target_id,
        email,
        auth,
        auth_bytes,
    })
}

fn prepared_account(candidate: &NormalizedCandidate) -> PreparedAccount {
    let mut files = vec![PreparedAccountFile {
        name: "auth.json",
        revision: sha256_revision(&candidate.auth_bytes),
        bytes: candidate.auth_bytes.clone(),
    }];
    let label = candidate.label.trim();
    if !label.is_empty() && !label.eq_ignore_ascii_case(candidate.email.trim()) {
        let bytes = label.as_bytes().to_vec();
        files.push(PreparedAccountFile {
            name: "note.txt",
            revision: sha256_revision(&bytes),
            bytes,
        });
    }
    files.sort_by_key(|file| file.name);
    let revision_parts = files
        .iter()
        .map(|file| {
            (
                file.name.to_string(),
                private_file_mode(),
                file.revision.clone(),
            )
        })
        .collect::<Vec<_>>();
    PreparedAccount {
        target_id: candidate.target_id.clone(),
        files,
        after_revision: account_revision_from_parts(private_directory_mode(), &revision_parts),
    }
}

fn target_auth_matches(path: &Path, source: &Value) -> bool {
    let Ok(bytes) = read_safe_file(&path.join("auth.json"), MAX_AUTH_BYTES) else {
        return false;
    };
    let Ok(mut current) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    canonicalize_chatgpt_auth(&mut current)
        .and_then(|_| validate_auth(&current))
        .is_ok()
        && current == *source
}

fn plan_account_import(
    bundle: &LegacyAccountImportBundle,
    accounts_root: &Path,
) -> Result<PlannedAccountImport, String> {
    let mut rows = Vec::with_capacity(bundle.accounts.len());
    let mut groups: BTreeMap<String, Vec<NormalizedCandidate>> = BTreeMap::new();
    let mut unsupported_count = 0;
    for (ordinal, record) in bundle.accounts.iter().enumerate() {
        match normalize_candidate(ordinal, record, bundle.preferred_source_id.as_deref()) {
            Ok(candidate) => groups
                .entry(candidate.target_id.clone())
                .or_default()
                .push(candidate),
            Err(reason_code) => {
                unsupported_count += 1;
                rows.push(preview_row(
                    ordinal,
                    record,
                    bundle.preferred_source_id.as_deref() == Some(record.source_id.as_str()),
                    "unsupported",
                    reason_code,
                ));
            }
        }
    }

    let mut imports = Vec::new();
    let mut already_present_count = 0;
    let mut conflict_count = 0;
    for candidates in groups.values() {
        let first = &candidates[0];
        if candidates
            .iter()
            .skip(1)
            .any(|item| item.auth != first.auth)
        {
            conflict_count += candidates.len();
            for candidate in candidates {
                rows.push(LegacyAccountMigrationPreviewRow {
                    source_ordinal: candidate.ordinal + 1,
                    label: candidate.label.clone(),
                    auth_mode: "chatgpt",
                    preferred: candidate.preferred,
                    disposition: "keep_existing",
                    reason_code: "legacy_source_identity_conflict",
                });
            }
            continue;
        }

        let target = accounts_root.join(&first.target_id);
        let observation = observe_account_directory(&target)?;
        match observation {
            AccountDirectoryObservation::Missing => {
                imports.push(prepared_account(first));
                rows.push(LegacyAccountMigrationPreviewRow {
                    source_ordinal: first.ordinal + 1,
                    label: first.label.clone(),
                    auth_mode: "chatgpt",
                    preferred: first.preferred,
                    disposition: "import",
                    reason_code: "legacy_account_new",
                });
            }
            AccountDirectoryObservation::Present { .. }
                if target_auth_matches(&target, &first.auth) =>
            {
                already_present_count += 1;
                rows.push(LegacyAccountMigrationPreviewRow {
                    source_ordinal: first.ordinal + 1,
                    label: first.label.clone(),
                    auth_mode: "chatgpt",
                    preferred: first.preferred,
                    disposition: "already_present",
                    reason_code: "legacy_account_already_present",
                });
            }
            AccountDirectoryObservation::Present { .. } => {
                conflict_count += 1;
                rows.push(LegacyAccountMigrationPreviewRow {
                    source_ordinal: first.ordinal + 1,
                    label: first.label.clone(),
                    auth_mode: "chatgpt",
                    preferred: first.preferred,
                    disposition: "keep_existing",
                    reason_code: "legacy_target_credential_preserved",
                });
            }
        }
        for duplicate in candidates.iter().skip(1) {
            already_present_count += 1;
            rows.push(LegacyAccountMigrationPreviewRow {
                source_ordinal: duplicate.ordinal + 1,
                label: duplicate.label.clone(),
                auth_mode: "chatgpt",
                preferred: duplicate.preferred,
                disposition: "already_present",
                reason_code: "legacy_source_identity_duplicate",
            });
        }
    }
    rows.sort_by_key(|row| row.source_ordinal);
    Ok(PlannedAccountImport {
        rows,
        imports,
        already_present_count,
        conflict_count,
        unsupported_count,
    })
}

fn prune_pending_imports(pending: &mut HashMap<String, PendingAccountImport>, now: DateTime<Utc>) {
    pending.retain(|_, plan| plan.expires_at > now);
    if pending.len() > 8 {
        let mut tokens = pending
            .iter()
            .map(|(token, plan)| (token.clone(), plan.expires_at))
            .collect::<Vec<_>>();
        tokens.sort_by_key(|(_, expires_at)| *expires_at);
        for (token, _) in tokens.into_iter().take(pending.len() - 8) {
            pending.remove(&token);
        }
    }
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "账号迁移恢复文件没有父目录。".to_string())?;
    ensure_private_directory(parent)?;
    let temporary = parent.join(format!(".tmp-{}", Uuid::new_v4().hyphenated()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| "无法创建账号迁移恢复文件。".to_string())?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|_| "无法写入账号迁移恢复文件。".to_string())?;
        file.sync_all()
            .map_err(|_| "无法持久化账号迁移恢复文件。".to_string())?;
        crate::storage::replace_file(&temporary, path)
            .map_err(|_| "无法提交账号迁移恢复文件。".to_string())?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "无法持久化账号迁移恢复目录。".to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn write_json_private(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|_| "无法序列化账号迁移恢复状态。".to_string())?;
    write_private_atomic(path, &bytes)
}

pub(crate) fn read_json_private<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "账号迁移恢复状态不可读取。".to_string())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || file_mode(&metadata) != private_file_mode()
    {
        return Err("账号迁移恢复状态权限或类型不安全。".to_string());
    }
    let bytes = read_safe_file(path, MAX_JOURNAL_BYTES)?;
    serde_json::from_slice(&bytes).map_err(|_| "账号迁移恢复状态已损坏。".to_string())
}

fn operation_directory(root: &Path, operation_id: &str) -> Result<PathBuf, String> {
    let parsed = Uuid::parse_str(operation_id).map_err(|_| "账号迁移操作标识无效。".to_string())?;
    if parsed.hyphenated().to_string() != operation_id {
        return Err("账号迁移操作标识无效。".to_string());
    }
    Ok(root.join(OPERATIONS_DIRECTORY).join(operation_id))
}

fn create_journal(
    migration_root: &Path,
    pending: &PendingAccountImport,
) -> Result<(PathBuf, AccountMigrationJournal), String> {
    ensure_private_directory(migration_root)?;
    let operations = migration_root.join(OPERATIONS_DIRECTORY);
    ensure_private_directory(&operations)?;
    let operation_id = Uuid::new_v4().hyphenated().to_string();
    let operation_root = operation_directory(migration_root, &operation_id)?;
    ensure_private_directory(&operation_root)?;
    ensure_private_directory(&operation_root.join(STAGING_DIRECTORY))?;
    let created_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let journal = AccountMigrationJournal {
        schema_version: "1.0".to_string(),
        operation_id: operation_id.clone(),
        state: JournalState::Prepared,
        source_revision: pending.source_revision.clone(),
        created_at: created_at.clone(),
        updated_at: created_at,
        accounts: pending
            .imports
            .iter()
            .map(|account| JournalAccount {
                target_id: account.target_id.clone(),
                after_revision: account.after_revision.clone(),
                files: account
                    .files
                    .iter()
                    .map(|file| JournalFile {
                        name: file.name.to_string(),
                        revision: file.revision.clone(),
                    })
                    .collect(),
            })
            .collect(),
    };
    write_json_private(&operation_root.join(JOURNAL_FILE), &journal)?;
    write_json_private(
        &migration_root.join(LATEST_FILE),
        &LatestOperation {
            schema_version: "1.0".to_string(),
            operation_id,
        },
    )?;
    Ok((operation_root, journal))
}

fn write_journal(operation_root: &Path, journal: &AccountMigrationJournal) -> Result<(), String> {
    write_json_private(&operation_root.join(JOURNAL_FILE), journal)
}

fn update_journal_state(
    operation_root: &Path,
    journal: &mut AccountMigrationJournal,
    state: JournalState,
) -> Result<(), String> {
    journal.state = state;
    journal.updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    write_journal(operation_root, journal)
}

fn read_latest_journal(
    migration_root: &Path,
) -> Result<Option<(PathBuf, AccountMigrationJournal)>, String> {
    let latest_path = migration_root.join(LATEST_FILE);
    if !latest_path.exists() {
        return Ok(None);
    }
    let latest: LatestOperation = read_json_private(&latest_path)?;
    if latest.schema_version != "1.0" {
        return Err("账号迁移恢复指针版本不受支持。".to_string());
    }
    let operation_root = operation_directory(migration_root, &latest.operation_id)?;
    let journal: AccountMigrationJournal = read_json_private(&operation_root.join(JOURNAL_FILE))?;
    if journal.schema_version != "1.0" || journal.operation_id != latest.operation_id {
        return Err("账号迁移恢复状态身份不一致。".to_string());
    }
    validate_journal(&journal)?;
    Ok(Some((operation_root, journal)))
}

fn write_staged_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "无法创建账号迁移暂存文件。".to_string())?;
    file.write_all(bytes)
        .map_err(|_| "无法写入账号迁移暂存文件。".to_string())?;
    file.sync_all()
        .map_err(|_| "无法持久化账号迁移暂存文件。".to_string())?;
    harden_private_file(path)
}

fn stage_accounts(operation_root: &Path, imports: &[PreparedAccount]) -> Result<(), String> {
    let staging = operation_root.join(STAGING_DIRECTORY);
    for account in imports {
        let account_staging = staging.join(&account.target_id);
        ensure_private_directory(&account_staging)?;
        for file in &account.files {
            write_staged_file(&account_staging.join(file.name), &file.bytes)?;
        }
        #[cfg(unix)]
        File::open(&account_staging)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "无法持久化账号迁移暂存目录。".to_string())?;
    }
    Ok(())
}

fn install_staged_account(
    accounts_root: &Path,
    operation_root: &Path,
    account: &PreparedAccount,
) -> Result<(), String> {
    if !matches!(
        observe_account_directory(&accounts_root.join(&account.target_id))?,
        AccountDirectoryObservation::Missing
    ) {
        return Err("账号迁移目标在确认后已存在。".to_string());
    }
    let target = accounts_root.join(&account.target_id);
    fs::create_dir(&target).map_err(|_| "无法建立账号迁移目标目录。".to_string())?;
    ensure_private_directory(&target)?;
    let staged = operation_root
        .join(STAGING_DIRECTORY)
        .join(&account.target_id);
    for file in &account.files {
        crate::storage::replace_file(&staged.join(file.name), &target.join(file.name))
            .map_err(|_| "无法提交账号迁移目标文件。".to_string())?;
        harden_private_file(&target.join(file.name))?;
    }
    #[cfg(unix)]
    {
        File::open(&target)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "无法持久化账号迁移目标目录。".to_string())?;
        File::open(accounts_root)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "无法持久化账号目录。".to_string())?;
    }
    match observe_account_directory(&target)? {
        AccountDirectoryObservation::Present { revision } if revision == account.after_revision => {
            Ok(())
        }
        _ => Err("账号迁移写入后的校验结果不一致。".to_string()),
    }
}

fn validate_journal_file(file: &JournalFile) -> bool {
    matches!(file.name.as_str(), "auth.json" | "note.txt") && valid_sha256_revision(&file.revision)
}

fn validate_journal(journal: &AccountMigrationJournal) -> Result<(), String> {
    let canonical_operation_id = Uuid::parse_str(&journal.operation_id)
        .ok()
        .map(|value| value.hyphenated().to_string());
    let created_at = DateTime::parse_from_rfc3339(&journal.created_at).ok();
    let updated_at = DateTime::parse_from_rfc3339(&journal.updated_at).ok();
    if journal.schema_version != "1.0"
        || canonical_operation_id.as_deref() != Some(journal.operation_id.as_str())
        || !valid_sha256_revision(&journal.source_revision)
        || created_at.is_none()
        || updated_at.is_none()
        || updated_at < created_at
        || journal.accounts.is_empty()
        || journal.accounts.len() > 1_024
    {
        return Err("账号迁移恢复记录数量无效。".to_string());
    }
    let mut target_ids = std::collections::BTreeSet::new();
    for account in &journal.accounts {
        if !safe_target_id(&account.target_id)
            || !target_ids.insert(account.target_id.as_str())
            || !valid_sha256_revision(&account.after_revision)
            || account.files.is_empty()
            || account.files.len() > 2
            || !account.files.iter().all(validate_journal_file)
        {
            return Err("账号迁移恢复记录无效。".to_string());
        }
        let mut names = std::collections::BTreeSet::new();
        if !account
            .files
            .iter()
            .all(|file| names.insert(file.name.as_str()))
        {
            return Err("账号迁移恢复记录包含重复文件。".to_string());
        }
        if !names.contains("auth.json") {
            return Err("账号迁移恢复记录缺少认证文件。".to_string());
        }
        let mut revision_parts = account
            .files
            .iter()
            .map(|file| {
                (
                    file.name.clone(),
                    private_file_mode(),
                    file.revision.clone(),
                )
            })
            .collect::<Vec<_>>();
        revision_parts.sort_by(|left, right| left.0.cmp(&right.0));
        if account.after_revision
            != account_revision_from_parts(private_directory_mode(), &revision_parts)
        {
            return Err("账号迁移恢复记录摘要不一致。".to_string());
        }
    }
    Ok(())
}

fn cleanup_staging(operation_root: &Path, journal: &AccountMigrationJournal) -> Result<(), String> {
    let staging = operation_root.join(STAGING_DIRECTORY);
    let staging_metadata = match fs::symlink_metadata(&staging) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("无法检查账号迁移暂存区。".to_string()),
    };
    if staging_metadata.file_type().is_symlink()
        || !staging_metadata.is_dir()
        || file_mode(&staging_metadata) != private_directory_mode()
    {
        return Err("账号迁移暂存区不安全。".to_string());
    }
    let expected_accounts = journal
        .accounts
        .iter()
        .map(|account| (account.target_id.as_str(), account))
        .collect::<BTreeMap<_, _>>();
    let account_entries = fs::read_dir(&staging)
        .map_err(|_| "无法检查账号迁移暂存区。".to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "无法检查账号迁移暂存区。".to_string())?;
    if account_entries.len() > expected_accounts.len() {
        return Err("账号迁移暂存区包含意外内容。".to_string());
    }

    let mut files_to_remove = Vec::new();
    let mut directories_to_remove = Vec::new();
    for entry in account_entries {
        let target_id = entry
            .file_name()
            .into_string()
            .map_err(|_| "账号迁移暂存区包含无效目录。".to_string())?;
        let account = expected_accounts
            .get(target_id.as_str())
            .ok_or_else(|| "账号迁移暂存区包含意外目录。".to_string())?;
        let account_metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| "无法检查账号迁移暂存目录。".to_string())?;
        if account_metadata.file_type().is_symlink()
            || !account_metadata.is_dir()
            || file_mode(&account_metadata) != private_directory_mode()
        {
            return Err("账号迁移暂存目录不安全。".to_string());
        }
        let expected_files = account
            .files
            .iter()
            .map(|file| (file.name.as_str(), file.revision.as_str()))
            .collect::<BTreeMap<_, _>>();
        let entries = fs::read_dir(entry.path())
            .map_err(|_| "无法检查账号迁移暂存文件。".to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "无法检查账号迁移暂存文件。".to_string())?;
        if entries.len() > expected_files.len() {
            return Err("账号迁移暂存目录包含意外内容。".to_string());
        }
        for file in entries {
            let name = file
                .file_name()
                .into_string()
                .map_err(|_| "账号迁移暂存区包含无效文件名。".to_string())?;
            let expected_revision = expected_files
                .get(name.as_str())
                .ok_or_else(|| "账号迁移暂存目录包含意外文件。".to_string())?;
            let metadata = fs::symlink_metadata(file.path())
                .map_err(|_| "无法检查账号迁移暂存文件。".to_string())?;
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || file_mode(&metadata) != private_file_mode()
                || sha256_revision(&read_safe_file(&file.path(), MAX_ACCOUNT_FILE_BYTES)?)
                    != **expected_revision
            {
                return Err("账号迁移暂存文件已发生变化。".to_string());
            }
            files_to_remove.push(file.path());
        }
        directories_to_remove.push(entry.path());
    }

    for file in files_to_remove {
        fs::remove_file(file).map_err(|_| "无法清除账号迁移暂存文件。".to_string())?;
    }
    for directory in directories_to_remove {
        fs::remove_dir(directory).map_err(|_| "无法清除账号迁移暂存目录。".to_string())?;
    }
    fs::remove_dir(&staging).map_err(|_| "无法清除账号迁移暂存区。".to_string())?;
    #[cfg(unix)]
    File::open(operation_root)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "无法持久化账号迁移暂存清理。".to_string())?;
    Ok(())
}

fn remove_imported_account_if_safe(
    accounts_root: &Path,
    account: &JournalAccount,
    allow_partial: bool,
) -> Result<(), String> {
    if !safe_target_id(&account.target_id)
        || account.files.is_empty()
        || account.files.len() > 2
        || !account.files.iter().all(validate_journal_file)
    {
        return Err("账号迁移恢复记录无效。".to_string());
    }
    let target = accounts_root.join(&account.target_id);
    let observation = observe_account_directory(&target)?;
    let AccountDirectoryObservation::Present { revision } = observation else {
        return Ok(());
    };
    if !allow_partial && revision != account.after_revision {
        return Err("导入账号在迁移后已发生变化，回滚已安全停止。".to_string());
    }
    let expected = account
        .files
        .iter()
        .map(|file| (file.name.as_str(), file.revision.as_str()))
        .collect::<BTreeMap<_, _>>();
    let entries = fs::read_dir(&target)
        .map_err(|_| "无法检查待回滚账号。".to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "无法检查待回滚账号。".to_string())?;
    if entries.len() > expected.len() {
        return Err("导入账号在迁移后已增加数据，回滚已安全停止。".to_string());
    }
    if file_mode(&fs::symlink_metadata(&target).map_err(|_| "无法检查待回滚账号。".to_string())?)
        != private_directory_mode()
    {
        return Err("导入账号在迁移后已发生变化，回滚已安全停止。".to_string());
    }
    for entry in &entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "待回滚账号包含无效文件名。".to_string())?;
        let Some(expected_revision) = expected.get(name.as_str()) else {
            return Err("导入账号在迁移后已增加数据，回滚已安全停止。".to_string());
        };
        let metadata =
            fs::symlink_metadata(entry.path()).map_err(|_| "无法检查待回滚账号。".to_string())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || file_mode(&metadata) != private_file_mode()
            || sha256_revision(&read_safe_file(&entry.path(), MAX_ACCOUNT_FILE_BYTES)?)
                != **expected_revision
        {
            return Err("导入账号在迁移后已发生变化，回滚已安全停止。".to_string());
        }
    }
    for entry in entries {
        fs::remove_file(entry.path()).map_err(|_| "无法移除待回滚账号文件。".to_string())?;
    }
    fs::remove_dir(&target).map_err(|_| "无法移除待回滚账号目录。".to_string())?;
    Ok(())
}

fn rollback_journal(
    accounts_root: &Path,
    operation_root: &Path,
    journal: &mut AccountMigrationJournal,
    allow_partial: bool,
) -> Result<(), String> {
    let rollback = (|| {
        validate_journal(journal)?;
        for account in journal.accounts.iter().rev() {
            remove_imported_account_if_safe(accounts_root, account, allow_partial)?;
        }
        for account in &journal.accounts {
            if !matches!(
                observe_account_directory(&accounts_root.join(&account.target_id))?,
                AccountDirectoryObservation::Missing
            ) {
                return Err("账号迁移回滚后的状态校验失败。".to_string());
            }
        }
        cleanup_staging(operation_root, journal)?;
        Ok(())
    })();
    match rollback {
        Ok(()) => update_journal_state(operation_root, journal, JournalState::RolledBack),
        Err(error) => {
            let _ = update_journal_state(operation_root, journal, JournalState::NeedsReview);
            Err(error)
        }
    }
}

fn execute_import<F: FnMut() -> Result<(), String>>(
    pending: &PendingAccountImport,
    migration_root: &Path,
    mut revalidate: F,
) -> Result<LegacyAccountMigrationResult, String> {
    revalidate()?;
    for account in &pending.imports {
        if !matches!(
            observe_account_directory(&pending.accounts_root.join(&account.target_id))?,
            AccountDirectoryObservation::Missing
        ) {
            return Err("账号迁移目标在预览后发生变化，请重新预览。".to_string());
        }
    }
    ensure_private_directory(&pending.accounts_root)?;
    let (operation_root, mut journal) = create_journal(migration_root, pending)?;
    let apply = (|| {
        stage_accounts(&operation_root, &pending.imports)?;
        update_journal_state(&operation_root, &mut journal, JournalState::Applying)?;
        for account in &pending.imports {
            revalidate()?;
            install_staged_account(&pending.accounts_root, &operation_root, account)?;
        }
        revalidate()?;
        for account in &pending.imports {
            match observe_account_directory(&pending.accounts_root.join(&account.target_id))? {
                AccountDirectoryObservation::Present { revision }
                    if revision == account.after_revision => {}
                _ => return Err("账号迁移完整校验失败。".to_string()),
            }
        }
        cleanup_staging(&operation_root, &journal)?;
        update_journal_state(&operation_root, &mut journal, JournalState::Committed)
    })();
    if let Err(error) = apply {
        return match rollback_journal(&pending.accounts_root, &operation_root, &mut journal, true) {
            Ok(()) => Err(format!("{error} 已自动恢复迁移前状态。")),
            Err(rollback_error) => Err(format!("{error} {rollback_error}")),
        };
    }
    Ok(LegacyAccountMigrationResult {
        schema_version: "1.0",
        status: "applied",
        operation_id: Some(journal.operation_id),
        imported_count: journal.accounts.len(),
        already_present_count: pending.already_present_count,
        conflict_count: pending.conflict_count,
        unsupported_count: pending.unsupported_count,
        source_unchanged: true,
        current_login_preserved: true,
        rollback_available: true,
    })
}

pub(crate) fn prepare_account_import_blocking<R: Runtime>(
    app: &AppHandle<R>,
    source_path: PathBuf,
) -> Result<LegacyAccountMigrationPreview, String> {
    let bundle = read_importable_accounts(&source_path)
        .map_err(|_| "无法读取可迁移的 QuotaViewer 账号。".to_string())?;
    if bundle.accounts.is_empty() {
        return Err("所选目录没有可迁移的 Viewer 账号。".to_string());
    }
    let revision = source_revision(&bundle);
    let (accounts_root, _) = app_data_paths(app)?;
    let plan = plan_account_import(&bundle, &accounts_root)?;
    if plan.imports.is_empty() {
        let status = if plan.conflict_count > 0 || plan.unsupported_count > 0 {
            "review_only"
        } else {
            "already_applied"
        };
        return Ok(LegacyAccountMigrationPreview {
            schema_version: "1.0",
            status,
            confirm_token: None,
            expires_at: None,
            typed_confirmation: CONFIRMATION_PHRASE,
            accounts: plan.rows,
            import_count: 0,
            already_present_count: plan.already_present_count,
            conflict_count: plan.conflict_count,
            unsupported_count: plan.unsupported_count,
            preserves_current_login: true,
            imports_quota_cache: false,
            creates_restore_point: false,
            automatic_rollback: true,
        });
    }
    let now = Utc::now();
    let expires_at = now + Duration::seconds(CONFIRMATION_TTL_SECONDS);
    let token = Uuid::new_v4().hyphenated().to_string();
    let import_count = plan.imports.len();
    let pending_import = PendingAccountImport {
        source_path,
        source_revision: revision,
        accounts_root,
        imports: plan.imports,
        already_present_count: plan.already_present_count,
        conflict_count: plan.conflict_count,
        unsupported_count: plan.unsupported_count,
        expires_at,
    };
    let mut pending = PENDING_IMPORTS
        .lock()
        .map_err(|_| "账号迁移确认状态不可用。".to_string())?;
    prune_pending_imports(&mut pending, now);
    pending.insert(token.clone(), pending_import);
    Ok(LegacyAccountMigrationPreview {
        schema_version: "1.0",
        status: "confirmation_required",
        confirm_token: Some(token),
        expires_at: Some(expires_at.to_rfc3339_opts(SecondsFormat::Millis, true)),
        typed_confirmation: CONFIRMATION_PHRASE,
        accounts: plan.rows,
        import_count,
        already_present_count: plan.already_present_count,
        conflict_count: plan.conflict_count,
        unsupported_count: plan.unsupported_count,
        preserves_current_login: true,
        imports_quota_cache: false,
        creates_restore_point: true,
        automatic_rollback: true,
    })
}

#[tauri::command]
pub(crate) async fn prepare_legacy_quotaviewer_account_import<R: Runtime + 'static>(
    app: AppHandle<R>,
    source_path: Option<String>,
) -> Result<LegacyAccountMigrationPreview, String> {
    let source_path = super::legacy_migration::legacy_source_path(source_path)?;
    tauri::async_runtime::spawn_blocking(move || prepare_account_import_blocking(&app, source_path))
        .await
        .map_err(|_| "QuotaViewer 账号迁移预览任务意外结束。".to_string())?
}

fn consume_pending_import(token: &str) -> Result<PendingAccountImport, String> {
    if Uuid::parse_str(token).is_err() {
        return Err("账号迁移确认令牌无效。请重新预览。".to_string());
    }
    let now = Utc::now();
    let mut pending = PENDING_IMPORTS
        .lock()
        .map_err(|_| "账号迁移确认状态不可用。".to_string())?;
    prune_pending_imports(&mut pending, now);
    let plan = pending
        .remove(token)
        .ok_or_else(|| "账号迁移确认已过期或已使用。请重新预览。".to_string())?;
    if plan.expires_at <= now {
        return Err("账号迁移确认已过期。请重新预览。".to_string());
    }
    Ok(plan)
}

#[tauri::command]
pub(crate) async fn confirm_legacy_quotaviewer_account_import<R: Runtime + 'static>(
    app: AppHandle<R>,
    confirm_token: String,
    typed_confirmation: String,
) -> Result<LegacyAccountMigrationResult, String> {
    if typed_confirmation != CONFIRMATION_PHRASE {
        return Err("请输入 IMPORT ACCOUNTS 以确认账号迁移。".to_string());
    }
    let pending = consume_pending_import(&confirm_token)?;
    tauri::async_runtime::spawn_blocking(move || {
        let current = read_importable_accounts(&pending.source_path)
            .map_err(|_| "QuotaViewer 账号源在确认前已不可用。".to_string())?;
        if source_revision(&current) != pending.source_revision {
            return Err("QuotaViewer 账号在预览后发生变化。请重新预览。".to_string());
        }
        let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
        let _account_guard = crate::commands::account_switch_lock()
            .lock()
            .map_err(|_| "账号操作锁不可用。".to_string())?;
        let (_, migration_root) = app_data_paths(&app)?;
        let source_path = pending.source_path.clone();
        let expected_source_revision = pending.source_revision.clone();
        let result = execute_import(&pending, &migration_root, || {
            lease
                .revalidate(&mut legacy_probe)
                .map_err(|_| "账号迁移安全互斥已失效。".to_string())?;
            let current = read_importable_accounts(&source_path)
                .map_err(|_| "QuotaViewer 账号源在导入时已不可用。".to_string())?;
            if source_revision(&current) != expected_source_revision {
                return Err("QuotaViewer 账号在导入时发生变化。".to_string());
            }
            Ok(())
        });
        if result.is_ok() {
            let _ = app.emit("accounts-changed", ());
            crate::system_tray::refresh_menu(&app);
        }
        result
    })
    .await
    .map_err(|_| "QuotaViewer 账号迁移执行任务意外结束。".to_string())?
}

pub(crate) fn latest_operation_blocking<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<LegacyAccountMigrationOperationView, String> {
    let (_, migration_root) = app_data_paths(app)?;
    let Some((_, journal)) = read_latest_journal(&migration_root)? else {
        return Ok(LegacyAccountMigrationOperationView {
            schema_version: "1.0",
            status: "none",
            operation_id: None,
            imported_count: 0,
            rollback_available: false,
            recovery_required: false,
            typed_rollback: ROLLBACK_PHRASE,
        });
    };
    let status = match journal.state {
        JournalState::Prepared => "prepared",
        JournalState::Applying => "applying",
        JournalState::Committed => "committed",
        JournalState::RolledBack => "rolled_back",
        JournalState::NeedsReview => "needs_review",
    };
    Ok(LegacyAccountMigrationOperationView {
        schema_version: "1.0",
        status,
        operation_id: Some(journal.operation_id),
        imported_count: journal.accounts.len(),
        rollback_available: journal.state == JournalState::Committed,
        recovery_required: matches!(
            journal.state,
            JournalState::Prepared | JournalState::Applying | JournalState::NeedsReview
        ),
        typed_rollback: ROLLBACK_PHRASE,
    })
}

#[tauri::command]
pub(crate) async fn get_latest_legacy_account_migration_operation<R: Runtime + 'static>(
    app: AppHandle<R>,
) -> Result<LegacyAccountMigrationOperationView, String> {
    tauri::async_runtime::spawn_blocking(move || latest_operation_blocking(&app))
        .await
        .map_err(|_| "读取账号迁移恢复状态的任务意外结束。".to_string())?
}

#[tauri::command]
pub(crate) async fn rollback_legacy_quotaviewer_account_import<R: Runtime + 'static>(
    app: AppHandle<R>,
    operation_id: String,
    typed_confirmation: String,
) -> Result<LegacyAccountMigrationResult, String> {
    if typed_confirmation != ROLLBACK_PHRASE {
        return Err("请输入 ROLLBACK ACCOUNTS 以确认撤销。".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
        lease
            .revalidate(&mut legacy_probe)
            .map_err(|_| "账号迁移回滚安全互斥已失效。".to_string())?;
        let _account_guard = crate::commands::account_switch_lock()
            .lock()
            .map_err(|_| "账号操作锁不可用。".to_string())?;
        let (accounts_root, migration_root) = app_data_paths(&app)?;
        let Some((operation_root, mut journal)) = read_latest_journal(&migration_root)? else {
            return Err("没有可撤销的 Viewer 账号迁移。".to_string());
        };
        if journal.operation_id != operation_id || journal.state != JournalState::Committed {
            return Err("该 Viewer 账号迁移已不可撤销。".to_string());
        }
        let imported_count = journal.accounts.len();
        rollback_journal(&accounts_root, &operation_root, &mut journal, false)?;
        let _ = app.emit("accounts-changed", ());
        crate::system_tray::refresh_menu(&app);
        Ok(LegacyAccountMigrationResult {
            schema_version: "1.0",
            status: "rolled_back",
            operation_id: Some(operation_id),
            imported_count,
            already_present_count: 0,
            conflict_count: 0,
            unsupported_count: 0,
            source_unchanged: true,
            current_login_preserved: true,
            rollback_available: false,
        })
    })
    .await
    .map_err(|_| "QuotaViewer 账号迁移回滚任务意外结束。".to_string())?
}

/// Removes only exact files created by an interrupted account import. Existing
/// accounts and committed imports are never changed during startup recovery.
pub(crate) fn recover_interrupted_import<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    let (accounts_root, migration_root) = app_data_paths(app)?;
    let Some((operation_root, mut journal)) = read_latest_journal(&migration_root)? else {
        return Ok(false);
    };
    if !matches!(
        journal.state,
        JournalState::Prepared | JournalState::Applying
    ) {
        return Ok(false);
    }
    let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "启动时无法取得账号迁移恢复互斥。".to_string())?;
    rollback_journal(&accounts_root, &operation_root, &mut journal, true)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;

    fn jwt(payload: Value) -> String {
        format!(
            "e30.{}.sig",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
        )
    }

    fn auth(identity: &str, account_id: &str, marker: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "tokens": {
                "id_token": jwt(json!({
                    "sub": identity,
                    "email": format!("{identity}@example.test"),
                    "https://api.openai.com/auth": {
                        "chatgpt_account_id": account_id,
                        "chatgpt_plan_type": "pro"
                    }
                })),
                "access_token": format!("access-{marker}"),
                "refresh_token": format!("refresh-{marker}")
            },
            "last_refresh": "2026-09-04T00:00:00Z"
        }))
        .unwrap()
    }

    fn record(source_id: &str, identity: &str, account_id: &str) -> LegacyAccountImportRecord {
        LegacyAccountImportRecord {
            source_id: source_id.to_string(),
            display_name: format!("{identity}@example.test"),
            auth_mode: LegacyAccountAuthMode::Chatgpt,
            created_at: "2026-09-01T00:00:00Z".to_string(),
            last_used_at: None,
            auth_json: auth(identity, account_id, source_id),
            config_toml: None,
        }
    }

    fn plan_with_two_accounts(
        accounts_root: &Path,
    ) -> (LegacyAccountImportBundle, PlannedAccountImport) {
        let bundle = LegacyAccountImportBundle {
            preferred_source_id: Some("source-two".to_string()),
            accounts: vec![
                record("source-one", "person-one", "account-one"),
                record("source-two", "person-two", "account-two"),
            ],
        };
        let first = normalize_candidate(0, &bundle.accounts[0], None).unwrap();
        let first_target = accounts_root.join(first.target_id);
        ensure_private_directory(&first_target).unwrap();
        let mut existing: Value = serde_json::from_slice(&bundle.accounts[0].auth_json).unwrap();
        canonicalize_chatgpt_auth(&mut existing).unwrap();
        write_private_atomic(
            &first_target.join("auth.json"),
            &serde_json::to_vec_pretty(&existing).unwrap(),
        )
        .unwrap();
        let plan = plan_account_import(&bundle, accounts_root).unwrap();
        (bundle, plan)
    }

    fn pending(root: &Path) -> PendingAccountImport {
        let accounts_root = root.join("accounts");
        ensure_private_directory(&accounts_root).unwrap();
        let (bundle, plan) = plan_with_two_accounts(&accounts_root);
        assert_eq!(plan.imports.len(), 1);
        assert_eq!(plan.already_present_count, 1);
        PendingAccountImport {
            source_path: root.join("source"),
            source_revision: source_revision(&bundle),
            accounts_root,
            imports: plan.imports,
            already_present_count: plan.already_present_count,
            conflict_count: plan.conflict_count,
            unsupported_count: plan.unsupported_count,
            expires_at: Utc::now() + Duration::seconds(120),
        }
    }

    #[test]
    fn account_plan_imports_only_missing_identity_and_preserves_current_copy() {
        let root = tempfile::tempdir().unwrap();
        let accounts_root = root.path().join("accounts");
        ensure_private_directory(&accounts_root).unwrap();
        let (_, plan) = plan_with_two_accounts(&accounts_root);
        assert_eq!(plan.imports.len(), 1);
        assert_eq!(plan.already_present_count, 1);
        assert_eq!(plan.conflict_count, 0);
        assert_eq!(plan.unsupported_count, 0);
        assert_eq!(
            plan.rows
                .iter()
                .map(|row| row.disposition)
                .collect::<Vec<_>>(),
            vec!["already_present", "import"]
        );
        assert!(plan.rows[1].preferred);
    }

    #[test]
    fn account_plan_never_overwrites_a_different_existing_credential() {
        let root = tempfile::tempdir().unwrap();
        let accounts_root = root.path().join("accounts");
        ensure_private_directory(&accounts_root).unwrap();
        let source = record("source-one", "person-one", "account-one");
        let candidate = normalize_candidate(0, &source, None).unwrap();
        let target = accounts_root.join(&candidate.target_id);
        ensure_private_directory(&target).unwrap();
        let different = auth("person-one", "account-one", "newer-target-copy");
        let mut different: Value = serde_json::from_slice(&different).unwrap();
        canonicalize_chatgpt_auth(&mut different).unwrap();
        write_private_atomic(
            &target.join("auth.json"),
            &serde_json::to_vec_pretty(&different).unwrap(),
        )
        .unwrap();
        let bundle = LegacyAccountImportBundle {
            preferred_source_id: None,
            accounts: vec![source],
        };

        let plan = plan_account_import(&bundle, &accounts_root).unwrap();

        assert!(plan.imports.is_empty());
        assert_eq!(plan.conflict_count, 1);
        assert_eq!(plan.rows[0].disposition, "keep_existing");
        assert_eq!(
            plan.rows[0].reason_code,
            "legacy_target_credential_preserved"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(target.join("auth.json")).unwrap()).unwrap(),
            different
        );
    }

    #[test]
    fn account_import_is_verified_private_and_exactly_reversible() {
        let root = tempfile::tempdir().unwrap();
        let pending = pending(root.path());
        let migration_root = root.path().join(MIGRATION_DIRECTORY);
        let result = execute_import(&pending, &migration_root, || Ok(())).unwrap();
        assert_eq!(result.imported_count, 1);
        assert!(result.current_login_preserved);
        let (operation_root, mut journal) = read_latest_journal(&migration_root).unwrap().unwrap();
        assert!(!operation_root.join(STAGING_DIRECTORY).exists());
        let imported = pending.accounts_root.join(&pending.imports[0].target_id);
        assert!(imported.join("auth.json").is_file());
        #[cfg(unix)]
        {
            assert_eq!(
                file_mode(&fs::metadata(&pending.accounts_root).unwrap()),
                0o700
            );
            assert_eq!(file_mode(&fs::metadata(&imported).unwrap()), 0o700);
            assert_eq!(
                file_mode(&fs::metadata(imported.join("auth.json")).unwrap()),
                0o600
            );
        }
        let journal_text = fs::read_to_string(operation_root.join(JOURNAL_FILE)).unwrap();
        for canary in [
            "access-source-two",
            "refresh-source-two",
            "person-two@example.test",
        ] {
            assert!(!journal_text.contains(canary));
        }
        rollback_journal(&pending.accounts_root, &operation_root, &mut journal, false).unwrap();
        assert!(!imported.exists());
    }

    #[test]
    fn failed_multi_account_import_removes_only_exact_partial_outputs() {
        let root = tempfile::tempdir().unwrap();
        let accounts_root = root.path().join("accounts");
        ensure_private_directory(&accounts_root).unwrap();
        let bundle = LegacyAccountImportBundle {
            preferred_source_id: None,
            accounts: vec![
                record("source-one", "person-one", "account-one"),
                record("source-two", "person-two", "account-two"),
            ],
        };
        let plan = plan_account_import(&bundle, &accounts_root).unwrap();
        let pending = PendingAccountImport {
            source_path: root.path().join("source"),
            source_revision: source_revision(&bundle),
            accounts_root: accounts_root.clone(),
            imports: plan.imports,
            already_present_count: 0,
            conflict_count: 0,
            unsupported_count: 0,
            expires_at: Utc::now() + Duration::seconds(120),
        };
        let mut checks = 0;
        let error = execute_import(&pending, &root.path().join(MIGRATION_DIRECTORY), || {
            checks += 1;
            if checks == 3 {
                Err("injected lease loss".to_string())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(error.contains("已自动恢复迁移前状态"));
        for account in &pending.imports {
            assert!(!accounts_root.join(&account.target_id).exists());
        }
        let (operation_root, _) = read_latest_journal(&root.path().join(MIGRATION_DIRECTORY))
            .unwrap()
            .unwrap();
        assert!(!operation_root.join(STAGING_DIRECTORY).exists());
    }

    #[test]
    fn explicit_rollback_refuses_to_delete_an_account_changed_after_import() {
        let root = tempfile::tempdir().unwrap();
        let pending = pending(root.path());
        let migration_root = root.path().join(MIGRATION_DIRECTORY);
        execute_import(&pending, &migration_root, || Ok(())).unwrap();
        let imported = pending.accounts_root.join(&pending.imports[0].target_id);
        write_private_atomic(&imported.join("usage.json"), b"{}").unwrap();
        let (operation_root, mut journal) = read_latest_journal(&migration_root).unwrap().unwrap();
        let error = rollback_journal(&pending.accounts_root, &operation_root, &mut journal, false)
            .unwrap_err();
        assert!(error.contains("回滚已安全停止"));
        assert!(imported.exists());
        assert_eq!(journal.state, JournalState::NeedsReview);
    }

    #[test]
    fn journal_digest_must_match_its_exact_file_inventory() {
        let root = tempfile::tempdir().unwrap();
        let pending = pending(root.path());
        let migration_root = root.path().join(MIGRATION_DIRECTORY);
        let (_, mut journal) = create_journal(&migration_root, &pending).unwrap();
        assert!(validate_journal(&journal).is_ok());

        journal.accounts[0].files[0].revision = format!("sha256:{}", "0".repeat(64));
        assert!(validate_journal(&journal).is_err());
    }

    #[test]
    fn opted_in_real_viewer_plan_is_compatible_without_writing() {
        let (Ok(source), Ok(target)) = (
            std::env::var("QUOTAHORIZON_TEST_VIEWER_SOURCE"),
            std::env::var("QUOTAHORIZON_TEST_ACCOUNT_TARGET"),
        ) else {
            return;
        };
        let bundle = read_importable_accounts(Path::new(&source)).unwrap();
        let plan = plan_account_import(&bundle, Path::new(&target)).unwrap();
        assert_eq!(bundle.accounts.len(), 2);
        assert_eq!(plan.imports.len(), 1);
        assert_eq!(plan.unsupported_count, 0);
        assert_eq!(
            plan.imports.len() + plan.already_present_count + plan.conflict_count,
            bundle.accounts.len()
        );
    }
}
