#[tauri::command]
pub(crate) async fn set_provider_group<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    id: String,
    group: String,
) -> Result<ProviderSummary, String> {
    tauri::async_runtime::spawn_blocking(move || set_provider_group_blocking(app, id, group))
        .await
        .map_err(|error| format!("Provider group update task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn set_provider_groups<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    groups: Vec<String>,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || set_provider_groups_blocking(app, groups))
        .await
        .map_err(|error| format!("Provider group catalog update task failed: {error}"))?
}

fn set_provider_groups_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    groups: Vec<String>,
) -> Result<Vec<String>, String> {
    let groups = normalize_provider_groups(groups)?;
    let mut settings = read_app_settings(&app)?;
    settings.provider_groups.clone_from(&groups);
    write_app_settings(&app, &settings)?;
    Ok(groups)
}

fn set_provider_group_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
    group: String,
) -> Result<ProviderSummary, String> {
    let _mutation_guard = crate::commands::account_switch_lock()
        .lock()
        .map_err(|_| "Account and Provider switch lock is poisoned".to_string())?;
    let paths = resolve_paths(&app)?;
    let prepared = prepare_provider_group_update(&paths, &id, &group)?;
    let state = read_state(&paths);
    ensure_provider_edit_is_inactive(
        &paths,
        &prepared.profile.id,
        prepared
            .existing
            .as_ref()
            .map(|provider| provider.group.as_str()),
        &prepared.profile.group,
    )?;
    write_local_provider(&paths, &prepared.profile, prepared.existing.as_ref())?;
    emit_providers_changed(&app)?;
    Ok(provider_summary(
        &prepared.profile,
        false,
        state.auto_switch_provider_id.as_deref() == Some(&prepared.profile.id),
    ))
}

pub(crate) fn prepare_provider_group_update(
    paths: &Paths,
    id: &str,
    group: &str,
) -> Result<PreparedProviderSave, String> {
    let existing = read_provider(paths, id)?;
    if existing.kind != ProviderKind::Custom {
        return Err("Only third-party Providers can be grouped".to_string());
    }
    let mut profile = existing.clone();
    profile.group = normalize_provider_group(group)?;
    profile = normalize_provider_profile(profile)?;
    Ok(PreparedProviderSave {
        existing: Some(existing),
        profile,
    })
}

pub(crate) fn provider_group_profiles(
    paths: &Paths,
    group: &str,
) -> Result<Vec<ProviderProfile>, String> {
    let group = normalize_provider_group(group)?;
    if group.is_empty() {
        return Err("Select a Provider group".to_string());
    }
    let providers = list_provider_profiles(paths)?
        .into_iter()
        .filter(|provider| provider.kind == ProviderKind::Custom && provider.group == group)
        .collect::<Vec<_>>();
    if providers.is_empty() {
        return Err("Provider group does not contain any available APIs".to_string());
    }
    validate_provider_group_models(&providers)?;
    Ok(providers)
}

fn validate_provider_group_models(providers: &[ProviderProfile]) -> Result<(), String> {
    let mut names = HashSet::new();
    for provider in providers {
        for model in group_visible_models(provider) {
            let name = provider_group_model_name(provider, model);
            if !names.insert(name) {
                return Err(
                    "APIs in a Provider group must have unique API and model names".to_string(),
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn provider_group_model_name(provider: &ProviderProfile, model: &str) -> String {
    format!("{}-{model}", provider.name)
}

pub(crate) fn provider_for_group_model(
    providers: &[ProviderProfile],
    requested_model: Option<&str>,
) -> Result<ProviderProfile, String> {
    let matching_provider = |requested: &str| {
        providers.iter().find_map(|provider| {
            group_visible_models(provider)
                .into_iter()
                .find(|model| provider_group_model_name(provider, model) == requested)
                .map(|model| (provider, model.to_string()))
        })
    };
    let selected = match requested_model {
        Some(requested) => matching_provider(requested).ok_or_else(|| {
            "The selected model is not available in this Provider group".to_string()
        })?,
        None => providers
            .first()
            .and_then(|provider| {
                group_visible_models(provider)
                    .first()
                    .map(|model| (provider, (*model).to_string()))
            })
            .ok_or_else(|| "Provider group does not contain any models".to_string())?,
    };
    let (provider, model) = selected;
    let mut selected_provider = provider.clone();
    selected_provider.model = model;
    selected_provider.model_selection_controlled_by_codex = false;
    Ok(selected_provider)
}

fn group_visible_models(provider: &ProviderProfile) -> Vec<&str> {
    if provider.model_selection_controlled_by_codex {
        provider.models.iter().map(String::as_str).collect()
    } else {
        vec![provider.model.as_str()]
    }
}

pub(crate) fn read_provider(paths: &Paths, id: &str) -> Result<ProviderProfile, String> {
    validate_provider_id(id)?;
    read_provider_file(provider_path(paths, id))
}

fn read_provider_file(path: PathBuf) -> Result<ProviderProfile, String> {
    let profile = crate::provider_credentials::read_provider_profile(&path)?;
    normalize_provider_profile(profile)
        .map_err(|error| format!("Provider profile {} is invalid: {error}", path.display()))
}

fn write_provider(paths: &Paths, provider: &ProviderProfile) -> Result<(), String> {
    crate::provider_credentials::write_provider_profile(
        &provider_path(paths, &provider.id),
        provider,
    )
}

fn provider_field_values(provider: &ProviderProfile) -> Vec<serde_json::Value> {
    vec![
        json!(provider.kind),
        json!(provider.name),
        json!(provider.group),
        json!(provider.base_url),
        json!(provider.api_key),
        json!(provider.model),
        json!(provider.models),
        json!(provider.model_reasoning_efforts),
        json!(provider.model_context_windows),
        json!(provider.model_api_formats),
        json!({
            "models": provider.image_input_models,
            "configured": provider.image_input_models_configured,
        }),
        json!(provider.context_window),
        json!(provider.model_selection_controlled_by_codex),
        json!(provider.api_format),
        json!(provider.balance_platform),
        json!(provider.balance_query_url),
        json!(provider.balance_query_token),
        json!(provider.wallet_query_url),
        json!(provider.wallet_query_token),
        json!(provider.wallet_username),
        json!(provider.wallet_password),
    ]
}

fn provider_field_versions_mut(values: &mut ProviderFieldModifiedAt) -> [&mut String; 21] {
    [
        &mut values.kind,
        &mut values.name,
        &mut values.group,
        &mut values.base_url,
        &mut values.api_key,
        &mut values.model,
        &mut values.models,
        &mut values.model_reasoning_efforts,
        &mut values.model_context_windows,
        &mut values.model_api_formats,
        &mut values.image_input_models,
        &mut values.context_window,
        &mut values.model_selection_controlled_by_codex,
        &mut values.api_format,
        &mut values.balance_platform,
        &mut values.balance_query_url,
        &mut values.balance_query_token,
        &mut values.wallet_query_url,
        &mut values.wallet_query_token,
        &mut values.wallet_username,
        &mut values.wallet_password,
    ]
}

pub(crate) fn load_or_init_provider_field_modified_at(
    paths: &Paths,
    id: &str,
) -> Result<ProviderFieldModifiedAt, String> {
    let fallback = provider_modified_at(paths, id)
        .unwrap_or_else(|_| chrono::Utc::now())
        .to_rfc3339();
    let path = provider_field_modified_at_path(paths, id);
    let mut values = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut changed = false;
    for value in provider_field_versions_mut(&mut values) {
        if value.trim().is_empty() {
            *value = fallback.clone();
            changed = true;
        }
    }
    if changed {
        save_provider_field_modified_at(paths, id, &values)?;
    }
    Ok(values)
}

pub(crate) fn save_provider_field_modified_at(
    paths: &Paths,
    id: &str,
    values: &ProviderFieldModifiedAt,
) -> Result<(), String> {
    write_json_atomic(
        &provider_field_modified_at_path(paths, id),
        &serde_json::to_value(values).map_err(|error| error.to_string())?,
    )
}

pub(crate) fn new_imported_provider_field_metadata() -> ProviderFieldModifiedAt {
    let mut values = ProviderFieldModifiedAt::default();
    let now = chrono::Utc::now().to_rfc3339();
    for field in provider_field_versions_mut(&mut values) {
        field.clone_from(&now);
    }
    values
}

fn write_local_provider(
    paths: &Paths,
    provider: &ProviderProfile,
    known_existing: Option<&ProviderProfile>,
) -> Result<(), String> {
    let existing = known_existing
        .cloned()
        .or_else(|| read_provider(paths, &provider.id).ok());
    ensure_provider_edit_is_inactive(
        paths,
        &provider.id,
        existing.as_ref().map(|profile| profile.group.as_str()),
        &provider.group,
    )?;
    write_local_provider_unchecked(paths, provider, existing.as_ref())
}

pub(crate) fn write_active_provider_profile_for_transaction(
    paths: &Paths,
    provider: &ProviderProfile,
    existing: &ProviderProfile,
) -> Result<(), String> {
    if provider.id != existing.id {
        return Err("Safe Provider edit target changed".to_string());
    }
    write_local_provider_unchecked(paths, provider, Some(existing))
}

fn write_local_provider_unchecked(
    paths: &Paths,
    provider: &ProviderProfile,
    existing: Option<&ProviderProfile>,
) -> Result<(), String> {
    let mut versions = if existing.is_some() {
        load_or_init_provider_field_modified_at(paths, &provider.id)?
    } else {
        ProviderFieldModifiedAt::default()
    };
    let now = chrono::Utc::now().to_rfc3339();
    let old_values = existing.map(provider_field_values);
    let new_values = provider_field_values(provider);
    for (index, version) in provider_field_versions_mut(&mut versions)
        .into_iter()
        .enumerate()
    {
        if old_values
            .as_ref()
            .is_none_or(|values| values[index] != new_values[index])
        {
            *version = now.clone();
        }
    }
    write_provider(paths, provider)?;
    save_provider_field_modified_at(paths, &provider.id, &versions)
}

pub(crate) fn write_synced_provider(
    paths: &Paths,
    provider: ProviderProfile,
    field_modified_at: &ProviderFieldModifiedAt,
) -> Result<ProviderProfile, String> {
    let _mutation_guard = crate::commands::account_switch_lock()
        .lock()
        .map_err(|_| "Account and Provider switch lock is poisoned".to_string())?;
    let profile = normalize_synced_provider(provider)?;
    let existing = read_provider(paths, &profile.id).ok();
    ensure_provider_edit_is_inactive(
        paths,
        &profile.id,
        existing.as_ref().map(|current| current.group.as_str()),
        &profile.group,
    )?;
    write_provider(paths, &profile)?;
    save_provider_field_modified_at(paths, &profile.id, field_modified_at)?;
    Ok(profile)
}

pub(crate) fn provider_modified_at(
    paths: &Paths,
    id: &str,
) -> Result<chrono::DateTime<chrono::Utc>, String> {
    let path = provider_path(paths, id);
    fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .map(chrono::DateTime::<chrono::Utc>::from)
        .map_err(|error| {
            format!(
                "Failed to read provider modified time {}: {error}",
                path.display()
            )
        })
}

fn provider_summary(
    provider: &ProviderProfile,
    active: bool,
    auto_switch_enabled: bool,
) -> ProviderSummary {
    ProviderSummary {
        id: provider.id.clone(),
        kind: provider.kind,
        name: provider.name.clone(),
        group: provider.group.clone(),
        base_url: provider.base_url.clone(),
        model: provider.model.clone(),
        models: provider.models.clone(),
        model_reasoning_efforts: provider.model_reasoning_efforts.clone(),
        model_context_windows: provider.model_context_windows.clone(),
        model_api_formats: provider.model_api_formats.clone(),
        image_input_models: provider.image_input_models.clone(),
        image_input_models_configured: provider.image_input_models_configured,
        context_window: provider.context_window,
        model_selection_controlled_by_codex: provider.model_selection_controlled_by_codex,
        api_format: provider.api_format,
        active,
        auto_switch_enabled: auto_switch_enabled && provider.kind == ProviderKind::Custom,
        has_api_key: !provider.api_key.trim().is_empty(),
        supports_direct_switch: provider_switch_supported(crate::local_proxy::is_running()),
        balance_platform: provider.balance_platform,
        balance_query_url: provider.balance_query_url.clone(),
        balance_query_uses_api_key: provider.balance_query_token.is_none(),
        has_balance_query_token: provider
            .balance_query_token
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        wallet_query_url: provider.wallet_query_url.clone(),
        has_wallet_query_token: provider
            .wallet_query_token
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        wallet_username: provider.wallet_username.clone(),
        has_wallet_login_credentials: provider
            .wallet_username
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            && provider
                .wallet_password
                .as_deref()
                .is_some_and(|value| !value.is_empty()),
    }
}
