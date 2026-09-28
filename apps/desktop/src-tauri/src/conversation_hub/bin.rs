const THREAD_RESTORE_PLAN_PREFIX: &str = "session-restore-plan:v1:";
const THREAD_RESTORE_PLAN_TTL_SECONDS: i64 = 120;
const MAX_PENDING_THREAD_RESTORE_PLANS: usize = 32;
const MAX_THREAD_RESTORE_TARGETS: usize = 512;

#[derive(Debug, Clone)]
struct PendingThreadRestorePlan {
    session_ids: Vec<String>,
    expected_revision: String,
    expires_at: DateTime<Utc>,
}

#[derive(Default)]
struct PendingThreadRestorePlans {
    plans: HashMap<String, PendingThreadRestorePlan>,
}

impl PendingThreadRestorePlans {
    fn issue(
        &mut self,
        session_ids: Vec<String>,
        expected_revision: String,
        now: DateTime<Utc>,
    ) -> Result<(String, DateTime<Utc>), String> {
        self.plans.retain(|_, plan| plan.expires_at > now);
        if self.plans.len() >= MAX_PENDING_THREAD_RESTORE_PLANS {
            return Err("待确认的会话恢复操作过多，请稍后重试。".to_string());
        }
        let token = format!(
            "{THREAD_RESTORE_PLAN_PREFIX}{}",
            Uuid::new_v4().hyphenated()
        );
        let expires_at = now + chrono::Duration::seconds(THREAD_RESTORE_PLAN_TTL_SECONDS);
        self.plans.insert(
            token.clone(),
            PendingThreadRestorePlan {
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
    ) -> Result<PendingThreadRestorePlan, String> {
        validate_thread_restore_plan_token(token)?;
        let plan = self
            .plans
            .remove(token)
            .ok_or_else(|| "恢复确认已失效，请重新预览后再执行。".to_string())?;
        if plan.expires_at <= now {
            return Err("恢复确认已过期，请重新预览后再执行。".to_string());
        }
        Ok(plan)
    }
}

fn validate_thread_restore_plan_token(token: &str) -> Result<(), String> {
    let value = token
        .strip_prefix(THREAD_RESTORE_PLAN_PREFIX)
        .ok_or_else(|| "恢复确认令牌格式无效。".to_string())?;
    let parsed = Uuid::parse_str(value).map_err(|_| "恢复确认令牌格式无效。".to_string())?;
    if parsed.hyphenated().to_string() != value {
        return Err("恢复确认令牌格式无效。".to_string());
    }
    Ok(())
}

fn pending_thread_restore_plans() -> &'static std::sync::Mutex<PendingThreadRestorePlans> {
    static PLANS: std::sync::OnceLock<std::sync::Mutex<PendingThreadRestorePlans>> =
        std::sync::OnceLock::new();
    PLANS.get_or_init(|| std::sync::Mutex::new(PendingThreadRestorePlans::default()))
}

fn issue_thread_restore_plan(
    session_ids: Vec<String>,
    expected_revision: String,
) -> Result<(String, DateTime<Utc>), String> {
    pending_thread_restore_plans()
        .lock()
        .map_err(|_| "会话恢复预览状态不可用。".to_string())?
        .issue(session_ids, expected_revision, Utc::now())
}

fn consume_thread_restore_plan(token: &str) -> Result<PendingThreadRestorePlan, String> {
    pending_thread_restore_plans()
        .lock()
        .map_err(|_| "会话恢复预览状态不可用。".to_string())?
        .consume(token, Utc::now())
}

#[derive(Debug, Clone)]
struct PreparedThreadRestoreFile {
    source: PathBuf,
    target: PathBuf,
    relative_path: PathBuf,
    content_revision: String,
}

#[derive(Debug, Clone)]
struct PreparedThreadRestoreItem {
    bin: BinSnapshot,
    current_visibility: Option<StateVisibilitySnapshot>,
    files: Vec<PreparedThreadRestoreFile>,
}

#[derive(Debug, Clone)]
struct PreparedThreadRestore {
    requested_count: usize,
    items: Vec<PreparedThreadRestoreItem>,
    state_db: Option<PathBuf>,
    revision: String,
}

fn stable_bin_rollout_revision(path: &Path) -> Result<String, String> {
    let before = fs::symlink_metadata(path)
        .map_err(|_| "回收站中的会话文件已消失，请刷新列表。".to_string())?;
    if before.file_type().is_symlink() || !before.file_type().is_file() {
        return Err("回收站中的会话文件类型不安全，已拒绝恢复。".to_string());
    }
    let content_sha256 =
        sha256(path).map_err(|_| "无法读取完整回收站会话文件，已拒绝恢复。".to_string())?;
    let after = fs::symlink_metadata(path)
        .map_err(|_| "回收站中的会话文件已消失，请刷新列表。".to_string())?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err("回收站中的会话文件仍在变化，请稍后重试。".to_string());
    }
    Ok(format!("{}\u{0}{}", after.len(), content_sha256))
}

fn ensure_restore_target_parent_is_safe(codex_home: &Path, target: &Path) -> Result<(), String> {
    let relative = target
        .strip_prefix(codex_home)
        .map_err(|_| "恢复目标越出受信任的 Codex 目录。".to_string())?;
    let mut current = codex_home.to_path_buf();
    for component in relative.parent().into_iter().flat_map(Path::components) {
        let std::path::Component::Normal(value) = component else {
            return Err("恢复目标路径不安全。".to_string());
        };
        current.push(value);
        if let Ok(metadata) = fs::symlink_metadata(&current) {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err(format!("恢复目标目录不安全：{}", current.display()));
            }
        }
    }
    Ok(())
}

fn restore_revision(
    trash_root: &Path,
    state_db: Option<&Path>,
    items: &[PreparedThreadRestoreItem],
) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest_thread_trash_field(&mut digest, b"session-restore-revision:v1");
    digest_thread_trash_field(
        &mut digest,
        state_db
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default()
            .as_bytes(),
    );
    for item in items {
        digest_thread_trash_field(&mut digest, item.bin.manifest.session_id.as_bytes());
        let relative_folder = item
            .bin
            .folder
            .strip_prefix(trash_root)
            .map_err(|_| "回收站条目越出受信任目录。".to_string())?;
        digest_thread_trash_field(&mut digest, relative_folder.to_string_lossy().as_bytes());
        digest_thread_trash_field(&mut digest, item.bin.manifest_revision.as_bytes());
        digest_thread_trash_field(
            &mut digest,
            &serde_json::to_vec(&item.current_visibility)
                .map_err(|_| "无法校验当前会话状态。".to_string())?,
        );
        for file in &item.files {
            digest_thread_trash_field(&mut digest, file.relative_path.to_string_lossy().as_bytes());
            digest_thread_trash_field(&mut digest, file.content_revision.as_bytes());
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn prepare_thread_restore(
    codex_home: &Path,
    trash_root: &Path,
    session_ids: Vec<String>,
) -> Result<PreparedThreadRestore, String> {
    let requested = normalized_ids(session_ids);
    if requested.is_empty() {
        return Err("请至少选择一条待恢复会话。".to_string());
    }
    if requested.len() > MAX_THREAD_RESTORE_TARGETS {
        return Err(format!(
            "单次最多可恢复 {MAX_THREAD_RESTORE_TARGETS} 条会话。"
        ));
    }
    let entries = collect_bin_entries_from_root(trash_root)?;
    let current_index = index_values(codex_home)?;
    let state_db = latest_state_db(codex_home);
    let mut ids = requested.iter().cloned().collect::<Vec<_>>();
    ids.sort();
    let mut targets = HashSet::<PathBuf>::new();
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        let mut matches = entries
            .iter()
            .filter(|entry| entry.manifest.session_id == id);
        let bin = matches
            .next()
            .cloned()
            .ok_or_else(|| format!("回收站中的会话 {id} 已不存在，请刷新列表。"))?;
        if matches.next().is_some() {
            return Err(format!("回收站中存在多个会话 {id} 条目，请先人工核查。"));
        }
        if current_index.contains_key(&id) {
            return Err(format!("当前会话索引已包含 {id}，为避免覆盖已拒绝恢复。"));
        }
        if bin
            .manifest
            .session_index_entry
            .get("id")
            .and_then(Value::as_str)
            != Some(id.as_str())
        {
            return Err(format!("会话 {id} 的回收站索引清单不一致。"));
        }
        let files_root = bin.folder.join("files");
        let mut files = Vec::with_capacity(bin.rollouts.len());
        for source in &bin.rollouts {
            let relative = source
                .strip_prefix(&files_root)
                .map_err(|_| "回收站文件越出受信任条目目录。".to_string())?
                .to_path_buf();
            let relative_text = relative
                .to_str()
                .ok_or_else(|| "回收站文件路径不是有效文本。".to_string())?;
            if safe_relative_path(relative_text).as_ref() != Some(&relative) {
                return Err("回收站文件路径不安全，已拒绝恢复。".to_string());
            }
            let target = codex_home.join(&relative);
            ensure_restore_target_parent_is_safe(codex_home, &target)?;
            if fs::symlink_metadata(&target).is_ok() {
                return Err(format!(
                    "恢复目标已存在，为避免覆盖已拒绝恢复：{}",
                    target.display()
                ));
            }
            if !targets.insert(target.clone()) {
                return Err("多个回收站条目指向同一恢复目标，已拒绝恢复。".to_string());
            }
            files.push(PreparedThreadRestoreFile {
                source: source.clone(),
                target,
                relative_path: relative,
                content_revision: stable_bin_rollout_revision(source)?,
            });
        }
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let primary_relative = safe_relative_path(&bin.manifest.relative_rollout_path)
            .ok_or_else(|| format!("会话 {id} 的主文件路径不安全。"))?;
        let primary = files
            .iter()
            .find(|file| file.relative_path == primary_relative)
            .ok_or_else(|| format!("会话 {id} 的主文件与回收清单不一致。"))?;
        let current_visibility = state_visibility_snapshot(state_db.as_deref(), &id)?;
        match (&bin.manifest.state_visibility, &current_visibility) {
            (Some(_), Some(current))
                if current.archived == 1
                    && current.preview.is_empty()
                    && Path::new(&current.rollout_path) == primary.source.as_path() => {}
            (None, None) => {}
            _ => {
                return Err(format!(
                    "会话 {id} 的当前可见性状态与回收清单不一致，已拒绝恢复。"
                ));
            }
        }
        items.push(PreparedThreadRestoreItem {
            bin,
            current_visibility,
            files,
        });
    }
    let revision = restore_revision(trash_root, state_db.as_deref(), &items)?;
    Ok(PreparedThreadRestore {
        requested_count: requested.len(),
        items,
        state_db,
        revision,
    })
}

pub(crate) fn browse_codex_thread_bin_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Vec<BinEntry>, String> {
    recover_incomplete_purge_transactions(&app)?;
    let mut grouped = HashMap::<String, BinEntry>::new();
    for item in collect_bin_entries(&app)? {
        let deleted_at = DateTime::parse_from_rfc3339(&item.manifest.deleted_at)
            .ok()
            .map(|value| value.timestamp());
        let size = item
            .rollouts
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .sum();
        let entry = grouped
            .entry(item.manifest.session_id.clone())
            .or_insert(BinEntry {
                session_id: item.manifest.session_id,
                title: item.manifest.title,
                cwd: item.manifest.cwd,
                deleted_at,
                size_bytes: 0,
            });
        entry.size_bytes = entry.size_bytes.saturating_add(size);
        if deleted_at.unwrap_or_default() > entry.deleted_at.unwrap_or_default() {
            entry.deleted_at = deleted_at;
        }
    }
    let mut result = grouped.into_values().collect::<Vec<_>>();
    result.sort_by(|a, b| {
        b.deleted_at
            .unwrap_or_default()
            .cmp(&a.deleted_at.unwrap_or_default())
    });
    Ok(result)
}

fn append_index_entries(
    codex_home: &Path,
    expected_content: Option<&str>,
    additions: &[(String, Value)],
) -> Result<(), String> {
    let path = codex_home.join(INDEX_NAME);
    let current = path
        .exists()
        .then(|| {
            fs::read_to_string(&path)
                .map_err(|error| format!("无法读取会话索引 {}：{error}", path.display()))
        })
        .transpose()?;
    if current.as_deref() != expected_content {
        return Err("会话索引在恢复事务期间发生变化。".to_string());
    }
    let existing = index_values(codex_home)?;
    if additions.iter().any(|(id, _)| existing.contains_key(id)) {
        return Err("恢复目标会话已出现在当前索引中。".to_string());
    }
    let mut output = current.unwrap_or_default();
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    for (_, entry) in additions {
        output.push_str(&serde_json::to_string(entry).map_err(|error| error.to_string())?);
        output.push('\n');
    }
    write_text_atomic(&path, &output)
}

fn append_index_entry(codex_home: &Path, session_id: &str, entry: &Value) -> Result<(), String> {
    let mut entries = index_values(codex_home)?;
    entries.insert(session_id.to_string(), entry.clone());
    let mut values = entries.into_values().collect::<Vec<_>>();
    values.sort_by(|a, b| {
        index_timestamp(Some(b))
            .unwrap_or_default()
            .cmp(&index_timestamp(Some(a)).unwrap_or_default())
    });
    let mut output = String::new();
    for value in values {
        output.push_str(&serde_json::to_string(&value).map_err(|error| error.to_string())?);
        output.push('\n');
    }
    write_text_atomic(&codex_home.join(INDEX_NAME), &output)
}

fn verify_thread_restore_precondition(
    codex_home: &Path,
    state_db: Option<&Path>,
    items: &[PreparedThreadRestoreItem],
) -> Result<(), String> {
    if latest_state_db(codex_home).as_deref() != state_db {
        return Err("Codex state DB 在恢复确认期间发生切换。".to_string());
    }
    let index = index_values(codex_home)?;
    for item in items {
        let id = &item.bin.manifest.session_id;
        if index.contains_key(id)
            || state_visibility_snapshot(state_db, id)? != item.current_visibility
        {
            return Err("会话索引或可见性在恢复确认期间发生变化。".to_string());
        }
        let (_, manifest_revision) =
            read_stable_bin_manifest(&item.bin.folder.join("manifest.json"))?;
        if manifest_revision != item.bin.manifest_revision {
            return Err("回收站清单在恢复确认期间发生变化。".to_string());
        }
        for file in &item.files {
            ensure_restore_target_parent_is_safe(codex_home, &file.target)?;
            if fs::symlink_metadata(&file.target).is_ok() {
                return Err("恢复目标在确认期间已被占用。".to_string());
            }
            if stable_bin_rollout_revision(&file.source)? != file.content_revision {
                return Err("回收站会话内容在恢复确认期间发生变化。".to_string());
            }
        }
    }
    Ok(())
}

fn relocate_file_without_replace(
    source: &Path,
    target: &Path,
    expected_revision: &str,
) -> Result<(), String> {
    if stable_bin_rollout_revision(source)? != expected_revision {
        return Err("待移动会话文件在操作前发生变化。".to_string());
    }
    let parent = target
        .parent()
        .ok_or_else(|| "无法定位会话恢复目录。".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建会话恢复目录：{error}"))?;
    if fs::symlink_metadata(target).is_ok() {
        return Err("恢复目标已存在，未覆盖任何文件。".to_string());
    }
    if fs::hard_link(source, target).is_ok() {
        if stable_bin_rollout_revision(target)? != expected_revision
            || stable_bin_rollout_revision(source)? != expected_revision
        {
            let _ = fs::remove_file(target);
            return Err("会话文件在移动期间发生变化。".to_string());
        }
        if let Err(error) = fs::remove_file(source) {
            let _ = fs::remove_file(target);
            return Err(format!("无法完成会话文件移动：{error}"));
        }
        return Ok(());
    }
    let mut input = File::open(source).map_err(|error| format!("无法读取会话文件：{error}"))?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| format!("无法创建无覆盖恢复目标：{error}"))?;
    let copy_result = (|| -> Result<(), String> {
        std::io::copy(&mut input, &mut output)
            .map_err(|error| format!("无法复制会话文件：{error}"))?;
        output
            .sync_all()
            .map_err(|error| format!("无法同步会话文件：{error}"))?;
        if stable_bin_rollout_revision(target)? != expected_revision
            || stable_bin_rollout_revision(source)? != expected_revision
        {
            return Err("会话文件在复制期间发生变化。".to_string());
        }
        fs::remove_file(source).map_err(|error| format!("无法移除原会话文件：{error}"))
    })();
    if let Err(error) = copy_result {
        drop(output);
        let _ = fs::remove_file(target);
        return Err(error);
    }
    Ok(())
}

#[derive(Debug)]
struct MovedThreadRestoreItem {
    session_id: String,
    original_visibility: Option<StateVisibilitySnapshot>,
    restored_visibility: Option<StateVisibilitySnapshot>,
    index_entry: Value,
    bin_folder: PathBuf,
    files: Vec<PreparedThreadRestoreFile>,
    moved_count: usize,
    visibility_changed: bool,
}

fn rollback_thread_restore(
    codex_home: &Path,
    state_db: Option<&Path>,
    moved: &[MovedThreadRestoreItem],
    original_index: Option<&str>,
    index_rewritten: bool,
) -> bool {
    let mut recovered = true;
    for item in moved.iter().rev() {
        if item.visibility_changed
            && restore_thread_visibility(
                state_db,
                &item.session_id,
                item.original_visibility.as_ref(),
            )
            .is_err()
        {
            recovered = false;
        }
        for file in item.files[..item.moved_count].iter().rev() {
            if !file.target.is_file()
                || file.source.exists()
                || relocate_file_without_replace(&file.target, &file.source, &file.content_revision)
                    .is_err()
            {
                recovered = false;
            }
        }
    }
    if index_rewritten && restore_index_snapshot(codex_home, original_index).is_err() {
        recovered = false;
    }
    for item in moved {
        if state_visibility_snapshot(state_db, &item.session_id)
            .ok()
            .as_ref()
            != Some(&item.original_visibility)
        {
            recovered = false;
        }
        for file in &item.files {
            if !file.source.is_file()
                || file.target.exists()
                || stable_bin_rollout_revision(&file.source).ok().as_deref()
                    != Some(file.content_revision.as_str())
            {
                recovered = false;
            }
        }
    }
    if index_rewritten {
        let restored = fs::read_to_string(codex_home.join(INDEX_NAME)).ok();
        if restored.as_deref() != original_index {
            recovered = false;
        }
    }
    recovered
}

fn verify_thread_restore_postcondition(
    codex_home: &Path,
    state_db: Option<&Path>,
    moved: &[MovedThreadRestoreItem],
) -> Result<(), String> {
    let index = index_values(codex_home)?;
    for item in moved {
        if index.get(&item.session_id) != Some(&item.index_entry) {
            return Err("会话索引恢复结果校验失败。".to_string());
        }
        if state_visibility_snapshot(state_db, &item.session_id)? != item.restored_visibility {
            return Err("会话可见性恢复结果校验失败。".to_string());
        }
        for file in &item.files {
            if file.source.exists()
                || !file.target.is_file()
                || stable_bin_rollout_revision(&file.target)? != file.content_revision
            {
                return Err("会话文件恢复结果校验失败。".to_string());
            }
        }
    }
    Ok(())
}

fn execute_thread_restore_transaction<Revalidate>(
    codex_home: &Path,
    prepared: PreparedThreadRestore,
    mut revalidate: Revalidate,
) -> Result<MutationReport, String>
where
    Revalidate: FnMut() -> Result<(), String>,
{
    let original_index = {
        let path = codex_home.join(INDEX_NAME);
        path.exists()
            .then(|| {
                fs::read_to_string(&path).map_err(|_| "无法读取会话索引事务快照。".to_string())
            })
            .transpose()?
    };
    let mut moved = Vec::<MovedThreadRestoreItem>::new();
    let mut index_rewritten = false;
    let result = (|| -> Result<MutationReport, String> {
        revalidate()?;
        verify_thread_restore_precondition(
            codex_home,
            prepared.state_db.as_deref(),
            &prepared.items,
        )?;
        revalidate()?;
        for item in prepared.items {
            let manifest = item.bin.manifest;
            moved.push(MovedThreadRestoreItem {
                session_id: manifest.session_id.clone(),
                original_visibility: item.current_visibility,
                restored_visibility: manifest.state_visibility.clone(),
                index_entry: manifest.session_index_entry,
                bin_folder: item.bin.folder,
                files: item.files,
                moved_count: 0,
                visibility_changed: false,
            });
            let moved_item = moved
                .last_mut()
                .expect("restore transaction item was registered");
            for file in &moved_item.files {
                ensure_restore_target_parent_is_safe(codex_home, &file.target)?;
                relocate_file_without_replace(&file.source, &file.target, &file.content_revision)?;
                moved_item.moved_count += 1;
            }
            restore_thread_visibility(
                prepared.state_db.as_deref(),
                &moved_item.session_id,
                moved_item.restored_visibility.as_ref(),
            )?;
            moved_item.visibility_changed = moved_item.restored_visibility.is_some();
            revalidate()?;
        }
        let additions = moved
            .iter()
            .map(|item| (item.session_id.clone(), item.index_entry.clone()))
            .collect::<Vec<_>>();
        append_index_entries(codex_home, original_index.as_deref(), &additions)?;
        index_rewritten = true;
        revalidate()?;
        verify_thread_restore_postcondition(codex_home, prepared.state_db.as_deref(), &moved)?;
        revalidate()?;
        for item in &moved {
            let _ = fs::remove_dir_all(&item.bin_folder);
        }
        Ok(MutationReport {
            requested_count: prepared.requested_count,
            affected_count: moved.len(),
            released_bytes: 0,
            message: format!("已安全恢复 {} 条会话", moved.len()),
        })
    })();

    match result {
        Ok(report) => Ok(report),
        Err(error) => {
            let mutation_started = index_rewritten
                || moved
                    .iter()
                    .any(|item| item.moved_count > 0 || item.visibility_changed);
            if !mutation_started {
                return Err(format!("{error} 未恢复任何会话，请重新预览后再试。"));
            }
            let recovered = rollback_thread_restore(
                codex_home,
                prepared.state_db.as_deref(),
                &moved,
                original_index.as_deref(),
                index_rewritten,
            );
            if recovered {
                Err(format!(
                    "{error} 回收站与原会话状态已自动还原，请重新预览后再试。"
                ))
            } else {
                Err(concat!(
                    "会话恢复操作未能完成，且无法确认自动还原结果。",
                    "请暂时不要继续操作回收站，并保留 QuotaHorizon 应用数据以便恢复。"
                )
                .to_string())
            }
        }
    }
}

pub(crate) fn prepare_codex_thread_restore_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    session_ids: Vec<String>,
) -> Result<ThreadRestorePreview, String> {
    let paths = resolve_paths(&app)?;
    let trash_root = bin_root(&app)?;
    let prepared = prepare_thread_restore(&paths.codex_home, &trash_root, session_ids)?;
    let ids = prepared
        .items
        .iter()
        .map(|item| item.bin.manifest.session_id.clone())
        .collect::<Vec<_>>();
    let items = prepared
        .items
        .iter()
        .map(|item| ThreadRestorePreviewItem {
            session_id: item.bin.manifest.session_id.clone(),
            title: item.bin.manifest.title.clone(),
            cwd: item.bin.manifest.cwd.clone(),
            size_bytes: item
                .files
                .iter()
                .filter_map(|file| fs::metadata(&file.source).ok())
                .map(|metadata| metadata.len())
                .sum(),
        })
        .collect::<Vec<_>>();
    let total_size_bytes = items.iter().map(|item| item.size_bytes).sum();
    let (confirm_token, expires_at) = issue_thread_restore_plan(ids, prepared.revision)?;
    Ok(ThreadRestorePreview {
        confirm_token,
        expires_at: expires_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        requested_count: prepared.requested_count,
        affected_count: items.len(),
        total_size_bytes,
        items,
        protected_targets: vec![
            ThreadRestoreProtectedTarget::RolloutFiles,
            ThreadRestoreProtectedTarget::SessionIndex,
            ThreadRestoreProtectedTarget::StateVisibility,
        ],
        conflicts_checked: true,
        automatic_rollback: true,
    })
}

pub(crate) fn confirm_codex_thread_restore_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<MutationReport, String> {
    let plan = consume_thread_restore_plan(&confirm_token)?;
    let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "会话恢复安全互斥在执行前失效，未修改任何会话。".to_string())?;
    let paths = resolve_paths(&app)?;
    let trash_root = bin_root(&app)?;
    let prepared = prepare_thread_restore(&paths.codex_home, &trash_root, plan.session_ids)?;
    if prepared.revision != plan.expected_revision {
        return Err(
            "回收站内容或恢复目标在预览后发生变化，未恢复任何会话。请重新预览。".to_string(),
        );
    }
    execute_thread_restore_transaction(&paths.codex_home, prepared, || {
        lease
            .revalidate(&mut legacy_probe)
            .map_err(|_| "会话恢复安全互斥失效。".to_string())
    })
}

fn sha256(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
