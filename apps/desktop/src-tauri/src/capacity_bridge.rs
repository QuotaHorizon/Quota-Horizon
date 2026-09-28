// QuotaHorizon capacity IPC bridge.
//
// The implementation lives in the shared capacity application service so the
// formal product and the legacy preview host cannot drift into separate quota
// readers, persistence rules, or recovery contracts.

use capacity_desktop_service::{
    DesktopAccountIdentity, DesktopActiveTimeEnvelope, DesktopActiveTimeQuotaEnvelope,
    DesktopActiveTimeUpdate, DesktopDemandPlanEnvelope, DesktopDemandPlanUpdate,
    DesktopDiagnosticsEnvelope, DesktopHistoryEnvelope, DesktopIssue, DesktopPaceEnvelope,
    DesktopPaceTrialsEnvelope, DesktopPlanningArchiveEnvelope, DesktopStatusEnvelope,
    DesktopVaultMutationStatusEnvelope, DesktopWorkPlanEnvelope, DesktopWorkScheduleUpdateRequest,
    ImplementationStatus, PlanningArchiveQuery,
};
use std::sync::atomic::Ordering;
use tauri::AppHandle;
use tauri::{Emitter, Manager};

pub(crate) mod account_overview;
mod quota_sync;
pub(crate) use quota_sync::{setup, shutdown};

use crate::storage::{managed_auth_path, read_json, read_state, resolve_paths, Paths};

pub(crate) fn preferred_codex_executable() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        capacity_desktop_service::bundled_codex_path(std::path::Path::new(
            "/Applications/ChatGPT.app",
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn capacity_account_digest_for_paths(paths: &Paths) -> Result<Option<String>, String> {
    capacity_account_metadata_for_paths(paths).map(|value| value.map(|(digest, _)| digest))
}

fn capacity_account_metadata_for_paths(paths: &Paths) -> Result<Option<(String, String)>, String> {
    let state = read_state(paths);
    if state.active_provider_id.is_some() || state.active_provider_group.is_some() {
        return Ok(None);
    }
    // The runtime auth file is authoritative when present: it is what a
    // non-proxy app-server will actually read, and it protects history if an
    // external login changed while manager state was stale. Managed auth is a
    // fallback for proxy/agent-identity modes that intentionally keep the
    // runtime file absent.
    let auth = match state.active_account_id.as_deref() {
        _ if paths.current_auth.exists() => read_json(&paths.current_auth)?,
        Some(account_id) => read_json(&managed_auth_path(paths, account_id))?,
        None => return Ok(None),
    };
    crate::auth::validate_auth(&auth)?;
    crate::auth::account_fields(&auth).map(|(_, plan, _, digest)| Some((digest, plan)))
}

async fn synchronize_account_binding(app_handle: &AppHandle) {
    let identity = resolve_paths(app_handle)
        .and_then(|paths| capacity_account_digest_for_paths(&paths))
        .ok()
        .flatten()
        .and_then(|digest| DesktopAccountIdentity::from_account_digest(digest).ok());
    let Some(identity) = identity else {
        capacity_desktop_service::invalidate_account_binding(app_handle);
        return;
    };
    // Stable-history failure must not hide an otherwise valid live quota read.
    // The service fails closed to an ephemeral binding on any Keychain or
    // derivation error.
    let _ = capacity_desktop_service::bind_account_identity(app_handle.clone(), identity).await;
}

#[tauri::command]
pub(crate) fn capacity_get_implementation_status(app_handle: AppHandle) -> ImplementationStatus {
    capacity_desktop_service::implementation_status(&app_handle)
}

#[tauri::command]
pub(crate) async fn capacity_get_cached_status(
    app_handle: AppHandle,
) -> Result<Option<DesktopStatusEnvelope>, String> {
    // Bootstrap must not wait behind a network refresh or a Keychain request.
    // Prefer the complete in-process snapshot for this identity. Only a cold
    // start falls back to quota-only disk data. Both are sequence-zero caches.
    tauri::async_runtime::spawn_blocking(move || quota_sync::cached_snapshot(&app_handle))
        .await
        .map_err(|_| "Unable to read the saved quota snapshot".to_owned())
}

#[tauri::command]
pub(crate) async fn capacity_authorize_history_keychain(
    window: tauri::WebviewWindow,
    app_handle: AppHandle,
) -> Result<DesktopStatusEnvelope, String> {
    if window.label() != "main" {
        return Err("History authorization is available in the main application only".to_owned());
    }
    capacity_desktop_service::authorize_history_keychain()
        .await
        .map_err(|_| {
            "History Keychain access was not authorized; live quota is unaffected".to_owned()
        })?;
    capacity_refresh_status(app_handle)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn capacity_get_status(
    app_handle: AppHandle,
) -> Result<DesktopStatusEnvelope, DesktopIssue> {
    let state = app_handle.state::<quota_sync::QuotaSyncState>();
    let _gate = state.gate.lock().await;
    synchronize_account_binding(&app_handle).await;
    let identity = resolve_paths(&app_handle)
        .ok()
        .and_then(|paths| capacity_account_digest_for_paths(&paths).ok().flatten());
    let result = capacity_desktop_service::status(app_handle.clone()).await;
    result.map(|envelope| quota_sync::reconcile(&app_handle, envelope, identity.as_deref()))
}

#[tauri::command]
pub(crate) async fn capacity_refresh_status(
    app_handle: AppHandle,
) -> Result<DesktopStatusEnvelope, DesktopIssue> {
    let state = app_handle.state::<quota_sync::QuotaSyncState>();
    let revision = state.refresh_revision.load(Ordering::SeqCst);
    let _gate = state.gate.lock().await;
    synchronize_account_binding(&app_handle).await;
    let identity = resolve_paths(&app_handle)
        .ok()
        .and_then(|paths| capacity_account_digest_for_paths(&paths).ok().flatten());
    let result = if state.refresh_revision.load(Ordering::SeqCst) != revision {
        capacity_desktop_service::status(app_handle.clone()).await
    } else {
        let result = capacity_desktop_service::refresh(app_handle.clone()).await;
        state.refresh_revision.fetch_add(1, Ordering::SeqCst);
        result
    };
    result.map(|envelope| {
        let envelope = quota_sync::reconcile(&app_handle, envelope, identity.as_deref());
        let _ = app_handle.emit(quota_sync::STATUS_EVENT, &envelope);
        envelope
    })
}

#[tauri::command]
pub(crate) async fn capacity_select_codex_executable(
    executable_id: String,
    app_handle: AppHandle,
) -> Result<DesktopStatusEnvelope, DesktopIssue> {
    let state = app_handle.state::<quota_sync::QuotaSyncState>();
    let _gate = state.gate.lock().await;
    synchronize_account_binding(&app_handle).await;
    let identity = resolve_paths(&app_handle)
        .ok()
        .and_then(|paths| capacity_account_digest_for_paths(&paths).ok().flatten());
    // The service already verifies and refreshes. A second refresh used to
    // discard its rejection and replace it with a generic selection prompt.
    let result =
        capacity_desktop_service::select_executable(app_handle.clone(), executable_id).await;
    state.refresh_revision.fetch_add(1, Ordering::SeqCst);
    result.map(|envelope| {
        let envelope = quota_sync::reconcile(&app_handle, envelope, identity.as_deref());
        let _ = app_handle.emit(quota_sync::STATUS_EVENT, &envelope);
        envelope
    })
}

#[tauri::command]
pub(crate) async fn capacity_get_history(
    limit_id: String,
    maximum_points: Option<u32>,
    app_handle: AppHandle,
) -> Result<DesktopHistoryEnvelope, DesktopIssue> {
    synchronize_account_binding(&app_handle).await;
    capacity_desktop_service::history(app_handle, limit_id, maximum_points).await
}

#[tauri::command]
pub(crate) async fn capacity_get_work_plan(
    app_handle: AppHandle,
) -> Result<DesktopWorkPlanEnvelope, DesktopIssue> {
    let status = capacity_get_status(app_handle.clone()).await?;
    capacity_desktop_service::work_plan_for_status(app_handle, status).await
}

#[tauri::command]
pub(crate) async fn capacity_get_demand_plan(
    history_context_id: String,
    app_handle: AppHandle,
) -> Result<DesktopDemandPlanEnvelope, DesktopIssue> {
    let status = capacity_get_status(app_handle.clone()).await?;
    capacity_desktop_service::demand_plan_for_status(app_handle, history_context_id, None, status)
        .await
}

#[tauri::command]
pub(crate) async fn capacity_update_demand_plan(
    request: DesktopDemandPlanUpdate,
    app_handle: AppHandle,
) -> Result<DesktopDemandPlanEnvelope, DesktopIssue> {
    let status = capacity_get_status(app_handle.clone()).await?;
    let result = capacity_desktop_service::demand_plan_for_status(
        app_handle.clone(),
        request.history_context_id.clone(),
        Some(request),
        status,
    )
    .await?;
    if result.status == "updated" {
        let _ = app_handle.emit("capacity-demand-plan-changed", &result.history_context_id);
    }
    Ok(result)
}

#[tauri::command]
pub(crate) async fn capacity_get_pace(
    history_context_id: String,
    app_handle: AppHandle,
) -> Result<DesktopPaceEnvelope, DesktopIssue> {
    let status = capacity_get_status(app_handle.clone()).await?;
    capacity_desktop_service::pace_for_status(app_handle, history_context_id, status).await
}

#[tauri::command]
pub(crate) async fn capacity_get_pace_trials(
    history_context_id: String,
    offset: u32,
    app_handle: AppHandle,
) -> Result<DesktopPaceTrialsEnvelope, DesktopIssue> {
    let status = capacity_get_status(app_handle.clone()).await?;
    capacity_desktop_service::pace_trials_for_status(app_handle, history_context_id, status, offset)
        .await
}

#[tauri::command]
pub(crate) async fn capacity_get_planning_archive(
    history_context_id: String,
    query: PlanningArchiveQuery,
    app_handle: AppHandle,
) -> Result<DesktopPlanningArchiveEnvelope, DesktopIssue> {
    let status = capacity_get_status(app_handle.clone()).await?;
    capacity_desktop_service::planning_archive_for_status(
        app_handle,
        history_context_id,
        query,
        status,
    )
    .await
}

#[tauri::command]
pub(crate) async fn capacity_get_active_time_quota(
    history_context_id: String,
    observation_id: String,
    app_handle: AppHandle,
) -> Result<DesktopActiveTimeQuotaEnvelope, DesktopIssue> {
    let status = capacity_get_status(app_handle.clone()).await?;
    capacity_desktop_service::active_time_quota_for_status(
        app_handle,
        history_context_id,
        observation_id,
        status,
    )
    .await
}

#[tauri::command]
pub(crate) async fn capacity_get_active_time(
    history_context_id: String,
    app_handle: AppHandle,
) -> Result<DesktopActiveTimeEnvelope, DesktopIssue> {
    let status = capacity_get_status(app_handle.clone()).await?;
    capacity_desktop_service::active_time_for_status(app_handle, history_context_id, None, status)
        .await
}

#[tauri::command]
pub(crate) async fn capacity_update_active_time(
    request: DesktopActiveTimeUpdate,
    app_handle: AppHandle,
) -> Result<DesktopActiveTimeEnvelope, DesktopIssue> {
    let status = capacity_get_status(app_handle.clone()).await?;
    let result = capacity_desktop_service::active_time_for_status(
        app_handle.clone(),
        request.history_context_id.clone(),
        Some(request),
        status,
    )
    .await?;
    if result.status == "updated" {
        let _ = app_handle.emit("capacity-active-time-changed", &result.history_context_id);
    }
    Ok(result)
}

#[tauri::command]
pub(crate) async fn capacity_update_work_schedule(
    request: DesktopWorkScheduleUpdateRequest,
    app_handle: AppHandle,
) -> Result<DesktopWorkPlanEnvelope, DesktopIssue> {
    let status = capacity_get_status(app_handle.clone()).await?;
    capacity_desktop_service::update_work_schedule_for_status(app_handle, request, status).await
}

#[tauri::command]
pub(crate) async fn capacity_get_diagnostics_preview(
    app_handle: AppHandle,
) -> Result<DesktopDiagnosticsEnvelope, DesktopIssue> {
    capacity_desktop_service::diagnostics(app_handle).await
}

#[tauri::command]
pub(crate) async fn capacity_get_vault_mutation_status(
    app_handle: AppHandle,
) -> Result<DesktopVaultMutationStatusEnvelope, DesktopIssue> {
    capacity_desktop_service::vault_mutation_status(app_handle).await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::storage::{write_json_atomic, write_state};

    fn paths(root: &std::path::Path) -> Paths {
        Paths {
            codex_home: root.join("codex-home"),
            current_auth: root.join("codex-home/auth.json"),
            current_config: root.join("codex-home/config.toml"),
            accounts: root.join("accounts"),
            providers: root.join("providers"),
            config_backup: root.join("config-before-provider.toml"),
            state_file: root.join("state.json"),
        }
    }

    fn auth() -> serde_json::Value {
        json!({
            "tokens": {
                "access_token": "opaque-access-token",
                "id_token": "",
                "chatgpt_user_id": "user-1",
                "account_id": "account-1"
            }
        })
    }

    #[test]
    fn managed_account_digest_is_stable_without_forwarding_auth_material() {
        let directory = tempfile::tempdir().expect("temporary account store");
        let paths = paths(directory.path());
        let state = crate::models::ManagerStateFile {
            active_account_id: Some("managed-a".to_owned()),
            ..Default::default()
        };
        write_state(&paths, &state).expect("write state");
        write_json_atomic(&managed_auth_path(&paths, "managed-a"), &auth()).expect("write auth");

        let digest = capacity_account_digest_for_paths(&paths)
            .expect("read digest")
            .expect("managed identity");
        assert_eq!(digest.len(), 24);
        assert!(!digest.contains("user-1"));
        assert!(!digest.contains("account-1"));
        assert!(!digest.contains("opaque-access-token"));
        assert_eq!(
            capacity_account_digest_for_paths(&paths)
                .unwrap()
                .as_deref(),
            Some(digest.as_str())
        );
    }

    #[test]
    fn startup_cache_survives_restart_but_never_crosses_an_external_login() {
        let directory = tempfile::tempdir().unwrap();
        let paths = paths(directory.path());
        let original = auth();
        let digest = crate::auth::account_fields(&original).unwrap().3;
        write_json_atomic(&paths.current_auth, &original).unwrap();
        write_json_atomic(
            &crate::storage::usage_path(&paths, &digest),
            &json!({
                "primary": {"usedPercent": 72.0, "remainingPercent": 28.0,
                    "windowMinutes": 10080, "resetsAt": 1789257600},
                "fetchedAt": "2026-09-05T00:00:00Z"
            }),
        )
        .unwrap();
        let cache = quota_sync::startup_snapshot_for_paths(&paths).unwrap();
        let cache = serde_json::to_value(cache).unwrap();
        assert_eq!(cache["status"]["quotaWindows"][0]["remainingPercent"], 28.0);
        assert_eq!(cache["status"]["quotaFreshness"], "stale");
        assert_eq!(cache["sequence"], 0);
        let mut changed = original;
        changed["tokens"]["account_id"] = json!("account-2");
        write_json_atomic(&paths.current_auth, &changed).unwrap();
        assert!(quota_sync::startup_snapshot_for_paths(&paths).is_none());
    }

    #[test]
    fn startup_cache_is_not_used_in_provider_mode() {
        let directory = tempfile::tempdir().unwrap();
        let paths = paths(directory.path());
        write_json_atomic(&paths.current_auth, &auth()).unwrap();
        write_state(
            &paths,
            &crate::models::ManagerStateFile {
                active_provider_id: Some("third-party".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(quota_sync::startup_snapshot_for_paths(&paths).is_none());
    }

    #[test]
    fn preferred_bundle_requires_a_real_file_and_accepts_current_layout() {
        let directory = tempfile::tempdir().expect("temporary executable root");
        assert!(capacity_desktop_service::bundled_codex_path(directory.path()).is_none());
        let executable = directory
            .path()
            .join("Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, b"fixture").expect("write fixture executable");
        assert_eq!(
            capacity_desktop_service::bundled_codex_path(directory.path()),
            Some(executable)
        );
    }

    #[test]
    fn provider_mode_never_reuses_the_previous_official_binding() {
        let directory = tempfile::tempdir().expect("temporary account store");
        let paths = paths(directory.path());
        write_json_atomic(&paths.current_auth, &auth()).expect("write current auth");
        let state = crate::models::ManagerStateFile {
            active_provider_id: Some("provider-a".to_owned()),
            ..Default::default()
        };
        write_state(&paths, &state).expect("write state");

        assert_eq!(capacity_account_digest_for_paths(&paths).unwrap(), None);
    }

    #[test]
    fn runtime_auth_wins_when_manager_state_points_at_a_different_account() {
        let directory = tempfile::tempdir().expect("temporary account store");
        let paths = paths(directory.path());
        let state = crate::models::ManagerStateFile {
            active_account_id: Some("managed-a".to_owned()),
            ..Default::default()
        };
        write_state(&paths, &state).expect("write state");
        write_json_atomic(&managed_auth_path(&paths, "managed-a"), &auth())
            .expect("write managed auth");
        let runtime_auth = json!({
            "tokens": {
                "access_token": "different-opaque-access-token",
                "id_token": "",
                "chatgpt_user_id": "runtime-user",
                "account_id": "runtime-account"
            }
        });
        write_json_atomic(&paths.current_auth, &runtime_auth).expect("write runtime auth");

        let expected = crate::auth::account_fields(&runtime_auth).unwrap().3;
        assert_eq!(
            capacity_account_digest_for_paths(&paths)
                .unwrap()
                .as_deref(),
            Some(expected.as_str())
        );
    }
}
