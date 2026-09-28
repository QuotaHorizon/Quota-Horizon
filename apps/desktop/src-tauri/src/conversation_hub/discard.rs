const THREAD_TRASH_PLAN_PREFIX: &str = "session-trash-plan:v1:";
const THREAD_TRASH_PLAN_TTL_SECONDS: i64 = 120;
const MAX_PENDING_THREAD_TRASH_PLANS: usize = 32;
const MAX_THREAD_TRASH_TARGETS: usize = 512;

#[derive(Debug, Clone)]
struct PendingThreadTrashPlan {
    session_ids: Vec<String>,
    expected_revision: String,
    expires_at: DateTime<Utc>,
}

#[derive(Default)]
struct PendingThreadTrashPlans {
    plans: HashMap<String, PendingThreadTrashPlan>,
}

impl PendingThreadTrashPlans {
    fn issue(
        &mut self,
        session_ids: Vec<String>,
        expected_revision: String,
        now: DateTime<Utc>,
    ) -> Result<(String, DateTime<Utc>), String> {
        self.plans.retain(|_, plan| plan.expires_at > now);
        if self.plans.len() >= MAX_PENDING_THREAD_TRASH_PLANS {
            return Err("待确认的会话回收操作过多，请稍后重试。".to_string());
        }
        let token = format!("{THREAD_TRASH_PLAN_PREFIX}{}", Uuid::new_v4().hyphenated());
        let expires_at = now + chrono::Duration::seconds(THREAD_TRASH_PLAN_TTL_SECONDS);
        self.plans.insert(
            token.clone(),
            PendingThreadTrashPlan {
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
    ) -> Result<PendingThreadTrashPlan, String> {
        validate_thread_trash_plan_token(token)?;
        let plan = self
            .plans
            .remove(token)
            .ok_or_else(|| "回收确认已失效，请重新预览后再执行。".to_string())?;
        if plan.expires_at <= now {
            return Err("回收确认已过期，请重新预览后再执行。".to_string());
        }
        Ok(plan)
    }
}

fn validate_thread_trash_plan_token(token: &str) -> Result<(), String> {
    let value = token
        .strip_prefix(THREAD_TRASH_PLAN_PREFIX)
        .ok_or_else(|| "回收确认令牌格式无效。".to_string())?;
    let parsed = Uuid::parse_str(value).map_err(|_| "回收确认令牌格式无效。".to_string())?;
    if parsed.hyphenated().to_string() != value {
        return Err("回收确认令牌格式无效。".to_string());
    }
    Ok(())
}

fn pending_thread_trash_plans() -> &'static std::sync::Mutex<PendingThreadTrashPlans> {
    static PLANS: std::sync::OnceLock<std::sync::Mutex<PendingThreadTrashPlans>> =
        std::sync::OnceLock::new();
    PLANS.get_or_init(|| std::sync::Mutex::new(PendingThreadTrashPlans::default()))
}

fn issue_thread_trash_plan(
    session_ids: Vec<String>,
    expected_revision: String,
) -> Result<(String, DateTime<Utc>), String> {
    pending_thread_trash_plans()
        .lock()
        .map_err(|_| "会话回收预览状态不可用。".to_string())?
        .issue(session_ids, expected_revision, Utc::now())
}

fn consume_thread_trash_plan(token: &str) -> Result<PendingThreadTrashPlan, String> {
    pending_thread_trash_plans()
        .lock()
        .map_err(|_| "会话回收预览状态不可用。".to_string())?
        .consume(token, Utc::now())
}

#[derive(Debug, Clone)]
struct PreparedThreadTrashItem {
    snapshot: RolloutSnapshot,
    index_present: bool,
    state_visibility: Option<StateVisibilitySnapshot>,
    rollout_revisions: Vec<(PathBuf, String)>,
}

#[derive(Debug, Clone)]
struct PreparedThreadTrash {
    requested_count: usize,
    items: Vec<PreparedThreadTrashItem>,
    state_db: Option<PathBuf>,
    revision: String,
}

fn digest_thread_trash_field(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn stable_rollout_revision(codex_home: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(codex_home)
        .map_err(|_| "会话文件不在受信任的 Codex 目录内。".to_string())?;
    let before = fs::symlink_metadata(path)
        .map_err(|_| "会话文件在预览期间消失，请刷新列表。".to_string())?;
    if before.file_type().is_symlink() || !before.file_type().is_file() {
        return Err("会话文件类型不安全，已拒绝移动。".to_string());
    }
    let content_sha256 =
        sha256(path).map_err(|_| "无法读取完整会话文件，已拒绝生成回收预览。".to_string())?;
    let after = fs::symlink_metadata(path)
        .map_err(|_| "会话文件在预览期间消失，请刷新列表。".to_string())?;
    let before_modified = before.modified().ok();
    let after_modified = after.modified().ok();
    if before.len() != after.len() || before_modified != after_modified {
        return Err("会话仍在写入，请稍后重新预览。".to_string());
    }
    Ok(format!(
        "{}\u{0}{}\u{0}{}",
        relative.to_string_lossy(),
        after.len(),
        content_sha256
    ))
}

fn thread_trash_revision(items: &[PreparedThreadTrashItem]) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest_thread_trash_field(&mut digest, b"session-trash-revision:v1");
    for item in items {
        let snapshot = &item.snapshot;
        digest_thread_trash_field(&mut digest, snapshot.session_id.as_bytes());
        digest_thread_trash_field(&mut digest, &[u8::from(item.index_present)]);
        digest_thread_trash_field(
            &mut digest,
            snapshot.relative_path.to_string_lossy().as_bytes(),
        );
        digest_thread_trash_field(
            &mut digest,
            &serde_json::to_vec(&snapshot.index_value)
                .map_err(|_| "无法校验会话索引。".to_string())?,
        );
        digest_thread_trash_field(
            &mut digest,
            &serde_json::to_vec(&item.state_visibility)
                .map_err(|_| "无法校验会话可见性。".to_string())?,
        );
        for (_, revision) in &item.rollout_revisions {
            digest_thread_trash_field(&mut digest, revision.as_bytes());
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn prepare_thread_trash(
    codex_home: &Path,
    session_ids: Vec<String>,
) -> Result<PreparedThreadTrash, String> {
    let requested = normalized_ids(session_ids);
    if requested.is_empty() {
        return Err("请至少选择一条会话。".to_string());
    }
    if requested.len() > MAX_THREAD_TRASH_TARGETS {
        return Err(format!(
            "单次最多可将 {MAX_THREAD_TRASH_TARGETS} 条会话移到回收站。"
        ));
    }
    let all_snapshots = gather_snapshots(codex_home)?;
    let current_index = index_values(codex_home)?;
    let state_db = latest_state_db(codex_home);
    ensure_threads_are_not_referenced(&all_snapshots, &requested, state_db.as_deref())?;
    let mut snapshots = all_snapshots
        .into_iter()
        .filter(|item| requested.contains(&item.session_id))
        .collect::<Vec<_>>();
    snapshots.sort_by(|left, right| left.session_id.cmp(&right.session_id));
    if snapshots.len() != requested.len() {
        return Err("部分所选会话已不存在，请刷新列表后重新预览。".to_string());
    }
    let mut items = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let state_visibility =
            state_visibility_snapshot(state_db.as_deref(), &snapshot.session_id)?;
        let mut physical_paths = snapshot.physical_paths.clone();
        physical_paths.sort();
        let rollout_revisions = physical_paths
            .into_iter()
            .map(|path| stable_rollout_revision(codex_home, &path).map(|revision| (path, revision)))
            .collect::<Result<Vec<_>, _>>()?;
        items.push(PreparedThreadTrashItem {
            index_present: current_index.contains_key(&snapshot.session_id),
            snapshot,
            state_visibility,
            rollout_revisions,
        });
    }
    let revision = thread_trash_revision(&items)?;
    Ok(PreparedThreadTrash {
        requested_count: requested.len(),
        items,
        state_db,
        revision,
    })
}

pub(crate) fn prepare_codex_thread_discard_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    session_ids: Vec<String>,
) -> Result<ThreadTrashPreview, String> {
    let paths = resolve_paths(&app)?;
    let prepared = prepare_thread_trash(&paths.codex_home, session_ids)?;
    let ids = prepared
        .items
        .iter()
        .map(|item| item.snapshot.session_id.clone())
        .collect::<Vec<_>>();
    let preview_items = prepared
        .items
        .iter()
        .map(|item| ThreadTrashPreviewItem {
            session_id: item.snapshot.session_id.clone(),
            title: item.snapshot.title.clone(),
            cwd: item.snapshot.cwd.clone(),
            size_bytes: item.snapshot.size_bytes,
        })
        .collect::<Vec<_>>();
    let total_size_bytes = preview_items
        .iter()
        .map(|item| item.size_bytes)
        .sum::<u64>();
    let (confirm_token, expires_at) = issue_thread_trash_plan(ids, prepared.revision)?;
    Ok(ThreadTrashPreview {
        confirm_token,
        expires_at: expires_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        requested_count: prepared.requested_count,
        affected_count: preview_items.len(),
        total_size_bytes,
        items: preview_items,
        protected_targets: vec![
            ThreadTrashProtectedTarget::RolloutFiles,
            ThreadTrashProtectedTarget::SessionIndex,
            ThreadTrashProtectedTarget::StateVisibility,
        ],
        creates_restore_point: true,
        automatic_rollback: true,
    })
}

#[derive(Debug)]
struct MovedThreadTrashItem {
    session_id: String,
    state_visibility: Option<StateVisibilitySnapshot>,
    expected_sources: Vec<PathBuf>,
    moved_files: Vec<(PathBuf, PathBuf)>,
    recycle_path: PathBuf,
}

fn restore_index_snapshot(codex_home: &Path, snapshot: Option<&str>) -> Result<(), String> {
    let path = codex_home.join(INDEX_NAME);
    match snapshot {
        Some(content) => write_text_atomic(&path, content),
        None if path.exists() => {
            fs::remove_file(&path).map_err(|error| format!("无法移除事务中新建的会话索引：{error}"))
        }
        None => Ok(()),
    }
}

fn rollback_thread_trash(
    codex_home: &Path,
    state_db: Option<&Path>,
    batch: &Path,
    moved: &[MovedThreadTrashItem],
    original_index: Option<&str>,
    index_rewritten: bool,
) -> bool {
    let mut recovered = true;
    for item in moved.iter().rev() {
        for (source, target) in item.moved_files.iter().rev() {
            if !target.exists() {
                recovered = false;
                continue;
            }
            if source.exists() {
                recovered = false;
                continue;
            }
            if source
                .parent()
                .is_some_and(|parent| fs::create_dir_all(parent).is_err())
                || fs::rename(target, source).is_err()
            {
                recovered = false;
            }
        }
        if restore_thread_visibility(state_db, &item.session_id, item.state_visibility.as_ref())
            .is_err()
        {
            recovered = false;
        }
    }
    if index_rewritten && restore_index_snapshot(codex_home, original_index).is_err() {
        recovered = false;
    }
    for item in moved {
        if item.expected_sources.iter().any(|source| !source.is_file())
            || item.moved_files.iter().any(|(_, target)| target.exists())
        {
            recovered = false;
        }
        if state_visibility_snapshot(state_db, &item.session_id)
            .ok()
            .as_ref()
            != Some(&item.state_visibility)
        {
            recovered = false;
        }
    }
    if index_rewritten {
        let restored = fs::read_to_string(codex_home.join(INDEX_NAME)).ok();
        if restored.as_deref() != original_index {
            recovered = false;
        }
    }
    if recovered {
        let _ = fs::remove_dir_all(batch);
    }
    recovered
}

fn verify_thread_trash_precondition(
    codex_home: &Path,
    state_db: Option<&Path>,
    items: &[PreparedThreadTrashItem],
) -> Result<(), String> {
    if latest_state_db(codex_home).as_deref() != state_db {
        return Err("Codex state DB 在确认期间发生切换。".to_string());
    }
    let index = index_values(codex_home)?;
    for item in items {
        let snapshot = &item.snapshot;
        let index_matches = match index.get(&snapshot.session_id) {
            Some(observed) => item.index_present && *observed == snapshot.index_value,
            None => !item.index_present,
        };
        if !index_matches
            || state_visibility_snapshot(state_db, &snapshot.session_id)? != item.state_visibility
        {
            return Err("会话索引或可见性在确认期间发生变化。".to_string());
        }
        for (path, expected_revision) in &item.rollout_revisions {
            if stable_rollout_revision(codex_home, path)? != *expected_revision {
                return Err("会话内容在确认期间发生变化。".to_string());
            }
        }
    }
    Ok(())
}

fn verify_thread_trash_postcondition(
    codex_home: &Path,
    state_db: Option<&Path>,
    moved: &[MovedThreadTrashItem],
) -> Result<(), String> {
    let index = index_values(codex_home)?;
    for item in moved {
        if index.contains_key(&item.session_id) {
            return Err("会话索引仍包含已回收会话。".to_string());
        }
        if item
            .moved_files
            .iter()
            .any(|(source, target)| source.exists() || !target.is_file())
        {
            return Err("会话文件回收结果校验失败。".to_string());
        }
        if item.state_visibility.is_some() {
            let observed = state_visibility_snapshot(state_db, &item.session_id)?
                .ok_or_else(|| "会话状态记录在回收期间消失。".to_string())?;
            if observed.archived != 1
                || !observed.preview.is_empty()
                || observed.rollout_path != item.recycle_path.to_string_lossy()
            {
                return Err("会话可见性回收结果校验失败。".to_string());
            }
        }
    }
    Ok(())
}

fn execute_thread_trash_transaction<Revalidate>(
    codex_home: &Path,
    trash_root: &Path,
    prepared: PreparedThreadTrash,
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
    let batch = trash_root.join(format!(
        "{}--{}",
        Utc::now().format("%Y%m%d-%H%M%S"),
        Uuid::new_v4().hyphenated()
    ));
    let mut moved = Vec::<MovedThreadTrashItem>::new();
    let mut index_rewritten = false;
    let result = (|| -> Result<MutationReport, String> {
        revalidate()?;
        verify_thread_trash_precondition(
            codex_home,
            prepared.state_db.as_deref(),
            &prepared.items,
        )?;
        revalidate()?;
        fs::create_dir_all(&batch).map_err(|_| "无法创建会话回收事务目录。".to_string())?;
        for item in prepared.items {
            let snapshot = item.snapshot;
            let folder = batch.join(format!(
                "{}--{}",
                safe_name(&snapshot.session_id),
                Uuid::new_v4().hyphenated()
            ));
            let manifest = BinManifest {
                session_id: snapshot.session_id.clone(),
                title: snapshot.title,
                cwd: snapshot.cwd,
                relative_rollout_path: snapshot.relative_path.to_string_lossy().to_string(),
                session_index_entry: snapshot.index_value,
                deleted_at: Utc::now().to_rfc3339(),
                state_visibility: item.state_visibility.clone(),
            };
            let manifest_text = serde_json::to_string_pretty(&manifest)
                .map_err(|_| "无法生成会话回收还原清单。".to_string())?;
            write_text_atomic(&folder.join("manifest.json"), &format!("{manifest_text}\n"))?;
            let primary_relative = snapshot
                .path
                .strip_prefix(codex_home)
                .map_err(|_| "会话文件越出受信任目录，已拒绝移动。".to_string())?;
            let recycle_path = folder.join("files").join(primary_relative);
            moved.push(MovedThreadTrashItem {
                session_id: snapshot.session_id.clone(),
                state_visibility: item.state_visibility,
                expected_sources: snapshot.physical_paths.clone(),
                moved_files: Vec::new(),
                recycle_path: recycle_path.clone(),
            });
            for source in &snapshot.physical_paths {
                let relative = source
                    .strip_prefix(codex_home)
                    .map_err(|_| "会话文件越出受信任目录，已拒绝移动。".to_string())?;
                let target = folder.join("files").join(relative);
                fs::create_dir_all(target.parent().unwrap_or(&folder))
                    .map_err(|_| "无法创建会话回收条目。".to_string())?;
                fs::rename(source, &target)
                    .map_err(|_| "无法将会话文件移入回收站。".to_string())?;
                moved
                    .last_mut()
                    .expect("trash transaction item was registered")
                    .moved_files
                    .push((source.clone(), target));
            }
            if !recycle_path.is_file() {
                return Err("无法定位回收后的会话文件。".to_string());
            }
            hide_thread_in_state(
                prepared.state_db.as_deref(),
                &snapshot.session_id,
                &recycle_path,
            )?;
            revalidate()?;
        }
        let removed = moved
            .iter()
            .map(|item| item.session_id.clone())
            .collect::<HashSet<_>>();
        index_rewritten = true;
        rewrite_index(codex_home, &removed)?;
        revalidate()?;
        verify_thread_trash_postcondition(codex_home, prepared.state_db.as_deref(), &moved)?;
        Ok(MutationReport {
            requested_count: prepared.requested_count,
            affected_count: moved.len(),
            released_bytes: 0,
            message: format!("已将 {} 条会话安全移到回收站", moved.len()),
        })
    })();

    match result {
        Ok(report) => Ok(report),
        Err(error) => {
            let mutation_started =
                index_rewritten || moved.iter().any(|item| !item.moved_files.is_empty());
            if !mutation_started {
                let _ = fs::remove_dir_all(&batch);
                return Err(format!("{error} 未移动任何会话，请重新预览后再试。"));
            }
            let recovered = rollback_thread_trash(
                codex_home,
                prepared.state_db.as_deref(),
                &batch,
                &moved,
                original_index.as_deref(),
                index_rewritten,
            );
            if recovered {
                Err(format!("{error} 原会话状态已自动恢复，请重新预览后再试。"))
            } else {
                Err(concat!(
                    "会话回收操作未能完成，且无法确认自动恢复结果。",
                    "请暂时不要继续操作回收站，并保留 QuotaHorizon 应用数据以便恢复。"
                )
                .to_string())
            }
        }
    }
}

pub(crate) fn confirm_codex_thread_discard_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<MutationReport, String> {
    let plan = consume_thread_trash_plan(&confirm_token)?;
    let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "会话回收安全互斥在执行前失效，未修改任何会话。".to_string())?;
    let paths = resolve_paths(&app)?;
    let prepared = prepare_thread_trash(&paths.codex_home, plan.session_ids)?;
    if prepared.revision != plan.expected_revision {
        return Err("会话内容或索引在预览后发生变化，未移动任何会话。请重新预览。".to_string());
    }
    let trash_root = bin_root(&app)?;
    execute_thread_trash_transaction(&paths.codex_home, &trash_root, prepared, || {
        lease
            .revalidate(&mut legacy_probe)
            .map_err(|_| "会话回收安全互斥失效。".to_string())
    })
}

fn read_stable_bin_manifest(path: &Path) -> Result<(BinManifest, String), String> {
    let before = fs::symlink_metadata(path)
        .map_err(|error| format!("无法读取回收站清单 {}：{error}", path.display()))?;
    if before.file_type().is_symlink() || !before.file_type().is_file() {
        return Err(format!("回收站清单文件类型不安全：{}", path.display()));
    }
    let bytes = fs::read(path)
        .map_err(|error| format!("无法读取回收站清单 {}：{error}", path.display()))?;
    let after = fs::symlink_metadata(path)
        .map_err(|error| format!("无法复核回收站清单 {}：{error}", path.display()))?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(format!("回收站清单仍在变化：{}", path.display()));
    }
    let manifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("回收站清单损坏 {}：{error}", path.display()))?;
    let revision = format!("{}:{:x}", after.len(), Sha256::digest(&bytes));
    Ok((manifest, revision))
}

fn collect_bin_entries_from_root(root: &Path) -> Result<Vec<BinSnapshot>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("无法验证会话回收站：{error}"))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.file_type().is_dir() {
        return Err("会话回收站目录类型不安全。".to_string());
    }
    let mut result = Vec::new();
    for batch in fs::read_dir(root).map_err(|error| format!("无法读取会话回收站：{error}"))?
    {
        let batch = batch.map_err(|error| error.to_string())?;
        if !batch
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        let batch = batch.path();
        for entry in fs::read_dir(&batch).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                continue;
            }
            let folder = entry.path();
            let manifest_path = folder.join("manifest.json");
            if !manifest_path.is_file() {
                continue;
            }
            let (manifest, manifest_revision) = read_stable_bin_manifest(&manifest_path)?;
            let files_root = folder.join("files");
            let mut rollouts = Vec::new();
            collect_physical_rollouts(&files_root, &mut rollouts)?;
            if !rollouts.is_empty() {
                result.push(BinSnapshot {
                    folder,
                    manifest,
                    manifest_revision,
                    rollouts,
                });
            }
        }
    }
    Ok(result)
}

fn collect_bin_entries<R: Runtime>(app: &tauri::AppHandle<R>) -> Result<Vec<BinSnapshot>, String> {
    collect_bin_entries_from_root(&bin_root(app)?)
}

fn collect_physical_rollouts(root: &Path, result: &mut Vec<PathBuf>) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            collect_physical_rollouts(&path, result)?;
        } else if logical_rollout_path(&path).is_some() {
            result.push(path);
        }
    }
    result.sort();
    Ok(())
}

fn directory_size(path: &Path) -> u64 {
    if path.is_file() {
        return fs::metadata(path).map(|value| value.len()).unwrap_or(0);
    }
    fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| directory_size(&entry.path()))
        .sum()
}

fn latest_versioned_db(codex_home: &Path, prefix: &str) -> Option<PathBuf> {
    fs::read_dir(codex_home)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let version = name
                .strip_prefix(prefix)?
                .strip_suffix(".sqlite")?
                .parse::<u64>()
                .ok()?;
            Some((version, entry.path()))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, path)| path)
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            params![table],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

fn delete_thread_rows(
    path: Option<PathBuf>,
    session_id: &str,
    operations: &[(&str, &str)],
) -> Result<(), String> {
    let Some(path) = path else {
        return Ok(());
    };
    let mut connection = Connection::open(&path)
        .map_err(|error| format!("无法打开 Codex 数据库 {}：{error}", path.display()))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    for (table, sql) in operations {
        if table_exists(&transaction, table)? {
            transaction
                .execute(sql, params![session_id])
                .map_err(|error| format!("无法清理 Codex 数据库 {}：{error}", path.display()))?;
        }
    }
    transaction.commit().map_err(|error| error.to_string())
}

fn purge_thread_catalogs(codex_home: &Path, session_id: &str) -> Result<(), String> {
    let catalog_dir = codex_home.join("sqlite");
    if !catalog_dir.exists() {
        return Ok(());
    }
    let catalog_metadata = fs::symlink_metadata(&catalog_dir)
        .map_err(|error| format!("无法验证 Codex catalog 目录：{error}"))?;
    if catalog_metadata.file_type().is_symlink() || !catalog_metadata.file_type().is_dir() {
        return Err("Codex catalog 目录类型不安全。".to_string());
    }
    for entry in fs::read_dir(&catalog_dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        let path = entry.path();
        if file_type.is_symlink() {
            return Err(format!(
                "Codex catalog 包含不安全的 symlink：{}",
                path.display()
            ));
        }
        if !file_type.is_file() || path.extension().and_then(|value| value.to_str()) != Some("db") {
            continue;
        }
        delete_thread_rows(
            Some(path),
            session_id,
            &[(
                "local_thread_catalog",
                "DELETE FROM local_thread_catalog WHERE thread_id = ?1",
            )],
        )?;
    }
    Ok(())
}

fn purge_thread_state(codex_home: &Path, session_id: &str) -> Result<(), String> {
    delete_thread_rows(
        latest_state_db(codex_home),
        session_id,
        &[
            ("thread_dynamic_tools", "DELETE FROM thread_dynamic_tools WHERE thread_id = ?1"),
            (
                "thread_spawn_edges",
                "DELETE FROM thread_spawn_edges WHERE child_thread_id = ?1 OR parent_thread_id = ?1",
            ),
            (
                "thread_goal_continuation_deferrals",
                "DELETE FROM thread_goal_continuation_deferrals WHERE thread_id = ?1",
            ),
            ("thread_goals", "DELETE FROM thread_goals WHERE thread_id = ?1"),
            ("stage1_outputs", "DELETE FROM stage1_outputs WHERE thread_id = ?1"),
            ("logs", "DELETE FROM logs WHERE thread_id = ?1"),
            ("threads", "DELETE FROM threads WHERE id = ?1"),
        ],
    )?;
    delete_thread_rows(
        latest_versioned_db(codex_home, "thread_history_"),
        session_id,
        &[
            (
                "thread_items",
                "DELETE FROM thread_items WHERE thread_id = ?1",
            ),
            (
                "thread_turns",
                "DELETE FROM thread_turns WHERE thread_id = ?1",
            ),
            (
                "thread_history_projection_state",
                "DELETE FROM thread_history_projection_state WHERE thread_id = ?1",
            ),
        ],
    )?;
    delete_thread_rows(
        latest_versioned_db(codex_home, "queue_"),
        session_id,
        &[(
            "queued_items",
            "DELETE FROM queued_items WHERE thread_id = ?1",
        )],
    )?;
    delete_thread_rows(
        latest_versioned_db(codex_home, "goals_"),
        session_id,
        &[
            (
                "thread_goal_continuation_deferrals",
                "DELETE FROM thread_goal_continuation_deferrals WHERE thread_id = ?1",
            ),
            (
                "thread_goals",
                "DELETE FROM thread_goals WHERE thread_id = ?1",
            ),
        ],
    )?;
    delete_thread_rows(
        latest_versioned_db(codex_home, "memories_"),
        session_id,
        &[(
            "stage1_outputs",
            "DELETE FROM stage1_outputs WHERE thread_id = ?1",
        )],
    )?;
    delete_thread_rows(
        latest_versioned_db(codex_home, "logs_"),
        session_id,
        &[("logs", "DELETE FROM logs WHERE thread_id = ?1")],
    )?;
    purge_thread_catalogs(codex_home, session_id)
}
