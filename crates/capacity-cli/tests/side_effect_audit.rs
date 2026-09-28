use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use capacity_cli::side_effect_audit::{
    AuditCategory, capture_protected_snapshot, compare_snapshots, protected_active_change_count,
    runtime_cache_active_change_count,
};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "capacity-side-effect-audit-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn redacted_diff_separates_control_active_and_runtime_cache_changes() {
    let directory = TestDirectory::new();
    let root = directory.path();
    fs::create_dir_all(root.join("sessions/2026/08/30")).unwrap();
    fs::create_dir_all(root.join("sqlite")).unwrap();
    fs::create_dir_all(root.join(".tmp")).unwrap();
    fs::write(root.join("auth.json"), b"fixture-auth-before").unwrap();
    fs::write(root.join("config.toml"), b"fixture-config-before").unwrap();
    fs::write(root.join("state_5.sqlite"), b"fixture-state").unwrap();

    let control_before = capture_protected_snapshot(root).unwrap();
    fs::write(root.join("config.toml"), b"fixture-config-control-change").unwrap();
    let active_before = capture_protected_snapshot(root).unwrap();
    fs::write(root.join("auth.json"), b"fixture-auth-active-change").unwrap();
    fs::write(
        root.join("sessions/2026/08/30/rollout-fixture.jsonl"),
        b"fixture-thread-active-change",
    )
    .unwrap();
    fs::write(
        root.join(".tmp/plugins.sha"),
        b"fixture-runtime-cache-change",
    )
    .unwrap();
    let active_after = capture_protected_snapshot(root).unwrap();

    let deltas = compare_snapshots(&control_before, &active_before, &active_after);
    let auth = deltas
        .iter()
        .find(|delta| delta.category == AuditCategory::Auth)
        .unwrap();
    let config = deltas
        .iter()
        .find(|delta| delta.category == AuditCategory::Config)
        .unwrap();
    assert_eq!(auth.control_changes, 0);
    assert_eq!(auth.active_content_changed, 1);
    assert_eq!(auth.active_only_changes, 1);
    assert_eq!(config.control_changes, 1);
    assert_eq!(config.active_changes, 0);
    assert_eq!(protected_active_change_count(&deltas), 2);
    assert_eq!(runtime_cache_active_change_count(&deltas), 1);

    let report_fragment = serde_json::to_string(&deltas).unwrap();
    for secret_or_path in [
        "auth.json",
        "config.toml",
        "rollout-fixture",
        "fixture-auth-active-change",
    ] {
        assert!(!report_fragment.contains(secret_or_path));
    }
}

#[test]
fn codex_read_path_has_no_common_production_file_write_api() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let app_owned_persistence =
        workspace.join("apps/capacity-preview/src-tauri/src/persistence.rs");
    let roots = [
        workspace.join("crates/capacity-domain/src"),
        workspace.join("crates/capacity-cli/src"),
        workspace.join("crates/codex-runtime/src"),
        workspace.join("apps/capacity-preview/src-tauri/src"),
    ];
    let forbidden = [
        "File::create(",
        "OpenOptions::",
        "fs::write(",
        "std::fs::write(",
        "fs::create_dir(",
        "fs::create_dir_all(",
        "fs::remove_file(",
        "fs::remove_dir(",
        "fs::remove_dir_all(",
        "fs::rename(",
        "fs::set_permissions(",
    ];

    for root in roots {
        for path in rust_files(&root) {
            if path == app_owned_persistence {
                continue;
            }
            let source = fs::read_to_string(&path).unwrap();
            let production = source.split("\n#[cfg(test)]").next().unwrap_or(&source);
            for token in forbidden {
                assert!(
                    !production.contains(token),
                    "production Codex read boundary contains forbidden write API {token}"
                );
            }
        }
    }
}

#[test]
fn app_owned_persistence_is_fixed_to_the_tauri_data_directory() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let persistence_path = workspace.join("apps/capacity-preview/src-tauri/src/persistence.rs");
    let persistence = fs::read_to_string(persistence_path).unwrap();
    let persistence = persistence
        .split("\n#[cfg(test)]\nmod tests")
        .next()
        .unwrap_or(&persistence);
    let desktop_entry =
        fs::read_to_string(workspace.join("apps/capacity-preview/src-tauri/src/lib.rs")).unwrap();
    let desktop_entry = desktop_entry
        .split("\n#[cfg(test)]\nmod tests")
        .next()
        .unwrap_or(&desktop_entry);

    assert!(persistence.contains("const STORE_FILENAME: &str = \"capacity.sqlite3\";"));
    assert!(persistence.contains("data_directory.join(STORE_FILENAME)"));
    assert!(desktop_entry.contains(".app_data_dir()"));
    assert!(desktop_entry.contains("const CAPACITY_DATA_DIRECTORY: &str = \"capacity-v1\";"));
    assert!(
        desktop_entry
            .contains("DesktopPersistence::open(&data_directory.join(CAPACITY_DATA_DIRECTORY))")
    );

    for codex_owned_target in [
        "\".codex\"",
        "\"auth.json\"",
        "\"config.toml\"",
        "\"sessions\"",
        "\"state_5.sqlite\"",
    ] {
        assert!(
            !persistence.contains(codex_owned_target),
            "app-owned persistence names a Codex-owned target {codex_owned_target}"
        );
    }
}

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}
