const THREAD_REBIND_PLAN_PREFIX: &str = "session-rebind-plan:v1:";
const THREAD_REBIND_PLAN_TTL_SECONDS: i64 = 120;
const MAX_PENDING_THREAD_REBIND_PLANS: usize = 32;
const REBIND_TRANSACTION_FORMAT: &str = "quota-horizon-session-rebind-v1";
const REBIND_TRANSACTION_PREFIX: &str = "session-rebind-tx-v1-";
const REBIND_TRANSACTION_MANIFEST: &str = "manifest.json";
const MAX_REBIND_TRANSACTION_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REBIND_CATALOG_DATABASES: usize = 16;

#[derive(Debug, Clone)]
struct PendingThreadRebindPlan {
    session_id: String,
    target_cwd: String,
    expected_revision: String,
    expires_at: DateTime<Utc>,
}

#[derive(Default)]
struct PendingThreadRebindPlans {
    plans: HashMap<String, PendingThreadRebindPlan>,
}

impl PendingThreadRebindPlans {
    fn issue(
        &mut self,
        prepared: &PreparedThreadRebind,
        now: DateTime<Utc>,
    ) -> Result<(String, DateTime<Utc>), String> {
        self.plans.retain(|_, plan| plan.expires_at > now);
        if self.plans.len() >= MAX_PENDING_THREAD_REBIND_PLANS {
            return Err("待确认的目录修改过多，请稍后重试。".to_string());
        }
        let token = format!(
            "{THREAD_REBIND_PLAN_PREFIX}{}",
            Uuid::new_v4().hyphenated()
        );
        let expires_at = now + chrono::Duration::seconds(THREAD_REBIND_PLAN_TTL_SECONDS);
        self.plans.insert(
            token.clone(),
            PendingThreadRebindPlan {
                session_id: prepared.session_id.clone(),
                target_cwd: prepared.target_cwd.clone(),
                expected_revision: prepared.revision.clone(),
                expires_at,
            },
        );
        Ok((token, expires_at))
    }

    fn consume(
        &mut self,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<PendingThreadRebindPlan, String> {
        validate_thread_rebind_plan_token(token)?;
        let plan = self
            .plans
            .remove(token)
            .ok_or_else(|| "目录修改确认已失效，请重新预览。".to_string())?;
        if plan.expires_at <= now {
            return Err("目录修改确认已过期，请重新预览。".to_string());
        }
        Ok(plan)
    }
}

fn validate_thread_rebind_plan_token(token: &str) -> Result<(), String> {
    let value = token
        .strip_prefix(THREAD_REBIND_PLAN_PREFIX)
        .ok_or_else(|| "目录修改确认令牌格式无效。".to_string())?;
    let parsed = Uuid::parse_str(value).map_err(|_| "目录修改确认令牌格式无效。".to_string())?;
    if parsed.hyphenated().to_string() != value {
        return Err("目录修改确认令牌格式无效。".to_string());
    }
    Ok(())
}

fn pending_thread_rebind_plans() -> &'static std::sync::Mutex<PendingThreadRebindPlans> {
    static PLANS: std::sync::OnceLock<std::sync::Mutex<PendingThreadRebindPlans>> =
        std::sync::OnceLock::new();
    PLANS.get_or_init(|| std::sync::Mutex::new(PendingThreadRebindPlans::default()))
}

fn issue_thread_rebind_plan(
    prepared: &PreparedThreadRebind,
) -> Result<(String, DateTime<Utc>), String> {
    pending_thread_rebind_plans()
        .lock()
        .map_err(|_| "目录修改预览状态不可用。".to_string())?
        .issue(prepared, Utc::now())
}

fn consume_thread_rebind_plan(token: &str) -> Result<PendingThreadRebindPlan, String> {
    pending_thread_rebind_plans()
        .lock()
        .map_err(|_| "目录修改预览状态不可用。".to_string())?
        .consume(token, Utc::now())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RebindStateCwd {
    database_relative: String,
    original_cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RebindCatalogCwd {
    database_relative: String,
    host_id: String,
    original_cwd: Option<String>,
}

#[derive(Debug, Clone)]
struct PreparedThreadRebindFile {
    source_relative: PathBuf,
    original_revision: String,
    compressed: bool,
}

#[derive(Debug, Clone)]
struct PreparedThreadRebind {
    session_id: String,
    title: String,
    old_cwd: String,
    target_cwd: String,
    index_present: bool,
    index_entry: Value,
    files: Vec<PreparedThreadRebindFile>,
    state_rows: Vec<RebindStateCwd>,
    catalog_rows: Vec<RebindCatalogCwd>,
    revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RebindTransactionState {
    Prepared,
    Committed,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RebindTransactionFile {
    source_relative: String,
    backup_name: String,
    replacement_name: String,
    original_revision: String,
    target_revision: String,
    compressed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RebindTransactionManifest {
    format: String,
    id: String,
    created_at: String,
    state: RebindTransactionState,
    session_id: String,
    old_cwd: String,
    target_cwd: String,
    index_present: bool,
    index_entry: Value,
    files: Vec<RebindTransactionFile>,
    state_rows: Vec<RebindStateCwd>,
    catalog_rows: Vec<RebindCatalogCwd>,
}

#[derive(Debug)]
struct RebindTransaction {
    directory: PathBuf,
    manifest: RebindTransactionManifest,
}

fn state_database_cwd(path: &Path, session_id: &str) -> Result<Option<String>, String> {
    validate_archive_state_database_path(path)?;
    let connection = Connection::open(path)
        .map_err(|error| format!("无法打开 Codex state DB {}：{error}", path.display()))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    if !table_exists(&connection, "threads")? || !table_has_column(&connection, "threads", "cwd")?
    {
        return Ok(None);
    }
    connection
        .query_row(
            "SELECT cwd FROM threads WHERE id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn catalog_database_paths(codex_home: &Path) -> Result<Vec<PathBuf>, String> {
    let directory = codex_home.join("sqlite");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() => {
            return Err("Codex catalog 目录类型不安全。".to_string())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("无法验证 Codex catalog 目录：{error}")),
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_symlink() {
            return Err(format!(
                "Codex catalog 包含不安全的 symlink：{}",
                entry.path().display()
            ));
        }
        let path = entry.path();
        if !file_type.is_file() || path.extension().and_then(|value| value.to_str()) != Some("db") {
            continue;
        }
        let connection = Connection::open(&path).map_err(|error| error.to_string())?;
        if table_exists(&connection, "local_thread_catalog")?
            && table_has_column(&connection, "local_thread_catalog", "host_id")?
            && table_has_column(&connection, "local_thread_catalog", "thread_id")?
            && table_has_column(&connection, "local_thread_catalog", "cwd")?
        {
            paths.push(path);
        }
    }
    paths.sort();
    if paths.len() > MAX_REBIND_CATALOG_DATABASES {
        return Err("检测到过多 Codex catalog，已拒绝自动修改目录。".to_string());
    }
    Ok(paths)
}

fn catalog_database_cwds(
    path: &Path,
    session_id: &str,
) -> Result<Vec<(String, Option<String>)>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法验证 Codex catalog {}：{error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(format!("Codex catalog 路径类型不安全：{}", path.display()));
    }
    let connection = Connection::open(path)
        .map_err(|error| format!("无法打开 Codex catalog {}：{error}", path.display()))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT host_id, cwd FROM local_thread_catalog WHERE thread_id = ?1 ORDER BY host_id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![session_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

fn normalized_rebind_relative(codex_home: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(codex_home)
        .map_err(|_| "受保护的会话路径越出 CODEX_HOME。".to_string())?;
    normalized_relative_text(relative)
}

fn rebind_revision(prepared: &PreparedThreadRebind) -> Result<String, String> {
    let value = json!({
        "sessionId": prepared.session_id,
        "oldCwd": prepared.old_cwd,
        "targetCwd": prepared.target_cwd,
        "indexPresent": prepared.index_present,
        "indexEntry": prepared.index_entry,
        "files": prepared.files.iter().map(|file| json!({
            "sourceRelative": file.source_relative.to_string_lossy(),
            "originalRevision": file.original_revision,
            "compressed": file.compressed,
        })).collect::<Vec<_>>(),
        "stateRows": prepared.state_rows,
        "catalogRows": prepared.catalog_rows,
    });
    let bytes = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn prepare_thread_rebind(
    codex_home: &Path,
    session_id: &str,
    target_cwd: &str,
) -> Result<PreparedThreadRebind, String> {
    if safe_resume_command(session_id).is_none() {
        return Err("会话 ID 无效，已拒绝修改。".to_string());
    }
    let target_path = validate_resume_directory(target_cwd)?;
    let target_cwd = target_path.to_string_lossy().into_owned();
    let snapshots = gather_snapshots(codex_home)?;
    let matches = snapshots
        .iter()
        .filter(|snapshot| snapshot.session_id == session_id)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!(
            "无法唯一定位会话 {session_id}，请刷新列表或先修复可见性。"
        ));
    }
    let snapshot = matches[0];
    if rollout_status(&snapshot.relative_path) != Some("active") {
        return Err("该会话已归档，请先恢复到 active 后再修改目录。".to_string());
    }
    if snapshot.cwd == target_cwd
        || Path::new(&snapshot.cwd)
            .canonicalize()
            .is_ok_and(|path| path == target_path)
    {
        return Err("所选目录与会话当前目录相同。".to_string());
    }
    let mut files = Vec::with_capacity(snapshot.physical_paths.len());
    for path in &snapshot.physical_paths {
        ensure_restore_target_parent_is_safe(codex_home, path)?;
        let meta = first_rollout_value(path)?
            .ok_or_else(|| "会话文件缺少 session_meta，已拒绝修改。".to_string())?;
        if snapshot_id(&meta).as_deref() != Some(session_id)
            || snapshot_cwd(&meta).as_deref() != Some(snapshot.cwd.as_str())
        {
            return Err("同一会话的 rollout metadata 不一致，请先修复后再改目录。".to_string());
        }
        files.push(PreparedThreadRebindFile {
            source_relative: path
                .strip_prefix(codex_home)
                .map_err(|_| "会话文件越出 CODEX_HOME。".to_string())?
                .to_path_buf(),
            original_revision: stable_archive_file_revision(path)?,
            compressed: path.extension().and_then(|value| value.to_str()) == Some("zst"),
        });
    }
    files.sort_by(|left, right| left.source_relative.cmp(&right.source_relative));

    let index = index_values(codex_home)?;
    let index_present = index.contains_key(session_id);
    let index_entry = index
        .get(session_id)
        .cloned()
        .unwrap_or_else(|| snapshot.index_value.clone());
    let mut state_rows = Vec::new();
    for database in archive_state_databases(codex_home)? {
        if let Some(original_cwd) = state_database_cwd(&database, session_id)? {
            state_rows.push(RebindStateCwd {
                database_relative: normalized_rebind_relative(codex_home, &database)?,
                original_cwd,
            });
        }
    }
    state_rows.sort_by(|left, right| left.database_relative.cmp(&right.database_relative));

    let mut catalog_rows = Vec::new();
    for database in catalog_database_paths(codex_home)? {
        let relative = normalized_rebind_relative(codex_home, &database)?;
        for (host_id, original_cwd) in catalog_database_cwds(&database, session_id)? {
            catalog_rows.push(RebindCatalogCwd {
                database_relative: relative.clone(),
                host_id,
                original_cwd,
            });
        }
    }
    catalog_rows.sort_by(|left, right| {
        left.database_relative
            .cmp(&right.database_relative)
            .then_with(|| left.host_id.cmp(&right.host_id))
    });

    let mut prepared = PreparedThreadRebind {
        session_id: session_id.to_string(),
        title: snapshot.title.clone(),
        old_cwd: snapshot.cwd.clone(),
        target_cwd,
        index_present,
        index_entry,
        files,
        state_rows,
        catalog_rows,
        revision: String::new(),
    };
    prepared.revision = rebind_revision(&prepared)?;
    Ok(prepared)
}

fn rebind_transaction_root<R: Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法定位应用数据目录：{error}"))?
        .join("codex-thread-rebind-transactions"))
}

fn ensure_rebind_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() => {
            return Err(format!("会话目录修改事务路径类型不安全：{}", path.display()))
        }
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("无法验证会话目录修改事务路径：{error}")),
    }
    fs::create_dir_all(path).map_err(|error| format!("无法创建会话目录修改事务路径：{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("无法保护会话目录修改事务路径：{error}"))?;
    }
    Ok(())
}

fn sync_rebind_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("无法同步会话目录修改事务：{error}"))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn write_rebind_manifest(
    directory: &Path,
    manifest: &RebindTransactionManifest,
) -> Result<(), String> {
    let text = serde_json::to_string_pretty(manifest).map_err(|error| error.to_string())?;
    if text.len() as u64 > MAX_REBIND_TRANSACTION_MANIFEST_BYTES {
        return Err("会话目录修改事务清单过大。".to_string());
    }
    write_text_atomic(
        &directory.join(REBIND_TRANSACTION_MANIFEST),
        &format!("{text}\n"),
    )?;
    sync_rebind_directory(directory)
}

fn rewrite_rebind_payload(
    source: &Path,
    target: &Path,
    compressed: bool,
    session_id: &str,
    old_cwd: &str,
    target_cwd: &str,
) -> Result<(), String> {
    let mut reader: Box<dyn BufRead> = if compressed {
        let file = File::open(source).map_err(|error| error.to_string())?;
        let decoder = zstd::stream::read::Decoder::new(file).map_err(|error| error.to_string())?;
        Box::new(BufReader::new(decoder))
    } else {
        Box::new(BufReader::new(
            File::open(source).map_err(|error| error.to_string())?,
        ))
    };
    let mut first = String::new();
    reader
        .read_line(&mut first)
        .map_err(|error| error.to_string())?;
    let mut meta: Value = serde_json::from_str(first.trim_end())
        .map_err(|_| "会话文件首行不是有效 session_meta。".to_string())?;
    if meta.get("type").and_then(Value::as_str) != Some("session_meta")
        || snapshot_id(&meta).as_deref() != Some(session_id)
        || snapshot_cwd(&meta).as_deref() != Some(old_cwd)
    {
        return Err("会话文件 metadata 与预览不一致。".to_string());
    }
    let cwd = meta
        .pointer_mut("/payload/cwd")
        .ok_or_else(|| "会话 metadata 缺少 cwd。".to_string())?;
    *cwd = Value::String(target_cwd.to_string());

    let output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| format!("无法创建会话目录修改 payload：{error}"))?;
    if compressed {
        let mut encoder =
            zstd::stream::write::Encoder::new(output, 3).map_err(|error| error.to_string())?;
        serde_json::to_writer(&mut encoder, &meta).map_err(|error| error.to_string())?;
        encoder.write_all(b"\n").map_err(|error| error.to_string())?;
        std::io::copy(&mut reader, &mut encoder).map_err(|error| error.to_string())?;
        encoder
            .finish()
            .map_err(|error| error.to_string())?
            .sync_all()
            .map_err(|error| error.to_string())?;
    } else {
        let mut output = output;
        serde_json::to_writer(&mut output, &meta).map_err(|error| error.to_string())?;
        output.write_all(b"\n").map_err(|error| error.to_string())?;
        std::io::copy(&mut reader, &mut output).map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn create_rebind_transaction(
    transaction_root: &Path,
    codex_home: &Path,
    prepared: &PreparedThreadRebind,
) -> Result<RebindTransaction, String> {
    ensure_rebind_directory(transaction_root)?;
    let id = format!(
        "{REBIND_TRANSACTION_PREFIX}{}",
        Uuid::new_v4().hyphenated()
    );
    let staging = transaction_root.join(format!(".capture-{}", Uuid::new_v4().hyphenated()));
    ensure_rebind_directory(&staging)?;
    let backup_dir = staging.join("backups");
    let replacement_dir = staging.join("replacements");
    ensure_rebind_directory(&backup_dir)?;
    ensure_rebind_directory(&replacement_dir)?;
    let result = (|| -> Result<RebindTransaction, String> {
        let mut files = Vec::with_capacity(prepared.files.len());
        for (index, item) in prepared.files.iter().enumerate() {
            let source = codex_home.join(&item.source_relative);
            if stable_archive_file_revision(&source)? != item.original_revision {
                return Err("会话内容在创建恢复点期间发生变化。".to_string());
            }
            let backup_name = format!("{index}.bin");
            let replacement_name = format!("{index}.bin");
            let backup = backup_dir.join(&backup_name);
            let replacement = replacement_dir.join(&replacement_name);
            fs::copy(&source, &backup)
                .map_err(|error| format!("无法备份会话 rollout：{error}"))?;
            File::open(&backup)
                .and_then(|file| file.sync_all())
                .map_err(|error| format!("无法同步会话 rollout 备份：{error}"))?;
            if stable_archive_file_revision(&source)? != item.original_revision
                || stable_archive_file_revision(&backup)? != item.original_revision
            {
                return Err("会话内容在复制恢复点期间发生变化。".to_string());
            }
            rewrite_rebind_payload(
                &backup,
                &replacement,
                item.compressed,
                &prepared.session_id,
                &prepared.old_cwd,
                &prepared.target_cwd,
            )?;
            let target_revision = stable_archive_file_revision(&replacement)?;
            files.push(RebindTransactionFile {
                source_relative: normalized_relative_text(&item.source_relative)?,
                backup_name,
                replacement_name,
                original_revision: item.original_revision.clone(),
                target_revision,
                compressed: item.compressed,
            });
        }
        sync_rebind_directory(&backup_dir)?;
        sync_rebind_directory(&replacement_dir)?;
        let manifest = RebindTransactionManifest {
            format: REBIND_TRANSACTION_FORMAT.to_string(),
            id: id.clone(),
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            state: RebindTransactionState::Prepared,
            session_id: prepared.session_id.clone(),
            old_cwd: prepared.old_cwd.clone(),
            target_cwd: prepared.target_cwd.clone(),
            index_present: prepared.index_present,
            index_entry: prepared.index_entry.clone(),
            files,
            state_rows: prepared.state_rows.clone(),
            catalog_rows: prepared.catalog_rows.clone(),
        };
        write_rebind_manifest(&staging, &manifest)?;
        let destination = transaction_root.join(&id);
        fs::rename(&staging, &destination)
            .map_err(|error| format!("无法提交会话目录修改恢复点：{error}"))?;
        sync_rebind_directory(transaction_root)?;
        read_rebind_transaction(codex_home, &destination)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn valid_transaction_payload_name(value: &str) -> bool {
    let Some(index) = value.strip_suffix(".bin") else {
        return false;
    };
    !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_recorded_rebind_cwd(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_RESUME_CWD_CHARS
        && !value.chars().any(char::is_control)
        && Path::new(value).is_absolute()
        && !Path::new(value).components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
}

fn valid_rebind_state_database_relative(value: &str) -> bool {
    let Some(relative) = safe_relative_path(value) else {
        return false;
    };
    let parts = relative
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    let versioned = |name: &str| {
        name.strip_prefix("state_")
            .and_then(|value| value.strip_suffix(".sqlite"))
            .is_some_and(|value| {
                !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
            })
    };
    matches!(parts.as_slice(), [name] if versioned(name))
        || matches!(parts.as_slice(), ["sqlite", "state.db"])
        || matches!(parts.as_slice(), ["sqlite", name] if versioned(name))
}

fn valid_rebind_catalog_database_relative(value: &str) -> bool {
    let Some(relative) = safe_relative_path(value) else {
        return false;
    };
    let parts = relative
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    matches!(parts.as_slice(), ["sqlite", name] if {
        let path = Path::new(name);
        path.file_stem().is_some_and(|stem| !stem.is_empty())
            && path.extension().and_then(|value| value.to_str()) == Some("db")
    })
}

fn read_rebind_transaction(
    codex_home: &Path,
    directory: &Path,
) -> Result<RebindTransaction, String> {
    ensure_rebind_directory(directory)?;
    let manifest_path = directory.join(REBIND_TRANSACTION_MANIFEST);
    let metadata = fs::symlink_metadata(&manifest_path)
        .map_err(|error| format!("无法验证会话目录修改事务清单：{error}"))?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > MAX_REBIND_TRANSACTION_MANIFEST_BYTES
    {
        return Err("会话目录修改事务清单类型或大小无效。".to_string());
    }
    let manifest: RebindTransactionManifest = serde_json::from_slice(
        &fs::read(&manifest_path).map_err(|error| error.to_string())?,
    )
    .map_err(|_| "会话目录修改事务清单无效。".to_string())?;
    let directory_name = directory.file_name().and_then(|value| value.to_str());
    let id_value = manifest
        .id
        .strip_prefix(REBIND_TRANSACTION_PREFIX)
        .ok_or_else(|| "会话目录修改事务 ID 无效。".to_string())?;
    if manifest.format != REBIND_TRANSACTION_FORMAT
        || directory_name != Some(manifest.id.as_str())
        || Uuid::parse_str(id_value)
            .ok()
            .is_none_or(|uuid| uuid.hyphenated().to_string() != id_value)
        || safe_resume_command(&manifest.session_id).is_none()
        || !valid_recorded_rebind_cwd(&manifest.old_cwd)
        || !valid_recorded_rebind_cwd(&manifest.target_cwd)
        || manifest.files.is_empty()
    {
        return Err("会话目录修改事务身份或目标无效。".to_string());
    }
    if manifest.state != RebindTransactionState::Prepared {
        return Ok(RebindTransaction {
            directory: directory.to_path_buf(),
            manifest,
        });
    }
    ensure_rebind_directory(&directory.join("backups"))?;
    ensure_rebind_directory(&directory.join("replacements"))?;
    if manifest.state_rows.len() > MAX_ARCHIVE_STATE_DATABASES
        || manifest.state_rows.iter().any(|row| {
            !valid_rebind_state_database_relative(&row.database_relative)
                || validate_archive_state_database_path(
                    &codex_home.join(&row.database_relative),
                )
                .is_err()
        })
        || manifest
            .catalog_rows
            .iter()
            .any(|row| !valid_rebind_catalog_database_relative(&row.database_relative))
    {
        return Err("会话目录修改事务包含不受信任的数据库路径。".to_string());
    }
    let mut source_paths = HashSet::new();
    let mut backup_names = HashSet::new();
    let mut replacement_names = HashSet::new();
    let mut state_rows = HashSet::new();
    let mut catalog_rows = HashSet::new();
    for row in &manifest.state_rows {
        if !state_rows.insert(row.database_relative.clone()) {
            return Err("会话目录修改事务包含重复 state DB。".to_string());
        }
    }
    for row in &manifest.catalog_rows {
        if !catalog_rows.insert((row.database_relative.clone(), row.host_id.clone())) {
            return Err("会话目录修改事务包含重复 catalog row。".to_string());
        }
    }
    for file in &manifest.files {
        let relative = safe_relative_path(&file.source_relative)
            .ok_or_else(|| "会话目录修改事务包含不安全的 rollout 路径。".to_string())?;
        let source = codex_home.join(relative);
        ensure_restore_target_parent_is_safe(codex_home, &source)?;
        if !valid_transaction_payload_name(&file.backup_name)
            || !valid_transaction_payload_name(&file.replacement_name)
            || !valid_archive_content_revision(&file.original_revision)
            || !valid_archive_content_revision(&file.target_revision)
        {
            return Err("会话目录修改事务包含无效的 payload 描述。".to_string());
        }
        if !source_paths.insert(file.source_relative.clone())
            || !backup_names.insert(file.backup_name.clone())
            || !replacement_names.insert(file.replacement_name.clone())
        {
            return Err("会话目录修改事务包含重复 payload。".to_string());
        }
        for payload in [
            directory.join("backups").join(&file.backup_name),
            directory
                .join("replacements")
                .join(&file.replacement_name),
        ] {
            let metadata = fs::symlink_metadata(&payload)
                .map_err(|_| "会话目录修改恢复 payload 缺失。".to_string())?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err("会话目录修改恢复 payload 类型不安全。".to_string());
            }
        }
        if stable_archive_file_revision(&directory.join("backups").join(&file.backup_name))?
            != file.original_revision
            || stable_archive_file_revision(
                &directory
                    .join("replacements")
                    .join(&file.replacement_name),
            )? != file.target_revision
        {
            return Err("会话目录修改恢复 payload 校验失败。".to_string());
        }
    }
    Ok(RebindTransaction {
        directory: directory.to_path_buf(),
        manifest,
    })
}

fn index_matches_rebind_manifest(
    codex_home: &Path,
    manifest: &RebindTransactionManifest,
) -> Result<bool, String> {
    let index = index_values(codex_home)?;
    Ok(match index.get(&manifest.session_id) {
        Some(value) => manifest.index_present && value == &manifest.index_entry,
        None => !manifest.index_present,
    })
}

fn state_row_matches(path: &Path, session_id: &str, expected: &str) -> Result<bool, String> {
    Ok(state_database_cwd(path, session_id)?.as_deref() == Some(expected))
}

fn catalog_row_cwd(
    path: &Path,
    session_id: &str,
    host_id: &str,
) -> Result<Option<Option<String>>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法验证 Codex catalog {}：{error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(format!("Codex catalog 路径类型不安全：{}", path.display()));
    }
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    connection
        .query_row(
            "SELECT cwd FROM local_thread_catalog WHERE thread_id = ?1 AND host_id = ?2",
            params![session_id, host_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn verify_rebind_precondition(
    codex_home: &Path,
    manifest: &RebindTransactionManifest,
) -> Result<(), String> {
    if !index_matches_rebind_manifest(codex_home, manifest)? {
        return Err("会话索引在确认期间发生变化。".to_string());
    }
    for file in &manifest.files {
        let source = codex_home.join(&file.source_relative);
        if stable_archive_file_revision(&source)? != file.original_revision {
            return Err("会话内容在确认期间发生变化。".to_string());
        }
    }
    for row in &manifest.state_rows {
        let path = codex_home.join(&row.database_relative);
        if !state_row_matches(&path, &manifest.session_id, &row.original_cwd)? {
            return Err("Codex state DB 在确认期间发生变化。".to_string());
        }
    }
    for row in &manifest.catalog_rows {
        let path = codex_home.join(&row.database_relative);
        if catalog_row_cwd(&path, &manifest.session_id, &row.host_id)?
            != Some(row.original_cwd.clone())
        {
            return Err("Codex thread catalog 在确认期间发生变化。".to_string());
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn ensure_rollout_not_open(path: &Path) -> Result<(), String> {
    let output = std::process::Command::new("/usr/sbin/lsof")
        .arg("-t")
        .arg(path)
        .output()
        .map_err(|error| format!("无法确认会话文件是否仍被使用：{error}"))?;
    if output.status.success() && !output.stdout.is_empty() {
        return Err("会话文件仍被 Codex 使用，请先关闭该会话后再永久修改目录。".to_string());
    }
    if output.status.success() || output.status.code() == Some(1) {
        Ok(())
    } else {
        Err("无法确认会话文件是否仍被使用，已拒绝修改。".to_string())
    }
}

#[cfg(not(target_os = "macos"))]
fn ensure_rollout_not_open(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn replace_rebind_file(payload: &Path, target: &Path) -> Result<(), String> {
    let temporary = target.with_extension(format!("rebind-{}.tmp", Uuid::new_v4().hyphenated()));
    fs::copy(payload, &temporary)
        .map_err(|error| format!("无法准备会话 metadata 原子替换：{error}"))?;
    File::open(&temporary)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("无法同步会话 metadata 临时文件：{error}"))?;
    if let Err(error) = replace_file(&temporary, target) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    if let Some(parent) = target.parent() {
        sync_rebind_directory(parent)?;
    }
    Ok(())
}

fn update_state_database_cwd(
    path: &Path,
    session_id: &str,
    expected: &str,
    target: &str,
) -> Result<(), String> {
    validate_archive_state_database_path(path)?;
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let changed = connection
        .execute(
            "UPDATE threads SET cwd = ?1 WHERE id = ?2 AND cwd = ?3",
            params![target, session_id, expected],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Codex state DB cwd 已变化，停止目录修改。".to_string());
    }
    Ok(())
}

fn update_catalog_cwd(
    path: &Path,
    session_id: &str,
    host_id: &str,
    expected: Option<&str>,
    target: Option<&str>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法验证 Codex catalog {}：{error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(format!("Codex catalog 路径类型不安全：{}", path.display()));
    }
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let changed = connection
        .execute(
            concat!(
                "UPDATE local_thread_catalog SET cwd = ?1 ",
                "WHERE thread_id = ?2 AND host_id = ?3 AND cwd IS ?4"
            ),
            params![target, session_id, host_id, expected],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Codex thread catalog cwd 已变化，停止目录修改。".to_string());
    }
    Ok(())
}

fn verify_rebind_postcondition(
    codex_home: &Path,
    manifest: &RebindTransactionManifest,
) -> Result<(), String> {
    if !index_matches_rebind_manifest(codex_home, manifest)? {
        return Err("目录修改后会话索引校验失败。".to_string());
    }
    for file in &manifest.files {
        let source = codex_home.join(&file.source_relative);
        if stable_archive_file_revision(&source)? != file.target_revision {
            return Err("目录修改后 rollout 内容校验失败。".to_string());
        }
        let meta = first_rollout_value(&source)?
            .ok_or_else(|| "目录修改后 rollout metadata 缺失。".to_string())?;
        if snapshot_id(&meta).as_deref() != Some(manifest.session_id.as_str())
            || snapshot_cwd(&meta).as_deref() != Some(manifest.target_cwd.as_str())
        {
            return Err("目录修改后 rollout cwd 校验失败。".to_string());
        }
    }
    for row in &manifest.state_rows {
        if !state_row_matches(
            &codex_home.join(&row.database_relative),
            &manifest.session_id,
            &manifest.target_cwd,
        )? {
            return Err("目录修改后 Codex state DB 校验失败。".to_string());
        }
    }
    for row in &manifest.catalog_rows {
        if catalog_row_cwd(
            &codex_home.join(&row.database_relative),
            &manifest.session_id,
            &row.host_id,
        )? != Some(Some(manifest.target_cwd.clone()))
        {
            return Err("目录修改后 Codex thread catalog 校验失败。".to_string());
        }
    }
    Ok(())
}

fn rollback_rebind_transaction(
    codex_home: &Path,
    transaction: &mut RebindTransaction,
) -> Result<(), String> {
    for row in transaction.manifest.catalog_rows.iter().rev() {
        let path = codex_home.join(&row.database_relative);
        let current = catalog_row_cwd(
            &path,
            &transaction.manifest.session_id,
            &row.host_id,
        )?;
        if current == Some(row.original_cwd.clone()) {
            continue;
        }
        if current == Some(Some(transaction.manifest.target_cwd.clone())) {
            update_catalog_cwd(
                &path,
                &transaction.manifest.session_id,
                &row.host_id,
                Some(&transaction.manifest.target_cwd),
                row.original_cwd.as_deref(),
            )?;
        } else {
            return Err("Codex thread catalog 出现无关变化，无法自动回滚。".to_string());
        }
    }
    for row in transaction.manifest.state_rows.iter().rev() {
        let path = codex_home.join(&row.database_relative);
        let current = state_database_cwd(&path, &transaction.manifest.session_id)?;
        if current.as_deref() == Some(row.original_cwd.as_str()) {
            continue;
        }
        if current.as_deref() == Some(transaction.manifest.target_cwd.as_str()) {
            update_state_database_cwd(
                &path,
                &transaction.manifest.session_id,
                &transaction.manifest.target_cwd,
                &row.original_cwd,
            )?;
        } else {
            return Err("Codex state DB 出现无关变化，无法自动回滚。".to_string());
        }
    }
    for file in transaction.manifest.files.iter().rev() {
        let source = codex_home.join(&file.source_relative);
        let current = stable_archive_file_revision(&source)?;
        if current == file.original_revision {
            continue;
        }
        if current != file.target_revision {
            return Err("rollout 出现无关变化，无法自动回滚目录修改。".to_string());
        }
        ensure_rollout_not_open(&source)?;
        replace_rebind_file(
            &transaction.directory.join("backups").join(&file.backup_name),
            &source,
        )?;
        if stable_archive_file_revision(&source)? != file.original_revision {
            return Err("rollout 自动回滚校验失败。".to_string());
        }
    }
    transaction.manifest.state = RebindTransactionState::RolledBack;
    write_rebind_manifest(&transaction.directory, &transaction.manifest)?;
    fs::remove_dir_all(&transaction.directory)
        .map_err(|error| format!("无法清理已回滚的目录修改事务：{error}"))?;
    Ok(())
}

fn execute_thread_rebind_transaction<Revalidate>(
    codex_home: &Path,
    transaction_root: &Path,
    prepared: PreparedThreadRebind,
    mut revalidate: Revalidate,
) -> Result<ThreadRebindReport, String>
where
    Revalidate: FnMut() -> Result<(), String>,
{
    revalidate()?;
    let mut transaction = create_rebind_transaction(transaction_root, codex_home, &prepared)?;
    let result = (|| -> Result<ThreadRebindReport, String> {
        verify_rebind_precondition(codex_home, &transaction.manifest)?;
        for file in &transaction.manifest.files {
            ensure_rollout_not_open(&codex_home.join(&file.source_relative))?;
        }
        revalidate()?;
        for file in &transaction.manifest.files {
            let source = codex_home.join(&file.source_relative);
            replace_rebind_file(
                &transaction
                    .directory
                    .join("replacements")
                    .join(&file.replacement_name),
                &source,
            )?;
            revalidate()?;
        }
        for row in &transaction.manifest.state_rows {
            update_state_database_cwd(
                &codex_home.join(&row.database_relative),
                &transaction.manifest.session_id,
                &row.original_cwd,
                &transaction.manifest.target_cwd,
            )?;
            revalidate()?;
        }
        for row in &transaction.manifest.catalog_rows {
            update_catalog_cwd(
                &codex_home.join(&row.database_relative),
                &transaction.manifest.session_id,
                &row.host_id,
                row.original_cwd.as_deref(),
                Some(&transaction.manifest.target_cwd),
            )?;
            revalidate()?;
        }
        verify_rebind_postcondition(codex_home, &transaction.manifest)?;
        revalidate()?;
        transaction.manifest.state = RebindTransactionState::Committed;
        write_rebind_manifest(&transaction.directory, &transaction.manifest)?;
        Ok(ThreadRebindReport {
            session_id: transaction.manifest.session_id.clone(),
            old_cwd: transaction.manifest.old_cwd.clone(),
            new_cwd: transaction.manifest.target_cwd.clone(),
            rollout_file_count: transaction.manifest.files.len(),
            state_database_row_count: transaction.manifest.state_rows.len(),
            catalog_row_count: transaction.manifest.catalog_rows.len(),
            message: "会话目录已安全更新；现在可在新目录继续。".to_string(),
        })
    })();
    match result {
        Ok(report) => {
            // Once the durable manifest says Committed, cleanup is not part of the
            // user-data transaction. A residue is safe and is retried by recovery.
            let _ = fs::remove_dir_all(&transaction.directory);
            Ok(report)
        }
        Err(error) => match rollback_rebind_transaction(codex_home, &mut transaction) {
            Ok(()) => Err(format!(
                "{error} 已自动恢复 rollout 与官方 cwd 到预览前状态；请重新预览后再试。"
            )),
            Err(_) => Err(concat!(
                "会话目录修改未能完成，且无法确认自动恢复结果。",
                "请暂时不要继续该会话，并保留 QuotaHorizon 应用数据以便恢复。"
            )
            .to_string()),
        },
    }
}

fn recover_rebind_transactions_with_revalidate<Revalidate>(
    codex_home: &Path,
    transaction_root: &Path,
    mut revalidate: Revalidate,
) -> Result<(), String>
where
    Revalidate: FnMut() -> Result<(), String>,
{
    if !transaction_root.exists() {
        return Ok(());
    }
    ensure_rebind_directory(transaction_root)?;
    for entry in fs::read_dir(transaction_root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if !file_type.is_dir() {
            return Err("会话目录修改事务目录包含意外文件。".to_string());
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(".capture-") {
            fs::remove_dir_all(entry.path())
                .map_err(|error| format!("无法清理未完成的目录修改恢复点：{error}"))?;
            continue;
        }
        let mut transaction = read_rebind_transaction(codex_home, &entry.path())?;
        revalidate()?;
        match transaction.manifest.state {
            RebindTransactionState::Prepared => {
                rollback_rebind_transaction(codex_home, &mut transaction)?;
            }
            RebindTransactionState::Committed | RebindTransactionState::RolledBack => {
                fs::remove_dir_all(&transaction.directory)
                    .map_err(|error| format!("无法清理已结束的目录修改事务：{error}"))?;
            }
        }
        revalidate()?;
    }
    Ok(())
}

fn recover_incomplete_rebind_transactions<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<(), String> {
    let transaction_root = rebind_transaction_root(app)?;
    if !transaction_root.exists()
        || fs::read_dir(&transaction_root)
            .map_err(|error| format!("无法读取会话目录修改事务：{error}"))?
            .next()
            .is_none()
    {
        return Ok(());
    }
    let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "会话目录修改恢复安全互斥不可用。".to_string())?;
    let codex_home = resolve_paths(app)?.codex_home;
    recover_rebind_transactions_with_revalidate(&codex_home, &transaction_root, || {
        lease
            .revalidate(&mut legacy_probe)
            .map_err(|_| "会话目录修改恢复期间安全互斥失效。".to_string())
    })
}

pub(crate) fn prepare_codex_thread_rebind_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    target_cwd: String,
) -> Result<ThreadRebindPreview, String> {
    recover_incomplete_rebind_transactions(&app)?;
    recover_incomplete_visibility_repair_transactions(&app)?;
    let paths = resolve_paths(&app)?;
    let prepared = prepare_thread_rebind(&paths.codex_home, &session_id, &target_cwd)?;
    let (confirm_token, expires_at) = issue_thread_rebind_plan(&prepared)?;
    let mut protected_targets = vec![
        ThreadRebindProtectedTarget::RolloutMetadata,
        ThreadRebindProtectedTarget::SessionIndexGuard,
    ];
    if prepared
        .state_rows
        .iter()
        .any(|row| row.database_relative.starts_with("sqlite/"))
    {
        protected_targets.push(ThreadRebindProtectedTarget::CanonicalState);
    }
    if prepared
        .state_rows
        .iter()
        .any(|row| !row.database_relative.starts_with("sqlite/"))
    {
        protected_targets.push(ThreadRebindProtectedTarget::LegacyState);
    }
    if !prepared.catalog_rows.is_empty() {
        protected_targets.push(ThreadRebindProtectedTarget::ThreadCatalog);
    }
    Ok(ThreadRebindPreview {
        confirm_token,
        expires_at: expires_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        session_id: prepared.session_id,
        title: prepared.title,
        old_cwd: prepared.old_cwd,
        new_cwd: prepared.target_cwd,
        rollout_file_count: prepared.files.len(),
        state_database_row_count: prepared.state_rows.len(),
        catalog_row_count: prepared.catalog_rows.len(),
        protected_targets,
        session_index_unchanged: true,
        automatic_rollback: true,
        crash_recovery: true,
    })
}

pub(crate) fn confirm_codex_thread_rebind_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<ThreadRebindReport, String> {
    let plan = consume_thread_rebind_plan(&confirm_token)?;
    let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "会话目录修改安全互斥在执行前失效，未修改任何内容。".to_string())?;
    let paths = resolve_paths(&app)?;
    let transaction_root = rebind_transaction_root(&app)?;
    recover_rebind_transactions_with_revalidate(
        &paths.codex_home,
        &transaction_root,
        || {
            lease
                .revalidate(&mut legacy_probe)
                .map_err(|_| "会话目录修改恢复期间安全互斥失效。".to_string())
        },
    )?;
    let prepared = prepare_thread_rebind(&paths.codex_home, &plan.session_id, &plan.target_cwd)?;
    if prepared.revision != plan.expected_revision {
        return Err(
            "会话内容、官方 cwd 或索引在预览后发生变化，未修改任何内容。请重新预览。"
                .to_string(),
        );
    }
    execute_thread_rebind_transaction(
        &paths.codex_home,
        &transaction_root,
        prepared,
        || {
            lease
                .revalidate(&mut legacy_probe)
                .map_err(|_| "会话目录修改安全互斥失效。".to_string())
        },
    )
}
