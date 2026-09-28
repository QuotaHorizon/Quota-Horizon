use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tokio::io::{AsyncRead, AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::jsonrpc::JsonlSession;
use super::{
    CapacityRead, InitializeResult, RuntimeConfig, RuntimeError, initialize_session,
    read_initialized_session,
};

const CODEX_APP_SERVER_ARGUMENTS: [&str; 5] = ["-s", "read-only", "-a", "never", "app-server"];

pub(super) async fn read_codex_process(
    executable: &Path,
    config: RuntimeConfig,
) -> Result<CapacityRead, RuntimeError> {
    let mut connection = AppServerConnection::connect(executable, config).await?;
    let read_result = connection.read_capacity().await;
    let shutdown_result = connection.shutdown().await;
    shutdown_result?;
    read_result
}

/// One verified Codex app-server child with a single initialized JSONL session.
///
/// The connection can serve multiple sequential quota reads. Callers must share
/// one instance per environment and call `shutdown` at idle/application exit.
pub struct AppServerConnection {
    child: Child,
    session: Option<JsonlSession<BufReader<ChildStdout>, ChildStdin>>,
    initialize: InitializeResult,
    stderr_task: Option<JoinHandle<StderrSummary>>,
    stderr_state: Arc<StderrState>,
    config: RuntimeConfig,
}

impl AppServerConnection {
    pub async fn connect(executable: &Path, config: RuntimeConfig) -> Result<Self, RuntimeError> {
        config.validate()?;

        let mut command = Command::new(executable);
        command
            .args(CODEX_APP_SERVER_ARGUMENTS)
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|_| RuntimeError::Spawn)?;
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
            shutdown_child(&mut child, &config).await?;
            return Err(RuntimeError::PipeUnavailable);
        };
        let stderr_state = Arc::new(StderrState::default());
        let stderr_task = tokio::spawn(capture_stderr(
            stderr,
            config.stderr_ring_bytes,
            Arc::clone(&stderr_state),
        ));

        let mut session = JsonlSession::new(BufReader::new(stdout), stdin, config.clone());
        let initialize = match initialize_session(&mut session, &config).await {
            Ok(initialize) => initialize,
            Err(error) => {
                drop(session);
                let shutdown_result = shutdown_child(&mut child, &config).await;
                let _ = finish_stderr_capture(stderr_task, &config).await;
                shutdown_result?;
                return Err(error);
            }
        };

        Ok(Self {
            child,
            session: Some(session),
            initialize,
            stderr_task: Some(stderr_task),
            stderr_state,
            config,
        })
    }

    pub async fn read_capacity(&mut self) -> Result<CapacityRead, RuntimeError> {
        let session = self.session.as_mut().ok_or(RuntimeError::PipeUnavailable)?;
        let mut read =
            read_initialized_session(session, &self.config, self.initialize.clone()).await?;
        let stderr_summary = self.stderr_state.snapshot();
        read.stderr_captured_bytes = stderr_summary.captured_bytes;
        read.stderr_truncated = stderr_summary.truncated;
        Ok(read)
    }

    pub async fn shutdown(mut self) -> Result<(), RuntimeError> {
        drop(self.session.take());
        let shutdown_result = shutdown_child(&mut self.child, &self.config).await;
        if let Some(stderr_task) = self.stderr_task.take() {
            let _ = finish_stderr_capture(stderr_task, &self.config).await;
        }
        shutdown_result
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct StderrSummary {
    captured_bytes: usize,
    truncated: bool,
}

#[derive(Debug, Default)]
struct StderrState {
    captured_bytes: AtomicUsize,
    truncated: AtomicBool,
}

impl StderrState {
    fn update(&self, summary: StderrSummary) {
        self.captured_bytes
            .store(summary.captured_bytes, Ordering::Relaxed);
        self.truncated.store(summary.truncated, Ordering::Relaxed);
    }

    fn snapshot(&self) -> StderrSummary {
        StderrSummary {
            captured_bytes: self.captured_bytes.load(Ordering::Relaxed),
            truncated: self.truncated.load(Ordering::Relaxed),
        }
    }
}

async fn capture_stderr(
    mut stderr: impl AsyncRead + Unpin,
    limit: usize,
    state: Arc<StderrState>,
) -> StderrSummary {
    let mut ring = VecDeque::with_capacity(limit.min(64 * 1024));
    let mut buffer = [0_u8; 4096];
    let mut truncated = false;

    loop {
        let bytes_read = match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(bytes_read) => bytes_read,
        };
        for byte in &buffer[..bytes_read] {
            if ring.len() == limit {
                ring.pop_front();
                truncated = true;
            }
            ring.push_back(*byte);
        }
        state.update(StderrSummary {
            captured_bytes: ring.len(),
            truncated,
        });
    }

    let summary = StderrSummary {
        captured_bytes: ring.len(),
        truncated,
    };
    state.update(summary);
    summary
}

async fn finish_stderr_capture(
    mut task: JoinHandle<StderrSummary>,
    config: &RuntimeConfig,
) -> StderrSummary {
    match timeout(config.request_timeout, &mut task).await {
        Ok(Ok(summary)) => summary,
        Ok(Err(_)) => StderrSummary::default(),
        Err(_) => {
            task.abort();
            StderrSummary::default()
        }
    }
}

async fn shutdown_child(child: &mut Child, config: &RuntimeConfig) -> Result<(), RuntimeError> {
    if child
        .try_wait()
        .map_err(|_| RuntimeError::Shutdown)?
        .is_none()
    {
        child.start_kill().map_err(|_| RuntimeError::Shutdown)?;
    }

    timeout(config.startup_timeout, child.wait())
        .await
        .map_err(|_| RuntimeError::Shutdown)?
        .map_err(|_| RuntimeError::Shutdown)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tokio::io::{AsyncWriteExt, duplex};

    #[tokio::test]
    async fn stderr_capture_keeps_only_the_bounded_tail() {
        let (mut writer, reader) = duplex(64);
        let producer = tokio::spawn(async move {
            writer.write_all(b"0123456789").await.unwrap();
        });

        let summary = capture_stderr(reader, 4, Arc::new(StderrState::default())).await;
        producer.await.unwrap();

        assert_eq!(summary.captured_bytes, 4);
        assert!(summary.truncated);
    }

    #[test]
    fn production_command_arguments_are_not_caller_configurable() {
        let arguments: Vec<Value> = CODEX_APP_SERVER_ARGUMENTS
            .into_iter()
            .map(Value::from)
            .collect();
        assert_eq!(arguments[4], "app-server");
    }
}
