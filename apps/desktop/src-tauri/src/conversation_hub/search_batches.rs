// Short, resumable reads keep the page responsive without treating a time slice
// as the end of a search. Cursors, open readers and partial JSONL records remain
// process-local, bounded and tied to the original client/request cancellation.
const SEARCH_BATCH_BYTES: u64 = 32 * 1024 * 1024;
const SEARCH_BATCH_MILLIS: u64 = 400;
const SEARCH_CURSOR_TTL_SECONDS: u64 = 300;

struct ThreadSearchCandidate {
    head: RolloutSnapshot,
    physical: Vec<RolloutSnapshot>,
    entry: ThreadEntry,
    metadata_match: bool,
}

struct ThreadContentCursor {
    sources: Vec<ThreadReadSource>,
    version: String,
    next_source: usize,
    reader: Option<Box<dyn BufRead + Send>>,
    source_bytes: u64,
    record: SearchRecordBuffer,
    result: ThreadTextMatch,
}

impl ThreadContentCursor {
    fn new(head: &RolloutSnapshot, physical: &[RolloutSnapshot]) -> Result<Self, String> {
        let sources = logical_thread_sources_for_head(head, physical)?;
        let version = source_read_version(&sources)?;
        Ok(Self {
            next_source: sources.len(),
            sources,
            version,
            reader: None,
            source_bytes: 0,
            record: SearchRecordBuffer::default(),
            result: ThreadTextMatch::default(),
        })
    }

    fn finish(&mut self) -> bool {
        if source_read_version(&self.sources).ok().as_ref() != Some(&self.version) {
            self.result.incomplete = true;
        }
        true
    }

    fn advance(&mut self, query: &str, budget: &mut ThreadSearchBudget) -> Result<bool, String> {
        loop {
            budget.check_cancelled()?;
            if budget.exhausted() {
                return Ok(false);
            }
            if self.reader.is_none() {
                if self.next_source == 0 {
                    return Ok(self.finish());
                }
                self.next_source -= 1;
                self.reader = match logical_source_reader(&self.sources[self.next_source]) {
                    Ok((reader, _)) => Some(reader),
                    Err(_) => {
                        self.result.incomplete = true;
                        continue;
                    }
                };
                self.result.content_checked = true;
                self.source_bytes = 0;
            }
            let before = budget.remaining_bytes;
            let record = read_search_record_continuable(
                self.reader.as_mut().unwrap().as_mut(),
                &mut self.record,
                budget,
            )?;
            self.source_bytes += before - budget.remaining_bytes;
            if self.source_bytes > MAX_SESSION_BYTES {
                // The compressed reader exposes one extra byte to distinguish
                // actual EOF from its decoded-size guard, even at a newline.
                self.result.incomplete = true;
                self.reader = None;
                self.record.reset();
                continue;
            }
            match record {
                SearchRecord::Paused => return Ok(false),
                SearchRecord::End => self.reader = None,
                SearchRecord::Incomplete => {
                    self.result.incomplete = true;
                    self.reader = None;
                }
                SearchRecord::Skipped => {
                    self.result.incomplete = true;
                    self.result.skipped_records += 1;
                }
                SearchRecord::Record => {
                    match serde_json::from_slice::<Value>(&self.record.buffer) {
                        Ok(value)
                            if value.get("type").and_then(Value::as_str)
                                != Some("session_meta") =>
                        {
                            if let Some(excerpt) = matching_json_text(&value, query) {
                                self.result.excerpt = Some(excerpt);
                                self.result.timestamp = value
                                    .get("timestamp")
                                    .and_then(unix_seconds)
                                    .and_then(|time| DateTime::<Utc>::from_timestamp(time, 0))
                                    .map(|time| time.to_rfc3339_opts(SecondsFormat::Secs, true));
                                return Ok(self.finish());
                            }
                        }
                        Ok(_) => {}
                        Err(_) => {
                            self.result.incomplete = true;
                            self.result.skipped_records += 1;
                        }
                    }
                }
            }
            self.record.reset();
        }
    }
}

struct ThreadSearchJob {
    query: Option<String>,
    queue: std::collections::VecDeque<ThreadSearchCandidate>,
    current: Option<(ThreadEntry, ThreadContentCursor)>,
    entries: Vec<ThreadEntry>,
    coverage: ThreadSearchCoverage,
}

impl ThreadSearchJob {
    fn new(query: Option<String>, candidates: Vec<ThreadSearchCandidate>) -> Self {
        let mut job = Self {
            query,
            queue: Default::default(),
            current: None,
            entries: Vec::new(),
            coverage: ThreadSearchCoverage {
                total_sessions: candidates.len(),
                ..Default::default()
            },
        };
        for candidate in candidates {
            if candidate.metadata_match {
                job.entries.push(candidate.entry);
                job.coverage.completed_sessions += 1;
            } else if job.query.is_some() {
                job.queue.push_back(candidate);
            } else {
                job.coverage.completed_sessions += 1;
            }
        }
        job
    }

    fn done(&self) -> bool {
        self.current.is_none() && self.queue.is_empty()
    }

    fn advance(&mut self, budget: &mut ThreadSearchBudget) -> Result<(), String> {
        let before = budget.remaining_bytes;
        let result = self.advance_inner(budget);
        self.coverage.decoded_bytes += before - budget.remaining_bytes;
        result
    }

    fn advance_inner(&mut self, budget: &mut ThreadSearchBudget) -> Result<(), String> {
        while !self.done() {
            budget.check_cancelled()?;
            if budget.exhausted() {
                break;
            }
            if self.current.is_none() {
                let candidate = self.queue.pop_front().unwrap();
                match ThreadContentCursor::new(&candidate.head, &candidate.physical) {
                    Ok(cursor) => self.current = Some((candidate.entry, cursor)),
                    Err(_) => {
                        self.coverage.completed_sessions += 1;
                        self.coverage.incomplete_sessions += 1;
                        continue;
                    }
                }
            }
            let (_, cursor) = self.current.as_mut().unwrap();
            if !cursor.advance(self.query.as_deref().unwrap_or(""), budget)? {
                break;
            }
            let (mut entry, cursor) = self.current.take().unwrap();
            self.coverage.completed_sessions += 1;
            self.coverage.searched_sessions += usize::from(cursor.result.content_checked);
            self.coverage.incomplete_sessions += usize::from(cursor.result.incomplete);
            self.coverage.skipped_records += cursor.result.skipped_records;
            if cursor.result.excerpt.is_some() {
                entry.match_excerpt = cursor.result.excerpt;
                entry.match_timestamp = cursor.result.timestamp;
                self.entries.push(entry);
            }
        }
        budget.check_cancelled()
    }

    fn response(&self, continuation: Option<String>) -> ThreadSearchResult {
        let mut entries = self.entries.clone();
        entries.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.cwd.cmp(&right.cwd))
                .then_with(|| left.title.cmp(&right.title))
        });
        let mut coverage = self.coverage.clone();
        coverage.pending_sessions = coverage.total_sessions - coverage.completed_sessions;
        if let Some((_, current)) = &self.current {
            coverage.searched_sessions += usize::from(current.result.content_checked);
            coverage.incomplete_sessions += usize::from(current.result.incomplete);
            coverage.skipped_records += current.result.skipped_records;
        }
        ThreadSearchResult {
            entries,
            coverage: self.query.as_ref().map(|_| coverage),
            continuation,
        }
    }
}

struct CachedThreadSearch {
    client_id: String,
    request_id: u32,
    created: std::time::Instant,
    cancelled: SearchCancellation,
    job: ThreadSearchJob,
}

#[derive(Default)]
struct ThreadSearchCache {
    jobs: HashMap<String, CachedThreadSearch>,
}

impl ThreadSearchCache {
    fn prune(&mut self) {
        self.jobs.retain(|_, item| {
            !item.cancelled.load(std::sync::atomic::Ordering::Relaxed)
                && item.created.elapsed().as_secs() < SEARCH_CURSOR_TTL_SECONDS
        });
    }

    fn put(&mut self, item: CachedThreadSearch) -> Result<String, String> {
        self.prune();
        if item.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Session search was superseded".into());
        }
        if self.jobs.len() >= MAX_THREAD_SEARCH_CLIENTS {
            let oldest = self
                .jobs
                .iter()
                .min_by_key(|(_, item)| item.created)
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                self.jobs.remove(&id);
            }
        }
        let token = Uuid::new_v4().to_string();
        self.jobs.insert(token.clone(), item);
        Ok(token)
    }

    fn take(
        &mut self,
        client_id: &str,
        request_id: u32,
        token: &str,
    ) -> Result<CachedThreadSearch, String> {
        self.prune();
        if !self
            .jobs
            .get(token)
            .is_some_and(|item| item.client_id == client_id && item.request_id == request_id)
        {
            return Err("搜索进度已过期或不可用，请重新搜索。".into());
        }
        Ok(self.jobs.remove(token).unwrap())
    }
}

fn thread_search_cache() -> &'static std::sync::Mutex<ThreadSearchCache> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<ThreadSearchCache>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(ThreadSearchCache::default()))
}

fn run_thread_search_batch(mut item: CachedThreadSearch) -> Result<ThreadSearchResult, String> {
    let mut budget = ThreadSearchBudget::new(item.cancelled.clone());
    budget.remaining_bytes = SEARCH_BATCH_BYTES;
    budget.deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(SEARCH_BATCH_MILLIS);
    item.job.advance(&mut budget)?;
    let mut result = item.job.response(None);
    if !item.job.done() {
        item.created = std::time::Instant::now();
        result.continuation = Some(
            thread_search_cache()
                .lock()
                .map_err(|_| "会话搜索暂不可用".to_owned())?
                .put(item)?,
        );
    }
    Ok(result)
}

fn start_thread_search_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    query: String,
    client_id: String,
    request_id: u32,
    cancelled: SearchCancellation,
) -> Result<ThreadSearchResult, String> {
    ThreadSearchBudget::new(cancelled.clone()).check_cancelled()?;
    let job = prepare_thread_search(app, Some(query.clone()), Some(query))?;
    run_thread_search_batch(CachedThreadSearch {
        client_id,
        request_id,
        cancelled,
        job,
        created: std::time::Instant::now(),
    })
}

pub(crate) fn continue_codex_thread_search_blocking(
    client_id: String,
    request_id: u32,
    continuation: String,
) -> Result<ThreadSearchResult, String> {
    let item = thread_search_cache()
        .lock()
        .map_err(|_| "会话搜索暂不可用".to_owned())?
        .take(&client_id, request_id, &continuation)?;
    run_thread_search_batch(item)
}
