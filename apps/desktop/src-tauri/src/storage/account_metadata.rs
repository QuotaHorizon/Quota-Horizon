pub(crate) fn save_note(path: &Path, note: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "The note path has no parent directory".to_string())?;
    ensure_private_directory(parent)?;

    if note.is_empty() {
        if path.exists() {
            fs::remove_file(path)
                .map_err(|error| format!("Failed to remove {}: {error}", path.display()))?;
        }
        return Ok(());
    }

    write_text_atomic(path, note)
}

pub(crate) fn save_expiration(path: &Path, expires_at: &str) -> Result<(), String> {
    save_note(path, expires_at)
}

pub(crate) fn save_account_private_details(
    path: &Path,
    details: &AccountPrivateDetails,
) -> Result<(), String> {
    let value = serde_json::to_value(details).map_err(|error| error.to_string())?;
    write_json_atomic(path, &value)
}

pub(crate) fn save_auto_switch_priority(path: &Path, priority: i32) -> Result<(), String> {
    write_text_atomic(path, &priority.to_string())
}

pub(crate) fn save_auto_switch_threshold(path: &Path, threshold: f64) -> Result<(), String> {
    write_text_atomic(path, &threshold.to_string())
}

pub(crate) fn parse_last_modified(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

pub(crate) fn load_last_modified(path: &Path) -> Option<DateTime<Utc>> {
    parse_last_modified(&fs::read_to_string(path).ok()?)
}

pub(crate) fn save_last_modified(path: &Path, modified_at: DateTime<Utc>) -> Result<(), String> {
    save_note(path, &modified_at.to_rfc3339())
}

pub(crate) fn save_account_last_modified(
    paths: &Paths,
    id: &str,
    modified_at: DateTime<Utc>,
) -> Result<(), String> {
    save_last_modified(&last_modified_path(paths, id), modified_at)
}

fn latest_file_modified(paths: &Paths, id: &str) -> Option<DateTime<Utc>> {
    [
        managed_auth_path(paths, id),
        note_path(paths, id),
        expiration_path(paths, id),
        account_private_details_path(paths, id),
        usage_path(paths, id),
        auto_switch_priority_path(paths, id),
        auto_switch_threshold_path(paths, id),
    ]
    .into_iter()
    .filter_map(|path| fs::metadata(path).ok()?.modified().ok())
    .map(DateTime::<Utc>::from)
    .max()
}

pub(crate) fn load_or_init_last_modified(paths: &Paths, id: &str) -> Result<DateTime<Utc>, String> {
    let path = last_modified_path(paths, id);
    if let Some(modified_at) = load_last_modified(&path) {
        return Ok(modified_at);
    }

    let modified_at = latest_file_modified(paths, id).unwrap_or_else(Utc::now);
    save_last_modified(&path, modified_at)?;
    Ok(modified_at)
}

fn file_modified_or_fallback(path: PathBuf, fallback: &str) -> String {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|| parse_last_modified(fallback).unwrap_or_else(Utc::now))
        .to_rfc3339()
}

fn fill_missing_field_modified_at(
    values: &mut AccountFieldModifiedAt,
    paths: &Paths,
    id: &str,
    fallback: &str,
) {
    if values.auth.trim().is_empty() {
        values.auth = file_modified_or_fallback(managed_auth_path(paths, id), fallback);
    }
    if values.note.trim().is_empty() {
        values.note = file_modified_or_fallback(note_path(paths, id), fallback);
    }
    if values.expires_at.trim().is_empty() {
        values.expires_at = file_modified_or_fallback(expiration_path(paths, id), fallback);
    }
    if values.private_details.trim().is_empty() {
        values.private_details =
            file_modified_or_fallback(account_private_details_path(paths, id), UNMODIFIED_FIELD_AT);
    }
    if values.usage.trim().is_empty() {
        values.usage = file_modified_or_fallback(usage_path(paths, id), fallback);
    }
    if values.active.trim().is_empty() {
        values.active = fallback.to_string();
    }
    if values.auto_switch_priority.trim().is_empty() {
        values.auto_switch_priority =
            file_modified_or_fallback(auto_switch_priority_path(paths, id), fallback);
    }
    if values.auto_switch_threshold.trim().is_empty() {
        values.auto_switch_threshold =
            file_modified_or_fallback(auto_switch_threshold_path(paths, id), fallback);
    }
}

pub(crate) fn load_or_init_account_field_modified_at(
    paths: &Paths,
    id: &str,
) -> Result<AccountFieldModifiedAt, String> {
    let fallback = load_or_init_last_modified(paths, id)?.to_rfc3339();
    let path = field_modified_at_path(paths, id);
    let mut values = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<AccountFieldModifiedAt>(&bytes).ok())
        .unwrap_or_default();
    let original = serde_json::to_value(&values).map_err(|error| error.to_string())?;
    fill_missing_field_modified_at(&mut values, paths, id, &fallback);
    if serde_json::to_value(&values).map_err(|error| error.to_string())? != original {
        save_account_field_modified_at(paths, id, &values)?;
    }
    Ok(values)
}

pub(crate) fn save_account_field_modified_at(
    paths: &Paths,
    id: &str,
    values: &AccountFieldModifiedAt,
) -> Result<(), String> {
    let value = serde_json::to_value(values).map_err(|error| error.to_string())?;
    write_json_atomic(&field_modified_at_path(paths, id), &value)?;
    let latest = [
        &values.auth,
        &values.note,
        &values.expires_at,
        &values.private_details,
        &values.usage,
        &values.active,
        &values.auto_switch_priority,
        &values.auto_switch_threshold,
    ]
    .into_iter()
    .filter_map(|value| parse_last_modified(value))
    .max();
    if let Some(latest) = latest {
        save_account_last_modified(paths, id, latest)?;
    }
    Ok(())
}

pub(crate) fn touch_account_field(
    paths: &Paths,
    id: &str,
    field: AccountSyncField,
) -> Result<DateTime<Utc>, String> {
    let modified_at = Utc::now();
    let mut values = load_or_init_account_field_modified_at(paths, id)?;
    let value = modified_at.to_rfc3339();
    match field {
        AccountSyncField::Auth => values.auth = value,
        AccountSyncField::Note => values.note = value,
        AccountSyncField::ExpiresAt => values.expires_at = value,
        AccountSyncField::PrivateDetails => values.private_details = value,
        AccountSyncField::Usage => values.usage = value,
        AccountSyncField::Active => values.active = value,
        AccountSyncField::AutoSwitchPriority => values.auto_switch_priority = value,
        AccountSyncField::AutoSwitchThreshold => values.auto_switch_threshold = value,
    }
    save_account_field_modified_at(paths, id, &values)?;
    Ok(modified_at)
}

pub(crate) fn write_managed_auth_if_changed(
    paths: &Paths,
    id: &str,
    auth: &Value,
) -> Result<bool, String> {
    let _guard = MANAGED_AUTH_WRITE_LOCK
        .lock()
        .map_err(|_| "Account credential write lock is poisoned".to_string())?;
    write_managed_auth_if_changed_unlocked(paths, id, auth)
}

fn write_managed_auth_if_changed_unlocked(
    paths: &Paths,
    id: &str,
    auth: &Value,
) -> Result<bool, String> {
    ensure_private_directory(&paths.accounts)?;
    ensure_private_directory(&account_dir(paths, id))?;
    let changed = write_json_if_changed(&managed_auth_path(paths, id), auth)?;
    harden_private_file(&managed_auth_path(paths, id))?;
    if changed {
        touch_account_field(paths, id, AccountSyncField::Auth)?;
    }
    Ok(changed)
}

/// Replaces a request's credential only when no newer login or refresh changed it meanwhile.
pub(crate) fn write_managed_auth_if_unchanged(
    paths: &Paths,
    id: &str,
    expected: &Value,
    auth: &Value,
) -> Result<bool, String> {
    let _guard = MANAGED_AUTH_WRITE_LOCK
        .lock()
        .map_err(|_| "Account credential write lock is poisoned".to_string())?;
    let current = read_json(&managed_auth_path(paths, id)).ok();
    if current.as_ref() == Some(auth) {
        return Ok(true);
    }
    if current.as_ref() != Some(expected) {
        return Ok(false);
    }
    write_managed_auth_if_changed_unlocked(paths, id, auth)?;
    Ok(true)
}

fn auth_last_refresh(auth: &Value) -> Option<DateTime<Utc>> {
    auth.get("last_refresh")
        .and_then(Value::as_str)
        .and_then(parse_last_modified)
        .filter(|value| value.timestamp() > 0)
        .or_else(|| {
            // Some legacy/login records predate last_refresh. Use an explicit
            // issuance time, never email, plan, file mtime or token expiry.
            ["id_token", "access_token"].into_iter().filter_map(|field| {
                let token = crate::auth::token_string(auth, field)?;
                let claims = crate::auth::decode_jwt(token).ok()?;
                let timestamp = claims.get("iat")?.as_i64()?;
                (timestamp > 0).then(|| DateTime::from_timestamp(timestamp, 0)).flatten()
            }).max()
        })
}

/// Imports a live Codex credential only when it is demonstrably newer than the managed copy.
pub(crate) fn write_managed_auth_if_newer(
    paths: &Paths,
    id: &str,
    auth: &Value,
) -> Result<bool, String> {
    write_managed_auth_if_newer_impl(paths, id, auth, false)
}

fn write_managed_auth_if_newer_impl(paths: &Paths, id: &str, auth: &Value, create_missing: bool) -> Result<bool, String> {
    validate_auth(auth)?;
    if account_fields(auth)?.3 != id {
        return Err("Runtime credential identity does not match the saved account".into());
    }
    let _guard = MANAGED_AUTH_WRITE_LOCK
        .lock()
        .map_err(|_| "Account credential write lock is poisoned".to_string())?;
    let path = managed_auth_path(paths, id);
    if !path.try_exists().map_err(|_| "Account credential unavailable".to_string())? {
        return if create_missing { write_managed_auth_if_changed_unlocked(paths, id, auth) } else { Ok(false) };
    }
    // Unreadable is not absent: retain damaged bytes for explicit repair/login.
    let current = read_json(&path)?;
    validate_auth(&current)?;
    if account_fields(&current)?.3 != id {
        return Err("Saved account identity does not match its directory".into());
    }
    if current == *auth {
        return Ok(false);
    }
    let candidate_is_newer = matches!(
        (auth_last_refresh(auth), auth_last_refresh(&current)),
        (Some(candidate), Some(existing)) if candidate > existing
    );
    if !candidate_is_newer {
        return Ok(false);
    }
    write_managed_auth_if_changed_unlocked(paths, id, auth)
}
