#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ProviderMutationTarget {
    Provider(String),
    Group(String),
    Aggregate(String),
    Official,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ProviderRuntimeSelection {
    Provider(String),
    Group(String),
    Aggregate(String),
    Official,
}

#[derive(Clone)]
enum PreparedProviderMutationTarget {
    Provider(Box<ProviderProfile>),
    Group {
        group: String,
        providers: Vec<ProviderProfile>,
    },
    Aggregate {
        config: Box<crate::aggregate_api::AggregateApiConfig>,
        providers: Vec<ProviderProfile>,
    },
    Official,
}

#[derive(Clone)]
pub(crate) struct ProviderMutationContext {
    target: ProviderRuntimeSelection,
    current: ProviderRuntimeSelection,
    target_label: String,
    target_revision: String,
    proxy_running: bool,
    write_codex: bool,
    paths: Paths,
    original_state: crate::models::ManagerStateFile,
    prepared: PreparedProviderMutationTarget,
}

impl ProviderMutationContext {
    pub(crate) fn target(&self) -> &ProviderRuntimeSelection {
        &self.target
    }

    pub(crate) fn current(&self) -> &ProviderRuntimeSelection {
        &self.current
    }

    pub(crate) fn target_label(&self) -> &str {
        &self.target_label
    }

    pub(crate) fn target_revision(&self) -> &str {
        &self.target_revision
    }

    pub(crate) fn proxy_running(&self) -> bool {
        self.proxy_running
    }

    pub(crate) fn write_codex(&self) -> bool {
        self.write_codex
    }

    pub(crate) fn paths(&self) -> &Paths {
        &self.paths
    }
}

#[derive(Clone)]
pub(crate) struct ActiveProviderEditContext {
    paths: Paths,
    original_state: crate::models::ManagerStateFile,
    original_profile: ProviderProfile,
    updated_profile: ProviderProfile,
    prepared_route: PreparedActiveProviderEditRoute,
    revision: String,
    proxy_running: bool,
    write_codex: bool,
}

#[derive(Clone)]
pub(crate) struct ActiveAggregateEditContext {
    paths: Paths,
    original_state: crate::models::ManagerStateFile,
    original_config: crate::aggregate_api::AggregateApiConfig,
    updated_config: crate::aggregate_api::AggregateApiConfig,
    updated_members: Vec<ProviderProfile>,
    revision: String,
    proxy_running: bool,
    write_codex: bool,
}

impl ActiveAggregateEditContext {
    pub(crate) fn paths(&self) -> &Paths {
        &self.paths
    }

    pub(crate) fn target_label(&self) -> &str {
        &self.updated_config.name
    }

    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }

    pub(crate) fn proxy_running(&self) -> bool {
        self.proxy_running
    }

    pub(crate) fn write_codex(&self) -> bool {
        self.write_codex
    }

    pub(crate) fn disables(&self) -> bool {
        !self.updated_config.enabled
    }
}

#[derive(Clone)]
enum PreparedActiveProviderEditRoute {
    Single,
    Group {
        group: String,
        providers: Vec<ProviderProfile>,
    },
    Aggregate {
        config: Box<crate::aggregate_api::AggregateApiConfig>,
        providers: Vec<ProviderProfile>,
    },
}

impl ActiveProviderEditContext {
    pub(crate) fn paths(&self) -> &Paths {
        &self.paths
    }

    pub(crate) fn provider_id(&self) -> &str {
        &self.updated_profile.id
    }

    pub(crate) fn target_label(&self) -> &str {
        &self.updated_profile.name
    }

    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }

    pub(crate) fn proxy_running(&self) -> bool {
        self.proxy_running
    }

    pub(crate) fn write_codex(&self) -> bool {
        self.write_codex
    }
}

fn provider_profile_revision(profile: &ProviderProfile) -> Result<String, String> {
    let bytes = serde_json::to_vec(profile)
        .map_err(|_| "无法验证 Provider profile 修订。".to_string())?;
    use sha2::Digest as _;
    Ok(format!("{:x}", sha2::Sha256::digest(bytes)))
}

fn active_provider_edit_revision(
    current: &ProviderProfile,
    updated: &ProviderProfile,
) -> Result<String, String> {
    let bytes = serde_json::to_vec(&("single", current, updated))
        .map_err(|_| "无法验证 Provider edit 修订。".to_string())?;
    use sha2::Digest as _;
    Ok(format!("{:x}", sha2::Sha256::digest(bytes)))
}

fn active_provider_group_edit_revision(
    group: &str,
    current_group: &[ProviderProfile],
    current: &ProviderProfile,
    updated: &ProviderProfile,
    updated_group: &[ProviderProfile],
) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(
        "group",
        group,
        current_group,
        current,
        updated,
        updated_group,
    ))
    .map_err(|_| "无法验证 Provider group edit 修订。".to_string())?;
    use sha2::Digest as _;
    Ok(format!("{:x}", sha2::Sha256::digest(bytes)))
}

fn active_aggregate_member_edit_revision(
    config: &crate::aggregate_api::AggregateApiConfig,
    current_members: &[ProviderProfile],
    current: &ProviderProfile,
    updated: &ProviderProfile,
    updated_members: &[ProviderProfile],
) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(
        "aggregate",
        config,
        current_members,
        current,
        updated,
        updated_members,
    ))
    .map_err(|_| "无法验证 Aggregate member edit 修订。".to_string())?;
    use sha2::Digest as _;
    Ok(format!("{:x}", sha2::Sha256::digest(bytes)))
}

fn provider_profiles_revision(providers: &[ProviderProfile]) -> Result<String, String> {
    let bytes = serde_json::to_vec(providers)
        .map_err(|_| "无法验证 Provider group profiles。".to_string())?;
    use sha2::Digest as _;
    Ok(format!("{:x}", sha2::Sha256::digest(bytes)))
}

pub(crate) fn prepare_active_provider_edit_context<R: Runtime>(
    app: &tauri::AppHandle<R>,
    updated_profile: ProviderProfile,
    expected_revision: Option<&str>,
) -> Result<ActiveProviderEditContext, String> {
    let paths = resolve_paths(app)?;
    let original_state = read_state(&paths);
    if original_state.active_account_id.is_some()
        || original_state.concurrent_account_routing_enabled
    {
        return Err("Provider edit found an inconsistent official-account route".to_string());
    }
    let original_profile = read_provider(&paths, &updated_profile.id)?;
    validate_provider_activation(&updated_profile)?;
    let (prepared_route, revision) = if let Some(group) = original_state.active_provider_group.as_ref()
    {
        if original_state.active_provider_id.is_some() {
            return Err("Provider edit found multiple active routes".to_string());
        }
        if original_profile.group != *group && updated_profile.group != *group {
            return Err("Provider no longer affects the active Provider group".to_string());
        }
        let current_group = provider_group_profiles(&paths, group)?;
        let mut updated_group = current_group
            .iter()
            .filter(|provider| provider.id != updated_profile.id)
            .cloned()
            .collect::<Vec<_>>();
        if updated_profile.group == *group {
            if updated_profile.kind != ProviderKind::Custom {
                return Err("Only third-party Providers can belong to an active group".to_string());
            }
            updated_group.push(updated_profile.clone());
        }
        updated_group.sort_by(|left, right| left.id.cmp(&right.id));
        if updated_group.is_empty() {
            return Err("The active Provider group must keep at least one API".to_string());
        }
        validate_provider_group_models(&updated_group)?;
        for provider in &updated_group {
            validate_provider_activation(provider)?;
        }
        let revision = active_provider_group_edit_revision(
            group,
            &current_group,
            &original_profile,
            &updated_profile,
            &updated_group,
        )?;
        (
            PreparedActiveProviderEditRoute::Group {
                group: group.clone(),
                providers: updated_group,
            },
            revision,
        )
    } else if let Some(active_id) = original_state
        .active_provider_id
        .as_deref()
        .filter(|id| crate::aggregate_api::is_active_id(id))
    {
        let config = crate::aggregate_api::read_active_config(&paths, active_id)?;
        let current_members = crate::aggregate_api::member_profiles(&paths, &config)?;
        if !current_members
            .iter()
            .any(|provider| provider.id == updated_profile.id)
        {
            return Err("Provider is no longer a member of the active Aggregate API".to_string());
        }
        let updated_members = current_members
            .iter()
            .map(|provider| {
                if provider.id == updated_profile.id {
                    updated_profile.clone()
                } else {
                    provider.clone()
                }
            })
            .collect::<Vec<_>>();
        crate::aggregate_api::validate_member_profiles(&config.model, &updated_members)?;
        for provider in &updated_members {
            validate_provider_activation(provider)?;
        }
        let revision = active_aggregate_member_edit_revision(
            &config,
            &current_members,
            &original_profile,
            &updated_profile,
            &updated_members,
        )?;
        (
            PreparedActiveProviderEditRoute::Aggregate {
                config: Box::new(config),
                providers: updated_members,
            },
            revision,
        )
    } else if original_state.active_provider_id.as_deref() == Some(&updated_profile.id)
        && !crate::aggregate_api::is_active_id(&updated_profile.id)
    {
        (
            PreparedActiveProviderEditRoute::Single,
            active_provider_edit_revision(&original_profile, &updated_profile)?,
        )
    } else {
        return Err("Provider is no longer part of the active editable route".to_string());
    };
    if expected_revision.is_some_and(|expected| expected != revision) {
        return Err("Provider profile 在预览后发生变化，未修改任何文件。请重新预览。".to_string());
    }
    Ok(ActiveProviderEditContext {
        paths,
        original_state,
        original_profile,
        updated_profile,
        prepared_route,
        revision,
        proxy_running: crate::local_proxy::is_running(),
        write_codex: crate::claude_code::should_write_codex_for_app(app)?,
    })
}

pub(crate) fn apply_active_provider_edit_files<R: Runtime>(
    app: &tauri::AppHandle<R>,
    context: &ActiveProviderEditContext,
) -> Result<(), String> {
    capacity_desktop_service::invalidate_account_binding(app);
    write_active_provider_profile_for_transaction(
        &context.paths,
        &context.updated_profile,
        &context.original_profile,
    )?;
    if context.write_codex {
        match &context.prepared_route {
            PreparedActiveProviderEditRoute::Single => {
                write_active_provider_config(&context.paths, &context.updated_profile)?;
                if uses_upstream_official_models(&context.updated_profile) {
                    let catalog = provider_model_catalog_path(&context.paths);
                    if catalog.exists() {
                        fs::remove_file(catalog).map_err(|_| {
                            "Failed to clear stale Provider model catalog".to_string()
                        })?;
                    }
                }
            }
            PreparedActiveProviderEditRoute::Group { group, providers } => {
                write_provider_group_local_proxy_config(&context.paths, group, providers)?;
            }
            PreparedActiveProviderEditRoute::Aggregate { config, providers } => {
                let profile = crate::aggregate_api::logical_profile(config, providers)?;
                write_provider_local_proxy_config(&context.paths, &profile)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn verify_active_provider_edit_postcondition(
    context: &ActiveProviderEditContext,
) -> Result<(), String> {
    let state = read_state(&context.paths);
    let route_matches = match &context.prepared_route {
        PreparedActiveProviderEditRoute::Single => {
            state.active_provider_group.is_none()
                && state.active_provider_id.as_deref() == Some(&context.updated_profile.id)
        }
        PreparedActiveProviderEditRoute::Group { group, .. } => {
            state.active_provider_id.is_none()
                && state.active_provider_group.as_deref() == Some(group)
        }
        PreparedActiveProviderEditRoute::Aggregate { config, .. } => {
            state.active_provider_group.is_none()
                && state
                    .active_provider_id
                    .as_deref()
                    .and_then(crate::aggregate_api::config_id_from_active_id)
                    == Some(config.id.as_str())
        }
    };
    if !route_matches || state.active_account_id.is_some() || state.concurrent_account_routing_enabled
    {
        return Err("active Provider route changed during profile edit".to_string());
    }
    let current = read_provider(&context.paths, &context.updated_profile.id)?;
    if provider_profile_revision(&current)? != provider_profile_revision(&context.updated_profile)? {
        return Err("saved Provider profile does not match the confirmed edit".to_string());
    }
    if !provider_field_modified_at_path(&context.paths, &context.updated_profile.id).exists() {
        return Err("Provider field metadata was not persisted".to_string());
    }
    if let PreparedActiveProviderEditRoute::Group { group, providers } = &context.prepared_route {
        let current_group = provider_group_profiles(&context.paths, group)?;
        if provider_profiles_revision(&current_group)? != provider_profiles_revision(providers)? {
            return Err("active Provider group members do not match the confirmed edit".to_string());
        }
    }
    if let PreparedActiveProviderEditRoute::Aggregate { config, providers } =
        &context.prepared_route
    {
        let current_members = crate::aggregate_api::member_profiles(&context.paths, config)?;
        if provider_profiles_revision(&current_members)? != provider_profiles_revision(providers)? {
            return Err("active Aggregate members do not match the confirmed edit".to_string());
        }
    }
    if !context.write_codex {
        return Ok(());
    }
    let config = fs::read_to_string(&context.paths.current_config)
        .map_err(|_| "Codex config is unavailable after Provider edit".to_string())?;
    if !codex_config::contains_local_proxy(&config) {
        return Err("local proxy config is not active after Provider edit".to_string());
    }
    let catalog_required = match &context.prepared_route {
        PreparedActiveProviderEditRoute::Single => {
            !uses_upstream_official_models(&context.updated_profile)
        }
        PreparedActiveProviderEditRoute::Group { .. } => true,
        PreparedActiveProviderEditRoute::Aggregate { .. } => true,
    };
    if provider_model_catalog_path(&context.paths).exists() != catalog_required {
        return Err("generated Provider model catalog does not match the edited profile".to_string());
    }
    Ok(())
}

pub(crate) fn finalize_active_provider_edit<R: Runtime>(
    app: &tauri::AppHandle<R>,
    context: &ActiveProviderEditContext,
) -> (ProviderSummary, Vec<String>) {
    let mut warnings = Vec::new();
    if context.write_codex {
        match &context.prepared_route {
            PreparedActiveProviderEditRoute::Single => {
                refresh_codex_models_now_best_effort(&context.paths, &context.updated_profile);
            }
            PreparedActiveProviderEditRoute::Group { providers, .. } => {
                refresh_codex_group_models_now_best_effort(&context.paths, providers);
            }
            PreparedActiveProviderEditRoute::Aggregate { config, providers } => {
                match crate::aggregate_api::logical_profile(config, providers) {
                    Ok(profile) => {
                        refresh_codex_models_now_best_effort(&context.paths, &profile);
                    }
                    Err(error) => warnings.push(format!(
                        "aggregate model refresh preparation failed: {error}"
                    )),
                }
            }
        }
    }
    if let Err(error) = crate::claude_code::sync_after_switch(app) {
        warnings.push(format!("third-party app sync failed: {error}"));
    }
    if let Err(error) = emit_providers_changed(app) {
        warnings.push(format!("providers event failed: {error}"));
    }
    let remains_active = match &context.prepared_route {
        PreparedActiveProviderEditRoute::Single => true,
        PreparedActiveProviderEditRoute::Group { group, .. } => {
            context.updated_profile.group == *group
        }
        PreparedActiveProviderEditRoute::Aggregate { .. } => false,
    };
    let summary = provider_summary(
        &context.updated_profile,
        remains_active,
        context.original_state.auto_switch_provider_id.as_deref()
            == Some(&context.updated_profile.id),
    );
    (summary, warnings)
}

fn active_aggregate_config_edit_revision(
    original: &crate::aggregate_api::AggregateApiConfig,
    updated: &crate::aggregate_api::AggregateApiConfig,
    original_members: &[ProviderProfile],
    updated_members: &[ProviderProfile],
) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(
        "aggregate-config",
        original,
        updated,
        original_members,
        updated_members,
    ))
    .map_err(|_| "无法验证 Aggregate config edit 修订。".to_string())?;
    use sha2::Digest as _;
    Ok(format!("{:x}", sha2::Sha256::digest(bytes)))
}

pub(crate) fn prepare_active_aggregate_edit_context<R: Runtime>(
    app: &tauri::AppHandle<R>,
    updated_config: crate::aggregate_api::AggregateApiConfig,
    expected_revision: Option<&str>,
) -> Result<ActiveAggregateEditContext, String> {
    let paths = resolve_paths(app)?;
    let original_state = read_state(&paths);
    if original_state.active_provider_group.is_some()
        || original_state.active_account_id.is_some()
        || original_state.concurrent_account_routing_enabled
        || original_state
            .active_provider_id
            .as_deref()
            .and_then(crate::aggregate_api::config_id_from_active_id)
            != Some(updated_config.id.as_str())
    {
        return Err("Aggregate API is no longer the exact active route".to_string());
    }
    let original_config = crate::aggregate_api::read_config(&paths, &updated_config.id)?;
    if !original_config.enabled {
        return Err("The active Aggregate API has an inconsistent disabled config".to_string());
    }
    let original_members = crate::aggregate_api::member_profiles(&paths, &original_config)?;
    let updated_members = crate::aggregate_api::member_profiles(&paths, &updated_config)?;
    if updated_config.enabled {
        for provider in &updated_members {
            validate_provider_activation(provider)?;
        }
    }
    let revision = active_aggregate_config_edit_revision(
        &original_config,
        &updated_config,
        &original_members,
        &updated_members,
    )?;
    if expected_revision.is_some_and(|expected| expected != revision) {
        return Err("Aggregate config 在预览后发生变化，未修改任何文件。请重新预览。".to_string());
    }
    Ok(ActiveAggregateEditContext {
        paths,
        original_state,
        original_config,
        updated_config,
        updated_members,
        revision,
        proxy_running: crate::local_proxy::is_running(),
        write_codex: crate::claude_code::should_write_codex_for_app(app)?,
    })
}

pub(crate) fn apply_active_aggregate_edit_files<R: Runtime>(
    app: &tauri::AppHandle<R>,
    context: &ActiveAggregateEditContext,
) -> Result<(), String> {
    capacity_desktop_service::invalidate_account_binding(app);
    apply_active_aggregate_edit_files_without_binding(context)
}

fn apply_active_aggregate_edit_files_without_binding(
    context: &ActiveAggregateEditContext,
) -> Result<(), String> {
    crate::aggregate_api::write_active_config_for_transaction(
        &context.paths,
        &context.original_config,
        &context.updated_config,
    )?;
    if context.updated_config.enabled {
        if context.write_codex {
            let profile = crate::aggregate_api::logical_profile(
                &context.updated_config,
                &context.updated_members,
            )?;
            write_provider_local_proxy_config(&context.paths, &profile)?;
        }
    } else {
        let mut state = context.original_state.clone();
        state.active_provider_id = None;
        state.active_provider_group = None;
        state.concurrent_account_routing_enabled = false;
        write_state(&context.paths, &state)?;
        if context.write_codex {
            if context.proxy_running {
                write_official_local_proxy_config(&context.paths)?;
            } else {
                restore_official_config(&context.paths)?;
            }
            let catalog = provider_model_catalog_path(&context.paths);
            if catalog.exists() {
                fs::remove_file(catalog).map_err(|_| {
                    "Failed to clear generated Aggregate model catalog".to_string()
                })?;
            }
        }
    }
    Ok(())
}

pub(crate) fn verify_active_aggregate_edit_postcondition(
    context: &ActiveAggregateEditContext,
) -> Result<(), String> {
    let current_config =
        crate::aggregate_api::read_config(&context.paths, &context.updated_config.id)?;
    if current_config != context.updated_config {
        return Err("Aggregate config does not match the confirmed edit".to_string());
    }
    let state = read_state(&context.paths);
    let route_matches = if context.updated_config.enabled {
        state.active_provider_group.is_none()
            && state
                .active_provider_id
                .as_deref()
                .and_then(crate::aggregate_api::config_id_from_active_id)
                == Some(context.updated_config.id.as_str())
    } else {
        state.active_provider_id.is_none() && state.active_provider_group.is_none()
    };
    if !route_matches
        || state.active_account_id.is_some()
        || state.concurrent_account_routing_enabled
    {
        return Err("Aggregate route does not match the confirmed config edit".to_string());
    }
    let current_members =
        crate::aggregate_api::member_profiles(&context.paths, &context.updated_config)?;
    if provider_profiles_revision(&current_members)?
        != provider_profiles_revision(&context.updated_members)?
    {
        return Err("Aggregate members do not match the confirmed config edit".to_string());
    }
    if !context.write_codex {
        return Ok(());
    }
    let config = fs::read_to_string(&context.paths.current_config)
        .map_err(|_| "Codex config is unavailable after Aggregate edit".to_string())?;
    if !context.updated_config.enabled && !context.proxy_running {
        if codex_config::contains_local_proxy(&config) || context.paths.config_backup.exists() {
            return Err("direct official config still contains Aggregate state".to_string());
        }
    } else if !codex_config::contains_local_proxy(&config) {
        return Err("local proxy config is not active after Aggregate edit".to_string());
    }
    if provider_model_catalog_path(&context.paths).exists() != context.updated_config.enabled {
        return Err("generated model catalog does not match the Aggregate state".to_string());
    }
    Ok(())
}

pub(crate) fn finalize_active_aggregate_edit<R: Runtime>(
    app: &tauri::AppHandle<R>,
    context: &ActiveAggregateEditContext,
) -> (crate::aggregate_api::AggregateApiSummary, Vec<String>) {
    let mut warnings = Vec::new();
    if context.write_codex {
        if context.updated_config.enabled {
            match crate::aggregate_api::logical_profile(
                &context.updated_config,
                &context.updated_members,
            ) {
                Ok(profile) => refresh_codex_models_now_best_effort(&context.paths, &profile),
                Err(error) => warnings.push(format!(
                    "Aggregate model refresh preparation failed: {error}"
                )),
            }
        } else {
            refresh_codex_models_for_current_target(&context.paths);
        }
    }
    if let Err(error) = crate::claude_code::sync_after_switch(app) {
        warnings.push(format!("third-party app sync failed: {error}"));
    }
    let (summary, aggregate_warnings) =
        crate::aggregate_api::finalize_active_config_edit(app, context.updated_config.clone());
    warnings.extend(aggregate_warnings);
    (summary, warnings)
}

fn provider_runtime_selection(
    state: &crate::models::ManagerStateFile,
) -> ProviderRuntimeSelection {
    if let Some(group) = state.active_provider_group.as_ref() {
        ProviderRuntimeSelection::Group(group.clone())
    } else if let Some(id) = state.active_provider_id.as_ref() {
        match crate::aggregate_api::config_id_from_active_id(id) {
            Some(id) => ProviderRuntimeSelection::Aggregate(id.to_string()),
            None => ProviderRuntimeSelection::Provider(id.clone()),
        }
    } else {
        ProviderRuntimeSelection::Official
    }
}

pub(crate) fn ensure_provider_edit_is_inactive(
    paths: &Paths,
    provider_id: &str,
    previous_group: Option<&str>,
    next_group: &str,
) -> Result<(), String> {
    let state = read_state(paths);
    if state.active_provider_id.as_deref() == Some(provider_id) {
        return Err("Stop the active Provider before editing it".to_string());
    }
    let active_group = state.active_provider_group.as_deref();
    let changes_active_group = active_group.is_some_and(|active| {
        (!next_group.is_empty() && active == next_group)
            || previous_group.is_some_and(|previous| !previous.is_empty() && active == previous)
    });
    if changes_active_group {
        return Err("Stop the active Provider group before editing one of its APIs".to_string());
    }
    if crate::aggregate_api::active_aggregate_contains_provider(paths, provider_id)? {
        return Err("Stop the active Aggregate API before editing one of its APIs".to_string());
    }
    Ok(())
}

fn provider_target_revision(
    prepared: &PreparedProviderMutationTarget,
) -> Result<String, String> {
    let revision_bytes = match prepared {
        PreparedProviderMutationTarget::Provider(provider) => serde_json::to_vec(provider),
        PreparedProviderMutationTarget::Group { group, providers } => {
            serde_json::to_vec(&(group, providers))
        }
        PreparedProviderMutationTarget::Aggregate { config, providers } => {
            serde_json::to_vec(&(config, providers))
        }
        PreparedProviderMutationTarget::Official => serde_json::to_vec(&"official"),
    }
    .map_err(|_| "无法验证 Provider 配置修订。".to_string())?;
    use sha2::Digest as _;
    Ok(format!("{:x}", sha2::Sha256::digest(revision_bytes)))
}

pub(crate) fn prepare_provider_mutation_context<R: Runtime>(
    app: &tauri::AppHandle<R>,
    target: &ProviderMutationTarget,
    expected_current: Option<&ProviderRuntimeSelection>,
) -> Result<ProviderMutationContext, String> {
    let paths = resolve_paths(app)?;
    let original_state = read_state(&paths);
    let current = provider_runtime_selection(&original_state);
    if expected_current.is_some_and(|expected| expected != &current) {
        return Err("Provider 状态在预览后发生变化，未修改任何文件。请重新预览。".to_string());
    }
    let (target, target_label, prepared) = match target {
        ProviderMutationTarget::Provider(id) => {
            let provider = read_provider(&paths, id)?;
            validate_provider_activation(&provider)?;
            (
                ProviderRuntimeSelection::Provider(provider.id.clone()),
                provider.name.clone(),
                PreparedProviderMutationTarget::Provider(Box::new(provider)),
            )
        }
        ProviderMutationTarget::Group(group) => {
            let providers = provider_group_profiles(&paths, group)?;
            for provider in &providers {
                validate_provider_activation(provider)?;
            }
            let group = providers
                .first()
                .map(|provider| provider.group.clone())
                .ok_or_else(|| "Provider group does not contain any available APIs".to_string())?;
            (
                ProviderRuntimeSelection::Group(group.clone()),
                group.clone(),
                PreparedProviderMutationTarget::Group { group, providers },
            )
        }
        ProviderMutationTarget::Aggregate(id) => {
            let config = crate::aggregate_api::read_config(&paths, id)?;
            if !config.enabled {
                return Err("Enable the aggregate API before using it".to_string());
            }
            let providers = crate::aggregate_api::member_profiles(&paths, &config)?;
            for provider in &providers {
                validate_provider_activation(provider)?;
            }
            let target_id = config.id.clone();
            let target_label = config.name.clone();
            (
                ProviderRuntimeSelection::Aggregate(target_id),
                target_label,
                PreparedProviderMutationTarget::Aggregate {
                    config: Box::new(config),
                    providers,
                },
            )
        }
        ProviderMutationTarget::Official => {
            if current == ProviderRuntimeSelection::Official {
                return Err("当前未启用第三方 Provider。".to_string());
            }
            (
                ProviderRuntimeSelection::Official,
                "Official Codex".to_string(),
                PreparedProviderMutationTarget::Official,
            )
        }
    };
    let target_revision = provider_target_revision(&prepared)?;
    let proxy_running = crate::local_proxy::is_running();
    let write_codex = crate::claude_code::should_write_codex_for_app(app)?;
    Ok(ProviderMutationContext {
        target,
        current,
        target_label,
        target_revision,
        proxy_running,
        write_codex,
        paths,
        original_state,
        prepared,
    })
}

pub(crate) fn switch_provider_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
) -> Result<(), String> {
    crate::commands::perform_safe_provider_mutation_blocking(
        app,
        ProviderMutationTarget::Provider(id),
        None,
        None,
        None,
    )
}

pub(crate) fn switch_provider_group_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    group: String,
) -> Result<(), String> {
    crate::commands::perform_safe_provider_mutation_blocking(
        app,
        ProviderMutationTarget::Group(group),
        None,
        None,
        None,
    )
}

pub(crate) fn switch_provider_model_and_activate_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
    model: String,
) -> Result<(), String> {
    let (provider_id, expected_current, expected_target_revision) = {
        let _mutation_guard = crate::commands::account_switch_lock()
            .lock()
            .map_err(|_| "Account and Provider switch lock is poisoned".to_string())?;
        let paths = resolve_paths(&app)?;
        let mut provider = read_provider(&paths, &id)?;
        ensure_provider_edit_is_inactive(
            &paths,
            &provider.id,
            Some(&provider.group),
            &provider.group,
        )?;
        if provider.model_selection_controlled_by_codex {
            return Err("Provider model selection is controlled within Codex".to_string());
        }
        let selected_model = require_non_empty("Model", &model)?;
        if !provider.models.iter().any(|value| value == &selected_model) {
            return Err("Provider model does not exist".to_string());
        }
        provider.model = selected_model;
        provider = normalize_provider_profile(provider)?;
        validate_provider_activation(&provider)?;
        write_local_provider(&paths, &provider, None)?;
        let provider_id = provider.id;
        let target = ProviderMutationTarget::Provider(provider_id.clone());
        let context = prepare_provider_mutation_context(&app, &target, None)?;
        (
            provider_id,
            context.current().clone(),
            context.target_revision().to_string(),
        )
    };
    crate::commands::perform_safe_provider_mutation_blocking(
        app,
        ProviderMutationTarget::Provider(provider_id),
        Some(expected_current),
        None,
        Some(expected_target_revision),
    )
}

pub(crate) fn provider_model_catalog_path(paths: &Paths) -> PathBuf {
    paths.codex_home.join(MODEL_CATALOG_FILENAME)
}

pub(crate) fn apply_provider_mutation_files<R: Runtime>(
    app: &tauri::AppHandle<R>,
    context: &ProviderMutationContext,
) -> Result<(), String> {
    capacity_desktop_service::invalidate_account_binding(app);
    apply_provider_mutation_files_without_binding(context)
}

fn apply_provider_mutation_files_without_binding(
    context: &ProviderMutationContext,
) -> Result<(), String> {
    let mut state = context.original_state.clone();
    match &context.prepared {
        PreparedProviderMutationTarget::Provider(provider) => {
            if context.write_codex {
                backup_codex_config_if_needed(
                    &context.paths,
                    context.current == ProviderRuntimeSelection::Official,
                )?;
            }
            state.active_provider_id = Some(provider.id.clone());
            state.active_provider_group = None;
            state.active_account_id = None;
            state.concurrent_account_routing_enabled = false;
            write_state(&context.paths, &state)?;
            if context.write_codex {
                write_provider_local_proxy_config(&context.paths, provider)?;
                if uses_upstream_official_models(provider) {
                    let catalog = provider_model_catalog_path(&context.paths);
                    if catalog.exists() {
                        fs::remove_file(catalog).map_err(|_| {
                            "Failed to clear stale Provider model catalog".to_string()
                        })?;
                    }
                }
            }
        }
        PreparedProviderMutationTarget::Group { group, providers } => {
            if context.write_codex {
                backup_codex_config_if_needed(
                    &context.paths,
                    context.current == ProviderRuntimeSelection::Official,
                )?;
            }
            state.active_provider_id = None;
            state.active_provider_group = Some(group.clone());
            state.active_account_id = None;
            state.concurrent_account_routing_enabled = false;
            write_state(&context.paths, &state)?;
            if context.write_codex {
                write_provider_group_local_proxy_config(&context.paths, group, providers)?;
            }
        }
        PreparedProviderMutationTarget::Aggregate { config, providers } => {
            if context.write_codex {
                backup_codex_config_if_needed(
                    &context.paths,
                    context.current == ProviderRuntimeSelection::Official,
                )?;
            }
            let profile = crate::aggregate_api::logical_profile(config, providers)?;
            state.active_provider_id = Some(profile.id.clone());
            state.active_provider_group = None;
            state.active_account_id = None;
            state.concurrent_account_routing_enabled = false;
            write_state(&context.paths, &state)?;
            if context.write_codex {
                write_provider_local_proxy_config(&context.paths, &profile)?;
            }
        }
        PreparedProviderMutationTarget::Official => {
            state.active_provider_id = None;
            state.active_provider_group = None;
            state.concurrent_account_routing_enabled = false;
            write_state(&context.paths, &state)?;
            if context.write_codex {
                if context.proxy_running {
                    write_official_local_proxy_config(&context.paths)?;
                } else {
                    restore_official_config(&context.paths)?;
                }
                let catalog = provider_model_catalog_path(&context.paths);
                if catalog.exists() {
                    fs::remove_file(catalog).map_err(|_| {
                        "Failed to clear generated Provider model catalog".to_string()
                    })?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn verify_provider_mutation_postcondition(
    context: &ProviderMutationContext,
) -> Result<(), String> {
    let state = read_state(&context.paths);
    if provider_runtime_selection(&state) != context.target {
        return Err("manager state does not match the selected Provider route".to_string());
    }
    if context.target != ProviderRuntimeSelection::Official
        && (state.active_account_id.is_some() || state.concurrent_account_routing_enabled)
    {
        return Err("Provider route retained an official account or concurrent routing".to_string());
    }
    if !context.write_codex {
        return Ok(());
    }
    let config = fs::read_to_string(&context.paths.current_config)
        .map_err(|_| "Codex config is unavailable after Provider mutation".to_string())?;
    if context.target == ProviderRuntimeSelection::Official && !context.proxy_running {
        if codex_config::contains_local_proxy(&config) || context.paths.config_backup.exists() {
            return Err("direct official config still contains Provider state".to_string());
        }
    } else if !codex_config::contains_local_proxy(&config) {
        return Err("local proxy config is not active after Provider mutation".to_string());
    }
    if context.target == ProviderRuntimeSelection::Official
        && provider_model_catalog_path(&context.paths).exists()
    {
        return Err("generated Provider model catalog still exists".to_string());
    }
    let catalog_required = match &context.prepared {
        PreparedProviderMutationTarget::Provider(provider) => {
            !uses_upstream_official_models(provider)
        }
        PreparedProviderMutationTarget::Group { .. } => true,
        PreparedProviderMutationTarget::Aggregate { .. } => true,
        PreparedProviderMutationTarget::Official => false,
    };
    if context.target != ProviderRuntimeSelection::Official
        && provider_model_catalog_path(&context.paths).exists() != catalog_required
    {
        return Err("generated Provider model catalog does not match the target".to_string());
    }
    Ok(())
}

pub(crate) fn finalize_provider_mutation<R: Runtime>(
    app: &tauri::AppHandle<R>,
    context: &ProviderMutationContext,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if context.write_codex {
        match &context.prepared {
            PreparedProviderMutationTarget::Provider(provider) => {
                refresh_codex_models_now_best_effort(&context.paths, provider);
            }
            PreparedProviderMutationTarget::Group { providers, .. } => {
                refresh_codex_group_models_now_best_effort(&context.paths, providers);
            }
            PreparedProviderMutationTarget::Aggregate { config, providers } => {
                match crate::aggregate_api::logical_profile(config, providers) {
                    Ok(profile) => refresh_codex_models_now_best_effort(&context.paths, &profile),
                    Err(error) => warnings.push(format!(
                        "aggregate model refresh preparation failed: {error}"
                    )),
                }
            }
            PreparedProviderMutationTarget::Official => {
                refresh_codex_models_for_current_target(&context.paths);
            }
        }
    }
    if let Err(error) = crate::claude_code::sync_after_switch(app) {
        warnings.push(format!("third-party app sync failed: {error}"));
    }
    if let Err(error) = emit_providers_changed(app) {
        warnings.push(format!("providers event failed: {error}"));
    }
    warnings
}

pub(crate) fn validate_provider_activation(provider: &ProviderProfile) -> Result<(), String> {
    ensure_not_local_proxy_base_url(&provider.base_url)?;
    ensure_local_proxy_running_for_provider()?;
    if provider.kind != ProviderKind::OpenAi
        && provider.api_key.trim().is_empty()
        && !crate::antigravity_provider::allows_missing_api_key(provider)
        && !crate::preset_provider::allows_missing_api_key(provider)
    {
        return Err("Provider API key is empty".to_string());
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn switch_provider_model<R: Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
    model: String,
) -> Result<ProviderSummary, String> {
    let _mutation_guard = crate::commands::account_switch_lock()
        .lock()
        .map_err(|_| "Account and Provider switch lock is poisoned".to_string())?;
    let paths = resolve_paths(&app)?;
    let prepared = prepare_provider_model_switch(&paths, &id, &model)?;
    ensure_provider_edit_is_inactive(
        &paths,
        &prepared.profile.id,
        prepared
            .existing
            .as_ref()
            .map(|provider| provider.group.as_str()),
        &prepared.profile.group,
    )?;
    write_local_provider(
        &paths,
        &prepared.profile,
        prepared.existing.as_ref(),
    )?;

    let state = read_state(&paths);
    emit_providers_changed(&app)?;
    Ok(provider_summary(
        &prepared.profile,
        false,
        state.auto_switch_provider_id.as_deref() == Some(&prepared.profile.id),
    ))
}

pub(crate) fn prepare_provider_model_switch(
    paths: &Paths,
    id: &str,
    model: &str,
) -> Result<PreparedProviderSave, String> {
    let existing = read_provider(paths, id)?;
    let mut profile = existing.clone();
    let selected_model = require_non_empty("Model", model)?;
    if !profile.models.iter().any(|value| value == &selected_model) {
        profile.models.push(selected_model.clone());
    }
    profile.model = selected_model;
    profile = normalize_provider_profile(profile)?;
    Ok(PreparedProviderSave {
        existing: Some(existing),
        profile,
    })
}

#[tauri::command]
pub(crate) fn set_provider_model_control<R: Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
    controlled_by_codex: bool,
) -> Result<ProviderSummary, String> {
    let _mutation_guard = crate::commands::account_switch_lock()
        .lock()
        .map_err(|_| "Account and Provider switch lock is poisoned".to_string())?;
    let paths = resolve_paths(&app)?;
    let prepared = prepare_provider_model_control(&paths, &id, controlled_by_codex)?;
    ensure_provider_edit_is_inactive(
        &paths,
        &prepared.profile.id,
        prepared
            .existing
            .as_ref()
            .map(|provider| provider.group.as_str()),
        &prepared.profile.group,
    )?;
    write_local_provider(
        &paths,
        &prepared.profile,
        prepared.existing.as_ref(),
    )?;

    let state = read_state(&paths);
    emit_providers_changed(&app)?;
    Ok(provider_summary(
        &prepared.profile,
        false,
        state.auto_switch_provider_id.as_deref() == Some(&prepared.profile.id),
    ))
}

pub(crate) fn prepare_provider_model_control(
    paths: &Paths,
    id: &str,
    controlled_by_codex: bool,
) -> Result<PreparedProviderSave, String> {
    let existing = read_provider(paths, id)?;
    let mut profile = existing.clone();
    profile.model_selection_controlled_by_codex = controlled_by_codex;
    profile = normalize_provider_profile(profile)?;
    Ok(PreparedProviderSave {
        existing: Some(existing),
        profile,
    })
}

#[tauri::command]
pub(crate) fn set_provider_auto_switch_enabled<R: Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let paths = resolve_paths(&app)?;
    let provider = read_provider(&paths, &id)?;
    if provider.kind != ProviderKind::Custom {
        return Err("Automatic fallback is only available for third-party Providers".to_string());
    }

    let mut state = read_state(&paths);
    let next_provider_id = if enabled { Some(id.clone()) } else { None };
    if enabled || state.auto_switch_provider_id.as_deref() == Some(&id) {
        state.auto_switch_provider_id = next_provider_id;
        write_state(&paths, &state)?;
        emit_providers_changed(&app)?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn delete_provider<R: Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
) -> Result<(), String> {
    let _mutation_guard = crate::commands::account_switch_lock()
        .lock()
        .map_err(|_| "Account and Provider switch lock is poisoned".to_string())?;
    let paths = resolve_paths(&app)?;
    validate_provider_id(&id)?;
    let original_state = read_state(&paths);
    let provider = read_provider(&paths, &id).ok();
    ensure_provider_edit_is_inactive(
        &paths,
        &id,
        provider.as_ref().map(|provider| provider.group.as_str()),
        "",
    )?;
    let was_auto_switch_provider = original_state.auto_switch_provider_id.as_deref() == Some(&id);
    if was_auto_switch_provider {
        let mut state = original_state.clone();
        state.auto_switch_provider_id = None;
        write_state(&paths, &state)?;
    }
    let mut image_state = read_state(&paths);
    let cleared_image_input = target_uses_provider(image_state.image_input_target.as_ref(), &id);
    let cleared_image_output = target_uses_provider(image_state.image_output_target.as_ref(), &id);
    if cleared_image_input {
        image_state.image_input_target = None;
    }
    if cleared_image_output {
        image_state.image_output_target = None;
    }
    if cleared_image_input || cleared_image_output {
        write_state(&paths, &image_state)?;
    }
    let path = provider_path(&paths, &id);
    if path.exists() {
        fs::remove_file(&path).map_err(|error| format!("Failed to delete provider: {error}"))?;
    }
    crate::aggregate_api::remove_provider_membership(&paths, &id)?;
    let versions_path = provider_field_modified_at_path(&paths, &id);
    if versions_path.exists() {
        fs::remove_file(&versions_path)
            .map_err(|error| format!("Failed to delete provider field versions: {error}"))?;
    }
    emit_providers_changed(&app)?;
    Ok(())
}
