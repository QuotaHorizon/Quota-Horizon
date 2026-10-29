use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, PartialEq)]
pub struct SourceIdentity {
    pub commit: Option<String>,
    pub dirty: Option<bool>,
    pub watch_paths: Vec<PathBuf>,
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let result = Command::new("git")
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .output()
        .ok()?;
    result
        .status
        .success()
        .then(|| String::from_utf8(result.stdout).ok())?
}

pub fn source_identity(root: &Path) -> SourceIdentity {
    let unknown = || SourceIdentity {
        commit: None,
        dirty: None,
        watch_paths: vec![],
    };
    let Some(top) = git(root, &["rev-parse", "--show-toplevel"]) else {
        return unknown();
    };
    // A source archive nested in another checkout must not inherit that repository's commit.
    if Path::new(top.trim()).canonicalize().ok() != root.canonicalize().ok() {
        return unknown();
    }
    let commit = git(root, &["rev-parse", "HEAD"])
        .map(|value| value.trim().to_owned())
        .filter(|value| value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit()));
    let dirty = git(
        root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )
    .map(|value| !value.trim().is_empty());
    let mut watch_paths = Vec::new();
    for name in [
        Some("HEAD".to_owned()),
        Some("index".to_owned()),
        git(root, &["symbolic-ref", "-q", "HEAD"]).map(|value| value.trim().to_owned()),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(path) = git(root, &["rev-parse", "--git-path", &name]) {
            watch_paths.push(root.join(path.trim()));
        }
    }
    SourceIdentity {
        commit,
        dirty,
        watch_paths,
    }
}

pub fn emit(root: &Path) {
    let identity = source_identity(root);
    for path in identity.watch_paths {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    // Frontend bundles and embedded Python modules can change without a Rust source edit.
    for path in [
        "apps/desktop/dist",
        "apps/desktop/src",
        "apps/desktop/src-tauri/src",
        "apps/desktop/src-tauri/build.rs",
        "apps/desktop/src-tauri/build_identity.rs",
        "apps/capacity-preview/src-tauri/src",
        "apps/capacity-preview/src/status.ts",
        "crates",
        "modules",
        "scripts/build-desktop-app.mjs",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    let built_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system build clock")
        .as_millis();
    println!(
        "cargo:rustc-env=QH_BUILD_COMMIT={}",
        identity.commit.as_deref().unwrap_or("")
    );
    println!(
        "cargo:rustc-env=QH_BUILD_DIRTY={}",
        match identity.dirty {
            Some(true) => "true",
            Some(false) => "false",
            None => "unknown",
        }
    );
    println!("cargo:rustc-env=QH_BUILD_AT_MS={built_at}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new(label: &str) -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("qh-build-{label}-{}-{stamp}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn git(&self, args: &[&str]) {
            assert!(git(&self.0, args).is_some());
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn distinguishes_clean_tracked_and_untracked_changes_without_reading_private_content() {
        let fixture = Fixture::new("git");
        fixture.git(&["init", "-q"]);
        fs::write(fixture.0.join("source"), "first").unwrap();
        fs::write(fixture.0.join(".gitignore"), "private\n").unwrap();
        fixture.git(&["add", "source", ".gitignore"]);
        fixture.git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "Fixture",
        ]);
        let clean = source_identity(&fixture.0);
        assert_eq!(clean.commit.as_ref().map(String::len), Some(40));
        assert_eq!(clean.dirty, Some(false));
        fs::write(fixture.0.join("private"), "ignored").unwrap();
        assert_eq!(source_identity(&fixture.0).dirty, Some(false));
        fs::write(fixture.0.join("source"), "second").unwrap();
        let modified = source_identity(&fixture.0);
        assert_eq!(modified.commit, clean.commit);
        assert_eq!(modified.dirty, Some(true));
        fs::write(fixture.0.join("source"), "first").unwrap();
        fs::write(fixture.0.join("new-source"), "third").unwrap();
        assert_eq!(source_identity(&fixture.0).dirty, Some(true));
    }

    #[test]
    fn source_archives_and_nested_non_repository_roots_have_unknown_identity() {
        let fixture = Fixture::new("archive");
        assert_eq!(source_identity(&fixture.0).commit, None);
        assert_eq!(source_identity(&fixture.0).dirty, None);
        fixture.git(&["init", "-q"]);
        let nested = fixture.0.join("archive");
        fs::create_dir(&nested).unwrap();
        assert_eq!(source_identity(&nested).dirty, None);
    }
}
