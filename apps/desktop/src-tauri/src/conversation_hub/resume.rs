const MAX_RESUME_CWD_CHARS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedThreadResume {
    session_id: String,
    cwd: String,
    mode: &'static str,
    arguments: Vec<String>,
    display_command: String,
}

fn validate_resume_directory(value: &str) -> Result<PathBuf, String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_RESUME_CWD_CHARS
        || trimmed.chars().any(char::is_control)
    {
        return Err("目标项目目录无效。".to_string());
    }
    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return Err("目标项目目录必须是绝对路径。".to_string());
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "目标项目目录不存在或无法访问。".to_string())?;
    let metadata = fs::metadata(&canonical)
        .map_err(|_| "目标项目目录不存在或无法访问。".to_string())?;
    if !metadata.is_dir() {
        return Err("目标项目路径不是目录。".to_string());
    }
    fs::read_dir(&canonical).map_err(|_| "目标项目目录无法读取。".to_string())?;
    Ok(canonical)
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn prepare_thread_resume(
    codex_home: &Path,
    session_id: &str,
    target_cwd: Option<&str>,
) -> Result<PreparedThreadResume, String> {
    let Some(base_command) = safe_resume_command(session_id) else {
        return Err("会话 ID 无效，已拒绝启动。".to_string());
    };
    let snapshots = gather_snapshots(codex_home)?;
    let matches = snapshots
        .iter()
        .filter(|snapshot| snapshot.session_id == session_id)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!(
            "无法唯一定位会话 {session_id}，请刷新列表或先修复可见性。"
        ));
    }
    let snapshot = matches[0];
    if rollout_status(&snapshot.relative_path) != Some("active") {
        return Err("该会话已归档，请先恢复到 active 后再继续。".to_string());
    }

    let (cwd, mode, arguments, display_command) = if let Some(target_cwd) = target_cwd {
        let cwd = validate_resume_directory(target_cwd)?
            .to_string_lossy()
            .into_owned();
        (
            cwd.clone(),
            "temporaryDirectory",
            vec![
                "resume".to_string(),
                session_id.to_string(),
                "-C".to_string(),
                cwd.clone(),
            ],
            format!("{base_command} -C {}", shell_quote(&cwd)),
        )
    } else {
        validate_resume_directory(&snapshot.cwd).map_err(|_| {
            "会话记录的项目目录已不可用，请选择“换目录继续”。".to_string()
        })?;
        (
            snapshot.cwd.clone(),
            "savedDirectory",
            vec!["resume".to_string(), session_id.to_string()],
            base_command,
        )
    };

    Ok(PreparedThreadResume {
        session_id: session_id.to_string(),
        cwd,
        mode,
        arguments,
        display_command,
    })
}

#[cfg(target_os = "macos")]
fn launch_resume_process(executable: &Path, arguments: &[String]) -> Result<bool, String> {
    const SCRIPT: &str = concat!(
        "on run argv\n",
        "set commandText to quoted form of item 1 of argv\n",
        "repeat with itemIndex from 2 to count of argv\n",
        "set commandText to commandText & \" \" & quoted form of item itemIndex of argv\n",
        "end repeat\n",
        "tell application \"Terminal\"\n",
        "activate\n",
        "do script commandText\n",
        "end tell\n",
        "end run"
    );
    let status = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", SCRIPT, "--"])
        .arg(executable)
        .args(arguments)
        .status()
        .map_err(|error| format!("无法打开 Terminal：{error}"))?;
    if status.success() {
        Ok(true)
    } else {
        Err(format!(
            "Terminal 未能启动 Codex（退出状态 {}）。",
            status.code().map_or_else(|| "unknown".to_string(), |code| code.to_string())
        ))
    }
}

#[cfg(target_os = "windows")]
fn launch_resume_process(executable: &Path, arguments: &[String]) -> Result<bool, String> {
    std::process::Command::new("wt.exe")
        .arg(executable)
        .args(arguments)
        .spawn()
        .map(|_| true)
        .map_err(|error| format!("无法打开 Windows Terminal：{error}"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn launch_resume_process(executable: &Path, arguments: &[String]) -> Result<bool, String> {
    let mut command = Vec::with_capacity(arguments.len() + 1);
    command.push(executable.to_string_lossy().into_owned());
    command.extend(arguments.iter().cloned());
    for terminal in ["x-terminal-emulator", "gnome-terminal", "konsole", "xfce4-terminal"] {
        let mut child = std::process::Command::new(terminal);
        if terminal == "gnome-terminal" {
            child.arg("--");
        } else {
            child.arg("-e");
        }
        match child.args(&command).spawn() {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("无法打开终端：{error}")),
        }
    }
    Err("未找到可用的终端程序。".to_string())
}

pub(crate) fn resume_codex_thread_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    target_cwd: Option<String>,
) -> Result<ThreadResumeResult, String> {
    recover_incomplete_rebind_transactions(&app)?;
    recover_incomplete_visibility_repair_transactions(&app)?;
    let paths = resolve_paths(&app)?;
    let prepared = prepare_thread_resume(&paths.codex_home, &session_id, target_cwd.as_deref())?;
    let executable = crate::capacity_bridge::preferred_codex_executable()
        .unwrap_or_else(|| PathBuf::from("codex"));
    let launched = launch_resume_process(&executable, &prepared.arguments)?;
    Ok(ThreadResumeResult {
        session_id: prepared.session_id,
        cwd: prepared.cwd,
        mode: prepared.mode.to_string(),
        resume_command: prepared.display_command,
        launched,
        message: if prepared.mode == "temporaryDirectory" {
            "已在所选目录打开 Terminal 并继续该会话；原会话目录未改变。".to_string()
        } else {
            "已在原项目目录打开 Terminal 并继续该会话。".to_string()
        },
    })
}
