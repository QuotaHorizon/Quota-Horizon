const THREAD_ARCHIVE_PLAN_PREFIX: &str = "session-archive-plan:v1:";
const THREAD_ARCHIVE_PLAN_TTL_SECONDS: i64 = 120;
const MAX_PENDING_THREAD_ARCHIVE_PLANS: usize = 32;
const MAX_THREAD_ARCHIVE_TARGETS: usize = 512;
const MAX_ARCHIVE_STATE_DATABASES: usize = 4;
const ARCHIVE_TRANSACTION_FORMAT: &str = "quota-horizon-session-archive-v1";
const ARCHIVE_TRANSACTION_PREFIX: &str = "session-archive-tx-v1-";
const ARCHIVE_TRANSACTION_MANIFEST: &str = "manifest.json";
const MAX_ARCHIVE_TRANSACTION_MANIFEST_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
struct PendingThreadArchivePlan {
    session_ids: Vec<String>,
    expected_revision: String,
    expires_at: DateTime<Utc>,
}

#[derive(Default)]
struct PendingThreadArchivePlans {
    plans: HashMap<String, PendingThreadArchivePlan>,
}

impl PendingThreadArchivePlans {
    fn issue(
        &mut self,
        session_ids: Vec<String>,
        expected_revision: String,
        now: DateTime<Utc>,
    ) -> Result<(String, DateTime<Utc>), String> {
        self.plans.retain(|_, plan| plan.expires_at > now);
        if self.plans.len() >= MAX_PENDING_THREAD_ARCHIVE_PLANS {
            return Err("待确认的会话归档操作过多，请稍后重试。".to_string());
        }
        let token = format!(
            "{THREAD_ARCHIVE_PLAN_PREFIX}{}",
            Uuid::new_v4().hyphenated()
        );
        let expires_at = now + chrono::Duration::seconds(THREAD_ARCHIVE_PLAN_TTL_SECONDS);
        self.plans.insert(
            token.clone(),
            PendingThreadArchivePlan {
                session_ids,
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
    ) -> Result<PendingThreadArchivePlan, String> {
        validate_thread_archive_plan_token(token)?;
        let plan = self
            .plans
            .remove(token)
            .ok_or_else(|| "归档确认已失效，请重新预览后再执行。".to_string())?;
        if plan.expires_at <= now {
            return Err("归档确认已过期，请重新预览后再执行。".to_string());
        }
        Ok(plan)
    }
}

fn validate_thread_archive_plan_token(token: &str) -> Result<(), String> {
    let value = token
        .strip_prefix(THREAD_ARCHIVE_PLAN_PREFIX)
        .ok_or_else(|| "归档确认令牌格式无效。".to_string())?;
    let parsed = Uuid::parse_str(value).map_err(|_| "归档确认令牌格式无效。".to_string())?;
    if parsed.hyphenated().to_string() != value {
        return Err("归档确认令牌格式无效。".to_string());
    }
    Ok(())
}

fn pending_thread_archive_plans() -> &'static std::sync::Mutex<PendingThreadArchivePlans> {
    static PLANS: std::sync::OnceLock<std::sync::Mutex<PendingThreadArchivePlans>> =
        std::sync::OnceLock::new();
    PLANS.get_or_init(|| std::sync::Mutex::new(PendingThreadArchivePlans::default()))
}

fn issue_thread_archive_plan(
    session_ids: Vec<String>,
    expected_revision: String,
) -> Result<(String, DateTime<Utc>), String> {
    pending_thread_archive_plans()
        .lock()
        .map_err(|_| "会话归档预览状态不可用。".to_string())?
        .issue(session_ids, expected_revision, Utc::now())
}

fn consume_thread_archive_plan(token: &str) -> Result<PendingThreadArchivePlan, String> {
    pending_thread_archive_plans()
        .lock()
        .map_err(|_| "会话归档预览状态不可用。".to_string())?
        .consume(token, Utc::now())
}

#[derive(Debug, Clone)]
struct PreparedThreadArchiveFile {
    source_relative: PathBuf,
    target_relative: PathBuf,
    content_revision: String,
}

#[derive(Debug, Clone)]
struct PreparedThreadArchiveItem {
    session_id: String,
    title: String,
    cwd: String,
    size_bytes: u64,
    index_present: bool,
    index_entry: Value,
    visibilities: Vec<PreparedThreadArchiveVisibility>,
    primary_target: PathBuf,
    files: Vec<PreparedThreadArchiveFile>,
}

#[derive(Debug, Clone)]
struct PreparedThreadArchiveVisibility {
    state_database: PathBuf,
    original: Option<StateVisibilitySnapshot>,
}

#[derive(Debug, Clone)]
struct PreparedThreadArchive {
    requested_count: usize,
    state_databases: Vec<PathBuf>,
    items: Vec<PreparedThreadArchiveItem>,
    revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ArchiveTransactionState {
    Prepared,
    Committed,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveTransactionFile {
    source_relative: String,
    target_relative: String,
    content_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveTransactionItem {
    session_id: String,
    index_present: bool,
    index_entry: Value,
    visibilities: Vec<ArchiveTransactionVisibility>,
    primary_target_relative: String,
    files: Vec<ArchiveTransactionFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveTransactionVisibility {
    state_database: String,
    original: Option<StateVisibilitySnapshot>,
    archived: Option<StateVisibilitySnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveTransactionManifest {
    format: String,
    id: String,
    created_at: String,
    state: ArchiveTransactionState,
    state_databases: Vec<String>,
    items: Vec<ArchiveTransactionItem>,
}

#[derive(Debug)]
struct ArchiveTransaction {
    directory: PathBuf,
    manifest: ArchiveTransactionManifest,
}

fn stable_archive_file_revision(path: &Path) -> Result<String, String> {
    let before =
        fs::symlink_metadata(path).map_err(|_| "待归档会话文件已消失，请刷新列表。".to_string())?;
    if before.file_type().is_symlink() || !before.file_type().is_file() {
        return Err("待归档会话文件类型不安全，已拒绝操作。".to_string());
    }
    let content_sha256 =
        sha256(path).map_err(|_| "无法读取完整会话文件，已拒绝归档。".to_string())?;
    let after =
        fs::symlink_metadata(path).map_err(|_| "待归档会话文件已消失，请刷新列表。".to_string())?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err("待归档会话仍在写入，请稍后重新预览。".to_string());
    }
    Ok(format!("{}\u{0}{}", after.len(), content_sha256))
}

fn valid_archive_content_revision(value: &str) -> bool {
    let Some((size, sha256)) = value.split_once('\0') else {
        return false;
    };
    size.parse::<u64>().is_ok()
        && sha256.len() == 64
        && sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn archive_target_relative(source_relative: &Path) -> Result<PathBuf, String> {
    let mut components = source_relative.components();
    if !matches!(
        components.next(),
        Some(std::path::Component::Normal(value)) if value == "sessions"
    ) {
        return Err("只有 active 会话可以归档。".to_string());
    }
    let mut target = PathBuf::from("archived_sessions");
    for component in components {
        let std::path::Component::Normal(value) = component else {
            return Err("会话归档路径不安全。".to_string());
        };
        target.push(value);
    }
    if target == Path::new("archived_sessions") {
        return Err("会话归档路径不完整。".to_string());
    }
    Ok(target)
}

fn archive_source_for_target(target_relative: &Path) -> Result<PathBuf, String> {
    let mut components = target_relative.components();
    if !matches!(
        components.next(),
        Some(std::path::Component::Normal(value)) if value == "archived_sessions"
    ) {
        return Err("会话归档目标路径无效。".to_string());
    }
    let mut source = PathBuf::from("sessions");
    for component in components {
        let std::path::Component::Normal(value) = component else {
            return Err("会话归档目标路径不安全。".to_string());
        };
        source.push(value);
    }
    Ok(source)
}

fn ensure_archive_path_absent(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(format!(
            "归档目标已存在，为避免覆盖已拒绝操作：{}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法验证归档目标 {}：{error}", path.display())),
    }
}

fn ensure_archive_managed_root(codex_home: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(codex_home)
        .map_err(|error| format!("无法验证 Codex 目录：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err("Codex 目录类型不安全。".to_string());
    }
    let sessions = codex_home.join("sessions");
    let metadata = fs::symlink_metadata(&sessions)
        .map_err(|error| format!("无法验证 active 会话目录：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err("active 会话目录类型不安全。".to_string());
    }
    let archived = codex_home.join("archived_sessions");
    match fs::symlink_metadata(&archived) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() => {
            return Err("归档会话目录类型不安全。".to_string())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("无法验证归档会话目录：{error}")),
    }
    Ok(())
}

fn archive_state_database_version(name: &str) -> Option<u64> {
    name.strip_prefix("state_")?
        .strip_suffix(".sqlite")?
        .parse::<u64>()
        .ok()
}

fn newest_archive_state_database(directory: &Path) -> Result<Option<PathBuf>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "无法读取 Codex state DB 目录 {}：{error}",
                directory.display()
            ))
        }
    };
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("无法读取 Codex state DB 目录项：{error}"))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(version) = archive_state_database_version(name) else {
            continue;
        };
        candidates.push((version, entry.path()));
    }
    Ok(candidates
        .into_iter()
        .max_by(|left, right| left.0.cmp(&right.0).then_with(|| right.1.cmp(&left.1)))
        .map(|(_, path)| path))
}

fn validate_archive_state_database_path(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法验证 Codex state DB {}：{error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(format!("Codex state DB 路径类型不安全：{}", path.display()));
    }
    Ok(())
}

fn archive_state_databases(codex_home: &Path) -> Result<Vec<PathBuf>, String> {
    let sqlite_directory = codex_home.join("sqlite");
    match fs::symlink_metadata(&sqlite_directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() => {
            return Err("Codex sqlite 目录类型不安全。".to_string())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("无法验证 Codex sqlite 目录：{error}")),
    }

    let mut databases = Vec::new();
    if let Some(path) = newest_archive_state_database(&sqlite_directory)? {
        databases.push(path);
    } else {
        let state_db = sqlite_directory.join("state.db");
        match fs::symlink_metadata(&state_db) {
            Ok(_) => databases.push(state_db),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("无法验证 Codex sqlite/state.db：{error}")),
        }
    }
    if let Some(path) = newest_archive_state_database(codex_home)? {
        databases.push(path);
    }
    databases.dedup();
    if databases.len() > MAX_ARCHIVE_STATE_DATABASES {
        return Err("检测到过多 Codex state DB，已拒绝自动归档。".to_string());
    }
    for path in &databases {
        validate_archive_state_database_path(path)?;
    }
    Ok(databases)
}

fn prepare_thread_archive(
    codex_home: &Path,
    session_ids: Vec<String>,
) -> Result<PreparedThreadArchive, String> {
    let requested = normalized_ids(session_ids);
    if requested.is_empty() {
        return Err("请至少选择一条待归档会话。".to_string());
    }
    if requested.len() > MAX_THREAD_ARCHIVE_TARGETS {
        return Err(format!(
            "单次最多可归档 {MAX_THREAD_ARCHIVE_TARGETS} 条会话。"
        ));
    }
    ensure_archive_managed_root(codex_home)?;
    let snapshots = gather_snapshots(codex_home)?;
    let index = index_values(codex_home)?;
    let state_databases = archive_state_databases(codex_home)?;
    let mut ids = requested.iter().cloned().collect::<Vec<_>>();
    ids.sort();
    let mut target_paths = HashSet::new();
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        let matches = snapshots
            .iter()
            .filter(|snapshot| snapshot.session_id == id)
            .collect::<Vec<_>>();
        let active = matches
            .iter()
            .filter(|snapshot| rollout_status(&snapshot.relative_path) == Some("active"))
            .copied()
            .collect::<Vec<_>>();
        let archived = matches
            .iter()
            .any(|snapshot| rollout_status(&snapshot.relative_path) == Some("archived"));
        if archived {
            return Err(format!("会话 {id} 已归档，未修改任何会话。"));
        }
        if active.len() != 1 || matches.len() != 1 {
            return Err(format!(
                "无法唯一定位 active 会话 {id}，请先运行可见性修复。"
            ));
        }
        let snapshot = active[0];
        let expected = logical_rollout_path(&snapshot.path)
            .unwrap_or_else(|| snapshot.path.clone())
            .to_string_lossy()
            .to_string();
        let visibilities = state_databases
            .iter()
            .map(|state_database| {
                let original = state_visibility_snapshot(Some(state_database), &id)?;
                if original.as_ref().is_some_and(|visibility| {
                    visibility.archived != 0 || visibility.rollout_path != expected
                }) {
                    return Err(format!(
                        "会话 {id} 的官方 active 状态与文件不一致，请先修复后再归档。"
                    ));
                }
                Ok(PreparedThreadArchiveVisibility {
                    state_database: state_database.clone(),
                    original,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut files = Vec::with_capacity(snapshot.physical_paths.len());
        for source in &snapshot.physical_paths {
            ensure_restore_target_parent_is_safe(codex_home, source)?;
            let source_relative = source
                .strip_prefix(codex_home)
                .map_err(|_| "待归档会话越出受信任目录。".to_string())?
                .to_path_buf();
            let target_relative = archive_target_relative(&source_relative)?;
            let target = codex_home.join(&target_relative);
            ensure_restore_target_parent_is_safe(codex_home, &target)?;
            ensure_archive_path_absent(&target)?;
            if !target_paths.insert(target.clone()) {
                return Err("多个会话指向同一归档目标，已拒绝操作。".to_string());
            }
            files.push(PreparedThreadArchiveFile {
                source_relative,
                target_relative,
                content_revision: stable_archive_file_revision(source)?,
            });
        }
        files.sort_by(|left, right| left.source_relative.cmp(&right.source_relative));
        let primary_source = snapshot
            .path
            .strip_prefix(codex_home)
            .map_err(|_| "待归档主文件越出受信任目录。".to_string())?;
        let primary_target = archive_target_relative(primary_source)?;
        if !files
            .iter()
            .any(|file| file.target_relative == primary_target)
        {
            return Err("待归档主文件与物理文件集合不一致。".to_string());
        }
        items.push(PreparedThreadArchiveItem {
            session_id: id.clone(),
            title: snapshot.title.clone(),
            cwd: snapshot.cwd.clone(),
            size_bytes: snapshot.size_bytes,
            index_present: index.contains_key(&id),
            index_entry: snapshot.index_value.clone(),
            visibilities,
            primary_target,
            files,
        });
    }
    let mut prepared = PreparedThreadArchive {
        requested_count: requested.len(),
        state_databases,
        items,
        revision: String::new(),
    };
    prepared.revision = thread_archive_revision(&prepared)?;
    Ok(prepared)
}

fn thread_archive_revision(prepared: &PreparedThreadArchive) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest_thread_trash_field(&mut digest, b"session-archive-revision:v1");
    for state_database in &prepared.state_databases {
        digest_thread_trash_field(&mut digest, state_database.to_string_lossy().as_bytes());
    }
    for item in &prepared.items {
        digest_thread_trash_field(&mut digest, item.session_id.as_bytes());
        digest_thread_trash_field(&mut digest, &[u8::from(item.index_present)]);
        digest_thread_trash_field(
            &mut digest,
            &serde_json::to_vec(&item.index_entry).map_err(|_| "无法校验会话索引。".to_string())?,
        );
        digest_thread_trash_field(
            &mut digest,
            &serde_json::to_vec(
                &item
                    .visibilities
                    .iter()
                    .map(|visibility| {
                        (
                            visibility.state_database.to_string_lossy().to_string(),
                            &visibility.original,
                        )
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(|_| "无法校验会话可见性。".to_string())?,
        );
        digest_thread_trash_field(
            &mut digest,
            item.primary_target.to_string_lossy().as_bytes(),
        );
        for file in &item.files {
            digest_thread_trash_field(
                &mut digest,
                file.source_relative.to_string_lossy().as_bytes(),
            );
            digest_thread_trash_field(
                &mut digest,
                file.target_relative.to_string_lossy().as_bytes(),
            );
            digest_thread_trash_field(&mut digest, file.content_revision.as_bytes());
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn archive_transaction_root<R: Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法定位应用数据目录：{error}"))?
        .join("codex-thread-archive-transactions-v1"))
}

fn harden_archive_transaction_permissions(path: &Path, directory: bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if directory { 0o700 } else { 0o600 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|error| format!("无法保护会话归档事务权限：{error}"))?;
    }
    #[cfg(not(unix))]
    let _ = (path, directory);
    Ok(())
}

fn ensure_archive_transaction_directory(path: &Path) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("无法验证会话归档事务目录：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err("会话归档事务目录类型不安全。".to_string());
    }
    Ok(())
}

fn sync_archive_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("无法同步会话归档事务目录：{error}"))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn write_archive_transaction_manifest(
    directory: &Path,
    manifest: &ArchiveTransactionManifest,
) -> Result<(), String> {
    let text = serde_json::to_string_pretty(manifest)
        .map_err(|_| "无法序列化会话归档事务。".to_string())?;
    if text.len() > MAX_ARCHIVE_TRANSACTION_MANIFEST_BYTES {
        return Err("会话归档事务清单过大，已拒绝操作。".to_string());
    }
    let path = directory.join(ARCHIVE_TRANSACTION_MANIFEST);
    write_text_atomic(&path, &format!("{text}\n"))?;
    harden_archive_transaction_permissions(&path, false)?;
    sync_archive_directory(directory)
}

fn normalized_relative_text(path: &Path) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or_else(|| "会话归档路径不是有效文本。".to_string())?
        .replace('\\', "/");
    if safe_relative_path(&text).as_deref() != Some(path) {
        return Err("会话归档路径不安全。".to_string());
    }
    Ok(text)
}

fn valid_archive_state_database(value: &str) -> bool {
    if value == "sqlite/state.db" {
        return true;
    }
    let file_name = value.strip_prefix("sqlite/").unwrap_or(value);
    if file_name.contains('/') {
        return false;
    }
    let Some(version) = file_name
        .strip_prefix("state_")
        .and_then(|value| value.strip_suffix(".sqlite"))
    else {
        return false;
    };
    !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_archive_transaction_manifest(
    codex_home: &Path,
    directory: &Path,
    manifest: &ArchiveTransactionManifest,
) -> Result<(), String> {
    let Some(id_suffix) = manifest.id.strip_prefix(ARCHIVE_TRANSACTION_PREFIX) else {
        return Err("会话归档事务 manifest 身份无效。".to_string());
    };
    let parsed =
        Uuid::parse_str(id_suffix).map_err(|_| "会话归档事务 manifest 身份无效。".to_string())?;
    if manifest.format != ARCHIVE_TRANSACTION_FORMAT
        || directory.file_name().and_then(|value| value.to_str()) != Some(manifest.id.as_str())
        || parsed.hyphenated().to_string() != id_suffix
        || DateTime::parse_from_rfc3339(&manifest.created_at).is_err()
        || manifest.items.is_empty()
        || manifest.items.len() > MAX_THREAD_ARCHIVE_TARGETS
    {
        return Err("会话归档事务 manifest 身份无效。".to_string());
    }
    if manifest.state_databases.len() > MAX_ARCHIVE_STATE_DATABASES {
        return Err("会话归档事务包含过多 state DB。".to_string());
    }
    let mut state_databases = HashSet::new();
    for relative in &manifest.state_databases {
        if !valid_archive_state_database(relative)
            || safe_relative_path(relative).is_none()
            || !state_databases.insert(relative.as_str())
        {
            return Err("会话归档事务 state DB 标识无效。".to_string());
        }
        let path = codex_home.join(relative);
        validate_archive_state_database_path(&path)
            .map_err(|error| format!("会话归档事务 state DB 不可用：{error}"))?;
    }
    let mut ids = HashSet::new();
    let mut sources = HashSet::new();
    let mut targets = HashSet::new();
    for item in &manifest.items {
        if item.session_id.is_empty()
            || !ids.insert(item.session_id.as_str())
            || item.files.is_empty()
            || item.index_entry.get("id").and_then(Value::as_str) != Some(item.session_id.as_str())
        {
            return Err("会话归档事务目标集合无效。".to_string());
        }
        let primary_target = safe_relative_path(&item.primary_target_relative)
            .ok_or_else(|| "会话归档事务主文件路径无效。".to_string())?;
        if archive_source_for_target(&primary_target).is_err() {
            return Err("会话归档事务主文件路径无效。".to_string());
        }
        let mut primary_found = false;
        for file in &item.files {
            let source = safe_relative_path(&file.source_relative)
                .ok_or_else(|| "会话归档事务包含不安全的源路径。".to_string())?;
            let target = safe_relative_path(&file.target_relative)
                .ok_or_else(|| "会话归档事务包含不安全的目标路径。".to_string())?;
            if normalized_relative_text(&source)? != file.source_relative
                || normalized_relative_text(&target)? != file.target_relative
                || archive_target_relative(&source)? != target
                || archive_source_for_target(&target)? != source
                || !valid_archive_content_revision(&file.content_revision)
                || !sources.insert(file.source_relative.as_str())
                || !targets.insert(file.target_relative.as_str())
            {
                return Err("会话归档事务文件清单无效。".to_string());
            }
            primary_found |= target == primary_target;
        }
        if !primary_found {
            return Err("会话归档事务未覆盖主文件。".to_string());
        }
        if item.visibilities.len() != manifest.state_databases.len() {
            return Err("会话归档事务可见性快照不完整。".to_string());
        }
        let mut item_databases = HashSet::new();
        for visibility in &item.visibilities {
            if !state_databases.contains(visibility.state_database.as_str())
                || !item_databases.insert(visibility.state_database.as_str())
            {
                return Err("会话归档事务可见性 state DB 标识无效。".to_string());
            }
            match (&visibility.original, &visibility.archived) {
                (None, None) => {}
                (Some(original), Some(archived)) => {
                    let primary_source = archive_source_for_target(&primary_target)?;
                    let expected_source = logical_rollout_path(&codex_home.join(&primary_source))
                        .unwrap_or_else(|| codex_home.join(&primary_source))
                        .to_string_lossy()
                        .to_string();
                    let expected_target = logical_rollout_path(&codex_home.join(&primary_target))
                        .unwrap_or_else(|| codex_home.join(&primary_target))
                        .to_string_lossy()
                        .to_string();
                    if original.archived != 0
                        || original.rollout_path != expected_source
                        || archived.archived != 1
                        || archived.archived_at.is_none()
                        || archived.rollout_path != expected_target
                        || archived.preview != original.preview
                    {
                        return Err("会话归档事务可见性快照无效。".to_string());
                    }
                }
                _ => return Err("会话归档事务可见性快照不完整。".to_string()),
            }
        }
    }
    Ok(())
}

fn read_archive_transaction(
    codex_home: &Path,
    directory: &Path,
) -> Result<ArchiveTransaction, String> {
    ensure_archive_transaction_directory(directory)?;
    let path = directory.join(ARCHIVE_TRANSACTION_MANIFEST);
    let before =
        fs::symlink_metadata(&path).map_err(|error| format!("无法读取会话归档事务：{error}"))?;
    if before.file_type().is_symlink() || !before.file_type().is_file() {
        return Err("会话归档事务 manifest 文件类型不安全。".to_string());
    }
    if before.len() as usize > MAX_ARCHIVE_TRANSACTION_MANIFEST_BYTES {
        return Err("会话归档事务 manifest 过大。".to_string());
    }
    let bytes = fs::read(&path).map_err(|error| format!("无法读取会话归档事务：{error}"))?;
    let after =
        fs::symlink_metadata(&path).map_err(|error| format!("无法复核会话归档事务：{error}"))?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err("会话归档事务 manifest 正在变化。".to_string());
    }
    let manifest: ArchiveTransactionManifest =
        serde_json::from_slice(&bytes).map_err(|_| "会话归档事务 manifest 已损坏。".to_string())?;
    validate_archive_transaction_manifest(codex_home, directory, &manifest)?;
    Ok(ArchiveTransaction {
        directory: directory.to_path_buf(),
        manifest,
    })
}

fn create_archive_transaction(
    transaction_root: &Path,
    codex_home: &Path,
    prepared: &PreparedThreadArchive,
) -> Result<ArchiveTransaction, String> {
    fs::create_dir_all(transaction_root)
        .map_err(|error| format!("无法创建会话归档事务目录：{error}"))?;
    ensure_archive_transaction_directory(transaction_root)?;
    harden_archive_transaction_permissions(transaction_root, true)?;
    let id = format!(
        "{ARCHIVE_TRANSACTION_PREFIX}{}",
        Uuid::new_v4().hyphenated()
    );
    let staging = transaction_root.join(format!(".capture-{}", Uuid::new_v4().hyphenated()));
    fs::create_dir(&staging).map_err(|error| format!("无法创建归档事务 staging：{error}"))?;
    harden_archive_transaction_permissions(&staging, true)?;
    let result = (|| -> Result<ArchiveTransaction, String> {
        let archived_at = Utc::now().timestamp();
        let items = prepared
            .items
            .iter()
            .map(|item| {
                let target_path = codex_home.join(&item.primary_target);
                let logical_target = logical_rollout_path(&target_path).unwrap_or(target_path);
                let visibilities =
                    item.visibilities
                        .iter()
                        .map(|visibility| {
                            let relative = visibility
                                .state_database
                                .strip_prefix(codex_home)
                                .map_err(|_| "Codex state DB 越出受信任目录。".to_string())?;
                            let state_database = normalized_relative_text(relative)?;
                            let archived = visibility.original.as_ref().map(|original| {
                                StateVisibilitySnapshot {
                                    rollout_path: logical_target.to_string_lossy().to_string(),
                                    archived: 1,
                                    archived_at: Some(archived_at),
                                    preview: original.preview.clone(),
                                }
                            });
                            Ok(ArchiveTransactionVisibility {
                                state_database,
                                original: visibility.original.clone(),
                                archived,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                Ok(ArchiveTransactionItem {
                    session_id: item.session_id.clone(),
                    index_present: item.index_present,
                    index_entry: item.index_entry.clone(),
                    visibilities,
                    primary_target_relative: normalized_relative_text(&item.primary_target)?,
                    files: item
                        .files
                        .iter()
                        .map(|file| {
                            Ok(ArchiveTransactionFile {
                                source_relative: normalized_relative_text(&file.source_relative)?,
                                target_relative: normalized_relative_text(&file.target_relative)?,
                                content_revision: file.content_revision.clone(),
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let state_databases = prepared
            .state_databases
            .iter()
            .map(|path| {
                let relative = path
                    .strip_prefix(codex_home)
                    .map_err(|_| "Codex state DB 越出受信任目录。".to_string())?;
                normalized_relative_text(relative)
            })
            .collect::<Result<Vec<_>, String>>()?;
        let manifest = ArchiveTransactionManifest {
            format: ARCHIVE_TRANSACTION_FORMAT.to_string(),
            id: id.clone(),
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            state: ArchiveTransactionState::Prepared,
            state_databases,
            items,
        };
        write_archive_transaction_manifest(&staging, &manifest)?;
        let destination = transaction_root.join(&id);
        fs::rename(&staging, &destination)
            .map_err(|error| format!("无法提交会话归档事务清单：{error}"))?;
        sync_archive_directory(transaction_root)?;
        read_archive_transaction(codex_home, &destination)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn archive_state_db_paths(
    codex_home: &Path,
    manifest: &ArchiveTransactionManifest,
) -> Vec<PathBuf> {
    manifest
        .state_databases
        .iter()
        .map(|relative| codex_home.join(relative))
        .collect()
}

fn archive_visibility_matches(
    codex_home: &Path,
    item: &ArchiveTransactionItem,
    archived: bool,
) -> Result<bool, String> {
    for visibility in &item.visibilities {
        let state_database = codex_home.join(&visibility.state_database);
        validate_archive_state_database_path(&state_database)?;
        let expected = if archived {
            &visibility.archived
        } else {
            &visibility.original
        };
        let observed = state_visibility_snapshot(Some(&state_database), &item.session_id)?;
        if observed.as_ref() != expected.as_ref() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn archive_index_matches(codex_home: &Path, item: &ArchiveTransactionItem) -> Result<bool, String> {
    let index = index_values(codex_home)?;
    Ok(match index.get(&item.session_id) {
        Some(value) => item.index_present && value == &item.index_entry,
        None => !item.index_present,
    })
}

fn verify_archive_transaction_precondition(
    codex_home: &Path,
    manifest: &ArchiveTransactionManifest,
) -> Result<(), String> {
    let state_databases = archive_state_db_paths(codex_home, manifest);
    if archive_state_databases(codex_home)? != state_databases {
        return Err("Codex state DB 在归档确认期间发生切换。".to_string());
    }
    for item in &manifest.items {
        if !archive_index_matches(codex_home, item)?
            || !archive_visibility_matches(codex_home, item, false)?
        {
            return Err("会话索引或可见性在归档确认期间发生变化。".to_string());
        }
        for file in &item.files {
            let source = codex_home.join(&file.source_relative);
            let target = codex_home.join(&file.target_relative);
            ensure_restore_target_parent_is_safe(codex_home, &source)?;
            ensure_restore_target_parent_is_safe(codex_home, &target)?;
            ensure_archive_path_absent(&target)?;
            if stable_archive_file_revision(&source)? != file.content_revision {
                return Err("会话内容在归档确认期间发生变化。".to_string());
            }
        }
    }
    Ok(())
}

fn verify_archive_transaction_postcondition(
    codex_home: &Path,
    manifest: &ArchiveTransactionManifest,
) -> Result<(), String> {
    for item in &manifest.items {
        if !archive_index_matches(codex_home, item)?
            || !archive_visibility_matches(codex_home, item, true)?
        {
            return Err("会话归档后的索引或可见性校验失败。".to_string());
        }
        for file in &item.files {
            let source = codex_home.join(&file.source_relative);
            let target = codex_home.join(&file.target_relative);
            if source.exists()
                || stable_archive_file_revision(&target).ok().as_deref()
                    != Some(file.content_revision.as_str())
            {
                return Err("会话归档后的文件校验失败。".to_string());
            }
        }
    }
    Ok(())
}

fn verify_archive_transaction_rollback(
    codex_home: &Path,
    manifest: &ArchiveTransactionManifest,
    restored_visibility: &HashSet<(String, String)>,
) -> Result<(), String> {
    for item in &manifest.items {
        for visibility in &item.visibilities {
            let key = (visibility.state_database.clone(), item.session_id.clone());
            if restored_visibility.contains(&key) && {
                let state_database = codex_home.join(&visibility.state_database);
                validate_archive_state_database_path(&state_database)?;
                state_visibility_snapshot(Some(&state_database), &item.session_id)?
                    != visibility.original
            } {
                return Err("会话归档回滚后的可见性校验失败。".to_string());
            }
        }
        for file in &item.files {
            let source = codex_home.join(&file.source_relative);
            let target = codex_home.join(&file.target_relative);
            let source_metadata = fs::symlink_metadata(&source).ok();
            if target.exists()
                || source_metadata
                    .as_ref()
                    .is_none_or(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
            {
                return Err("会话归档回滚后的文件校验失败。".to_string());
            }
        }
    }
    Ok(())
}

fn sync_archive_move_parents(source: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        sync_archive_directory(parent)?;
    }
    if let Some(parent) = source.parent() {
        sync_archive_directory(parent)?;
    }
    Ok(())
}

fn relocate_archive_file_without_replace(
    codex_home: &Path,
    source: &Path,
    target: &Path,
    expected_revision: &str,
) -> Result<(), String> {
    if stable_archive_file_revision(source)? != expected_revision {
        return Err("待归档会话文件在移动前发生变化。".to_string());
    }
    let parent = target
        .parent()
        .ok_or_else(|| "无法定位会话归档目录。".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建会话归档目录：{error}"))?;
    ensure_restore_target_parent_is_safe(codex_home, target)?;
    ensure_archive_path_absent(target)?;
    if fs::hard_link(source, target).is_ok() {
        if stable_archive_file_revision(target)? != expected_revision
            || stable_archive_file_revision(source)? != expected_revision
        {
            let _ = fs::remove_file(target);
            return Err("会话文件在归档移动期间发生变化。".to_string());
        }
        if let Err(error) = fs::remove_file(source) {
            let _ = fs::remove_file(target);
            return Err(format!("无法完成会话归档移动：{error}"));
        }
        return sync_archive_move_parents(source, target);
    }
    let mut input = File::open(source).map_err(|error| format!("无法读取会话文件：{error}"))?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| format!("无法创建无覆盖归档目标：{error}"))?;
    let result = (|| -> Result<(), String> {
        std::io::copy(&mut input, &mut output)
            .map_err(|error| format!("无法复制会话文件：{error}"))?;
        output
            .sync_all()
            .map_err(|error| format!("无法同步归档会话文件：{error}"))?;
        if stable_archive_file_revision(target)? != expected_revision
            || stable_archive_file_revision(source)? != expected_revision
        {
            return Err("会话文件在归档复制期间发生变化。".to_string());
        }
        fs::remove_file(source).map_err(|error| format!("无法移除归档源文件：{error}"))?;
        sync_archive_move_parents(source, target)
    })();
    if let Err(error) = result {
        drop(output);
        let _ = fs::remove_file(target);
        return Err(error);
    }
    Ok(())
}

fn rollback_archive_transaction(
    codex_home: &Path,
    transaction: &mut ArchiveTransaction,
) -> Result<(), String> {
    let mut restored_visibility = HashSet::new();
    for item in transaction.manifest.items.iter().rev() {
        let mut visibility_states = Vec::with_capacity(item.visibilities.len());
        for visibility in &item.visibilities {
            let state_database = codex_home.join(&visibility.state_database);
            validate_archive_state_database_path(&state_database)?;
            let current = state_visibility_snapshot(Some(&state_database), &item.session_id)?;
            visibility_states.push((visibility, state_database, current));
        }
        for file in item.files.iter().rev() {
            let source = codex_home.join(&file.source_relative);
            let target = codex_home.join(&file.target_relative);
            match (source.exists(), target.exists()) {
                (true, false) => {}
                (true, true) => {
                    let source_metadata = fs::symlink_metadata(&source)
                        .map_err(|_| "归档回滚无法验证 active 源文件。".to_string())?;
                    let target_metadata = fs::symlink_metadata(&target)
                        .map_err(|_| "归档回滚无法验证重复目标文件。".to_string())?;
                    if source_metadata.file_type().is_symlink()
                        || !source_metadata.file_type().is_file()
                        || target_metadata.file_type().is_symlink()
                        || !target_metadata.file_type().is_file()
                    {
                        return Err("归档回滚发现不安全的重复文件。".to_string());
                    }
                    if stable_archive_file_revision(&source)? != file.content_revision
                        || stable_archive_file_revision(&target)? != file.content_revision
                    {
                        return Err(
                            "归档回滚发现同名文件内容冲突，已保留两份文件并停止自动操作。"
                                .to_string(),
                        );
                    }
                    fs::remove_file(&target)
                        .map_err(|error| format!("无法清理未完成的归档目标：{error}"))?;
                    if let Some(parent) = target.parent() {
                        sync_archive_directory(parent)?;
                    }
                }
                (false, true) => {
                    ensure_restore_target_parent_is_safe(codex_home, &source)?;
                    relocate_archive_file_without_replace(
                        codex_home,
                        &target,
                        &source,
                        &file.content_revision,
                    )?;
                }
                (false, false) => {
                    return Err("归档回滚发现会话文件同时丢失，已停止自动操作。".to_string())
                }
            }
        }
        for (visibility, state_database, current) in visibility_states {
            if current == visibility.archived && visibility.archived != visibility.original {
                restore_thread_visibility(
                    Some(&state_database),
                    &item.session_id,
                    visibility.original.as_ref(),
                )?;
                restored_visibility
                    .insert((visibility.state_database.clone(), item.session_id.clone()));
            }
        }
    }
    verify_archive_transaction_rollback(codex_home, &transaction.manifest, &restored_visibility)?;
    transaction.manifest.state = ArchiveTransactionState::RolledBack;
    write_archive_transaction_manifest(&transaction.directory, &transaction.manifest)?;
    fs::remove_dir_all(&transaction.directory)
        .map_err(|error| format!("无法清理已回滚的归档事务：{error}"))?;
    Ok(())
}

fn execute_thread_archive_transaction<Revalidate>(
    codex_home: &Path,
    transaction_root: &Path,
    prepared: PreparedThreadArchive,
    mut revalidate: Revalidate,
) -> Result<MutationReport, String>
where
    Revalidate: FnMut() -> Result<(), String>,
{
    revalidate()?;
    let mut transaction = create_archive_transaction(transaction_root, codex_home, &prepared)?;
    let result = (|| -> Result<MutationReport, String> {
        verify_archive_transaction_precondition(codex_home, &transaction.manifest)?;
        revalidate()?;
        for item in &transaction.manifest.items {
            for file in &item.files {
                let source = codex_home.join(&file.source_relative);
                let target = codex_home.join(&file.target_relative);
                ensure_restore_target_parent_is_safe(codex_home, &target)?;
                relocate_archive_file_without_replace(
                    codex_home,
                    &source,
                    &target,
                    &file.content_revision,
                )?;
            }
            for visibility in &item.visibilities {
                let state_database = codex_home.join(&visibility.state_database);
                validate_archive_state_database_path(&state_database)?;
                restore_thread_visibility(
                    Some(&state_database),
                    &item.session_id,
                    visibility.archived.as_ref(),
                )?;
            }
            revalidate()?;
        }
        verify_archive_transaction_postcondition(codex_home, &transaction.manifest)?;
        revalidate()?;
        transaction.manifest.state = ArchiveTransactionState::Committed;
        write_archive_transaction_manifest(&transaction.directory, &transaction.manifest)?;
        let affected_count = transaction.manifest.items.len();
        let _ = fs::remove_dir_all(&transaction.directory);
        Ok(MutationReport {
            requested_count: prepared.requested_count,
            affected_count,
            released_bytes: 0,
            message: format!("已安全归档 {affected_count} 条会话"),
        })
    })();
    match result {
        Ok(report) => Ok(report),
        Err(error) => match rollback_archive_transaction(codex_home, &mut transaction) {
            Ok(()) => Err(format!(
                "{error} 归档产生的文件与官方可见性变更已自动恢复至预览前状态；会话索引未被本操作写入。请重新预览后再试。"
            )),
            Err(_) => Err(concat!(
                "会话归档操作未能完成，且无法确认自动恢复结果。",
                "请暂时不要继续修改会话，并保留 QuotaHorizon 应用数据以便恢复。"
            )
            .to_string()),
        },
    }
}

fn collect_archive_transactions(
    transaction_root: &Path,
    codex_home: &Path,
) -> Result<Vec<ArchiveTransaction>, String> {
    if !transaction_root.exists() {
        return Ok(Vec::new());
    }
    ensure_archive_transaction_directory(transaction_root)?;
    let mut transactions = Vec::new();
    for entry in fs::read_dir(transaction_root)
        .map_err(|error| format!("无法读取会话归档事务目录：{error}"))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if !file_type.is_dir() {
            return Err("会话归档事务目录包含意外文件。".to_string());
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(".capture-") {
            fs::remove_dir_all(entry.path())
                .map_err(|error| format!("无法清理未完成的归档 capture：{error}"))?;
            continue;
        }
        transactions.push(read_archive_transaction(codex_home, &entry.path())?);
    }
    transactions.sort_by(|left, right| left.directory.cmp(&right.directory));
    Ok(transactions)
}

fn recover_incomplete_archive_transactions_with_revalidate<Revalidate>(
    codex_home: &Path,
    transaction_root: &Path,
    mut revalidate: Revalidate,
) -> Result<(), String>
where
    Revalidate: FnMut() -> Result<(), String>,
{
    for mut transaction in collect_archive_transactions(transaction_root, codex_home)? {
        revalidate()?;
        match transaction.manifest.state {
            ArchiveTransactionState::Prepared => {
                rollback_archive_transaction(codex_home, &mut transaction)?;
            }
            ArchiveTransactionState::Committed => {
                verify_archive_transaction_postcondition(codex_home, &transaction.manifest)
                    .map_err(|_| {
                        "已提交的会话归档事务状态异常，已保留事务清单并停止自动操作。".to_string()
                    })?;
                fs::remove_dir_all(&transaction.directory)
                    .map_err(|error| format!("无法清理已提交的归档事务：{error}"))?;
            }
            ArchiveTransactionState::RolledBack => {
                fs::remove_dir_all(&transaction.directory)
                    .map_err(|error| format!("无法清理已回滚的归档事务：{error}"))?;
            }
        }
        revalidate()?;
    }
    Ok(())
}

fn recover_incomplete_archive_transactions<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<(), String> {
    let transaction_root = archive_transaction_root(app)?;
    if !transaction_root.exists()
        || fs::read_dir(&transaction_root)
            .map_err(|error| format!("无法读取会话归档事务目录：{error}"))?
            .next()
            .is_none()
    {
        return Ok(());
    }
    let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "会话归档恢复安全互斥不可用。".to_string())?;
    let codex_home = resolve_paths(app)?.codex_home;
    recover_incomplete_archive_transactions_with_revalidate(&codex_home, &transaction_root, || {
        lease
            .revalidate(&mut legacy_probe)
            .map_err(|_| "会话归档恢复期间安全互斥失效。".to_string())
    })
}

pub(crate) fn prepare_codex_thread_archive_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    session_ids: Vec<String>,
) -> Result<ThreadArchivePreview, String> {
    recover_incomplete_archive_transactions(&app)?;
    let paths = resolve_paths(&app)?;
    let prepared = prepare_thread_archive(&paths.codex_home, session_ids)?;
    let ids = prepared
        .items
        .iter()
        .map(|item| item.session_id.clone())
        .collect::<Vec<_>>();
    let items = prepared
        .items
        .iter()
        .map(|item| ThreadArchivePreviewItem {
            session_id: item.session_id.clone(),
            title: item.title.clone(),
            cwd: item.cwd.clone(),
            size_bytes: item.size_bytes,
        })
        .collect::<Vec<_>>();
    let total_size_bytes = items.iter().map(|item| item.size_bytes).sum();
    let (confirm_token, expires_at) = issue_thread_archive_plan(ids, prepared.revision)?;
    Ok(ThreadArchivePreview {
        confirm_token,
        expires_at: expires_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        requested_count: prepared.requested_count,
        affected_count: items.len(),
        total_size_bytes,
        items,
        protected_targets: vec![
            ThreadArchiveProtectedTarget::RolloutFiles,
            ThreadArchiveProtectedTarget::SessionIndex,
            ThreadArchiveProtectedTarget::StateVisibility,
        ],
        conflicts_checked: true,
        preserves_session_index: true,
        automatic_rollback: true,
        crash_recovery: true,
    })
}

pub(crate) fn confirm_codex_thread_archive_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<MutationReport, String> {
    let plan = consume_thread_archive_plan(&confirm_token)?;
    let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "会话归档安全互斥在执行前失效，未修改任何会话。".to_string())?;
    let paths = resolve_paths(&app)?;
    let transaction_root = archive_transaction_root(&app)?;
    recover_incomplete_archive_transactions_with_revalidate(
        &paths.codex_home,
        &transaction_root,
        || {
            lease
                .revalidate(&mut legacy_probe)
                .map_err(|_| "会话归档恢复期间安全互斥失效。".to_string())
        },
    )?;
    let prepared = prepare_thread_archive(&paths.codex_home, plan.session_ids)?;
    if prepared.revision != plan.expected_revision {
        return Err(
            "会话内容、索引或归档目标在预览后发生变化，未归档任何会话。请重新预览。".to_string(),
        );
    }
    execute_thread_archive_transaction(&paths.codex_home, &transaction_root, prepared, || {
        lease
            .revalidate(&mut legacy_probe)
            .map_err(|_| "会话归档安全互斥失效。".to_string())
    })
}
