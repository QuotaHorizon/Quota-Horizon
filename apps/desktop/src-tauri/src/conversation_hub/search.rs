// On-demand local search, using the same explicit history links as the reader.
// No transcript index or search text is persisted. Bounds apply to decoded bytes
// too, so compressed files and oversized records cannot bypass the budget.
const MAX_THREAD_SEARCH_BYTES: u64 = 512 * 1024 * 1024;
const MAX_THREAD_SEARCH_SECONDS: u64 = 10;
const MAX_THREAD_SEARCH_CLIENTS: usize = 8;

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadSearchCoverage {
    total_sessions: usize,
    searched_sessions: usize,
    incomplete_sessions: usize,
    skipped_records: usize,
    completed_sessions: usize,
    pending_sessions: usize,
    decoded_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadSearchResult {
    entries: Vec<ThreadEntry>,
    coverage: Option<ThreadSearchCoverage>,
    continuation: Option<String>,
}

type SearchCancellation = std::sync::Arc<std::sync::atomic::AtomicBool>;

#[derive(Default)]
struct ThreadSearchClients {
    clients: HashMap<String, (std::time::Instant, u32, SearchCancellation)>,
}

impl ThreadSearchClients {
    fn begin(&mut self, client_id: &str, request_id: u32) -> Result<SearchCancellation, String> {
        if Uuid::parse_str(client_id).is_err() || client_id.len() != 36 || request_id == 0 {
            return Err("Invalid session search context".into());
        }
        if let Some((_, current_id, flag)) = self.clients.get(client_id) {
            if request_id <= *current_id {
                return Err("Session search was superseded".into());
            }
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.make_room(client_id);
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.clients.insert(
            client_id.into(),
            (std::time::Instant::now(), request_id, cancelled.clone()),
        );
        Ok(cancelled)
    }

    fn make_room(&mut self, client_id: &str) {
        if !self.clients.contains_key(client_id) && self.clients.len() >= MAX_THREAD_SEARCH_CLIENTS
        {
            let oldest = self
                .clients
                .iter()
                .min_by_key(|(_, (created, _, _))| *created)
                .map(|(id, _)| id.clone());
            if let Some((_, _, flag)) = oldest.and_then(|id| self.clients.remove(&id)) {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    fn cancel(&mut self, client_id: &str, request_id: u32) {
        if let Some((_, current_id, flag)) = self.clients.get(client_id) {
            if *current_id > request_id {
                return;
            }
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
            if *current_id == request_id {
                return;
            }
        }
        // IPC cancellation can arrive before its queued search command. Keep
        // that request's high-water mark so it cannot start obsolete work later.
        self.make_room(client_id);
        self.clients.insert(
            client_id.into(),
            (
                std::time::Instant::now(),
                request_id,
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            ),
        );
    }
}

fn thread_search_clients() -> &'static std::sync::Mutex<ThreadSearchClients> {
    static CLIENTS: std::sync::OnceLock<std::sync::Mutex<ThreadSearchClients>> =
        std::sync::OnceLock::new();
    CLIENTS.get_or_init(|| std::sync::Mutex::new(ThreadSearchClients::default()))
}

fn begin_thread_search(client_id: &str, request_id: u32) -> Result<SearchCancellation, String> {
    let cancelled = thread_search_clients()
        .lock()
        .map_err(|_| "Session search is unavailable".to_owned())?
        .begin(client_id, request_id)?;
    if let Ok(mut cache) = thread_search_cache().lock() {
        cache.prune();
    }
    Ok(cancelled)
}

pub(crate) fn cancel_codex_thread_search_blocking(
    client_id: &str,
    request_id: u32,
) -> Result<(), String> {
    if client_id.len() != 36 || Uuid::parse_str(client_id).is_err() || request_id == 0 {
        return Err("Invalid session search context".into());
    }
    thread_search_clients()
        .lock()
        .map_err(|_| "Session search is unavailable".to_owned())?
        .cancel(client_id, request_id);
    if let Ok(mut cache) = thread_search_cache().lock() {
        cache.prune();
    }
    Ok(())
}

struct ThreadSearchBudget {
    remaining_bytes: u64,
    deadline: std::time::Instant,
    cancelled: SearchCancellation,
}

impl ThreadSearchBudget {
    fn new(cancelled: SearchCancellation) -> Self {
        Self {
            remaining_bytes: MAX_THREAD_SEARCH_BYTES,
            deadline: std::time::Instant::now()
                + std::time::Duration::from_secs(MAX_THREAD_SEARCH_SECONDS),
            cancelled,
        }
    }

    fn check_cancelled(&self) -> Result<(), String> {
        if self.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            Err("Session search was superseded".into())
        } else {
            Ok(())
        }
    }

    fn exhausted(&self) -> bool {
        self.remaining_bytes == 0 || std::time::Instant::now() >= self.deadline
    }
}

#[derive(Default, Clone)]
struct ThreadTextMatch {
    excerpt: Option<String>,
    timestamp: Option<String>,
    content_checked: bool,
    incomplete: bool,
    skipped_records: usize,
}

enum SearchRecord {
    End,
    Record,
    Skipped,
    Incomplete,
    Paused,
}

#[derive(Default)]
struct SearchRecordBuffer {
    buffer: Vec<u8>,
    bytes: usize,
    oversized: bool,
}

impl SearchRecordBuffer {
    fn reset(&mut self) {
        self.buffer.clear();
        self.bytes = 0;
        self.oversized = false;
    }
}

#[cfg(test)]
fn read_search_record(
    reader: &mut dyn BufRead,
    buffer: &mut Vec<u8>,
    budget: &mut ThreadSearchBudget,
) -> Result<SearchRecord, String> {
    let mut state = SearchRecordBuffer {
        buffer: std::mem::take(buffer),
        ..Default::default()
    };
    state.reset();
    let result = read_search_record_continuable(reader, &mut state, budget);
    *buffer = state.buffer;
    result.map(|record| {
        if matches!(record, SearchRecord::Paused) {
            SearchRecord::Incomplete
        } else {
            record
        }
    })
}

fn read_search_record_continuable(
    reader: &mut dyn BufRead,
    state: &mut SearchRecordBuffer,
    budget: &mut ThreadSearchBudget,
) -> Result<SearchRecord, String> {
    loop {
        budget.check_cancelled()?;
        if budget.exhausted() {
            return Ok(SearchRecord::Paused);
        }
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(_) => return Ok(SearchRecord::Incomplete),
        };
        if chunk.is_empty() {
            return Ok(if state.bytes == 0 {
                SearchRecord::End
            } else {
                SearchRecord::Skipped
            });
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let count = newline
            .map_or(chunk.len(), |index| index + 1)
            .min(budget.remaining_bytes as usize);
        budget.remaining_bytes -= count as u64;
        state.bytes = state.bytes.saturating_add(count);
        if !state.oversized && state.bytes <= MAX_JSONL_LINE_BYTES {
            state.buffer.extend_from_slice(&chunk[..count]);
        } else {
            state.oversized = true;
            state.buffer.clear();
        }
        reader.consume(count);
        if newline.is_some_and(|index| index < count) {
            return Ok(if state.oversized {
                SearchRecord::Skipped
            } else {
                SearchRecord::Record
            });
        }
    }
}

#[cfg(test)]
fn search_thread_reader(
    reader: &mut dyn BufRead,
    needle: &str,
    budget: &mut ThreadSearchBudget,
) -> Result<ThreadTextMatch, String> {
    let mut result = ThreadTextMatch {
        content_checked: true,
        ..Default::default()
    };
    let mut buffer = Vec::new();
    loop {
        match read_search_record(reader, &mut buffer, budget)? {
            SearchRecord::End => break,
            SearchRecord::Incomplete | SearchRecord::Paused => {
                result.incomplete = true;
                break;
            }
            SearchRecord::Skipped => {
                result.skipped_records += 1;
                result.incomplete = true;
                continue;
            }
            SearchRecord::Record => {}
        }
        let value: Value = match serde_json::from_slice(&buffer) {
            Ok(value) => value,
            Err(_) => {
                result.skipped_records += 1;
                result.incomplete = true;
                continue;
            }
        };
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            continue;
        }
        if let Some(excerpt) = matching_json_text(&value, needle) {
            result.excerpt = Some(excerpt);
            result.timestamp = value
                .get("timestamp")
                .and_then(unix_seconds)
                .and_then(|time| DateTime::<Utc>::from_timestamp(time, 0))
                .map(|time| time.to_rfc3339_opts(SecondsFormat::Secs, true));
            break;
        }
    }
    Ok(result)
}

#[cfg(test)]
fn locate_rollout_text(path: &Path, needle: &str) -> Result<Option<String>, String> {
    let mut budget = ThreadSearchBudget::new(std::sync::Arc::new(
        std::sync::atomic::AtomicBool::new(false),
    ));
    Ok(search_thread_reader(rollout_reader(path)?.as_mut(), needle, &mut budget)?.excerpt)
}

#[cfg(test)]
fn search_logical_thread(
    head: &RolloutSnapshot,
    physical: &[RolloutSnapshot],
    needle: &str,
    budget: &mut ThreadSearchBudget,
) -> Result<ThreadTextMatch, String> {
    budget.check_cancelled()?;
    let mut cursor = match ThreadContentCursor::new(head, physical) {
        Ok(cursor) => cursor,
        Err(_) => {
            return Ok(ThreadTextMatch {
                incomplete: true,
                ..Default::default()
            })
        }
    };
    if !cursor.advance(needle, budget)? {
        cursor.result.incomplete = true;
    }
    Ok(cursor.result)
}

pub(crate) fn search_codex_threads_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    query: String,
    client_id: String,
    request_id: u32,
) -> Result<ThreadSearchResult, String> {
    let cancelled = begin_thread_search(&client_id, request_id)?;
    start_thread_search_blocking(app, query, client_id, request_id, cancelled)
}
