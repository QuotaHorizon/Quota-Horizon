use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::{
    models::{
        AppSettings, CpaBridgeSettings, MAX_CPA_REFRESH_SECONDS, MAX_CPA_TIMEOUT_SECONDS,
        MIN_CPA_REFRESH_SECONDS, MIN_CPA_TIMEOUT_SECONDS,
    },
    storage::{read_app_settings, write_app_settings},
};

const CACHE_VERSION: u8 = 1;
const CACHE_FILE: &str = "cpa/pool-status-v1.json";
const MAX_BRIDGE_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const CPA_POOL_UPDATED_EVENT: &str = "cpa-pool-updated";
static CPA_REFRESH_LOCK: Mutex<()> = Mutex::new(());
static CPA_BACKGROUND_SENDER: OnceLock<Mutex<Option<mpsc::Sender<BackgroundCommand>>>> =
    OnceLock::new();

#[derive(Debug, Clone, Copy)]
enum BackgroundCommand {
    Wake,
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CpaQuotaWindow {
    used_percent: f64,
    remaining_percent: f64,
    window_minutes: Option<u64>,
    resets_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CpaPoolMember {
    id: String,
    display_name: String,
    is_current_route: bool,
    is_route_preferred: bool,
    is_latest_request_route: bool,
    primary: Option<CpaQuotaWindow>,
    secondary: Option<CpaQuotaWindow>,
    plan_type: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    status_code: Option<u16>,
    failed: Option<bool>,
    is_stale: bool,
    refresh_skipped: bool,
    skip_reason: Option<String>,
    quota_guard_state: Option<String>,
    quota_guard_reason: Option<String>,
    stats_sample_count: Option<u64>,
    estimated_remaining_successes: Option<u64>,
    observed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CpaPoolSource {
    Unavailable,
    Cached,
    Live,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CpaPoolIssueCode {
    BridgeDisabled,
    InvalidConfiguration,
    UnsupportedPlatform,
    BridgeUnavailable,
    BridgeTimeout,
    BridgeRejected,
    OutputTooLarge,
    InvalidResponse,
    NoRecords,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CpaPoolIssue {
    code: CpaPoolIssueCode,
    retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CpaPoolStatus {
    source: CpaPoolSource,
    stale: bool,
    fetched_at: Option<String>,
    checked_at: String,
    parent_member_id: Option<String>,
    members: Vec<CpaPoolMember>,
    issue: Option<CpaPoolIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CpaPoolCache {
    version: u8,
    fetched_at: String,
    parent_member_id: Option<String>,
    members: Vec<CpaPoolMember>,
}

#[derive(Debug, Deserialize)]
struct ScriptResponse {
    #[allow(dead_code)]
    #[serde(default)]
    records_saved: Option<u64>,
    #[serde(default)]
    current: Option<UsageRecord>,
    #[serde(default)]
    latest: Vec<UsageRecord>,
    #[serde(default)]
    accounts: Option<Vec<PoolAccountRecord>>,
}

#[derive(Debug, Deserialize)]
struct PoolAccountRecord {
    id: String,
    display_name: String,
    #[serde(default)]
    auth_file: Option<String>,
    #[serde(default)]
    auth_index: Option<String>,
    #[serde(default)]
    is_current_route: Option<bool>,
    #[serde(default)]
    is_route_preferred: Option<bool>,
    #[serde(default)]
    is_latest_request_route: Option<bool>,
    #[serde(default)]
    latest: Option<UsageRecord>,
    #[serde(default)]
    quota_guard: Option<QuotaGuardRecord>,
    #[serde(default)]
    refresh_skipped: Option<bool>,
    #[serde(default)]
    skip_reason: Option<String>,
    #[serde(default)]
    account_stats: Option<AccountStatsRecord>,
}

#[derive(Debug, Clone, Deserialize)]
struct UsageRecord {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    alias: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    failed: Option<bool>,
    #[serde(default)]
    status_code: Option<u16>,
    #[serde(default)]
    stale: bool,
    #[serde(default)]
    refresh_skipped: bool,
    #[serde(default)]
    skip_reason: Option<String>,
    #[serde(default)]
    quota_guard: Option<QuotaGuardRecord>,
    #[serde(default)]
    codex_headers: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuotaGuardRecord {
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AccountStatsRecord {
    #[serde(default)]
    sample_count: Option<u64>,
    #[serde(default)]
    primary: Option<AccountStatsCapacity>,
    #[serde(default)]
    secondary: Option<AccountStatsCapacity>,
}

#[derive(Debug, Deserialize)]
struct AccountStatsCapacity {
    #[serde(default)]
    estimated_remaining_successes: Option<u64>,
}

#[derive(Debug, Clone)]
struct BridgeConfiguration {
    host: String,
    remote_command: String,
    timeout: Duration,
}

impl BridgeConfiguration {
    fn from_settings(settings: &CpaBridgeSettings) -> Result<Self, CpaPoolIssue> {
        let remote_command = environment_value("QUOTA_HORIZON_CPA_QUOTA_COMMAND")
            .or_else(|| environment_value("CODEX_QUOTA_VIEWER_CPA_QUOTA_COMMAND"));
        Self::from_settings_and_command(settings, remote_command)
    }

    fn from_settings_and_command(
        settings: &CpaBridgeSettings,
        remote_command: Option<String>,
    ) -> Result<Self, CpaPoolIssue> {
        let settings = normalize_bridge_settings(settings.clone())
            .map_err(|_| issue(CpaPoolIssueCode::InvalidConfiguration, false))?;
        // A configured destination and command are required before spawning SSH.
        // Existing explicit overrides remain supported; there is no private fallback.
        let remote_command = remote_command
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| issue(CpaPoolIssueCode::InvalidConfiguration, false))?;
        if !valid_ssh_host(&settings.ssh_host)
            || remote_command.len() > 2_048
            || remote_command.contains(['\n', '\r', '\0'])
        {
            return Err(issue(CpaPoolIssueCode::InvalidConfiguration, false));
        }
        Ok(Self {
            host: settings.ssh_host,
            remote_command,
            timeout: Duration::from_secs(settings.timeout_seconds),
        })
    }
}

fn environment_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn valid_ssh_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || b"._@:-".contains(&value))
}

fn normalize_bridge_settings(mut settings: CpaBridgeSettings) -> Result<CpaBridgeSettings, String> {
    settings.ssh_host = settings.ssh_host.trim().to_string();
    if (settings.enabled || !settings.ssh_host.is_empty()) && !valid_ssh_host(&settings.ssh_host) {
        return Err("CPA SSH host is invalid".to_string());
    }
    if !(MIN_CPA_REFRESH_SECONDS..=MAX_CPA_REFRESH_SECONDS).contains(&settings.refresh_seconds) {
        return Err(format!(
            "CPA refresh interval must be between {MIN_CPA_REFRESH_SECONDS} and {MAX_CPA_REFRESH_SECONDS} seconds"
        ));
    }
    if !(MIN_CPA_TIMEOUT_SECONDS..=MAX_CPA_TIMEOUT_SECONDS).contains(&settings.timeout_seconds) {
        return Err(format!(
            "CPA timeout must be between {MIN_CPA_TIMEOUT_SECONDS} and {MAX_CPA_TIMEOUT_SECONDS} seconds"
        ));
    }
    Ok(settings)
}

fn issue(code: CpaPoolIssueCode, retryable: bool) -> CpaPoolIssue {
    CpaPoolIssue { code, retryable }
}

fn now_string() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn empty_status(issue: Option<CpaPoolIssue>) -> CpaPoolStatus {
    CpaPoolStatus {
        source: CpaPoolSource::Unavailable,
        stale: false,
        fetched_at: None,
        checked_at: now_string(),
        parent_member_id: None,
        members: Vec::new(),
        issue,
    }
}

fn cache_path<R: Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join(CACHE_FILE))
        .map_err(|error| format!("Unable to locate the CPA cache directory: {error}"))
}

fn read_cache(path: &Path) -> Option<CpaPoolCache> {
    let bytes = fs::read(path).ok()?;
    let cache: CpaPoolCache = serde_json::from_slice(&bytes).ok()?;
    (cache.version == CACHE_VERSION).then_some(cache)
}

fn write_cache(path: &Path, cache: &CpaPoolCache) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "CPA cache path has no parent directory".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Unable to create the CPA cache directory: {error}"))?;
    let bytes = serde_json::to_vec(cache)
        .map_err(|error| format!("Unable to serialize the CPA cache: {error}"))?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    {
        let mut file = fs::File::create(&temporary)
            .map_err(|error| format!("Unable to create the CPA cache: {error}"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("Unable to write the CPA cache: {error}"))?;
    }
    crate::storage::replace_file(&temporary, path)
        .map_err(|error| format!("Unable to commit the CPA cache: {error}"))
}

fn status_from_cache(cache: CpaPoolCache, issue: Option<CpaPoolIssue>) -> CpaPoolStatus {
    let mut members = cache.members;
    for member in &mut members {
        member.is_stale = true;
    }
    CpaPoolStatus {
        source: CpaPoolSource::Cached,
        stale: true,
        fetched_at: Some(cache.fetched_at),
        checked_at: now_string(),
        parent_member_id: cache.parent_member_id,
        members,
        issue,
    }
}

fn background_sender() -> &'static Mutex<Option<mpsc::Sender<BackgroundCommand>>> {
    CPA_BACKGROUND_SENDER.get_or_init(|| Mutex::new(None))
}

pub(crate) fn setup_background_refresh<R: Runtime + 'static>(
    app: &AppHandle<R>,
) -> Result<(), String> {
    let mut sender = background_sender()
        .lock()
        .map_err(|_| "CPA background refresh state is unavailable".to_string())?;
    if sender.is_some() {
        return Ok(());
    }
    let (next_sender, receiver) = mpsc::channel();
    let worker_app = app.clone();
    thread::Builder::new()
        .name("quota-horizon-cpa-refresh".to_string())
        .spawn(move || background_refresh_loop(worker_app, receiver))
        .map_err(|_| "Unable to start the CPA background refresh worker".to_string())?;
    *sender = Some(next_sender);
    Ok(())
}

pub(crate) fn shutdown_background_refresh() {
    if let Ok(mut sender) = background_sender().lock() {
        if let Some(sender) = sender.take() {
            let _ = sender.send(BackgroundCommand::Stop);
        }
    }
}

fn wake_background_refresh() {
    if let Ok(sender) = background_sender().lock() {
        if let Some(sender) = sender.as_ref() {
            let _ = sender.send(BackgroundCommand::Wake);
        }
    }
}

fn background_refresh_interval(settings: &CpaBridgeSettings) -> Duration {
    Duration::from_secs(
        settings
            .refresh_seconds
            .clamp(MIN_CPA_REFRESH_SECONDS, MAX_CPA_REFRESH_SECONDS),
    )
}

fn background_refresh_loop<R: Runtime>(
    app: AppHandle<R>,
    receiver: mpsc::Receiver<BackgroundCommand>,
) {
    loop {
        let settings = match read_app_settings(&app) {
            Ok(settings) => settings.cpa_bridge,
            Err(_) => match receiver.recv_timeout(Duration::from_secs(60)) {
                Ok(BackgroundCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(BackgroundCommand::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => continue,
            },
        };
        if !settings.enabled {
            match receiver.recv() {
                Ok(BackgroundCommand::Wake) => continue,
                Ok(BackgroundCommand::Stop) | Err(_) => break,
            }
        }

        if let Ok(status) = refresh_cpa_pool_status_blocking(&app) {
            let _ = app.emit(CPA_POOL_UPDATED_EVENT, status);
        }
        match receiver.recv_timeout(background_refresh_interval(&settings)) {
            Ok(BackgroundCommand::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Ok(BackgroundCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[tauri::command]
pub(crate) fn set_cpa_bridge_settings<R: Runtime>(
    app: AppHandle<R>,
    settings: CpaBridgeSettings,
) -> Result<AppSettings, String> {
    let settings = normalize_bridge_settings(settings)?;
    let mut app_settings = read_app_settings(&app)?;
    app_settings.cpa_bridge = settings;
    write_app_settings(&app, &app_settings)?;
    wake_background_refresh();
    Ok(app_settings)
}

#[tauri::command]
pub(crate) fn get_cpa_pool_status<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<CpaPoolStatus, String> {
    let path = cache_path(&app)?;
    Ok(read_cache(&path)
        .map(|cache| status_from_cache(cache, None))
        .unwrap_or_else(|| empty_status(None)))
}

#[tauri::command]
pub(crate) async fn refresh_cpa_pool_status<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
) -> Result<CpaPoolStatus, String> {
    let worker_app = app.clone();
    let status =
        tauri::async_runtime::spawn_blocking(move || refresh_cpa_pool_status_blocking(&worker_app))
            .await
            .map_err(|error| format!("CPA refresh task failed: {error}"))??;
    let _ = app.emit(CPA_POOL_UPDATED_EVENT, status.clone());
    Ok(status)
}

fn refresh_cpa_pool_status_blocking<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<CpaPoolStatus, String> {
    let _refresh_guard = CPA_REFRESH_LOCK
        .lock()
        .map_err(|_| "CPA refresh lock is unavailable".to_string())?;
    let settings = read_app_settings(app)?.cpa_bridge;
    let path = cache_path(app)?;
    let cached = read_cache(&path);
    if !settings.enabled {
        let disabled = issue(CpaPoolIssueCode::BridgeDisabled, false);
        return Ok(cached
            .map(|cache| status_from_cache(cache, Some(disabled.clone())))
            .unwrap_or_else(|| empty_status(Some(disabled))));
    }
    let configuration = match BridgeConfiguration::from_settings(&settings) {
        Ok(configuration) => configuration,
        Err(failure) => {
            return Ok(cached
                .map(|cache| status_from_cache(cache, Some(failure.clone())))
                .unwrap_or_else(|| empty_status(Some(failure))));
        }
    };
    let result = run_bridge(&configuration).and_then(|bytes| parse_bridge_response(&bytes));
    match result {
        Ok(mut status) => {
            if let Some(cache) = cached.as_ref() {
                merge_cached_quota_facts(&mut status, cache);
            }
            let fetched_at = status
                .members
                .iter()
                .filter_map(|member| member.observed_at.as_deref())
                .filter_map(|value| DateTime::parse_from_rfc3339(value).ok())
                .max()
                .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
                .unwrap_or_else(now_string);
            status.fetched_at = Some(fetched_at.clone());
            status.checked_at = now_string();
            let cache = CpaPoolCache {
                version: CACHE_VERSION,
                fetched_at,
                parent_member_id: status.parent_member_id.clone(),
                members: status.members.clone(),
            };
            write_cache(&path, &cache)?;
            Ok(status)
        }
        Err(failure) => Ok(cached
            .map(|cache| status_from_cache(cache, Some(failure.clone())))
            .unwrap_or_else(|| empty_status(Some(failure)))),
    }
}

fn merge_cached_quota_facts(status: &mut CpaPoolStatus, cache: &CpaPoolCache) {
    for member in &mut status.members {
        let Some(cached) = cache.members.iter().find(|cached| cached.id == member.id) else {
            continue;
        };
        let mut restored = false;
        if member.primary.is_none() && cached.primary.is_some() {
            member.primary.clone_from(&cached.primary);
            restored = true;
        }
        if member.secondary.is_none() && cached.secondary.is_some() {
            member.secondary.clone_from(&cached.secondary);
            restored = true;
        }
        if restored {
            member.is_stale = true;
            member.observed_at.clone_from(&cached.observed_at);
            if member.plan_type.is_none() {
                member.plan_type.clone_from(&cached.plan_type);
            }
        }
    }
    status.stale = status.members.iter().any(|member| member.is_stale);
}

fn ssh_executable() -> &'static str {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        "/usr/bin/ssh"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        "ssh"
    }
}

fn run_bridge(configuration: &BridgeConfiguration) -> Result<Vec<u8>, CpaPoolIssue> {
    let connect_timeout = configuration.timeout.as_secs().clamp(2, 30);
    let mut child = Command::new(ssh_executable())
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg(format!("ConnectTimeout={connect_timeout}"))
        .arg("-o")
        .arg("ServerAliveInterval=5")
        .arg("-o")
        .arg("ServerAliveCountMax=2")
        .arg("--")
        .arg(&configuration.host)
        .arg(&configuration.remote_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| issue(CpaPoolIssueCode::BridgeUnavailable, true))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| issue(CpaPoolIssueCode::BridgeUnavailable, true))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| issue(CpaPoolIssueCode::BridgeUnavailable, true))?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout));
    let stderr_reader = thread::spawn(move || read_bounded(stderr));
    let started = Instant::now();
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < configuration.timeout => {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(issue(CpaPoolIssueCode::BridgeTimeout, true));
            }
            Err(_) => break Err(issue(CpaPoolIssueCode::BridgeUnavailable, true)),
        }
    };
    let (stdout, stdout_truncated) = stdout_reader
        .join()
        .map_err(|_| issue(CpaPoolIssueCode::BridgeUnavailable, true))?;
    let (_, stderr_truncated) = stderr_reader
        .join()
        .map_err(|_| issue(CpaPoolIssueCode::BridgeUnavailable, true))?;
    let exit_status = exit_status?;
    if stdout_truncated || stderr_truncated {
        return Err(issue(CpaPoolIssueCode::OutputTooLarge, false));
    }
    if !exit_status.success() {
        return Err(issue(CpaPoolIssueCode::BridgeRejected, true));
    }
    Ok(stdout)
}

fn read_bounded(mut reader: impl Read) -> (Vec<u8>, bool) {
    let mut captured = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                let remaining = MAX_BRIDGE_OUTPUT_BYTES.saturating_sub(captured.len());
                if count <= remaining {
                    captured.extend_from_slice(&buffer[..count]);
                } else {
                    captured.extend_from_slice(&buffer[..remaining]);
                    truncated = true;
                }
            }
        }
    }
    (captured, truncated)
}

fn parse_bridge_response(bytes: &[u8]) -> Result<CpaPoolStatus, CpaPoolIssue> {
    let response: ScriptResponse = serde_json::from_slice(bytes)
        .map_err(|_| issue(CpaPoolIssueCode::InvalidResponse, false))?;
    let observed_now = now_string();
    let mut members = response
        .accounts
        .unwrap_or_default()
        .into_iter()
        .filter(|account| {
            account
                .auth_file
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
        })
        .map(|account| member_from_account(account, &observed_now))
        .collect::<Vec<_>>();

    if members.is_empty() {
        if let Some(record) = response
            .current
            .or_else(|| response.latest.into_iter().next())
        {
            if record_has_quota(&record) {
                members.push(member_from_legacy_record(record));
            }
        }
    }
    if members.is_empty() {
        return Err(issue(CpaPoolIssueCode::NoRecords, true));
    }
    let parent_member_id = members
        .iter()
        .find(|member| member.is_current_route)
        .or_else(|| members.iter().find(|member| member.is_route_preferred))
        .or_else(|| {
            members
                .iter()
                .find(|member| member.primary.is_some() || member.secondary.is_some())
        })
        .map(|member| member.id.clone());
    Ok(CpaPoolStatus {
        source: CpaPoolSource::Live,
        stale: false,
        fetched_at: None,
        checked_at: observed_now,
        parent_member_id,
        members,
        issue: None,
    })
}

fn member_from_account(account: PoolAccountRecord, observed_now: &str) -> CpaPoolMember {
    let latest = account.latest;
    let headers = latest
        .as_ref()
        .map(|record| &record.codex_headers)
        .cloned()
        .unwrap_or_default();
    let observed_at = latest
        .as_ref()
        .and_then(|record| normalized_timestamp(record.timestamp.as_deref()));
    let guard_state = account
        .quota_guard
        .as_ref()
        .and_then(|guard| clean_optional(guard.state.as_deref()))
        .or_else(|| {
            latest
                .as_ref()
                .and_then(|record| record.quota_guard.as_ref())
                .and_then(|guard| clean_optional(guard.state.as_deref()))
        });
    let guard_reason = account
        .quota_guard
        .as_ref()
        .and_then(|guard| clean_optional(guard.reason.as_deref()))
        .or_else(|| {
            latest
                .as_ref()
                .and_then(|record| record.quota_guard.as_ref())
                .and_then(|guard| clean_optional(guard.reason.as_deref()))
        });
    let estimated_remaining_successes = account.account_stats.as_ref().and_then(|stats| {
        [
            stats
                .primary
                .as_ref()
                .and_then(|capacity| capacity.estimated_remaining_successes),
            stats
                .secondary
                .as_ref()
                .and_then(|capacity| capacity.estimated_remaining_successes),
        ]
        .into_iter()
        .flatten()
        .min()
    });
    let id = clean_optional(Some(&account.id))
        .or_else(|| {
            account
                .auth_index
                .as_deref()
                .and_then(|value| clean_optional(Some(value)))
        })
        .unwrap_or_else(|| format!("member-{}", account.display_name));
    CpaPoolMember {
        id,
        display_name: clean_optional(Some(&account.display_name))
            .unwrap_or_else(|| "CPA member".to_string()),
        is_current_route: account.is_current_route.unwrap_or(false),
        is_route_preferred: account.is_route_preferred.unwrap_or(false),
        is_latest_request_route: account.is_latest_request_route.unwrap_or(false),
        primary: quota_window(&headers, "X-Codex-Primary"),
        secondary: quota_window(&headers, "X-Codex-Secondary"),
        plan_type: header_value(&headers, "X-Codex-Plan-Type"),
        model: latest.as_ref().and_then(|record| {
            clean_optional(record.model.as_deref())
                .or_else(|| clean_optional(record.alias.as_deref()))
        }),
        reasoning_effort: latest
            .as_ref()
            .and_then(|record| clean_optional(record.reasoning_effort.as_deref())),
        status_code: latest.as_ref().and_then(|record| record.status_code),
        failed: latest.as_ref().and_then(|record| record.failed),
        is_stale: latest.as_ref().is_some_and(|record| record.stale),
        refresh_skipped: account.refresh_skipped.unwrap_or(false)
            || latest.as_ref().is_some_and(|record| record.refresh_skipped),
        skip_reason: clean_optional(account.skip_reason.as_deref()).or_else(|| {
            latest
                .as_ref()
                .and_then(|record| clean_optional(record.skip_reason.as_deref()))
        }),
        quota_guard_state: guard_state,
        quota_guard_reason: guard_reason,
        stats_sample_count: account
            .account_stats
            .as_ref()
            .and_then(|stats| stats.sample_count),
        estimated_remaining_successes,
        observed_at: observed_at.or_else(|| Some(observed_now.to_string())),
    }
}

fn member_from_legacy_record(record: UsageRecord) -> CpaPoolMember {
    let observed_at =
        normalized_timestamp(record.timestamp.as_deref()).or_else(|| Some(now_string()));
    let guard_state = record
        .quota_guard
        .as_ref()
        .and_then(|guard| clean_optional(guard.state.as_deref()));
    let guard_reason = record
        .quota_guard
        .as_ref()
        .and_then(|guard| clean_optional(guard.reason.as_deref()));
    CpaPoolMember {
        id: "legacy-current".to_string(),
        display_name: "CPA current route".to_string(),
        is_current_route: true,
        is_route_preferred: false,
        is_latest_request_route: true,
        primary: quota_window(&record.codex_headers, "X-Codex-Primary"),
        secondary: quota_window(&record.codex_headers, "X-Codex-Secondary"),
        plan_type: header_value(&record.codex_headers, "X-Codex-Plan-Type"),
        model: clean_optional(record.model.as_deref())
            .or_else(|| clean_optional(record.alias.as_deref())),
        reasoning_effort: clean_optional(record.reasoning_effort.as_deref()),
        status_code: record.status_code,
        failed: record.failed,
        is_stale: record.stale,
        refresh_skipped: record.refresh_skipped,
        skip_reason: clean_optional(record.skip_reason.as_deref()),
        quota_guard_state: guard_state,
        quota_guard_reason: guard_reason,
        stats_sample_count: None,
        estimated_remaining_successes: None,
        observed_at,
    }
}

fn clean_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(512).collect())
}

fn normalized_timestamp(value: Option<&str>) -> Option<String> {
    let value = value?;
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn record_has_quota(record: &UsageRecord) -> bool {
    quota_window(&record.codex_headers, "X-Codex-Primary").is_some()
        || quota_window(&record.codex_headers, "X-Codex-Secondary").is_some()
}

fn header_value(headers: &BTreeMap<String, Vec<String>>, name: &str) -> Option<String> {
    headers.iter().find_map(|(key, values)| {
        key.eq_ignore_ascii_case(name)
            .then(|| values.iter().find_map(|value| clean_optional(Some(value))))
            .flatten()
    })
}

fn quota_window(headers: &BTreeMap<String, Vec<String>>, prefix: &str) -> Option<CpaQuotaWindow> {
    let used_percent = header_value(headers, &format!("{prefix}-Used-Percent"))?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))?;
    Some(CpaQuotaWindow {
        used_percent,
        remaining_percent: (100.0 - used_percent).clamp(0.0, 100.0),
        window_minutes: header_value(headers, &format!("{prefix}-Window-Minutes"))
            .and_then(|value| value.parse().ok()),
        resets_at: header_value(headers, &format!("{prefix}-Reset-At"))
            .and_then(|value| value.parse().ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_response() -> Vec<u8> {
        br#"{
          "records_saved": 2,
          "accounts": [
            {
              "id": "a",
              "display_name": "Alpha",
              "auth_file": "/secret/a.json",
              "is_current_route": false,
              "is_route_preferred": true,
              "is_latest_request_route": false,
              "quota_guard": {"state":"guarded","reason":"weekly floor"},
              "account_stats": {
                "sample_count": 9,
                "primary": {"estimated_remaining_successes": 12},
                "secondary": {"estimated_remaining_successes": 8}
              },
              "latest": {
                "timestamp": "2026-08-30T12:00:00.123Z",
                "model":"gpt-5.6-sol",
                "reasoning_effort":"high",
                "codex_headers": {
                  "x-codex-primary-used-percent":["25"],
                  "X-Codex-Primary-Window-Minutes":["300"],
                  "X-Codex-Primary-Reset-At":["1788100000"],
                  "X-Codex-Secondary-Used-Percent":["60"],
                  "X-Codex-Plan-Type":["pro"]
                }
              }
            },
            {
              "id": "b",
              "display_name": "Beta",
              "auth_file": "/secret/b.json",
              "is_current_route": true,
              "is_latest_request_route": true,
              "refresh_skipped": true,
              "skip_reason": "guarded"
            }
          ]
        }"#
        .to_vec()
    }

    #[test]
    fn parses_members_and_keeps_route_roles_separate() {
        let status = parse_bridge_response(&sample_response()).expect("response");
        assert_eq!(status.source, CpaPoolSource::Live);
        assert_eq!(status.parent_member_id.as_deref(), Some("b"));
        assert_eq!(status.members.len(), 2);
        assert!(status.members[0].is_route_preferred);
        assert!(!status.members[0].is_current_route);
        assert!(status.members[1].is_current_route);
        assert!(status.members[1].is_latest_request_route);
    }

    #[test]
    fn calculates_quota_and_conservative_success_estimate() {
        let status = parse_bridge_response(&sample_response()).expect("response");
        let member = &status.members[0];
        assert_eq!(
            member.primary.as_ref().map(|value| value.remaining_percent),
            Some(75.0)
        );
        assert_eq!(
            member
                .secondary
                .as_ref()
                .map(|value| value.remaining_percent),
            Some(40.0)
        );
        assert_eq!(member.estimated_remaining_successes, Some(8));
        assert_eq!(member.stats_sample_count, Some(9));
    }

    #[test]
    fn cache_never_contains_bridge_auth_paths() {
        let status = parse_bridge_response(&sample_response()).expect("response");
        let serialized = serde_json::to_string(&status).expect("serialize");
        assert!(!serialized.contains("/secret/"));
        assert!(!serialized.contains("authFile"));
    }

    #[test]
    fn placeholder_member_does_not_invent_full_quota() {
        let status = parse_bridge_response(&sample_response()).expect("response");
        assert_eq!(status.members[1].primary, None);
        assert_eq!(status.members[1].secondary, None);
        assert!(status.members[1].refresh_skipped);
    }

    #[test]
    fn ignores_accounts_without_auth_file() {
        let response = br#"{"accounts":[{"id":"hidden","display_name":"Hidden","latest":{"codex_headers":{"X-Codex-Primary-Used-Percent":["10"]}}}]}"#;
        let error = parse_bridge_response(response).expect_err("no eligible account");
        assert_eq!(error.code, CpaPoolIssueCode::NoRecords);
    }

    #[test]
    fn falls_back_to_legacy_current_record() {
        let response = br#"{"current":{"codex_headers":{"X-Codex-Primary-Used-Percent":["10"]}}}"#;
        let status = parse_bridge_response(response).expect("legacy status");
        assert_eq!(status.members.len(), 1);
        assert_eq!(status.parent_member_id.as_deref(), Some("legacy-current"));
        assert_eq!(
            status.members[0]
                .primary
                .as_ref()
                .map(|value| value.remaining_percent),
            Some(90.0)
        );
    }

    #[test]
    fn rejects_invalid_percentages_and_empty_payloads() {
        let response = br#"{"current":{"codex_headers":{"X-Codex-Primary-Used-Percent":["101"]}}}"#;
        let error = parse_bridge_response(response).expect_err("invalid quota");
        assert_eq!(error.code, CpaPoolIssueCode::NoRecords);
    }

    #[test]
    fn accepts_nullable_legacy_pool_fields_without_inventing_state() {
        let response = br#"{
          "accounts": [{
            "id":"member",
            "display_name":"Member",
            "auth_file":"member.json",
            "is_current_route":null,
            "is_route_preferred":null,
            "is_latest_request_route":null,
            "refresh_skipped":null
          }]
        }"#;
        let status = parse_bridge_response(response).expect("nullable status");
        assert_eq!(status.members.len(), 1);
        assert!(!status.members[0].is_current_route);
        assert!(!status.members[0].is_route_preferred);
        assert!(!status.members[0].is_latest_request_route);
        assert_eq!(status.parent_member_id, None);
    }

    #[test]
    fn issue_codes_have_stable_snake_case_wire_values() {
        let value = serde_json::to_value(issue(CpaPoolIssueCode::BridgeTimeout, true))
            .expect("serialize issue");
        assert_eq!(value["code"], "bridge_timeout");
        assert_eq!(value["retryable"], true);
        let disabled = serde_json::to_value(issue(CpaPoolIssueCode::BridgeDisabled, false))
            .expect("serialize disabled issue");
        assert_eq!(disabled["code"], "bridge_disabled");
        assert_eq!(disabled["retryable"], false);
    }

    #[test]
    fn validates_ssh_destinations_without_accepting_options_or_spaces() {
        assert!(valid_ssh_host("cpa-host"));
        assert!(valid_ssh_host("ubuntu@10.0.0.8"));
        assert!(!valid_ssh_host("-Fbad"));
        assert!(!valid_ssh_host("host; command"));
        assert!(!valid_ssh_host("host name"));
    }

    #[test]
    fn validates_and_normalizes_persisted_bridge_settings() {
        let normalized = normalize_bridge_settings(CpaBridgeSettings {
            enabled: true,
            ssh_host: " operator@cpa-host ".to_string(),
            refresh_seconds: 120,
            timeout_seconds: 20,
        })
        .expect("valid settings");
        assert_eq!(normalized.ssh_host, "operator@cpa-host");

        let invalid_host = normalize_bridge_settings(CpaBridgeSettings {
            ssh_host: "host; command".to_string(),
            ..CpaBridgeSettings::default()
        });
        assert!(invalid_host.is_err());
        let invalid_refresh = normalize_bridge_settings(CpaBridgeSettings {
            refresh_seconds: MIN_CPA_REFRESH_SECONDS - 1,
            ..CpaBridgeSettings::default()
        });
        assert!(invalid_refresh.is_err());
        let invalid_timeout = normalize_bridge_settings(CpaBridgeSettings {
            timeout_seconds: MAX_CPA_TIMEOUT_SECONDS + 1,
            ..CpaBridgeSettings::default()
        });
        assert!(invalid_timeout.is_err());
    }

    #[test]
    fn unconfigured_bridge_can_stay_disabled_but_cannot_be_enabled() {
        let defaults = CpaBridgeSettings::default();
        assert!(!defaults.enabled);
        assert!(defaults.ssh_host.is_empty());
        assert!(normalize_bridge_settings(defaults.clone()).is_ok());
        assert!(normalize_bridge_settings(CpaBridgeSettings {
            enabled: true,
            ..defaults
        })
        .is_err());
    }

    #[test]
    fn bridge_requires_an_explicit_host_and_remote_command() {
        let settings = CpaBridgeSettings {
            enabled: true,
            ssh_host: "operator@cpa-host".to_string(),
            ..CpaBridgeSettings::default()
        };
        for command in [None, Some(String::new()), Some(" \t".to_string())] {
            let failure = BridgeConfiguration::from_settings_and_command(&settings, command)
                .expect_err("missing command must not fall back to a private server script");
            assert_eq!(failure.code, CpaPoolIssueCode::InvalidConfiguration);
        }
        let command = "/opt/cpa/bin/quota --json".to_string();
        let configured =
            BridgeConfiguration::from_settings_and_command(&settings, Some(command.clone()))
                .expect("explicit configuration");
        assert_eq!(configured.host, settings.ssh_host);
        assert_eq!(configured.remote_command, command);
        assert!(BridgeConfiguration::from_settings_and_command(
            &CpaBridgeSettings::default(),
            Some(command),
        )
        .is_err());
    }

    #[test]
    fn background_refresh_interval_is_bounded_for_manually_edited_settings() {
        let mut settings = CpaBridgeSettings {
            refresh_seconds: 0,
            ..CpaBridgeSettings::default()
        };
        assert_eq!(
            background_refresh_interval(&settings),
            Duration::from_secs(MIN_CPA_REFRESH_SECONDS)
        );
        settings.refresh_seconds = u64::MAX;
        assert_eq!(
            background_refresh_interval(&settings),
            Duration::from_secs(MAX_CPA_REFRESH_SECONDS)
        );
    }

    #[test]
    fn failed_refresh_marks_cached_members_stale_without_changing_values() {
        let live = parse_bridge_response(&sample_response()).expect("response");
        let cache = CpaPoolCache {
            version: CACHE_VERSION,
            fetched_at: "2026-08-30T12:00:00Z".to_string(),
            parent_member_id: live.parent_member_id,
            members: live.members,
        };
        let cached = status_from_cache(cache, Some(issue(CpaPoolIssueCode::BridgeTimeout, true)));
        assert_eq!(cached.source, CpaPoolSource::Cached);
        assert!(cached.stale);
        assert!(cached.members.iter().all(|member| member.is_stale));
        assert_eq!(
            cached.members[0]
                .primary
                .as_ref()
                .map(|value| value.remaining_percent),
            Some(75.0)
        );
    }

    #[test]
    fn successful_placeholder_refresh_retains_previous_member_quota_as_stale() {
        let previous = parse_bridge_response(&sample_response()).expect("previous response");
        let cache = CpaPoolCache {
            version: CACHE_VERSION,
            fetched_at: "2026-08-30T12:00:00Z".to_string(),
            parent_member_id: previous.parent_member_id.clone(),
            members: previous.members.clone(),
        };
        let mut current = parse_bridge_response(
            br#"{"accounts":[{"id":"a","display_name":"Alpha","auth_file":"a.json","is_current_route":true}]}"#,
        )
        .expect("placeholder response");
        merge_cached_quota_facts(&mut current, &cache);
        assert_eq!(current.source, CpaPoolSource::Live);
        assert!(current.stale);
        assert!(current.members[0].is_stale);
        assert_eq!(
            current.members[0]
                .primary
                .as_ref()
                .map(|value| value.remaining_percent),
            Some(75.0)
        );
        assert_eq!(
            current.members[0].observed_at,
            previous.members[0].observed_at
        );
    }

    #[test]
    fn bounded_reader_drains_excess_output_and_reports_truncation() {
        let bytes = vec![b'x'; MAX_BRIDGE_OUTPUT_BYTES + 17];
        let (captured, truncated) = read_bounded(bytes.as_slice());
        assert_eq!(captured.len(), MAX_BRIDGE_OUTPUT_BYTES);
        assert!(truncated);
    }
}
