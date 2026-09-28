pub(crate) fn read_state(paths: &Paths) -> ManagerStateFile {
    fs::read(&paths.state_file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn write_state(paths: &Paths, state: &ManagerStateFile) -> Result<(), String> {
    let value = serde_json::to_value(state).map_err(|error| error.to_string())?;
    write_json_atomic(&paths.state_file, &value)
}

pub(crate) fn app_settings_path<R: Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法定位应用数据目录：{error}"))?
        .join("settings.json"))
}

pub(crate) fn read_app_settings<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<AppSettings, String> {
    let path = app_settings_path(app)?;
    Ok(fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default())
}

pub(crate) fn write_app_settings<R: Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &AppSettings,
) -> Result<(), String> {
    let path = app_settings_path(app)?;
    let value = serde_json::to_value(settings).map_err(|error| error.to_string())?;
    write_json_atomic(&path, &value)
}

fn apply_app_settings_version_migration(settings: &mut AppSettings, current_version: &str) -> bool {
    let mut changed = false;
    if settings
        .cloud_base_url
        .as_deref()
        .is_some_and(|value| {
            value.trim().trim_end_matches('/').eq_ignore_ascii_case(
                LEGACY_UPSTREAM_CLOUD_BASE_URL,
            )
        })
    {
        settings.cloud_base_url = None;
        settings.launch_at_startup = false;
        settings.floating_bubble_enabled = false;
        settings.cloud_user_email = None;
        settings.cloud_user_id = None;
        settings.cloud_last_sync_at = None;
        settings.cloud_session_expired = false;
        changed = true;
    }
    if settings.last_started_version.as_deref() == Some(current_version) {
        return changed;
    }
    settings.last_started_version = Some(current_version.to_string());
    true
}

pub(crate) fn migrate_app_settings_for_version<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<(), String> {
    let mut settings = read_app_settings(app)?;
    let current_version = app.package_info().version.to_string();
    if apply_app_settings_version_migration(&mut settings, &current_version) {
        write_app_settings(app, &settings)?;
    }
    Ok(())
}

fn should_activate_import(
    state: &ManagerStateFile,
    activate: bool,
    current_auth_exists: bool,
) -> bool {
    activate
        || (!current_auth_exists
            && state.active_account_id.is_none()
            && state.active_provider_id.is_none()
            && state.active_provider_group.is_none())
}

fn should_sync_current_as_active(
    state: &ManagerStateFile,
    id: &str,
    agent_identity: bool,
    proxy_running: bool,
) -> bool {
    state.active_provider_id.is_none()
        && state.active_provider_group.is_none()
        && !state.local_proxy_enabled
        && !proxy_running
        && state.active_account_id.as_deref() != Some(id)
        && !agent_identity
}

/// Explicit sign-in saves to Horizon only, including first-account onboarding.
/// Current auth/config, active routing and account notes/cache remain untouched.
pub(crate) fn save_login_account(paths: &Paths, mut auth: Value) -> Result<String, String> {
    canonicalize_chatgpt_auth(&mut auth)?;
    validate_auth(&auth)?;
    let (_, _, _, id) = account_fields(&auth)?;
    write_managed_auth_if_changed(paths, &id, &auth)?;
    Ok(id)
}

pub(crate) fn import_value<R: Runtime>(
    app: &tauri::AppHandle<R>,
    mut auth: Value,
    activate: bool,
) -> Result<String, String> {
    canonicalize_chatgpt_auth(&mut auth)?;
    validate_auth(&auth)?;
    let paths = resolve_paths(app)?;
    let (_, _, _, id) = account_fields(&auth)?;
    let mut state = read_state(&paths);
    let should_activate = should_activate_import(&state, activate, paths.current_auth.exists());
    write_managed_auth_if_changed(&paths, &id, &auth)?;
    if should_activate {
        let proxy_running = crate::local_proxy::is_running();
        let can_activate = if proxy_running {
            true
        } else if crate::auth::is_agent_identity_auth(&auth) {
            false
        } else {
            crate::commands::sync_current_auth_if_client_stopped(&paths, &auth)?
        };
        if can_activate {
            state.active_account_id = Some(id.clone());
            write_state(&paths, &state)?;
            if crate::local_proxy::is_running() {
                crate::providers::apply_local_proxy_config_for_paths(&paths)?;
            }
        }
    }
    Ok(id)
}

pub(crate) fn sync_current_into_store<R: Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let paths = resolve_paths(app)?;
    sync_current_into_store_for_paths(&paths, crate::local_proxy::is_running())
}

fn read_runtime_auth(paths: &Paths) -> Result<Value, String> {
    use std::io::Read;
    const MAX_AUTH_BYTES: u64 = 1024 * 1024;
    let mut bytes = Vec::new();
    File::open(&paths.current_auth)
        .map_err(|_| "Runtime credential unavailable".to_string())?
        .take(MAX_AUTH_BYTES + 1).read_to_end(&mut bytes)
        .map_err(|_| "Runtime credential unreadable".to_string())?;
    if bytes.len() as u64 > MAX_AUTH_BYTES { return Err("Runtime credential exceeds size limit".into()); }
    let mut auth: Value = serde_json::from_slice(&bytes).map_err(|_| "Runtime credential invalid".to_string())?;
    canonicalize_chatgpt_auth(&mut auth)?;
    validate_auth(&auth)?;
    Ok(auth)
}

/// One-way, same-account import. Never selects another account or repairs Codex files.
pub(crate) fn capture_runtime_credential(paths: &Paths, id: &str) -> Result<bool, String> {
    let auth = read_runtime_auth(paths)?;
    if account_fields(&auth)?.3 != id || crate::auth::is_agent_identity_auth(&auth) {
        return Ok(false);
    }
    write_managed_auth_if_newer(paths, id, &auth)
}

fn sync_current_into_store_for_paths(paths: &Paths, proxy_running: bool) -> Result<(), String> {
    if !paths.current_auth.exists() {
        return Ok(());
    }
    let auth = read_runtime_auth(paths)?;
    let id = account_fields(&auth)?.3;
    write_managed_auth_if_newer_impl(paths, &id, &auth, true)?;
    let mut state = read_state(paths);
    // In proxy mode auth.json is either absent or belongs to the optional OpenAI
    // login-state account, which is independent from the upstream official account.
    // Do not let startup synchronization turn that credential into the active account.
    if should_sync_current_as_active(
        &state,
        &id,
        crate::auth::is_agent_identity_auth(&auth),
        proxy_running,
    ) {
        state.active_account_id = Some(id);
        write_state(paths, &state)?;
    }
    Ok(())
}

pub(crate) fn load_usage(path: &Path) -> UsageSummary {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn save_usage(path: &Path, usage: &UsageSummary) -> Result<(), String> {
    let value = serde_json::to_value(usage).map_err(|error| error.to_string())?;
    write_json_atomic(path, &value)
}

/// Serialize quota commits across the app-server and managed-account readers.
/// A delayed response or failed request may not overwrite a newer observation.
pub(crate) fn save_usage_if_newer(
    path: &Path,
    incoming: &UsageSummary,
    request_started_at: &str,
) -> Result<(UsageSummary, bool), String> {
    static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = GATE
        .lock()
        .map_err(|_| "Quota cache commit lock unavailable".to_owned())?;
    let current = load_usage(path);
    let timestamp = |value: &str| chrono::DateTime::parse_from_rfc3339(value).ok();
    let started_at = timestamp(request_started_at)
        .ok_or_else(|| "Invalid quota request timestamp".to_owned())?;
    if current
        .fetched_at
        .as_deref()
        .and_then(timestamp)
        .is_some_and(|current| current > started_at)
    {
        return Ok((current, false));
    }
    let changed = serde_json::to_value(&current).ok() != serde_json::to_value(incoming).ok();
    if changed {
        save_usage(path, incoming)?;
    }
    Ok((incoming.clone(), changed))
}
