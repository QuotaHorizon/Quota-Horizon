//! Optional process supervision. The independent CLI owns its lock, data and
//! worker; Horizon only asks it to ensure that worker is running.
use super::*;
use std::process::{Command, Stdio};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configuration {
    python_path: PathBuf,
    module_directory: PathBuf,
    database_path: PathBuf,
    producer_database_path: Option<PathBuf>,
    config_path: Option<PathBuf>,
    output_path: PathBuf,
    html_path: Option<PathBuf>,
}
impl Configuration {
    fn valid(&self) -> bool {
        self.python_path.is_absolute()
            && self.python_path.is_file()
            && self.module_directory.is_absolute()
            && self
                .module_directory
                .join("reset_observer/__main__.py")
                .is_file()
            && self.database_path.is_absolute()
            && self.output_path.is_absolute()
            && [
                &self.producer_database_path,
                &self.config_path,
                &self.html_path,
            ]
            .into_iter()
            .flatten()
            .all(|p| p.is_absolute())
    }
    fn command(&self) -> Command {
        self.action("start")
    }
    fn action(&self, action: &str) -> Command {
        let mut command = Command::new(&self.python_path);
        if let Some(root) = self
            .python_path
            .parent()
            .into_iter()
            .flat_map(|parent| [Some(parent), parent.parent()])
            .flatten()
            .find(|root| root.join("reset-runtime-manifest.json").is_file())
        {
            command.args(["-I", "-B", "-X", "utf8"]);
            if root.join("certs.pem").is_file() {
                command.env("SSL_CERT_FILE", root.join("certs.pem"));
            }
        }
        command
            .current_dir(&self.module_directory)
            .args(["-m", "reset_observer", action, "--db"])
            .arg(&self.database_path)
            .arg("--output")
            .arg(&self.output_path);
        for (flag, value) in [
            ("--producer-db", &self.producer_database_path),
            ("--config", &self.config_path),
            ("--html", &self.html_path),
        ] {
            if let Some(value) = value {
                command.arg(flag).arg(value);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        command
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub managed: bool,
    state: String,
    checked_at: Option<String>,
    retry_at: Option<String>,
    issue: Option<String>,
    health: Option<String>,
    last_capture_at: Option<String>,
    last_report_at: Option<String>,
    storage: Option<StorageStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase", deserialize = "snake_case"))]
struct StorageStatus {
    observer_bytes: u64,
    producer_bytes: Option<u64>,
    log_bytes: u64,
    log_limit_bytes: u64,
    maintenance: Vec<MaintenanceStatus>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct MaintenanceStatus {
    module: String,
    phase: String,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Default, Deserialize)]
struct Progress {
    health: Option<String>,
    issue: Option<String>,
    last_capture_at: Option<String>,
    last_report_at: Option<String>,
    storage: Option<StorageStatus>,
}
impl Progress {
    fn apply(self, state: &mut Status) {
        state.health = self.health.filter(|v| {
            matches!(
                v.as_str(),
                "healthy"
                    | "catching_up"
                    | "collecting"
                    | "evaluating"
                    | "maintaining"
                    | "degraded"
                    | "stalled"
                    | "recovering"
                    | "starting"
                    | "external"
            )
        });
        state.issue = self.issue.filter(|v| {
            matches!(
                v.as_str(),
                "clock_changed"
                    | "capture_stalled"
                    | "evaluation_stalled"
                    | "capture_failed"
                    | "evaluation_failed"
                    | "source_updates_failed"
            )
        });
        fn timestamp(value: Option<String>) -> Option<String> {
            value
                .and_then(|v| DateTime::parse_from_rfc3339(&v).ok())
                .map(|v| v.to_rfc3339())
        }
        state.last_capture_at = timestamp(self.last_capture_at);
        state.last_report_at = timestamp(self.last_report_at);
        state.storage = self
            .storage
            .filter(|s| {
                s.observer_bytes <= 1 << 50
                    && s.producer_bytes.unwrap_or(0) <= 1 << 50
                    && s.log_bytes <= 1 << 50
                    && s.log_limit_bytes <= 1 << 30
                    && s.maintenance.len() <= 2
            })
            .map(|mut s| {
                s.maintenance
                    .retain(|v| matches!(v.module.as_str(), "observer" | "producer"));
                for m in &mut s.maintenance {
                    if !matches!(
                        m.phase.as_str(),
                        "compress" | "retire" | "bodies" | "vacuum" | "done" | "unavailable"
                    ) {
                        m.phase = "unknown".into();
                    }
                    m.state = m.state.take().filter(|v| {
                        matches!(
                            v.as_str(),
                            "progress" | "deferred" | "retry_later" | "waiting_for_disk_space"
                        )
                    });
                    m.error = m.error.take().filter(|v| {
                        matches!(
                            v.as_str(),
                            "OSError"
                                | "PermissionError"
                                | "ValueError"
                                | "OperationalError"
                                | "DatabaseError"
                                | "IntegrityError"
                        )
                    });
                }
                s
            });
    }
}
static STATUS: Mutex<Option<(PathBuf, Status)>> = Mutex::new(None);
static ACTIVE: Mutex<Option<(PathBuf, Configuration)>> = Mutex::new(None);

pub(crate) fn refresh(directory: &Path) -> Result<(), String> {
    let configuration = ACTIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|(path, _)| path == directory)
        .map(|(_, config)| config.clone())
        .or_else(|| read_json::<Configuration>(&directory.join("reset-radar-runtime.json")).ok())
        .filter(Configuration::valid);
    if let Some(config) = configuration {
        refresh_receipt(invoke(config.action("refresh"), Duration::from_secs(45)))?;
    }
    Ok(())
}

fn refresh_receipt(result: Result<&str, &str>) -> Result<(), String> {
    match result {
        Ok("refreshed" | "partial") => Ok(()),
        Ok("cached") => Err("结果来源沿用有效缓存，本轮未完成新采集。".into()),
        Err("refresh_failed") => Err("独立雷达数据更新失败，后台将继续重试。".into()),
        _ => Err("独立雷达更新未完成，后台将继续重试。".into()),
    }
}

pub(crate) fn status(directory: &Path) -> Status {
    if let Some((path, status)) = STATUS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if path == directory {
            return status.clone();
        }
    }
    let managed = directory.join("reset-radar-runtime.json").exists();
    Status {
        managed,
        state: if managed { "checking" } else { "external" }.into(),
        ..Status::default()
    }
}

#[derive(Default)]
pub(crate) struct Supervisor {
    configuration: Option<Configuration>,
    bundled: Option<Configuration>,
    failures: u32,
    next_check: Option<DateTime<Utc>>,
    state: Status,
}
impl Supervisor {
    pub(crate) fn bundled(resources: Option<PathBuf>, directory: &Path) -> Self {
        let mut supervisor = Self::default();
        if let Some(root) = resources.map(|p| p.join("reset-runtime")) {
            if root.join("reset-runtime-manifest.json").is_file() {
                supervisor.bundled = Some(bundled_configuration(&root, directory));
            }
        }
        supervisor
    }
    pub(crate) fn tick(&mut self, directory: &Path, at: DateTime<Utc>) {
        let mut progress = None;
        self.check(directory, at, |c| {
            let (state, receipt) = invoke_progress(c.command(), Duration::from_secs(10))?;
            progress = Some(receipt);
            Ok(state)
        });
        if let Some(progress) = progress {
            progress.apply(&mut self.state);
        }
        *STATUS.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((directory.into(), self.state.clone()));
        *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) =
            self.configuration.clone().map(|c| (directory.into(), c));
    }
    fn check(
        &mut self,
        directory: &Path,
        at: DateTime<Utc>,
        ensure: impl FnOnce(&Configuration) -> Result<&'static str, &'static str>,
    ) {
        let path = directory.join("reset-radar-runtime.json");
        let fallback = if !directory.join("reset-radar-connection.json").exists() {
            self.bundled.clone()
        } else {
            None
        };
        if !path.exists() && fallback.is_none() {
            self.configuration = None;
            self.failures = 0;
            self.next_check = None;
            self.state = Status::default();
            self.state.state = "external".into();
            return;
        }
        self.state.managed = true;
        let config = if path.exists() {
            read_json::<Configuration>(&path).ok()
        } else {
            fallback
        }
        .filter(Configuration::valid);
        if config != self.configuration {
            self.configuration = config.clone();
            self.failures = 0;
            self.next_check = None;
        }
        if self.next_check.is_some_and(|next| next > at) {
            return;
        }
        self.state.checked_at = Some(at.to_rfc3339_opts(SecondsFormat::Millis, true));
        match config
            .as_ref()
            .ok_or("configuration_invalid")
            .and_then(ensure)
        {
            Ok(state) => {
                self.failures = 0;
                self.state.state = state.into();
                self.state.issue = None;
                self.state.retry_at = None;
                self.next_check = Some(at + chrono::Duration::seconds(60));
            }
            Err(issue) => {
                self.failures = self.failures.saturating_add(1);
                let seconds = (60 * (1_i64 << (self.failures - 1).min(4))).min(900);
                let next = at + chrono::Duration::seconds(seconds);
                self.next_check = Some(next);
                self.state.state = "retrying".into();
                self.state.issue = Some(issue.into());
                self.state.retry_at = Some(next.to_rfc3339_opts(SecondsFormat::Millis, true));
                self.state.health = None;
            }
        }
    }
}

fn bundled_configuration(root: &Path, directory: &Path) -> Configuration {
    let python = root.join(if cfg!(windows) {
        "python.exe"
    } else {
        "bin/python3"
    });
    let version = read_json::<serde_json::Value>(&root.join("reset-runtime-manifest.json"))
        .ok()
        .and_then(|v| v.get("python").and_then(|p| p.as_str()).map(str::to_owned))
        .filter(|s| matches!(s.as_str(), "3.11" | "3.12" | "3.13" | "3.14"))
        .unwrap_or_else(|| "3.11".into());
    let producer = directory.join("reset-intelligence");
    let observer = directory.join("reset-observer");
    let config_path = observer.join("runner.json");
    let config = serde_json::json!({
        "producer_command": [python, "-I", "-B", "-X", "utf8", "-m", "reset_intelligence", "run", "--db", producer.join("state.sqlite"),
            "--output", producer.join("consumer.json"), "--audit-output", producer.join("audit.json")]
    });
    // Generated launch arguments contain no user configuration. Refresh them
    // when the app moves; the independent module databases stay in app data.
    if std::fs::create_dir_all(&observer).is_ok() {
        let text = serde_json::to_vec_pretty(&config).unwrap_or_default();
        if std::fs::read(&config_path).ok().as_ref() != Some(&text) {
            let temporary = observer.join("runner.json.tmp");
            if std::fs::write(&temporary, text).is_ok() {
                let _ = std::fs::rename(temporary, &config_path);
            }
        }
    }
    Configuration {
        python_path: python,
        module_directory: root.join(if cfg!(windows) {
            "Lib/site-packages".into()
        } else {
            format!("lib/python{version}/site-packages")
        }),
        database_path: observer.join("state.sqlite"),
        producer_database_path: Some(producer.join("state.sqlite")),
        config_path: Some(config_path),
        output_path: observer.join("report.json"),
        html_path: Some(observer.join("report.html")),
    }
}

fn invoke(command: Command, timeout: Duration) -> Result<&'static str, &'static str> {
    invoke_progress(command, timeout).map(|(state, _)| state)
}

fn invoke_progress(
    mut command: Command,
    timeout: Duration,
) -> Result<(&'static str, Progress), &'static str> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "environment_unavailable")?;
    let stdout = child.stdout.take().ok_or("start_failed")?;
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.take(16_385).read_to_end(&mut bytes);
        let _ = send.send(result.map(|_| bytes));
    });
    let began = Instant::now();
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if began.elapsed() < timeout => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("start_timeout");
            }
        }
    };
    if !success {
        return Err("start_failed");
    }
    let bytes = receive
        .recv_timeout(Duration::from_secs(1))
        .map_err(|_| "start_failed")?
        .map_err(|_| "start_failed")?;
    if bytes.len() > 16_384 {
        return Err("start_failed");
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "start_failed")?;
    let state = match value.get("state").and_then(|s| s.as_str()) {
        Some("already_running") => Ok("running"),
        Some("starting" | "restarting") => Ok("starting"),
        Some("refreshed") => Ok("refreshed"),
        Some("partial") => Ok("partial"),
        Some("cached") => Ok("cached"),
        Some("failed") => Err("refresh_failed"),
        Some("refreshing") => Ok("refreshing"),
        _ => Err("start_failed"),
    }?;
    Ok((state, serde_json::from_value(value).unwrap_or_default()))
}

#[cfg(test)]
mod tests;
