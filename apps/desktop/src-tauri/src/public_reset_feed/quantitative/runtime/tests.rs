use super::*;
use std::cell::Cell;

#[test]
fn bundled_install_starts_without_developer_configuration_and_respects_external_connection() {
    let root = tempfile::tempdir().unwrap();
    let resources = root.path().join("resources");
    let runtime = resources.join("reset-runtime");
    fs::create_dir_all(runtime.join("bin")).unwrap();
    fs::create_dir_all(runtime.join("lib/python3.11/site-packages/reset_observer")).unwrap();
    fs::write(runtime.join("bin/python3"), "").unwrap();
    fs::write(
        runtime.join("lib/python3.11/site-packages/reset_observer/__main__.py"),
        "",
    )
    .unwrap();
    fs::write(runtime.join("reset-runtime-manifest.json"), "{}").unwrap();
    let data = root.path().join("data");
    let mut supervisor = Supervisor::bundled(Some(resources), &data);
    supervisor.check(&data, Utc::now(), |config| {
        assert!(config.valid());
        assert!(config.command().get_args().any(|a| a == "-I"));
        assert_eq!(
            config.database_path,
            data.join("reset-observer/state.sqlite")
        );
        Ok("starting")
    });
    assert!(supervisor.state.managed);
    fs::write(data.join("reset-radar-connection.json"), "{}").unwrap();
    supervisor.check(&data, Utc::now(), |_| {
        panic!("external connection must be preserved")
    });
    assert!(!supervisor.state.managed);
}

fn configured(root: &Path) -> Configuration {
    fs::create_dir(root.join("reset_observer")).unwrap();
    fs::write(root.join("reset_observer/__main__.py"), "").unwrap();
    fs::write(root.join("python"), "").unwrap();
    let value = serde_json::json!({"pythonPath":root.join("python"),"moduleDirectory":root,
        "databasePath":root.join("observer.sqlite"),"producerDatabasePath":root.join("producer.sqlite"),
        "configPath":root.join("private config.json"),"outputPath":root.join("report.json"),"htmlPath":null});
    fs::write(root.join("reset-radar-runtime.json"), value.to_string()).unwrap();
    serde_json::from_value(value).unwrap()
}
#[test]
fn supervision_is_opt_in_and_never_starts_from_an_export() {
    let root = tempfile::tempdir().unwrap();
    let mut supervisor = Supervisor::default();
    supervisor.check(root.path(), Utc::now(), |_| panic!("not configured"));
    assert!(!supervisor.state.managed);
    fs::write(root.path().join("report.json"), r#"{"command":"ignored"}"#).unwrap();
    supervisor.check(root.path(), Utc::now(), |_| {
        panic!("exports cannot start processes")
    });
    assert_eq!(supervisor.state.state, "external");
}
#[test]
fn recovery_backs_off_and_rechecks_after_wake_without_replaying_missed_ticks() {
    let root = tempfile::tempdir().unwrap();
    configured(root.path());
    let mut supervisor = Supervisor::default();
    let t = Utc::now();
    let calls = Cell::new(0);
    let fail = |_: &Configuration| {
        calls.set(calls.get() + 1);
        Err("start_failed")
    };
    supervisor.check(root.path(), t, fail);
    supervisor.check(root.path(), t + chrono::Duration::seconds(59), fail);
    assert_eq!(calls.get(), 1);
    supervisor.check(root.path(), t + chrono::Duration::seconds(60), fail);
    assert_eq!(
        supervisor.next_check,
        Some(t + chrono::Duration::seconds(180))
    );
    supervisor.check(root.path(), t + chrono::Duration::hours(9), |_| {
        Ok("running")
    });
    assert_eq!(supervisor.failures, 0);
    assert_eq!(supervisor.state.state, "running");
    assert!(supervisor.state.issue.is_none());
}
#[test]
fn command_preserves_paths_and_ui_status_does_not_disclose_them() {
    let root = tempfile::tempdir().unwrap();
    let config = configured(root.path());
    let command = config.command();
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args[0], "-m");
    assert!(args.contains(&root.path().join("private config.json").as_os_str()));
    let mut supervisor = Supervisor::default();
    supervisor.check(root.path(), Utc::now(), |_| Ok("starting"));
    let json = serde_json::to_string(&supervisor.state).unwrap();
    assert!(!json.contains(&root.path().display().to_string()));
    assert!(!json.contains("python"));
    fs::remove_file(root.path().join("reset-radar-runtime.json")).unwrap();
    supervisor.check(root.path(), Utc::now(), |_| panic!("detached"));
    assert!(!supervisor.state.managed);
}
#[cfg(unix)]
#[test]
fn process_failures_and_timeouts_are_bounded_and_sanitized() {
    fn shell(script: &str) -> Command {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", script]);
        c
    }
    assert_eq!(
        invoke(
            shell("printf '{\"state\":\"already_running\"}'"),
            Duration::from_secs(1)
        ),
        Ok("running")
    );
    assert_eq!(
        invoke(
            shell("printf 'private text'; exit 1"),
            Duration::from_secs(1)
        ),
        Err("start_failed")
    );
    assert_eq!(
        invoke(shell("exec sleep 3"), Duration::from_millis(60)),
        Err("start_timeout")
    );
}
