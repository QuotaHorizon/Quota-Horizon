const DEFAULT_TIMELINE_PAGE_SIZE: usize = 100;
const MAX_TIMELINE_PAGE_SIZE: usize = 500;
const MAX_SESSION_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SESSION_LINES: usize = 1_000_000;
const MAX_TIMELINE_ITEMS: usize = 100_000;
const MAX_JSONL_LINE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TIMELINE_TEXT_CHARS: usize = 64 * 1024;
const EXCERPT_CHARS: usize = 180;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadDetailSummary {
    session_id: String,
    title: String,
    cwd: String,
    started_at: Option<String>,
    updated_at: Option<String>,
    originator: String,
    source: String,
    cli_version: String,
    model_provider: String,
    size_bytes: u64,
    segment_count: usize,
    skipped_record_count: usize,
    line_count: usize,
    event_count: usize,
    tool_call_count: usize,
    user_prompt_excerpt: String,
    latest_agent_message_excerpt: String,
    resume_command: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadTimelineItem {
    id: String,
    kind: String,
    timestamp: Option<String>,
    text: Option<String>,
    tool_name: Option<String>,
    tool_input: Option<String>,
    tool_output: Option<String>,
    tool_status: Option<String>,
    truncated: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadDetailPage {
    summary: ThreadDetailSummary,
    items: Vec<ThreadTimelineItem>,
    total: usize,
    next_offset: Option<usize>,
    previous_offset: Option<usize>,
    offset: usize,
    revision: String,
}

#[derive(Debug, Clone)]
struct TimelineDraft {
    item: ThreadTimelineItem,
    order: usize,
}

#[derive(Debug)]
struct ParsedThreadDetail {
    summary: ThreadDetailSummary,
    timeline: Vec<ThreadTimelineItem>,
    revision: String,
}

fn string_value(value: Option<&Value>, fallback: &str) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(value @ (Value::Array(_) | Value::Object(_))) => {
            serde_json::to_string(value).unwrap_or_else(|_| fallback.to_string())
        }
        _ => fallback.to_string(),
    }
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    let value = string_value(value, "");
    (!value.is_empty()).then_some(value)
}

fn normalized_timeline_text(value: &str) -> String {
    value.trim().to_string()
}

fn bounded_timeline_text(value: String) -> (String, bool) {
    let normalized = normalized_timeline_text(&value);
    if normalized.chars().count() <= MAX_TIMELINE_TEXT_CHARS {
        return (normalized, false);
    }
    let mut clipped = normalized
        .chars()
        .take(MAX_TIMELINE_TEXT_CHARS.saturating_sub(1))
        .collect::<String>();
    clipped.push('…');
    (clipped, true)
}

fn excerpt(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= EXCERPT_CHARS {
        return normalized;
    }
    let mut clipped = normalized
        .chars()
        .take(EXCERPT_CHARS.saturating_sub(1))
        .collect::<String>();
    clipped.push('…');
    clipped
}

fn response_message_text(payload: &Value) -> String {
    payload
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| {
            part.get("text")
                .or_else(|| part.get("content"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn normalized_source(value: Option<&Value>) -> String {
    if let Some(role) = value
        .and_then(|source| source.pointer("/subagent/thread_spawn/agent_role"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return format!("subagent:{role}");
    }
    string_value(value, "unknown")
}

fn tool_status(output: &str) -> String {
    let lowered = output.to_ascii_lowercase();
    if ["error", "failed", "exception"]
        .iter()
        .any(|marker| lowered.contains(marker))
    {
        "errored".to_string()
    } else {
        "completed".to_string()
    }
}

fn timestamp_sort_key(value: Option<&str>) -> Option<i64> {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis())
}

fn bounded_read_line(reader: &mut dyn BufRead, buffer: &mut String) -> Result<usize, String> {
    buffer.clear();
    let mut limited = reader.take((MAX_JSONL_LINE_BYTES + 1) as u64);
    let read = limited
        .read_line(buffer)
        .map_err(|error| format!("无法读取会话时间线：{error}"))?;
    if read > MAX_JSONL_LINE_BYTES {
        return Err(format!(
            "会话 JSONL 单行超过安全上限（{} bytes）",
            MAX_JSONL_LINE_BYTES
        ));
    }
    Ok(read)
}

fn push_timeline_item(
    timeline: &mut Vec<TimelineDraft>,
    item: ThreadTimelineItem,
    order: &mut usize,
) -> Result<(), String> {
    if timeline.len() >= MAX_TIMELINE_ITEMS {
        return Err(format!(
            "会话时间线超过安全上限（{MAX_TIMELINE_ITEMS} items）"
        ));
    }
    timeline.push(TimelineDraft {
        item,
        order: *order,
    });
    *order += 1;
    Ok(())
}

#[cfg(test)]
fn parse_thread_detail(snapshot: &RolloutSnapshot) -> Result<ParsedThreadDetail, String> {
    let metadata =
        fs::metadata(&snapshot.path).map_err(|error| format!("无法读取会话文件属性：{error}"))?;
    if metadata.len() > MAX_SESSION_BYTES {
        return Err(format!(
            "会话文件超过安全上限（{} MiB）",
            MAX_SESSION_BYTES / 1024 / 1024
        ));
    }

    parse_thread_detail_reader(snapshot, rollout_reader(&snapshot.path)?)
}

fn parse_thread_detail_reader(
    snapshot: &RolloutSnapshot,
    mut reader: Box<dyn BufRead>,
) -> Result<ParsedThreadDetail, String> {
    let mut buffer = Vec::new();
    let mut parsed_bytes = 0_u64;
    let mut line_count = 0_usize;
    let mut event_count = 0_usize;
    let mut tool_call_count = 0_usize;
    let mut skipped_record_count = 0_usize;
    let mut meta: Option<Value> = None;
    let mut timeline = Vec::<TimelineDraft>::new();
    let mut tool_call_index = HashMap::<String, usize>::new();
    let mut order = 0_usize;
    let mut user_prompt_excerpt = String::new();
    let mut latest_agent_message_excerpt = String::new();
    let mut latest_timestamp: Option<String> = None;
    let mut content_digest = Sha256::new();

    loop {
        let (read, oversized) = read_timeline_record(
            reader.as_mut(),
            &mut buffer,
            &mut content_digest,
            MAX_SESSION_BYTES - parsed_bytes,
        )?;
        if read == 0 {
            break;
        }
        parsed_bytes = parsed_bytes.saturating_add(read as u64);
        if parsed_bytes > MAX_SESSION_BYTES {
            return Err(format!(
                "解压后的会话文件超过安全上限（{} MiB）",
                MAX_SESSION_BYTES / 1024 / 1024
            ));
        }
        if !oversized && buffer.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        line_count += 1;
        if line_count > MAX_SESSION_LINES {
            return Err(format!("会话文件超过安全上限（{MAX_SESSION_LINES} lines）"));
        }
        if oversized {
            skipped_record_count += 1;
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Value>(&buffer) else {
            skipped_record_count += 1;
            continue;
        };
        let entry_type = entry.get("type").and_then(Value::as_str);
        let timestamp = optional_string(entry.get("timestamp"));
        if timestamp_sort_key(timestamp.as_deref())
            > timestamp_sort_key(latest_timestamp.as_deref())
        {
            latest_timestamp = timestamp.clone();
        }
        if entry_type == Some("session_meta") && meta.is_none() {
            meta = Some(entry.clone());
            continue;
        }
        if entry_type == Some("event_msg") {
            event_count += 1;
            let payload = entry.get("payload").unwrap_or(&Value::Null);
            let message_type = payload.get("type").and_then(Value::as_str);
            let message = payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let kind = match message_type {
                Some("user_message") => Some("message:user"),
                Some("agent_message") => Some("message:assistant"),
                _ => None,
            };
            if let Some(kind) = kind.filter(|_| !message.is_empty()) {
                if kind == "message:user" && user_prompt_excerpt.is_empty() {
                    user_prompt_excerpt = excerpt(message);
                } else if kind == "message:assistant" {
                    latest_agent_message_excerpt = excerpt(message);
                }
                let (text, truncated) = bounded_timeline_text(message.to_string());
                push_timeline_item(
                    &mut timeline,
                    ThreadTimelineItem {
                        id: format!("event-{}", order + 1),
                        kind: kind.to_string(),
                        timestamp,
                        text: Some(text),
                        tool_name: None,
                        tool_input: None,
                        tool_output: None,
                        tool_status: None,
                        truncated,
                    },
                    &mut order,
                )?;
            }
            continue;
        }
        if entry_type != Some("response_item") {
            continue;
        }
        let payload = entry.get("payload").unwrap_or(&Value::Null);
        match payload.get("type").and_then(Value::as_str) {
            Some("message") => {
                let role = payload.get("role").and_then(Value::as_str);
                let kind = match role {
                    Some("user") => Some("message:user"),
                    Some("assistant") => Some("message:assistant"),
                    _ => None,
                };
                let message = response_message_text(payload);
                if let Some(kind) = kind.filter(|_| !message.is_empty()) {
                    if kind == "message:user" && user_prompt_excerpt.is_empty() {
                        user_prompt_excerpt = excerpt(&message);
                    } else if kind == "message:assistant" {
                        latest_agent_message_excerpt = excerpt(&message);
                    }
                    let (text, truncated) = bounded_timeline_text(message);
                    push_timeline_item(
                        &mut timeline,
                        ThreadTimelineItem {
                            id: format!("message-{}", order + 1),
                            kind: kind.to_string(),
                            timestamp,
                            text: Some(text),
                            tool_name: None,
                            tool_input: None,
                            tool_output: None,
                            tool_status: None,
                            truncated,
                        },
                        &mut order,
                    )?;
                }
            }
            Some("function_call") => {
                tool_call_count += 1;
                let call_id = string_value(payload.get("call_id"), &format!("tool-{}", order + 1));
                let tool_name = string_value(payload.get("name"), "unknown_tool");
                let input = string_value(payload.get("arguments"), "");
                let (input, truncated) = bounded_timeline_text(input);
                let index = timeline.len();
                push_timeline_item(
                    &mut timeline,
                    ThreadTimelineItem {
                        id: format!("tool-{}", order + 1),
                        kind: "tool_call".to_string(),
                        timestamp,
                        text: None,
                        tool_name: Some(tool_name),
                        tool_input: Some(input),
                        tool_output: Some(String::new()),
                        tool_status: Some("pending".to_string()),
                        truncated,
                    },
                    &mut order,
                )?;
                tool_call_index.insert(call_id, index);
            }
            Some("function_call_output") => {
                let call_id = string_value(payload.get("call_id"), "");
                let Some(index) = tool_call_index.get(&call_id).copied() else {
                    continue;
                };
                let output = string_value(payload.get("output"), "");
                let status = tool_status(&output);
                let (output, truncated) = bounded_timeline_text(output);
                if let Some(existing) = timeline.get_mut(index) {
                    existing.item.tool_output = Some(output);
                    existing.item.tool_status = Some(status);
                    existing.item.truncated |= truncated;
                }
            }
            _ => {}
        }
    }

    timeline.sort_by(|left, right| {
        match (
            timestamp_sort_key(left.item.timestamp.as_deref()),
            timestamp_sort_key(right.item.timestamp.as_deref()),
        ) {
            (Some(left_time), Some(right_time)) if left_time != right_time => {
                left_time.cmp(&right_time)
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            _ => left.order.cmp(&right.order),
        }
    });
    let mut seen = HashSet::new();
    timeline.retain(|draft| {
        let item = &draft.item;
        let key = format!(
            "{}::{:?}::{:?}::{:?}::{:?}::{:?}",
            item.kind, item.timestamp, item.text, item.tool_name, item.tool_input, item.tool_output
        );
        seen.insert(key)
    });
    let timeline = timeline
        .into_iter()
        .map(|draft| draft.item)
        .collect::<Vec<_>>();

    let meta_entry = meta.as_ref();
    let meta_payload = meta_entry.and_then(|value| value.get("payload"));
    let started_at = meta_payload
        .and_then(|payload| optional_string(payload.get("timestamp")))
        .or_else(|| meta_entry.and_then(|entry| optional_string(entry.get("timestamp"))));
    let updated_at = latest_timestamp.or_else(|| {
        snapshot.updated_at.and_then(|timestamp| {
            DateTime::<Utc>::from_timestamp(timestamp, 0)
                .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
        })
    });
    let summary = ThreadDetailSummary {
        session_id: snapshot.session_id.clone(),
        title: snapshot.title.clone(),
        cwd: snapshot.cwd.clone(),
        started_at,
        updated_at,
        originator: string_value(
            meta_payload.and_then(|value| value.get("originator")),
            "unknown",
        ),
        source: normalized_source(meta_payload.and_then(|value| value.get("source"))),
        cli_version: string_value(
            meta_payload
                .and_then(|value| value.get("cli_version").or_else(|| value.get("cliVersion"))),
            "unknown",
        ),
        model_provider: string_value(
            meta_payload.and_then(|value| {
                value
                    .get("model_provider")
                    .or_else(|| value.get("modelProvider"))
            }),
            "unknown",
        ),
        size_bytes: snapshot.size_bytes,
        segment_count: 1,
        skipped_record_count,
        line_count,
        event_count,
        tool_call_count,
        user_prompt_excerpt,
        latest_agent_message_excerpt,
        resume_command: safe_resume_command(&snapshot.session_id),
    };
    Ok(ParsedThreadDetail {
        summary,
        timeline,
        revision: format!("{:x}", content_digest.finalize()),
    })
}

fn safe_resume_command(session_id: &str) -> Option<String> {
    let valid = !session_id.is_empty()
        && session_id.len() <= 128
        && session_id
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_'));
    valid.then(|| format!("codex resume {session_id}"))
}

fn timeline_page_size(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(DEFAULT_TIMELINE_PAGE_SIZE)
        .clamp(1, MAX_TIMELINE_PAGE_SIZE)
}

fn page_thread_detail(
    parsed: &ParsedThreadDetail,
    offset: Option<usize>,
    limit: Option<usize>,
    expected_revision: Option<String>,
) -> Result<ThreadDetailPage, String> {
    if expected_revision
        .as_deref()
        .is_some_and(|expected| expected != parsed.revision)
    {
        return Err("session_revision_changed".to_string());
    }
    let offset = offset.unwrap_or(0).min(parsed.timeline.len());
    let limit = timeline_page_size(limit);
    let end = offset.saturating_add(limit).min(parsed.timeline.len());
    let total = parsed.timeline.len();
    Ok(ThreadDetailPage {
        summary: parsed.summary.clone(),
        items: parsed.timeline[offset..end].to_vec(),
        total,
        next_offset: (end < total).then_some(end),
        previous_offset: (offset > 0).then_some(offset.saturating_sub(limit)),
        offset,
        revision: parsed.revision.clone(),
    })
}

pub(crate) fn inspect_codex_thread_detail_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    offset: Option<usize>,
    limit: Option<usize>,
    expected_revision: Option<String>,
    from_end: Option<bool>,
) -> Result<ThreadDetailPage, String> {
    recover_incomplete_rebind_transactions(&app)?;
    recover_incomplete_visibility_repair_transactions(&app)?;
    let session_id = session_id.trim();
    if session_id.is_empty() || session_id.len() > 256 || session_id.chars().any(char::is_control) {
        return Err("会话 ID 无效".to_string());
    }
    let codex_home = resolve_paths(&app)?.codex_home;
    let parsed = read_logical_thread_detail(&codex_home, session_id, expected_revision.as_deref())?;
    let offset = if from_end == Some(true) {
        Some(
            parsed
                .timeline
                .len()
                .saturating_sub(timeline_page_size(limit)),
        )
    } else {
        offset
    };
    page_thread_detail(&parsed, offset, limit, expected_revision)
}
