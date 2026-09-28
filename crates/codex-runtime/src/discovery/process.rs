use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::ProbeFailure;

pub(super) fn run_bounded(
    executable: &Path,
    arguments: &[&OsStr],
    timeout: Duration,
    stdout_limit: usize,
) -> Result<Vec<u8>, ProbeFailure> {
    let current_directory = env::current_dir().unwrap_or_else(|_| env::temp_dir());
    let sanitized_path = sanitized_path(env::var_os("PATH"), &current_directory);
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .env_clear()
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(path) = sanitized_path {
        command.env("PATH", path);
    }

    let mut child = command.spawn().map_err(classify_spawn_error)?;

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child.wait_with_output().map_err(|_| ProbeFailure::Io)?;
                if !status.success() {
                    return Err(ProbeFailure::NonZeroExit(status.code()));
                }
                if output.stdout.len() > stdout_limit {
                    return Err(ProbeFailure::OutputTooLarge);
                }
                return Ok(output.stdout);
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProbeFailure::Timeout);
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProbeFailure::Io);
            }
        }
    }
}

fn sanitized_path(raw_path: Option<OsString>, current_directory: &Path) -> Option<OsString> {
    let raw_path = raw_path?;
    let canonical_current_directory = fs::canonicalize(current_directory).ok();
    let mut accepted = Vec::<PathBuf>::new();

    for directory in env::split_paths(&raw_path) {
        if directory.as_os_str().is_empty() || !directory.is_absolute() {
            continue;
        }
        if canonical_current_directory.as_ref().is_some_and(|cwd| {
            fs::canonicalize(&directory)
                .ok()
                .is_some_and(|candidate| candidate == *cwd)
        }) {
            continue;
        }
        if !accepted.contains(&directory) {
            accepted.push(directory);
        }
    }

    if accepted.is_empty() {
        None
    } else {
        env::join_paths(accepted).ok()
    }
}

fn classify_spawn_error(error: io::Error) -> ProbeFailure {
    match error.kind() {
        io::ErrorKind::NotFound => ProbeFailure::NotFound,
        io::ErrorKind::PermissionDenied => ProbeFailure::PermissionDenied,
        _ => ProbeFailure::Io,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_sanitizer_removes_relative_empty_and_current_directory_entries() {
        let root = tempfile::TempDir::new().unwrap();
        let base = root.path().to_path_buf();
        let safe = base.join("safe-bin");
        fs::create_dir(&safe).unwrap();
        let raw = env::join_paths([
            Path::new(""),
            Path::new("relative-bin"),
            base.as_path(),
            safe.as_path(),
            safe.as_path(),
        ])
        .unwrap();

        let sanitized = sanitized_path(Some(raw), &base).unwrap();
        let entries: Vec<_> = env::split_paths(&sanitized).collect();

        assert_eq!(entries, vec![safe]);
    }
}
