use std::path::{Component, Path, PathBuf};

use capacity_migration::{dry_run_quotaviewer, LegacyDryRunReport};
use tauri::Runtime;

const MAX_LEGACY_SOURCE_PATH_CHARS: usize = 4096;

fn validate_legacy_source_path(path: &Path) -> Result<(), String> {
    let rendered = path.to_string_lossy();
    if !path.is_absolute()
        || rendered.is_empty()
        || rendered.chars().count() > MAX_LEGACY_SOURCE_PATH_CHARS
        || rendered.chars().any(char::is_control)
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("QuotaViewer 迁移源目录无效。".to_string());
    }
    Ok(())
}

fn default_legacy_source_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir()
            .map(|home| {
                home.join("Library")
                    .join("Application Support")
                    .join("CodexQuotaViewer")
            })
            .ok_or_else(|| "无法定位 QuotaViewer 默认数据目录。".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("此平台没有 QuotaViewer 默认数据目录，请选择迁移备份文件夹。".to_string())
    }
}

pub(crate) fn legacy_source_path(source_path: Option<String>) -> Result<PathBuf, String> {
    let path = match source_path {
        Some(value) if !value.trim().is_empty() => PathBuf::from(value),
        Some(_) => return Err("QuotaViewer 迁移源目录无效。".to_string()),
        None => default_legacy_source_path()?,
    };
    validate_legacy_source_path(&path)?;
    Ok(path)
}

pub(crate) fn inspect_legacy_quotaviewer_migration_blocking(
    source_path: Option<String>,
) -> Result<LegacyDryRunReport, String> {
    Ok(dry_run_quotaviewer(&legacy_source_path(source_path)?))
}

#[tauri::command]
pub(crate) async fn inspect_legacy_quotaviewer_migration<R: Runtime + 'static>(
    _app: tauri::AppHandle<R>,
    source_path: Option<String>,
) -> Result<LegacyDryRunReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        inspect_legacy_quotaviewer_migration_blocking(source_path)
    })
    .await
    .map_err(|error| format!("QuotaViewer migration inspection task failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_legacy_source_requires_a_safe_absolute_path() {
        assert!(legacy_source_path(Some("relative/source".to_string())).is_err());
        assert!(legacy_source_path(Some("/tmp/../source".to_string())).is_err());
        assert!(legacy_source_path(Some("/tmp/source\nother".to_string())).is_err());
        assert_eq!(
            legacy_source_path(Some("/tmp/source".to_string())).unwrap(),
            PathBuf::from("/tmp/source")
        );
    }

    #[test]
    fn desktop_inspection_preserves_the_versioned_redacted_report() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../fixtures/migration/source/ready")
            .canonicalize()
            .unwrap();
        let report = inspect_legacy_quotaviewer_migration_blocking(Some(
            source.to_string_lossy().into_owned(),
        ))
        .unwrap();
        assert_eq!(report.exit_code(), 0);
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(serialized.contains("\"schemaVersion\":\"1.0\""));
        assert!(serialized.contains("\"readOnly\":true"));
        assert!(!serialized.contains("alpha@example.test"));
        assert!(!serialized.contains("fixture-access-token"));
    }
}
