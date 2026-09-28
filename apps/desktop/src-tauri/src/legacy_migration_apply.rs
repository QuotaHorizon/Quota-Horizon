use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

use capacity_domain::{UtcTimestamp, WorkSchedule, WorkSchedulePeriod};
use capacity_migration::{
    read_importable_preferences, LegacyAppLanguage, LegacyImportPreferences,
    LegacyResolvedLanguage, LegacyStatusItemStyle,
};
use capacity_store::{
    CapacityStore, MonitorSettings, MonitorSettingsUpdate, SettingsLanguage, SettingsUpdateOutcome,
    WorkScheduleSettings, WorkScheduleSettingsUpdate, WorkScheduleSettingsUpdateOutcome,
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, Runtime};
use uuid::Uuid;

use crate::autostart;

const CONFIRMATION_PHRASE: &str = "IMPORT";
const ROLLBACK_PHRASE: &str = "ROLLBACK";
const CONFIRMATION_TTL_SECONDS: i64 = 120;
const CAPACITY_DIRECTORY: &str = "capacity-v1";
const CAPACITY_DATABASE: &str = "capacity.sqlite3";
const MIGRATION_DIRECTORY: &str = "legacy-migration-v1";
const OPERATIONS_DIRECTORY: &str = "operations";
const JOURNAL_FILE: &str = "journal.json";
const SETTINGS_BACKUP_FILE: &str = "app-settings.before";
const LATEST_FILE: &str = "latest.json";
const MAX_APP_SETTINGS_BYTES: u64 = 4 * 1024 * 1024;
const MAX_JOURNAL_BYTES: u64 = 512 * 1024;

static PENDING_IMPORTS: LazyLock<Mutex<HashMap<String, PendingImport>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
struct PendingImport {
    source_path: PathBuf,
    source_revision: String,
    expected: CapturedTargets,
    desired: DesiredTargets,
    changed_targets: Vec<String>,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct CapturedTargets {
    app_settings_bytes: Option<Vec<u8>>,
    app_settings_value: Value,
    app_settings_revision: String,
    app_settings_mode: Option<u32>,
    launch_at_login: bool,
    monitor: MonitorSnapshot,
    schedule: ScheduleSnapshot,
}

#[derive(Debug, Clone)]
struct DesiredTargets {
    app_settings_bytes: Vec<u8>,
    app_settings_value: Value,
    launch_at_login: bool,
    monitor: MonitorSnapshot,
    schedule: ScheduleSnapshot,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LegacyMigrationApplyPreview {
    schema_version: &'static str,
    status: &'static str,
    confirm_token: Option<String>,
    expires_at: Option<String>,
    typed_confirmation: &'static str,
    changed_targets: Vec<String>,
    review_targets_excluded: Vec<&'static str>,
    imported: LegacyMigrationImportSummary,
    creates_backup: bool,
    automatic_rollback: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LegacyMigrationImportSummary {
    auto_refresh_enabled: Option<bool>,
    refresh_interval_seconds: Option<u32>,
    launch_at_login: Option<bool>,
    language: Option<String>,
    legacy_status_item_style: Option<&'static str>,
    status_item_policy: &'static str,
    work_plan_enabled: Option<bool>,
    off_period_count: usize,
    session_language: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LegacyMigrationApplyResult {
    schema_version: &'static str,
    status: &'static str,
    operation_id: Option<String>,
    changed_targets: Vec<String>,
    source_unchanged: bool,
    rollback_available: bool,
    language: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LegacyMigrationOperationView {
    schema_version: &'static str,
    status: &'static str,
    operation_id: Option<String>,
    changed_targets: Vec<String>,
    rollback_available: bool,
    recovery_required: bool,
    typed_rollback: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalState {
    Prepared,
    Applying,
    Committed,
    RolledBack,
    NeedsReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppFileObservation {
    exists: bool,
    revision: String,
    mode: Option<u32>,
}

struct RegularFileContents {
    bytes: Vec<u8>,
    mode: Option<u32>,
}

struct AppSettingsTarget {
    bytes: Option<Vec<u8>>,
    value: Value,
    mode: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MonitorSnapshot {
    revision: u32,
    auto_refresh_enabled: bool,
    refresh_interval_seconds: u32,
    notification_threshold_basis_points: Option<u16>,
    reset_credit_notice_hours: u16,
    quiet_hours_enabled: bool,
    quiet_hours_start_minute: Option<u16>,
    quiet_hours_end_minute: Option<u16>,
    language: String,
    lock_screen_privacy: bool,
    launch_at_login: bool,
    history_retention_days: u16,
    updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SchedulePeriodSnapshot {
    start_minute_of_day: u16,
    end_minute_of_day: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScheduleSnapshot {
    revision: u32,
    enabled: bool,
    off_periods: Vec<SchedulePeriodSnapshot>,
    updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DurableTargets {
    app_settings: AppFileObservation,
    launch_at_login: bool,
    monitor: MonitorSnapshot,
    schedule: ScheduleSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MigrationJournal {
    schema_version: String,
    operation_id: String,
    state: JournalState,
    source_revision: String,
    created_at: String,
    updated_at: String,
    changed_targets: Vec<String>,
    before: DurableTargets,
    after: DurableTargets,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LatestOperation {
    schema_version: String,
    operation_id: String,
}

trait StartupPreference {
    fn is_enabled(&mut self) -> Result<bool, String>;
    fn apply(&mut self, enabled: bool) -> Result<(), String>;
}

struct TauriStartupPreference<'a, R: Runtime> {
    app: &'a AppHandle<R>,
}

impl<R: Runtime> StartupPreference for TauriStartupPreference<'_, R> {
    fn is_enabled(&mut self) -> Result<bool, String> {
        autostart::preference_enabled(self.app)
            .map_err(|_| "无法读取当前登录启动状态。".to_string())
    }

    fn apply(&mut self, enabled: bool) -> Result<(), String> {
        autostart::apply_preference(self.app, enabled)
            .map_err(|_| "无法更新登录启动状态。".to_string())
    }
}

fn now_timestamp() -> Result<UtcTimestamp, String> {
    UtcTimestamp::parse(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true))
        .map_err(|_| "无法创建迁移时间戳。".to_string())
}

fn app_data_paths<R: Runtime>(app: &AppHandle<R>) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|_| "无法定位 QuotaHorizon 数据目录。".to_string())?;
    Ok((
        app_data.join("settings.json"),
        app_data.join(CAPACITY_DIRECTORY).join(CAPACITY_DATABASE),
        app_data.join(MIGRATION_DIRECTORY),
    ))
}

fn source_revision(preferences: &LegacyImportPreferences) -> Result<String, String> {
    let bytes = serde_json::to_vec(preferences)
        .map_err(|_| "无法固定 QuotaViewer 迁移计划。".to_string())?;
    Ok(sha256_revision(true, &bytes))
}

fn sha256_revision(exists: bool, bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(if exists { b"present:" } else { b"missing:" });
    digest.update(bytes);
    let digest = digest.finalize();
    format!("sha256:{digest:x}")
}

fn read_optional_regular_file(
    path: &Path,
    maximum: u64,
) -> Result<Option<RegularFileContents>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取迁移目标状态。".to_string()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > maximum {
        return Err("迁移目标文件不安全或超过大小上限。".to_string());
    }
    let mut file = File::open(path).map_err(|_| "无法读取迁移目标状态。".to_string())?;
    let opened = file
        .metadata()
        .map_err(|_| "无法验证迁移目标状态。".to_string())?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取迁移目标状态。".to_string())?;
    let current =
        fs::symlink_metadata(path).map_err(|_| "迁移目标在读取时发生变化。".to_string())?;
    if bytes.len() as u64 > maximum
        || current.file_type().is_symlink()
        || !current.is_file()
        || !same_file_identity(&opened, &current)
        || file_mode(&opened) != file_mode(&current)
    {
        return Err("迁移目标在读取时发生变化。".to_string());
    }
    Ok(Some(RegularFileContents {
        bytes,
        mode: file_mode(&opened),
    }))
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn file_mode(_: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn same_file_identity(opened: &fs::Metadata, current: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    opened.dev() == current.dev()
        && opened.ino() == current.ino()
        && opened.len() == current.len()
        && opened.modified().ok() == current.modified().ok()
}

#[cfg(not(unix))]
fn same_file_identity(opened: &fs::Metadata, current: &fs::Metadata) -> bool {
    opened.len() == current.len() && opened.modified().ok() == current.modified().ok()
}

fn read_app_settings_target(path: &Path) -> Result<AppSettingsTarget, String> {
    let contents = read_optional_regular_file(path, MAX_APP_SETTINGS_BYTES)?;
    let value = match contents.as_ref() {
        Some(contents) => serde_json::from_slice::<Value>(&contents.bytes)
            .map_err(|_| "QuotaHorizon 设置文件不是有效 JSON。".to_string())?,
        None => Value::Object(Map::new()),
    };
    if !value.is_object() {
        return Err("QuotaHorizon 设置文件不是 JSON 对象。".to_string());
    }
    let mode = contents.as_ref().and_then(|contents| contents.mode);
    Ok(AppSettingsTarget {
        bytes: contents.map(|contents| contents.bytes),
        value,
        mode,
    })
}

fn language_name(value: LegacyResolvedLanguage) -> &'static str {
    match value {
        LegacyResolvedLanguage::English => "en",
        LegacyResolvedLanguage::Chinese => "zh",
    }
}

fn resolved_import_language(
    preferences: &LegacyImportPreferences,
    current: &Value,
) -> Option<String> {
    let settings = preferences.settings.as_ref();
    match settings.map(|settings| settings.language) {
        Some(LegacyAppLanguage::English) => Some("en".to_string()),
        Some(LegacyAppLanguage::Chinese) => Some("zh".to_string()),
        Some(LegacyAppLanguage::System) => preferences
            .session_language
            .or(settings.and_then(|settings| settings.last_resolved_language))
            .map(|language| language_name(language).to_string())
            .or_else(|| {
                current
                    .get("language")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
        None => preferences
            .session_language
            .map(|language| language_name(language).to_string()),
    }
    .filter(|language| matches!(language.as_str(), "en" | "zh"))
}

fn monitor_language(value: LegacyAppLanguage) -> &'static str {
    match value {
        LegacyAppLanguage::System => "system",
        LegacyAppLanguage::English => "en",
        LegacyAppLanguage::Chinese => "zh-CN",
    }
}

fn status_item_style(value: LegacyStatusItemStyle) -> &'static str {
    match value {
        LegacyStatusItemStyle::Meter => "meter",
        LegacyStatusItemStyle::Text => "text",
    }
}

fn settings_language_name(value: SettingsLanguage) -> &'static str {
    match value {
        SettingsLanguage::System => "system",
        SettingsLanguage::English => "en",
        SettingsLanguage::SimplifiedChinese => "zh-CN",
    }
}

fn parse_settings_language(value: &str) -> Result<SettingsLanguage, String> {
    match value {
        "system" => Ok(SettingsLanguage::System),
        "en" => Ok(SettingsLanguage::English),
        "zh-CN" => Ok(SettingsLanguage::SimplifiedChinese),
        _ => Err("迁移日志中的语言值无效。".to_string()),
    }
}

impl From<MonitorSettings> for MonitorSnapshot {
    fn from(value: MonitorSettings) -> Self {
        Self {
            revision: value.revision,
            auto_refresh_enabled: value.auto_refresh_enabled,
            refresh_interval_seconds: value.refresh_interval_seconds,
            notification_threshold_basis_points: value.notification_threshold_basis_points,
            reset_credit_notice_hours: value.reset_credit_notice_hours,
            quiet_hours_enabled: value.quiet_hours_enabled,
            quiet_hours_start_minute: value.quiet_hours_start_minute,
            quiet_hours_end_minute: value.quiet_hours_end_minute,
            language: settings_language_name(value.language).to_string(),
            lock_screen_privacy: value.lock_screen_privacy,
            launch_at_login: value.launch_at_login,
            history_retention_days: value.history_retention_days,
            updated_at: value.updated_at.as_str().to_string(),
        }
    }
}

impl From<WorkScheduleSettings> for ScheduleSnapshot {
    fn from(value: WorkScheduleSettings) -> Self {
        Self {
            revision: value.revision,
            enabled: value.schedule.enabled,
            off_periods: value
                .schedule
                .off_periods
                .into_iter()
                .map(|period| SchedulePeriodSnapshot {
                    start_minute_of_day: period.start_minute_of_day,
                    end_minute_of_day: period.end_minute_of_day,
                })
                .collect(),
            updated_at: value.updated_at.as_str().to_string(),
        }
    }
}

impl MonitorSnapshot {
    fn same_values(&self, other: &Self) -> bool {
        let mut left = self.clone();
        let mut right = other.clone();
        left.revision = 0;
        right.revision = 0;
        left.updated_at.clear();
        right.updated_at.clear();
        left == right
    }

    fn update(&self, expected_revision: u32) -> Result<MonitorSettingsUpdate, String> {
        Ok(MonitorSettingsUpdate {
            expected_revision,
            auto_refresh_enabled: self.auto_refresh_enabled,
            refresh_interval_seconds: self.refresh_interval_seconds,
            notification_threshold_basis_points: self.notification_threshold_basis_points,
            reset_credit_notice_hours: self.reset_credit_notice_hours,
            quiet_hours_enabled: self.quiet_hours_enabled,
            quiet_hours_start_minute: self.quiet_hours_start_minute,
            quiet_hours_end_minute: self.quiet_hours_end_minute,
            language: parse_settings_language(&self.language)?,
            lock_screen_privacy: self.lock_screen_privacy,
            launch_at_login: self.launch_at_login,
            history_retention_days: self.history_retention_days,
        })
    }
}

impl ScheduleSnapshot {
    fn same_values(&self, other: &Self) -> bool {
        self.enabled == other.enabled && self.off_periods == other.off_periods
    }

    fn update(&self, expected_revision: u32) -> Result<WorkScheduleSettingsUpdate, String> {
        let off_periods = self
            .off_periods
            .iter()
            .map(|period| {
                WorkSchedulePeriod::new(period.start_minute_of_day, period.end_minute_of_day)
                    .map_err(|_| "迁移日程包含无效停用时段。".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WorkScheduleSettingsUpdate {
            expected_revision,
            enabled: self.enabled,
            off_periods,
        })
    }
}

fn capture_targets<P: StartupPreference>(
    app_settings_path: &Path,
    capacity_database_path: &Path,
    startup: &mut P,
) -> Result<CapturedTargets, String> {
    let app_settings = read_app_settings_target(app_settings_path)?;
    let app_settings_revision = sha256_revision(
        app_settings.bytes.is_some(),
        app_settings.bytes.as_deref().unwrap_or_default(),
    );
    let store = CapacityStore::open(capacity_database_path)
        .map_err(|_| "无法打开 QuotaHorizon 容量数据库。".to_string())?;
    Ok(CapturedTargets {
        app_settings_bytes: app_settings.bytes,
        app_settings_value: app_settings.value,
        app_settings_revision,
        app_settings_mode: app_settings.mode,
        launch_at_login: startup.is_enabled()?,
        monitor: store
            .settings()
            .map(MonitorSnapshot::from)
            .map_err(|_| "无法读取 QuotaHorizon 监控设置。".to_string())?,
        schedule: store
            .work_schedule()
            .map(ScheduleSnapshot::from)
            .map_err(|_| "无法读取 QuotaHorizon Work Plan。".to_string())?,
    })
}

fn desired_targets(
    expected: &CapturedTargets,
    preferences: &LegacyImportPreferences,
) -> Result<(DesiredTargets, LegacyMigrationImportSummary, Vec<String>), String> {
    let mut app_settings_value = expected.app_settings_value.clone();
    let app_settings = app_settings_value
        .as_object_mut()
        .expect("captured settings is an object");
    let resolved_language = resolved_import_language(preferences, &expected.app_settings_value);
    if let Some(language) = resolved_language.as_ref() {
        app_settings.insert("language".to_string(), Value::String(language.clone()));
    }

    let mut monitor = expected.monitor.clone();
    let mut schedule = expected.schedule.clone();
    let mut launch_at_login = expected.launch_at_login;
    if let Some(settings) = preferences.settings.as_ref() {
        monitor.auto_refresh_enabled = settings.auto_refresh_enabled;
        monitor.refresh_interval_seconds = settings.refresh_interval_seconds;
        monitor.language = monitor_language(settings.language).to_string();
        monitor.launch_at_login = settings.launch_at_login;
        let mut imported_periods = settings
            .off_periods
            .iter()
            .map(|period| SchedulePeriodSnapshot {
                start_minute_of_day: period.start_minute_of_day,
                end_minute_of_day: period.end_minute_of_day,
            })
            .collect::<Vec<_>>();
        imported_periods.retain(|period| period.start_minute_of_day != period.end_minute_of_day);
        imported_periods
            .sort_by_key(|period| (period.start_minute_of_day, period.end_minute_of_day));
        imported_periods.dedup();
        let imported_schedule = WorkSchedule::new(
            settings.work_plan_enabled,
            imported_periods
                .iter()
                .map(|period| {
                    WorkSchedulePeriod::new(period.start_minute_of_day, period.end_minute_of_day)
                        .expect("legacy scanner already bounded every work period")
                })
                .collect(),
        )
        .map_err(|_| "Viewer Work Plan 超过 QuotaHorizon 的安全导入上限。".to_string())?;
        schedule.enabled = imported_schedule.enabled;
        schedule.off_periods = imported_schedule
            .off_periods
            .into_iter()
            .map(|period| SchedulePeriodSnapshot {
                start_minute_of_day: period.start_minute_of_day,
                end_minute_of_day: period.end_minute_of_day,
            })
            .collect();
        launch_at_login = settings.launch_at_login;
        app_settings.insert(
            "launchAtStartup".to_string(),
            Value::Bool(settings.launch_at_login),
        );
    }

    let app_settings_bytes = serde_json::to_vec_pretty(&app_settings_value)
        .map_err(|_| "无法生成迁移后的 QuotaHorizon 设置。".to_string())?;
    let desired = DesiredTargets {
        app_settings_bytes,
        app_settings_value,
        launch_at_login,
        monitor,
        schedule,
    };
    let mut changed_targets = Vec::new();
    if !expected.monitor.same_values(&desired.monitor) {
        changed_targets.push("monitor_settings".to_string());
    }
    if expected.app_settings_value != desired.app_settings_value
        || expected.launch_at_login != desired.launch_at_login
    {
        changed_targets.push("shell_preferences".to_string());
    }
    if !expected.schedule.same_values(&desired.schedule) {
        changed_targets.push("work_plan".to_string());
    }
    if preferences.session_language.is_some()
        && expected.app_settings_value.get("language") != desired.app_settings_value.get("language")
    {
        changed_targets.push("session_preferences".to_string());
    }
    let settings = preferences.settings.as_ref();
    let summary = LegacyMigrationImportSummary {
        auto_refresh_enabled: settings.map(|settings| settings.auto_refresh_enabled),
        refresh_interval_seconds: settings.map(|settings| settings.refresh_interval_seconds),
        launch_at_login: settings.map(|settings| settings.launch_at_login),
        language: resolved_language,
        legacy_status_item_style: settings
            .map(|settings| status_item_style(settings.status_item_style)),
        status_item_policy: "dynamic_capacity",
        work_plan_enabled: settings.map(|settings| settings.work_plan_enabled),
        off_period_count: settings.map_or(0, |settings| settings.off_periods.len()),
        session_language: preferences.session_language.map(language_name),
    };
    Ok((desired, summary, changed_targets))
}

fn target_precondition_matches(current: &CapturedTargets, expected: &CapturedTargets) -> bool {
    current.app_settings_revision == expected.app_settings_revision
        && current.app_settings_mode == expected.app_settings_mode
        && current.launch_at_login == expected.launch_at_login
        && current.monitor == expected.monitor
        && current.schedule == expected.schedule
}

fn predicted_after(
    expected: &CapturedTargets,
    desired: &DesiredTargets,
    updated_at: &UtcTimestamp,
) -> DurableTargets {
    let mut monitor = desired.monitor.clone();
    if !expected.monitor.same_values(&desired.monitor) {
        monitor.revision = expected.monitor.revision.saturating_add(1);
        monitor.updated_at = updated_at.as_str().to_string();
    }
    let mut schedule = desired.schedule.clone();
    if !expected.schedule.same_values(&desired.schedule) {
        schedule.revision = expected.schedule.revision.saturating_add(1);
        schedule.updated_at = updated_at.as_str().to_string();
    }
    DurableTargets {
        app_settings: if expected.app_settings_value == desired.app_settings_value {
            AppFileObservation {
                exists: expected.app_settings_bytes.is_some(),
                revision: expected.app_settings_revision.clone(),
                mode: expected.app_settings_mode,
            }
        } else {
            AppFileObservation {
                exists: true,
                revision: sha256_revision(true, &desired.app_settings_bytes),
                mode: target_file_mode(),
            }
        },
        launch_at_login: desired.launch_at_login,
        monitor,
        schedule,
    }
}

fn durable_before(expected: &CapturedTargets) -> DurableTargets {
    DurableTargets {
        app_settings: AppFileObservation {
            exists: expected.app_settings_bytes.is_some(),
            revision: expected.app_settings_revision.clone(),
            mode: expected.app_settings_mode,
        },
        launch_at_login: expected.launch_at_login,
        monitor: expected.monitor.clone(),
        schedule: expected.schedule.clone(),
    }
}

fn operation_directory(root: &Path, operation_id: &str) -> Result<PathBuf, String> {
    if Uuid::parse_str(operation_id).is_err() {
        return Err("迁移操作标识无效。".to_string());
    }
    Ok(root.join(OPERATIONS_DIRECTORY).join(operation_id))
}

#[cfg(unix)]
fn ensure_private_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
            .create(path)
            .map_err(|_| "无法创建迁移恢复目录。".to_string())?;
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| "无法验证迁移恢复目录。".to_string())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err("迁移恢复目录权限不安全。".to_string());
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|_| "无法创建迁移恢复目录。".to_string())?;
    let metadata = fs::symlink_metadata(path).map_err(|_| "无法验证迁移恢复目录。".to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("迁移恢复目录不安全。".to_string());
    }
    Ok(())
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "迁移恢复文件没有父目录。".to_string())?;
    ensure_private_directory(parent)?;
    let temporary = parent.join(format!(".tmp-{}", Uuid::new_v4().hyphenated()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| "无法创建迁移恢复临时文件。".to_string())?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|_| "无法写入迁移恢复文件。".to_string())?;
        file.sync_all()
            .map_err(|_| "无法持久化迁移恢复文件。".to_string())?;
        crate::storage::replace_file(&temporary, path)
            .map_err(|_| "无法提交迁移恢复文件。".to_string())?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "无法持久化迁移恢复目录。".to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_json_private(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|_| "无法序列化迁移恢复状态。".to_string())?;
    write_private_atomic(path, &bytes)
}

fn read_json_private<T: for<'de> Deserialize<'de>>(path: &Path, maximum: u64) -> Result<T, String> {
    let contents = read_optional_regular_file(path, maximum)?
        .ok_or_else(|| "迁移恢复状态不存在。".to_string())?;
    serde_json::from_slice(&contents.bytes).map_err(|_| "迁移恢复状态已损坏。".to_string())
}

fn write_journal(operation_root: &Path, journal: &MigrationJournal) -> Result<(), String> {
    write_json_private(&operation_root.join(JOURNAL_FILE), journal)
}

fn update_journal_state(
    operation_root: &Path,
    journal: &mut MigrationJournal,
    state: JournalState,
) -> Result<(), String> {
    journal.state = state;
    journal.updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    write_journal(operation_root, journal)
}

fn create_journal(
    migration_root: &Path,
    pending: &PendingImport,
    updated_at: &UtcTimestamp,
) -> Result<(PathBuf, MigrationJournal), String> {
    ensure_private_directory(migration_root)?;
    ensure_private_directory(&migration_root.join(OPERATIONS_DIRECTORY))?;
    let operation_id = Uuid::new_v4().hyphenated().to_string();
    let operation_root = operation_directory(migration_root, &operation_id)?;
    ensure_private_directory(&operation_root)?;
    let backup = pending
        .expected
        .app_settings_bytes
        .as_deref()
        .unwrap_or_default();
    write_private_atomic(&operation_root.join(SETTINGS_BACKUP_FILE), backup)?;
    let created_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let journal = MigrationJournal {
        schema_version: "1.0".to_string(),
        operation_id: operation_id.clone(),
        state: JournalState::Prepared,
        source_revision: pending.source_revision.clone(),
        created_at: created_at.clone(),
        updated_at: created_at,
        changed_targets: pending.changed_targets.clone(),
        before: durable_before(&pending.expected),
        after: predicted_after(&pending.expected, &pending.desired, updated_at),
    };
    write_journal(&operation_root, &journal)?;
    write_json_private(
        &migration_root.join(LATEST_FILE),
        &LatestOperation {
            schema_version: "1.0".to_string(),
            operation_id,
        },
    )?;
    Ok((operation_root, journal))
}

fn read_latest_journal(
    migration_root: &Path,
) -> Result<Option<(PathBuf, MigrationJournal)>, String> {
    let latest_path = migration_root.join(LATEST_FILE);
    if !latest_path.exists() {
        return Ok(None);
    }
    let latest: LatestOperation = read_json_private(&latest_path, MAX_JOURNAL_BYTES)?;
    if latest.schema_version != "1.0" {
        return Err("迁移恢复指针版本不受支持。".to_string());
    }
    let operation_root = operation_directory(migration_root, &latest.operation_id)?;
    let journal: MigrationJournal =
        read_json_private(&operation_root.join(JOURNAL_FILE), MAX_JOURNAL_BYTES)?;
    if journal.schema_version != "1.0" || journal.operation_id != latest.operation_id {
        return Err("迁移恢复状态身份不一致。".to_string());
    }
    Ok(Some((operation_root, journal)))
}

fn write_target_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "迁移目标没有父目录。".to_string())?;
    let parent_metadata =
        fs::symlink_metadata(parent).map_err(|_| "无法验证迁移目标目录。".to_string())?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err("迁移目标目录不安全。".to_string());
    }
    let temporary = parent.join(format!(".legacy-migration-{}", Uuid::new_v4().hyphenated()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| "无法创建迁移目标临时文件。".to_string())?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|_| "无法写入迁移目标。".to_string())?;
        file.sync_all()
            .map_err(|_| "无法持久化迁移目标。".to_string())?;
        crate::storage::replace_file(&temporary, path)
            .map_err(|_| "无法提交迁移目标。".to_string())?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "无法持久化迁移目标目录。".to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn target_file_mode() -> Option<u32> {
    Some(0o600)
}

#[cfg(not(unix))]
fn target_file_mode() -> Option<u32> {
    None
}

#[cfg(unix)]
fn restore_file_mode(path: &Path, mode: Option<u32>) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|_| "无法恢复迁移前的设置文件权限。".to_string())?;
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|_| "无法持久化迁移前的设置文件权限。".to_string())?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restore_file_mode(_: &Path, _: Option<u32>) -> Result<(), String> {
    Ok(())
}

fn app_file_observation(path: &Path) -> Result<AppFileObservation, String> {
    let contents = read_optional_regular_file(path, MAX_APP_SETTINGS_BYTES)?;
    Ok(AppFileObservation {
        exists: contents.is_some(),
        revision: sha256_revision(
            contents.is_some(),
            contents
                .as_ref()
                .map(|contents| contents.bytes.as_slice())
                .unwrap_or_default(),
        ),
        mode: contents.as_ref().and_then(|contents| contents.mode),
    })
}

fn capture_durable<P: StartupPreference>(
    app_settings_path: &Path,
    store: &CapacityStore,
    startup: &mut P,
) -> Result<DurableTargets, String> {
    Ok(DurableTargets {
        app_settings: app_file_observation(app_settings_path)?,
        launch_at_login: startup.is_enabled()?,
        monitor: store
            .settings()
            .map(MonitorSnapshot::from)
            .map_err(|_| "无法读取迁移后的监控设置。".to_string())?,
        schedule: store
            .work_schedule()
            .map(ScheduleSnapshot::from)
            .map_err(|_| "无法读取迁移后的 Work Plan。".to_string())?,
    })
}

fn apply_monitor(
    store: &mut CapacityStore,
    expected: &MonitorSnapshot,
    desired: &MonitorSnapshot,
    updated_at: &UtcTimestamp,
) -> Result<MonitorSnapshot, String> {
    if expected.same_values(desired) {
        return Ok(expected.clone());
    }
    match store
        .update_settings(&desired.update(expected.revision)?, updated_at)
        .map_err(|_| "无法写入迁移后的监控设置。".to_string())?
    {
        SettingsUpdateOutcome::Updated(settings) => Ok(settings.into()),
        SettingsUpdateOutcome::RevisionConflict(_) => {
            Err("监控设置在确认后发生变化，已停止导入。".to_string())
        }
    }
}

fn apply_schedule(
    store: &mut CapacityStore,
    expected: &ScheduleSnapshot,
    desired: &ScheduleSnapshot,
    updated_at: &UtcTimestamp,
) -> Result<ScheduleSnapshot, String> {
    if expected.same_values(desired) {
        return Ok(expected.clone());
    }
    match store
        .update_work_schedule(&desired.update(expected.revision)?, updated_at)
        .map_err(|_| "无法写入迁移后的 Work Plan。".to_string())?
    {
        WorkScheduleSettingsUpdateOutcome::Updated(schedule) => Ok(schedule.into()),
        WorkScheduleSettingsUpdateOutcome::RevisionConflict(_) => {
            Err("Work Plan 在确认后发生变化，已停止导入。".to_string())
        }
    }
}

fn restore_app_file(
    app_settings_path: &Path,
    operation_root: &Path,
    before: &AppFileObservation,
) -> Result<(), String> {
    let backup = read_optional_regular_file(
        &operation_root.join(SETTINGS_BACKUP_FILE),
        MAX_APP_SETTINGS_BYTES,
    )?
    .ok_or_else(|| "迁移设置备份不存在。".to_string())?;
    if sha256_revision(before.exists, &backup.bytes) != before.revision {
        return Err("迁移设置备份完整性检查失败。".to_string());
    }
    if before.exists {
        write_target_atomic(app_settings_path, &backup.bytes)?;
        restore_file_mode(app_settings_path, before.mode)
    } else {
        match fs::symlink_metadata(app_settings_path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                Err("迁移目标设置文件不安全。".to_string())
            }
            Ok(_) => fs::remove_file(app_settings_path)
                .map_err(|_| "无法恢复迁移前的设置文件缺失状态。".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("无法检查迁移目标设置文件。".to_string()),
        }
    }
}

fn rollback_journal<P: StartupPreference>(
    app_settings_path: &Path,
    capacity_database_path: &Path,
    operation_root: &Path,
    journal: &mut MigrationJournal,
    startup: &mut P,
) -> Result<(), String> {
    let mut store = CapacityStore::open(capacity_database_path)
        .map_err(|_| "无法打开用于回滚的容量数据库。".to_string())?;
    let current = capture_durable(app_settings_path, &store, startup)?;
    let app_safe = current.app_settings == journal.before.app_settings
        || current.app_settings == journal.after.app_settings;
    let monitor_safe =
        current.monitor == journal.before.monitor || current.monitor == journal.after.monitor;
    let schedule_safe =
        current.schedule == journal.before.schedule || current.schedule == journal.after.schedule;
    if !app_safe || !monitor_safe || !schedule_safe {
        let _ = update_journal_state(operation_root, journal, JournalState::NeedsReview);
        return Err("迁移目标在导入后又发生变化，自动回滚已安全停止。".to_string());
    }

    let rollback = (|| {
        if current.app_settings == journal.after.app_settings
            && journal.before.app_settings != journal.after.app_settings
        {
            restore_app_file(
                app_settings_path,
                operation_root,
                &journal.before.app_settings,
            )?;
        }
        if current.launch_at_login != journal.before.launch_at_login {
            startup.apply(journal.before.launch_at_login)?;
        }
        if current.schedule == journal.after.schedule
            && !journal.before.schedule.same_values(&journal.after.schedule)
        {
            apply_schedule(
                &mut store,
                &current.schedule,
                &journal.before.schedule,
                &now_timestamp()?,
            )?;
        }
        if current.monitor == journal.after.monitor
            && !journal.before.monitor.same_values(&journal.after.monitor)
        {
            apply_monitor(
                &mut store,
                &current.monitor,
                &journal.before.monitor,
                &now_timestamp()?,
            )?;
        }
        let restored = capture_durable(app_settings_path, &store, startup)?;
        if restored.app_settings != journal.before.app_settings
            || restored.launch_at_login != journal.before.launch_at_login
            || !restored.monitor.same_values(&journal.before.monitor)
            || !restored.schedule.same_values(&journal.before.schedule)
        {
            return Err("迁移回滚后的状态校验失败。".to_string());
        }
        Ok(())
    })();
    match rollback {
        Ok(()) => update_journal_state(operation_root, journal, JournalState::RolledBack),
        Err(error) => {
            let _ = update_journal_state(operation_root, journal, JournalState::NeedsReview);
            Err(error)
        }
    }
}

fn execute_import<P: StartupPreference, F: FnMut() -> Result<(), String>>(
    pending: &PendingImport,
    app_settings_path: &Path,
    capacity_database_path: &Path,
    migration_root: &Path,
    startup: &mut P,
    mut revalidate: F,
) -> Result<LegacyMigrationApplyResult, String> {
    revalidate()?;
    let current = capture_targets(app_settings_path, capacity_database_path, startup)?;
    if !target_precondition_matches(&current, &pending.expected) {
        return Err("QuotaHorizon 设置在预览后发生变化，未执行导入。请重新预览。".to_string());
    }
    let updated_at = now_timestamp()?;
    let (operation_root, mut journal) = create_journal(migration_root, pending, &updated_at)?;
    update_journal_state(&operation_root, &mut journal, JournalState::Applying)?;
    let mut store = CapacityStore::open(capacity_database_path)
        .map_err(|_| "无法打开 QuotaHorizon 容量数据库。".to_string())?;

    let apply = (|| {
        revalidate()?;
        let monitor = apply_monitor(
            &mut store,
            &pending.expected.monitor,
            &pending.desired.monitor,
            &updated_at,
        )?;
        if monitor != journal.after.monitor {
            return Err("监控设置写入后的校验结果不一致。".to_string());
        }
        revalidate()?;
        let schedule = apply_schedule(
            &mut store,
            &pending.expected.schedule,
            &pending.desired.schedule,
            &updated_at,
        )?;
        if schedule != journal.after.schedule {
            return Err("Work Plan 写入后的校验结果不一致。".to_string());
        }
        revalidate()?;
        let current_app_settings = app_file_observation(app_settings_path)?;
        if current_app_settings.revision != pending.expected.app_settings_revision
            || current_app_settings.mode != pending.expected.app_settings_mode
        {
            return Err("桌面设置在导入时发生变化，已停止导入。".to_string());
        }
        if pending.expected.app_settings_value != pending.desired.app_settings_value {
            write_target_atomic(app_settings_path, &pending.desired.app_settings_bytes)?;
        }
        revalidate()?;
        if pending.expected.launch_at_login != pending.desired.launch_at_login {
            startup.apply(pending.desired.launch_at_login)?;
        }
        revalidate()?;
        let postflight = capture_durable(app_settings_path, &store, startup)?;
        if postflight != journal.after {
            return Err("迁移写入后的完整状态校验失败。".to_string());
        }
        Ok(())
    })();

    if let Err(error) = apply {
        return match rollback_journal(
            app_settings_path,
            capacity_database_path,
            &operation_root,
            &mut journal,
            startup,
        ) {
            Ok(()) => Err(format!("{error} 已自动恢复迁移前状态。")),
            Err(rollback_error) => Err(format!("{error} {rollback_error}")),
        };
    }
    if let Err(error) = update_journal_state(&operation_root, &mut journal, JournalState::Committed)
    {
        return match rollback_journal(
            app_settings_path,
            capacity_database_path,
            &operation_root,
            &mut journal,
            startup,
        ) {
            Ok(()) => Err(format!("{error} 已自动恢复迁移前状态。")),
            Err(rollback_error) => Err(format!("{error} {rollback_error}")),
        };
    }
    Ok(LegacyMigrationApplyResult {
        schema_version: "1.0",
        status: "applied",
        operation_id: Some(journal.operation_id),
        changed_targets: journal.changed_targets,
        source_unchanged: true,
        rollback_available: true,
        language: pending
            .desired
            .app_settings_value
            .get("language")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn prune_pending_imports(pending: &mut HashMap<String, PendingImport>, now: DateTime<Utc>) {
    pending.retain(|_, plan| plan.expires_at > now);
    if pending.len() > 8 {
        let mut tokens = pending
            .iter()
            .map(|(token, plan)| (token.clone(), plan.expires_at))
            .collect::<Vec<_>>();
        tokens.sort_by_key(|(_, expires_at)| *expires_at);
        for (token, _) in tokens.into_iter().take(pending.len() - 8) {
            pending.remove(&token);
        }
    }
}

pub(crate) fn prepare_import_blocking<R: Runtime>(
    app: &AppHandle<R>,
    source_path: PathBuf,
) -> Result<LegacyMigrationApplyPreview, String> {
    let preferences = read_importable_preferences(&source_path)
        .map_err(|_| "无法读取可安全导入的 QuotaViewer 偏好。".to_string())?;
    if preferences.settings.is_none() && preferences.session_language.is_none() {
        return Err("所选目录没有可直接迁移的 Viewer 设置。".to_string());
    }
    let revision = source_revision(&preferences)?;
    let (app_settings_path, capacity_database_path, _) = app_data_paths(app)?;
    let mut startup = TauriStartupPreference { app };
    let expected = capture_targets(&app_settings_path, &capacity_database_path, &mut startup)?;
    let (desired, summary, changed_targets) = desired_targets(&expected, &preferences)?;
    if changed_targets.is_empty() {
        return Ok(LegacyMigrationApplyPreview {
            schema_version: "1.0",
            status: "already_applied",
            confirm_token: None,
            expires_at: None,
            typed_confirmation: CONFIRMATION_PHRASE,
            changed_targets,
            review_targets_excluded: vec![
                "account_vault",
                "quota_cache",
                "provider_mode_state",
                "restore_point_ledger",
            ],
            imported: summary,
            creates_backup: false,
            automatic_rollback: true,
        });
    }
    let now = Utc::now();
    let expires_at = now + Duration::seconds(CONFIRMATION_TTL_SECONDS);
    let token = Uuid::new_v4().hyphenated().to_string();
    let plan = PendingImport {
        source_path,
        source_revision: revision,
        expected,
        desired,
        changed_targets: changed_targets.clone(),
        expires_at,
    };
    let mut pending = PENDING_IMPORTS
        .lock()
        .map_err(|_| "迁移确认状态不可用。".to_string())?;
    prune_pending_imports(&mut pending, now);
    pending.insert(token.clone(), plan);
    Ok(LegacyMigrationApplyPreview {
        schema_version: "1.0",
        status: "confirmation_required",
        confirm_token: Some(token),
        expires_at: Some(expires_at.to_rfc3339_opts(SecondsFormat::Millis, true)),
        typed_confirmation: CONFIRMATION_PHRASE,
        changed_targets,
        review_targets_excluded: vec![
            "account_vault",
            "quota_cache",
            "provider_mode_state",
            "restore_point_ledger",
        ],
        imported: summary,
        creates_backup: true,
        automatic_rollback: true,
    })
}

#[tauri::command]
pub(crate) async fn prepare_legacy_quotaviewer_import<R: Runtime + 'static>(
    app: AppHandle<R>,
    source_path: Option<String>,
) -> Result<LegacyMigrationApplyPreview, String> {
    let source_path = super::legacy_migration::legacy_source_path(source_path)?;
    tauri::async_runtime::spawn_blocking(move || prepare_import_blocking(&app, source_path))
        .await
        .map_err(|_| "QuotaViewer 迁移预览任务意外结束。".to_string())?
}

fn consume_pending_import(token: &str) -> Result<PendingImport, String> {
    if Uuid::parse_str(token).is_err() {
        return Err("迁移确认令牌无效。请重新预览。".to_string());
    }
    let now = Utc::now();
    let mut pending = PENDING_IMPORTS
        .lock()
        .map_err(|_| "迁移确认状态不可用。".to_string())?;
    prune_pending_imports(&mut pending, now);
    let plan = pending
        .remove(token)
        .ok_or_else(|| "迁移确认已过期或已使用。请重新预览。".to_string())?;
    if plan.expires_at <= now {
        return Err("迁移确认已过期。请重新预览。".to_string());
    }
    Ok(plan)
}

#[tauri::command]
pub(crate) async fn confirm_legacy_quotaviewer_import<R: Runtime + 'static>(
    app: AppHandle<R>,
    confirm_token: String,
    typed_confirmation: String,
) -> Result<LegacyMigrationApplyResult, String> {
    if typed_confirmation != CONFIRMATION_PHRASE {
        return Err("请输入 IMPORT 以确认迁移。".to_string());
    }
    let pending = consume_pending_import(&confirm_token)?;
    tauri::async_runtime::spawn_blocking(move || {
        let preferences = read_importable_preferences(&pending.source_path)
            .map_err(|_| "QuotaViewer 源数据在确认前已不可用。".to_string())?;
        if source_revision(&preferences)? != pending.source_revision {
            return Err("QuotaViewer 源设置在预览后发生变化。请重新预览。".to_string());
        }
        let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
        let (app_settings_path, capacity_database_path, migration_root) = app_data_paths(&app)?;
        let source_path = pending.source_path.clone();
        let expected_source_revision = pending.source_revision.clone();
        let mut startup = TauriStartupPreference { app: &app };
        let result = execute_import(
            &pending,
            &app_settings_path,
            &capacity_database_path,
            &migration_root,
            &mut startup,
            || {
                lease
                    .revalidate(&mut legacy_probe)
                    .map_err(|_| "迁移安全互斥已失效。".to_string())?;
                let current = read_importable_preferences(&source_path)
                    .map_err(|_| "QuotaViewer 源数据在导入时已不可用。".to_string())?;
                if source_revision(&current)? != expected_source_revision {
                    return Err("QuotaViewer 源设置在导入时发生变化。".to_string());
                }
                Ok(())
            },
        );
        capacity_desktop_service::notify_persistence_settings_changed(&app);
        crate::system_tray::refresh_menu(&app);
        result
    })
    .await
    .map_err(|_| "QuotaViewer 迁移执行任务意外结束。".to_string())?
}

pub(crate) fn latest_operation_blocking<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<LegacyMigrationOperationView, String> {
    let (_, _, migration_root) = app_data_paths(app)?;
    let Some((_, journal)) = read_latest_journal(&migration_root)? else {
        return Ok(LegacyMigrationOperationView {
            schema_version: "1.0",
            status: "none",
            operation_id: None,
            changed_targets: Vec::new(),
            rollback_available: false,
            recovery_required: false,
            typed_rollback: ROLLBACK_PHRASE,
        });
    };
    let status = match journal.state {
        JournalState::Prepared => "prepared",
        JournalState::Applying => "applying",
        JournalState::Committed => "committed",
        JournalState::RolledBack => "rolled_back",
        JournalState::NeedsReview => "needs_review",
    };
    Ok(LegacyMigrationOperationView {
        schema_version: "1.0",
        status,
        operation_id: Some(journal.operation_id),
        changed_targets: journal.changed_targets,
        rollback_available: journal.state == JournalState::Committed,
        recovery_required: matches!(
            journal.state,
            JournalState::Prepared | JournalState::Applying | JournalState::NeedsReview
        ),
        typed_rollback: ROLLBACK_PHRASE,
    })
}

#[tauri::command]
pub(crate) async fn get_latest_legacy_migration_operation<R: Runtime + 'static>(
    app: AppHandle<R>,
) -> Result<LegacyMigrationOperationView, String> {
    tauri::async_runtime::spawn_blocking(move || latest_operation_blocking(&app))
        .await
        .map_err(|_| "读取迁移恢复状态的任务意外结束。".to_string())?
}

#[tauri::command]
pub(crate) async fn rollback_legacy_quotaviewer_import<R: Runtime + 'static>(
    app: AppHandle<R>,
    operation_id: String,
    typed_confirmation: String,
) -> Result<LegacyMigrationApplyResult, String> {
    if typed_confirmation != ROLLBACK_PHRASE {
        return Err("请输入 ROLLBACK 以确认撤销。".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
        lease
            .revalidate(&mut legacy_probe)
            .map_err(|_| "迁移回滚安全互斥已失效。".to_string())?;
        let (app_settings_path, capacity_database_path, migration_root) = app_data_paths(&app)?;
        let Some((operation_root, mut journal)) = read_latest_journal(&migration_root)? else {
            return Err("没有可撤销的 Viewer 迁移。".to_string());
        };
        if journal.operation_id != operation_id || journal.state != JournalState::Committed {
            return Err("该 Viewer 迁移已不可撤销。".to_string());
        }
        let changed_targets = journal.changed_targets.clone();
        let operation_id = journal.operation_id.clone();
        let mut startup = TauriStartupPreference { app: &app };
        let rollback = rollback_journal(
            &app_settings_path,
            &capacity_database_path,
            &operation_root,
            &mut journal,
            &mut startup,
        );
        capacity_desktop_service::notify_persistence_settings_changed(&app);
        crate::system_tray::refresh_menu(&app);
        rollback?;
        Ok(LegacyMigrationApplyResult {
            schema_version: "1.0",
            status: "rolled_back",
            operation_id: Some(operation_id),
            changed_targets,
            source_unchanged: true,
            rollback_available: false,
            language: read_app_settings_target(&app_settings_path)?
                .value
                .get("language")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    })
    .await
    .map_err(|_| "QuotaViewer 迁移回滚任务意外结束。".to_string())?
}

/// Roll back a crash-interrupted import before the capacity service opens its
/// long-lived SQLite connection. Committed imports are left intact and remain
/// explicitly reversible from Settings.
pub(crate) fn recover_interrupted_import<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    let (app_settings_path, capacity_database_path, migration_root) = app_data_paths(app)?;
    let Some((operation_root, mut journal)) = read_latest_journal(&migration_root)? else {
        return Ok(false);
    };
    if !matches!(
        journal.state,
        JournalState::Prepared | JournalState::Applying
    ) {
        return Ok(false);
    }
    let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "启动时无法取得迁移恢复互斥。".to_string())?;
    let mut startup = TauriStartupPreference { app };
    rollback_journal(
        &app_settings_path,
        &capacity_database_path,
        &operation_root,
        &mut journal,
        &mut startup,
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeStartup {
        enabled: bool,
    }

    impl StartupPreference for FakeStartup {
        fn is_enabled(&mut self) -> Result<bool, String> {
            Ok(self.enabled)
        }

        fn apply(&mut self, enabled: bool) -> Result<(), String> {
            self.enabled = enabled;
            Ok(())
        }
    }

    fn source_fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../fixtures/migration/source/ready")
            .canonicalize()
            .unwrap()
    }

    fn initialized_database(path: &Path) {
        ensure_private_directory(path.parent().unwrap()).unwrap();
        let mut store = CapacityStore::open(path).unwrap();
        let current = MonitorSnapshot::from(store.settings().unwrap());
        let mut changed = current.clone();
        changed.auto_refresh_enabled = false;
        changed.refresh_interval_seconds = 900;
        changed.language = "en".to_string();
        changed.launch_at_login = true;
        let outcome = store
            .update_settings(
                &changed.update(current.revision).unwrap(),
                &UtcTimestamp::parse("2026-09-04T00:00:00Z").unwrap(),
            )
            .unwrap();
        assert!(matches!(outcome, SettingsUpdateOutcome::Updated(_)));
    }

    fn test_plan(
        root: &Path,
        startup: &mut FakeStartup,
    ) -> (PendingImport, PathBuf, PathBuf, PathBuf, Vec<u8>) {
        let app_settings_path = root.join("settings.json");
        let capacity_database_path = root.join(CAPACITY_DIRECTORY).join(CAPACITY_DATABASE);
        let migration_root = root.join(MIGRATION_DIRECTORY);
        initialized_database(&capacity_database_path);
        let original = br#"{
  "language": "zh",
  "launchAtStartup": true,
  "unknownUserSetting": { "keep": true }
}"#
        .to_vec();
        fs::write(&app_settings_path, &original).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&app_settings_path, fs::Permissions::from_mode(0o640)).unwrap();
        }
        let preferences = read_importable_preferences(&source_fixture()).unwrap();
        let expected =
            capture_targets(&app_settings_path, &capacity_database_path, startup).unwrap();
        let (desired, _, changed_targets) = desired_targets(&expected, &preferences).unwrap();
        let pending = PendingImport {
            source_path: source_fixture(),
            source_revision: source_revision(&preferences).unwrap(),
            expected,
            desired,
            changed_targets,
            expires_at: Utc::now() + Duration::seconds(120),
        };
        (
            pending,
            app_settings_path,
            capacity_database_path,
            migration_root,
            original,
        )
    }

    #[test]
    fn import_is_verified_idempotent_and_manually_reversible() {
        let directory = tempfile::tempdir().unwrap();
        let mut startup = FakeStartup { enabled: true };
        let (pending, app_settings_path, database_path, migration_root, original) =
            test_plan(directory.path(), &mut startup);
        let result = execute_import(
            &pending,
            &app_settings_path,
            &database_path,
            &migration_root,
            &mut startup,
            || Ok(()),
        )
        .unwrap();
        assert_eq!(result.status, "applied");
        assert!(result.rollback_available);
        assert!(!startup.enabled);

        let imported: Value =
            serde_json::from_slice(&fs::read(&app_settings_path).unwrap()).unwrap();
        assert_eq!(imported["language"], "en");
        assert_eq!(imported["launchAtStartup"], false);
        assert_eq!(imported["unknownUserSetting"]["keep"], true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&app_settings_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o600
            );
        }
        let preferences = read_importable_preferences(&source_fixture()).unwrap();
        let current = capture_targets(&app_settings_path, &database_path, &mut startup).unwrap();
        let (_, _, repeated_changes) = desired_targets(&current, &preferences).unwrap();
        assert!(repeated_changes.is_empty());

        let (operation_root, mut journal) = read_latest_journal(&migration_root).unwrap().unwrap();
        assert_eq!(journal.state, JournalState::Committed);
        let journal_text = fs::read_to_string(operation_root.join(JOURNAL_FILE)).unwrap();
        assert!(!journal_text.contains(source_fixture().to_string_lossy().as_ref()));
        for canary in [
            "fixture-access-token",
            "fixture-api-key",
            "person@example.com",
            "account-alpha",
        ] {
            assert!(!journal_text.contains(canary));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [
                operation_root.join(JOURNAL_FILE),
                operation_root.join(SETTINGS_BACKUP_FILE),
                migration_root.join(LATEST_FILE),
            ] {
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o7777,
                    0o600
                );
            }
            assert_eq!(
                fs::metadata(&operation_root).unwrap().permissions().mode() & 0o7777,
                0o700
            );
        }
        rollback_journal(
            &app_settings_path,
            &database_path,
            &operation_root,
            &mut journal,
            &mut startup,
        )
        .unwrap();
        assert_eq!(fs::read(&app_settings_path).unwrap(), original);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&app_settings_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o640
            );
        }
        assert!(startup.enabled);
        let restored = capture_targets(&app_settings_path, &database_path, &mut startup).unwrap();
        assert!(restored.monitor.same_values(&pending.expected.monitor));
        assert!(restored.schedule.same_values(&pending.expected.schedule));
        assert_eq!(
            read_latest_journal(&migration_root)
                .unwrap()
                .unwrap()
                .1
                .state,
            JournalState::RolledBack
        );
    }

    #[test]
    fn failure_after_sqlite_updates_rolls_every_target_back() {
        let directory = tempfile::tempdir().unwrap();
        let mut startup = FakeStartup { enabled: true };
        let (pending, app_settings_path, database_path, migration_root, original) =
            test_plan(directory.path(), &mut startup);
        let mut checks = 0;
        let error = execute_import(
            &pending,
            &app_settings_path,
            &database_path,
            &migration_root,
            &mut startup,
            || {
                checks += 1;
                if checks == 4 {
                    Err("injected lease loss".to_string())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert!(error.contains("已自动恢复迁移前状态"));
        assert_eq!(fs::read(&app_settings_path).unwrap(), original);
        assert!(startup.enabled);
        let restored = capture_targets(&app_settings_path, &database_path, &mut startup).unwrap();
        assert!(restored.monitor.same_values(&pending.expected.monitor));
        assert!(restored.schedule.same_values(&pending.expected.schedule));
        assert_eq!(
            read_latest_journal(&migration_root)
                .unwrap()
                .unwrap()
                .1
                .state,
            JournalState::RolledBack
        );
    }

    #[test]
    fn work_plan_import_normalizes_empty_duplicates_and_rejects_excess() {
        let directory = tempfile::tempdir().unwrap();
        let mut startup = FakeStartup { enabled: true };
        let (pending, _, _, _, _) = test_plan(directory.path(), &mut startup);
        let mut preferences = read_importable_preferences(&source_fixture()).unwrap();
        preferences.settings.as_mut().unwrap().off_periods = vec![
            capacity_migration::LegacyWorkPeriod {
                start_minute_of_day: 60,
                end_minute_of_day: 60,
            },
            capacity_migration::LegacyWorkPeriod {
                start_minute_of_day: 300,
                end_minute_of_day: 420,
            },
            capacity_migration::LegacyWorkPeriod {
                start_minute_of_day: 300,
                end_minute_of_day: 420,
            },
        ];
        let (desired, _, _) = desired_targets(&pending.expected, &preferences).unwrap();
        assert_eq!(
            desired.schedule.off_periods,
            vec![SchedulePeriodSnapshot {
                start_minute_of_day: 300,
                end_minute_of_day: 420,
            }]
        );

        preferences.settings.as_mut().unwrap().off_periods = (0..17)
            .map(|index| capacity_migration::LegacyWorkPeriod {
                start_minute_of_day: index * 2,
                end_minute_of_day: index * 2 + 1,
            })
            .collect();
        assert!(desired_targets(&pending.expected, &preferences)
            .unwrap_err()
            .contains("安全导入上限"));
    }
}
