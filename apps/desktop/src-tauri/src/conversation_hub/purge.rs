const THREAD_PURGE_PLAN_PREFIX: &str = "session-purge-plan:v1:";
const THREAD_PURGE_PLAN_TTL_SECONDS: i64 = 120;
const MAX_PENDING_THREAD_PURGE_PLANS: usize = 32;
const MAX_THREAD_PURGE_TARGETS: usize = 512;
const PURGE_RESTORE_FORMAT: &str = "quota-horizon-session-purge-v1";
const PURGE_RESTORE_POINT_PREFIX: &str = "session-purge-rp-v1-";
const PURGE_RESTORE_MANIFEST: &str = "manifest.json";
const PURGE_RESTORE_PAYLOAD: &str = "payload.zip";
const PURGE_RESTORE_PAYLOAD_ENTRY: &str = "restore-payload.json";
const MAX_PURGE_RESTORE_PAYLOAD_JSON_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
struct PendingThreadPurgePlan {
    session_ids: Vec<String>,
    empty_bin: bool,
    expected_revision: String,
    expires_at: DateTime<Utc>,
}

#[derive(Default)]
struct PendingThreadPurgePlans {
    plans: HashMap<String, PendingThreadPurgePlan>,
}

impl PendingThreadPurgePlans {
    fn issue(
        &mut self,
        session_ids: Vec<String>,
        empty_bin: bool,
        expected_revision: String,
        now: DateTime<Utc>,
    ) -> Result<(String, DateTime<Utc>), String> {
        self.plans.retain(|_, plan| plan.expires_at > now);
        if self.plans.len() >= MAX_PENDING_THREAD_PURGE_PLANS {
            return Err("待确认的永久清除操作过多，请稍后重试。".to_string());
        }
        let token = format!("{THREAD_PURGE_PLAN_PREFIX}{}", Uuid::new_v4().hyphenated());
        let expires_at = now + chrono::Duration::seconds(THREAD_PURGE_PLAN_TTL_SECONDS);
        self.plans.insert(
            token.clone(),
            PendingThreadPurgePlan {
                session_ids,
                empty_bin,
                expected_revision,
                expires_at,
            },
        );
        Ok((token, expires_at))
    }

    fn consume(
        &mut self,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<PendingThreadPurgePlan, String> {
        validate_thread_purge_plan_token(token)?;
        let plan = self
            .plans
            .remove(token)
            .ok_or_else(|| "永久清除确认已失效，请重新预览后再执行。".to_string())?;
        if plan.expires_at <= now {
            return Err("永久清除确认已过期，请重新预览后再执行。".to_string());
        }
        Ok(plan)
    }
}

fn validate_thread_purge_plan_token(token: &str) -> Result<(), String> {
    let value = token
        .strip_prefix(THREAD_PURGE_PLAN_PREFIX)
        .ok_or_else(|| "永久清除确认令牌格式无效。".to_string())?;
    let parsed = Uuid::parse_str(value).map_err(|_| "永久清除确认令牌格式无效。".to_string())?;
    if parsed.hyphenated().to_string() != value {
        return Err("永久清除确认令牌格式无效。".to_string());
    }
    Ok(())
}

fn pending_thread_purge_plans() -> &'static std::sync::Mutex<PendingThreadPurgePlans> {
    static PLANS: std::sync::OnceLock<std::sync::Mutex<PendingThreadPurgePlans>> =
        std::sync::OnceLock::new();
    PLANS.get_or_init(|| std::sync::Mutex::new(PendingThreadPurgePlans::default()))
}

fn issue_thread_purge_plan(
    session_ids: Vec<String>,
    empty_bin: bool,
    expected_revision: String,
) -> Result<(String, DateTime<Utc>), String> {
    pending_thread_purge_plans()
        .lock()
        .map_err(|_| "永久清除预览状态不可用。".to_string())?
        .issue(session_ids, empty_bin, expected_revision, Utc::now())
}

fn consume_thread_purge_plan(token: &str) -> Result<PendingThreadPurgePlan, String> {
    pending_thread_purge_plans()
        .lock()
        .map_err(|_| "永久清除预览状态不可用。".to_string())?
        .consume(token, Utc::now())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PurgeRestoreState {
    Prepared,
    Committed,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PurgeBackupFile {
    relative_path: String,
    content_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PurgeBinBackup {
    session_id: String,
    original_relative_folder: String,
    archive_prefix: String,
    files: Vec<PurgeBackupFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PurgeRestoreManifest {
    format: String,
    id: String,
    created_at: String,
    state: PurgeRestoreState,
    session_ids: Vec<String>,
    payload_sha256: String,
    bin_backups: Vec<PurgeBinBackup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PurgeOwnershipSnapshot {
    session_id: String,
    account_id: Option<String>,
    observed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PurgeSessionStateSnapshot {
    session_id: String,
    sqlite: Vec<SqliteTableSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PurgeRestorePayload {
    sessions: Vec<PurgeSessionStateSnapshot>,
    ownership: Vec<PurgeOwnershipSnapshot>,
}

#[derive(Debug, Clone)]
struct PreparedPurgeFile {
    path: PathBuf,
    relative_path: PathBuf,
    content_revision: String,
}

#[derive(Debug, Clone)]
struct PreparedPurgeBin {
    snapshot: BinSnapshot,
    files: Vec<PreparedPurgeFile>,
}

#[derive(Debug, Clone)]
struct PreparedThreadPurgeItem {
    session_id: String,
    title: String,
    cwd: String,
    size_bytes: u64,
    released_bytes: u64,
    bins: Vec<PreparedPurgeBin>,
    sqlite: Vec<SqliteTableSnapshot>,
    ownership: PurgeOwnershipSnapshot,
}

#[derive(Debug, Clone)]
struct PreparedThreadPurge {
    empty_bin: bool,
    requested_count: usize,
    database_paths: Vec<PathBuf>,
    items: Vec<PreparedThreadPurgeItem>,
    revision: String,
}

#[derive(Debug)]
struct PurgeRestorePoint {
    directory: PathBuf,
    manifest: PurgeRestoreManifest,
}

fn ensure_purge_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法验证{label}目录 {}：{error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(format!("{label}目录类型不安全：{}", path.display()));
    }
    Ok(())
}

fn collect_regular_tree(root: &Path) -> Result<Vec<PathBuf>, String> {
    fn visit(root: &Path, current: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
        for entry in fs::read_dir(current).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            let path = entry.path();
            if file_type.is_symlink() {
                return Err(format!(
                    "永久清除目标包含不安全的 symlink：{}",
                    path.display()
                ));
            }
            if file_type.is_dir() {
                visit(root, &path, files)?;
            } else if file_type.is_file() {
                path.strip_prefix(root)
                    .map_err(|_| "永久清除目标越出受信任目录。".to_string())?;
                files.push(path);
            } else {
                return Err(format!(
                    "永久清除目标包含不支持的文件类型：{}",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    ensure_purge_directory(root, "永久清除目标")?;
    let mut files = Vec::new();
    visit(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn purge_database_paths(codex_home: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = [
        latest_state_db(codex_home),
        latest_versioned_db(codex_home, "thread_history_"),
        latest_versioned_db(codex_home, "queue_"),
        latest_versioned_db(codex_home, "goals_"),
        latest_versioned_db(codex_home, "memories_"),
        latest_versioned_db(codex_home, "logs_"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let catalogs = codex_home.join("sqlite");
    if catalogs.exists() {
        ensure_purge_directory(&catalogs, "Codex catalog")?;
        for entry in fs::read_dir(catalogs).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
                && entry.path().extension().and_then(|value| value.to_str()) == Some("db")
            {
                paths.push(entry.path());
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn ensure_purge_threads_have_no_cross_edges(
    state_db: Option<&Path>,
    requested: &HashSet<String>,
) -> Result<(), String> {
    let Some(state_db) = state_db else {
        return Ok(());
    };
    let connection =
        Connection::open(state_db).map_err(|error| format!("无法打开 Codex state DB：{error}"))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    if !table_exists(&connection, "thread_spawn_edges")? {
        return Ok(());
    }
    let mut statement = connection
        .prepare("SELECT parent_thread_id, child_thread_id FROM thread_spawn_edges")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?;
    for row in rows {
        let (parent, child) = row.map_err(|error| error.to_string())?;
        if requested.contains(&parent) != requested.contains(&child) {
            return Err(format!(
                "会话 {parent} 与 {child} 仍存在跨目标父子关系，请同时选择后再永久清除"
            ));
        }
    }
    Ok(())
}

fn canonical_sqlite_snapshot_bytes(snapshots: &[SqliteTableSnapshot]) -> Result<Vec<u8>, String> {
    let mut tables = snapshots
        .iter()
        .map(|snapshot| {
            let mut rows = snapshot
                .rows
                .iter()
                .map(|row| serde_json::to_vec(row).map_err(|error| error.to_string()))
                .collect::<Result<Vec<_>, _>>()?;
            rows.sort();
            Ok((snapshot.database.clone(), snapshot.table.clone(), rows))
        })
        .collect::<Result<Vec<_>, String>>()?;
    tables.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    serde_json::to_vec(&tables).map_err(|error| error.to_string())
}

fn thread_purge_revision(prepared: &PreparedThreadPurge) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest_thread_trash_field(&mut digest, b"session-purge-revision:v1");
    digest_thread_trash_field(&mut digest, &[u8::from(prepared.empty_bin)]);
    for path in &prepared.database_paths {
        digest_thread_trash_field(&mut digest, path.to_string_lossy().as_bytes());
    }
    for item in &prepared.items {
        digest_thread_trash_field(&mut digest, item.session_id.as_bytes());
        digest_thread_trash_field(&mut digest, &canonical_sqlite_snapshot_bytes(&item.sqlite)?);
        digest_thread_trash_field(
            &mut digest,
            &serde_json::to_vec(&item.ownership).map_err(|error| error.to_string())?,
        );
        for bin in &item.bins {
            digest_thread_trash_field(
                &mut digest,
                bin.snapshot.folder.to_string_lossy().as_bytes(),
            );
            digest_thread_trash_field(&mut digest, bin.snapshot.manifest_revision.as_bytes());
            for file in &bin.files {
                digest_thread_trash_field(
                    &mut digest,
                    file.relative_path.to_string_lossy().as_bytes(),
                );
                digest_thread_trash_field(&mut digest, file.content_revision.as_bytes());
            }
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn prepare_thread_purge(
    codex_home: &Path,
    trash_root: &Path,
    manager_state: &crate::models::ManagerStateFile,
    session_ids: Vec<String>,
    empty_bin: bool,
) -> Result<PreparedThreadPurge, String> {
    let entries = collect_bin_entries_from_root(trash_root)?;
    let requested = if empty_bin {
        entries
            .iter()
            .map(|entry| entry.manifest.session_id.clone())
            .collect::<HashSet<_>>()
    } else {
        normalized_ids(session_ids)
    };
    if requested.is_empty() {
        return Err(if empty_bin {
            "回收站已经为空。".to_string()
        } else {
            "请至少选择一条要永久清除的会话。".to_string()
        });
    }
    if requested.len() > MAX_THREAD_PURGE_TARGETS {
        return Err(format!(
            "单次最多可永久清除 {MAX_THREAD_PURGE_TARGETS} 条会话。"
        ));
    }
    let active = gather_snapshots(codex_home)?;
    if active
        .iter()
        .any(|snapshot| requested.contains(&snapshot.session_id))
    {
        return Err("当前活动会话与回收站目标 ID 冲突，已拒绝永久清除。".to_string());
    }
    ensure_threads_are_not_referenced(&active, &requested, latest_state_db(codex_home).as_deref())?;
    ensure_purge_threads_have_no_cross_edges(latest_state_db(codex_home).as_deref(), &requested)?;
    let index = index_values(codex_home)?;
    if requested.iter().any(|id| index.contains_key(id)) {
        return Err("当前会话索引仍包含永久清除目标，已拒绝操作。".to_string());
    }
    let database_paths = purge_database_paths(codex_home)?;
    let mut ids = requested.iter().cloned().collect::<Vec<_>>();
    ids.sort();
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        let mut matching = entries
            .iter()
            .filter(|entry| entry.manifest.session_id == id)
            .cloned()
            .collect::<Vec<_>>();
        if matching.is_empty() {
            return Err(format!("回收站中的会话 {id} 已不存在，请刷新列表。"));
        }
        matching.sort_by(|left, right| left.folder.cmp(&right.folder));
        let current_visibility =
            state_visibility_snapshot(latest_state_db(codex_home).as_deref(), &id)?;
        if let Some(visibility) = &current_visibility {
            let matches_hidden_source = matching.iter().any(|bin| {
                safe_relative_path(&bin.manifest.relative_rollout_path)
                    .map(|relative| bin.folder.join("files").join(relative))
                    .is_some_and(|path| path == Path::new(&visibility.rollout_path))
            });
            if visibility.archived != 1 || !visibility.preview.is_empty() || !matches_hidden_source
            {
                return Err(format!("会话 {id} 的隐藏状态与回收站内容不一致。"));
            }
        } else if matching
            .iter()
            .any(|bin| bin.manifest.state_visibility.is_some())
        {
            return Err(format!(
                "会话 {id} 的 state row 已异常消失，已拒绝永久清除。"
            ));
        }
        let mut bins = Vec::with_capacity(matching.len());
        let mut size_bytes = 0u64;
        let mut released_bytes = 0u64;
        for snapshot in matching {
            let mut files = Vec::new();
            for path in collect_regular_tree(&snapshot.folder)? {
                let relative_path = path
                    .strip_prefix(&snapshot.folder)
                    .map_err(|_| "回收站条目越出受信任目录。".to_string())?
                    .to_path_buf();
                let relative_text = relative_path
                    .to_str()
                    .ok_or_else(|| "回收站条目路径不是有效文本。".to_string())?;
                if safe_relative_path(relative_text).as_ref() != Some(&relative_path) {
                    return Err("回收站条目包含不安全路径。".to_string());
                }
                files.push(PreparedPurgeFile {
                    content_revision: stable_bin_rollout_revision(&path)?,
                    path,
                    relative_path,
                });
            }
            if files.is_empty() {
                return Err(format!("会话 {id} 的回收站条目为空。"));
            }
            size_bytes = size_bytes.saturating_add(
                snapshot
                    .rollouts
                    .iter()
                    .filter_map(|path| fs::metadata(path).ok())
                    .map(|metadata| metadata.len())
                    .sum(),
            );
            released_bytes = released_bytes.saturating_add(directory_size(&snapshot.folder));
            bins.push(PreparedPurgeBin { snapshot, files });
        }
        let first = &bins[0].snapshot.manifest;
        items.push(PreparedThreadPurgeItem {
            session_id: id.clone(),
            title: first.title.clone(),
            cwd: first.cwd.clone(),
            size_bytes,
            released_bytes,
            bins,
            sqlite: snapshot_purge_related_state(codex_home, &id)?,
            ownership: PurgeOwnershipSnapshot {
                session_id: id.clone(),
                account_id: manager_state.conversation_account_ids.get(&id).cloned(),
                observed: manager_state.observed_conversation_ids.contains(&id),
            },
        });
    }
    let mut prepared = PreparedThreadPurge {
        empty_bin,
        requested_count: requested.len(),
        database_paths,
        items,
        revision: String::new(),
    };
    prepared.revision = thread_purge_revision(&prepared)?;
    Ok(prepared)
}

fn purge_restore_root<R: Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法定位应用数据目录：{error}"))?
        .join("codex-thread-purge-transactions-v1"))
}

fn sync_purge_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("无法同步永久清除恢复点目录：{error}"))
}

fn harden_purge_restore_permissions(path: &Path, directory: bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if directory { 0o700 } else { 0o600 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|error| format!("无法保护永久清除恢复点权限：{error}"))?;
    }
    #[cfg(not(unix))]
    let _ = (path, directory);
    Ok(())
}

fn write_purge_restore_manifest(
    directory: &Path,
    manifest: &PurgeRestoreManifest,
) -> Result<(), String> {
    let text = serde_json::to_string_pretty(manifest)
        .map_err(|_| "无法序列化永久清除恢复点。".to_string())?;
    write_text_atomic(
        &directory.join(PURGE_RESTORE_MANIFEST),
        &format!("{text}\n"),
    )?;
    harden_purge_restore_permissions(&directory.join(PURGE_RESTORE_MANIFEST), false)?;
    sync_purge_directory(directory)
}

fn valid_purge_content_revision(value: &str) -> bool {
    let Some((size, sha256)) = value.split_once('\0') else {
        return false;
    };
    size.parse::<u64>().is_ok()
        && sha256.len() == 64
        && sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_purge_restore_manifest(
    directory: &Path,
    manifest: &PurgeRestoreManifest,
) -> Result<(), String> {
    let Some(id_suffix) = manifest.id.strip_prefix(PURGE_RESTORE_POINT_PREFIX) else {
        return Err("永久清除恢复点 manifest 身份无效。".to_string());
    };
    let parsed =
        Uuid::parse_str(id_suffix).map_err(|_| "永久清除恢复点 manifest 身份无效。".to_string())?;
    if manifest.format != PURGE_RESTORE_FORMAT
        || directory.file_name().and_then(|value| value.to_str()) != Some(manifest.id.as_str())
        || parsed.hyphenated().to_string() != id_suffix
        || DateTime::parse_from_rfc3339(&manifest.created_at).is_err()
        || manifest.payload_sha256.len() != 64
        || !manifest
            .payload_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("永久清除恢复点 manifest 身份无效。".to_string());
    }
    if manifest.session_ids.is_empty()
        || manifest.session_ids.len() > MAX_THREAD_PURGE_TARGETS
        || manifest.session_ids.iter().any(|value| value.is_empty())
    {
        return Err("永久清除恢复点目标集合无效。".to_string());
    }
    let session_ids = manifest.session_ids.iter().collect::<HashSet<_>>();
    if session_ids.len() != manifest.session_ids.len() {
        return Err("永久清除恢复点包含重复目标。".to_string());
    }
    let mut covered_sessions = HashSet::new();
    let mut original_folders = HashSet::new();
    for (index, backup) in manifest.bin_backups.iter().enumerate() {
        if !session_ids.contains(&backup.session_id)
            || backup.archive_prefix != format!("entries/{index:04}")
            || safe_relative_path(&backup.original_relative_folder).is_none()
            || !original_folders.insert(backup.original_relative_folder.clone())
            || backup.files.is_empty()
        {
            return Err("永久清除恢复点 bin 清单无效。".to_string());
        }
        covered_sessions.insert(backup.session_id.as_str());
        let mut relative_files = HashSet::new();
        for file in &backup.files {
            let Some(relative) = safe_relative_path(&file.relative_path) else {
                return Err("永久清除恢复点包含不安全的文件路径。".to_string());
            };
            if relative.to_string_lossy().replace('\\', "/") != file.relative_path
                || !relative_files.insert(file.relative_path.clone())
                || !valid_purge_content_revision(&file.content_revision)
            {
                return Err("永久清除恢复点文件清单无效。".to_string());
            }
        }
    }
    if covered_sessions.len() != session_ids.len()
        || session_ids
            .iter()
            .any(|session_id| !covered_sessions.contains(session_id.as_str()))
    {
        return Err("永久清除恢复点未覆盖全部目标。".to_string());
    }
    Ok(())
}

fn read_purge_restore_manifest(directory: &Path) -> Result<PurgeRestoreManifest, String> {
    ensure_purge_directory(directory, "永久清除恢复点")?;
    let path = directory.join(PURGE_RESTORE_MANIFEST);
    let before =
        fs::symlink_metadata(&path).map_err(|error| format!("无法读取永久清除恢复点：{error}"))?;
    if before.file_type().is_symlink() || !before.file_type().is_file() {
        return Err("永久清除恢复点 manifest 文件类型不安全。".to_string());
    }
    let bytes = fs::read(&path).map_err(|error| format!("无法读取永久清除恢复点：{error}"))?;
    let after =
        fs::symlink_metadata(&path).map_err(|error| format!("无法复核永久清除恢复点：{error}"))?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err("永久清除恢复点 manifest 正在变化。".to_string());
    }
    let manifest: PurgeRestoreManifest = serde_json::from_slice(&bytes)
        .map_err(|_| "永久清除恢复点 manifest 已损坏。".to_string())?;
    validate_purge_restore_manifest(directory, &manifest)?;
    Ok(manifest)
}

fn create_purge_restore_point(
    restore_root: &Path,
    trash_root: &Path,
    prepared: &PreparedThreadPurge,
) -> Result<PurgeRestorePoint, String> {
    fs::create_dir_all(restore_root)
        .map_err(|error| format!("无法创建永久清除恢复点目录：{error}"))?;
    ensure_purge_directory(restore_root, "永久清除恢复点")?;
    harden_purge_restore_permissions(restore_root, true)?;
    let id = format!(
        "{PURGE_RESTORE_POINT_PREFIX}{}",
        Uuid::new_v4().hyphenated()
    );
    let staging = restore_root.join(format!(".capture-{}", Uuid::new_v4().hyphenated()));
    fs::create_dir(&staging).map_err(|error| format!("无法创建恢复点 staging：{error}"))?;
    harden_purge_restore_permissions(&staging, true)?;
    let result = (|| -> Result<PurgeRestorePoint, String> {
        let payload_path = staging.join(PURGE_RESTORE_PAYLOAD);
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&payload_path)
            .map_err(|error| format!("无法创建永久清除恢复 payload：{error}"))?;
        harden_purge_restore_permissions(&payload_path, false)?;
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let mut bin_backups = Vec::new();
        let mut bin_index = 0usize;
        for item in &prepared.items {
            for bin in &item.bins {
                let relative_folder = bin
                    .snapshot
                    .folder
                    .strip_prefix(trash_root)
                    .map_err(|_| "回收站条目越出受信任目录。".to_string())?;
                let relative_folder_text = relative_folder
                    .to_str()
                    .ok_or_else(|| "回收站条目路径不是有效文本。".to_string())?;
                if safe_relative_path(relative_folder_text).as_ref()
                    != Some(&relative_folder.to_path_buf())
                {
                    return Err("回收站条目路径不安全。".to_string());
                }
                let archive_prefix = format!("entries/{bin_index:04}");
                bin_index += 1;
                let mut files = Vec::new();
                for source in &bin.files {
                    if stable_bin_rollout_revision(&source.path)? != source.content_revision {
                        return Err("回收站内容在恢复点创建前发生变化。".to_string());
                    }
                    let relative = source.relative_path.to_string_lossy().replace('\\', "/");
                    let archive_name = format!("{archive_prefix}/{relative}");
                    archive
                        .start_file(&archive_name, options)
                        .map_err(|error| format!("无法写入永久清除恢复 payload：{error}"))?;
                    let mut input = File::open(&source.path)
                        .map_err(|error| format!("无法读取回收站文件：{error}"))?;
                    std::io::copy(&mut input, &mut archive)
                        .map_err(|error| format!("无法复制回收站文件到恢复点：{error}"))?;
                    if stable_bin_rollout_revision(&source.path)? != source.content_revision {
                        return Err("回收站内容在恢复点创建期间发生变化。".to_string());
                    }
                    files.push(PurgeBackupFile {
                        relative_path: relative,
                        content_revision: source.content_revision.clone(),
                    });
                }
                bin_backups.push(PurgeBinBackup {
                    session_id: item.session_id.clone(),
                    original_relative_folder: relative_folder_text.replace('\\', "/"),
                    archive_prefix,
                    files,
                });
            }
        }
        let payload = PurgeRestorePayload {
            sessions: prepared
                .items
                .iter()
                .map(|item| PurgeSessionStateSnapshot {
                    session_id: item.session_id.clone(),
                    sqlite: item.sqlite.clone(),
                })
                .collect(),
            ownership: prepared
                .items
                .iter()
                .map(|item| item.ownership.clone())
                .collect(),
        };
        let payload_bytes =
            serde_json::to_vec(&payload).map_err(|_| "无法序列化永久清除状态快照。".to_string())?;
        if payload_bytes.len() > MAX_PURGE_RESTORE_PAYLOAD_JSON_BYTES {
            return Err("永久清除状态快照过大，已拒绝操作。".to_string());
        }
        let payload_sha256 = format!("{:x}", Sha256::digest(&payload_bytes));
        archive
            .start_file(PURGE_RESTORE_PAYLOAD_ENTRY, options)
            .map_err(|error| format!("无法写入永久清除状态快照：{error}"))?;
        archive
            .write_all(&payload_bytes)
            .map_err(|error| format!("无法写入永久清除状态快照：{error}"))?;
        let payload_file = archive
            .finish()
            .map_err(|error| format!("无法完成永久清除恢复 payload：{error}"))?;
        payload_file
            .sync_all()
            .map_err(|error| format!("无法同步永久清除恢复 payload：{error}"))?;
        let manifest = PurgeRestoreManifest {
            format: PURGE_RESTORE_FORMAT.to_string(),
            id: id.clone(),
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            state: PurgeRestoreState::Prepared,
            session_ids: prepared
                .items
                .iter()
                .map(|item| item.session_id.clone())
                .collect(),
            payload_sha256,
            bin_backups,
        };
        write_purge_restore_manifest(&staging, &manifest)?;
        sync_purge_directory(&staging)?;
        let destination = restore_root.join(&id);
        fs::rename(&staging, &destination)
            .map_err(|error| format!("无法提交永久清除恢复点：{error}"))?;
        sync_purge_directory(restore_root)?;
        let verified = read_purge_restore_manifest(&destination)?;
        if verified.payload_sha256 != manifest.payload_sha256 {
            return Err("永久清除恢复点提交后校验失败。".to_string());
        }
        let point = PurgeRestorePoint {
            directory: destination,
            manifest: verified,
        };
        load_purge_restore_payload(&point)?;
        Ok(point)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn validate_purge_archive_layout(
    archive: &mut ZipArchive<File>,
    manifest: &PurgeRestoreManifest,
) -> Result<(), String> {
    let mut expected = HashMap::new();
    expected.insert(PURGE_RESTORE_PAYLOAD_ENTRY.to_string(), None);
    for backup in &manifest.bin_backups {
        for file in &backup.files {
            let name = format!("{}/{}", backup.archive_prefix, file.relative_path);
            if expected
                .insert(name, expected_revision_size(&file.content_revision))
                .is_some()
            {
                return Err("永久清除恢复 payload 文件清单重复。".to_string());
            }
        }
    }
    if archive.len() != expected.len() {
        return Err("永久清除恢复 payload 文件数量不一致。".to_string());
    }
    let mut observed = HashSet::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("永久清除恢复 payload 已损坏：{error}"))?;
        let name = entry.name().to_string();
        let Some(expected_size) = expected.get(&name) else {
            return Err("永久清除恢复 payload 包含意外文件。".to_string());
        };
        if entry.is_dir()
            || entry.compression() != CompressionMethod::Stored
            || !observed.insert(name)
            || expected_size.is_some_and(|size| size != entry.size())
        {
            return Err("永久清除恢复 payload 文件属性无效。".to_string());
        }
    }
    if observed.len() != expected.len() {
        return Err("永久清除恢复 payload 文件集合不一致。".to_string());
    }
    Ok(())
}

fn load_purge_restore_payload(point: &PurgeRestorePoint) -> Result<PurgeRestorePayload, String> {
    let path = point.directory.join(PURGE_RESTORE_PAYLOAD);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("永久清除恢复 payload 不可用：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("永久清除恢复 payload 文件类型不安全。".to_string());
    }
    let file =
        File::open(&path).map_err(|error| format!("无法读取永久清除恢复 payload：{error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("永久清除恢复 payload 已损坏：{error}"))?;
    validate_purge_archive_layout(&mut archive, &point.manifest)?;
    let mut entry = archive
        .by_name(PURGE_RESTORE_PAYLOAD_ENTRY)
        .map_err(|_| "永久清除恢复 payload 缺少状态快照。".to_string())?;
    if entry.size() > MAX_PURGE_RESTORE_PAYLOAD_JSON_BYTES as u64 {
        return Err("永久清除恢复状态快照过大。".to_string());
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut bytes)
        .map_err(|error| format!("无法读取永久清除状态快照：{error}"))?;
    if format!("{:x}", Sha256::digest(&bytes)) != point.manifest.payload_sha256 {
        return Err("永久清除状态快照校验失败。".to_string());
    }
    let payload: PurgeRestorePayload =
        serde_json::from_slice(&bytes).map_err(|_| "永久清除状态快照格式无效。".to_string())?;
    let mut session_ids = payload
        .sessions
        .iter()
        .map(|item| item.session_id.clone())
        .collect::<Vec<_>>();
    session_ids.sort();
    let mut expected = point.manifest.session_ids.clone();
    expected.sort();
    let mut ownership_ids = payload
        .ownership
        .iter()
        .map(|item| item.session_id.clone())
        .collect::<Vec<_>>();
    ownership_ids.sort();
    if session_ids != expected || ownership_ids != expected {
        return Err("永久清除状态快照目标集合不一致。".to_string());
    }
    let expected_set = expected.iter().map(String::as_str).collect::<HashSet<_>>();
    for session in &payload.sessions {
        let mut tables = HashSet::new();
        for snapshot in &session.sqlite {
            if !valid_purge_related_table(&snapshot.database, &snapshot.table)
                || snapshot.rows.is_empty()
                || !tables.insert((snapshot.database.as_str(), snapshot.table.as_str()))
            {
                return Err("永久清除状态快照包含无效数据库表。".to_string());
            }
            for row in &snapshot.rows {
                if row.columns.len() != row.values.len()
                    || row.columns.iter().collect::<HashSet<_>>().len() != row.columns.len()
                {
                    return Err("永久清除状态快照包含无效数据库行。".to_string());
                }
                let belongs_to_session = if snapshot.table == "threads" {
                    sqlite_row_text(row, "id") == Some(session.session_id.as_str())
                } else if snapshot.table == "thread_spawn_edges" {
                    let parent = sqlite_row_text(row, "parent_thread_id");
                    let child = sqlite_row_text(row, "child_thread_id");
                    (parent == Some(session.session_id.as_str())
                        || child == Some(session.session_id.as_str()))
                        && parent.is_some_and(|value| expected_set.contains(value))
                        && child.is_some_and(|value| expected_set.contains(value))
                } else {
                    sqlite_row_text(row, "thread_id") == Some(session.session_id.as_str())
                };
                if !belongs_to_session {
                    return Err("永久清除状态快照包含无关数据库行。".to_string());
                }
            }
        }
    }
    Ok(payload)
}

fn expected_revision_size(revision: &str) -> Option<u64> {
    revision.split_once('\0')?.0.parse().ok()
}

fn ensure_purge_restore_parent_safe(root: &Path, target: &Path) -> Result<(), String> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| "永久清除恢复目标越出受信任目录。".to_string())?;
    let mut current = root.to_path_buf();
    for component in relative.parent().into_iter().flat_map(Path::components) {
        let std::path::Component::Normal(value) = component else {
            return Err("永久清除恢复目标路径不安全。".to_string());
        };
        current.push(value);
        if let Ok(metadata) = fs::symlink_metadata(&current) {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err("永久清除恢复目标父目录不安全。".to_string());
            }
        }
    }
    Ok(())
}

fn verify_purge_bin_backups(trash_root: &Path, backups: &[PurgeBinBackup]) -> Result<(), String> {
    for backup in backups {
        let relative_folder = safe_relative_path(&backup.original_relative_folder)
            .ok_or_else(|| "永久清除恢复点包含不安全的 bin 路径。".to_string())?;
        let folder = trash_root.join(relative_folder);
        let observed = collect_regular_tree(&folder)?;
        if observed.len() != backup.files.len() {
            return Err("永久清除 bin 恢复结果文件数不一致。".to_string());
        }
        let expected = backup
            .files
            .iter()
            .map(|file| (file.relative_path.clone(), file.content_revision.clone()))
            .collect::<HashMap<_, _>>();
        for path in observed {
            let relative = path
                .strip_prefix(&folder)
                .map_err(|_| "永久清除恢复文件越出 bin 目录。".to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let revision = expected
                .get(&relative)
                .ok_or_else(|| "永久清除 bin 恢复结果包含意外文件。".to_string())?;
            if stable_bin_rollout_revision(&path)? != *revision {
                return Err("永久清除 bin 恢复结果内容不一致。".to_string());
            }
        }
    }
    Ok(())
}

fn restore_purge_bin_backups(trash_root: &Path, point: &PurgeRestorePoint) -> Result<(), String> {
    fs::create_dir_all(trash_root)
        .map_err(|error| format!("无法创建会话回收站目录：{error}"))?;
    ensure_purge_directory(trash_root, "会话回收站")?;
    let file = File::open(point.directory.join(PURGE_RESTORE_PAYLOAD))
        .map_err(|error| format!("无法打开永久清除恢复 payload：{error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("永久清除恢复 payload 已损坏：{error}"))?;
    validate_purge_archive_layout(&mut archive, &point.manifest)?;
    for backup in &point.manifest.bin_backups {
        let relative_folder = safe_relative_path(&backup.original_relative_folder)
            .ok_or_else(|| "永久清除恢复点包含不安全的 bin 路径。".to_string())?;
        let folder = trash_root.join(relative_folder);
        for backup_file in &backup.files {
            let relative = safe_relative_path(&backup_file.relative_path)
                .ok_or_else(|| "永久清除恢复点包含不安全的文件路径。".to_string())?;
            let target = folder.join(relative);
            ensure_purge_restore_parent_safe(trash_root, &target)?;
            if fs::symlink_metadata(&target).is_ok() {
                if stable_bin_rollout_revision(&target)? != backup_file.content_revision {
                    return Err("永久清除恢复目标已存在且内容不一致。".to_string());
                }
                continue;
            }
            let parent = target
                .parent()
                .ok_or_else(|| "永久清除恢复目标缺少父目录。".to_string())?;
            fs::create_dir_all(parent)
                .map_err(|error| format!("无法创建永久清除恢复目录：{error}"))?;
            ensure_purge_restore_parent_safe(trash_root, &target)?;
            let archive_name = format!("{}/{}", backup.archive_prefix, backup_file.relative_path);
            let mut source = archive
                .by_name(&archive_name)
                .map_err(|_| "永久清除恢复 payload 缺少 bin 文件。".to_string())?;
            if expected_revision_size(&backup_file.content_revision) != Some(source.size()) {
                return Err("永久清除恢复 payload 的 bin 文件大小不一致。".to_string());
            }
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|error| format!("无法创建永久清除恢复文件：{error}"))?;
            let copy_result = (|| -> Result<(), String> {
                std::io::copy(&mut source, &mut output)
                    .map_err(|error| format!("无法提取永久清除恢复文件：{error}"))?;
                output
                    .sync_all()
                    .map_err(|error| format!("无法同步永久清除恢复文件：{error}"))?;
                if stable_bin_rollout_revision(&target)? != backup_file.content_revision {
                    return Err("永久清除恢复文件内容校验失败。".to_string());
                }
                Ok(())
            })();
            if let Err(error) = copy_result {
                drop(output);
                let _ = fs::remove_file(&target);
                return Err(error);
            }
        }
    }
    verify_purge_bin_backups(trash_root, &point.manifest.bin_backups)
}

fn restore_purge_ownership(
    paths: &crate::storage::Paths,
    ownership: &[PurgeOwnershipSnapshot],
) -> Result<(), String> {
    let mut state = crate::storage::read_state(paths);
    for snapshot in ownership {
        match &snapshot.account_id {
            Some(account_id) => {
                state
                    .conversation_account_ids
                    .insert(snapshot.session_id.clone(), account_id.clone());
            }
            None => {
                state.conversation_account_ids.remove(&snapshot.session_id);
            }
        }
        if snapshot.observed {
            state
                .observed_conversation_ids
                .insert(snapshot.session_id.clone());
        } else {
            state.observed_conversation_ids.remove(&snapshot.session_id);
        }
    }
    crate::storage::write_state(paths, &state)
}

fn purge_ownership_matches(
    paths: &crate::storage::Paths,
    ownership: &[PurgeOwnershipSnapshot],
) -> bool {
    let state = crate::storage::read_state(paths);
    ownership.iter().all(|snapshot| {
        state
            .conversation_account_ids
            .get(&snapshot.session_id)
            .cloned()
            == snapshot.account_id
            && state
                .observed_conversation_ids
                .contains(&snapshot.session_id)
                == snapshot.observed
    })
}

fn cleanup_purge_restore_point(point: &mut PurgeRestorePoint) -> Result<bool, String> {
    if point.manifest.state == PurgeRestoreState::Prepared {
        point.manifest.state = PurgeRestoreState::RolledBack;
        write_purge_restore_manifest(&point.directory, &point.manifest)?;
    }
    let payload = point.directory.join(PURGE_RESTORE_PAYLOAD);
    if payload.exists() {
        fs::remove_file(&payload)
            .map_err(|error| format!("无法删除永久清除恢复 payload：{error}"))?;
        let _ = sync_purge_directory(&point.directory);
    }
    match fs::remove_dir_all(&point.directory) {
        Ok(()) => Ok(true),
        Err(_) => Ok(false),
    }
}

fn rollback_purge_restore_point(
    paths: &crate::storage::Paths,
    trash_root: &Path,
    point: &mut PurgeRestorePoint,
) -> Result<(), String> {
    let payload = load_purge_restore_payload(point)?;
    restore_purge_bin_backups(trash_root, point)?;
    for session in &payload.sessions {
        purge_thread_state(&paths.codex_home, &session.session_id)?;
        restore_purge_related_state(&paths.codex_home, &session.sqlite, &session.session_id)?;
    }
    restore_purge_ownership(paths, &payload.ownership)?;
    verify_purge_bin_backups(trash_root, &point.manifest.bin_backups)?;
    for session in &payload.sessions {
        let observed = snapshot_purge_related_state(&paths.codex_home, &session.session_id)?;
        if canonical_sqlite_snapshot_bytes(&observed)?
            != canonical_sqlite_snapshot_bytes(&session.sqlite)?
        {
            return Err("永久清除数据库状态未能完整恢复。".to_string());
        }
    }
    if !purge_ownership_matches(paths, &payload.ownership) {
        return Err("永久清除会话归属状态未能完整恢复。".to_string());
    }
    let _ = cleanup_purge_restore_point(point)?;
    Ok(())
}

fn verify_thread_purge_postcondition(
    paths: &crate::storage::Paths,
    trash_root: &Path,
    manifest: &PurgeRestoreManifest,
) -> Result<(), String> {
    let index = index_values(&paths.codex_home)?;
    let state = crate::storage::read_state(paths);
    for id in &manifest.session_ids {
        if index.contains_key(id)
            || state.conversation_account_ids.contains_key(id)
            || state.observed_conversation_ids.contains(id)
            || !snapshot_purge_related_state(&paths.codex_home, id)?.is_empty()
        {
            return Err("永久清除 postflight 仍发现会话状态。".to_string());
        }
    }
    for backup in &manifest.bin_backups {
        let relative = safe_relative_path(&backup.original_relative_folder)
            .ok_or_else(|| "永久清除恢复点包含不安全的 bin 路径。".to_string())?;
        if trash_root.join(relative).exists() {
            return Err("永久清除 postflight 仍发现回收站文件。".to_string());
        }
    }
    Ok(())
}

fn verify_thread_purge_precondition(
    paths: &crate::storage::Paths,
    trash_root: &Path,
    prepared: &PreparedThreadPurge,
) -> Result<(), String> {
    let manager_state = crate::storage::read_state(paths);
    let session_ids = prepared
        .items
        .iter()
        .map(|item| item.session_id.clone())
        .collect::<Vec<_>>();
    let latest = prepare_thread_purge(
        &paths.codex_home,
        trash_root,
        &manager_state,
        session_ids,
        prepared.empty_bin,
    )?;
    if latest.revision != prepared.revision {
        return Err("永久清除目标在确认期间发生变化。".to_string());
    }
    Ok(())
}

fn execute_thread_purge_transaction<Revalidate>(
    paths: &crate::storage::Paths,
    trash_root: &Path,
    restore_root: &Path,
    prepared: PreparedThreadPurge,
    mut revalidate: Revalidate,
) -> Result<ThreadPurgeReport, String>
where
    Revalidate: FnMut() -> Result<(), String>,
{
    revalidate()?;
    verify_thread_purge_precondition(paths, trash_root, &prepared)?;
    let mut point = create_purge_restore_point(restore_root, trash_root, &prepared)?;
    let mut mutation_started = false;
    let result = (|| -> Result<ThreadPurgeReport, String> {
        revalidate()?;
        verify_thread_purge_precondition(paths, trash_root, &prepared)?;
        revalidate()?;
        mutation_started = true;
        for item in &prepared.items {
            purge_thread_state(&paths.codex_home, &item.session_id)?;
            revalidate()?;
        }
        let mut manager_state = crate::storage::read_state(paths);
        for item in &prepared.items {
            manager_state
                .conversation_account_ids
                .remove(&item.session_id);
            manager_state
                .observed_conversation_ids
                .remove(&item.session_id);
        }
        crate::storage::write_state(paths, &manager_state)?;
        revalidate()?;
        for item in &prepared.items {
            for bin in &item.bins {
                fs::remove_dir_all(&bin.snapshot.folder)
                    .map_err(|error| format!("无法永久删除会话 {}：{error}", item.session_id))?;
            }
            revalidate()?;
        }
        verify_thread_purge_postcondition(paths, trash_root, &point.manifest)?;
        revalidate()?;
        point.manifest.state = PurgeRestoreState::Committed;
        write_purge_restore_manifest(&point.directory, &point.manifest)?;
        let payload_path = point.directory.join(PURGE_RESTORE_PAYLOAD);
        if let Err(error) = fs::remove_file(&payload_path) {
            point.manifest.state = PurgeRestoreState::Prepared;
            write_purge_restore_manifest(&point.directory, &point.manifest)?;
            return Err(format!("无法销毁永久清除临时恢复 payload：{error}"));
        }
        let _ = sync_purge_directory(&point.directory);
        let restore_point_removed = fs::remove_dir_all(&point.directory).is_ok();
        let released_bytes = prepared.items.iter().map(|item| item.released_bytes).sum();
        Ok(ThreadPurgeReport {
            requested_count: prepared.requested_count,
            affected_count: prepared.items.len(),
            released_bytes,
            outcomes: prepared
                .items
                .iter()
                .map(|item| ThreadPurgeOutcome {
                    session_id: item.session_id.clone(),
                    status: "purged".to_string(),
                    released_bytes: item.released_bytes,
                })
                .collect(),
            restore_point_removed,
            message: if restore_point_removed {
                format!("已永久清除 {} 条会话", prepared.items.len())
            } else {
                format!(
                    "已永久清除 {} 条会话；临时恢复点内容已销毁，残留元数据将自动清理",
                    prepared.items.len()
                )
            },
        })
    })();

    match result {
        Ok(report) => Ok(report),
        Err(error) if !mutation_started => {
            let cleanup = cleanup_purge_restore_point(&mut point);
            Err(format!(
                "{error} 未永久清除任何会话。{}",
                match cleanup {
                    Ok(true) => "临时恢复点已清理。",
                    Ok(false) => "临时恢复 payload 已销毁，残留元数据将自动清理。",
                    Err(_) => "临时恢复点已安全保留，稍后将自动处理。",
                }
            ))
        }
        Err(error) => {
            if point.manifest.state != PurgeRestoreState::Prepared {
                point.manifest.state = PurgeRestoreState::Prepared;
                let _ = write_purge_restore_manifest(&point.directory, &point.manifest);
            }
            match rollback_purge_restore_point(paths, trash_root, &mut point) {
                Ok(()) => Err(format!(
                    "{error} 回收站、数据库和会话归属状态已自动恢复，请重新预览后再试。"
                )),
                Err(_) => Err(concat!(
                    "永久清除未能完成，且无法确认自动恢复结果。",
                    "请暂时不要继续操作回收站，并保留 QuotaHorizon 应用数据以便恢复。"
                )
                .to_string()),
            }
        }
    }
}

fn collect_purge_restore_points(root: &Path) -> Result<Vec<PurgeRestorePoint>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    ensure_purge_directory(root, "永久清除恢复点")?;
    let mut points = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| format!("无法读取永久清除恢复点：{error}"))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if !file_type.is_dir() {
            return Err("永久清除恢复点目录包含意外文件。".to_string());
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(".capture-") {
            fs::remove_dir_all(entry.path())
                .map_err(|error| format!("无法清理未完成的恢复点 capture：{error}"))?;
            continue;
        }
        let directory = entry.path();
        let manifest = read_purge_restore_manifest(&directory)?;
        points.push(PurgeRestorePoint {
            directory,
            manifest,
        });
    }
    points.sort_by(|left, right| left.directory.cmp(&right.directory));
    Ok(points)
}

fn recover_incomplete_purge_transactions_with_lease<P: capacity_mutation::LegacyViewerProbe>(
    paths: &crate::storage::Paths,
    trash_root: &Path,
    restore_root: &Path,
    lease: &capacity_mutation::MutationLock,
    probe: &mut P,
) -> Result<(), String> {
    for mut point in collect_purge_restore_points(restore_root)? {
        lease
            .revalidate(probe)
            .map_err(|_| "永久清除恢复期间安全互斥失效。".to_string())?;
        match point.manifest.state {
            PurgeRestoreState::Prepared => {
                rollback_purge_restore_point(paths, trash_root, &mut point)?;
            }
            PurgeRestoreState::Committed => {
                if verify_thread_purge_postcondition(paths, trash_root, &point.manifest).is_err() {
                    if !point.directory.join(PURGE_RESTORE_PAYLOAD).is_file() {
                        return Err(
                            "已提交的永久清除事务状态异常且恢复 payload 已不存在。".to_string()
                        );
                    }
                    point.manifest.state = PurgeRestoreState::Prepared;
                    write_purge_restore_manifest(&point.directory, &point.manifest)?;
                    rollback_purge_restore_point(paths, trash_root, &mut point)?;
                } else {
                    let _ = cleanup_purge_restore_point(&mut point)?;
                }
            }
            PurgeRestoreState::RolledBack => {
                let _ = cleanup_purge_restore_point(&mut point)?;
            }
        }
        lease
            .revalidate(probe)
            .map_err(|_| "永久清除恢复完成后安全互斥失效。".to_string())?;
    }
    Ok(())
}

fn recover_incomplete_purge_transactions<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<(), String> {
    let restore_root = purge_restore_root(app)?;
    if !restore_root.exists()
        || fs::read_dir(&restore_root)
            .map_err(|error| format!("无法读取永久清除恢复点：{error}"))?
            .next()
            .is_none()
    {
        return Ok(());
    }
    let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "永久清除恢复安全互斥不可用。".to_string())?;
    let paths = resolve_paths(app)?;
    let trash_root = bin_root(app)?;
    recover_incomplete_purge_transactions_with_lease(
        &paths,
        &trash_root,
        &restore_root,
        &lease,
        &mut legacy_probe,
    )
}

pub(crate) fn prepare_codex_thread_purge_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    session_ids: Vec<String>,
    empty_bin: bool,
) -> Result<ThreadPurgePreview, String> {
    recover_incomplete_purge_transactions(&app)?;
    let paths = resolve_paths(&app)?;
    let trash_root = bin_root(&app)?;
    let manager_state = crate::storage::read_state(&paths);
    let prepared = prepare_thread_purge(
        &paths.codex_home,
        &trash_root,
        &manager_state,
        session_ids,
        empty_bin,
    )?;
    let ids = prepared
        .items
        .iter()
        .map(|item| item.session_id.clone())
        .collect::<Vec<_>>();
    let items = prepared
        .items
        .iter()
        .map(|item| ThreadPurgePreviewItem {
            session_id: item.session_id.clone(),
            title: item.title.clone(),
            cwd: item.cwd.clone(),
            size_bytes: item.size_bytes,
        })
        .collect::<Vec<_>>();
    let total_size_bytes = items.iter().map(|item| item.size_bytes).sum();
    let (confirm_token, expires_at) = issue_thread_purge_plan(ids, empty_bin, prepared.revision)?;
    Ok(ThreadPurgePreview {
        confirm_token,
        expires_at: expires_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        empty_bin,
        requested_count: prepared.requested_count,
        affected_count: items.len(),
        total_size_bytes,
        items,
        creates_temporary_restore_point: true,
        automatic_rollback: true,
        permanently_deletes: true,
    })
}

pub(crate) fn confirm_codex_thread_purge_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<ThreadPurgeReport, String> {
    let plan = consume_thread_purge_plan(&confirm_token)?;
    let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "永久清除安全互斥在执行前失效，未修改任何会话。".to_string())?;
    let paths = resolve_paths(&app)?;
    let trash_root = bin_root(&app)?;
    let restore_root = purge_restore_root(&app)?;
    recover_incomplete_purge_transactions_with_lease(
        &paths,
        &trash_root,
        &restore_root,
        &lease,
        &mut legacy_probe,
    )?;
    let manager_state = crate::storage::read_state(&paths);
    let prepared = prepare_thread_purge(
        &paths.codex_home,
        &trash_root,
        &manager_state,
        plan.session_ids,
        plan.empty_bin,
    )?;
    if prepared.revision != plan.expected_revision {
        return Err("永久清除目标在预览后发生变化，未删除任何内容。请重新预览。".to_string());
    }
    execute_thread_purge_transaction(&paths, &trash_root, &restore_root, prepared, || {
        lease
            .revalidate(&mut legacy_probe)
            .map_err(|_| "永久清除安全互斥失效。".to_string())
    })
}
