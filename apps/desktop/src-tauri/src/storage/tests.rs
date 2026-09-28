#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        apply_app_settings_version_migration, harden_account_store, should_activate_import,
        should_sync_current_as_active, write_managed_auth_if_unchanged, write_text_if_changed,
    };
    use crate::models::{AppSettings, ManagerStateFile, LEGACY_UPSTREAM_CLOUD_BASE_URL};
    use crate::storage::{managed_auth_path, read_json, write_json_atomic, Paths};
    use serde_json::json;

    fn test_paths() -> Paths {
        let root = std::env::temp_dir().join(format!(
            "codex-switch-storage-test-{}",
            uuid::Uuid::new_v4()
        ));
        Paths {
            current_auth: root.join("codex-home/auth.json"),
            current_config: root.join("codex-home/config.toml"),
            codex_home: root.join("codex-home"),
            accounts: root.join("app-data/accounts"),
            providers: root.join("app-data/providers"),
            config_backup: root.join("app-data/config-before-provider.toml"),
            state_file: root.join("app-data/state.json"),
        }
    }

    #[test]
    fn text_is_only_replaced_when_contents_change() {
        let root = std::env::temp_dir().join(format!(
            "codex-switch-storage-test-{}",
            uuid::Uuid::new_v4()
        ));
        let path = root.join("config.toml");

        assert!(write_text_if_changed(&path, "model = \"first\"\n").unwrap());
        assert!(!write_text_if_changed(&path, "model = \"first\"\n").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "model = \"first\"\n");

        assert!(write_text_if_changed(&path, "model = \"second\"\n").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "model = \"second\"\n");

        fs::remove_dir_all(root).unwrap();
    }

    fn quota(time: &str, remaining: f64) -> crate::models::UsageSummary {
        crate::models::UsageSummary {
            primary: Some(crate::models::UsageWindow {
                used_percent: 100.0 - remaining,
                remaining_percent: remaining,
                window_minutes: Some(10_080),
                resets_at: Some(1_789_257_600),
            }),
            fetched_at: Some(time.to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn a_late_quota_response_cannot_restore_29_after_a_newer_28() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("usage.json");
        let old = quota("2026-09-05T00:00:00Z", 29.0);
        let latest = quota("2026-09-05T00:01:00Z", 28.0);
        super::save_usage(&path, &old).unwrap();
        let (_, changed) =
            super::save_usage_if_newer(&path, &latest, latest.fetched_at.as_deref().unwrap())
                .unwrap();
        assert!(changed);
        let (effective, changed) =
            super::save_usage_if_newer(&path, &old, old.fetched_at.as_deref().unwrap()).unwrap();
        assert!(!changed);
        assert_eq!(effective.primary.unwrap().remaining_percent, 28.0);
        assert_eq!(super::load_usage(&path).fetched_at, latest.fetched_at);
    }

    #[test]
    fn failed_refresh_preserves_observation_time_and_a_late_failure_is_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("usage.json");
        let latest = quota("2026-09-05T00:01:00Z", 28.0);
        super::save_usage(&path, &latest).unwrap();
        let mut failed = latest.clone();
        failed.error = Some("network unavailable".to_owned());
        let (effective, changed) =
            super::save_usage_if_newer(&path, &failed, "2026-09-05T00:00:00Z").unwrap();
        assert!(!changed);
        assert!(effective.error.is_none());
        let (effective, changed) =
            super::save_usage_if_newer(&path, &failed, "2026-09-05T00:02:00Z").unwrap();
        assert!(changed);
        assert!(effective.error.is_some());
        assert_eq!(effective.primary.unwrap().remaining_percent, 28.0);
        assert_eq!(effective.fetched_at, latest.fetched_at);
    }

    #[test]
    fn a_newer_official_reset_can_increase_quota_and_duplicate_reads_do_not_rewrite() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("usage.json");
        super::save_usage(&path, &quota("2026-09-05T00:00:00Z", 28.0)).unwrap();
        let latest = quota("2026-09-05T00:01:00Z", 100.0);
        let (effective, changed) =
            super::save_usage_if_newer(&path, &latest, latest.fetched_at.as_deref().unwrap())
                .unwrap();
        assert!(changed);
        assert_eq!(effective.primary.unwrap().remaining_percent, 100.0);
        let (_, changed) =
            super::save_usage_if_newer(&path, &latest, latest.fetched_at.as_deref().unwrap())
                .unwrap();
        assert!(!changed);
    }

    #[test]
    fn invalid_request_time_cannot_bypass_the_quota_ordering_guard() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("usage.json");
        super::save_usage(&path, &quota("2026-09-05T00:01:00Z", 28.0)).unwrap();
        assert!(
            super::save_usage_if_newer(&path, &quota("2026-09-05T00:00:00Z", 29.0), "invalid")
                .is_err()
        );
        assert_eq!(
            super::load_usage(&path).primary.unwrap().remaining_percent,
            28.0
        );
    }

    #[cfg(unix)]
    #[test]
    fn account_store_is_hardened_to_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let paths = test_paths();
        let record = paths.accounts.join("account-1");
        fs::create_dir_all(&record).unwrap();
        fs::write(record.join("auth.json"), b"{}").unwrap();
        fs::set_permissions(&paths.accounts, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&record, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(record.join("auth.json"), fs::Permissions::from_mode(0o644)).unwrap();

        harden_account_store(&paths).unwrap();

        assert_eq!(
            fs::metadata(&paths.accounts).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(
            fs::metadata(&record).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(
            fs::metadata(record.join("auth.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o600
        );
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn stale_request_cannot_overwrite_a_new_login_credential() {
        let paths = test_paths();
        let id = "account-1";
        let stale = json!({ "tokens": { "access_token": "stale" } });
        let refreshed_stale = json!({ "tokens": { "access_token": "refreshed-stale" } });
        let fresh_login = json!({ "tokens": { "access_token": "fresh-login" } });
        write_json_atomic(&managed_auth_path(&paths, id), &fresh_login).unwrap();

        assert!(!write_managed_auth_if_unchanged(&paths, id, &stale, &refreshed_stale).unwrap());
        assert_eq!(read_json(&managed_auth_path(&paths, id)).unwrap(), fresh_login);

        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn new_app_version_disables_the_legacy_upstream_cloud_server() {
        let mut settings: AppSettings =
            serde_json::from_str(&format!(
                r#"{{"cloudBaseUrl":"{}","lastStartedVersion":"1.1.19"}}"#,
                LEGACY_UPSTREAM_CLOUD_BASE_URL,
            ))
            .unwrap();

        assert!(apply_app_settings_version_migration(
            &mut settings,
            "1.1.20"
        ));
        assert!(settings.cloud_base_url.is_none());
        assert!(!settings.launch_at_startup);
        assert!(!settings.floating_bubble_enabled);
        assert_eq!(settings.last_started_version.as_deref(), Some("1.1.20"));
    }

    #[test]
    fn current_version_still_disables_the_legacy_upstream_cloud_server() {
        let mut settings = AppSettings {
            cloud_base_url: Some(LEGACY_UPSTREAM_CLOUD_BASE_URL.to_string()),
            cloud_user_email: Some("legacy@example.com".to_string()),
            last_started_version: Some("1.1.20".to_string()),
            ..AppSettings::default()
        };

        assert!(apply_app_settings_version_migration(
            &mut settings,
            "1.1.20"
        ));
        assert!(settings.cloud_base_url.is_none());
        assert!(!settings.launch_at_startup);
        assert!(!settings.floating_bubble_enabled);
        assert!(settings.cloud_user_email.is_none());
    }

    #[test]
    fn version_migration_preserves_a_custom_cloud_server() {
        let mut settings = AppSettings {
            cloud_base_url: Some("https://cloud.example.com".to_string()),
            last_started_version: Some("1.1.19".to_string()),
            ..AppSettings::default()
        };

        assert!(apply_app_settings_version_migration(
            &mut settings,
            "1.1.20"
        ));
        assert_eq!(
            settings.cloud_base_url.as_deref(),
            Some("https://cloud.example.com")
        );
    }

    #[test]
    fn version_migration_only_runs_once_per_version() {
        let mut settings = AppSettings {
            cloud_base_url: None,
            last_started_version: Some("1.1.20".to_string()),
            ..AppSettings::default()
        };

        assert!(!apply_app_settings_version_migration(
            &mut settings,
            "1.1.20"
        ));
        assert!(settings.cloud_base_url.is_none());
    }

    #[test]
    fn version_migration_keeps_local_mode_unconfigured() {
        let mut settings = AppSettings {
            cloud_base_url: None,
            last_started_version: Some("1.1.19".to_string()),
            ..AppSettings::default()
        };

        assert!(apply_app_settings_version_migration(
            &mut settings,
            "1.1.20"
        ));
        assert!(settings.cloud_base_url.is_none());
    }

    #[test]
    fn first_official_import_becomes_active_when_codex_has_no_auth() {
        assert!(should_activate_import(
            &ManagerStateFile::default(),
            false,
            false
        ));
    }

    fn login_auth(identity: &str, credential: &str) -> serde_json::Value {
        use base64::Engine;
        let claims = json!({"sub": identity, "email": "synthetic@example.com", "https://api.openai.com/auth": {
            "chatgpt_account_id": identity, "chatgpt_user_id": identity, "chatgpt_plan_type": "pro"
        }});
        let token = format!("e30.{}.signature", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap()));
        json!({"tokens": {"id_token": token, "access_token": credential, "refresh_token": "synthetic-refresh"}})
    }

    #[test]
    fn first_login_is_saved_without_creating_current_auth_or_active_state() {
        let paths = test_paths();
        let id = super::save_login_account(&paths, login_auth("one", "synthetic-access")).unwrap();
        assert!(managed_auth_path(&paths, &id).exists());
        assert!(!paths.codex_home.exists());
        assert!(!paths.current_auth.exists());
        assert!(!paths.current_config.exists());
        assert!(!paths.state_file.exists());
        fs::remove_dir_all(paths.accounts.parent().unwrap()).unwrap();
    }

    #[test]
    fn relogin_preserves_current_login_selection_notes_and_quota() {
        let paths = test_paths();
        let id = super::save_login_account(&paths, login_auth("one", "old-access")).unwrap();
        let current = json!({"untouched": "running-account"});
        write_json_atomic(&paths.current_auth, &current).unwrap();
        write_text_if_changed(&paths.current_config, "model = \"unchanged\"\n").unwrap();
        let selected = json!({"activeAccountId": id, "activeProviderId": "keep-provider"});
        write_json_atomic(&paths.state_file, &selected).unwrap();
        let note_path = super::note_path(&paths, &id);
        write_text_if_changed(&note_path, "keep note").unwrap();
        let quota_path = super::usage_path(&paths, &id);
        write_json_atomic(&quota_path, &json!({"fetchedAt": "2026-09-07T00:00:00Z"})).unwrap();
        let quota_before = fs::read(&quota_path).unwrap();
        let result = super::save_login_account(&paths, login_auth("one", "new-access")).unwrap();
        assert_eq!(result, id);
        assert_eq!(read_json(&managed_auth_path(&paths, &id)).unwrap()["tokens"]["access_token"], "new-access");
        assert_eq!(read_json(&paths.current_auth).unwrap(), current);
        assert_eq!(read_json(&paths.state_file).unwrap(), selected);
        assert_eq!(fs::read_to_string(&paths.current_config).unwrap(), "model = \"unchanged\"\n");
        assert_eq!(fs::read_to_string(note_path).unwrap(), "keep note");
        assert_eq!(fs::read(quota_path).unwrap(), quota_before);
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn invalid_login_does_not_modify_saved_accounts_or_current_login() {
        let paths = test_paths();
        let id = super::save_login_account(&paths, login_auth("one", "old-access")).unwrap();
        let before = fs::read(managed_auth_path(&paths, &id)).unwrap();
        assert!(super::save_login_account(&paths, json!({"tokens": {}})).is_err());
        assert_eq!(fs::read(managed_auth_path(&paths, &id)).unwrap(), before);
        assert!(!paths.current_auth.exists());
        assert!(!paths.state_file.exists());
        fs::remove_dir_all(paths.accounts.parent().unwrap()).unwrap();
    }

    fn dated_auth(identity: &str, credential: &str, timestamp: &str) -> serde_json::Value {
        let mut auth = login_auth(identity, credential);
        auth["last_refresh"] = json!(timestamp);
        auth
    }

    #[test]
    fn restart_preserves_a_new_login_instead_of_importing_an_old_runtime_copy() {
        let paths = test_paths();
        let fresh = dated_auth("one", "new-login", "2026-09-07T01:00:00Z");
        let id = super::save_login_account(&paths, fresh).unwrap();
        let stored_before = fs::read(managed_auth_path(&paths, &id)).unwrap();
        let old = dated_auth("one", "old-runtime", "2026-09-07T00:00:00Z");
        write_json_atomic(&paths.current_auth, &old).unwrap();
        let source_before = fs::read(&paths.current_auth).unwrap();
        super::sync_current_into_store_for_paths(&paths, false).unwrap();
        assert_eq!(fs::read(managed_auth_path(&paths, &id)).unwrap(), stored_before);
        assert_eq!(fs::read(&paths.current_auth).unwrap(), source_before);
        assert!(!paths.current_config.exists());
        assert_eq!(super::read_state(&paths).active_account_id.as_deref(), Some(id.as_str()));
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn runtime_capture_is_newer_only_same_account_and_never_selects_or_recreates() {
        let paths = test_paths();
        let id = super::save_login_account(&paths, dated_auth("one", "old", "2026-09-07T00:00:00Z")).unwrap();
        let source = dated_auth("one", "runtime-renewed", "2026-09-07T01:00:00Z");
        write_json_atomic(&paths.current_auth, &source).unwrap();
        let before = fs::read(&paths.current_auth).unwrap();
        assert!(super::capture_runtime_credential(&paths, &id).unwrap());
        assert_eq!(read_json(&managed_auth_path(&paths, &id)).unwrap()["tokens"]["access_token"], "runtime-renewed");
        assert!(!super::capture_runtime_credential(&paths, &id).unwrap());
        assert!(!paths.state_file.exists());
        assert_eq!(fs::read(&paths.current_auth).unwrap(), before);
        write_json_atomic(&paths.current_auth, &dated_auth("different", "different", "2026-09-07T02:00:00Z")).unwrap();
        assert!(!super::capture_runtime_credential(&paths, &id).unwrap());
        assert_eq!(read_json(&managed_auth_path(&paths, &id)).unwrap()["tokens"]["access_token"], "runtime-renewed");
        write_json_atomic(&paths.current_auth, &source).unwrap();
        fs::remove_file(managed_auth_path(&paths, &id)).unwrap();
        assert!(!super::capture_runtime_credential(&paths, &id).unwrap());
        assert!(!managed_auth_path(&paths, &id).exists());
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn unknown_age_and_corrupt_saved_credentials_are_not_silently_replaced() {
        let paths = test_paths();
        let id = super::save_login_account(&paths, login_auth("one", "keep-existing")).unwrap();
        write_json_atomic(&paths.current_auth, &dated_auth("one", "runtime", "2026-09-07T02:00:00Z")).unwrap();
        assert!(!super::capture_runtime_credential(&paths, &id).unwrap());
        assert_eq!(read_json(&managed_auth_path(&paths, &id)).unwrap()["tokens"]["access_token"], "keep-existing");
        fs::write(managed_auth_path(&paths, &id), b"{damaged record").unwrap();
        assert!(super::capture_runtime_credential(&paths, &id).is_err());
        assert_eq!(fs::read(managed_auth_path(&paths, &id)).unwrap(), b"{damaged record");
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn startup_capture_does_not_repair_codex_source_or_take_over_proxy_selection() {
        let paths = test_paths();
        let source = login_auth("one", "runtime");
        write_json_atomic(&paths.current_auth, &source).unwrap();
        let before = fs::read(&paths.current_auth).unwrap();
        let state = ManagerStateFile { active_account_id: Some("other-upstream".into()), local_proxy_enabled: true, ..Default::default() };
        super::write_state(&paths, &state).unwrap();
        let state_before = fs::read(&paths.state_file).unwrap();
        super::sync_current_into_store_for_paths(&paths, true).unwrap();
        assert_eq!(fs::read(&paths.current_auth).unwrap(), before);
        assert_eq!(fs::read(&paths.state_file).unwrap(), state_before);
        assert!(!paths.current_config.exists());
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn missing_refresh_time_uses_issued_at_not_expiry_or_email() {
        let auth = json!({"tokens": {"id_token": "e30.eyJpYXQiOjE3ODg3MjAwMDB9.signature"}});
        assert_eq!(super::auth_last_refresh(&auth).unwrap().timestamp(), 1788720000);
        let no_issuance = json!({"tokens": {"id_token": "e30.eyJleHAiOjE3ODg3MjAwMDB9.signature", "email": "same@example.com"}});
        assert!(super::auth_last_refresh(&no_issuance).is_none());
    }

    #[test]
    fn passive_import_does_not_replace_existing_codex_auth() {
        assert!(!should_activate_import(
            &ManagerStateFile::default(),
            false,
            true
        ));
    }

    #[test]
    fn passive_import_does_not_take_over_an_active_provider() {
        let state = ManagerStateFile {
            active_provider_id: Some("provider-1".to_string()),
            ..ManagerStateFile::default()
        };

        assert!(!should_activate_import(&state, false, false));
    }

    #[test]
    fn explicit_activation_still_replaces_existing_codex_auth() {
        assert!(should_activate_import(
            &ManagerStateFile::default(),
            true,
            true
        ));
    }

    #[test]
    fn proxy_login_auth_does_not_replace_the_active_upstream_account_on_startup() {
        let state = ManagerStateFile {
            active_account_id: Some("upstream-account".to_string()),
            local_proxy_enabled: true,
            local_proxy_openai_auth_account_id: Some("login-account".to_string()),
            ..ManagerStateFile::default()
        };

        assert!(!should_sync_current_as_active(
            &state,
            "login-account",
            false,
            false
        ));
    }
}
