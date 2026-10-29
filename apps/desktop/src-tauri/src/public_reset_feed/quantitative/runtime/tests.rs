use super::*;
use std::cell::Cell;

#[test]
fn actual_progress_and_storage_are_projected_without_trusting_process_presence() {
    let wire = serde_json::json!({"health":"stalled", "issue":"evaluation_stalled",
        "last_capture_at":"2026-10-30T10:00:00Z", "last_report_at":"2026-10-30T09:00:00Z",
        "storage":{"observer_bytes":1000,"producer_bytes":2000,"log_bytes":400,
            "log_limit_bytes":5242880,"maintenance":[{"module":"observer","phase":"done"},
                {"module":"producer","phase":"vacuum","state":"waiting_for_disk_space"}]}});
    let progress: Progress = serde_json::from_value(wire).unwrap();
    let mut status = Status {
        state: "running".into(),
        ..Status::default()
    };
    progress.apply(&mut status);
    assert_eq!(status.health.as_deref(), Some("stalled"));
    assert_eq!(status.issue.as_deref(), Some("evaluation_stalled"));
    let value = serde_json::to_value(status).unwrap();
    assert_eq!(value["storage"]["logLimitBytes"], 5242880);
    assert_eq!(
        value["storage"]["maintenance"][1]["state"],
        "waiting_for_disk_space"
    );
    assert!(value["lastReportAt"].as_str().is_some());
}

#[test]
fn telemetry_does_not_expose_unknown_errors_or_local_paths() {
    let progress: Progress = serde_json::from_value(serde_json::json!({
        "health":"/private/config", "issue":"secret", "last_report_at":"private-path",
        "storage":{"observer_bytes":10,"producer_bytes":null,"log_bytes":2,"log_limit_bytes":5242880,
        "maintenance":[{"module":"observer","phase":"/private/db","error":"sqlite error /private/db","state":"secret"}]}
    })).unwrap();
    let mut status = Status::default();
    progress.apply(&mut status);
    let text = serde_json::to_string(&status).unwrap();
    assert!(!text.contains("private"));
    assert!(!text.contains("secret"));
    assert!(status.health.is_none());
    assert!(status.last_report_at.is_none());
}

#[test]
fn refresh_feedback_does_not_claim_cache_or_pending_collection_completed() {
    assert!(refresh_receipt(Ok("refreshed")).is_ok());
    assert!(refresh_receipt(Ok("partial")).is_ok());
    assert!(refresh_receipt(Ok("cached")).unwrap_err().contains("缓存"));
    assert!(refresh_receipt(Err("refresh_failed"))
        .unwrap_err()
        .contains("数据更新失败"));
    for state in ["refreshing", "starting", "running"] {
        assert!(refresh_receipt(Ok(state)).unwrap_err().contains("未完成"));
    }
}

#[cfg(unix)]
#[test]
fn refresh_receipts_distinguish_collection_cache_failure_and_process_start() {
    for (state, expected) in [
        ("refreshed", Ok("refreshed")),
        ("partial", Ok("partial")),
        ("cached", Ok("cached")),
        ("failed", Err("refresh_failed")),
        ("refreshing", Ok("refreshing")),
        ("already_running", Ok("running")),
        ("starting", Ok("starting")),
        ("restarting", Ok("starting")),
        ("unavailable", Err("start_failed")),
    ] {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", &format!("printf '{{\"state\":\"{state}\"}}'")]);
        assert_eq!(invoke(command, Duration::from_secs(1)), expected, "{state}");
    }
}

#[test]
fn bundled_install_starts_without_developer_configuration_and_respects_external_connection() {
    let root = tempfile::tempdir().unwrap();
    let resources = root.path().join("resources");
    let runtime = resources.join("reset-runtime");
    let site = runtime.join(if cfg!(windows) {
        "Lib/site-packages"
    } else {
        "lib/python3.11/site-packages"
    });
    fs::create_dir_all(runtime.join("bin")).unwrap();
    fs::create_dir_all(site.join("reset_observer")).unwrap();
    fs::write(
        runtime.join(if cfg!(windows) {
            "python.exe"
        } else {
            "bin/python3"
        }),
        "",
    )
    .unwrap();
    fs::write(site.join("reset_observer/__main__.py"), "").unwrap();
    fs::write(runtime.join("reset-runtime-manifest.json"), "{}").unwrap();
    let data = root.path().join("data");
    let mut supervisor = Supervisor::bundled(Some(resources), &data);
    supervisor.check(&data, Utc::now(), |config| {
        assert!(config.valid());
        assert!(config.command().get_args().any(|a| a == "-I"));
        assert!(config.command().get_args().any(|a| a == "utf8"));
        assert_eq!(config.module_directory, site);
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
