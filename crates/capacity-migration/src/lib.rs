use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Component, Path};

use serde::Serialize;
use serde_json::{Map, Value};

pub const LEGACY_BASELINE_COMMIT: &str = "261fe1c";
pub const LEGACY_SOURCE_FORMAT: &str = "quotaviewer-1.2.0-120";

const SETTINGS_FILE: &str = "settings.json";
const ACCOUNTS_DIRECTORY: &str = "Accounts";
const ACCOUNTS_INDEX_FILE: &str = "accounts.json";
const QUOTA_CACHE_FILE: &str = "quota-cache.json";
const SESSION_SETTINGS_FILE: &str = "session-manager-ui.json";
const PROVIDER_MODE_FILE: &str = "chatgpt-provider-mode.json";
const RESTORE_POINTS_DIRECTORY: &str = "SwitchBackups";
const RESTORE_MANIFEST_FILE: &str = "manifest.json";

const MAX_SETTINGS_BYTES: u64 = 256 * 1024;
const MAX_ACCOUNT_INDEX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ACCOUNT_METADATA_BYTES: u64 = 256 * 1024;
const MAX_AUTH_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_QUOTA_CACHE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SESSION_SETTINGS_BYTES: u64 = 64 * 1024;
const MAX_PROVIDER_MODE_BYTES: u64 = 256 * 1024;
const MAX_RESTORE_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RESTORE_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ACCOUNT_RECORDS: usize = 1_024;
const MAX_QUOTA_RECORDS: usize = 4_096;
const MAX_POOL_RECORDS: usize = 4_096;
const MAX_RESTORE_POINTS: usize = 1_024;
const MAX_RESTORE_FILES: usize = 4_096;
const TARGET_OVERHEAD_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyDryRunStatus {
    Ready,
    Partial,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyArtifactKind {
    Settings,
    AccountIndex,
    AccountRecords,
    QuotaCache,
    SessionManagerSettings,
    ProviderMode,
    RestorePoints,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyArtifactState {
    Present,
    Missing,
    Invalid,
    Unsafe,
    LimitExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportTarget {
    MonitorSettings,
    ShellPreferences,
    WorkPlan,
    AccountVault,
    QuotaCache,
    SessionPreferences,
    ProviderModeState,
    RestorePointLedger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportDisposition {
    Import,
    Skip,
    Review,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportConflictKind {
    DuplicateAccountId,
    MissingAccountRecord,
    OrphanAccountRecord,
    InvalidAccountRecord,
    InvalidRestorePoint,
    ProviderRestorePointMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacySourceView {
    product: &'static str,
    format: Option<&'static str>,
    baseline_commit: &'static str,
    read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactInventory {
    pub kind: LegacyArtifactKind,
    pub state: LegacyArtifactState,
    pub file_count: u32,
    pub record_count: u32,
    pub source_bytes: u64,
    pub reason_codes: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlanAction {
    pub target: ImportTarget,
    pub disposition: ImportDisposition,
    pub source_item_count: u32,
    pub expected_target_count: u32,
    pub reason_code: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportConflict {
    pub kind: ImportConflictKind,
    pub count: u32,
    pub reason_code: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSpaceEstimate {
    pub source_bytes: u64,
    pub minimum_free_bytes: u64,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyDryRunReport {
    pub schema_version: &'static str,
    pub status: LegacyDryRunStatus,
    pub reason_codes: Vec<&'static str>,
    pub source: LegacySourceView,
    pub inventory: Vec<ArtifactInventory>,
    pub plan: Vec<ImportPlanAction>,
    pub conflicts: Vec<ImportConflict>,
    pub space_estimate: ImportSpaceEstimate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyStatusItemStyle {
    Meter,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyAppLanguage {
    System,
    English,
    Chinese,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyResolvedLanguage {
    English,
    Chinese,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyWorkPeriod {
    pub start_minute_of_day: u16,
    pub end_minute_of_day: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacySettingsPreferences {
    pub auto_refresh_enabled: bool,
    pub refresh_interval_seconds: u32,
    pub launch_at_login: bool,
    pub status_item_style: LegacyStatusItemStyle,
    pub language: LegacyAppLanguage,
    pub last_resolved_language: Option<LegacyResolvedLanguage>,
    pub work_plan_enabled: bool,
    pub off_periods: Vec<LegacyWorkPeriod>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyImportPreferences {
    pub settings: Option<LegacySettingsPreferences>,
    pub session_language: Option<LegacyResolvedLanguage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyAccountAuthMode {
    Chatgpt,
    ApiKey,
    Unknown,
}

/// Secret-bearing account material for a trusted host-side migration adapter.
///
/// This type deliberately does not implement `Serialize`: credentials must
/// never cross the desktop IPC boundary or enter a diagnostic payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyAccountImportRecord {
    pub source_id: String,
    pub display_name: String,
    pub auth_mode: LegacyAccountAuthMode,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub auth_json: Vec<u8>,
    pub config_toml: Option<Vec<u8>>,
}

/// A bounded, read-only snapshot of the legacy account vault. The caller must
/// validate each credential against its current runtime before importing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyAccountImportBundle {
    pub preferred_source_id: Option<String>,
    pub accounts: Vec<LegacyAccountImportRecord>,
}

impl LegacyDryRunReport {
    pub fn exit_code(&self) -> u8 {
        match self.status {
            LegacyDryRunStatus::Ready => 0,
            LegacyDryRunStatus::Partial => 2,
            LegacyDryRunStatus::Unsupported => 3,
            LegacyDryRunStatus::Failed => 5,
        }
    }
}

pub fn dry_run_quotaviewer(root: &Path) -> LegacyDryRunReport {
    if let Err(reason_code) = inspect_root(root) {
        return root_failure_report(reason_code);
    }

    let settings = scan_settings(&root.join(SETTINGS_FILE));
    let account_index =
        scan_account_index(&root.join(ACCOUNTS_DIRECTORY).join(ACCOUNTS_INDEX_FILE));
    let account_records =
        scan_account_records(&root.join(ACCOUNTS_DIRECTORY), &account_index.account_ids);
    let quota_cache = scan_quota_cache(&root.join(QUOTA_CACHE_FILE));
    let session_settings = scan_session_settings(&root.join(SESSION_SETTINGS_FILE));
    let provider_mode = scan_provider_mode(&root.join(PROVIDER_MODE_FILE));
    let restore_points = scan_restore_points(&root.join(RESTORE_POINTS_DIRECTORY));

    let mut conflict_counts = BTreeMap::new();
    add_conflict(
        &mut conflict_counts,
        ImportConflictKind::DuplicateAccountId,
        account_index.duplicate_count,
    );
    add_conflict(
        &mut conflict_counts,
        ImportConflictKind::MissingAccountRecord,
        account_records.missing_indexed_count,
    );
    add_conflict(
        &mut conflict_counts,
        ImportConflictKind::OrphanAccountRecord,
        account_records.orphan_count,
    );
    add_conflict(
        &mut conflict_counts,
        ImportConflictKind::InvalidAccountRecord,
        account_records.invalid_count,
    );
    add_conflict(
        &mut conflict_counts,
        ImportConflictKind::InvalidRestorePoint,
        restore_points.invalid_count,
    );
    if provider_mode
        .restore_point_id
        .as_ref()
        .is_some_and(|id| !restore_points.restore_point_ids.contains(id))
    {
        add_conflict(
            &mut conflict_counts,
            ImportConflictKind::ProviderRestorePointMissing,
            1,
        );
    }

    let inventory = vec![
        settings,
        account_index.inventory,
        account_records.inventory,
        quota_cache,
        session_settings,
        provider_mode.inventory,
        restore_points.inventory,
    ];
    let recognized = inventory
        .iter()
        .any(|artifact| artifact.state == LegacyArtifactState::Present);
    let has_invalid = inventory.iter().any(|artifact| {
        matches!(
            artifact.state,
            LegacyArtifactState::Invalid
                | LegacyArtifactState::Unsafe
                | LegacyArtifactState::LimitExceeded
        )
    });
    let status = if !recognized {
        LegacyDryRunStatus::Unsupported
    } else if has_invalid || !conflict_counts.is_empty() {
        LegacyDryRunStatus::Partial
    } else {
        LegacyDryRunStatus::Ready
    };
    let reason_codes = match status {
        LegacyDryRunStatus::Ready => vec!["legacy_inventory_ready"],
        LegacyDryRunStatus::Partial => vec!["legacy_inventory_partial"],
        LegacyDryRunStatus::Unsupported if has_invalid => {
            vec!["legacy_format_unrecognized"]
        }
        LegacyDryRunStatus::Unsupported => vec!["legacy_artifacts_not_found"],
        LegacyDryRunStatus::Failed => unreachable!("root failures return before scanning"),
    };
    let plan = build_plan(&inventory);
    let conflicts = conflict_counts
        .into_iter()
        .map(|(kind, count)| ImportConflict {
            kind,
            count,
            reason_code: conflict_reason(kind),
        })
        .collect();
    let source_bytes = inventory.iter().fold(0_u64, |total, artifact| {
        total.saturating_add(artifact.source_bytes)
    });

    LegacyDryRunReport {
        schema_version: "1.0",
        status,
        reason_codes,
        source: LegacySourceView {
            product: "codex_quota_viewer",
            format: recognized.then_some(LEGACY_SOURCE_FORMAT),
            baseline_commit: LEGACY_BASELINE_COMMIT,
            read_only: true,
        },
        inventory,
        plan,
        conflicts,
        space_estimate: ImportSpaceEstimate {
            source_bytes,
            minimum_free_bytes: source_bytes
                .saturating_mul(2)
                .saturating_add(TARGET_OVERHEAD_BYTES),
            status: "target_preflight_required",
        },
    }
}

/// Reads only the non-secret legacy preferences that have a direct 1.0 target.
///
/// This reuses the same bounded, symlink-rejecting reads and strict allowlists
/// as the inventory scanner. Account credentials, quota cache payloads,
/// Provider state, and restore-point contents are intentionally inaccessible
/// through this API.
pub fn read_importable_preferences(root: &Path) -> Result<LegacyImportPreferences, &'static str> {
    inspect_root(root)?;
    let settings = match read_json_bounded(&root.join(SETTINGS_FILE), MAX_SETTINGS_BYTES) {
        JsonRead::Missing => None,
        JsonRead::Value(value, _) if validate_settings(&value) => {
            Some(parse_settings_preferences(&value))
        }
        JsonRead::Value(_, _) => return Err("legacy_settings_invalid"),
        problem => return Err(problem.reason_code()),
    };
    let session_language = match read_json_bounded(
        &root.join(SESSION_SETTINGS_FILE),
        MAX_SESSION_SETTINGS_BYTES,
    ) {
        JsonRead::Missing => None,
        JsonRead::Value(value, _) if validate_session_settings(&value) => value
            .get("language")
            .and_then(Value::as_str)
            .map(parse_resolved_language),
        JsonRead::Value(_, _) => return Err("legacy_session_settings_invalid"),
        problem => return Err(problem.reason_code()),
    };
    Ok(LegacyImportPreferences {
        settings,
        session_language,
    })
}

/// Reads a consistent legacy account-vault snapshot without mutating or
/// following links from the source tree.
///
/// The returned credentials are intentionally host-only material. This
/// function validates the Viewer container/index contract; the desktop
/// adapter remains responsible for validating the current Codex auth format
/// and deciding how a target-side identity conflict is resolved.
pub fn read_importable_accounts(root: &Path) -> Result<LegacyAccountImportBundle, &'static str> {
    inspect_root(root)?;
    let accounts_root = root.join(ACCOUNTS_DIRECTORY);
    let index_path = accounts_root.join(ACCOUNTS_INDEX_FILE);
    let index_scan = scan_account_index(&index_path);
    if index_scan.inventory.state == LegacyArtifactState::Missing {
        let records = scan_account_records(&accounts_root, &BTreeSet::new());
        return if records.inventory.state == LegacyArtifactState::Missing {
            Ok(LegacyAccountImportBundle {
                preferred_source_id: read_preferred_account_id(root)?,
                accounts: Vec::new(),
            })
        } else {
            Err("legacy_account_index_missing")
        };
    }
    if index_scan.inventory.state != LegacyArtifactState::Present {
        return Err(index_scan
            .inventory
            .reason_codes
            .first()
            .copied()
            .unwrap_or("legacy_account_index_invalid"));
    }
    if index_scan.duplicate_count != 0 {
        return Err("legacy_duplicate_account_id");
    }
    let records_scan = scan_account_records(&accounts_root, &index_scan.account_ids);
    if records_scan.inventory.state != LegacyArtifactState::Present
        || records_scan.missing_indexed_count != 0
        || records_scan.orphan_count != 0
        || records_scan.invalid_count != 0
    {
        return Err("legacy_account_records_invalid");
    }

    let index = match read_json_bounded(&index_path, MAX_ACCOUNT_INDEX_BYTES) {
        JsonRead::Value(Value::Array(records), _) => records,
        _ => return Err("legacy_account_index_changed_during_read"),
    };
    let mut accounts = Vec::with_capacity(index.len());
    for indexed_metadata in index {
        let source_id = validate_account_metadata(&indexed_metadata)
            .ok_or("legacy_account_index_changed_during_read")?;
        let record_root = accounts_root.join(&source_id);
        let record_metadata = match read_json_bounded(
            &record_root.join("metadata.json"),
            MAX_ACCOUNT_METADATA_BYTES,
        ) {
            JsonRead::Value(value, _) => value,
            _ => return Err("legacy_account_record_changed_during_read"),
        };
        if record_metadata != indexed_metadata
            || validate_account_metadata(&record_metadata).as_deref() != Some(source_id.as_str())
        {
            return Err("legacy_account_metadata_mismatch");
        }
        let metadata = record_metadata
            .as_object()
            .expect("validated account metadata is an object");
        let auth_json = read_regular_file_bytes(&record_root.join("auth.json"), MAX_AUTH_BYTES)?;
        if !serde_json::from_slice::<Value>(&auth_json).is_ok_and(|value| value.is_object()) {
            return Err("legacy_account_auth_invalid");
        }
        let config_toml =
            read_optional_regular_file_bytes(&record_root.join("config.toml"), MAX_CONFIG_BYTES)?;
        accounts.push(LegacyAccountImportRecord {
            source_id,
            display_name: metadata["displayName"]
                .as_str()
                .expect("validated display name")
                .to_owned(),
            auth_mode: match metadata["authMode"].as_str().expect("validated auth mode") {
                "chatgpt" => LegacyAccountAuthMode::Chatgpt,
                "apiKey" => LegacyAccountAuthMode::ApiKey,
                _ => LegacyAccountAuthMode::Unknown,
            },
            created_at: metadata["createdAt"]
                .as_str()
                .expect("validated created timestamp")
                .to_owned(),
            last_used_at: metadata
                .get("lastUsedAt")
                .and_then(Value::as_str)
                .map(str::to_owned),
            auth_json,
            config_toml,
        });
    }
    Ok(LegacyAccountImportBundle {
        preferred_source_id: read_preferred_account_id(root)?,
        accounts,
    })
}

fn read_preferred_account_id(root: &Path) -> Result<Option<String>, &'static str> {
    match read_json_bounded(&root.join(SETTINGS_FILE), MAX_SETTINGS_BYTES) {
        JsonRead::Missing => Ok(None),
        JsonRead::Value(value, _) if validate_settings(&value) => Ok(value
            .get("preferredAccountID")
            .and_then(Value::as_str)
            .map(str::to_owned)),
        JsonRead::Value(_, _) => Err("legacy_settings_invalid"),
        problem => Err(problem.reason_code()),
    }
}

fn root_failure_report(reason_code: &'static str) -> LegacyDryRunReport {
    let inventory = all_missing_inventory();
    LegacyDryRunReport {
        schema_version: "1.0",
        status: LegacyDryRunStatus::Failed,
        reason_codes: vec![reason_code],
        source: LegacySourceView {
            product: "codex_quota_viewer",
            format: None,
            baseline_commit: LEGACY_BASELINE_COMMIT,
            read_only: true,
        },
        plan: build_plan(&inventory),
        conflicts: Vec::new(),
        space_estimate: ImportSpaceEstimate {
            source_bytes: 0,
            minimum_free_bytes: 0,
            status: "unavailable",
        },
        inventory,
    }
}

fn inspect_root(root: &Path) -> Result<(), &'static str> {
    let metadata = fs::symlink_metadata(root).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "legacy_root_missing",
        _ => "legacy_root_unavailable",
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("legacy_root_unsafe");
    }
    Ok(())
}

fn scan_settings(path: &Path) -> ArtifactInventory {
    scan_single_json(
        LegacyArtifactKind::Settings,
        path,
        MAX_SETTINGS_BYTES,
        validate_settings,
        "legacy_settings_valid",
    )
}

fn scan_session_settings(path: &Path) -> ArtifactInventory {
    scan_single_json(
        LegacyArtifactKind::SessionManagerSettings,
        path,
        MAX_SESSION_SETTINGS_BYTES,
        validate_session_settings,
        "legacy_session_settings_valid",
    )
}

fn scan_quota_cache(path: &Path) -> ArtifactInventory {
    scan_json_array(
        LegacyArtifactKind::QuotaCache,
        path,
        MAX_QUOTA_CACHE_BYTES,
        MAX_QUOTA_RECORDS,
        validate_quota_record,
        "legacy_quota_cache_valid",
    )
}

struct AccountIndexScan {
    inventory: ArtifactInventory,
    account_ids: BTreeSet<String>,
    duplicate_count: u32,
}

fn scan_account_index(path: &Path) -> AccountIndexScan {
    let read = read_json_bounded(path, MAX_ACCOUNT_INDEX_BYTES);
    let mut account_ids = BTreeSet::new();
    let mut duplicate_count = 0_u32;
    let inventory = match read {
        JsonRead::Missing => ArtifactInventory::missing(LegacyArtifactKind::AccountIndex),
        JsonRead::Invalid(reason_code, bytes)
        | JsonRead::Unsafe(reason_code, bytes)
        | JsonRead::LimitExceeded(reason_code, bytes) => ArtifactInventory::problem(
            LegacyArtifactKind::AccountIndex,
            problem_state(&read),
            bytes,
            reason_code,
        ),
        JsonRead::Value(value, bytes) => {
            let Some(records) = value.as_array() else {
                return AccountIndexScan {
                    inventory: ArtifactInventory::invalid(
                        LegacyArtifactKind::AccountIndex,
                        bytes,
                        "legacy_account_index_invalid",
                    ),
                    account_ids,
                    duplicate_count,
                };
            };
            if records.len() > MAX_ACCOUNT_RECORDS {
                return AccountIndexScan {
                    inventory: ArtifactInventory::limit(
                        LegacyArtifactKind::AccountIndex,
                        bytes,
                        "legacy_account_limit_exceeded",
                    ),
                    account_ids,
                    duplicate_count,
                };
            }
            let mut invalid_count = 0_u32;
            for record in records {
                if let Some(id) = validate_account_metadata(record) {
                    if !account_ids.insert(id) {
                        duplicate_count = duplicate_count.saturating_add(1);
                    }
                } else {
                    invalid_count = invalid_count.saturating_add(1);
                }
            }
            if invalid_count == 0 {
                ArtifactInventory::present(
                    LegacyArtifactKind::AccountIndex,
                    1,
                    usize_to_u32(records.len()),
                    bytes,
                    "legacy_account_index_valid",
                )
            } else {
                ArtifactInventory {
                    kind: LegacyArtifactKind::AccountIndex,
                    state: LegacyArtifactState::Invalid,
                    file_count: 1,
                    record_count: usize_to_u32(
                        records.len().saturating_sub(invalid_count as usize),
                    ),
                    source_bytes: bytes,
                    reason_codes: vec!["legacy_account_index_invalid"],
                }
            }
        }
    };
    AccountIndexScan {
        inventory,
        account_ids,
        duplicate_count,
    }
}

struct AccountRecordsScan {
    inventory: ArtifactInventory,
    missing_indexed_count: u32,
    orphan_count: u32,
    invalid_count: u32,
}

fn scan_account_records(root: &Path, indexed_ids: &BTreeSet<String>) -> AccountRecordsScan {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return AccountRecordsScan {
                inventory: ArtifactInventory::missing(LegacyArtifactKind::AccountRecords),
                missing_indexed_count: usize_to_u32(indexed_ids.len()),
                orphan_count: 0,
                invalid_count: 0,
            };
        }
        Err(_) => {
            return AccountRecordsScan {
                inventory: ArtifactInventory::problem(
                    LegacyArtifactKind::AccountRecords,
                    LegacyArtifactState::Unsafe,
                    0,
                    "legacy_accounts_unavailable",
                ),
                missing_indexed_count: usize_to_u32(indexed_ids.len()),
                orphan_count: 0,
                invalid_count: 1,
            };
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return AccountRecordsScan {
            inventory: ArtifactInventory::problem(
                LegacyArtifactKind::AccountRecords,
                LegacyArtifactState::Unsafe,
                0,
                "legacy_accounts_unsafe",
            ),
            missing_indexed_count: usize_to_u32(indexed_ids.len()),
            orphan_count: 0,
            invalid_count: 1,
        };
    }

    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => {
            return AccountRecordsScan {
                inventory: ArtifactInventory::problem(
                    LegacyArtifactKind::AccountRecords,
                    LegacyArtifactState::Unsafe,
                    0,
                    "legacy_accounts_unavailable",
                ),
                missing_indexed_count: usize_to_u32(indexed_ids.len()),
                orphan_count: 0,
                invalid_count: 1,
            };
        }
    };

    let mut directory_ids = BTreeSet::new();
    let mut valid_count = 0_u32;
    let mut invalid_count = 0_u32;
    let mut file_count = 0_u32;
    let mut source_bytes = 0_u64;
    let mut directory_count = 0_usize;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == ACCOUNTS_INDEX_FILE || name.to_string_lossy().starts_with('.') {
            continue;
        }
        directory_count = directory_count.saturating_add(1);
        if directory_count > MAX_ACCOUNT_RECORDS {
            return AccountRecordsScan {
                inventory: ArtifactInventory::limit(
                    LegacyArtifactKind::AccountRecords,
                    source_bytes,
                    "legacy_account_limit_exceeded",
                ),
                missing_indexed_count: usize_to_u32(indexed_ids.len()),
                orphan_count: 0,
                invalid_count: invalid_count.saturating_add(1),
            };
        }
        let id = name.to_string_lossy().into_owned();
        let path = entry.path();
        let safe_directory = fs::symlink_metadata(&path)
            .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_dir());
        if !safe_directory || !safe_component(&id, 128) {
            invalid_count = invalid_count.saturating_add(1);
            continue;
        }
        directory_ids.insert(id.clone());
        match scan_account_record(&path, &id) {
            Some((files, bytes)) => {
                valid_count = valid_count.saturating_add(1);
                file_count = file_count.saturating_add(files);
                source_bytes = source_bytes.saturating_add(bytes);
            }
            None => invalid_count = invalid_count.saturating_add(1),
        }
    }

    let missing_indexed_count = usize_to_u32(indexed_ids.difference(&directory_ids).count());
    let orphan_count = usize_to_u32(directory_ids.difference(indexed_ids).count());
    let state = if invalid_count == 0 {
        LegacyArtifactState::Present
    } else {
        LegacyArtifactState::Invalid
    };
    let reason = if invalid_count == 0 {
        "legacy_account_records_valid"
    } else {
        "legacy_account_records_invalid"
    };
    AccountRecordsScan {
        inventory: ArtifactInventory {
            kind: LegacyArtifactKind::AccountRecords,
            state,
            file_count,
            record_count: valid_count,
            source_bytes,
            reason_codes: vec![reason],
        },
        missing_indexed_count,
        orphan_count,
        invalid_count,
    }
}

fn scan_account_record(root: &Path, expected_id: &str) -> Option<(u32, u64)> {
    let metadata_path = root.join("metadata.json");
    let (metadata_value, metadata_bytes) =
        match read_json_bounded(&metadata_path, MAX_ACCOUNT_METADATA_BYTES) {
            JsonRead::Value(value, bytes) => (value, bytes),
            _ => return None,
        };
    if validate_account_metadata(&metadata_value).as_deref() != Some(expected_id) {
        return None;
    }
    let auth_bytes = match inspect_regular_file(&root.join("auth.json"), MAX_AUTH_BYTES) {
        RegularFileState::Present(bytes) => bytes,
        _ => return None,
    };
    let (config_files, config_bytes) =
        match inspect_regular_file(&root.join("config.toml"), MAX_CONFIG_BYTES) {
            RegularFileState::Missing => (0, 0),
            RegularFileState::Present(bytes) => (1, bytes),
            RegularFileState::Unsafe | RegularFileState::LimitExceeded => return None,
        };
    Some((
        2_u32.saturating_add(config_files),
        metadata_bytes
            .saturating_add(auth_bytes)
            .saturating_add(config_bytes),
    ))
}

struct ProviderModeScan {
    inventory: ArtifactInventory,
    restore_point_id: Option<String>,
}

fn scan_provider_mode(path: &Path) -> ProviderModeScan {
    match read_json_bounded(path, MAX_PROVIDER_MODE_BYTES) {
        JsonRead::Missing => ProviderModeScan {
            inventory: ArtifactInventory::missing(LegacyArtifactKind::ProviderMode),
            restore_point_id: None,
        },
        JsonRead::Value(value, bytes) => {
            if let Some(restore_point_id) = validate_provider_mode(&value) {
                ProviderModeScan {
                    inventory: ArtifactInventory::present(
                        LegacyArtifactKind::ProviderMode,
                        1,
                        1,
                        bytes,
                        "legacy_provider_mode_valid",
                    ),
                    restore_point_id: Some(restore_point_id),
                }
            } else {
                ProviderModeScan {
                    inventory: ArtifactInventory::invalid(
                        LegacyArtifactKind::ProviderMode,
                        bytes,
                        "legacy_provider_mode_invalid",
                    ),
                    restore_point_id: None,
                }
            }
        }
        read => ProviderModeScan {
            inventory: ArtifactInventory::problem(
                LegacyArtifactKind::ProviderMode,
                problem_state(&read),
                read.bytes(),
                read.reason_code(),
            ),
            restore_point_id: None,
        },
    }
}

struct RestorePointsScan {
    inventory: ArtifactInventory,
    restore_point_ids: BTreeSet<String>,
    invalid_count: u32,
}

fn scan_restore_points(root: &Path) -> RestorePointsScan {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return RestorePointsScan {
                inventory: ArtifactInventory::missing(LegacyArtifactKind::RestorePoints),
                restore_point_ids: BTreeSet::new(),
                invalid_count: 0,
            };
        }
        Err(_) => {
            return RestorePointsScan {
                inventory: ArtifactInventory::problem(
                    LegacyArtifactKind::RestorePoints,
                    LegacyArtifactState::Unsafe,
                    0,
                    "legacy_restore_points_unavailable",
                ),
                restore_point_ids: BTreeSet::new(),
                invalid_count: 1,
            };
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return RestorePointsScan {
            inventory: ArtifactInventory::problem(
                LegacyArtifactKind::RestorePoints,
                LegacyArtifactState::Unsafe,
                0,
                "legacy_restore_points_unsafe",
            ),
            restore_point_ids: BTreeSet::new(),
            invalid_count: 1,
        };
    }

    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => {
            return RestorePointsScan {
                inventory: ArtifactInventory::problem(
                    LegacyArtifactKind::RestorePoints,
                    LegacyArtifactState::Unsafe,
                    0,
                    "legacy_restore_points_unavailable",
                ),
                restore_point_ids: BTreeSet::new(),
                invalid_count: 1,
            };
        }
    };
    let mut ids = BTreeSet::new();
    let mut invalid_count = 0_u32;
    let mut file_count = 0_u32;
    let mut source_bytes = 0_u64;
    let mut count = 0_usize;
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        if id.starts_with('.') {
            continue;
        }
        count = count.saturating_add(1);
        if count > MAX_RESTORE_POINTS {
            return RestorePointsScan {
                inventory: ArtifactInventory::limit(
                    LegacyArtifactKind::RestorePoints,
                    source_bytes,
                    "legacy_restore_point_limit_exceeded",
                ),
                restore_point_ids: ids,
                invalid_count: invalid_count.saturating_add(1),
            };
        }
        let directory = entry.path();
        let safe_directory = fs::symlink_metadata(&directory)
            .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_dir());
        if !safe_directory || !safe_component(&id, 128) {
            invalid_count = invalid_count.saturating_add(1);
            continue;
        }
        match scan_restore_point(&directory, &id) {
            Some((files, bytes)) => {
                ids.insert(id);
                file_count = file_count.saturating_add(files);
                source_bytes = source_bytes.saturating_add(bytes);
            }
            None => invalid_count = invalid_count.saturating_add(1),
        }
    }
    let state = if invalid_count == 0 {
        LegacyArtifactState::Present
    } else {
        LegacyArtifactState::Invalid
    };
    RestorePointsScan {
        inventory: ArtifactInventory {
            kind: LegacyArtifactKind::RestorePoints,
            state,
            file_count,
            record_count: usize_to_u32(ids.len()),
            source_bytes,
            reason_codes: vec![if invalid_count == 0 {
                "legacy_restore_points_valid"
            } else {
                "legacy_restore_points_invalid"
            }],
        },
        restore_point_ids: ids,
        invalid_count,
    }
}

fn scan_restore_point(root: &Path, expected_id: &str) -> Option<(u32, u64)> {
    let (value, manifest_bytes) = match read_json_bounded(
        &root.join(RESTORE_MANIFEST_FILE),
        MAX_RESTORE_MANIFEST_BYTES,
    ) {
        JsonRead::Value(value, bytes) => (value, bytes),
        _ => return None,
    };
    let backup_files = validate_restore_manifest(&value, expected_id)?;
    let mut file_count = 1_u32;
    let mut total_bytes = manifest_bytes;
    for (relative_path, expected_size) in backup_files {
        let path = root.join(&relative_path);
        let size = match inspect_regular_file(&path, MAX_RESTORE_FILE_BYTES) {
            RegularFileState::Present(size) => size,
            _ => return None,
        };
        if expected_size.is_some_and(|expected| expected != size) {
            return None;
        }
        file_count = file_count.saturating_add(1);
        total_bytes = total_bytes.saturating_add(size);
    }
    Some((file_count, total_bytes))
}

fn scan_single_json(
    kind: LegacyArtifactKind,
    path: &Path,
    maximum_bytes: u64,
    validator: fn(&Value) -> bool,
    ready_reason: &'static str,
) -> ArtifactInventory {
    match read_json_bounded(path, maximum_bytes) {
        JsonRead::Missing => ArtifactInventory::missing(kind),
        JsonRead::Value(value, bytes) if validator(&value) => {
            ArtifactInventory::present(kind, 1, 1, bytes, ready_reason)
        }
        JsonRead::Value(_, bytes) => {
            ArtifactInventory::invalid(kind, bytes, "legacy_json_shape_invalid")
        }
        read => {
            ArtifactInventory::problem(kind, problem_state(&read), read.bytes(), read.reason_code())
        }
    }
}

fn scan_json_array(
    kind: LegacyArtifactKind,
    path: &Path,
    maximum_bytes: u64,
    maximum_records: usize,
    validator: fn(&Value) -> bool,
    ready_reason: &'static str,
) -> ArtifactInventory {
    match read_json_bounded(path, maximum_bytes) {
        JsonRead::Missing => ArtifactInventory::missing(kind),
        JsonRead::Value(value, bytes) => {
            let Some(records) = value.as_array() else {
                return ArtifactInventory::invalid(kind, bytes, "legacy_json_shape_invalid");
            };
            if records.len() > maximum_records {
                return ArtifactInventory::limit(kind, bytes, "legacy_record_limit_exceeded");
            }
            if records.iter().all(validator) {
                ArtifactInventory::present(
                    kind,
                    1,
                    usize_to_u32(records.len()),
                    bytes,
                    ready_reason,
                )
            } else {
                ArtifactInventory::invalid(kind, bytes, "legacy_json_shape_invalid")
            }
        }
        read => {
            ArtifactInventory::problem(kind, problem_state(&read), read.bytes(), read.reason_code())
        }
    }
}

impl ArtifactInventory {
    fn missing(kind: LegacyArtifactKind) -> Self {
        Self {
            kind,
            state: LegacyArtifactState::Missing,
            file_count: 0,
            record_count: 0,
            source_bytes: 0,
            reason_codes: vec!["legacy_artifact_not_present"],
        }
    }

    fn present(
        kind: LegacyArtifactKind,
        file_count: u32,
        record_count: u32,
        source_bytes: u64,
        reason_code: &'static str,
    ) -> Self {
        Self {
            kind,
            state: LegacyArtifactState::Present,
            file_count,
            record_count,
            source_bytes,
            reason_codes: vec![reason_code],
        }
    }

    fn invalid(kind: LegacyArtifactKind, source_bytes: u64, reason_code: &'static str) -> Self {
        Self::problem(
            kind,
            LegacyArtifactState::Invalid,
            source_bytes,
            reason_code,
        )
    }

    fn limit(kind: LegacyArtifactKind, source_bytes: u64, reason_code: &'static str) -> Self {
        Self::problem(
            kind,
            LegacyArtifactState::LimitExceeded,
            source_bytes,
            reason_code,
        )
    }

    fn problem(
        kind: LegacyArtifactKind,
        state: LegacyArtifactState,
        source_bytes: u64,
        reason_code: &'static str,
    ) -> Self {
        Self {
            kind,
            state,
            file_count: u32::from(source_bytes > 0),
            record_count: 0,
            source_bytes,
            reason_codes: vec![reason_code],
        }
    }
}

enum JsonRead {
    Missing,
    Value(Value, u64),
    Invalid(&'static str, u64),
    Unsafe(&'static str, u64),
    LimitExceeded(&'static str, u64),
}

impl JsonRead {
    fn bytes(&self) -> u64 {
        match self {
            Self::Missing => 0,
            Self::Value(_, bytes)
            | Self::Invalid(_, bytes)
            | Self::Unsafe(_, bytes)
            | Self::LimitExceeded(_, bytes) => *bytes,
        }
    }

    fn reason_code(&self) -> &'static str {
        match self {
            Self::Missing => "legacy_artifact_not_present",
            Self::Value(_, _) => "legacy_json_valid",
            Self::Invalid(reason, _) | Self::Unsafe(reason, _) | Self::LimitExceeded(reason, _) => {
                reason
            }
        }
    }
}

fn problem_state(read: &JsonRead) -> LegacyArtifactState {
    match read {
        JsonRead::Unsafe(_, _) => LegacyArtifactState::Unsafe,
        JsonRead::LimitExceeded(_, _) => LegacyArtifactState::LimitExceeded,
        JsonRead::Invalid(_, _) => LegacyArtifactState::Invalid,
        JsonRead::Missing => LegacyArtifactState::Missing,
        JsonRead::Value(_, _) => LegacyArtifactState::Present,
    }
}

fn read_json_bounded(path: &Path, maximum_bytes: u64) -> JsonRead {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return JsonRead::Missing,
        Err(_) => return JsonRead::Unsafe("legacy_file_unavailable", 0),
    };
    if before.file_type().is_symlink() || !before.is_file() {
        return JsonRead::Unsafe("legacy_file_unsafe", 0);
    }
    if before.len() > maximum_bytes {
        return JsonRead::LimitExceeded("legacy_file_too_large", before.len());
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return JsonRead::Unsafe("legacy_file_unavailable", before.len()),
    };
    let opened = match file.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return JsonRead::Unsafe("legacy_file_unsafe", before.len()),
    };
    let mut bytes = Vec::new();
    if file
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .is_err()
    {
        return JsonRead::Invalid("legacy_file_read_failed", opened.len());
    }
    if bytes.len() as u64 > maximum_bytes {
        return JsonRead::LimitExceeded("legacy_file_too_large", bytes.len() as u64);
    }
    let after = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return JsonRead::Unsafe("legacy_file_changed_during_read", bytes.len() as u64),
    };
    if after.file_type().is_symlink() || !after.is_file() || !same_file_identity(&opened, &after) {
        return JsonRead::Unsafe("legacy_file_changed_during_read", bytes.len() as u64);
    }
    match serde_json::from_slice(&bytes) {
        Ok(value) => JsonRead::Value(value, bytes.len() as u64),
        Err(_) => JsonRead::Invalid("legacy_json_invalid", bytes.len() as u64),
    }
}

enum RegularFileState {
    Missing,
    Present(u64),
    Unsafe,
    LimitExceeded,
}

fn inspect_regular_file(path: &Path, maximum_bytes: u64) -> RegularFileState {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            RegularFileState::Unsafe
        }
        Ok(metadata) if metadata.len() > maximum_bytes => RegularFileState::LimitExceeded,
        Ok(metadata) => RegularFileState::Present(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => RegularFileState::Missing,
        Err(_) => RegularFileState::Unsafe,
    }
}

fn read_regular_file_bytes(path: &Path, maximum_bytes: u64) -> Result<Vec<u8>, &'static str> {
    let before = fs::symlink_metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "legacy_file_missing",
        _ => "legacy_file_unavailable",
    })?;
    if before.file_type().is_symlink() || !before.is_file() {
        return Err("legacy_file_unsafe");
    }
    if before.len() > maximum_bytes {
        return Err("legacy_file_too_large");
    }
    let mut file = File::open(path).map_err(|_| "legacy_file_unavailable")?;
    let opened = file.metadata().map_err(|_| "legacy_file_unavailable")?;
    if !opened.is_file() || !same_file_identity(&before, &opened) {
        return Err("legacy_file_changed_during_read");
    }
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| "legacy_file_read_failed")?;
    let after = fs::symlink_metadata(path).map_err(|_| "legacy_file_changed_during_read")?;
    if bytes.len() as u64 > maximum_bytes
        || after.file_type().is_symlink()
        || !after.is_file()
        || !same_file_identity(&opened, &after)
    {
        return Err("legacy_file_changed_during_read");
    }
    Ok(bytes)
}

fn read_optional_regular_file_bytes(
    path: &Path,
    maximum_bytes: u64,
) -> Result<Option<Vec<u8>>, &'static str> {
    match fs::symlink_metadata(path) {
        Ok(_) => read_regular_file_bytes(path, maximum_bytes).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("legacy_file_unavailable"),
    }
}

#[cfg(unix)]
fn same_file_identity(opened: &Metadata, current: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    opened.dev() == current.dev() && opened.ino() == current.ino()
}

#[cfg(not(unix))]
fn same_file_identity(opened: &Metadata, current: &Metadata) -> bool {
    opened.len() == current.len() && opened.modified().ok() == current.modified().ok()
}

fn validate_settings(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if !only_keys(
        object,
        &[
            "refreshIntervalPreset",
            "launchAtLoginEnabled",
            "statusItemStyle",
            "appLanguage",
            "lastResolvedLanguage",
            "preferredAccountID",
            "quotaWorkPlan",
        ],
    ) {
        return false;
    }
    optional_enum(
        object,
        "refreshIntervalPreset",
        &["manual", "oneMinute", "fiveMinutes", "fifteenMinutes"],
    ) && optional_bool(object, "launchAtLoginEnabled")
        && optional_enum(object, "statusItemStyle", &["meter", "text"])
        && optional_enum(object, "appLanguage", &["system", "en", "zh"])
        && optional_nullable_enum(object, "lastResolvedLanguage", &["en", "zh"])
        && optional_nullable_safe_string(object, "preferredAccountID", 256)
        && object.get("quotaWorkPlan").is_none_or(validate_work_plan)
}

fn validate_work_plan(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if !only_keys(object, &["isEnabled", "offPeriods", "hourlyModes"])
        || !optional_bool(object, "isEnabled")
    {
        return false;
    }
    let periods_valid = object.get("offPeriods").is_none_or(|periods| {
        periods.as_array().is_some_and(|periods| {
            periods.len() <= 64
                && periods.iter().all(|period| {
                    period.as_object().is_some_and(|period| {
                        only_keys(period, &["startMinuteOfDay", "endMinuteOfDay"])
                            && bounded_integer(period.get("startMinuteOfDay"), 0, 1_439)
                            && bounded_integer(period.get("endMinuteOfDay"), 0, 1_439)
                    })
                })
        })
    });
    let modes_valid = object.get("hourlyModes").is_none_or(|modes| {
        modes.as_array().is_some_and(|modes| {
            modes.len() <= 24
                && modes.iter().all(|mode| {
                    mode.as_str()
                        .is_some_and(|mode| matches!(mode, "paused" | "normal" | "intensive"))
                })
        })
    });
    periods_valid && modes_valid
}

fn parse_settings_preferences(value: &Value) -> LegacySettingsPreferences {
    let object = value.as_object().expect("validated settings object");
    let refresh_preset = object
        .get("refreshIntervalPreset")
        .and_then(Value::as_str)
        .unwrap_or("fiveMinutes");
    let (auto_refresh_enabled, refresh_interval_seconds) = match refresh_preset {
        "manual" => (false, 300),
        "oneMinute" => (true, 60),
        "fifteenMinutes" => (true, 900),
        _ => (true, 300),
    };
    let work_plan = object.get("quotaWorkPlan").and_then(Value::as_object);
    let off_periods = work_plan
        .and_then(|plan| plan.get("offPeriods"))
        .and_then(Value::as_array)
        .map(|periods| {
            periods
                .iter()
                .map(|period| {
                    let period = period.as_object().expect("validated work period");
                    LegacyWorkPeriod {
                        start_minute_of_day: period["startMinuteOfDay"]
                            .as_u64()
                            .expect("validated start minute")
                            as u16,
                        end_minute_of_day: period["endMinuteOfDay"]
                            .as_u64()
                            .expect("validated end minute")
                            as u16,
                    }
                })
                .collect()
        })
        .or_else(|| {
            work_plan
                .and_then(|plan| plan.get("hourlyModes"))
                .and_then(Value::as_array)
                .map(|modes| legacy_hourly_modes_to_periods(modes))
        })
        .unwrap_or_default();
    LegacySettingsPreferences {
        auto_refresh_enabled,
        refresh_interval_seconds,
        launch_at_login: object
            .get("launchAtLoginEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        status_item_style: match object.get("statusItemStyle").and_then(Value::as_str) {
            Some("text") => LegacyStatusItemStyle::Text,
            _ => LegacyStatusItemStyle::Meter,
        },
        language: match object.get("appLanguage").and_then(Value::as_str) {
            Some("en") => LegacyAppLanguage::English,
            Some("zh") => LegacyAppLanguage::Chinese,
            _ => LegacyAppLanguage::System,
        },
        last_resolved_language: object
            .get("lastResolvedLanguage")
            .and_then(Value::as_str)
            .map(parse_resolved_language),
        work_plan_enabled: work_plan
            .and_then(|plan| plan.get("isEnabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        off_periods,
    }
}

fn parse_resolved_language(value: &str) -> LegacyResolvedLanguage {
    match value {
        "zh" => LegacyResolvedLanguage::Chinese,
        _ => LegacyResolvedLanguage::English,
    }
}

fn legacy_hourly_modes_to_periods(modes: &[Value]) -> Vec<LegacyWorkPeriod> {
    let mut paused = [false; 24];
    for (index, mode) in modes.iter().take(24).enumerate() {
        paused[index] = mode.as_str() == Some("paused");
    }
    if paused.iter().all(|value| *value) || paused.iter().all(|value| !*value) {
        return Vec::new();
    }
    let mut periods = Vec::new();
    for start_hour in 0..24 {
        let previous = (start_hour + 23) % 24;
        if !paused[start_hour] || paused[previous] {
            continue;
        }
        let mut end_hour = (start_hour + 1) % 24;
        while paused[end_hour] {
            end_hour = (end_hour + 1) % 24;
        }
        periods.push(LegacyWorkPeriod {
            start_minute_of_day: (start_hour * 60) as u16,
            end_minute_of_day: (end_hour * 60) as u16,
        });
    }
    periods.sort_by_key(|period| (period.start_minute_of_day, period.end_minute_of_day));
    periods
}

fn validate_account_metadata(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    if !only_keys(
        object,
        &[
            "id",
            "displayName",
            "authMode",
            "providerID",
            "baseURL",
            "model",
            "createdAt",
            "lastUsedAt",
            "source",
            "runtimeKey",
            "isImportedFromCCSwitch",
        ],
    ) {
        return None;
    }
    let id = object.get("id")?.as_str()?;
    if !safe_component(id, 128)
        || !safe_string(object.get("displayName")?, 512)
        || !enum_value(object.get("authMode")?, &["chatgpt", "apiKey", "unknown"])
        || !nullable_safe_string(object.get("providerID"), 256)
        || !nullable_safe_string(object.get("baseURL"), 2_048)
        || !nullable_safe_string(object.get("model"), 512)
        || !safe_string(object.get("createdAt")?, 64)
        || !nullable_safe_string(object.get("lastUsedAt"), 64)
        || !object.get("source").is_none_or(|value| {
            enum_value(
                value,
                &[
                    "currentRuntime",
                    "manualChatGPT",
                    "manualAPI",
                    "legacyCCSwitch",
                ],
            )
        })
        || !safe_string(object.get("runtimeKey")?, 1_024)
        || !object
            .get("isImportedFromCCSwitch")
            .is_none_or(Value::is_boolean)
    {
        return None;
    }
    Some(id.to_owned())
}

fn validate_quota_record(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    only_keys(
        object,
        &[
            "accountID",
            "snapshot",
            "poolSnapshots",
            "healthStatus",
            "errorSummary",
            "failureDisposition",
            "fetchedAt",
            "authMode",
            "isCurrent",
        ],
    ) && object
        .get("accountID")
        .is_some_and(|value| safe_string(value, 256))
        && object
            .get("snapshot")
            .is_none_or(|value| value.is_null() || value.is_object())
        && object.get("poolSnapshots").is_none_or(|value| {
            value
                .as_array()
                .is_some_and(|records| records.len() <= MAX_POOL_RECORDS)
        })
        && object.get("healthStatus").is_some_and(|value| {
            enum_value(value, &["healthy", "readFailure", "needsLogin", "expired"])
        })
        && nullable_safe_string(object.get("errorSummary"), 2_048)
        && object
            .get("failureDisposition")
            .is_none_or(|value| value.is_null() || enum_value(value, &["transient", "terminal"]))
        && object
            .get("fetchedAt")
            .is_some_and(|value| safe_string(value, 64))
        && object
            .get("authMode")
            .is_some_and(|value| enum_value(value, &["chatgpt", "apiKey", "unknown"]))
        && object.get("isCurrent").is_some_and(Value::is_boolean)
}

fn validate_session_settings(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        only_keys(object, &["language"])
            && object
                .get("language")
                .is_some_and(|value| enum_value(value, &["en", "zh"]))
    })
}

fn validate_provider_mode(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    if !only_keys(
        object,
        &[
            "restorePointID",
            "providerAccountID",
            "providerDisplayName",
            "activatedAt",
        ],
    ) {
        return None;
    }
    let restore_point_id = object.get("restorePointID")?.as_str()?;
    if !safe_component(restore_point_id, 128)
        || !object
            .get("providerAccountID")
            .is_some_and(|value| safe_string(value, 256))
        || !object
            .get("providerDisplayName")
            .is_some_and(|value| safe_string(value, 512))
        || !object
            .get("activatedAt")
            .is_some_and(|value| safe_string(value, 64))
    {
        return None;
    }
    Some(restore_point_id.to_owned())
}

fn validate_restore_manifest(
    value: &Value,
    expected_id: &str,
) -> Option<Vec<(String, Option<u64>)>> {
    let object = value.as_object()?;
    if !only_keys(
        object,
        &[
            "id",
            "createdAt",
            "reason",
            "summary",
            "codexWasRunning",
            "files",
        ],
    ) || object.get("id")?.as_str()? != expected_id
        || !object
            .get("createdAt")
            .is_some_and(|value| safe_string(value, 64))
        || !object
            .get("reason")
            .is_some_and(|value| safe_string(value, 256))
        || !object
            .get("summary")
            .is_some_and(|value| safe_string(value, 1_024))
        || !object.get("codexWasRunning").is_some_and(Value::is_boolean)
    {
        return None;
    }
    let files = object.get("files")?.as_array()?;
    if files.len() > MAX_RESTORE_FILES {
        return None;
    }
    let mut backup_files = Vec::new();
    for file in files {
        let file = file.as_object()?;
        if !only_keys(
            file,
            &[
                "originalPath",
                "backupRelativePath",
                "exists",
                "sha256",
                "fileSize",
                "modifiedAt",
            ],
        ) || !file
            .get("originalPath")
            .is_some_and(|value| safe_string(value, 4_096))
            || !file.get("exists").is_some_and(Value::is_boolean)
            || !nullable_safe_string(file.get("modifiedAt"), 64)
        {
            return None;
        }
        let exists = file.get("exists")?.as_bool()?;
        let relative = file.get("backupRelativePath").and_then(Value::as_str);
        let sha256 = file.get("sha256").and_then(Value::as_str);
        let file_size = file.get("fileSize").and_then(Value::as_u64);
        if exists {
            let relative = relative?;
            if !safe_relative_path(relative)
                || !sha256.is_some_and(|value| {
                    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            {
                return None;
            }
            backup_files.push((relative.to_owned(), file_size));
        } else if relative.is_some() || sha256.is_some() || file_size.is_some() {
            return None;
        }
    }
    Some(backup_files)
}

fn build_plan(inventory: &[ArtifactInventory]) -> Vec<ImportPlanAction> {
    let artifact = |kind| {
        inventory
            .iter()
            .find(|artifact| artifact.kind == kind)
            .expect("fixed inventory kind")
    };
    let settings = artifact(LegacyArtifactKind::Settings);
    let accounts = artifact(LegacyArtifactKind::AccountRecords);
    let quota = artifact(LegacyArtifactKind::QuotaCache);
    let session = artifact(LegacyArtifactKind::SessionManagerSettings);
    let provider = artifact(LegacyArtifactKind::ProviderMode);
    let restores = artifact(LegacyArtifactKind::RestorePoints);
    vec![
        action_for_artifact(
            settings,
            ImportTarget::MonitorSettings,
            ImportDisposition::Import,
            "legacy_monitor_settings_ready",
        ),
        action_for_artifact(
            settings,
            ImportTarget::ShellPreferences,
            ImportDisposition::Import,
            "legacy_shell_preferences_ready",
        ),
        action_for_artifact(
            settings,
            ImportTarget::WorkPlan,
            ImportDisposition::Import,
            "legacy_work_plan_ready",
        ),
        action_for_artifact(
            accounts,
            ImportTarget::AccountVault,
            ImportDisposition::Review,
            "legacy_account_identity_resolution_required",
        ),
        action_for_artifact(
            quota,
            ImportTarget::QuotaCache,
            ImportDisposition::Review,
            "legacy_quota_binding_review_required",
        ),
        action_for_artifact(
            session,
            ImportTarget::SessionPreferences,
            ImportDisposition::Import,
            "legacy_session_preferences_ready",
        ),
        action_for_artifact(
            provider,
            ImportTarget::ProviderModeState,
            ImportDisposition::Review,
            "legacy_provider_state_review_required",
        ),
        action_for_artifact(
            restores,
            ImportTarget::RestorePointLedger,
            ImportDisposition::Review,
            "legacy_restore_point_review_required",
        ),
    ]
}

fn action_for_artifact(
    artifact: &ArtifactInventory,
    target: ImportTarget,
    ready_disposition: ImportDisposition,
    ready_reason: &'static str,
) -> ImportPlanAction {
    let (disposition, expected_target_count, reason_code) = match artifact.state {
        LegacyArtifactState::Present => (ready_disposition, artifact.record_count, ready_reason),
        LegacyArtifactState::Missing => (ImportDisposition::Skip, 0, "legacy_source_not_present"),
        LegacyArtifactState::Invalid
        | LegacyArtifactState::Unsafe
        | LegacyArtifactState::LimitExceeded => {
            (ImportDisposition::Blocked, 0, "legacy_source_blocked")
        }
    };
    ImportPlanAction {
        target,
        disposition,
        source_item_count: artifact.record_count,
        expected_target_count,
        reason_code,
    }
}

fn all_missing_inventory() -> Vec<ArtifactInventory> {
    [
        LegacyArtifactKind::Settings,
        LegacyArtifactKind::AccountIndex,
        LegacyArtifactKind::AccountRecords,
        LegacyArtifactKind::QuotaCache,
        LegacyArtifactKind::SessionManagerSettings,
        LegacyArtifactKind::ProviderMode,
        LegacyArtifactKind::RestorePoints,
    ]
    .into_iter()
    .map(ArtifactInventory::missing)
    .collect()
}

fn add_conflict(
    conflicts: &mut BTreeMap<ImportConflictKind, u32>,
    kind: ImportConflictKind,
    count: u32,
) {
    if count > 0 {
        conflicts.insert(kind, count);
    }
}

fn conflict_reason(kind: ImportConflictKind) -> &'static str {
    match kind {
        ImportConflictKind::DuplicateAccountId => "legacy_duplicate_account_id",
        ImportConflictKind::MissingAccountRecord => "legacy_account_record_missing",
        ImportConflictKind::OrphanAccountRecord => "legacy_account_record_orphaned",
        ImportConflictKind::InvalidAccountRecord => "legacy_account_record_invalid",
        ImportConflictKind::InvalidRestorePoint => "legacy_restore_point_invalid",
        ImportConflictKind::ProviderRestorePointMissing => "legacy_provider_restore_point_missing",
    }
}

fn only_keys(object: &Map<String, Value>, allowed: &[&str]) -> bool {
    object
        .keys()
        .all(|key| allowed.iter().any(|allowed| key == allowed))
}

fn optional_bool(object: &Map<String, Value>, key: &str) -> bool {
    object.get(key).is_none_or(Value::is_boolean)
}

fn optional_enum(object: &Map<String, Value>, key: &str, allowed: &[&str]) -> bool {
    object
        .get(key)
        .is_none_or(|value| enum_value(value, allowed))
}

fn optional_nullable_enum(object: &Map<String, Value>, key: &str, allowed: &[&str]) -> bool {
    object
        .get(key)
        .is_none_or(|value| value.is_null() || enum_value(value, allowed))
}

fn optional_nullable_safe_string(object: &Map<String, Value>, key: &str, maximum: usize) -> bool {
    nullable_safe_string(object.get(key), maximum)
}

fn nullable_safe_string(value: Option<&Value>, maximum: usize) -> bool {
    value.is_none_or(|value| value.is_null() || safe_string(value, maximum))
}

fn safe_string(value: &Value, maximum: usize) -> bool {
    value.as_str().is_some_and(|value| {
        !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
    })
}

fn enum_value(value: &Value, allowed: &[&str]) -> bool {
    value.as_str().is_some_and(|value| allowed.contains(&value))
}

fn bounded_integer(value: Option<&Value>, minimum: i64, maximum: i64) -> bool {
    value
        .and_then(Value::as_i64)
        .is_some_and(|value| (minimum..=maximum).contains(&value))
}

fn safe_component(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value != "."
        && value != ".."
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn safe_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn usize_to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Write;

    fn source_fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/migration/source")
            .join(name)
    }

    fn report_fixture(name: &str) -> Value {
        let bytes = fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/migration/v1")
                .join(name),
        )
        .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn source_snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fn visit(root: &Path, directory: &Path, snapshot: &mut BTreeMap<String, Vec<u8>>) {
            let mut entries = fs::read_dir(directory)
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).unwrap();
                if metadata.is_dir() {
                    visit(root, &path, snapshot);
                } else if metadata.is_file() {
                    snapshot.insert(
                        path.strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                        fs::read(path).unwrap(),
                    );
                }
            }
        }

        let mut snapshot = BTreeMap::new();
        visit(root, root, &mut snapshot);
        snapshot
    }

    #[test]
    fn ready_fixture_produces_exact_redacted_dry_run_without_source_changes() {
        let root = source_fixture("ready");
        let before = source_snapshot(&root);
        let report = dry_run_quotaviewer(&root);
        let json = serde_json::to_string(&report).unwrap();

        assert_eq!(report.status, LegacyDryRunStatus::Ready);
        assert_eq!(report.exit_code(), 0);
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            report_fixture("ready-report.json")
        );
        assert_eq!(source_snapshot(&root), before);
        for canary in [
            "fixture-access-token",
            "fixture-api-key",
            "person@example.com",
            "api.example.invalid",
            "/Users/fixture",
            "account-alpha",
            "restore-alpha",
        ] {
            assert!(!json.contains(canary), "dry run leaked {canary}");
        }
    }

    #[test]
    fn importable_preferences_expose_only_direct_non_secret_targets() {
        let root = source_fixture("ready");
        let before = source_snapshot(&root);
        let preferences = read_importable_preferences(&root).unwrap();
        let settings = preferences.settings.as_ref().unwrap();

        assert!(settings.auto_refresh_enabled);
        assert_eq!(settings.refresh_interval_seconds, 300);
        assert!(!settings.launch_at_login);
        assert_eq!(settings.status_item_style, LegacyStatusItemStyle::Meter);
        assert_eq!(settings.language, LegacyAppLanguage::System);
        assert_eq!(
            settings.last_resolved_language,
            Some(LegacyResolvedLanguage::English)
        );
        assert!(settings.work_plan_enabled);
        assert_eq!(
            settings.off_periods,
            vec![LegacyWorkPeriod {
                start_minute_of_day: 0,
                end_minute_of_day: 480,
            }]
        );
        assert_eq!(
            preferences.session_language,
            Some(LegacyResolvedLanguage::English)
        );
        let serialized = serde_json::to_string(&preferences).unwrap();
        for canary in [
            "fixture-access-token",
            "fixture-api-key",
            "person@example.com",
            "account-alpha",
        ] {
            assert!(!serialized.contains(canary));
        }
        assert_eq!(source_snapshot(&root), before);
    }

    #[test]
    fn account_snapshot_is_bounded_ordered_and_kept_host_only() {
        let root = source_fixture("ready");
        let before = source_snapshot(&root);
        let bundle = read_importable_accounts(&root).unwrap();

        assert_eq!(bundle.accounts.len(), 1);
        assert_eq!(bundle.preferred_source_id.as_deref(), Some("account-alpha"));
        let account = &bundle.accounts[0];
        assert_eq!(account.source_id, "account-alpha");
        assert_eq!(account.display_name, "person@example.com");
        assert_eq!(account.auth_mode, LegacyAccountAuthMode::Chatgpt);
        assert_eq!(account.created_at, "2026-08-29T00:00:00Z");
        assert_eq!(
            account.last_used_at.as_deref(),
            Some("2026-08-30T00:00:00Z")
        );
        assert!(account.auth_json.starts_with(b"{"));
        assert!(account.config_toml.is_some());
        assert_eq!(source_snapshot(&root), before);
    }

    #[test]
    fn hourly_mode_fallback_preserves_a_wrapped_off_period() {
        let modes = (0..24)
            .map(|hour| {
                Value::String(if !(2..22).contains(&hour) {
                    "paused".to_string()
                } else {
                    "normal".to_string()
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            legacy_hourly_modes_to_periods(&modes),
            vec![LegacyWorkPeriod {
                start_minute_of_day: 1_320,
                end_minute_of_day: 120,
            }]
        );
    }

    #[test]
    fn inconsistent_fixture_reports_aggregate_conflicts_without_identifiers() {
        let report = dry_run_quotaviewer(&source_fixture("partial"));
        let json = serde_json::to_string(&report).unwrap();

        assert_eq!(report.status, LegacyDryRunStatus::Partial);
        assert_eq!(report.exit_code(), 2);
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            report_fixture("partial-report.json")
        );
        for canary in ["missing-account", "orphan-account", "private@example.com"] {
            assert!(!json.contains(canary));
        }
    }

    #[test]
    fn empty_source_is_unsupported_not_a_successful_zero_item_import() {
        let root = tempfile::tempdir().unwrap();
        let report = dry_run_quotaviewer(root.path());

        assert_eq!(report.status, LegacyDryRunStatus::Unsupported);
        assert_eq!(report.exit_code(), 3);
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            report_fixture("unsupported-report.json")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_known_file_is_rejected_without_reading_its_target() {
        use std::os::unix::fs::symlink;

        let source = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let mut outside_file = outside.reopen().unwrap();
        outside_file
            .write_all(b"{\"fixture-access-token\":\"must-not-be-read\"}")
            .unwrap();
        symlink(outside.path(), source.path().join(SETTINGS_FILE)).unwrap();

        let report = dry_run_quotaviewer(source.path());
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(report.status, LegacyDryRunStatus::Unsupported);
        assert!(json.contains("legacy_file_unsafe"));
        assert!(!json.contains("must-not-be-read"));
    }

    #[test]
    fn oversized_json_is_bounded_before_parsing() {
        let source = tempfile::tempdir().unwrap();
        let mut file = File::create(source.path().join(SETTINGS_FILE)).unwrap();
        file.write_all(&vec![b'x'; MAX_SETTINGS_BYTES as usize + 1])
            .unwrap();

        let report = dry_run_quotaviewer(source.path());
        let settings = report
            .inventory
            .iter()
            .find(|artifact| artifact.kind == LegacyArtifactKind::Settings)
            .unwrap();
        assert_eq!(settings.state, LegacyArtifactState::LimitExceeded);
        assert_eq!(report.status, LegacyDryRunStatus::Unsupported);
    }

    #[test]
    fn production_importer_contains_no_file_write_or_delete_primitive() {
        let source = include_str!("lib.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        for forbidden in [
            "fs::write(",
            "File::create(",
            "OpenOptions",
            "remove_file(",
            "remove_dir",
            "rename(",
            "copy(",
            "create_dir",
        ] {
            assert!(
                !production.contains(forbidden),
                "production importer contains write primitive {forbidden}"
            );
        }
    }
}
