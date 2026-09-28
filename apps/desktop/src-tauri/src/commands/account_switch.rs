/// Blocking account switch that must run off the UI thread. The switch spawns
/// process-control subprocesses and performs file I/O. Interactive WebView
/// switches use the prepare/confirm commands; non-interactive remote-control
/// and automatic proxy routes enter this function directly.
pub(crate) fn switch_account_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
) -> Result<(), String> {
    let _switch_guard = account_switch_lock()
        .lock()
        .map_err(|_| "Account switch lock is poisoned".to_string())?;
    refresh_local_codex_path(&app);
    perform_safe_account_switch(&app, &id, false, true)
}

pub(crate) fn switch_account_and_restart_chatgpt_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
) -> Result<(), String> {
    let _switch_guard = account_switch_lock()
        .lock()
        .map_err(|_| "Account switch lock is poisoned".to_string())?;

    refresh_local_codex_path(&app);
    perform_safe_account_switch(&app, &id, true, true)
}

fn prepare_account_switch_context<R: Runtime>(
    app: &tauri::AppHandle<R>,
    id: &str,
    preserve_previous: bool,
) -> Result<AccountSwitchContext, String> {
    validate_managed_account_id(id)?;
    let proxy_running = crate::local_proxy::is_running();
    let paths = resolve_paths(app)?;
    let selected = load_validated_managed_auth(&paths, id)?;
    ensure_account_switch_allowed(&selected, proxy_running)?;
    let mut original_state = read_state(&paths);
    let previous_account_id = original_state.active_account_id.clone();
    crate::conversation_hub::mark_threads_before_account_switch(
        &paths,
        &mut original_state,
        previous_account_id.as_deref(),
    )?;
    let previous_refresh = preserve_previous
        .then(|| {
            previous_account_id.as_deref().and_then(|account_id| {
                capture_refreshed_auth_candidate(&paths, account_id)
                    .map(|auth| (account_id.to_string(), auth))
            })
        })
        .flatten();
    let write_codex = crate::claude_code::should_write_codex_for_app(app)?;
    Ok(AccountSwitchContext {
        proxy_running,
        write_codex,
        paths,
        selected,
        original_state,
        previous_refresh,
    })
}

#[derive(Clone)]
struct AccountSwitchContext {
    proxy_running: bool,
    write_codex: bool,
    paths: Paths,
    selected: Value,
    original_state: ManagerStateFile,
    previous_refresh: Option<(String, Value)>,
}

fn apply_account_switch_files<R: Runtime>(
    app: &tauri::AppHandle<R>,
    id: &str,
    context: &AccountSwitchContext,
) -> Result<(), String> {
    let AccountSwitchContext {
        proxy_running,
        write_codex,
        paths,
        selected,
        original_state,
        previous_refresh: _,
    } = context;
    let mut state = original_state.clone();
    state.active_provider_id = None;
    state.active_provider_group = None;
    state.active_account_id = Some(id.to_string());
    state.concurrent_account_routing_enabled = false;

    // Prevent an overlapping monitor refresh from being persisted under the
    // previous account while auth/config/state are transitioning. The formal
    // capacity bridge establishes the new stable binding after post-switch
    // account state is readable.
    capacity_desktop_service::invalidate_account_binding(app);

    if !*write_codex {
        write_state(paths, &state)?;
    } else if *proxy_running {
        // Publish the official route before changing config.toml. Codex watches that
        // file and may reconnect immediately; writing the config first would let the
        // reconnect race through the previously selected third-party Provider.
        write_state(paths, &state)?;
        if let Err(error) = crate::providers::write_official_local_proxy_config_from_store(paths) {
            let _ = write_state(paths, original_state);
            return Err(error);
        }
    } else {
        // The local proxy reads the selected managed credential.  Avoid modifying the
        // authentication file watched by the already-running Codex application.
        write_json_atomic(&paths.current_auth, selected)?;
        // Always remove a stale managed Provider block, even if an older or partially
        // completed switch left active_provider_id out of sync with config.toml.
        crate::providers::restore_official_config(paths)?;
        write_state(paths, &state)?;
    }
    Ok(())
}

fn finalize_account_switch<R: Runtime>(
    app: &tauri::AppHandle<R>,
    id: &str,
    context: &AccountSwitchContext,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Some((previous_account_id, auth)) = &context.previous_refresh {
        if let Err(error) = write_managed_auth_if_newer(&context.paths, previous_account_id, auth) {
            warnings.push(format!("previous auth refresh preservation failed: {error}"));
        }
    }
    if let Err(error) = touch_account_field(&context.paths, id, AccountSyncField::Active) {
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

fn ensure_account_switch_allowed(auth: &Value, proxy_running: bool) -> Result<(), String> {
    if !proxy_running && is_agent_identity_auth(auth) {
        return Err(
            "Agent Identity 账号只能在本地代理模式下切换。请先启动本地代理，再切换到该账号"
                .to_string(),
        );
    }
    Ok(())
}

pub(crate) fn load_validated_managed_auth(paths: &Paths, id: &str) -> Result<Value, String> {
    validate_managed_account_id(id)?;
    let mut auth = read_json(&managed_auth_path(paths, id))?;
    canonicalize_chatgpt_auth(&mut auth)?;
    validate_auth(&auth)?;
    let (_, _, _, actual_id) = account_fields(&auth)?;
    if actual_id != id {
        return Err("Managed account credential does not match its account identifier".to_string());
    }
    Ok(auth)
}

fn validate_managed_account_id(id: &str) -> Result<(), String> {
    let valid = !id.is_empty()
        && id.len() <= 200
        && !id.chars().any(char::is_control)
        && !id.contains(['/', '\\'])
        && id != "."
        && id != ".."
        && Path::new(id).components().count() == 1;
    if !valid {
        return Err("Account identifier is invalid".to_string());
    }
    Ok(())
}

fn capture_refreshed_auth_candidate(paths: &Paths, account_id: &str) -> Option<Value> {
    let mut current_auth = read_json(&paths.current_auth).ok()?;
    let has_valid_last_refresh = current_auth
        .get("last_refresh")
        .and_then(Value::as_str)
        .is_some_and(|value| chrono::DateTime::parse_from_rfc3339(value).is_ok());
    if is_agent_identity_auth(&current_auth)
        || !has_valid_last_refresh
        || canonicalize_chatgpt_auth(&mut current_auth).is_err()
        || validate_auth(&current_auth).is_err()
        || account_fields(&current_auth)
            .map(|(_, _, _, id)| id != account_id)
            .unwrap_or(true)
    {
        return None;
    }
    Some(current_auth)
}

pub(crate) fn write_managed_auth_to_current(paths: &Paths, id: &str) -> Result<(), String> {
    let auth = load_validated_managed_auth(paths, id)?;
    write_json_atomic(&paths.current_auth, &auth)
}

#[tauri::command]
pub(crate) fn set_account_auto_switch_enabled<R: Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let paths = resolve_paths(&app)?;
    if !managed_auth_path(&paths, &id).exists() {
        return Err("Account does not exist".to_string());
    }
    set_account_auto_switch_enabled_for_paths(&paths, &id, enabled)?;
    app.emit("accounts-changed", ())
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) fn set_auto_disable_status_codes<R: Runtime>(
    app: tauri::AppHandle<R>,
    mut status_codes: Vec<u16>,
) -> Result<AppSettings, String> {
    if status_codes
        .iter()
        .any(|status| !(100..=599).contains(status))
    {
        return Err("automatic disable status codes must be between 100 and 599".to_string());
    }
    status_codes.sort_unstable();
    status_codes.dedup();

    let mut settings = read_app_settings(&app)?;
    settings.auto_disable_status_codes = status_codes;
    write_app_settings(&app, &settings)?;
    Ok(settings)
}

#[tauri::command]
pub(crate) fn set_account_auto_switch_priority<R: Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
    priority: i32,
) -> Result<(), String> {
    let paths = resolve_paths(&app)?;
    if !managed_auth_path(&paths, &id).exists() {
        return Err("Account does not exist".to_string());
    }
    let path = auto_switch_priority_path(&paths, &id);
    if load_auto_switch_priority(&path) != priority || !path.exists() {
        save_auto_switch_priority(&path, priority)?;
        touch_account_field(&paths, &id, AccountSyncField::AutoSwitchPriority)?;
    }
    app.emit("accounts-changed", ())
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn set_account_auto_switch_threshold<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    id: String,
    threshold: f64,
) -> Result<(), String> {
    if !threshold.is_finite() || !(0.0..=100.0).contains(&threshold) {
        return Err("Auto-switch threshold must be between 0 and 100".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let paths = resolve_paths(&app)?;
        if !managed_auth_path(&paths, &id).exists() {
            return Err("Account does not exist".to_string());
        }
        let path = auto_switch_threshold_path(&paths, &id);
        if load_auto_switch_threshold(&path) != threshold || !path.exists() {
            save_auto_switch_threshold(&path, threshold)?;
            touch_account_field(&paths, &id, AccountSyncField::AutoSwitchThreshold)?;
        }
        app.emit("accounts-changed", ())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Account threshold task failed: {error}"))?
}

fn update_disabled_account_ids(state: &mut ManagerStateFile, id: &str, enabled: bool) -> bool {
    let was_disabled = state
        .disabled_account_ids
        .iter()
        .any(|account_id| account_id == id);
    let should_be_disabled = !enabled;
    if enabled {
        state
            .disabled_account_ids
            .retain(|account_id| account_id != id);
    } else if !was_disabled {
        state.disabled_account_ids.push(id.to_string());
        state.disabled_account_ids.sort();
    }
    was_disabled != should_be_disabled
}

pub(crate) fn set_account_auto_switch_enabled_for_paths(
    paths: &Paths,
    id: &str,
    enabled: bool,
) -> Result<bool, String> {
    let _guard = account_auto_switch_state_lock()
        .lock()
        .map_err(|_| "Account auto-switch state lock is poisoned".to_string())?;
    let mut state = read_state(paths);
    let changed = update_disabled_account_ids(&mut state, id, enabled);
    if changed {
        write_state(paths, &state)?;
    }
    Ok(changed)
}
