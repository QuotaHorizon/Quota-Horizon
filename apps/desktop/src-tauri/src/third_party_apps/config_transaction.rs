use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::Value;

static CONFIG_COMMIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A short-lived transaction for an explicitly requested client configuration
/// update. Before-images stay in memory; no backup files are retained.
#[derive(Default)]
pub(crate) struct ConfigTransaction {
    files: Vec<Change>,
}

struct Change {
    path: PathBuf,
    resolved: PathBuf,
    before: Option<Vec<u8>>,
    permissions: Option<fs::Permissions>,
    after: Option<Vec<u8>>,
    changed: bool,
}

fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("无法读取配置 {}：{error}", path.display())),
    }
}

// Follow existing symlinks rather than replacing a user's linked config file.
fn resolved_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_relative() {
        let current =
            std::env::current_dir().map_err(|error| format!("无法解析当前目录：{error}"))?;
        return resolved_path(&current.join(path));
    }
    match fs::canonicalize(path) {
        Ok(resolved) => Ok(resolved),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
                let target =
                    fs::read_link(path).map_err(|error| format!("无法读取配置链接：{error}"))?;
                let target = if target.is_absolute() {
                    target
                } else {
                    path.parent().ok_or("配置路径缺少父目录")?.join(target)
                };
                return resolved_path(&target);
            }
            let parent = path.parent().ok_or("配置路径缺少父目录")?;
            let name = path.file_name().ok_or("配置路径缺少文件名")?;
            if parent == path {
                return Err("无法解析配置路径".into());
            }
            Ok(resolved_path(parent)?.join(name))
        }
        Err(error) => Err(format!("无法解析配置路径 {}：{error}", path.display())),
    }
}

impl ConfigTransaction {
    pub(crate) fn read(&mut self, path: &Path) -> Result<Option<Vec<u8>>, String> {
        if let Some(change) = self.files.iter().find(|change| change.path == path) {
            return Ok(change.after.clone());
        }
        let resolved = resolved_path(path)?;
        let before = read_bytes(&resolved)?;
        let permissions = if before.is_some() {
            Some(
                fs::metadata(&resolved)
                    .map_err(|error| format!("无法读取配置权限：{error}"))?
                    .permissions(),
            )
        } else {
            None
        };
        self.files.push(Change {
            path: path.to_path_buf(),
            resolved,
            after: before.clone(),
            before: before.clone(),
            permissions,
            changed: false,
        });
        Ok(before)
    }

    pub(crate) fn read_json(&mut self, path: &Path) -> Result<Value, String> {
        match self.read(path)? {
            Some(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| format!("{} 不是有效 JSON", path.display())),
            None => Ok(serde_json::json!({})),
        }
    }

    pub(crate) fn write_json(&mut self, path: &Path, value: &Value) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|_| "无法序列化应用配置")?;
        self.stage(path, Some(bytes))
    }

    pub(crate) fn write_text(&mut self, path: &Path, text: &str) -> Result<(), String> {
        self.stage(path, Some(text.as_bytes().to_vec()))
    }

    pub(crate) fn remove(&mut self, path: &Path) -> Result<(), String> {
        self.stage(path, None)
    }

    fn stage(&mut self, path: &Path, after: Option<Vec<u8>>) -> Result<(), String> {
        self.read(path)?;
        let change = self
            .files
            .iter_mut()
            .find(|change| change.path == path)
            .expect("read captured the path");
        change.changed = change.before != after;
        change.after = after;
        Ok(())
    }

    pub(crate) fn commit(self) -> Result<(), String> {
        self.commit_with(|_, change| apply(change, change.after.as_deref()))
    }

    fn commit_with(
        self,
        mut write: impl FnMut(usize, &Change) -> Result<(), String>,
    ) -> Result<(), String> {
        let _guard = CONFIG_COMMIT_LOCK.lock().map_err(|_| "配置写入锁不可用")?;
        // Validate every input before the first mutation, including read-only
        // inputs used to prepare the update. Check again before each write.
        for change in &self.files {
            change.check_unchanged()?;
        }
        let mut attempted: Vec<&Change> = Vec::new();
        let mut created_directories = Vec::new();
        for (index, change) in self
            .files
            .iter()
            .filter(|change| change.changed)
            .enumerate()
        {
            let result = (|| {
                change.check_unchanged()?;
                if change.after.is_some() {
                    create_parents(&change.resolved, &mut created_directories)?;
                }
                attempted.push(change);
                write(index, change)
            })();
            if let Err(error) = result {
                let mut failures = Vec::new();
                for previous in attempted.iter().rev() {
                    if let Err(failure) = previous.restore() {
                        failures.push(failure);
                    }
                }
                for directory in created_directories.iter().rev() {
                    let _ = fs::remove_dir(directory);
                }
                return if failures.is_empty() {
                    Err(format!("配置更新失败，已恢复本次修改：{error}"))
                } else {
                    Err(format!(
                        "配置更新失败：{error}；部分配置需检查：{}",
                        failures.join("；")
                    ))
                };
            }
        }
        Ok(())
    }
}

impl Change {
    fn check_unchanged(&self) -> Result<(), String> {
        if resolved_path(&self.path)? != self.resolved || read_bytes(&self.resolved)? != self.before
        {
            return Err(format!(
                "{} 已被其他程序修改，请重新操作",
                self.path.display()
            ));
        }
        if let Some(permissions) = &self.permissions {
            if fs::metadata(&self.resolved)
                .map_err(|error| format!("无法读取配置权限：{error}"))?
                .permissions()
                != *permissions
            {
                return Err(format!("{} 的权限已改变，请重新操作", self.path.display()));
            }
        }
        Ok(())
    }

    fn restore(&self) -> Result<(), String> {
        let current = read_bytes(&self.resolved)?;
        if current == self.before {
            return Ok(());
        }
        if current != self.after {
            return Err(format!("{} 有外部修改，已保留", self.path.display()));
        }
        apply(self, self.before.as_deref())
    }
}

fn apply(change: &Change, bytes: Option<&[u8]>) -> Result<(), String> {
    match bytes {
        Some(bytes) => {
            crate::storage::write_bytes_atomic(&change.resolved, bytes)?;
            if let Some(permissions) = &change.permissions {
                fs::set_permissions(&change.resolved, permissions.clone())
                    .map_err(|error| format!("无法保留配置权限：{error}"))?;
            }
            Ok(())
        }
        None => match fs::remove_file(&change.resolved) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("无法移除配置 {}：{error}", change.path.display())),
        },
    }
}

fn create_parents(path: &Path, created: &mut Vec<PathBuf>) -> Result<(), String> {
    let parent = path.parent().ok_or("配置路径缺少父目录")?;
    if parent.is_dir() {
        return Ok(());
    }
    create_parents(parent, created)?;
    match fs::create_dir(parent) {
        Ok(()) => {
            created.push(parent.to_path_buf());
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists && parent.is_dir() => Ok(()),
        Err(error) => Err(format!("无法创建配置目录 {}：{error}", parent.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_write_failure_restores_exact_bytes_absence_and_permissions() {
        let root = tempfile::tempdir().unwrap();
        let existing = root.path().join("existing.json");
        let added = root.path().join("new").join("profile.json");
        let last = root.path().join("last.json");
        let original = b"{ \"keep\": true }\n";
        fs::write(&existing, original).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&existing, fs::Permissions::from_mode(0o640)).unwrap();
        }
        let mut tx = ConfigTransaction::default();
        tx.write_json(&existing, &serde_json::json!({"new": true}))
            .unwrap();
        tx.write_json(&added, &serde_json::json!({"new": true}))
            .unwrap();
        tx.write_json(&last, &serde_json::json!({})).unwrap();
        let result = tx.commit_with(|index, change| {
            if index == 2 {
                Err("injected write failure".into())
            } else {
                apply(change, change.after.as_deref())
            }
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&existing).unwrap(), original);
        assert!(!added.exists());
        assert!(!added.parent().unwrap().exists());
        assert!(!last.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(existing).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
    }

    #[test]
    fn external_edit_before_commit_leaves_every_file_untouched() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first.json");
        let last = root.path().join("last.json");
        fs::write(&first, "{\"keep\":1}").unwrap();
        fs::write(&last, "{}").unwrap();
        let mut tx = ConfigTransaction::default();
        tx.write_json(&first, &serde_json::json!({"changed":true}))
            .unwrap();
        tx.write_json(&last, &serde_json::json!({"changed":true}))
            .unwrap();
        fs::write(&last, "{\"external\":true}").unwrap();
        assert!(tx.commit().is_err());
        assert_eq!(fs::read_to_string(first).unwrap(), "{\"keep\":1}");
        assert_eq!(fs::read_to_string(last).unwrap(), "{\"external\":true}");
    }

    #[test]
    fn rollback_preserves_a_concurrent_external_edit() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first.json");
        let last = root.path().join("last.json");
        fs::write(&first, "{}").unwrap();
        let mut tx = ConfigTransaction::default();
        tx.write_json(&first, &serde_json::json!({"changed":true}))
            .unwrap();
        tx.write_json(&last, &serde_json::json!({})).unwrap();
        let error = tx
            .commit_with(|index, change| {
                if index == 1 {
                    fs::write(&first, "{\"external\":true}").unwrap();
                    Err("injected failure".into())
                } else {
                    apply(change, change.after.as_deref())
                }
            })
            .unwrap_err();
        assert!(error.contains("外部修改"));
        assert_eq!(fs::read_to_string(first).unwrap(), "{\"external\":true}");
    }

    #[test]
    fn a_failure_after_replacement_rolls_back_the_failed_file_too() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let original = b"{ \"old\": true }\n";
        fs::write(&path, original).unwrap();
        let mut tx = ConfigTransaction::default();
        tx.write_json(&path, &serde_json::json!({"new":true}))
            .unwrap();
        assert!(tx
            .commit_with(|_, change| {
                apply(change, change.after.as_deref())?;
                Err("injected post-replacement failure".into())
            })
            .is_err());
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn linked_configs_keep_the_link_and_write_its_target() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("settings.json");
        let link = root.path().join("client.json");
        fs::write(&target, b"{}").unwrap();
        symlink(&target, &link).unwrap();
        let mut tx = ConfigTransaction::default();
        tx.write_json(&link, &serde_json::json!({"new":true}))
            .unwrap();
        tx.commit().unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(target).unwrap()).unwrap(),
            serde_json::json!({"new":true})
        );
    }
}
