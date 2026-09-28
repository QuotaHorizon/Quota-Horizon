const SAFE_MUTATION_PLAN_PREFIX: &str = "safe-mutation-plan:v1:";
const SAFE_MUTATION_PLAN_TTL_SECONDS: i64 = 120;
const MAX_PENDING_SAFE_MUTATION_PLANS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SafeAccountSwitchMode {
    ManagerOnly,
    LocalProxy,
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
enum SafeSwitchProtectedTarget {
    CurrentAuth,
    CurrentConfig,
    ManagerState,
    ProviderConfigBackup,
    ProviderModelCatalog,
    AggregateApiStore,
    ActiveProviderProfile,
    ActiveProviderFieldMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
enum SafeProviderMutationKind {
    Enter,
    Exit,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeAccountSwitchPreview {
    confirm_token: String,
    expires_at: String,
    mode: SafeAccountSwitchMode,
    protected_targets: Vec<SafeSwitchProtectedTarget>,
    will_restart_client: bool,
    creates_restore_point: bool,
    automatic_rollback: bool,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeAccountDeactivatePreview {
    confirm_token: String,
    expires_at: String,
    mode: SafeAccountSwitchMode,
    protected_targets: Vec<SafeSwitchProtectedTarget>,
    will_restart_client: bool,
    creates_restore_point: bool,
    automatic_rollback: bool,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeProviderMutationPreview {
    confirm_token: String,
    expires_at: String,
    kind: SafeProviderMutationKind,
    target_label: String,
    mode: SafeAccountSwitchMode,
    protected_targets: Vec<SafeSwitchProtectedTarget>,
    will_restart_client: bool,
    creates_restore_point: bool,
    automatic_rollback: bool,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeProviderEditPreview {
    confirm_token: String,
    expires_at: String,
    target_label: String,
    mode: SafeAccountSwitchMode,
    protected_targets: Vec<SafeSwitchProtectedTarget>,
    will_restart_client: bool,
    creates_restore_point: bool,
    automatic_rollback: bool,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeRollbackPreview {
    confirm_token: String,
    expires_at: String,
    protected_targets: Vec<SafeSwitchProtectedTarget>,
    will_restart_client: bool,
    verifies_current_state: bool,
}

#[derive(Clone)]
enum PendingSafeMutationAction {
    SwitchAccount {
        target_account_id: String,
        expected_mode: SafeAccountSwitchMode,
    },
    DeactivateAccount {
        expected_account_id: String,
        expected_mode: SafeAccountSwitchMode,
    },
    MutateProvider {
        target: crate::providers::ProviderMutationTarget,
        expected_current: crate::providers::ProviderRuntimeSelection,
        expected_mode: SafeAccountSwitchMode,
        expected_target_revision: String,
    },
    EditActiveProvider {
        updated_profile: Box<crate::models::ProviderProfile>,
        expected_mode: SafeAccountSwitchMode,
        expected_revision: String,
    },
    EditActiveAggregate {
        updated_config: Box<crate::aggregate_api::AggregateApiConfig>,
        expected_mode: SafeAccountSwitchMode,
        expected_revision: String,
    },
    RollbackLastChange {
        restore_point_id: capacity_mutation::RestorePointId,
    },
}

#[derive(Clone)]
struct PendingSafeMutationPlan {
    action: PendingSafeMutationAction,
    expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Default)]
struct PendingSafeMutationPlans {
    plans: std::collections::HashMap<String, PendingSafeMutationPlan>,
}

impl PendingSafeMutationPlans {
    fn issue(
        &mut self,
        action: PendingSafeMutationAction,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(String, chrono::DateTime<chrono::Utc>), String> {
        self.plans.retain(|_, plan| plan.expires_at > now);
        if self.plans.len() >= MAX_PENDING_SAFE_MUTATION_PLANS {
            return Err("待确认的安全更改过多，请稍后重试。".to_string());
        }
        let token = format!(
            "{SAFE_MUTATION_PLAN_PREFIX}{}",
            uuid::Uuid::new_v4().hyphenated()
        );
        let expires_at = now + chrono::Duration::seconds(SAFE_MUTATION_PLAN_TTL_SECONDS);
        self.plans.insert(
            token.clone(),
            PendingSafeMutationPlan { action, expires_at },
        );
        Ok((token, expires_at))
    }

    fn consume(
        &mut self,
        token: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<PendingSafeMutationAction, String> {
        validate_safe_mutation_plan_token(token)?;
        let plan = self
            .plans
            .remove(token)
            .ok_or_else(|| "确认已失效，请重新预览后再执行。".to_string())?;
        if plan.expires_at <= now {
            return Err("确认已过期，请重新预览后再执行。".to_string());
        }
        Ok(plan.action)
    }
}

fn validate_safe_mutation_plan_token(token: &str) -> Result<(), String> {
    let value = token
        .strip_prefix(SAFE_MUTATION_PLAN_PREFIX)
        .ok_or_else(|| "确认令牌格式无效。".to_string())?;
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| "确认令牌格式无效。".to_string())?;
    if parsed.hyphenated().to_string() != value {
        return Err("确认令牌格式无效。".to_string());
    }
    Ok(())
}

fn pending_safe_mutation_plans() -> &'static std::sync::Mutex<PendingSafeMutationPlans> {
    static PLANS: std::sync::OnceLock<std::sync::Mutex<PendingSafeMutationPlans>> =
        std::sync::OnceLock::new();
    PLANS.get_or_init(|| std::sync::Mutex::new(PendingSafeMutationPlans::default()))
}

fn issue_safe_mutation_plan(
    action: PendingSafeMutationAction,
) -> Result<(String, chrono::DateTime<chrono::Utc>), String> {
    pending_safe_mutation_plans()
        .lock()
        .map_err(|_| "安全更改预览状态不可用。".to_string())?
        .issue(action, chrono::Utc::now())
}

fn consume_safe_mutation_plan(token: &str) -> Result<PendingSafeMutationAction, String> {
    pending_safe_mutation_plans()
        .lock()
        .map_err(|_| "安全更改预览状态不可用。".to_string())?
        .consume(token, chrono::Utc::now())
}

fn safe_switch_mode(context: &AccountSwitchContext) -> SafeAccountSwitchMode {
    if !context.write_codex {
        SafeAccountSwitchMode::ManagerOnly
    } else if context.proxy_running {
        SafeAccountSwitchMode::LocalProxy
    } else {
        SafeAccountSwitchMode::Direct
    }
}

fn safe_switch_preview_targets() -> Vec<SafeSwitchProtectedTarget> {
    vec![
        SafeSwitchProtectedTarget::CurrentAuth,
        SafeSwitchProtectedTarget::CurrentConfig,
        SafeSwitchProtectedTarget::ManagerState,
        SafeSwitchProtectedTarget::ProviderConfigBackup,
    ]
}

fn safe_provider_preview_targets() -> Vec<SafeSwitchProtectedTarget> {
    let mut targets = safe_switch_preview_targets();
    targets.push(SafeSwitchProtectedTarget::ProviderModelCatalog);
    targets
}

fn safe_active_provider_edit_preview_targets() -> Vec<SafeSwitchProtectedTarget> {
    let mut targets = safe_provider_preview_targets();
    targets.push(SafeSwitchProtectedTarget::ActiveProviderProfile);
    targets.push(SafeSwitchProtectedTarget::ActiveProviderFieldMetadata);
    targets
}

fn safe_active_aggregate_edit_preview_targets() -> Vec<SafeSwitchProtectedTarget> {
    let mut targets = safe_provider_preview_targets();
    targets.push(SafeSwitchProtectedTarget::AggregateApiStore);
    targets
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SafeSwitchFailureStage {
    Admission,
    RestorePoint,
    Apply,
    Postflight,
    Commit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SafeSwitchTransactionError {
    stage: SafeSwitchFailureStage,
    recovery_required: bool,
}

impl SafeSwitchTransactionError {
    fn before_mutation(stage: SafeSwitchFailureStage) -> Self {
        Self {
            stage,
            recovery_required: false,
        }
    }

    fn after_restore_attempt(stage: SafeSwitchFailureStage, recovered: bool) -> Self {
        Self {
            stage,
            recovery_required: !recovered,
        }
    }

    fn user_message(self) -> String {
        if self.recovery_required {
            return concat!(
                "安全更改未能完成，且无法确认自动恢复结果。请先退出 ChatGPT/Codex，",
                "不要再次切换，并使用“恢复上一次更改”处理。"
            )
            .to_string();
        }
        let stage = match self.stage {
            SafeSwitchFailureStage::Admission => "安全互斥复核",
            SafeSwitchFailureStage::RestorePoint => "创建还原点",
            SafeSwitchFailureStage::Apply => "写入本地状态",
            SafeSwitchFailureStage::Postflight => "切换结果校验",
            SafeSwitchFailureStage::Commit => "提交还原点",
        };
        format!("安全更改在{stage}阶段失败，原状态已保留或恢复。请重试。")
    }
}

fn safe_switch_timestamp() -> Result<capacity_domain::UtcTimestamp, String> {
    capacity_domain::UtcTimestamp::parse(
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    )
    .map_err(|_| "无法生成账户切换时间戳".to_string())
}

fn safe_switch_targets(
    paths: &Paths,
) -> Result<Vec<capacity_mutation::ProtectedTargetPath>, String> {
    use capacity_mutation::{ProtectedTarget, ProtectedTargetPath};

    [
        (ProtectedTarget::CurrentAuth, paths.current_auth.clone()),
        (
            ProtectedTarget::CurrentConfig,
            paths.current_config.clone(),
        ),
        (ProtectedTarget::ManagerState, paths.state_file.clone()),
        (
            ProtectedTarget::ProviderConfigBackup,
            paths.config_backup.clone(),
        ),
    ]
    .into_iter()
    .map(|(target, path)| {
        ProtectedTargetPath::new(target, path)
            .map_err(|_| "账户切换保护目标无效".to_string())
    })
    .collect()
}

fn safe_provider_targets(
    paths: &Paths,
) -> Result<Vec<capacity_mutation::ProtectedTargetPath>, String> {
    let mut targets = safe_switch_targets(paths)?;
    targets.push(
        capacity_mutation::ProtectedTargetPath::new(
            capacity_mutation::ProtectedTarget::ProviderModelCatalog,
            crate::providers::provider_model_catalog_path(paths),
        )
        .map_err(|_| "Provider model catalog 保护目标无效".to_string())?,
    );
    Ok(targets)
}

fn safe_active_provider_edit_targets(
    paths: &Paths,
    provider_id: &str,
) -> Result<Vec<capacity_mutation::ProtectedTargetPath>, String> {
    let mut targets = safe_provider_targets(paths)?;
    targets.push(
        capacity_mutation::ProtectedTargetPath::new(
            capacity_mutation::ProtectedTarget::ActiveProviderProfile,
            crate::providers::provider_path(paths, provider_id),
        )
        .map_err(|_| "Provider profile 保护目标无效".to_string())?,
    );
    targets.push(
        capacity_mutation::ProtectedTargetPath::new(
            capacity_mutation::ProtectedTarget::ActiveProviderFieldMetadata,
            crate::providers::provider_field_modified_at_path(paths, provider_id),
        )
        .map_err(|_| "Provider field metadata 保护目标无效".to_string())?,
    );
    Ok(targets)
}

fn safe_active_aggregate_edit_targets(
    paths: &Paths,
) -> Result<Vec<capacity_mutation::ProtectedTargetPath>, String> {
    let mut targets = safe_provider_targets(paths)?;
    targets.push(
        capacity_mutation::ProtectedTargetPath::new(
            capacity_mutation::ProtectedTarget::AggregateApiStore,
            crate::aggregate_api::store_path(paths)?,
        )
        .map_err(|_| "Aggregate API store 保护目标无效".to_string())?,
    );
    Ok(targets)
}

fn safe_switch_restore_store(paths: &Paths) -> Result<capacity_mutation::RestorePointStore, String> {
    let app_data = paths
        .state_file
        .parent()
        .ok_or_else(|| "无法定位账户切换还原点目录".to_string())?;
    capacity_mutation::RestorePointStore::new(app_data.join("restore-points-v1"))
        .map_err(|_| "无法初始化账户切换还原点".to_string())
}

pub(crate) fn acquire_shared_mutation_lease() -> Result<
    (
        capacity_mutation::MutationLock,
        capacity_mutation::SystemLegacyViewerProbe,
    ),
    String,
> {
    use capacity_mutation::{
        MutationLock, MutationLockError, MutationLockOwner, MutationOwnerId,
        SystemLegacyViewerProbe, macos_shared_mutation_lock_root,
    };

    let home = dirs::home_dir().ok_or_else(|| "无法定位当前用户目录".to_string())?;
    let root = macos_shared_mutation_lock_root(home)
        .map_err(|_| "无法初始化共享本地变更互斥".to_string())?;
    let owner_id = MutationOwnerId::parse(format!(
        "mutation-owner:v1:{}",
        uuid::Uuid::new_v4().hyphenated()
    ))
    .map_err(|_| "无法创建本地变更操作标识".to_string())?;
    let owner = MutationLockOwner::new(
        owner_id,
        std::process::id(),
        safe_switch_timestamp()?,
    )
    .map_err(|_| "无法创建本地变更互斥所有者".to_string())?;
    let mut probe = SystemLegacyViewerProbe;
    let lease = MutationLock::acquire(root, owner, &mut probe).map_err(|error| match error {
        MutationLockError::LegacyViewerRunning => concat!(
            "旧版 CodexQuotaViewer 仍在运行。为避免两个应用同时改写本地状态，",
            "请先完全退出旧版应用后再重试。"
        )
        .to_string(),
        MutationLockError::Busy => "另一个本地安全更改正在进行，请稍后重试。".to_string(),
        MutationLockError::LegacyViewerStateUnavailable => {
            "无法确认旧版 CodexQuotaViewer 是否已退出，已拒绝修改本地状态。".to_string()
        }
        _ => "无法取得本地安全更改互斥，未修改任何本地文件。".to_string(),
    })?;
    Ok((lease, probe))
}

fn rollback_prepared_safe_switch<P: capacity_mutation::LegacyViewerProbe>(
    lease: &capacity_mutation::MutationLock,
    probe: &mut P,
    store: &capacity_mutation::RestorePointStore,
    restore_point_id: &capacity_mutation::RestorePointId,
    targets: &[capacity_mutation::ProtectedTargetPath],
) -> bool {
    // A legacy Viewer that appears after acquisition cannot own the SQLite
    // lease. Continue only while this process can still prove that lease.
    if let Err(error) = lease.revalidate(probe) {
        if !matches!(
            error,
            capacity_mutation::MutationLockError::LegacyViewerRunning
        ) {
            return false;
        }
    }
    let Ok(current) = store.observe(targets) else {
        return false;
    };
    let Ok(rolled_back_at) = safe_switch_timestamp() else {
        return false;
    };
    store
        .rollback_prepared(restore_point_id, targets, &current, &rolled_back_at)
        .is_ok()
}

#[derive(Clone, Copy)]
struct SafeSwitchTransactionMetadata {
    operation: capacity_mutation::RestorePointOperation,
    codex_was_running: bool,
}

fn execute_safe_switch_transaction<P, Apply, Verify>(
    lease: &capacity_mutation::MutationLock,
    probe: &mut P,
    store: &capacity_mutation::RestorePointStore,
    targets: &[capacity_mutation::ProtectedTargetPath],
    metadata: SafeSwitchTransactionMetadata,
    apply: Apply,
    verify: Verify,
) -> Result<capacity_mutation::RestorePointId, SafeSwitchTransactionError>
where
    P: capacity_mutation::LegacyViewerProbe,
    Apply: FnOnce() -> Result<(), String>,
    Verify: FnOnce() -> Result<(), String>,
{
    lease.revalidate(probe).map_err(|_| {
        SafeSwitchTransactionError::before_mutation(SafeSwitchFailureStage::Admission)
    })?;
    let created_at = safe_switch_timestamp().map_err(|_| {
        SafeSwitchTransactionError::before_mutation(SafeSwitchFailureStage::RestorePoint)
    })?;
    let point = store
        .create_restore_point(
            metadata.operation,
            targets,
            metadata.codex_was_running,
            &created_at,
            &std::collections::BTreeSet::new(),
        )
        .map_err(|_| {
            SafeSwitchTransactionError::before_mutation(SafeSwitchFailureStage::RestorePoint)
        })?;
    let restore_point_id = point.id().clone();

    let run_stage = || -> Result<(), SafeSwitchFailureStage> {
        lease
            .revalidate(probe)
            .map_err(|_| SafeSwitchFailureStage::Admission)?;
        apply().map_err(|_| SafeSwitchFailureStage::Apply)?;
        lease
            .revalidate(probe)
            .map_err(|_| SafeSwitchFailureStage::Admission)?;
        verify().map_err(|_| SafeSwitchFailureStage::Postflight)?;
        lease
            .revalidate(probe)
            .map_err(|_| SafeSwitchFailureStage::Admission)?;
        Ok(())
    };

    if let Err(stage) = run_stage() {
        let recovered =
            rollback_prepared_safe_switch(lease, probe, store, &restore_point_id, targets);
        return Err(SafeSwitchTransactionError::after_restore_attempt(
            stage, recovered,
        ));
    }

    let committed_at = safe_switch_timestamp().map_err(|_| {
        let recovered =
            rollback_prepared_safe_switch(lease, probe, store, &restore_point_id, targets);
        SafeSwitchTransactionError::after_restore_attempt(
            SafeSwitchFailureStage::Commit,
            recovered,
        )
    })?;
    if store
        .mark_committed(&restore_point_id, targets, &committed_at)
        .is_err()
    {
        let recovered =
            rollback_prepared_safe_switch(lease, probe, store, &restore_point_id, targets);
        return Err(SafeSwitchTransactionError::after_restore_attempt(
            SafeSwitchFailureStage::Commit,
            recovered,
        ));
    }
    Ok(restore_point_id)
}

fn verify_account_switch_postcondition(
    id: &str,
    context: &AccountSwitchContext,
) -> Result<(), String> {
    let state = read_state(&context.paths);
    if state.active_account_id.as_deref() != Some(id)
        || state.active_provider_id.is_some()
        || state.active_provider_group.is_some()
        || state.concurrent_account_routing_enabled
    {
        return Err("manager state does not name the selected official account".to_string());
    }
    if !context.write_codex {
        return Ok(());
    }

    if context.proxy_running {
        let config = fs::read_to_string(&context.paths.current_config)
            .map_err(|_| "local proxy config is unavailable".to_string())?;
        if !crate::codex_config::contains_local_proxy(&config) {
            return Err("local proxy config was not activated".to_string());
        }
        match state.local_proxy_openai_auth_account_id.as_deref() {
            Some(proxy_account_id) => {
                let auth = read_json(&context.paths.current_auth)
                    .map_err(|_| "local proxy auth is unavailable".to_string())?;
                let (_, _, _, actual_id) = account_fields(&auth)?;
                if actual_id != proxy_account_id {
                    return Err("local proxy auth does not match its configured login".to_string());
                }
            }
            None if context.paths.current_auth.exists() => {
                return Err("local proxy auth should be absent".to_string());
            }
            None => {}
        }
        return Ok(());
    }

    let auth = read_json(&context.paths.current_auth)
        .map_err(|_| "current auth is unavailable after switch".to_string())?;
    validate_auth(&auth)?;
    let (_, _, _, actual_id) = account_fields(&auth)?;
    if actual_id != id {
        return Err("current auth does not match the selected account".to_string());
    }
    if context.paths.current_config.exists() {
        let config = fs::read_to_string(&context.paths.current_config)
            .map_err(|_| "current config is unavailable after switch".to_string())?;
        if crate::codex_config::contains_local_proxy(&config) {
            return Err("current config still contains the local proxy".to_string());
        }
    }
    if context.paths.config_backup.exists() {
        return Err("provider config backup still exists after official switch".to_string());
    }
    Ok(())
}

fn restart_client_after_safe_switch(
    launch_target: Option<&ChatGptLaunchTarget>,
) -> Result<(), String> {
    if crate::codex_runtime::restart_managed_session()? {
        Ok(())
    } else {
        start_chatgpt(launch_target)
    }
}

#[tauri::command]
pub(crate) async fn prepare_safe_account_switch<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    id: String,
) -> Result<SafeAccountSwitchPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let context = prepare_account_switch_context(&app, &id, true)?;
        let mode = safe_switch_mode(&context);
        let (confirm_token, expires_at) = issue_safe_mutation_plan(
            PendingSafeMutationAction::SwitchAccount {
                target_account_id: id,
                expected_mode: mode,
            },
        )?;
        Ok(SafeAccountSwitchPreview {
            confirm_token,
            expires_at: expires_at
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            mode,
            protected_targets: safe_switch_preview_targets(),
            will_restart_client: mode == SafeAccountSwitchMode::Direct,
            creates_restore_point: true,
            automatic_rollback: true,
        })
    })
    .await
    .map_err(|error| format!("Account switch preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn confirm_safe_account_switch<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _switch_guard = account_switch_lock()
            .lock()
            .map_err(|_| "Account switch lock is poisoned".to_string())?;
        let plan = consume_safe_mutation_plan(&confirm_token)?;
        let PendingSafeMutationAction::SwitchAccount {
            target_account_id,
            expected_mode,
        } = plan
        else {
            return Err("确认令牌不属于账户切换。请重新预览。".to_string());
        };
        refresh_local_codex_path(&app);
        perform_safe_account_switch_with_expected_mode(
            &app,
            &target_account_id,
            true,
            true,
            Some(expected_mode),
        )
    })
    .await
    .map_err(|error| format!("Account switch confirmation task failed: {error}"))?
}

#[derive(Clone)]
struct AccountDeactivateContext {
    proxy_running: bool,
    write_codex: bool,
    paths: Paths,
    original_state: ManagerStateFile,
    active_account_id: String,
    refreshed_auth: Option<Value>,
}

fn prepare_account_deactivate_context<R: Runtime>(
    app: &tauri::AppHandle<R>,
    expected_account_id: Option<&str>,
) -> Result<AccountDeactivateContext, String> {
    let proxy_running = crate::local_proxy::is_running();
    let paths = resolve_paths(app)?;
    let mut original_state = read_state(&paths);
    let active_account_id = original_state
        .active_account_id
        .clone()
        .ok_or_else(|| "当前没有启用的官方账户。".to_string())?;
    if expected_account_id.is_some_and(|expected| expected != active_account_id) {
        return Err("当前账户在预览后发生变化，未修改任何文件。请重新预览。".to_string());
    }
    if original_state.active_provider_id.is_some()
        || original_state.active_provider_group.is_some()
    {
        return Err("账户与 Provider 状态不一致，已拒绝停用。".to_string());
    }
    crate::conversation_hub::mark_threads_before_account_switch(
        &paths,
        &mut original_state,
        Some(&active_account_id),
    )?;
    let refreshed_auth = capture_refreshed_auth_candidate(&paths, &active_account_id);
    let write_codex = crate::claude_code::should_write_codex_for_app(app)?;
    Ok(AccountDeactivateContext {
        proxy_running,
        write_codex,
        paths,
        original_state,
        active_account_id,
        refreshed_auth,
    })
}

fn account_deactivate_mode(context: &AccountDeactivateContext) -> SafeAccountSwitchMode {
    if !context.write_codex {
        SafeAccountSwitchMode::ManagerOnly
    } else if context.proxy_running {
        SafeAccountSwitchMode::LocalProxy
    } else {
        SafeAccountSwitchMode::Direct
    }
}

fn apply_account_deactivate_files<R: Runtime>(
    app: &tauri::AppHandle<R>,
    context: &AccountDeactivateContext,
) -> Result<(), String> {
    let mut state = context.original_state.clone();
    state.active_account_id = None;
    state.concurrent_account_routing_enabled = false;
    capacity_desktop_service::invalidate_account_binding(app);
    write_state(&context.paths, &state)?;

    if !context.write_codex {
        return Ok(());
    }
    if context.proxy_running {
        crate::providers::sync_local_proxy_openai_auth(&context.paths)
    } else {
        match fs::remove_file(&context.paths.current_auth) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("无法移除当前 Codex 认证文件。".to_string()),
        }
    }
}

fn verify_account_deactivate_postcondition(
    context: &AccountDeactivateContext,
) -> Result<(), String> {
    let state = read_state(&context.paths);
    if state.active_account_id.is_some()
        || state.active_provider_id.is_some()
        || state.active_provider_group.is_some()
        || state.concurrent_account_routing_enabled
    {
        return Err("manager state still has an active route".to_string());
    }
    if !context.write_codex {
        return Ok(());
    }
    if !context.proxy_running {
        return (!context.paths.current_auth.exists())
            .then_some(())
            .ok_or_else(|| "current auth still exists after deactivation".to_string());
    }
    let config = fs::read_to_string(&context.paths.current_config)
        .map_err(|_| "local proxy config is unavailable".to_string())?;
    if !crate::codex_config::contains_local_proxy(&config) {
        return Err("local proxy config is not active".to_string());
    }
    match state.local_proxy_openai_auth_account_id.as_deref() {
        Some(proxy_account_id) => {
            let auth = read_json(&context.paths.current_auth)
                .map_err(|_| "local proxy auth is unavailable".to_string())?;
            let (_, _, _, actual_id) = account_fields(&auth)?;
            (actual_id == proxy_account_id)
                .then_some(())
                .ok_or_else(|| "local proxy auth does not match its configured login".to_string())
        }
        None if context.paths.current_auth.exists() => {
            Err("local proxy auth should be absent".to_string())
        }
        None => Ok(()),
    }
}

fn finalize_account_deactivation<R: Runtime>(
    app: &tauri::AppHandle<R>,
    context: &AccountDeactivateContext,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Some(auth) = &context.refreshed_auth {
        if let Err(error) = write_managed_auth_if_newer(
            &context.paths,
            &context.active_account_id,
            auth,
        ) {
            warnings.push(format!("auth refresh preservation failed: {error}"));
        }
    }
    if let Err(error) = touch_account_field(
        &context.paths,
        &context.active_account_id,
        AccountSyncField::Active,
    ) {
        warnings.push(format!("account activity metadata update failed: {error}"));
    }
    if let Err(error) = app.emit("accounts-changed", ()) {
        warnings.push(format!("accounts event failed: {error}"));
    }
    if let Err(error) = app.emit("providers-changed", ()) {
        warnings.push(format!("providers event failed: {error}"));
    }
    if context.proxy_running && context.write_codex {
        crate::providers::refresh_official_codex_models();
    }
    if let Err(error) = crate::claude_code::sync_after_switch(app) {
        warnings.push(format!("third-party app sync failed: {error}"));
    }
    crate::system_tray::refresh_menu(app);
    warnings
}

#[tauri::command]
pub(crate) async fn prepare_safe_account_deactivation<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
) -> Result<SafeAccountDeactivatePreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let context = prepare_account_deactivate_context(&app, None)?;
        let mode = account_deactivate_mode(&context);
        let will_restart_client =
            mode == SafeAccountSwitchMode::Direct && chatgpt_or_codex_is_running()?;
        let (confirm_token, expires_at) = issue_safe_mutation_plan(
            PendingSafeMutationAction::DeactivateAccount {
                expected_account_id: context.active_account_id,
                expected_mode: mode,
            },
        )?;
        Ok(SafeAccountDeactivatePreview {
            confirm_token,
            expires_at: expires_at
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            mode,
            protected_targets: safe_switch_preview_targets(),
            will_restart_client,
            creates_restore_point: true,
            automatic_rollback: true,
        })
    })
    .await
    .map_err(|error| format!("Account deactivation preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn confirm_safe_account_deactivation<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _switch_guard = account_switch_lock()
            .lock()
            .map_err(|_| "Account switch lock is poisoned".to_string())?;
        let plan = consume_safe_mutation_plan(&confirm_token)?;
        let PendingSafeMutationAction::DeactivateAccount {
            expected_account_id,
            expected_mode,
        } = plan
        else {
            return Err("确认令牌不属于账户停用。请重新预览。".to_string());
        };
        refresh_local_codex_path(&app);
        perform_safe_account_deactivation(&app, &expected_account_id, expected_mode)
    })
    .await
    .map_err(|error| format!("Account deactivation confirmation task failed: {error}"))?
}

fn perform_safe_account_deactivation<R: Runtime>(
    app: &tauri::AppHandle<R>,
    expected_account_id: &str,
    expected_mode: SafeAccountSwitchMode,
) -> Result<(), String> {
    let initial_context = prepare_account_deactivate_context(app, Some(expected_account_id))?;
    if account_deactivate_mode(&initial_context) != expected_mode {
        return Err("账户停用模式在预览后发生变化，未修改任何文件。请重新预览。".to_string());
    }
    let (lease, mut legacy_probe) = acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "账户停用安全互斥在执行前失效，未修改任何文件。".to_string())?;

    let controls_runtime = initial_context.write_codex && !initial_context.proxy_running;
    let client_was_running = controls_runtime && chatgpt_or_codex_is_running()?;
    let launch_target = client_was_running
        .then(|| refresh_and_get_chatgpt_launch_target(app))
        .flatten();
    if client_was_running {
        stop_chatgpt_processes()?;
        if let Err(error) = wait_for_chatgpt_processes_to_exit(Duration::from_secs(10)) {
            let _ = restart_client_after_safe_switch(launch_target.as_ref());
            return Err(error);
        }
    }

    let context = match prepare_account_deactivate_context(app, Some(expected_account_id)) {
        Ok(context) if account_deactivate_mode(&context) == expected_mode => context,
        Ok(_) | Err(_) => {
            if client_was_running {
                let _ = restart_client_after_safe_switch(launch_target.as_ref());
            }
            return Err("账户停用条件在客户端停止后发生变化，未修改文件。请重试。".to_string());
        }
    };
    let targets = safe_switch_targets(&context.paths)?;
    let store = safe_switch_restore_store(&context.paths)?;
    let transaction = execute_safe_switch_transaction(
        &lease,
        &mut legacy_probe,
        &store,
        &targets,
        SafeSwitchTransactionMetadata {
            operation: capacity_mutation::RestorePointOperation::DeactivateAccount,
            codex_was_running: client_was_running,
        },
        || apply_account_deactivate_files(app, &context),
        || verify_account_deactivate_postcondition(&context),
    );
    if let Err(error) = transaction {
        let message = error.user_message();
        if client_was_running
            && restart_client_after_safe_switch(launch_target.as_ref()).is_err()
        {
            return Err(format!(
                "{message} ChatGPT/Codex 也未能自动重新启动，请手动启动。"
            ));
        }
        return Err(message);
    }
    for warning in finalize_account_deactivation(app, &context) {
        eprintln!("safe account deactivation post-commit warning: {warning}");
    }
    if client_was_running {
        restart_client_after_safe_switch(launch_target.as_ref()).map_err(|_| {
            concat!(
                "账户已安全停用，但无法自动启动 ChatGPT/Codex。",
                "请手动启动 ChatGPT 或 Codex。"
            )
            .to_string()
        })?;
    }
    Ok(())
}

fn provider_mutation_mode(
    context: &crate::providers::ProviderMutationContext,
) -> SafeAccountSwitchMode {
    if !context.write_codex() {
        SafeAccountSwitchMode::ManagerOnly
    } else if context.proxy_running() {
        SafeAccountSwitchMode::LocalProxy
    } else {
        SafeAccountSwitchMode::Direct
    }
}

fn provider_target_from_selection(
    selection: &crate::providers::ProviderRuntimeSelection,
) -> crate::providers::ProviderMutationTarget {
    match selection {
        crate::providers::ProviderRuntimeSelection::Provider(id) => {
            crate::providers::ProviderMutationTarget::Provider(id.clone())
        }
        crate::providers::ProviderRuntimeSelection::Group(group) => {
            crate::providers::ProviderMutationTarget::Group(group.clone())
        }
        crate::providers::ProviderRuntimeSelection::Aggregate(id) => {
            crate::providers::ProviderMutationTarget::Aggregate(id.clone())
        }
        crate::providers::ProviderRuntimeSelection::Official => {
            crate::providers::ProviderMutationTarget::Official
        }
    }
}

fn prepare_safe_provider_mutation<R: Runtime>(
    app: &tauri::AppHandle<R>,
    target: crate::providers::ProviderMutationTarget,
) -> Result<SafeProviderMutationPreview, String> {
    let context = crate::providers::prepare_provider_mutation_context(app, &target, None)?;
    let target = provider_target_from_selection(context.target());
    let mode = provider_mutation_mode(&context);
    let kind = if *context.target() == crate::providers::ProviderRuntimeSelection::Official {
        SafeProviderMutationKind::Exit
    } else {
        SafeProviderMutationKind::Enter
    };
    let will_restart_client =
        mode == SafeAccountSwitchMode::Direct && chatgpt_or_codex_is_running()?;
    let (confirm_token, expires_at) =
        issue_safe_mutation_plan(PendingSafeMutationAction::MutateProvider {
            target,
            expected_current: context.current().clone(),
            expected_mode: mode,
            expected_target_revision: context.target_revision().to_string(),
        })?;
    Ok(SafeProviderMutationPreview {
        confirm_token,
        expires_at: expires_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        kind,
        target_label: context.target_label().to_string(),
        mode,
        protected_targets: safe_provider_preview_targets(),
        will_restart_client,
        creates_restore_point: true,
        automatic_rollback: true,
    })
}

#[tauri::command]
pub(crate) async fn prepare_safe_provider_switch<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    id: String,
) -> Result<SafeProviderMutationPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_safe_provider_mutation(
            &app,
            crate::providers::ProviderMutationTarget::Provider(id),
        )
    })
    .await
    .map_err(|error| format!("Provider switch preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn prepare_safe_provider_group_switch<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    group: String,
) -> Result<SafeProviderMutationPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_safe_provider_mutation(
            &app,
            crate::providers::ProviderMutationTarget::Group(group),
        )
    })
    .await
    .map_err(|error| format!("Provider group preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn prepare_safe_aggregate_switch<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    id: String,
) -> Result<SafeProviderMutationPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_safe_provider_mutation(
            &app,
            crate::providers::ProviderMutationTarget::Aggregate(id),
        )
    })
    .await
    .map_err(|error| format!("Aggregate API switch preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn prepare_safe_provider_exit<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
) -> Result<SafeProviderMutationPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_safe_provider_mutation(
            &app,
            crate::providers::ProviderMutationTarget::Official,
        )
    })
    .await
    .map_err(|error| format!("Provider exit preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn confirm_safe_provider_mutation<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let plan = consume_safe_mutation_plan(&confirm_token)?;
        let PendingSafeMutationAction::MutateProvider {
            target,
            expected_current,
            expected_mode,
            expected_target_revision,
        } = plan
        else {
            return Err("确认令牌不属于 Provider 更改。请重新预览。".to_string());
        };
        perform_safe_provider_mutation_blocking(
            app,
            target,
            Some(expected_current),
            Some(expected_mode),
            Some(expected_target_revision),
        )
    })
    .await
    .map_err(|error| format!("Provider mutation confirmation task failed: {error}"))?
}

pub(crate) fn perform_safe_provider_mutation_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    target: crate::providers::ProviderMutationTarget,
    expected_current: Option<crate::providers::ProviderRuntimeSelection>,
    expected_mode: Option<SafeAccountSwitchMode>,
    expected_target_revision: Option<String>,
) -> Result<(), String> {
    let _switch_guard = account_switch_lock()
        .lock()
        .map_err(|_| "Account and Provider switch lock is poisoned".to_string())?;
    refresh_local_codex_path(&app);
    let initial_context = crate::providers::prepare_provider_mutation_context(
        &app,
        &target,
        expected_current.as_ref(),
    )?;
    let initial_mode = provider_mutation_mode(&initial_context);
    if expected_mode.is_some_and(|expected| expected != initial_mode) {
        return Err("Provider 更改模式在预览后发生变化，未修改任何文件。请重新预览。".to_string());
    }
    if expected_target_revision
        .as_deref()
        .is_some_and(|expected| expected != initial_context.target_revision())
    {
        return Err("Provider 配置在预览后发生变化，未修改任何文件。请重新预览。".to_string());
    }
    let (lease, mut legacy_probe) = acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "Provider 安全互斥在执行前失效，未修改任何文件。".to_string())?;

    let controls_runtime = initial_context.write_codex() && !initial_context.proxy_running();
    let client_was_running = controls_runtime && chatgpt_or_codex_is_running()?;
    let launch_target = client_was_running
        .then(|| refresh_and_get_chatgpt_launch_target(&app))
        .flatten();
    if client_was_running {
        stop_chatgpt_processes()?;
        if let Err(error) = wait_for_chatgpt_processes_to_exit(Duration::from_secs(10)) {
            let _ = restart_client_after_safe_switch(launch_target.as_ref());
            return Err(error);
        }
    }

    let context = match crate::providers::prepare_provider_mutation_context(
        &app,
        &target,
        expected_current.as_ref(),
    ) {
        Ok(context)
            if context.target() == initial_context.target()
                && provider_mutation_mode(&context) == initial_mode
                && context.target_revision() == initial_context.target_revision() =>
        {
            context
        }
        Ok(_) | Err(_) => {
            if client_was_running {
                let _ = restart_client_after_safe_switch(launch_target.as_ref());
            }
            return Err("Provider 条件在客户端停止后发生变化，未修改文件。请重试。".to_string());
        }
    };
    let targets = safe_provider_targets(context.paths())?;
    let store = safe_switch_restore_store(context.paths())?;
    let operation = if *context.target() == crate::providers::ProviderRuntimeSelection::Official {
        capacity_mutation::RestorePointOperation::ExitProviderMode
    } else {
        capacity_mutation::RestorePointOperation::EnterProviderMode
    };
    let transaction = execute_safe_switch_transaction(
        &lease,
        &mut legacy_probe,
        &store,
        &targets,
        SafeSwitchTransactionMetadata {
            operation,
            codex_was_running: client_was_running,
        },
        || crate::providers::apply_provider_mutation_files(&app, &context),
        || crate::providers::verify_provider_mutation_postcondition(&context),
    );
    if let Err(error) = transaction {
        let message = error.user_message();
        if client_was_running
            && restart_client_after_safe_switch(launch_target.as_ref()).is_err()
        {
            return Err(format!(
                "{message} ChatGPT/Codex 也未能自动重新启动，请手动启动。"
            ));
        }
        return Err(message);
    }
    for warning in crate::providers::finalize_provider_mutation(&app, &context) {
        eprintln!("safe Provider mutation post-commit warning: {warning}");
    }
    if client_was_running {
        restart_client_after_safe_switch(launch_target.as_ref()).map_err(|_| {
            concat!(
                "Provider 更改已安全提交，但无法自动启动 ChatGPT/Codex。",
                "请手动启动 ChatGPT 或 Codex。"
            )
            .to_string()
        })?;
    }
    Ok(())
}

fn active_provider_edit_mode(
    context: &crate::providers::ActiveProviderEditContext,
) -> SafeAccountSwitchMode {
    if !context.write_codex() {
        SafeAccountSwitchMode::ManagerOnly
    } else if context.proxy_running() {
        SafeAccountSwitchMode::LocalProxy
    } else {
        SafeAccountSwitchMode::Direct
    }
}

fn prepare_safe_active_provider_edit_plan<R: Runtime>(
    app: &tauri::AppHandle<R>,
    paths: &Paths,
    prepared: crate::providers::PreparedProviderSave,
) -> Result<Option<SafeProviderEditPreview>, String> {
    let Some(existing) = prepared.existing.as_ref() else {
        return Ok(None);
    };
    let state = read_state(paths);
    let affects_active_group = state
        .active_provider_group
        .as_deref()
        .is_some_and(|group| existing.group == group || prepared.profile.group == group);
    let affects_active_aggregate = crate::aggregate_api::active_aggregate_contains_provider(
        paths,
        &prepared.profile.id,
    )?;
    let is_active_single = state.active_provider_group.is_none()
        && state.active_provider_id.as_deref() == Some(&prepared.profile.id)
        && !crate::aggregate_api::is_active_id(&prepared.profile.id);
    if !is_active_single && !affects_active_group && !affects_active_aggregate {
        crate::providers::ensure_provider_edit_is_inactive(
            paths,
            &prepared.profile.id,
            Some(&existing.group),
            &prepared.profile.group,
        )?;
        return Ok(None);
    }
    let context = crate::providers::prepare_active_provider_edit_context(
        app,
        prepared.profile.clone(),
        None,
    )?;
    let mode = active_provider_edit_mode(&context);
    if mode == SafeAccountSwitchMode::Direct {
        return Err("Active Provider edit requires the local proxy".to_string());
    }
    let (confirm_token, expires_at) =
        issue_safe_mutation_plan(PendingSafeMutationAction::EditActiveProvider {
            updated_profile: Box::new(prepared.profile),
            expected_mode: mode,
            expected_revision: context.revision().to_string(),
        })?;
    Ok(Some(SafeProviderEditPreview {
        confirm_token,
        expires_at: expires_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        target_label: context.target_label().to_string(),
        mode,
        protected_targets: safe_active_provider_edit_preview_targets(),
        will_restart_client: false,
        creates_restore_point: true,
        automatic_rollback: true,
    }))
}

#[tauri::command]
pub(crate) async fn prepare_safe_active_provider_edit<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    provider: crate::providers::ProviderInput,
) -> Result<Option<SafeProviderEditPreview>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = resolve_paths(&app)?;
        let prepared = crate::providers::prepare_provider_save(&paths, provider)?;
        prepare_safe_active_provider_edit_plan(&app, &paths, prepared)
    })
    .await
    .map_err(|error| format!("Provider edit preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn prepare_safe_provider_group_edit<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    id: String,
    group: String,
) -> Result<Option<SafeProviderEditPreview>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = resolve_paths(&app)?;
        let prepared = crate::providers::prepare_provider_group_update(&paths, &id, &group)?;
        prepare_safe_active_provider_edit_plan(&app, &paths, prepared)
    })
    .await
    .map_err(|error| format!("Provider group edit preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn prepare_safe_provider_model_switch<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    id: String,
    model: String,
) -> Result<Option<SafeProviderEditPreview>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = resolve_paths(&app)?;
        let prepared = crate::providers::prepare_provider_model_switch(&paths, &id, &model)?;
        prepare_safe_active_provider_edit_plan(&app, &paths, prepared)
    })
    .await
    .map_err(|error| format!("Provider model switch preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn prepare_safe_provider_model_control<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    id: String,
    controlled_by_codex: bool,
) -> Result<Option<SafeProviderEditPreview>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = resolve_paths(&app)?;
        let prepared =
            crate::providers::prepare_provider_model_control(&paths, &id, controlled_by_codex)?;
        prepare_safe_active_provider_edit_plan(&app, &paths, prepared)
    })
    .await
    .map_err(|error| format!("Provider model control preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn confirm_safe_active_provider_edit<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<crate::models::ProviderSummary, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let plan = consume_safe_mutation_plan(&confirm_token)?;
        let PendingSafeMutationAction::EditActiveProvider {
            updated_profile,
            expected_mode,
            expected_revision,
        } = plan
        else {
            return Err("确认令牌不属于 Provider edit。请重新预览。".to_string());
        };
        perform_safe_active_provider_edit_blocking(
            app,
            *updated_profile,
            expected_mode,
            expected_revision,
        )
    })
    .await
    .map_err(|error| format!("Provider edit confirmation task failed: {error}"))?
}

fn active_aggregate_edit_mode(
    context: &crate::providers::ActiveAggregateEditContext,
) -> SafeAccountSwitchMode {
    if !context.write_codex() {
        SafeAccountSwitchMode::ManagerOnly
    } else if context.proxy_running() {
        SafeAccountSwitchMode::LocalProxy
    } else {
        SafeAccountSwitchMode::Direct
    }
}

#[tauri::command]
pub(crate) async fn prepare_safe_active_aggregate_edit<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    aggregate: crate::aggregate_api::AggregateApiInput,
) -> Result<Option<SafeProviderEditPreview>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = resolve_paths(&app)?;
        let prepared = crate::aggregate_api::prepare_aggregate_save(&paths, aggregate)?;
        let Some(existing) = prepared.existing.as_ref() else {
            return Ok(None);
        };
        let state = read_state(&paths);
        if state
            .active_provider_id
            .as_deref()
            .and_then(crate::aggregate_api::config_id_from_active_id)
            != Some(prepared.config.id.as_str())
        {
            return Ok(None);
        }
        if existing.id != prepared.config.id {
            return Err("Aggregate edit target changed during preview".to_string());
        }
        let context = crate::providers::prepare_active_aggregate_edit_context(
            &app,
            prepared.config.clone(),
            None,
        )?;
        let mode = active_aggregate_edit_mode(&context);
        if mode == SafeAccountSwitchMode::Direct && !context.disables() {
            return Err("Editing an active Aggregate requires the local proxy".to_string());
        }
        let will_restart_client =
            mode == SafeAccountSwitchMode::Direct && chatgpt_or_codex_is_running()?;
        let (confirm_token, expires_at) =
            issue_safe_mutation_plan(PendingSafeMutationAction::EditActiveAggregate {
                updated_config: Box::new(prepared.config),
                expected_mode: mode,
                expected_revision: context.revision().to_string(),
            })?;
        Ok(Some(SafeProviderEditPreview {
            confirm_token,
            expires_at: expires_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            target_label: context.target_label().to_string(),
            mode,
            protected_targets: safe_active_aggregate_edit_preview_targets(),
            will_restart_client,
            creates_restore_point: true,
            automatic_rollback: true,
        }))
    })
    .await
    .map_err(|error| format!("Aggregate edit preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn confirm_safe_active_aggregate_edit<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<crate::aggregate_api::AggregateApiSummary, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let plan = consume_safe_mutation_plan(&confirm_token)?;
        let PendingSafeMutationAction::EditActiveAggregate {
            updated_config,
            expected_mode,
            expected_revision,
        } = plan
        else {
            return Err("确认令牌不属于 Aggregate edit。请重新预览。".to_string());
        };
        perform_safe_active_aggregate_edit_blocking(
            app,
            *updated_config,
            expected_mode,
            expected_revision,
        )
    })
    .await
    .map_err(|error| format!("Aggregate edit confirmation task failed: {error}"))?
}

fn perform_safe_active_aggregate_edit_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    updated_config: crate::aggregate_api::AggregateApiConfig,
    expected_mode: SafeAccountSwitchMode,
    expected_revision: String,
) -> Result<crate::aggregate_api::AggregateApiSummary, String> {
    let _switch_guard = account_switch_lock()
        .lock()
        .map_err(|_| "Account and Provider switch lock is poisoned".to_string())?;
    refresh_local_codex_path(&app);
    let initial_context = crate::providers::prepare_active_aggregate_edit_context(
        &app,
        updated_config.clone(),
        Some(&expected_revision),
    )?;
    if active_aggregate_edit_mode(&initial_context) != expected_mode {
        return Err("Aggregate edit 模式在预览后发生变化，未修改任何文件。请重新预览。".to_string());
    }
    if expected_mode == SafeAccountSwitchMode::Direct && !initial_context.disables() {
        return Err("Editing an active Aggregate requires the local proxy".to_string());
    }
    let targets = safe_active_aggregate_edit_targets(initial_context.paths())?;
    let store = safe_switch_restore_store(initial_context.paths())?;
    let (lease, mut legacy_probe) = acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "Aggregate edit 安全互斥在执行前失效，未修改任何文件。".to_string())?;

    let controls_runtime = initial_context.write_codex() && !initial_context.proxy_running();
    let client_was_running = controls_runtime && chatgpt_or_codex_is_running()?;
    let launch_target = client_was_running
        .then(|| refresh_and_get_chatgpt_launch_target(&app))
        .flatten();
    if client_was_running {
        stop_chatgpt_processes()?;
        if let Err(error) = wait_for_chatgpt_processes_to_exit(Duration::from_secs(10)) {
            let _ = restart_client_after_safe_switch(launch_target.as_ref());
            return Err(error);
        }
    }

    let context = match crate::providers::prepare_active_aggregate_edit_context(
        &app,
        updated_config,
        Some(&expected_revision),
    ) {
        Ok(context)
            if active_aggregate_edit_mode(&context) == expected_mode
                && context.revision() == initial_context.revision() =>
        {
            context
        }
        Ok(_) | Err(_) => {
            if client_was_running {
                let _ = restart_client_after_safe_switch(launch_target.as_ref());
            }
            return Err(
                "Aggregate edit 条件在客户端停止后发生变化，未修改文件。请重试。".to_string(),
            );
        }
    };
    let transaction = execute_safe_switch_transaction(
        &lease,
        &mut legacy_probe,
        &store,
        &targets,
        SafeSwitchTransactionMetadata {
            operation: capacity_mutation::RestorePointOperation::EditAggregateApi,
            codex_was_running: client_was_running,
        },
        || crate::providers::apply_active_aggregate_edit_files(&app, &context),
        || crate::providers::verify_active_aggregate_edit_postcondition(&context),
    );
    if let Err(error) = transaction {
        let message = error.user_message();
        if client_was_running
            && restart_client_after_safe_switch(launch_target.as_ref()).is_err()
        {
            return Err(format!(
                "{message} ChatGPT/Codex 也未能自动重新启动，请手动启动。"
            ));
        }
        return Err(message);
    }
    let (summary, warnings) = crate::providers::finalize_active_aggregate_edit(&app, &context);
    for warning in warnings {
        eprintln!("safe Aggregate edit post-commit warning: {warning}");
    }
    if client_was_running {
        restart_client_after_safe_switch(launch_target.as_ref()).map_err(|_| {
            concat!(
                "Aggregate 更改已安全提交，但无法自动启动 ChatGPT/Codex。",
                "请手动启动 ChatGPT 或 Codex。"
            )
            .to_string()
        })?;
    }
    Ok(summary)
}

fn perform_safe_active_provider_edit_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    updated_profile: crate::models::ProviderProfile,
    expected_mode: SafeAccountSwitchMode,
    expected_revision: String,
) -> Result<crate::models::ProviderSummary, String> {
    let _switch_guard = account_switch_lock()
        .lock()
        .map_err(|_| "Account and Provider switch lock is poisoned".to_string())?;
    refresh_local_codex_path(&app);
    let initial_context = crate::providers::prepare_active_provider_edit_context(
        &app,
        updated_profile.clone(),
        Some(&expected_revision),
    )?;
    if active_provider_edit_mode(&initial_context) != expected_mode {
        return Err("Provider edit 模式在预览后发生变化，未修改任何文件。请重新预览。".to_string());
    }
    let (lease, mut legacy_probe) = acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "Provider edit 安全互斥在执行前失效，未修改任何文件。".to_string())?;
    let context = crate::providers::prepare_active_provider_edit_context(
        &app,
        updated_profile,
        Some(&expected_revision),
    )?;
    if active_provider_edit_mode(&context) != expected_mode
        || context.revision() != initial_context.revision()
    {
        return Err("Provider edit 条件在执行前发生变化，未修改任何文件。请重试。".to_string());
    }
    let targets = safe_active_provider_edit_targets(context.paths(), context.provider_id())?;
    let store = safe_switch_restore_store(context.paths())?;
    execute_safe_switch_transaction(
        &lease,
        &mut legacy_probe,
        &store,
        &targets,
        SafeSwitchTransactionMetadata {
            operation: capacity_mutation::RestorePointOperation::EditProviderProfile,
            codex_was_running: false,
        },
        || crate::providers::apply_active_provider_edit_files(&app, &context),
        || crate::providers::verify_active_provider_edit_postcondition(&context),
    )
    .map_err(SafeSwitchTransactionError::user_message)?;
    let (summary, warnings) = crate::providers::finalize_active_provider_edit(&app, &context);
    for warning in warnings {
        eprintln!("safe Provider edit post-commit warning: {warning}");
    }
    Ok(summary)
}

fn restore_point_matches_safe_switch_targets(
    point: &capacity_mutation::RestorePoint,
    targets: &[capacity_mutation::ProtectedTargetPath],
) -> bool {
    let actual: std::collections::BTreeSet<_> = point.protected_targets().collect();
    let expected: std::collections::BTreeSet<_> =
        targets.iter().map(|target| target.target()).collect();
    actual == expected
}

fn rollback_operation_supported(operation: capacity_mutation::RestorePointOperation) -> bool {
    matches!(
        operation,
        capacity_mutation::RestorePointOperation::SwitchAccount
            | capacity_mutation::RestorePointOperation::DeactivateAccount
            | capacity_mutation::RestorePointOperation::EnterProviderMode
            | capacity_mutation::RestorePointOperation::ExitProviderMode
            | capacity_mutation::RestorePointOperation::EditProviderProfile
            | capacity_mutation::RestorePointOperation::EditAggregateApi
    )
}

fn provider_edit_targets_for_restore_point(
    paths: &Paths,
    store: &capacity_mutation::RestorePointStore,
    point: &capacity_mutation::RestorePoint,
) -> Result<Vec<capacity_mutation::ProtectedTargetPath>, String> {
    let capacity_mutation::RestorePointState::Committed { postconditions, .. } = point.state()
    else {
        return Err("Provider edit restore point is not committed".to_string());
    };
    let expected_profile = postconditions
        .state(capacity_mutation::ProtectedTarget::ActiveProviderProfile)
        .ok_or_else(|| "Provider edit restore point is missing its profile target".to_string())?;
    let expected_metadata = postconditions
        .state(capacity_mutation::ProtectedTarget::ActiveProviderFieldMetadata)
        .ok_or_else(|| "Provider edit restore point is missing its metadata target".to_string())?;
    let mut matches = Vec::new();
    for provider in crate::providers::list_provider_profiles(paths)? {
        let targets = safe_active_provider_edit_targets(paths, &provider.id)?;
        let observed = store
            .observe(&targets)
            .map_err(|_| "无法识别上一次 Provider edit 的安全目标。".to_string())?;
        if observed.state(capacity_mutation::ProtectedTarget::ActiveProviderProfile)
            == Some(expected_profile)
            && observed.state(capacity_mutation::ProtectedTarget::ActiveProviderFieldMetadata)
                == Some(expected_metadata)
        {
            matches.push(targets);
        }
    }
    if matches.len() != 1 {
        return Err("无法唯一识别上一次 Provider edit 的安全目标。".to_string());
    }
    matches
        .pop()
        .ok_or_else(|| "无法识别上一次 Provider edit 的安全目标。".to_string())
}

fn mutation_targets_for_restore_point(
    paths: &Paths,
    store: &capacity_mutation::RestorePointStore,
    point: &capacity_mutation::RestorePoint,
) -> Result<Vec<capacity_mutation::ProtectedTargetPath>, String> {
    match point.operation() {
        capacity_mutation::RestorePointOperation::SwitchAccount
        | capacity_mutation::RestorePointOperation::DeactivateAccount => safe_switch_targets(paths),
        capacity_mutation::RestorePointOperation::EnterProviderMode
        | capacity_mutation::RestorePointOperation::ExitProviderMode => safe_provider_targets(paths),
        capacity_mutation::RestorePointOperation::EditProviderProfile => {
            provider_edit_targets_for_restore_point(paths, store, point)
        }
        capacity_mutation::RestorePointOperation::EditAggregateApi => {
            safe_active_aggregate_edit_targets(paths)
        }
        _ => Err("该安全更改不能由当前版本回滚。".to_string()),
    }
}

fn preview_targets_for_operation(
    operation: capacity_mutation::RestorePointOperation,
) -> Result<Vec<SafeSwitchProtectedTarget>, String> {
    match operation {
        capacity_mutation::RestorePointOperation::SwitchAccount
        | capacity_mutation::RestorePointOperation::DeactivateAccount => {
            Ok(safe_switch_preview_targets())
        }
        capacity_mutation::RestorePointOperation::EnterProviderMode
        | capacity_mutation::RestorePointOperation::ExitProviderMode => {
            Ok(safe_provider_preview_targets())
        }
        capacity_mutation::RestorePointOperation::EditProviderProfile => {
            Ok(safe_active_provider_edit_preview_targets())
        }
        capacity_mutation::RestorePointOperation::EditAggregateApi => {
            Ok(safe_active_aggregate_edit_preview_targets())
        }
        _ => Err("该安全更改不能由当前版本回滚。".to_string()),
    }
}

#[tauri::command]
pub(crate) async fn prepare_rollback_last_change<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
) -> Result<SafeRollbackPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = resolve_paths(&app)?;
        let store = safe_switch_restore_store(&paths)?;
        let point = store
            .latest_committed()
            .map_err(|_| "无法读取上一次安全更改。".to_string())?
            .ok_or_else(|| "没有可回滚的已提交更改。".to_string())?;
        let targets = mutation_targets_for_restore_point(&paths, &store, &point)?;
        if !rollback_operation_supported(point.operation())
            || !restore_point_matches_safe_switch_targets(&point, &targets)
        {
            return Err("上一次更改不能由当前版本安全回滚。".to_string());
        }
        store
            .verify_rollback_ready(point.id(), &targets)
            .map_err(|error| match error {
                capacity_mutation::RestorePointError::PreconditionChanged(_) => {
                    "上次更改后相关文件又被修改，已拒绝覆盖。".to_string()
                }
                _ => "无法验证上一次安全更改的当前状态。".to_string(),
            })?;
        let will_restart_client = chatgpt_or_codex_is_running()? || point.codex_was_running();
        let (confirm_token, expires_at) = issue_safe_mutation_plan(
            PendingSafeMutationAction::RollbackLastChange {
                restore_point_id: point.id().clone(),
            },
        )?;
        Ok(SafeRollbackPreview {
            confirm_token,
            expires_at: expires_at
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            protected_targets: preview_targets_for_operation(point.operation())?,
            will_restart_client,
            verifies_current_state: true,
        })
    })
    .await
    .map_err(|error| format!("Rollback preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn confirm_rollback_last_change<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _switch_guard = account_switch_lock()
            .lock()
            .map_err(|_| "Account switch lock is poisoned".to_string())?;
        let plan = consume_safe_mutation_plan(&confirm_token)?;
        let PendingSafeMutationAction::RollbackLastChange { restore_point_id } = plan else {
            return Err("确认令牌不属于回滚操作。请重新预览。".to_string());
        };
        refresh_local_codex_path(&app);
        perform_safe_rollback_last_change(&app, &restore_point_id)
    })
    .await
    .map_err(|error| format!("Rollback confirmation task failed: {error}"))?
}

fn perform_safe_rollback_last_change<R: Runtime>(
    app: &tauri::AppHandle<R>,
    restore_point_id: &capacity_mutation::RestorePointId,
) -> Result<(), String> {
    let paths = resolve_paths(app)?;
    let store = safe_switch_restore_store(&paths)?;
    let point = store
        .verify(restore_point_id)
        .map_err(|_| "无法验证上一次安全更改，未修改任何文件。".to_string())?;
    let targets = mutation_targets_for_restore_point(&paths, &store, &point)?;
    let point = store
        .verify_rollback_ready(restore_point_id, &targets)
        .map_err(|error| match error {
            capacity_mutation::RestorePointError::PreconditionChanged(_) => {
                "上次更改后相关文件又被修改，已拒绝覆盖。".to_string()
            }
            _ => "无法验证上一次安全更改，未修改任何文件。".to_string(),
        })?;
    if !rollback_operation_supported(point.operation())
        || !restore_point_matches_safe_switch_targets(&point, &targets)
    {
        return Err("上一次更改不能由当前版本安全回滚。".to_string());
    }

    let (lease, mut legacy_probe) = acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "回滚安全互斥在执行前失效，未修改任何文件。".to_string())?;
    let client_was_running = chatgpt_or_codex_is_running()?;
    let should_restart = client_was_running || point.codex_was_running();
    let launch_target = should_restart
        .then(|| refresh_and_get_chatgpt_launch_target(app))
        .flatten();
    if client_was_running {
        stop_chatgpt_processes()?;
        if let Err(error) = wait_for_chatgpt_processes_to_exit(Duration::from_secs(10)) {
            let _ = restart_client_after_safe_switch(launch_target.as_ref());
            return Err(error);
        }
    }

    if lease.revalidate(&mut legacy_probe).is_err() {
        if client_was_running {
            let _ = restart_client_after_safe_switch(launch_target.as_ref());
        }
        return Err("回滚安全互斥在写入前失效，未修改任何文件。".to_string());
    }
    let rolled_back_at = match safe_switch_timestamp() {
        Ok(timestamp) => timestamp,
        Err(error) => {
            if client_was_running {
                let _ = restart_client_after_safe_switch(launch_target.as_ref());
            }
            return Err(error);
        }
    };
    if let Err(error) = store.rollback_committed(
        restore_point_id,
        &targets,
        &rolled_back_at,
    ) {
        if client_was_running {
            let _ = restart_client_after_safe_switch(launch_target.as_ref());
        }
        return Err(match error {
            capacity_mutation::RestorePointError::PreconditionChanged(_) => {
                "上次更改后相关文件又被修改，已拒绝覆盖。".to_string()
            }
            capacity_mutation::RestorePointError::RollbackCompensationFailed => concat!(
                "回滚未能完成，且无法确认补偿结果。请保持 ChatGPT/Codex 关闭，",
                "不要继续切换，并检查本地还原点。"
            )
            .to_string(),
            _ => "回滚未能完成；原文件保持不变或已完成内部补偿。".to_string(),
        });
    }

    capacity_desktop_service::invalidate_account_binding(app);
    if let Err(error) = app.emit("accounts-changed", ()) {
        eprintln!("safe rollback post-commit warning: accounts event failed: {error}");
    }
    if let Err(error) = app.emit("providers-changed", ()) {
        eprintln!("safe rollback post-commit warning: providers event failed: {error}");
    }
    if let Err(error) = crate::claude_code::sync_after_switch(app) {
        eprintln!("safe rollback post-commit warning: third-party sync failed: {error}");
    }
    if crate::local_proxy::is_running() {
        crate::providers::refresh_codex_models_for_current_target_blocking(&paths);
    }
    crate::system_tray::refresh_menu(app);

    if should_restart {
        restart_client_after_safe_switch(launch_target.as_ref()).map_err(|_| {
            concat!(
                "上一次更改已安全回滚，但无法自动启动 ChatGPT/Codex。",
                "请手动启动 ChatGPT 或 Codex。"
            )
            .to_string()
        })?;
    }
    Ok(())
}

fn perform_safe_account_switch<R: Runtime>(
    app: &tauri::AppHandle<R>,
    id: &str,
    launch_when_stopped: bool,
    preserve_previous: bool,
) -> Result<(), String> {
    perform_safe_account_switch_with_expected_mode(
        app,
        id,
        launch_when_stopped,
        preserve_previous,
        None,
    )
}

fn perform_safe_account_switch_with_expected_mode<R: Runtime>(
    app: &tauri::AppHandle<R>,
    id: &str,
    launch_when_stopped: bool,
    preserve_previous: bool,
    expected_mode: Option<SafeAccountSwitchMode>,
) -> Result<(), String> {
    // Validate the selected managed credential before stopping a client.
    let initial_context = prepare_account_switch_context(app, id, preserve_previous)?;
    if expected_mode.is_some_and(|expected| expected != safe_switch_mode(&initial_context)) {
        return Err(
            "账户切换模式在预览后发生变化，未修改任何文件。请重新预览。".to_string(),
        );
    }
    let (lease, mut legacy_probe) = acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "账户切换安全互斥在执行前失效，未修改任何账户文件。".to_string())?;

    let controls_runtime = initial_context.write_codex && !initial_context.proxy_running;
    let client_was_running = controls_runtime && chatgpt_or_codex_is_running()?;
    let should_restart = controls_runtime && (client_was_running || launch_when_stopped);
    let launch_target = should_restart
        .then(|| refresh_and_get_chatgpt_launch_target(app))
        .flatten();

    if client_was_running {
        stop_chatgpt_processes()?;
        if let Err(error) = wait_for_chatgpt_processes_to_exit(Duration::from_secs(10)) {
            let _ = restart_client_after_safe_switch(launch_target.as_ref());
            return Err(error);
        }
    }

    // Re-read all preflight inputs after the client has stopped. This captures
    // the final token refresh and closes the gap between preview and capture.
    let context = match prepare_account_switch_context(app, id, preserve_previous) {
        Ok(context)
            if context.write_codex == initial_context.write_codex
                && context.proxy_running == initial_context.proxy_running =>
        {
            context
        }
        Ok(_) | Err(_) => {
            if client_was_running {
                let _ = restart_client_after_safe_switch(launch_target.as_ref());
            }
            return Err(
                "账户切换条件在客户端停止后发生变化，未修改账户文件。请重试。".to_string(),
            );
        }
    };
    let (targets, store) = match safe_switch_targets(&context.paths)
        .and_then(|targets| safe_switch_restore_store(&context.paths).map(|store| (targets, store)))
    {
        Ok(prepared) => prepared,
        Err(error) => {
            if client_was_running {
                let _ = restart_client_after_safe_switch(launch_target.as_ref());
            }
            return Err(error);
        }
    };
    let transaction = execute_safe_switch_transaction(
        &lease,
        &mut legacy_probe,
        &store,
        &targets,
        SafeSwitchTransactionMetadata {
            operation: capacity_mutation::RestorePointOperation::SwitchAccount,
            codex_was_running: client_was_running,
        },
        || apply_account_switch_files(app, id, &context),
        || verify_account_switch_postcondition(id, &context),
    );

    if let Err(error) = transaction {
        let message = error.user_message();
        if client_was_running
            && restart_client_after_safe_switch(launch_target.as_ref()).is_err()
        {
            return Err(format!(
                "{message} ChatGPT/Codex 也未能自动重新启动，请手动启动。"
            ));
        }
        return Err(message);
    }

    for warning in finalize_account_switch(app, id, &context) {
        eprintln!("safe switch post-commit warning: {warning}");
    }
    if should_restart {
        restart_client_after_safe_switch(launch_target.as_ref()).map_err(|_| {
            concat!(
                "账户切换已经安全提交，但无法自动启动 ChatGPT/Codex。",
                "请手动启动 ChatGPT 或 Codex。"
            )
            .to_string()
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod safe_account_switch_tests {
    use super::{
        AccountDeactivateContext, PendingSafeMutationAction, PendingSafeMutationPlans,
        SafeAccountSwitchMode, SafeSwitchFailureStage, SafeSwitchTransactionMetadata,
        execute_safe_switch_transaction, provider_edit_targets_for_restore_point,
        safe_active_aggregate_edit_targets, safe_active_provider_edit_targets,
        safe_provider_targets, safe_switch_targets,
        verify_account_deactivate_postcondition,
    };
    use crate::storage::Paths;
    use capacity_domain::UtcTimestamp;
    use capacity_mutation::{
        LegacyViewerProbe, LegacyViewerState, MutationLock, MutationLockOwner, MutationOwnerId,
        RestorePointStore,
    };
    use std::fs;

    #[derive(Default)]
    struct StoppedLegacyViewer;

    impl LegacyViewerProbe for StoppedLegacyViewer {
        fn legacy_viewer_state(&mut self) -> LegacyViewerState {
            LegacyViewerState::NotRunning
        }
    }

    fn owner() -> MutationLockOwner {
        MutationLockOwner::new(
            MutationOwnerId::parse(
                "mutation-owner:v1:00000000-0000-4000-8000-000000000001",
            )
            .expect("owner ID"),
            std::process::id(),
            UtcTimestamp::parse("2026-08-31T00:00:00Z").expect("timestamp"),
        )
        .expect("owner")
    }

    fn paths(root: &std::path::Path) -> Paths {
        let codex_home = root.join("codex-home");
        let app_data = root.join("app-data");
        Paths {
            current_auth: codex_home.join("auth.json"),
            current_config: codex_home.join("config.toml"),
            codex_home,
            accounts: app_data.join("accounts"),
            providers: app_data.join("providers"),
            config_backup: app_data.join("config-before-provider.toml"),
            state_file: app_data.join("state.json"),
        }
    }

    fn fixture(
        root: &std::path::Path,
    ) -> (
        Paths,
        RestorePointStore,
        Vec<capacity_mutation::ProtectedTargetPath>,
    ) {
        let paths = paths(root);
        fs::create_dir_all(&paths.codex_home).expect("Codex home");
        fs::create_dir_all(paths.state_file.parent().expect("app data")).expect("app data");
        fs::write(&paths.current_auth, b"auth-before").expect("auth");
        fs::write(&paths.current_config, b"config-before").expect("config");
        fs::write(&paths.state_file, b"state-before").expect("state");
        fs::write(&paths.config_backup, b"backup-before").expect("backup");
        let targets = safe_switch_targets(&paths).expect("targets");
        let store = RestorePointStore::new(root.join("restore-points")).expect("store");
        (paths, store, targets)
    }

    fn acquire(root: &std::path::Path, probe: &mut StoppedLegacyViewer) -> MutationLock {
        MutationLock::acquire(root.join("lock"), owner(), probe).expect("lease")
    }

    fn plan_time(value: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(value)
            .expect("plan time")
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn confirmation_plan_is_target_bound_and_one_time() {
        let now = plan_time("2026-08-31T00:00:00Z");
        let mut plans = PendingSafeMutationPlans::default();
        let (token, expires_at) = plans
            .issue(
                PendingSafeMutationAction::SwitchAccount {
                    target_account_id: "account-a".to_string(),
                    expected_mode: SafeAccountSwitchMode::Direct,
                },
                now,
            )
            .expect("issue plan");
        assert_eq!(expires_at, plan_time("2026-08-31T00:02:00Z"));

        let action = plans
            .consume(&token, plan_time("2026-08-31T00:01:59Z"))
            .expect("consume plan");
        assert!(matches!(
            action,
            PendingSafeMutationAction::SwitchAccount {
                target_account_id,
                expected_mode: SafeAccountSwitchMode::Direct,
            } if target_account_id == "account-a"
        ));
        assert!(plans
            .consume(&token, plan_time("2026-08-31T00:01:59Z"))
            .is_err());
    }

    #[test]
    fn expired_confirmation_plan_cannot_be_replayed() {
        let now = plan_time("2026-08-31T00:00:00Z");
        let mut plans = PendingSafeMutationPlans::default();
        let (token, _) = plans
            .issue(
                PendingSafeMutationAction::SwitchAccount {
                    target_account_id: "account-a".to_string(),
                    expected_mode: SafeAccountSwitchMode::LocalProxy,
                },
                now,
            )
            .expect("issue plan");

        assert!(plans
            .consume(&token, plan_time("2026-08-31T00:02:00Z"))
            .is_err());
        assert!(plans
            .consume(&token, plan_time("2026-08-31T00:01:00Z"))
            .is_err());
    }

    #[test]
    fn deactivation_confirmation_is_bound_to_the_active_account_and_mode() {
        let now = plan_time("2026-08-31T00:00:00Z");
        let mut plans = PendingSafeMutationPlans::default();
        let (token, _) = plans
            .issue(
                PendingSafeMutationAction::DeactivateAccount {
                    expected_account_id: "account-a".to_string(),
                    expected_mode: SafeAccountSwitchMode::Direct,
                },
                now,
            )
            .expect("issue deactivation plan");

        assert!(matches!(
            plans.consume(&token, now).expect("consume plan"),
            PendingSafeMutationAction::DeactivateAccount {
                expected_account_id,
                expected_mode: SafeAccountSwitchMode::Direct,
            } if expected_account_id == "account-a"
        ));
    }

    #[test]
    fn provider_confirmation_is_bound_to_target_current_route_and_mode() {
        let now = plan_time("2026-08-31T00:00:00Z");
        let mut plans = PendingSafeMutationPlans::default();
        let (token, _) = plans
            .issue(
                PendingSafeMutationAction::MutateProvider {
                    target: crate::providers::ProviderMutationTarget::Provider(
                        "provider-a".to_string(),
                    ),
                    expected_current: crate::providers::ProviderRuntimeSelection::Official,
                    expected_mode: SafeAccountSwitchMode::LocalProxy,
                    expected_target_revision: "revision-a".to_string(),
                },
                now,
            )
            .expect("issue Provider plan");

        assert!(matches!(
            plans.consume(&token, now).expect("consume plan"),
            PendingSafeMutationAction::MutateProvider {
                target: crate::providers::ProviderMutationTarget::Provider(id),
                expected_current: crate::providers::ProviderRuntimeSelection::Official,
                expected_mode: SafeAccountSwitchMode::LocalProxy,
                expected_target_revision,
            } if id == "provider-a" && expected_target_revision == "revision-a"
        ));
    }

    #[test]
    fn aggregate_confirmation_is_bound_to_aggregate_and_current_route() {
        let now = plan_time("2026-08-31T00:00:00Z");
        let mut plans = PendingSafeMutationPlans::default();
        let (token, _) = plans
            .issue(
                PendingSafeMutationAction::MutateProvider {
                    target: crate::providers::ProviderMutationTarget::Aggregate(
                        "aggregate-a".to_string(),
                    ),
                    expected_current: crate::providers::ProviderRuntimeSelection::Provider(
                        "provider-a".to_string(),
                    ),
                    expected_mode: SafeAccountSwitchMode::LocalProxy,
                    expected_target_revision: "revision-a".to_string(),
                },
                now,
            )
            .expect("issue Aggregate plan");

        assert!(matches!(
            plans.consume(&token, now).expect("consume plan"),
            PendingSafeMutationAction::MutateProvider {
                target: crate::providers::ProviderMutationTarget::Aggregate(id),
                expected_current: crate::providers::ProviderRuntimeSelection::Provider(provider),
                expected_mode: SafeAccountSwitchMode::LocalProxy,
                expected_target_revision,
            } if id == "aggregate-a"
                && provider == "provider-a"
                && expected_target_revision == "revision-a"
        ));
    }

    #[test]
    fn direct_deactivation_postflight_requires_state_and_auth_to_be_cleared() {
        let root = tempfile::tempdir().expect("tempdir");
        let paths = paths(root.path());
        fs::create_dir_all(&paths.codex_home).expect("Codex home");
        fs::create_dir_all(paths.state_file.parent().expect("app data")).expect("app data");
        crate::storage::write_state(&paths, &crate::models::ManagerStateFile::default())
            .expect("write state");
        let context = AccountDeactivateContext {
            proxy_running: false,
            write_codex: true,
            paths: paths.clone(),
            original_state: crate::models::ManagerStateFile::default(),
            active_account_id: "account-a".to_string(),
            refreshed_auth: None,
        };
        verify_account_deactivate_postcondition(&context).expect("cleared postcondition");

        fs::write(&paths.current_auth, b"stale-auth").expect("stale auth");
        assert!(verify_account_deactivate_postcondition(&context).is_err());
    }

    #[test]
    fn safe_transaction_commits_only_after_postflight() {
        let root = tempfile::tempdir().expect("tempdir");
        let (paths, store, targets) = fixture(root.path());
        let mut probe = StoppedLegacyViewer;
        let lease = acquire(root.path(), &mut probe);

        let id = execute_safe_switch_transaction(
            &lease,
            &mut probe,
            &store,
            &targets,
            SafeSwitchTransactionMetadata {
                operation: capacity_mutation::RestorePointOperation::SwitchAccount,
                codex_was_running: true,
            },
            || {
                fs::write(&paths.current_auth, b"auth-after").map_err(|error| error.to_string())
            },
            || {
                (fs::read(&paths.current_auth).ok().as_deref() == Some(b"auth-after"))
                    .then_some(())
                    .ok_or_else(|| "postflight mismatch".to_string())
            },
        )
        .expect("commit");

        let point = store.verify(&id).expect("verify point");
        assert!(matches!(
            point.state(),
            capacity_mutation::RestorePointState::Committed { .. }
        ));
        assert!(point.codex_was_running());
        assert_eq!(fs::read(&paths.current_auth).unwrap(), b"auth-after");
    }

    #[test]
    fn safe_transaction_records_deactivation_operation() {
        let root = tempfile::tempdir().expect("tempdir");
        let (paths, store, targets) = fixture(root.path());
        let mut probe = StoppedLegacyViewer;
        let lease = acquire(root.path(), &mut probe);
        let id = execute_safe_switch_transaction(
            &lease,
            &mut probe,
            &store,
            &targets,
            SafeSwitchTransactionMetadata {
                operation: capacity_mutation::RestorePointOperation::DeactivateAccount,
                codex_was_running: false,
            },
            || fs::remove_file(&paths.current_auth).map_err(|error| error.to_string()),
            || {
                (!paths.current_auth.exists())
                    .then_some(())
                    .ok_or_else(|| "auth remains".to_string())
            },
        )
        .expect("commit deactivation");

        assert_eq!(
            store.verify(&id).expect("restore point").operation(),
            capacity_mutation::RestorePointOperation::DeactivateAccount
        );
    }

    #[test]
    fn safe_transaction_rolls_back_a_partial_apply_failure() {
        let root = tempfile::tempdir().expect("tempdir");
        let (paths, store, targets) = fixture(root.path());
        let mut probe = StoppedLegacyViewer;
        let lease = acquire(root.path(), &mut probe);

        let error = execute_safe_switch_transaction(
            &lease,
            &mut probe,
            &store,
            &targets,
            SafeSwitchTransactionMetadata {
                operation: capacity_mutation::RestorePointOperation::SwitchAccount,
                codex_was_running: false,
            },
            || {
                fs::write(&paths.current_auth, b"partial").expect("partial write");
                Err("injected apply failure".to_string())
            },
            || Ok(()),
        )
        .expect_err("must fail");

        assert_eq!(error.stage, SafeSwitchFailureStage::Apply);
        assert!(!error.recovery_required);
        assert_eq!(fs::read(&paths.current_auth).unwrap(), b"auth-before");
        assert_eq!(fs::read(&paths.current_config).unwrap(), b"config-before");
        assert_eq!(fs::read(&paths.state_file).unwrap(), b"state-before");
        assert_eq!(fs::read(&paths.config_backup).unwrap(), b"backup-before");
    }

    #[test]
    fn safe_transaction_rolls_back_a_postflight_failure() {
        let root = tempfile::tempdir().expect("tempdir");
        let (paths, store, targets) = fixture(root.path());
        let mut probe = StoppedLegacyViewer;
        let lease = acquire(root.path(), &mut probe);

        let error = execute_safe_switch_transaction(
            &lease,
            &mut probe,
            &store,
            &targets,
            SafeSwitchTransactionMetadata {
                operation: capacity_mutation::RestorePointOperation::SwitchAccount,
                codex_was_running: false,
            },
            || {
                fs::write(&paths.current_auth, b"auth-after").map_err(|error| error.to_string())
            },
            || Err("injected postflight failure".to_string()),
        )
        .expect_err("must fail");

        assert_eq!(error.stage, SafeSwitchFailureStage::Postflight);
        assert!(!error.recovery_required);
        assert_eq!(fs::read(&paths.current_auth).unwrap(), b"auth-before");
    }

    #[test]
    fn provider_transaction_protects_generated_model_catalog() {
        let root = tempfile::tempdir().expect("tempdir");
        let paths = paths(root.path());
        fs::create_dir_all(&paths.codex_home).expect("Codex home");
        fs::create_dir_all(paths.state_file.parent().expect("app data")).expect("app data");
        fs::write(&paths.current_auth, b"auth-before").expect("auth");
        fs::write(&paths.current_config, b"config-before").expect("config");
        fs::write(&paths.state_file, b"state-before").expect("state");
        fs::write(&paths.config_backup, b"backup-before").expect("backup");
        let catalog = crate::providers::provider_model_catalog_path(&paths);
        fs::write(&catalog, b"catalog-before").expect("catalog");
        let targets = safe_provider_targets(&paths).expect("Provider targets");
        let store = RestorePointStore::new(root.path().join("restore-points")).expect("store");
        let mut probe = StoppedLegacyViewer;
        let lease = acquire(root.path(), &mut probe);

        let error = execute_safe_switch_transaction(
            &lease,
            &mut probe,
            &store,
            &targets,
            SafeSwitchTransactionMetadata {
                operation: capacity_mutation::RestorePointOperation::EnterProviderMode,
                codex_was_running: false,
            },
            || fs::write(&catalog, b"catalog-after").map_err(|error| error.to_string()),
            || Err("injected Provider postflight failure".to_string()),
        )
        .expect_err("must roll back");

        assert_eq!(error.stage, SafeSwitchFailureStage::Postflight);
        assert_eq!(fs::read(catalog).unwrap(), b"catalog-before");
    }

    #[test]
    fn active_provider_edit_transaction_restores_profile_and_field_metadata() {
        let root = tempfile::tempdir().expect("tempdir");
        let paths = paths(root.path());
        fs::create_dir_all(&paths.codex_home).expect("Codex home");
        fs::create_dir_all(&paths.providers).expect("Provider store");
        fs::write(&paths.current_auth, b"auth-before").expect("auth");
        fs::write(&paths.current_config, b"config-before").expect("config");
        fs::write(&paths.state_file, b"state-before").expect("state");
        fs::write(&paths.config_backup, b"backup-before").expect("backup");
        let catalog = crate::providers::provider_model_catalog_path(&paths);
        fs::write(&catalog, b"catalog-before").expect("catalog");
        let profile = crate::providers::provider_path(&paths, "provider-a");
        let metadata = crate::providers::provider_field_modified_at_path(&paths, "provider-a");
        fs::write(&profile, b"profile-before").expect("profile");
        fs::write(&metadata, b"metadata-before").expect("metadata");
        let targets = safe_active_provider_edit_targets(&paths, "provider-a").expect("targets");
        let store = RestorePointStore::new(root.path().join("restore-points")).expect("store");
        let mut probe = StoppedLegacyViewer;
        let lease = acquire(root.path(), &mut probe);

        let error = execute_safe_switch_transaction(
            &lease,
            &mut probe,
            &store,
            &targets,
            SafeSwitchTransactionMetadata {
                operation: capacity_mutation::RestorePointOperation::EditProviderProfile,
                codex_was_running: false,
            },
            || {
                fs::write(&profile, b"profile-after").map_err(|error| error.to_string())?;
                fs::write(&metadata, b"metadata-after").map_err(|error| error.to_string())
            },
            || Err("injected active Provider edit postflight failure".to_string()),
        )
        .expect_err("must roll back");

        assert_eq!(error.stage, SafeSwitchFailureStage::Postflight);
        assert_eq!(fs::read(profile).unwrap(), b"profile-before");
        assert_eq!(fs::read(metadata).unwrap(), b"metadata-before");
    }

    #[test]
    fn provider_edit_rollback_finds_the_exact_profile_from_committed_postconditions() {
        let root = tempfile::tempdir().expect("tempdir");
        let paths = paths(root.path());
        fs::create_dir_all(&paths.codex_home).expect("Codex home");
        fs::create_dir_all(&paths.providers).expect("Provider store");
        fs::write(&paths.current_auth, b"auth-before").expect("auth");
        fs::write(&paths.current_config, b"config-before").expect("config");
        fs::write(&paths.state_file, b"state-before").expect("state");
        fs::write(&paths.config_backup, b"backup-before").expect("backup");
        fs::write(
            crate::providers::provider_model_catalog_path(&paths),
            b"catalog-before",
        )
        .expect("catalog");
        let profile = |id: &str, name: &str| {
            serde_json::to_vec(&serde_json::json!({
                "id": id,
                "name": name,
                "group": "production",
                "baseUrl": "https://provider.example/v1",
                "apiKey": "test-secret",
                "model": "gpt-4.1",
                "models": ["gpt-4.1"],
                "apiFormat": "openaiResponses"
            }))
            .expect("profile JSON")
        };
        let profile_a = crate::providers::provider_path(&paths, "provider-a");
        let metadata_a =
            crate::providers::provider_field_modified_at_path(&paths, "provider-a");
        let profile_b = crate::providers::provider_path(&paths, "provider-b");
        let metadata_b =
            crate::providers::provider_field_modified_at_path(&paths, "provider-b");
        fs::write(&profile_a, profile("provider-a", "Provider A")).expect("profile A");
        fs::write(&metadata_a, b"metadata-a-before").expect("metadata A");
        fs::write(&profile_b, profile("provider-b", "Provider B")).expect("profile B");
        fs::write(&metadata_b, b"metadata-b-before").expect("metadata B");
        let targets = safe_active_provider_edit_targets(&paths, "provider-a").expect("targets");
        let store = RestorePointStore::new(root.path().join("restore-points")).expect("store");
        let created_at = UtcTimestamp::parse("2026-08-31T00:00:00Z").expect("created at");
        let point = store
            .create_restore_point(
                capacity_mutation::RestorePointOperation::EditProviderProfile,
                &targets,
                false,
                &created_at,
                &std::collections::BTreeSet::new(),
            )
            .expect("restore point");
        fs::write(&profile_a, profile("provider-a", "Provider A Updated"))
            .expect("updated profile A");
        fs::write(&metadata_a, b"metadata-a-after").expect("updated metadata A");
        let committed_at = UtcTimestamp::parse("2026-08-31T00:01:00Z").expect("committed at");
        let point = store
            .mark_committed(point.id(), &targets, &committed_at)
            .expect("commit");

        let inferred = provider_edit_targets_for_restore_point(&paths, &store, &point)
            .expect("unique Provider edit target");
        let inferred_profile = inferred
            .iter()
            .find(|target| {
                target.target() == capacity_mutation::ProtectedTarget::ActiveProviderProfile
            })
            .expect("profile target");
        assert_eq!(inferred_profile.path(), profile_a);
    }

    #[test]
    fn active_aggregate_edit_transaction_restores_the_config_store() {
        let root = tempfile::tempdir().expect("tempdir");
        let paths = paths(root.path());
        fs::create_dir_all(&paths.codex_home).expect("Codex home");
        fs::create_dir_all(paths.state_file.parent().expect("app data")).expect("app data");
        fs::write(&paths.current_auth, b"auth-before").expect("auth");
        fs::write(&paths.current_config, b"config-before").expect("config");
        fs::write(&paths.state_file, b"state-before").expect("state");
        fs::write(&paths.config_backup, b"backup-before").expect("backup");
        fs::write(
            crate::providers::provider_model_catalog_path(&paths),
            b"catalog-before",
        )
        .expect("catalog");
        let aggregate_store = crate::aggregate_api::store_path(&paths).expect("aggregate store");
        fs::write(&aggregate_store, b"aggregate-before").expect("aggregate store");
        let targets = safe_active_aggregate_edit_targets(&paths).expect("targets");
        let store = RestorePointStore::new(root.path().join("restore-points")).expect("store");
        let mut probe = StoppedLegacyViewer;
        let lease = acquire(root.path(), &mut probe);

        let error = execute_safe_switch_transaction(
            &lease,
            &mut probe,
            &store,
            &targets,
            SafeSwitchTransactionMetadata {
                operation: capacity_mutation::RestorePointOperation::EditAggregateApi,
                codex_was_running: false,
            },
            || {
                fs::write(&aggregate_store, b"aggregate-after")
                    .map_err(|error| error.to_string())
            },
            || Err("injected Aggregate edit postflight failure".to_string()),
        )
        .expect_err("must roll back");

        assert_eq!(error.stage, SafeSwitchFailureStage::Postflight);
        assert_eq!(fs::read(aggregate_store).unwrap(), b"aggregate-before");
    }
}
