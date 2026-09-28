#[cfg(test)]
mod search_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    const ROOT_ID: &str = "00000000-0000-4000-8000-000000000011";
    const CHILD_ID: &str = "00000000-0000-4000-8000-000000000012";
    const OTHER_ID: &str = "00000000-0000-4000-8000-000000000013";

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("horizon-search-{}", Uuid::new_v4())))
        }

        fn segment(&self, suffix: &str, day: u8, base: Option<(&str, u64)>, body: &str) -> PathBuf {
            let path = self
                .0
                .join("sessions/2026/09/12")
                .join(format!("rollout-{ROOT_ID}{suffix}.jsonl"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let timestamp = format!("2026-09-{day:02}T00:00:00Z");
            let mut meta = json!({"type":"session_meta","timestamp":timestamp,"payload":{
                "id":ROOT_ID,"cwd":"/tmp/search-fixture","timestamp":timestamp,"history_mode":"paginated"}});
            if let Some((id, end)) = base {
                meta["payload"]["history_base"] = json!({"thread_id":id,"end_byte_offset":end});
            }
            fs::write(&path, format!("{meta}\n{body}")).unwrap();
            path
        }

        fn search(&self, query: &str) -> ThreadTextMatch {
            let physical = gather_snapshots(&self.0).unwrap();
            let head = select_thread_read_snapshots(&self.0, physical.clone())
                .remove(0)
                .snapshot;
            search_logical_thread(&head, &physical, query, &mut budget()).unwrap()
        }

        fn job(&self, query: &str) -> ThreadSearchJob {
            search_job_for_root(&self.0, query)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if self.0.exists() {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
    }

    fn message(text: &str) -> String {
        format!(
            "{}\n",
            json!({"type":"event_msg","timestamp":"2026-09-07T01:02:03Z",
            "payload":{"type":"agent_message","message":text}})
        )
    }

    fn budget() -> ThreadSearchBudget {
        ThreadSearchBudget::new(Arc::new(AtomicBool::new(false)))
    }

    fn search_job_for_root(root: &Path, query: &str) -> ThreadSearchJob {
        let physical = gather_snapshots(root).unwrap();
        let mut selections = select_thread_read_snapshots(root, physical.clone());
        selections.sort_by_key(|head| std::cmp::Reverse(head.snapshot.updated_at));
        let candidates = selections
            .into_iter()
            .map(|selection| {
                let head = selection.snapshot;
                let entry = ThreadEntry {
                    session_id: head.session_id.clone(),
                    session_kind: "conversation".into(),
                    title: head.title.clone(),
                    cwd: head.cwd.clone(),
                    status: "active".into(),
                    updated_at: head.updated_at,
                    size_bytes: head.size_bytes,
                    rollout_count: selection.rollout_count,
                    match_excerpt: None,
                    match_timestamp: None,
                    account_id: None,
                    account_email: None,
                    account_active: false,
                };
                ThreadSearchCandidate {
                    physical: physical
                        .iter()
                        .filter(|item| item.session_id == head.session_id)
                        .cloned()
                        .collect(),
                    head,
                    entry,
                    metadata_match: false,
                }
            })
            .collect();
        ThreadSearchJob::new(Some(query.into()), candidates)
    }

    fn finish_batches(job: &mut ThreadSearchJob, bytes_per_batch: u64) -> usize {
        let mut count = 0;
        let mut previous_bytes = 0;
        while !job.done() {
            let mut limited = budget();
            limited.remaining_bytes = bytes_per_batch;
            job.advance(&mut limited).unwrap();
            assert!(job.coverage.decoded_bytes >= previous_bytes);
            previous_bytes = job.coverage.decoded_bytes;
            count += 1;
            assert!(count < 10000, "Continuation did not advance");
        }
        count
    }

    #[test]
    fn batches_resume_mid_utf8_record_and_linked_segment_without_false_omissions() {
        let fixture = Fixture::new();
        let parent = fixture.segment("", 1, None, &message("较早的热力图反馈"));
        let cutoff = fs::metadata(&parent).unwrap().len();
        let child = fixture.segment(
            &format!("_{CHILD_ID}"),
            12,
            Some((ROOT_ID, cutoff)),
            &message("latest"),
        );
        let total = cutoff + fs::metadata(child).unwrap().len();
        let mut job = fixture.job("热力图");
        assert!(finish_batches(&mut job, 7) > 10);
        let response = job.response(None);
        let coverage = response.coverage.unwrap();
        assert_eq!(coverage.pending_sessions, 0);
        assert_eq!(coverage.completed_sessions, 1);
        assert_eq!(coverage.incomplete_sessions, 0);
        assert_eq!(coverage.decoded_bytes, total);
        assert_eq!(response.entries.len(), 1);
        assert_eq!(
            response.entries[0].match_excerpt.as_deref(),
            Some("较早的热力图反馈")
        );
    }

    #[test]
    fn oversized_record_is_counted_once_across_many_batches_and_later_match_survives() {
        let fixture = Fixture::new();
        let body = format!(
            "{}\n{}",
            "x".repeat(MAX_JSONL_LINE_BYTES + 100000),
            message("late needle")
        );
        let path = fixture.segment("", 12, None, &body);
        let mut job = fixture.job("needle");
        assert!(finish_batches(&mut job, 64000) > 20);
        let result = job.response(None);
        assert_eq!(result.entries.len(), 1);
        let coverage = result.coverage.unwrap();
        assert_eq!(coverage.skipped_records, 1);
        assert_eq!(coverage.incomplete_sessions, 1);
        assert_eq!(coverage.decoded_bytes, fs::metadata(path).unwrap().len());
    }

    #[test]
    fn compressed_decoder_keeps_its_position_between_batches() {
        let fixture = Fixture::new();
        let path = fixture.segment(
            "",
            12,
            None,
            &format!("{}{}", message(&"x".repeat(200000)), message("late needle")),
        );
        let decoded_size = fs::metadata(&path).unwrap().len();
        fs::write(
            compressed_rollout_path(&path),
            zstd::stream::encode_all(File::open(&path).unwrap(), 1).unwrap(),
        )
        .unwrap();
        fs::remove_file(path).unwrap();
        let mut job = fixture.job("needle");
        assert!(finish_batches(&mut job, 8192) > 20);
        assert_eq!(job.coverage.decoded_bytes, decoded_size);
        assert_eq!(job.coverage.incomplete_sessions, 0);
        assert_eq!(job.entries.len(), 1);
    }

    #[test]
    fn append_between_batches_does_not_turn_a_frozen_negative_into_complete_coverage() {
        let fixture = Fixture::new();
        let path = fixture.segment("", 12, None, &message("first"));
        let mut job = fixture.job("needle");
        let mut limited = budget();
        limited.remaining_bytes = 8;
        job.advance(&mut limited).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(message("new needle").as_bytes())
            .unwrap();
        finish_batches(&mut job, 32);
        assert_eq!(job.coverage.incomplete_sessions, 1);
        assert!(job.entries.is_empty());
    }

    fn cached_empty(client: &str, request: u32, flag: SearchCancellation) -> CachedThreadSearch {
        CachedThreadSearch {
            client_id: client.into(),
            request_id: request,
            cancelled: flag,
            created: std::time::Instant::now(),
            job: ThreadSearchJob::new(None, Vec::new()),
        }
    }

    #[test]
    fn continuation_tokens_are_one_use_bound_to_client_request_and_expire() {
        let mut cache = ThreadSearchCache::default();
        let token = cache
            .put(cached_empty(ROOT_ID, 2, budget().cancelled))
            .unwrap();
        assert!(cache.take(OTHER_ID, 2, &token).is_err());
        assert!(cache.take(ROOT_ID, 3, &token).is_err());
        assert!(cache.take(ROOT_ID, 2, &token).is_ok());
        assert!(cache.take(ROOT_ID, 2, &token).is_err());
        let mut expired = cached_empty(ROOT_ID, 4, budget().cancelled);
        expired.created = std::time::Instant::now()
            - std::time::Duration::from_secs(SEARCH_CURSOR_TTL_SECONDS + 1);
        let expired_token = cache.put(expired).unwrap();
        assert!(cache.take(ROOT_ID, 4, &expired_token).is_err());
    }

    #[test]
    fn cancelled_continuations_release_the_job_and_the_cache_remains_bounded() {
        let mut cache = ThreadSearchCache::default();
        let flag = budget().cancelled;
        let token = cache.put(cached_empty(ROOT_ID, 1, flag.clone())).unwrap();
        flag.store(true, Ordering::Relaxed);
        assert!(cache.take(ROOT_ID, 1, &token).is_err());
        assert!(cache.jobs.is_empty());
        for request in 1..=MAX_THREAD_SEARCH_CLIENTS + 2 {
            cache
                .put(cached_empty(ROOT_ID, request as u32, budget().cancelled))
                .unwrap();
        }
        assert_eq!(cache.jobs.len(), MAX_THREAD_SEARCH_CLIENTS);
    }

    #[test]
    fn deadline_pause_preserves_pending_work_and_metadata_hits_do_not_scan_content() {
        let fixture = Fixture::new();
        fixture.segment("", 12, None, &message("needle"));
        let mut job = fixture.job("needle");
        let mut expired = budget();
        expired.deadline = std::time::Instant::now();
        job.advance(&mut expired).unwrap();
        assert_eq!(job.response(None).coverage.unwrap().pending_sessions, 1);
        assert_eq!(job.coverage.incomplete_sessions, 0);
        let mut candidate = job.queue.pop_front().unwrap();
        candidate.metadata_match = true;
        let metadata_job = ThreadSearchJob::new(Some("needle".into()), vec![candidate]);
        assert!(metadata_job.done());
        assert_eq!(metadata_job.entries.len(), 1);
        assert_eq!(metadata_job.coverage.decoded_bytes, 0);
    }

    #[test]
    fn decoded_source_limit_is_incomplete_even_when_the_guard_byte_ends_at_a_newline() {
        let fixture = Fixture::new();
        fixture.segment("", 12, None, &message("fixture"));
        let physical = gather_snapshots(&fixture.0).unwrap();
        let mut cursor = ThreadContentCursor::new(&physical[0], &physical).unwrap();
        cursor.reader = Some(Box::new(std::io::Cursor::new(message("guarded hit"))));
        cursor.next_source = 0;
        cursor.source_bytes = MAX_SESSION_BYTES;
        assert!(cursor.advance("guarded hit", &mut budget()).unwrap());
        assert!(cursor.result.incomplete);
        assert!(cursor.result.excerpt.is_none());
    }

    #[test]
    #[ignore = "Requires explicit HORIZON_READONLY_HISTORY_ROOT and HORIZON_READONLY_SEARCH_QUERY"]
    fn readonly_progressive_local_search_smoke() {
        let root =
            PathBuf::from(std::env::var("HORIZON_READONLY_HISTORY_ROOT").expect("explicit root"));
        let query = std::env::var("HORIZON_READONLY_SEARCH_QUERY").expect("explicit query");
        assert!(!query.trim().is_empty() && query.chars().count() <= 256);
        let started = std::time::Instant::now();
        let mut job = search_job_for_root(&root, &query);
        let mut batches = 0;
        while !job.done() {
            let mut limited = budget();
            limited.remaining_bytes = SEARCH_BATCH_BYTES;
            limited.deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(SEARCH_BATCH_MILLIS);
            job.advance(&mut limited).unwrap();
            batches += 1;
            if batches % 25 == 0 {
                eprintln!(
                    "Read-only progress: checked={}/{}, decoded_bytes={}, batches={}",
                    job.coverage.completed_sessions,
                    job.coverage.total_sessions,
                    job.coverage.decoded_bytes,
                    batches
                );
            }
            assert!(
                started.elapsed().as_secs() < 120,
                "Local search exceeded smoke-check time bound"
            );
        }
        eprintln!("Read-only progressive search: sessions={}, hits={}, incomplete={}, skipped_records={}, decoded_bytes={}, batches={}, elapsed_ms={}",
            job.coverage.total_sessions, job.entries.len(), job.coverage.incomplete_sessions, job.coverage.skipped_records,
            job.coverage.decoded_bytes, batches, started.elapsed().as_millis());
        assert!(!job.entries.is_empty());
        assert_eq!(job.coverage.completed_sessions, job.coverage.total_sessions);
    }

    #[test]
    fn search_finds_older_linked_content_and_its_original_time_without_modifying_sources() {
        let fixture = Fixture::new();
        let parent = fixture.segment("", 1, None, &message("较早的热力图反馈"));
        let cutoff = fs::metadata(&parent).unwrap().len();
        let child = fixture.segment(
            &format!("_{CHILD_ID}"),
            12,
            Some((ROOT_ID, cutoff)),
            &message("latest unrelated message"),
        );
        let before = [fs::read(&parent).unwrap(), fs::read(&child).unwrap()];
        let found = fixture.search("热力图");
        assert_eq!(found.excerpt.as_deref(), Some("较早的热力图反馈"));
        assert_eq!(found.timestamp.as_deref(), Some("2026-09-07T01:02:03Z"));
        assert!(!found.incomplete);
        assert_eq!(
            before,
            [fs::read(&parent).unwrap(), fs::read(&child).unwrap()]
        );
        assert!(!fixture.0.join("state_5.sqlite").exists());
    }

    #[test]
    fn search_obeys_linked_prefix_and_does_not_merge_unrelated_same_id_copies() {
        let fixture = Fixture::new();
        let parent = fixture.segment("", 1, None, &message("included"));
        let cutoff = fs::metadata(&parent).unwrap().len();
        fs::OpenOptions::new()
            .append(true)
            .open(parent)
            .unwrap()
            .write_all(message("excluded parent append").as_bytes())
            .unwrap();
        fixture.segment(
            &format!("_{OTHER_ID}"),
            2,
            None,
            &message("excluded unrelated copy"),
        );
        fixture.segment(
            &format!("_{CHILD_ID}"),
            12,
            Some((ROOT_ID, cutoff)),
            &message("latest"),
        );
        let found = fixture.search("excluded");
        assert!(found.excerpt.is_none());
        assert!(!found.incomplete);
        assert_eq!(fixture.search("latest").excerpt.as_deref(), Some("latest"));
    }

    #[test]
    fn missing_cyclic_ambiguous_and_invalid_prefix_sources_are_incomplete_not_empty_success() {
        let fixture = Fixture::new();
        let parent = fixture.segment("", 1, None, &message("parent"));
        let end = fs::metadata(&parent).unwrap().len();
        for (id, cutoff) in [
            (OTHER_ID, end),
            (CHILD_ID, end),
            (ROOT_ID, end - 1),
            (ROOT_ID, end + 1),
        ] {
            fixture.segment(
                &format!("_{CHILD_ID}"),
                12,
                Some((id, cutoff)),
                &message("child"),
            );
            let result = fixture.search("not present");
            assert!(
                result.incomplete,
                "invalid source was treated as a complete negative"
            );
            assert!(result.excerpt.is_none());
        }
        fixture.segment(
            &format!("_{CHILD_ID}"),
            12,
            Some((ROOT_ID, end)),
            &message("child"),
        );
        fs::copy(
            &parent,
            parent.with_file_name(format!("rollout-duplicate-{ROOT_ID}.jsonl")),
        )
        .unwrap();
        assert!(fixture.search("not present").incomplete);
    }

    #[test]
    fn compressed_body_is_searchable_but_unverifiable_compressed_prefix_is_disclosed() {
        let fixture = Fixture::new();
        let parent = fixture.segment("", 1, None, &message("compressed match"));
        let end = fs::metadata(&parent).unwrap().len();
        let compressed = compressed_rollout_path(&parent);
        fs::write(
            &compressed,
            zstd::stream::encode_all(File::open(&parent).unwrap(), 1).unwrap(),
        )
        .unwrap();
        fs::remove_file(&parent).unwrap();
        assert_eq!(
            fixture.search("compressed match").excerpt.as_deref(),
            Some("compressed match")
        );
        fixture.segment(
            &format!("_{CHILD_ID}"),
            12,
            Some((ROOT_ID, end)),
            &message("child"),
        );
        assert!(fixture.search("compressed match").incomplete);
    }

    #[test]
    fn oversized_malformed_and_partial_records_do_not_hide_later_valid_matches() {
        let body = format!(
            "{}\nnot json\n{}",
            "x".repeat(MAX_JSONL_LINE_BYTES + 1),
            message("searchable later message")
        );
        let mut reader = BufReader::with_capacity(4096, std::io::Cursor::new(body.as_bytes()));
        let found = search_thread_reader(&mut reader, "later", &mut budget()).unwrap();
        assert_eq!(found.excerpt.as_deref(), Some("searchable later message"));
        assert_eq!(found.skipped_records, 2);
        assert!(found.incomplete);
        let mut partial = std::io::Cursor::new(b"{\"text\":\"partial \xe4\xb8");
        let result = search_thread_reader(&mut partial, "partial", &mut budget()).unwrap();
        assert!(result.incomplete);
        assert!(result.excerpt.is_none());
        assert_eq!(result.skipped_records, 1);
    }

    #[test]
    fn decoded_byte_budget_and_deadline_return_explicit_incomplete_results() {
        let source = format!("{}{}", message(&"x".repeat(8192)), message("late hit"));
        let compressed = zstd::stream::encode_all(source.as_bytes(), 1).unwrap();
        assert!(compressed.len() < 1024);
        let decoder = zstd::stream::read::Decoder::new(compressed.as_slice()).unwrap();
        let mut reader = BufReader::with_capacity(256, decoder);
        let mut limited = budget();
        limited.remaining_bytes = 1024;
        let result = search_thread_reader(&mut reader, "late hit", &mut limited).unwrap();
        assert!(result.incomplete);
        assert!(result.excerpt.is_none());
        assert_eq!(limited.remaining_bytes, 0);
        let mut expired = budget();
        expired.deadline = std::time::Instant::now();
        assert!(
            search_thread_reader(&mut std::io::Cursor::new(source), "late hit", &mut expired)
                .unwrap()
                .incomplete
        );
    }

    #[test]
    fn record_buffer_stays_bounded_while_draining_a_large_line() {
        let source = format!(
            "{}\n{}",
            "x".repeat(MAX_JSONL_LINE_BYTES + 8000),
            message("after")
        );
        let mut reader = BufReader::with_capacity(1024, std::io::Cursor::new(source));
        let mut buffer = Vec::new();
        let mut limited = budget();
        assert!(matches!(
            read_search_record(&mut reader, &mut buffer, &mut limited).unwrap(),
            SearchRecord::Skipped
        ));
        assert!(buffer.len() <= MAX_JSONL_LINE_BYTES);
        assert!(buffer.capacity() <= MAX_JSONL_LINE_BYTES);
        assert!(matches!(
            read_search_record(&mut reader, &mut buffer, &mut limited).unwrap(),
            SearchRecord::Record
        ));
        assert!(String::from_utf8(buffer).unwrap().contains("after"));
    }

    struct CancelOnConsume {
        inner: std::io::Cursor<Vec<u8>>,
        flag: SearchCancellation,
    }
    impl Read for CancelOnConsume {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read(out)
        }
    }
    impl BufRead for CancelOnConsume {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            let chunk = self.inner.fill_buf()?;
            Ok(&chunk[..chunk.len().min(16)])
        }
        fn consume(&mut self, count: usize) {
            self.inner.consume(count);
            self.flag.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn cancellation_is_checked_between_chunks_not_after_a_whole_record() {
        let mut limited = budget();
        let mut reader = CancelOnConsume {
            inner: std::io::Cursor::new(vec![b'x'; 8192]),
            flag: limited.cancelled.clone(),
        };
        assert!(search_thread_reader(&mut reader, "not present", &mut limited).is_err());
        assert_eq!(reader.inner.position(), 16);
    }

    #[test]
    fn obsolete_requests_are_cancelled_and_delayed_cancel_cannot_kill_the_new_query() {
        let mut clients = ThreadSearchClients::default();
        let old = clients.begin(ROOT_ID, 1).unwrap();
        let other = clients.begin(OTHER_ID, 1).unwrap();
        let current = clients.begin(ROOT_ID, 3).unwrap();
        assert!(old.load(Ordering::Relaxed));
        clients.cancel(ROOT_ID, 1);
        assert!(!current.load(Ordering::Relaxed));
        assert!(!other.load(Ordering::Relaxed));
        assert!(clients.begin(ROOT_ID, 2).is_err());
        clients.cancel(ROOT_ID, 3);
        assert!(current.load(Ordering::Relaxed));
        assert!(clients.begin(ROOT_ID, 3).is_err());
    }

    #[test]
    fn cancel_before_begin_rejects_a_queued_obsolete_search() {
        let mut clients = ThreadSearchClients::default();
        clients.cancel(ROOT_ID, 4);
        assert!(clients.begin(ROOT_ID, 4).is_err());
        assert!(clients.begin(ROOT_ID, 3).is_err());
        let current = clients.begin(ROOT_ID, 5).unwrap();
        clients.cancel(ROOT_ID, 4);
        assert!(!current.load(Ordering::Relaxed));
    }

    #[test]
    fn client_registry_is_bounded_and_validates_inputs() {
        let mut clients = ThreadSearchClients::default();
        assert!(clients.begin("not a client", 1).is_err());
        assert!(clients.begin(ROOT_ID, 0).is_err());
        let mut flags = Vec::new();
        for _ in 0..MAX_THREAD_SEARCH_CLIENTS + 1 {
            flags.push(clients.begin(&Uuid::new_v4().to_string(), 1).unwrap());
        }
        assert_eq!(clients.clients.len(), MAX_THREAD_SEARCH_CLIENTS);
        assert_eq!(
            flags
                .iter()
                .filter(|flag| flag.load(Ordering::Relaxed))
                .count(),
            1
        );
    }

    #[test]
    fn unicode_case_expansion_keeps_the_actual_match_in_the_excerpt() {
        let source = format!("{} Needle {}", "İ".repeat(200), "界".repeat(200));
        let excerpt = clipped_match(&source, "NEEDLE").unwrap();
        assert!(excerpt.contains("Needle"));
        assert!(excerpt.starts_with('…'));
        assert!(excerpt.ends_with('…'));
        assert!(excerpt.chars().count() < 130);
    }

    #[test]
    fn fast_negative_probe_preserves_case_whitespace_and_unicode_matching() {
        assert_eq!(
            clipped_match("one\n\t  TWO", "ONE TWO").as_deref(),
            Some("one TWO")
        );
        assert_eq!(
            clipped_match("before 热力图 after", "热力图").as_deref(),
            Some("before 热力图 after")
        );
        assert!(clipped_match(&"ascii output ".repeat(10000), "热力图").is_none());
        assert_eq!(
            clipped_match("İSTANBUL", "i\u{307}stanbul").as_deref(),
            Some("İSTANBUL")
        );
        assert!(clipped_match("many words with no matching phrase", "different phrase").is_none());
    }

    #[test]
    fn metadata_is_not_reported_as_a_body_match_and_nested_text_still_matches() {
        let body = format!(
            "{}\n{}",
            json!({"type":"session_meta","payload":{"cwd":"metadata-only"}}),
            message("nested body match")
        );
        assert!(search_thread_reader(
            &mut std::io::Cursor::new(body.as_bytes()),
            "metadata-only",
            &mut budget()
        )
        .unwrap()
        .excerpt
        .is_none());
        assert_eq!(
            search_thread_reader(&mut std::io::Cursor::new(body), "body", &mut budget())
                .unwrap()
                .excerpt
                .as_deref(),
            Some("nested body match")
        );
    }

    // Explicitly selected local roots only. No recovery, ownership writes,
    // transcript logging, credentials, or catalog mutations are used here.
    #[test]
    #[ignore = "Requires explicit HORIZON_READONLY_HISTORY_ROOT and HORIZON_READONLY_SEARCH_QUERY"]
    fn readonly_local_search_smoke() {
        let root =
            PathBuf::from(std::env::var("HORIZON_READONLY_HISTORY_ROOT").expect("explicit root"));
        let query = std::env::var("HORIZON_READONLY_SEARCH_QUERY").expect("explicit query");
        assert!(!query.trim().is_empty() && query.chars().count() <= 256);
        let started = std::time::Instant::now();
        let physical = gather_snapshots(&root).unwrap();
        let mut heads = select_thread_read_snapshots(&root, physical.clone());
        heads.sort_by_key(|head| std::cmp::Reverse(head.snapshot.updated_at));
        let mut limited = budget();
        let mut hits = 0;
        let mut incomplete = 0;
        let mut checked = 0;
        for head in &heads {
            let result =
                search_logical_thread(&head.snapshot, &physical, &query, &mut limited).unwrap();
            hits += usize::from(result.excerpt.is_some());
            incomplete += usize::from(result.incomplete);
            checked += usize::from(result.content_checked);
        }
        eprintln!("Read-only search: sessions={}, content_checked={}, hits={}, incomplete={}, decoded_bytes={}, elapsed_ms={}",
            heads.len(), checked, hits, incomplete, MAX_THREAD_SEARCH_BYTES - limited.remaining_bytes, started.elapsed().as_millis());
        assert!(hits > 0, "Expected a known local phrase to be found");
    }
}
