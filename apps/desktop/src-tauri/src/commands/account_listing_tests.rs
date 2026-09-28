use super::*;
use serde_json::json;

fn fixture(root: &Path) -> (Paths, String) {
    let paths = Paths {
        codex_home: root.join("codex"),
        current_auth: root.join("codex/auth.json"),
        current_config: root.join("codex/config.toml"),
        accounts: root.join("accounts"),
        providers: root.join("providers"),
        config_backup: root.join("backup.toml"),
        state_file: root.join("state.json"),
    };
    let auth = json!({"tokens": {"access_token": "synthetic-opaque-token", "chatgpt_user_id": "fixture-user", "account_id": "fixture-account", "email": "fixture@example.com"}});
    let id = account_fields(&auth).unwrap().3;
    write_json_atomic(&managed_auth_path(&paths, &id), &auth).unwrap();
    (paths, id)
}

#[test]
fn one_broken_record_does_not_hide_valid_accounts_or_erase_its_quota() {
    let root = tempfile::tempdir().unwrap();
    let (paths, healthy) = fixture(root.path());
    let damaged = "aaaaaaaaaaaaaaaaaaaaaaaa";
    let file = managed_auth_path(&paths, damaged);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, b"{incomplete json").unwrap();
    write_json_atomic(&usage_path(&paths, damaged), &json!({
        "primary": {"usedPercent": 72, "remainingPercent": 28, "windowMinutes": 10080, "resetsAt": 1789257600},
        "fetchedAt": "2026-09-06T10:00:00Z", "plan": "pro"
    })).unwrap();
    let rows = list_accounts_for_paths(&paths).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|row| row.id == healthy && row.usage.error.is_none()));
    let retained = rows.iter().find(|row| row.id == damaged).unwrap();
    assert_eq!(
        retained.usage.primary.as_ref().unwrap().remaining_percent,
        28.0
    );
    assert_eq!(
        retained.usage.fetched_at.as_deref(),
        Some("2026-09-06T10:00:00Z")
    );
    assert_eq!(
        retained.usage.error.as_deref(),
        Some("account_record_unreadable")
    );
    assert!(!retained.direct_switch_compatible);
    assert_eq!(fs::read(&file).unwrap(), b"{incomplete json");
}

#[test]
fn missing_login_with_cached_account_information_remains_visible() {
    let root = tempfile::tempdir().unwrap();
    let (paths, _) = fixture(root.path());
    let missing = "bbbbbbbbbbbbbbbbbbbbbbbb";
    write_json_atomic(&usage_path(&paths, missing), &json!({"plan": "plus"})).unwrap();
    let rows = list_accounts_for_paths(&paths).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|row| row.id == missing
            && row.usage.error.as_deref() == Some("account_record_unreadable")));
    assert!(!managed_auth_path(&paths, missing).exists());
}

#[cfg(unix)]
#[test]
fn keeping_damaged_records_does_not_relax_the_unsafe_link_guard() {
    let root = tempfile::tempdir().unwrap();
    let (paths, _) = fixture(root.path());
    std::os::unix::fs::symlink(root.path(), paths.accounts.join("unsafe-link")).unwrap();
    assert!(list_accounts_for_paths(&paths).is_err());
}
