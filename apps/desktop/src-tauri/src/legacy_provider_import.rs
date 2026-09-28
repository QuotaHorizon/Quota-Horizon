//! User-confirmed, additive Viewer API imports into the existing Provider store.
//! A protected (not plaintext) profile is staged before an exclusive install.
//! The restore ledger contains only identifiers and file observations, never keys.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

use capacity_domain::UtcTimestamp;
use capacity_migration::{read_importable_accounts, LegacyAccountImportBundle};
use capacity_mutation::{
    ProtectedTarget, ProtectedTargetPath, RestorePointId, RestorePointOperation, RestorePointState,
    RestorePointStore, TargetSnapshot, TargetState,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};
use uuid::Uuid;

use crate::{
    legacy_account_migration::{
        read_json_private, read_safe_file, source_revision, write_json_private,
    },
    legacy_provider_migration::map_legacy_api_account,
    models::ProviderProfile,
    storage::{ensure_private_directory, resolve_paths, Paths},
};

const IMPORT: &str = "IMPORT API";
const ROLLBACK: &str = "ROLLBACK API";
const TTL_SECONDS: i64 = 120;
const MAX_PENDING: usize = 256;
static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// No Serialize or Debug: this is the sole in-memory confirmation material.
struct Pending {
    source: PathBuf,
    revision: String,
    providers: PathBuf,
    profile: ProviderProfile,
    expires_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderImportRow {
    source_ordinal: usize,
    label: String,
    endpoint: Option<String>,
    model: Option<String>,
    api_format: Option<crate::models::ProviderApiFormat>,
    reasoning_effort: Option<crate::models::ReasoningEffort>,
    context_window: Option<u64>,
    disposition: &'static str,
    reason_code: Option<&'static str>,
    confirm_token: Option<String>,
    expires_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderImportPreview {
    providers: Vec<ProviderImportRow>,
    typed_confirmation: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderImportOperation {
    operation_id: Option<String>,
    label: Option<String>,
    status: &'static str,
    rollback_available: bool,
    recovery_required: bool,
    typed_rollback: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalState {
    Preparing,
    Staged,
    Committed,
    RolledBack,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    operation_id: String,
    provider_id: String,
    label: String,
    point: RestorePointId,
    state: JournalState,
    after: Option<TargetSnapshot>,
}

fn now() -> Result<UtcTimestamp, String> {
    UtcTimestamp::parse(Utc::now().to_rfc3339()).map_err(|_| "无法读取迁移时间。".into())
}

fn app_data(paths: &Paths) -> Result<&Path, String> {
    paths
        .state_file
        .parent()
        .ok_or_else(|| "无法定位连接迁移目录。".into())
}

fn journal_path(paths: &Paths) -> Result<PathBuf, String> {
    Ok(app_data(paths)?.join("legacy-provider-import-v1.json"))
}

fn store(paths: &Paths) -> Result<RestorePointStore, String> {
    RestorePointStore::new(app_data(paths)?.join("legacy-provider-restore-points-v1"))
        .map_err(|_| "连接迁移恢复点不可用。".into())
}

fn valid_provider_id(id: &str) -> bool {
    id.strip_prefix("viewer-").is_some_and(|s| {
        s.len() == 24
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn targets(paths: &Paths, id: &str) -> Result<Vec<ProtectedTargetPath>, String> {
    if !valid_provider_id(id) {
        return Err("连接迁移目标无效。".into());
    }
    [
        (
            ProtectedTarget::ImportedProviderProfile,
            crate::providers::provider_path(paths, id),
        ),
        (
            ProtectedTarget::ImportedProviderFieldMetadata,
            crate::providers::provider_field_modified_at_path(paths, id),
        ),
    ]
    .into_iter()
    .map(|(kind, path)| {
        ProtectedTargetPath::new(kind, path).map_err(|_| "连接迁移路径不安全。".into())
    })
    .collect()
}

fn stages(paths: &Paths, journal: &Journal) -> Result<Vec<ProtectedTargetPath>, String> {
    [
        (
            ProtectedTarget::ImportedProviderProfile,
            paths
                .providers
                .join(format!(".viewer-import-{}.staged", journal.operation_id)),
        ),
        (
            ProtectedTarget::ImportedProviderFieldMetadata,
            paths
                .providers
                .join(format!(".viewer-import-{}.metadata", journal.operation_id)),
        ),
    ]
    .into_iter()
    .map(|(kind, path)| {
        ProtectedTargetPath::new(kind, path).map_err(|_| "连接迁移暂存路径不安全。".into())
    })
    .collect()
}

fn absent(snapshot: &TargetSnapshot) -> bool {
    snapshot
        .observations()
        .iter()
        .all(|item| item.state() == &TargetState::Absent)
}

fn read_journal(paths: &Paths) -> Result<Option<Journal>, String> {
    let path = journal_path(paths)?;
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取连接迁移恢复记录。".into()),
        Ok(_) => {}
    }
    let journal: Journal = read_json_private(&path)?;
    if journal.version != 1
        || !valid_provider_id(&journal.provider_id)
        || !Uuid::parse_str(&journal.operation_id)
            .is_ok_and(|id| id.to_string() == journal.operation_id)
        || journal.label.len() > 256
        || journal.label.chars().any(char::is_control)
    {
        return Err("连接迁移恢复记录已损坏，未更改任何连接。".into());
    }
    let point = store(paths)?
        .verify(&journal.point)
        .map_err(|_| "连接迁移恢复点校验失败。".to_string())?;
    if point.operation() != RestorePointOperation::ImportLegacyProvider
        || point.protected_targets().collect::<BTreeSet<_>>()
            != BTreeSet::from([
                ProtectedTarget::ImportedProviderProfile,
                ProtectedTarget::ImportedProviderFieldMetadata,
            ])
    {
        return Err("连接迁移恢复点不匹配。".into());
    }
    Ok(Some(journal))
}

fn write_journal(paths: &Paths, journal: &Journal) -> Result<(), String> {
    write_json_private(&journal_path(paths)?, journal)
}

fn ensure_inactive(paths: &Paths, id: &str) -> Result<(), String> {
    // The normal reader tolerates a missing state file. A corrupt existing state
    // must not be treated as proof that an imported connection is inactive.
    match fs::symlink_metadata(&paths.state_file) {
        Ok(_) => {
            let bytes = read_safe_file(&paths.state_file, 1024 * 1024)?;
            serde_json::from_slice::<crate::models::ManagerStateFile>(&bytes)
                .map_err(|_| "无法确认当前连接状态，请先解决状态读取问题。".to_string())?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("无法确认当前连接状态。".into()),
    }
    crate::providers::ensure_provider_edit_is_inactive(paths, id, None, "")
        .map_err(|_| "该连接正在使用。请先在连接页面停用，再撤销迁移。".into())
}

type ConnectionMetadata = HashSet<(String, String, String)>;

fn connection_metadata(paths: &Paths) -> Result<ConnectionMetadata, String> {
    match fs::symlink_metadata(&paths.providers) {
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
            return Err("已有连接目录不安全。".into())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(_) => return Err("已有连接目录不可读取。".into()),
        Ok(_) => {}
    }
    let entries = match fs::read_dir(&paths.providers) {
        Ok(entries) => entries,
        Err(_) => return Err("已有连接目录不可读取。".into()),
    };
    let mut metadata = HashSet::new();
    let mut total_bytes = 0_usize;
    for (index, entry) in entries.enumerate() {
        if index >= 4096 {
            return Err("已有连接过多，需先整理后再导入。".into());
        }
        let path = entry
            .map_err(|_| "已有连接目录不可读取。".to_string())?
            .path();
        if path.extension().and_then(|s| s.to_str()) != Some("json")
            || path
                .file_name()
                .is_some_and(|s| s.to_string_lossy().ends_with(".field-modified-at.json"))
        {
            continue;
        }
        let bytes = read_safe_file(&path, 1024 * 1024)?;
        total_bytes = total_bytes.saturating_add(bytes.len());
        if total_bytes > 32 * 1024 * 1024 {
            return Err("已有连接数据过大，需先整理后再导入。".into());
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| "已有连接信息损坏，已停止导入预览。".to_string())?;
        if let (Some(name), Some(model), Some(endpoint)) = (
            value.get("name").and_then(|v| v.as_str()),
            value.get("model").and_then(|v| v.as_str()),
            value.get("baseUrl").and_then(|v| v.as_str()),
        ) {
            metadata.insert((
                name.to_owned(),
                model.to_owned(),
                endpoint.trim_end_matches('/').to_owned(),
            ));
        }
    }
    Ok(metadata)
}

fn duplicate_metadata(metadata: &ConnectionMetadata, profile: &ProviderProfile) -> bool {
    metadata.contains(&(
        profile.name.clone(),
        profile.model.clone(),
        profile.base_url.clone(),
    ))
}

fn prepare_rows(
    paths: &Paths,
    source: PathBuf,
    bundle: &LegacyAccountImportBundle,
) -> Result<ProviderImportPreview, String> {
    let revision = source_revision(bundle);
    let at = Utc::now();
    let expires = at + Duration::seconds(TTL_SECONDS);
    // Scan the existing catalog once, not once per source account. No protected
    // credentials are hydrated, and aggregate input bytes are bounded.
    let existing = connection_metadata(paths)?;
    let mut pending = PENDING
        .lock()
        .map_err(|_| "连接迁移确认状态不可用。".to_string())?;
    pending.retain(|_, plan| plan.expires_at > at);
    let mut rows = Vec::new();
    for (index, record) in bundle.accounts.iter().enumerate() {
        let mapped = map_legacy_api_account(record);
        if matches!(&mapped, Ok(None)) {
            continue;
        }
        let mut row = ProviderImportRow {
            source_ordinal: index + 1,
            label: record.display_name.clone(),
            endpoint: None,
            model: None,
            api_format: None,
            reasoning_effort: None,
            context_window: None,
            disposition: "review",
            reason_code: None,
            confirm_token: None,
            expires_at: None,
        };
        match mapped {
            Err(code) => row.reason_code = Some(code),
            Ok(Some(profile)) => {
                row.endpoint = Some(profile.base_url.clone());
                row.model = Some(profile.model.clone());
                row.api_format = Some(profile.api_format);
                row.reasoning_effort = profile
                    .model_reasoning_efforts
                    .get(&profile.model)
                    .and_then(|efforts| efforts.first())
                    .copied();
                row.context_window = profile.context_window;
                let current = store(paths)?
                    .observe(&targets(paths, &profile.id)?)
                    .map_err(|_| "已有连接路径不安全或不可读取。".to_string())?;
                if !absent(&current) || duplicate_metadata(&existing, &profile) {
                    row.disposition = "keep_existing";
                    row.reason_code = Some("legacy_provider_existing");
                } else if pending.len() >= MAX_PENDING {
                    row.reason_code = Some("legacy_provider_preview_limit");
                } else {
                    let token = Uuid::new_v4().to_string();
                    row.disposition = "import";
                    row.confirm_token = Some(token.clone());
                    row.expires_at = Some(expires.to_rfc3339());
                    pending.insert(
                        token,
                        Pending {
                            source: source.clone(),
                            revision: revision.clone(),
                            providers: paths.providers.clone(),
                            profile,
                            expires_at: expires,
                        },
                    );
                }
            }
            Ok(None) => unreachable!(),
        }
        rows.push(row);
    }
    Ok(ProviderImportPreview {
        providers: rows,
        typed_confirmation: IMPORT,
    })
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| "无法持久化连接迁移目录。".to_string())?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn remove_matching_stages(paths: &Paths, journal: &Journal) -> Result<(), String> {
    let store = store(paths)?;
    let stages = stages(paths, journal)?;
    let current = store
        .observe(&stages)
        .map_err(|_| "连接暂存文件无法验证。".to_string())?;
    for item in current.observations() {
        if item.state() == &TargetState::Absent {
            continue;
        }
        if journal.after.as_ref().and_then(|s| s.state(item.target())) != Some(item.state()) {
            return Err("连接暂存文件已变化，已保留以供检查。".into());
        }
    }
    for stage in stages {
        if current.state(stage.target()) != Some(&TargetState::Absent) {
            fs::remove_file(stage.path())
                .map_err(|_| "无法移除已验证的连接暂存文件。".to_string())?;
        }
    }
    sync_directory(&paths.providers)
}

fn rollback(paths: &Paths, journal: &mut Journal) -> Result<(), String> {
    ensure_inactive(paths, &journal.provider_id)?;
    let store = store(paths)?;
    let targets = targets(paths, &journal.provider_id)?;
    let current = store
        .observe(&targets)
        .map_err(|_| "无法校验待撤销连接。".to_string())?;
    // An interrupted multi-file install may contain only one owned target.
    for observation in current.observations() {
        if observation.state() != &TargetState::Absent
            && journal
                .after
                .as_ref()
                .and_then(|s| s.state(observation.target()))
                != Some(observation.state())
        {
            return Err("连接在导入后发生了变化，已保留当前版本，不会覆盖你的修改。".into());
        }
    }
    let point = store
        .verify(&journal.point)
        .map_err(|_| "无法验证连接恢复点。".to_string())?;
    match point.state() {
        RestorePointState::Prepared => {
            store.rollback_prepared(&journal.point, &targets, &current, &now()?)
        }
        RestorePointState::Committed { .. } | RestorePointState::RolledBack { .. } => {
            store.rollback_committed(&journal.point, &targets, &now()?)
        }
    }
    .map_err(|_| "连接撤销校验未通过，已保留恢复记录。".to_string())?;
    remove_matching_stages(paths, journal)?;
    journal.state = JournalState::RolledBack;
    write_journal(paths, journal)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImportCheck {
    SourceAndLease,
    RollbackLease,
}

fn execute_import<F, W>(
    paths: &Paths,
    profile: &ProviderProfile,
    mut check: F,
    write_profile: W,
) -> Result<ProviderImportOperation, String>
where
    F: FnMut(ImportCheck) -> Result<(), String>,
    W: FnOnce(&Path, &ProviderProfile) -> Result<(), String>,
{
    let mut protected = BTreeSet::new();
    if let Some(previous) = read_journal(paths)? {
        if matches!(
            previous.state,
            JournalState::Preparing | JournalState::Staged
        ) {
            return Err("上一次连接迁移尚未收尾，请先撤销未完成的迁移。".into());
        }
        protected.insert(previous.point);
    }
    check(ImportCheck::SourceAndLease)?;
    ensure_inactive(paths, &profile.id)?;
    let store = store(paths)?;
    let targets = targets(paths, &profile.id)?;
    if !absent(
        &store
            .observe(&targets)
            .map_err(|_| "无法校验连接目标。".to_string())?,
    ) || duplicate_metadata(&connection_metadata(paths)?, profile)
    {
        return Err("该连接已存在，保留当前版本。请重新预览。".into());
    }
    ensure_private_directory(&paths.providers)?;
    let point = store
        .create_restore_point(
            RestorePointOperation::ImportLegacyProvider,
            &targets,
            false,
            &now()?,
            &protected,
        )
        .map_err(|_| "无法创建连接迁移恢复点，未导入连接。".to_string())?;
    let mut journal = Journal {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        provider_id: profile.id.clone(),
        label: profile.name.clone(),
        point: point.id().clone(),
        state: JournalState::Preparing,
        after: None,
    };
    write_journal(paths, &journal)?;
    let staged = stages(paths, &journal)?;
    let applied: Result<(), String> = (|| {
        if !absent(
            &store
                .observe(&staged)
                .map_err(|_| "连接暂存目录不安全。".to_string())?,
        ) {
            return Err("连接暂存文件已存在，未覆盖任何文件。".into());
        }
        write_profile(staged[0].path(), profile)
            .map_err(|_| "无法安全保存连接密钥；请检查系统凭据授权后重试。".to_string())?;
        write_json_private(
            staged[1].path(),
            &crate::providers::new_imported_provider_field_metadata(),
        )?;
        journal.after = Some(
            store
                .observe(&staged)
                .map_err(|_| "无法验证暂存连接。".to_string())?,
        );
        journal.state = JournalState::Staged;
        write_journal(paths, &journal)?;
        check(ImportCheck::SourceAndLease)?;
        for (stage, target) in staged.iter().zip(&targets) {
            // Same-volume hard-link installation is exclusive: it cannot replace
            // a profile created after the preview, including another process.
            fs::hard_link(stage.path(), target.path())
                .map_err(|_| "连接目标已变化或无法写入，导入已停止。".to_string())?;
        }
        sync_directory(&paths.providers)?;
        check(ImportCheck::SourceAndLease)?;
        if store.observe(&targets).ok().as_ref() != journal.after.as_ref() {
            return Err("导入连接未通过完整性校验。".into());
        }
        store
            .mark_committed(&journal.point, &targets, &now()?)
            .map_err(|_| "无法提交连接恢复记录。".to_string())?;
        journal.state = JournalState::Committed;
        write_journal(paths, &journal)?;
        Ok(())
    })();
    if let Err(error) = applied {
        if check(ImportCheck::RollbackLease).is_err() {
            return Err(format!(
                "{error} 当前无法证明恢复操作互斥，已保留恢复记录且停止进一步写入。"
            ));
        }
        return match rollback(paths, &mut journal) {
            Ok(()) => Err(format!("{error} 本次已撤回，原有连接和当前登录未改变。")),
            Err(_) => Err(format!(
                "{error} 有未完成的恢复记录，请检查迁移面板；未覆盖原有连接。"
            )),
        };
    }
    // Cleanup failure does not turn a verified committed connection into a fake
    // failure. Its exact staging files can be cleaned during the next recovery.
    let _ = remove_matching_stages(paths, &journal);
    operation_view(paths, Some(&journal))
}

fn operation_view(
    paths: &Paths,
    journal: Option<&Journal>,
) -> Result<ProviderImportOperation, String> {
    let Some(journal) = journal else {
        return Ok(ProviderImportOperation {
            operation_id: None,
            label: None,
            status: "none",
            rollback_available: false,
            recovery_required: false,
            typed_rollback: ROLLBACK,
        });
    };
    let unfinished = matches!(
        journal.state,
        JournalState::Preparing | JournalState::Staged
    );
    let available = journal.state != JournalState::RolledBack
        && ensure_inactive(paths, &journal.provider_id).is_ok()
        && rollback_ready(paths, journal);
    Ok(ProviderImportOperation {
        operation_id: Some(journal.operation_id.clone()),
        label: Some(journal.label.clone()),
        status: match journal.state {
            JournalState::Preparing | JournalState::Staged => "unfinished",
            JournalState::Committed => "committed",
            JournalState::RolledBack => "rolled_back",
        },
        rollback_available: available,
        recovery_required: unfinished,
        typed_rollback: ROLLBACK,
    })
}

fn rollback_ready(paths: &Paths, journal: &Journal) -> bool {
    let checked = (|| -> Result<bool, String> {
        let store = store(paths)?;
        let targets = targets(paths, &journal.provider_id)?;
        let point = store
            .verify(&journal.point)
            .map_err(|_| "invalid".to_string())?;
        if matches!(point.state(), RestorePointState::Committed { .. })
            && store
                .verify_rollback_ready(&journal.point, &targets)
                .is_err()
        {
            return Ok(false);
        }
        for set in [&targets, &stages(paths, journal)?] {
            let current = store.observe(set).map_err(|_| "invalid".to_string())?;
            if current.observations().iter().any(|item| {
                item.state() != &TargetState::Absent
                    && journal.after.as_ref().and_then(|s| s.state(item.target()))
                        != Some(item.state())
            }) {
                return Ok(false);
            }
        }
        Ok(true)
    })();
    checked.unwrap_or(false)
}

fn notify<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.emit("providers-changed", ());
    crate::system_tray::refresh_menu(app);
}

#[tauri::command]
pub(crate) async fn prepare_legacy_quotaviewer_provider_import<R: Runtime + 'static>(
    app: AppHandle<R>,
    source_path: Option<String>,
) -> Result<ProviderImportPreview, String> {
    let source = crate::legacy_migration::legacy_source_path(source_path)?;
    tauri::async_runtime::spawn_blocking(move || {
        let bundle = read_importable_accounts(&source)
            .map_err(|_| "无法读取 Viewer API 账号。".to_string())?;
        prepare_rows(&resolve_paths(&app)?, source, &bundle)
    })
    .await
    .map_err(|_| "连接迁移预览任务意外结束。".to_string())?
}

#[tauri::command]
pub(crate) async fn confirm_legacy_quotaviewer_provider_import<R: Runtime + 'static>(
    app: AppHandle<R>,
    confirm_token: String,
    typed_confirmation: String,
) -> Result<ProviderImportOperation, String> {
    let pending = consume_confirmation(&confirm_token, &typed_confirmation)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (lease, mut probe) = crate::commands::acquire_shared_mutation_lease()?;
        let _guard = crate::commands::account_switch_lock()
            .lock()
            .map_err(|_| "连接操作锁不可用。".to_string())?;
        let paths = resolve_paths(&app)?;
        if paths.providers != pending.providers {
            return Err("连接目录已变化，请重新预览。".into());
        }
        let result = execute_import(
            &paths,
            &pending.profile,
            |phase| {
                if let Err(error) = lease.revalidate(&mut probe) {
                    // A late legacy process cannot take our still-held SQLite
                    // lease. Its appearance prevents import, but does not stop
                    // compensation. Loss of the actual lease stops all writes.
                    if phase != ImportCheck::RollbackLease
                        || !matches!(
                            error,
                            capacity_mutation::MutationLockError::LegacyViewerRunning
                        )
                    {
                        return Err("连接迁移安全互斥已失效。".into());
                    }
                }
                if phase == ImportCheck::RollbackLease {
                    return Ok(());
                }
                let current = read_importable_accounts(&pending.source)
                    .map_err(|_| "Viewer 源数据不可读取，请重新预览。".to_string())?;
                if source_revision(&current) != pending.revision {
                    return Err("Viewer 数据在预览后发生了变化，请重新预览。".into());
                }
                Ok(())
            },
            crate::provider_credentials::write_provider_profile,
        );
        notify(&app);
        result
    })
    .await
    .map_err(|_| "连接导入任务意外结束。请检查迁移恢复状态。".to_string())?
}

fn consume_confirmation(token: &str, typed: &str) -> Result<Pending, String> {
    if typed != IMPORT {
        return Err("请输入 IMPORT API 以确认导入该连接。".into());
    }
    if token.len() != 36 || Uuid::parse_str(token).is_err() {
        return Err("连接导入预览已过期或已使用，请重新预览。".into());
    }
    let mut pending = PENDING
        .lock()
        .map_err(|_| "连接确认状态不可用。".to_string())?;
    pending.retain(|_, plan| plan.expires_at > Utc::now());
    pending
        .remove(token)
        .ok_or_else(|| "连接导入预览已过期或已使用，请重新预览。".to_string())
}

#[tauri::command]
pub(crate) async fn get_latest_legacy_provider_import<R: Runtime + 'static>(
    app: AppHandle<R>,
) -> Result<ProviderImportOperation, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = resolve_paths(&app)?;
        operation_view(&paths, read_journal(&paths)?.as_ref())
    })
    .await
    .map_err(|_| "无法读取连接迁移状态。".to_string())?
}

#[tauri::command]
pub(crate) async fn rollback_legacy_quotaviewer_provider_import<R: Runtime + 'static>(
    app: AppHandle<R>,
    operation_id: String,
    typed_confirmation: String,
) -> Result<ProviderImportOperation, String> {
    if typed_confirmation != ROLLBACK {
        return Err("请输入 ROLLBACK API 以确认撤销。".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let (lease, mut probe) = crate::commands::acquire_shared_mutation_lease()?;
        let _guard = crate::commands::account_switch_lock()
            .lock()
            .map_err(|_| "连接操作锁不可用。".to_string())?;
        let paths = resolve_paths(&app)?;
        let mut journal = read_journal(&paths)?
            .filter(|j| j.operation_id == operation_id)
            .ok_or_else(|| "该连接迁移已不再是最近一次操作，请重新打开预览。".to_string())?;
        lease
            .revalidate(&mut probe)
            .map_err(|_| "连接撤销安全互斥已失效。".to_string())?;
        rollback(&paths, &mut journal)?;
        notify(&app);
        operation_view(&paths, Some(&journal))
    })
    .await
    .map_err(|_| "连接撤销任务意外结束。".to_string())?
}

pub(crate) fn recover_interrupted_import<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let paths = resolve_paths(app)?;
    let Some(mut journal) = read_journal(&paths)? else {
        return Ok(());
    };
    if matches!(
        journal.state,
        JournalState::Committed | JournalState::RolledBack
    ) && store(&paths)?
        .observe(&stages(&paths, &journal)?)
        .is_ok_and(|state| absent(&state))
    {
        return Ok(());
    }
    let (lease, mut probe) = crate::commands::acquire_shared_mutation_lease()?;
    let _guard = crate::commands::account_switch_lock()
        .lock()
        .map_err(|_| "连接恢复互斥不可用。".to_string())?;
    lease
        .revalidate(&mut probe)
        .map_err(|_| "连接恢复安全互斥已失效。".to_string())?;
    if matches!(
        journal.state,
        JournalState::Preparing | JournalState::Staged
    ) {
        rollback(&paths, &mut journal)
    } else {
        remove_matching_stages(&paths, &journal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_credentials::tests::MemoryBackend;
    use capacity_migration::{LegacyAccountAuthMode, LegacyAccountImportRecord};

    fn fixture() -> (
        tempfile::TempDir,
        Paths,
        LegacyAccountImportBundle,
        ProviderProfile,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let paths = Paths {
            codex_home: root.join("codex"),
            current_auth: root.join("codex/auth.json"),
            current_config: root.join("codex/config.toml"),
            accounts: root.join("app/accounts"),
            providers: root.join("app/providers"),
            config_backup: root.join("app/config-backup.toml"),
            state_file: root.join("app/state.json"),
        };
        let record = LegacyAccountImportRecord { source_id: Uuid::new_v4().to_string(),
            display_name: "Fixture_API".into(), auth_mode: LegacyAccountAuthMode::ApiKey,
            created_at: "2026-09-07T00:00:00Z".into(), last_used_at: None,
            auth_json: br#"{"OPENAI_API_KEY":"fixture-private-key"}"#.to_vec(),
            config_toml: Some(b"model='fixture-model'\n[model_providers.openai]\nbase_url='https://fixture.test/v1'\n".to_vec()) };
        let profile = map_legacy_api_account(&record).unwrap().unwrap();
        (
            dir,
            paths,
            LegacyAccountImportBundle {
                preferred_source_id: None,
                accounts: vec![record],
            },
            profile,
        )
    }

    #[test]
    fn preview_is_read_only_secret_free_and_preserves_real_user_labels() {
        let (_dir, paths, bundle, _) = fixture();
        let before = bundle.accounts[0].auth_json.clone();
        let preview = prepare_rows(&paths, paths.codex_home.clone(), &bundle).unwrap();
        assert!(!paths.providers.exists());
        assert!(!journal_path(&paths).unwrap().exists());
        assert_eq!(preview.providers[0].disposition, "import");
        assert_eq!(preview.providers[0].label, "Fixture_API");
        let serialized = serde_json::to_string(&preview).unwrap();
        assert!(!serialized.contains("fixture-private-key"));
        assert!(!serialized.contains("OPENAI_API_KEY"));
        assert_eq!(bundle.accounts[0].auth_json, before);
    }

    #[test]
    fn protected_import_is_inactive_idempotent_and_reversible_without_changing_codex() {
        let (_dir, paths, _, profile) = fixture();
        ensure_private_directory(&paths.codex_home).unwrap();
        fs::write(&paths.current_auth, b"unchanged fixture login").unwrap();
        fs::write(&paths.current_config, b"unchanged fixture configuration").unwrap();
        let backend = MemoryBackend::default();
        let result = execute_import(
            &paths,
            &profile,
            |_| Ok(()),
            |p, v| backend.write_profile(p, v),
        )
        .unwrap();
        assert_eq!(result.status, "committed");
        let target = crate::providers::provider_path(&paths, &profile.id);
        let persisted = fs::read_to_string(&target).unwrap();
        assert!(!persisted.contains("fixture-private-key"));
        assert!(persisted.contains("credentialRef"));
        assert_eq!(
            backend.read_profile(&target).unwrap().api_key,
            profile.api_key
        );
        assert!(crate::providers::provider_field_modified_at_path(&paths, &profile.id).exists());
        assert!(!paths.state_file.exists());
        assert!(execute_import(
            &paths,
            &profile,
            |_| Ok(()),
            |p, v| backend.write_profile(p, v)
        )
        .is_err());
        let mut journal = read_journal(&paths).unwrap().unwrap();
        assert!(!fs::read_to_string(journal_path(&paths).unwrap())
            .unwrap()
            .contains("fixture-private-key"));
        rollback(&paths, &mut journal).unwrap();
        rollback(&paths, &mut journal).unwrap();
        assert!(!target.exists());
        assert!(!crate::providers::provider_field_modified_at_path(&paths, &profile.id).exists());
        assert_eq!(
            fs::read(&paths.current_auth).unwrap(),
            b"unchanged fixture login"
        );
        assert_eq!(
            fs::read(&paths.current_config).unwrap(),
            b"unchanged fixture configuration"
        );
    }

    #[test]
    fn source_or_lock_change_before_or_after_install_rolls_back_exact_owned_files() {
        for fail_on in [2, 3] {
            let (_dir, paths, _, profile) = fixture();
            let backend = MemoryBackend::default();
            let mut checks = 0;
            let error = execute_import(
                &paths,
                &profile,
                |phase| {
                    if phase == ImportCheck::RollbackLease {
                        return Ok(());
                    }
                    checks += 1;
                    if checks == fail_on {
                        Err("fixture source revision changed".into())
                    } else {
                        Ok(())
                    }
                },
                |p, v| backend.write_profile(p, v),
            )
            .err()
            .unwrap();
            assert!(error.contains("本次已撤回"));
            let journal = read_journal(&paths).unwrap().unwrap();
            assert!(journal.state == JournalState::RolledBack);
            assert!(absent(
                &store(&paths)
                    .unwrap()
                    .observe(&targets(&paths, &profile.id).unwrap())
                    .unwrap()
            ));
            assert!(absent(
                &store(&paths)
                    .unwrap()
                    .observe(&stages(&paths, &journal).unwrap())
                    .unwrap()
            ));
        }
    }

    #[test]
    fn credential_denial_never_installs_a_plaintext_fallback() {
        let (_dir, paths, _, profile) = fixture();
        let result = execute_import(
            &paths,
            &profile,
            |_| Ok(()),
            |_, _| Err("fixture denied".into()),
        );
        assert!(result.is_err());
        assert!(read_journal(&paths).unwrap().unwrap().state == JournalState::RolledBack);
        assert!(!crate::providers::provider_path(&paths, &profile.id).exists());
    }

    #[test]
    fn preserves_later_edits_and_active_connections_on_rollback() {
        let (_dir, paths, _, profile) = fixture();
        let backend = MemoryBackend::default();
        execute_import(
            &paths,
            &profile,
            |_| Ok(()),
            |p, v| backend.write_profile(p, v),
        )
        .unwrap();
        let mut journal = read_journal(&paths).unwrap().unwrap();
        let mut state = crate::models::ManagerStateFile::default();
        state.active_provider_id = Some(profile.id.clone());
        write_json_private(&paths.state_file, &state).unwrap();
        assert!(rollback(&paths, &mut journal).is_err());
        state.active_provider_id = None;
        write_json_private(&paths.state_file, &state).unwrap();
        let target = crate::providers::provider_path(&paths, &profile.id);
        let mut changed = backend.read_profile(&target).unwrap();
        changed.name = "User edited label".into();
        backend.write_profile(&target, &changed).unwrap();
        assert!(rollback(&paths, &mut journal).is_err());
        assert_eq!(
            backend.read_profile(&target).unwrap().name,
            "User edited label"
        );
    }

    #[test]
    fn target_created_during_confirmation_is_never_overwritten_or_deleted() {
        let (_dir, paths, _, profile) = fixture();
        let backend = MemoryBackend::default();
        let target = crate::providers::provider_path(&paths, &profile.id);
        let mut checks = 0;
        let result = execute_import(
            &paths,
            &profile,
            |phase| {
                if phase == ImportCheck::RollbackLease {
                    return Ok(());
                }
                checks += 1;
                if checks == 2 {
                    fs::write(&target, b"user-owned competing file").unwrap();
                }
                Ok(())
            },
            |p, v| backend.write_profile(p, v),
        );
        assert!(result.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"user-owned competing file");
        let journal = read_journal(&paths).unwrap().unwrap();
        assert!(
            operation_view(&paths, Some(&journal))
                .unwrap()
                .recovery_required
        );
    }

    #[test]
    fn interrupted_partial_install_can_recover_without_reading_system_credentials() {
        let (_dir, paths, _, profile) = fixture();
        ensure_private_directory(&paths.providers).unwrap();
        let store = store(&paths).unwrap();
        let targets = targets(&paths, &profile.id).unwrap();
        let point = store
            .create_restore_point(
                RestorePointOperation::ImportLegacyProvider,
                &targets,
                false,
                &now().unwrap(),
                &BTreeSet::new(),
            )
            .unwrap();
        let mut journal = Journal {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            provider_id: profile.id.clone(),
            label: profile.name.clone(),
            point: point.id().clone(),
            state: JournalState::Staged,
            after: None,
        };
        let stages = stages(&paths, &journal).unwrap();
        let backend = MemoryBackend::default();
        backend.write_profile(stages[0].path(), &profile).unwrap();
        write_json_private(
            stages[1].path(),
            &crate::providers::new_imported_provider_field_metadata(),
        )
        .unwrap();
        journal.after = Some(store.observe(&stages).unwrap());
        write_journal(&paths, &journal).unwrap();
        fs::hard_link(stages[0].path(), targets[0].path()).unwrap();
        drop(backend); // Recovery requires only public file observations.
        let mut recovered = read_journal(&paths).unwrap().unwrap();
        rollback(&paths, &mut recovered).unwrap();
        assert!(absent(&store.observe(&targets).unwrap()));
        assert!(absent(&store.observe(&stages).unwrap()));
    }

    #[test]
    fn matching_current_metadata_is_reviewed_without_accessing_its_key() {
        let (_dir, paths, bundle, profile) = fixture();
        ensure_private_directory(&paths.providers).unwrap();
        let existing = paths.providers.join("existing-independent-id.json");
        write_json_private(
            &existing,
            &serde_json::json!({"name": profile.name, "model": profile.model,
            "baseUrl": profile.base_url, "credentialRef": "fixture-unreadable-keychain-reference"}),
        )
        .unwrap();
        let preview = prepare_rows(&paths, paths.codex_home.clone(), &bundle).unwrap();
        assert_eq!(preview.providers[0].disposition, "keep_existing");
        assert!(preview.providers[0].confirm_token.is_none());
    }

    #[test]
    fn confirmation_is_exact_single_use_and_expires() {
        let (_dir, paths, bundle, profile) = fixture();
        let preview = prepare_rows(&paths, paths.codex_home.clone(), &bundle).unwrap();
        let token = preview.providers[0].confirm_token.as_deref().unwrap();
        assert!(consume_confirmation(token, "import api").is_err());
        assert_eq!(
            consume_confirmation(token, IMPORT).unwrap().profile.id,
            profile.id
        );
        assert!(consume_confirmation(token, IMPORT).is_err());
        let preview = prepare_rows(&paths, paths.codex_home.clone(), &bundle).unwrap();
        let token = preview.providers[0].confirm_token.as_deref().unwrap();
        PENDING.lock().unwrap().get_mut(token).unwrap().expires_at =
            Utc::now() - Duration::seconds(1);
        assert!(consume_confirmation(token, IMPORT).is_err());
    }

    #[test]
    fn loss_of_actual_lease_prevents_compensation_writes_until_reacquired() {
        let (_dir, paths, _, profile) = fixture();
        let backend = MemoryBackend::default();
        let mut checks = 0;
        let result = execute_import(
            &paths,
            &profile,
            |phase| {
                if phase == ImportCheck::RollbackLease {
                    return Err("fixture lease lost".into());
                }
                checks += 1;
                if checks == 3 {
                    Err("fixture lease lost".into())
                } else {
                    Ok(())
                }
            },
            |p, v| backend.write_profile(p, v),
        );
        assert!(result.is_err());
        let mut journal = read_journal(&paths).unwrap().unwrap();
        assert!(journal.state == JournalState::Staged);
        assert!(crate::providers::provider_path(&paths, &profile.id).exists());
        // Simulate a later recovery that has successfully reacquired the lease.
        rollback(&paths, &mut journal).unwrap();
        assert!(!crate::providers::provider_path(&paths, &profile.id).exists());
    }

    #[test]
    fn missing_ownership_observations_never_authorize_removing_a_profile() {
        let (_dir, paths, _, profile) = fixture();
        let backend = MemoryBackend::default();
        execute_import(
            &paths,
            &profile,
            |_| Ok(()),
            |p, v| backend.write_profile(p, v),
        )
        .unwrap();
        let mut journal = read_journal(&paths).unwrap().unwrap();
        journal.after = None;
        assert!(!rollback_ready(&paths, &journal));
        assert!(rollback(&paths, &mut journal).is_err());
        assert!(crate::providers::provider_path(&paths, &profile.id).exists());
    }

    #[cfg(unix)]
    #[test]
    fn preview_rejects_symlinked_provider_directories_without_reading_through_them() {
        let (dir, paths, bundle, _) = fixture();
        ensure_private_directory(app_data(&paths).unwrap()).unwrap();
        let external = dir.path().join("external");
        fs::create_dir(&external).unwrap();
        std::os::unix::fs::symlink(&external, &paths.providers).unwrap();
        assert!(prepare_rows(&paths, paths.codex_home.clone(), &bundle).is_err());
        assert_eq!(fs::read_dir(external).unwrap().count(), 0);
    }
}
