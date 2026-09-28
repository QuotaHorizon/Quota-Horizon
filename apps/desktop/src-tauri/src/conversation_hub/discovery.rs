fn normalized_ids(values: Vec<String>) -> HashSet<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn index_values(codex_home: &Path) -> Result<HashMap<String, Value>, String> {
    let path = codex_home.join(INDEX_NAME);
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("无法读取会话索引 {}：{error}", path.display()))?;
    let mut result: HashMap<String, Value> = HashMap::new();
    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            // Match the Viewer index repository: an older duplicate appended
            // later must not replace a newer title/activity observation.
            if result.get(id).is_some_and(|previous| {
                index_timestamp(Some(previous)) > index_timestamp(Some(&value))
            }) {
                continue;
            }
            result.insert(id.to_string(), value);
        }
    }
    Ok(result)
}

fn title_from_index(value: &Value) -> Option<String> {
    ["thread_name", "threadName", "title", "name"]
        .into_iter()
        .filter_map(|key| value.get(key).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

fn unix_seconds(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_i64() {
        return Some(if number > 10_000_000_000 {
            number / 1000
        } else {
            number
        });
    }
    let text = value.as_str()?.trim();
    if let Ok(number) = text.parse::<i64>() {
        return Some(if number > 10_000_000_000 {
            number / 1000
        } else {
            number
        });
    }
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|date| date.timestamp())
}

fn modified_seconds(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
}

fn index_timestamp(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    [
        "updated_at",
        "updatedAt",
        "last_updated_at",
        "lastUpdatedAt",
    ]
    .into_iter()
    .filter_map(|key| value.get(key))
    .find_map(unix_seconds)
}

fn compressed_rollout_path(path: &Path) -> PathBuf {
    path.with_extension("jsonl.zst")
}

fn logical_rollout_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    if name.starts_with("rollout-") && name.ends_with(".jsonl") {
        return Some(path.to_path_buf());
    }
    if name.starts_with("rollout-") && name.ends_with(".jsonl.zst") {
        return Some(path.with_file_name(name.trim_end_matches(".zst")));
    }
    None
}

fn rollout_physical_paths(logical_path: &Path) -> Vec<PathBuf> {
    [
        logical_path.to_path_buf(),
        compressed_rollout_path(logical_path),
    ]
    .into_iter()
    .filter(|path| path.is_file())
    .collect()
}

fn preferred_rollout_path(logical_path: &Path) -> Option<PathBuf> {
    logical_path
        .is_file()
        .then(|| logical_path.to_path_buf())
        .or_else(|| {
            let compressed = compressed_rollout_path(logical_path);
            compressed.is_file().then_some(compressed)
        })
}

fn rollout_reader(path: &Path) -> Result<Box<dyn BufRead + Send>, String> {
    let file = File::open(path)
        .map_err(|error| format!("无法打开会话文件 {}：{error}", path.display()))?;
    if path.extension().and_then(|value| value.to_str()) == Some("zst") {
        let decoder = zstd::stream::read::Decoder::new(file)
            .map_err(|error| format!("无法解压会话文件 {}：{error}", path.display()))?;
        Ok(Box::new(BufReader::new(decoder)))
    } else {
        Ok(Box::new(BufReader::new(file)))
    }
}

fn collect_rollout_paths(root: &Path, result: &mut Vec<PathBuf>) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)
        .map_err(|error| format!("无法读取会话目录 {}：{error}", root.display()))?
    {
        let entry = entry.map_err(|error| format!("无法读取会话目录项：{error}"))?;
        let path = entry.path();
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            collect_rollout_paths(&path, result)?;
        } else if let Some(logical) = logical_rollout_path(&path) {
            result.push(logical);
        }
    }
    Ok(())
}

fn first_rollout_value(path: &Path) -> Result<Option<Value>, String> {
    for line in rollout_reader(path)?.lines() {
        let line = line.map_err(|error| format!("无法读取会话文件 {}：{error}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            return Ok(None);
        };
        return Ok(
            (value.get("type").and_then(Value::as_str) == Some("session_meta")).then_some(value),
        );
    }
    Ok(None)
}

fn snapshot_id(meta: &Value) -> Option<String> {
    meta.pointer("/payload/id")
        .or_else(|| meta.pointer("/payload/session_id"))
        .or_else(|| meta.get("id"))
        .or_else(|| meta.get("session_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn snapshot_cwd(meta: &Value) -> Option<String> {
    meta.pointer("/payload/cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn snapshot_reference_id(meta: &Value, field: &str) -> Option<String> {
    meta.pointer(&format!("/payload/{field}/thread_id"))
        .or_else(|| meta.pointer(&format!("/payload/{field}/threadId")))
        .or_else(|| meta.pointer(&format!("/payload/{field}")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn gather_snapshots(codex_home: &Path) -> Result<Vec<RolloutSnapshot>, String> {
    let index = index_values(codex_home)?;
    let mut files = Vec::new();
    for folder in ROLLOUT_FOLDERS {
        collect_rollout_paths(&codex_home.join(folder), &mut files)?;
    }
    files.sort();
    files.dedup();
    let mut snapshots = Vec::new();
    for logical_path in files {
        let Some(path) = preferred_rollout_path(&logical_path) else {
            continue;
        };
        let physical_paths = rollout_physical_paths(&logical_path);
        let Some(meta) = first_rollout_value(&path)? else {
            continue;
        };
        let Some(session_id) = snapshot_id(&meta) else {
            continue;
        };
        let indexed = index.get(&session_id);
        let title = indexed
            .and_then(title_from_index)
            .unwrap_or_else(|| session_id.clone());
        let cwd = snapshot_cwd(&meta).unwrap_or_else(|| "未知工作目录".to_string());
        let updated_at = index_timestamp(indexed).or_else(|| modified_seconds(&path));
        let relative_path = path.strip_prefix(codex_home).unwrap_or(&path).to_path_buf();
        let index_value = indexed
            .cloned()
            .unwrap_or_else(|| json!({ "id": session_id, "thread_name": title }));
        let size_bytes = physical_paths
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .map(|item| item.len())
            .sum();
        let history_base_thread_id = snapshot_reference_id(&meta, "history_base");
        let parent_thread_id = snapshot_reference_id(&meta, "parent_thread_id")
            .or_else(|| snapshot_reference_id(&meta, "parent_thread"));
        snapshots.push(RolloutSnapshot {
            session_id,
            title,
            cwd,
            updated_at,
            started_at: meta
                .pointer("/payload/timestamp")
                .or_else(|| meta.get("timestamp"))
                .and_then(unix_seconds),
            path,
            physical_paths,
            relative_path,
            index_value,
            size_bytes,
            history_base_thread_id,
            parent_thread_id,
        });
    }
    Ok(snapshots)
}

fn thread_kind(title: &str, cwd: &str) -> String {
    let text = format!("{title} {cwd}").to_ascii_lowercase();
    if text.contains("subagent") || text.contains("sub-agent") || text.contains("agent run") {
        "subagent".to_string()
    } else if text.contains("external") || text.contains("imported") || text.contains("cli run") {
        "external".to_string()
    } else {
        "conversation".to_string()
    }
}

fn rollout_status(relative_path: &Path) -> Option<&'static str> {
    match relative_path.components().next() {
        Some(std::path::Component::Normal(value)) if value == "sessions" => Some("active"),
        Some(std::path::Component::Normal(value)) if value == "archived_sessions" => {
            Some("archived")
        }
        _ => None,
    }
}

fn clipped_match(value: &str, needle: &str) -> Option<String> {
    let lowered_needle = needle.to_lowercase();
    // Most transcript fields are not matches. Reject them before allocating a
    // whitespace-normalized copy (which is expensive for large tool outputs).
    // An ASCII source cannot contain a non-ASCII folded query component.
    let probe = lowered_needle
        .split_whitespace()
        .max_by_key(|part| part.len())?;
    if (value.is_ascii() && !probe.is_ascii()) || !value.to_lowercase().contains(probe) {
        return None;
    }
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let lowered = normalized.to_lowercase();
    let hit = lowered.find(&lowered_needle)?;
    // Lowercase can expand one character (e.g. İ -> i + combining dot).
    // Map the folded byte offset back to original characters before clipping.
    let mut lowered_offset = 0;
    let hit_chars = normalized
        .chars()
        .take_while(|character| {
            let end = lowered_offset + character.to_lowercase().map(char::len_utf8).sum::<usize>();
            if end > hit {
                return false;
            }
            lowered_offset = end;
            true
        })
        .count();
    let start = hit_chars.saturating_sub(48);
    let take = lowered_needle.chars().count().saturating_add(112);
    let total_chars = normalized.chars().count();
    let body = normalized
        .chars()
        .skip(start)
        .take(take)
        .collect::<String>();
    Some(format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        body,
        if start.saturating_add(take) < total_chars {
            "…"
        } else {
            ""
        }
    ))
}

fn matching_json_text(value: &Value, needle: &str) -> Option<String> {
    match value {
        Value::String(text) => clipped_match(text, needle),
        Value::Array(values) => values
            .iter()
            .find_map(|value| matching_json_text(value, needle)),
        Value::Object(values) => {
            for key in ["content", "text", "message", "input", "output"] {
                if let Some(found) = values
                    .get(key)
                    .and_then(|value| matching_json_text(value, needle))
                {
                    return Some(found);
                }
            }
            values
                .iter()
                .filter(|(key, _)| {
                    !["content", "text", "message", "input", "output"].contains(&key.as_str())
                })
                .find_map(|(_, value)| matching_json_text(value, needle))
        }
        _ => None,
    }
}

pub(crate) fn browse_codex_threads_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    title_query: Option<String>,
    content_query: Option<String>,
) -> Result<Vec<ThreadEntry>, String> {
    let mut job = prepare_thread_search(app, title_query, content_query)?;
    job.advance(&mut ThreadSearchBudget::new(std::sync::Arc::new(
        std::sync::atomic::AtomicBool::new(false),
    )))?;
    let result = job.response(None);
    if result
        .coverage
        .is_some_and(|coverage| coverage.incomplete_sessions > 0 || coverage.pending_sessions > 0)
    {
        return Err("部分会话未完成搜索。请使用会话页面查看搜索覆盖说明。".into());
    }
    Ok(result.entries)
}

fn prepare_thread_search<R: Runtime>(
    app: tauri::AppHandle<R>,
    title_query: Option<String>,
    content_query: Option<String>,
) -> Result<ThreadSearchJob, String> {
    if [&title_query, &content_query]
        .into_iter()
        .flatten()
        .any(|query| query.chars().count() > 256)
    {
        return Err("搜索词最多支持 256 个字符".into());
    }
    recover_incomplete_archive_transactions(&app)?;
    recover_incomplete_rebind_transactions(&app)?;
    recover_incomplete_visibility_repair_transactions(&app)?;
    let paths = resolve_paths(&app)?;
    let codex_home = paths.codex_home.clone();
    let snapshots = gather_snapshots(&codex_home)?;
    let state = sync_thread_ownership(&paths, &snapshots)?;
    let mut physical: HashMap<String, Vec<RolloutSnapshot>> = HashMap::new();
    for snapshot in &snapshots {
        physical
            .entry(snapshot.session_id.clone())
            .or_default()
            .push(snapshot.clone());
    }
    let mut snapshots = select_thread_read_snapshots(&codex_home, snapshots);
    snapshots.sort_by_key(|selection| std::cmp::Reverse(selection.snapshot.updated_at));
    let account_emails = account_email_by_id(&paths);
    let title_query = title_query
        .map(|value| value.trim().to_lowercase())
        .filter(|v| !v.is_empty());
    let content_query = content_query
        .map(|value| value.trim().to_string())
        .filter(|v| !v.is_empty());
    let mut candidates = Vec::new();
    for selection in snapshots {
        let snapshot = selection.snapshot;
        let title_matches = title_query
            .as_ref()
            .is_some_and(|query| thread_metadata_matches(&snapshot, query));
        let account_id = state
            .conversation_account_ids
            .get(&snapshot.session_id)
            .cloned();
        let account_email = account_id
            .as_deref()
            .and_then(|id| account_emails.get(id).cloned());
        let account_active = account_id
            .as_deref()
            .zip(state.active_account_id.as_deref())
            .is_some_and(|(thread_account, active_account)| thread_account == active_account);
        let entry = ThreadEntry {
            session_id: snapshot.session_id.clone(),
            session_kind: thread_kind(&snapshot.title, &snapshot.cwd),
            status: rollout_status(&snapshot.relative_path)
                .unwrap_or("active")
                .to_string(),
            title: snapshot.title.clone(),
            cwd: snapshot.cwd.clone(),
            updated_at: snapshot.updated_at,
            size_bytes: snapshot.size_bytes,
            rollout_count: selection.rollout_count,
            match_excerpt: None,
            match_timestamp: None,
            account_id,
            account_email,
            account_active,
        };
        candidates.push(ThreadSearchCandidate {
            physical: physical.remove(&snapshot.session_id).unwrap_or_default(),
            head: snapshot,
            entry,
            metadata_match: title_matches || (title_query.is_none() && content_query.is_none()),
        });
    }
    Ok(ThreadSearchJob::new(content_query, candidates))
}

fn token_totals(path: &Path) -> Option<(u64, u64, u64)> {
    let mut latest = None;
    for line in rollout_reader(path).ok()?.lines().map_while(Result::ok) {
        if !line.contains("\"token_count\"") || !line.contains("\"total_token_usage\"") {
            continue;
        }
        let value = serde_json::from_str::<Value>(&line).ok()?;
        let usage = value.pointer("/payload/info/total_token_usage")?;
        latest = Some((
            usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            usage
                .get("total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ));
    }
    latest
}

pub(crate) fn measure_codex_thread_tokens_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    session_ids: Vec<String>,
) -> Result<Vec<ThreadTokenTotals>, String> {
    let requested = normalized_ids(session_ids);
    let codex_home = resolve_paths(&app)?.codex_home;
    Ok(
        select_thread_read_snapshots(&codex_home, gather_snapshots(&codex_home)?)
            .into_iter()
            .map(|selection| selection.snapshot)
            .filter(|item| requested.contains(&item.session_id))
            .filter_map(|item| {
                let (input_tokens, output_tokens, total_tokens) = token_totals(&item.path)?;
                Some(ThreadTokenTotals {
                    session_id: item.session_id,
                    input_tokens,
                    output_tokens,
                    total_tokens,
                })
            })
            .collect(),
    )
}
