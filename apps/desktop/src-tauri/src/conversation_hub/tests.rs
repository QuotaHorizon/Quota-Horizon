#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "codex-switch-thread-test-{label}-{}",
            Uuid::new_v4()
        ))
    }

    fn read_model_rollout(root: &Path, name: &str, started: &str, updated: &str) -> PathBuf {
        let path = root.join("sessions/2026/09/07").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let meta = json!({"type":"session_meta", "timestamp":started,
            "payload":{"id":"logical-thread", "timestamp":started, "cwd":"/tmp/project-alpha"}});
        let tail = json!({"type":"event_msg", "timestamp":updated, "payload":{"type":"agent_message", "message":"fixture"}});
        fs::write(&path, format!("{meta}\n{tail}\n")).unwrap();
        path
    }

    #[test]
    fn logical_session_list_uses_latest_activity_without_hiding_physical_sources() {
        let root = test_root("logical-read");
        let old = read_model_rollout(&root, "rollout-old.jsonl", "2026-08-29T00:00:00Z", "2026-08-29T00:01:00Z");
        let latest = read_model_rollout(&root, "rollout-latest.jsonl", "2026-09-07T00:00:00Z", "2026-09-07T01:02:00Z");
        fs::write(root.join(INDEX_NAME), "{\"id\":\"logical-thread\",\"thread_name\":\"Old indexed title\",\"updated_at\":\"2026-08-29T00:00:00Z\"}\n").unwrap();
        let old_bytes = fs::read(&old).unwrap();
        let latest_bytes = fs::read(&latest).unwrap();
        let raw = gather_snapshots(&root).unwrap();
        assert_eq!(raw.len(), 2);
        let list = select_thread_read_snapshots(&root, raw);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].rollout_count, 2);
        assert_eq!(list[0].snapshot.path, latest);
        assert_eq!(list[0].snapshot.updated_at, unix_seconds(&json!("2026-09-07T01:02:00Z")));
        assert!(thread_metadata_matches(&list[0].snapshot, "project-alpha"));
        assert!(thread_metadata_matches(&list[0].snapshot, "logical-thread"));
        assert_eq!(gather_snapshots(&root).unwrap().len(), 2);
        assert_eq!(fs::read(old).unwrap(), old_bytes);
        assert_eq!(fs::read(latest).unwrap(), latest_bytes);
        assert!(!root.join("state_5.sqlite").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn canonical_state_projection_is_read_only_and_cannot_redirect_to_an_arbitrary_file() {
        let root = test_root("canonical-read");
        let canonical = read_model_rollout(&root, "rollout-canonical.jsonl", "2026-09-01T00:00:00Z", "2026-09-07T01:00:00Z");
        let newer = read_model_rollout(&root, "rollout-newer.jsonl", "2026-09-07T00:00:00Z", "2026-09-07T01:02:00Z");
        fs::create_dir_all(root.join("sqlite")).unwrap();
        let database = root.join("sqlite/state_5.sqlite");
        {
            let db = Connection::open(&database).unwrap();
            db.execute_batch("CREATE TABLE threads(id TEXT, rollout_path TEXT, title TEXT, updated_at INTEGER, updated_at_ms INTEGER)").unwrap();
            let updated = unix_seconds(&json!("2026-09-07T01:03:00Z")).unwrap();
            db.execute("INSERT INTO threads VALUES (?1, ?2, ?3, ?4, ?5)", params!["logical-thread", canonical.to_str().unwrap(), "Current title", updated - 1, updated * 1000]).unwrap();
        }
        let before = fs::read(&database).unwrap();
        let list = select_thread_read_snapshots(&root, gather_snapshots(&root).unwrap());
        assert_eq!(list[0].snapshot.path, canonical);
        assert_eq!(list[0].snapshot.title, "Current title");
        assert_eq!(list[0].snapshot.updated_at, unix_seconds(&json!("2026-09-07T01:03:00Z")));
        assert_eq!(fs::read(&database).unwrap(), before);
        {
            let db = Connection::open(&database).unwrap();
            db.execute("UPDATE threads SET rollout_path='/outside/rollout-unknown.jsonl'", []).unwrap();
        }
        let list = select_thread_read_snapshots(&root, gather_snapshots(&root).unwrap());
        assert_eq!(list[0].snapshot.path, newer);
        assert_ne!(list[0].snapshot.title, "Current title");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_appended_index_entry_does_not_win_over_a_newer_title() {
        let root = test_root("index-read");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(INDEX_NAME), concat!(
            "{\"id\":\"thread\",\"title\":\"Current\",\"updated_at\":\"2026-09-07T00:00:00Z\"}\n",
            "{\"id\":\"thread\",\"title\":\"Stale\",\"updated_at\":\"2026-08-29T00:00:00Z\"}\n",
        )).unwrap();
        assert_eq!(title_from_index(&index_values(&root).unwrap()["thread"]).as_deref(), Some("Current"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bounded_tail_scan_tolerates_a_partial_unicode_line_and_in_progress_append() {
        let root = test_root("bounded-tail");
        let path = read_model_rollout(&root, "rollout-tail.jsonl", "2026-08-29T00:00:00Z", "2026-08-29T00:00:00Z");
        let huge = json!({"type":"event_msg", "timestamp":"2026-09-01T00:00:00Z", "payload":{"message":"好".repeat(128 * 1024)}});
        let recent = json!({"timestamp":"2026-09-07T01:02:00Z", "type":"event_msg"});
        fs::write(&path, format!("{huge}\n{recent}\n{{\"timestamp\":\"unfinished")).unwrap();
        assert_eq!(rollout_tail_timestamp(&path), unix_seconds(&json!("2026-09-07T01:02:00Z")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_revision_detects_append_rename_and_state_wal_without_reading_messages() {
        let root = test_root("catalog-revision");
        let path = read_model_rollout(&root, "rollout-revision.jsonl", "2026-09-07T00:00:00Z", "2026-09-07T00:00:01Z");
        let first = thread_catalog_revision(&root).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(first, thread_catalog_revision(&root).unwrap());
        fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"not-json\n").unwrap();
        let appended = thread_catalog_revision(&root).unwrap();
        assert_ne!(first, appended);
        let renamed = path.with_file_name("rollout-renamed.jsonl");
        fs::rename(&path, &renamed).unwrap();
        assert_ne!(appended, thread_catalog_revision(&root).unwrap());
        let database = root.join("state_5.sqlite");
        Connection::open(&database).unwrap().execute_batch("CREATE TABLE threads(id TEXT)").unwrap();
        let before_wal = thread_catalog_revision(&root).unwrap();
        fs::write(root.join("state_5.sqlite-wal"), b"fixture").unwrap();
        assert_ne!(before_wal, thread_catalog_revision(&root).unwrap());
        assert!(fs::read(renamed).unwrap().ends_with(b"not-json\n"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scans_rollouts_with_index_titles() {
        let root = test_root("scan");
        let session_dir = root.join("sessions/2026/08/08");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("rollout-thread-a.jsonl"),
            concat!(
                r#"{"timestamp":"2026-08-08T10:00:00Z","type":"session_meta","payload":{"#,
                r#""id":"thread-a","cwd":"F:\\projects\\alpha","model_provider":"openai"}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"#,
                r#""total_token_usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}}"#,
                "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"#,
                r#""type":"input_text","text":"Please inspect the Alpha search result carefully."}]}}"#,
                "\n",
            ),
        )
        .unwrap();
        fs::write(
            root.join(INDEX_NAME),
            r#"{"id":"thread-a","thread_name":"Alpha session","updated_at":"2026-08-08T10:30:00Z"}
"#,
        )
        .unwrap();

        let snapshots = gather_snapshots(&root).unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].session_id, "thread-a");
        assert_eq!(snapshots[0].title, "Alpha session");
        assert_eq!(snapshots[0].cwd, r#"F:\projects\alpha"#);
        assert_eq!(token_totals(&snapshots[0].path), Some((10, 5, 15)));
        assert_eq!(
            locate_rollout_text(&snapshots[0].path, "alpha")
                .unwrap()
                .as_deref(),
            Some("Please inspect the Alpha search result carefully.")
        );
        assert!(locate_rollout_text(&snapshots[0].path, "missing")
            .unwrap()
            .is_none());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn parses_bounded_session_summary_and_structured_timeline() {
        let root = test_root("timeline");
        let session_dir = root.join("sessions/2026/08/08");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("rollout-thread-timeline.jsonl"),
            concat!(
                r#"{"timestamp":"2026-08-08T10:00:00Z","type":"session_meta","payload":{"id":"thread-timeline","timestamp":"2026-08-08T10:00:00Z","cwd":"/tmp/project","originator":"codex","source":{"subagent":{"thread_spawn":{"agent_role":"worker"}}},"cli_version":"1.2.3","model_provider":"openai"}}"#,
                "\n",
                r#"{"timestamp":"2026-08-08T10:01:00Z","type":"event_msg","payload":{"type":"user_message","message":"Inspect the parser"}}"#,
                "\n",
                r#"{"timestamp":"2026-08-08T10:02:00Z","type":"event_msg","payload":{"type":"agent_message","message":"I will inspect it"}}"#,
                "\n",
                r#"{"timestamp":"2026-08-08T10:03:00Z","type":"response_item","payload":{"type":"function_call","call_id":"call-1","name":"shell","arguments":"rg parser"}}"#,
                "\n",
                r#"{"timestamp":"2026-08-08T10:04:00Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call-1","output":"failed: fixture"}}"#,
                "\n",
                "{malformed}\n",
            ),
        )
        .unwrap();
        fs::write(
            root.join(INDEX_NAME),
            r#"{"id":"thread-timeline","thread_name":"Timeline session"}
"#,
        )
        .unwrap();

        let snapshot = gather_snapshots(&root).unwrap().remove(0);
        let parsed = parse_thread_detail(&snapshot).unwrap();
        assert_eq!(parsed.summary.title, "Timeline session");
        assert_eq!(parsed.summary.started_at.as_deref(), Some("2026-08-08T10:00:00Z"));
        assert_eq!(parsed.summary.updated_at.as_deref(), Some("2026-08-08T10:04:00Z"));
        assert_eq!(parsed.summary.source, "subagent:worker");
        assert_eq!(parsed.summary.line_count, 6);
        assert_eq!(parsed.summary.event_count, 2);
        assert_eq!(parsed.summary.tool_call_count, 1);
        assert_eq!(parsed.summary.user_prompt_excerpt, "Inspect the parser");
        assert_eq!(parsed.summary.latest_agent_message_excerpt, "I will inspect it");
        assert_eq!(
            parsed.summary.resume_command.as_deref(),
            Some("codex resume thread-timeline")
        );
        assert_eq!(parsed.timeline.len(), 3);
        assert_eq!(parsed.timeline[0].kind, "message:user");
        assert_eq!(parsed.timeline[1].kind, "message:assistant");
        assert_eq!(parsed.timeline[2].kind, "tool_call");
        assert_eq!(parsed.timeline[2].tool_name.as_deref(), Some("shell"));
        assert_eq!(parsed.timeline[2].tool_status.as_deref(), Some("errored"));
        assert_eq!(
            parsed.timeline[2].tool_output.as_deref(),
            Some("failed: fixture")
        );
        let revision = parsed.revision.clone();
        assert_eq!(revision.len(), 64);
        let page = page_thread_detail(&parsed, Some(0), Some(2), None).unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.total, 3);
        assert_eq!(page.next_offset, Some(2));
        assert_eq!(page.revision, revision);

        let parsed = parse_thread_detail(&snapshot).unwrap();
        assert_eq!(
            page_thread_detail(&parsed, Some(2), Some(2), Some("stale".to_string()))
                .unwrap_err(),
            "session_revision_changed"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn timeline_bounds_page_size_text_and_resume_command() {
        assert_eq!(timeline_page_size(None), DEFAULT_TIMELINE_PAGE_SIZE);
        assert_eq!(timeline_page_size(Some(0)), 1);
        assert_eq!(timeline_page_size(Some(usize::MAX)), MAX_TIMELINE_PAGE_SIZE);
        assert!(safe_resume_command("thread-safe_1").is_some());
        assert!(safe_resume_command("thread unsafe").is_none());

        let source = "好".repeat(MAX_TIMELINE_TEXT_CHARS + 10);
        let (bounded, truncated) = bounded_timeline_text(source);
        assert!(truncated);
        assert_eq!(bounded.chars().count(), MAX_TIMELINE_TEXT_CHARS);
        assert!(bounded.ends_with('…'));
    }

    #[test]
    fn prepares_saved_and_temporary_resume_without_mutating_metadata() {
        let root = test_root("resume");
        let original = root.join("original project");
        let temporary = root.join("temporary project");
        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&temporary).unwrap();
        let session_dir = root.join("sessions/2026/09/04");
        fs::create_dir_all(&session_dir).unwrap();
        let rollout = session_dir.join("rollout-thread-resume.jsonl");
        fs::write(
            &rollout,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"thread-resume\",\"cwd\":{}}}}}\n",
                serde_json::to_string(&original.to_string_lossy()).unwrap()
            ),
        )
        .unwrap();

        let saved = prepare_thread_resume(&root, "thread-resume", None).unwrap();
        assert_eq!(saved.mode, "savedDirectory");
        assert_eq!(saved.arguments, ["resume", "thread-resume"]);
        assert_eq!(saved.display_command, "codex resume thread-resume");

        let elsewhere = prepare_thread_resume(
            &root,
            "thread-resume",
            Some(temporary.to_string_lossy().as_ref()),
        )
        .unwrap();
        assert_eq!(elsewhere.mode, "temporaryDirectory");
        assert_eq!(elsewhere.arguments[0..3], ["resume", "thread-resume", "-C"]);
        assert_eq!(elsewhere.arguments[3], temporary.canonicalize().unwrap().to_string_lossy());
        assert!(elsewhere.display_command.contains("temporary project"));
        assert!(fs::read_to_string(&rollout)
            .unwrap()
            .contains(original.to_string_lossy().as_ref()));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resume_rejects_archived_missing_and_unsafe_sessions() {
        let root = test_root("resume-invalid");
        let cwd = root.join("project");
        let archive = root.join("archived_sessions/2026/09/04");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&archive).unwrap();
        fs::write(
            archive.join("rollout-thread-archived.jsonl"),
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"thread-archived\",\"cwd\":{}}}}}\n",
                serde_json::to_string(&cwd.to_string_lossy()).unwrap()
            ),
        )
        .unwrap();

        assert!(prepare_thread_resume(&root, "thread-archived", None)
            .unwrap_err()
            .contains("已归档"));
        assert!(prepare_thread_resume(&root, "thread missing", None)
            .unwrap_err()
            .contains("ID 无效"));
        assert!(prepare_thread_resume(&root, "thread-missing", None)
            .unwrap_err()
            .contains("无法唯一定位"));

        fs::remove_dir_all(root).unwrap();
    }

    fn write_rebind_state_database(path: &Path, session_id: &str, cwd: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT NOT NULL,
                    cwd TEXT NOT NULL,
                    archived INTEGER NOT NULL DEFAULT 0,
                    archived_at INTEGER,
                    preview TEXT NOT NULL DEFAULT ''
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, rollout_path, cwd) VALUES (?1, ?2, ?3)",
                params![session_id, "sessions/rollout.jsonl", cwd],
            )
            .unwrap();
    }

    fn write_rebind_fixture(root: &Path, session_id: &str) -> (PathBuf, PathBuf, Vec<PathBuf>) {
        let old_cwd = root.join("old project");
        let target_cwd = root.join("new project");
        fs::create_dir_all(&old_cwd).unwrap();
        fs::create_dir_all(&target_cwd).unwrap();
        let session_dir = root.join("sessions/2026/09/04");
        fs::create_dir_all(&session_dir).unwrap();
        let logical = session_dir.join(format!("rollout-{session_id}.jsonl"));
        let source = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":{}}}}}\n{{\"type\":\"event_msg\",\"payload\":{{\"message\":\"keep me\"}}}}\n",
            serde_json::to_string(&old_cwd.to_string_lossy()).unwrap()
        );
        fs::write(&logical, &source).unwrap();
        let compressed = compressed_rollout_path(&logical);
        let output = File::create(&compressed).unwrap();
        let mut encoder = zstd::stream::write::Encoder::new(output, 3).unwrap();
        encoder.write_all(source.as_bytes()).unwrap();
        encoder.finish().unwrap();
        fs::write(
            root.join(INDEX_NAME),
            format!(
                "{{\"id\":\"{session_id}\",\"thread_name\":\"Rebind fixture\"}}\n"
            ),
        )
        .unwrap();

        write_rebind_state_database(
            &root.join("sqlite/state_5.sqlite"),
            session_id,
            old_cwd.to_string_lossy().as_ref(),
        );
        write_rebind_state_database(
            &root.join("state_4.sqlite"),
            session_id,
            old_cwd.to_string_lossy().as_ref(),
        );
        let catalog = root.join("sqlite/codex-dev.db");
        let connection = Connection::open(&catalog).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE local_thread_catalog (
                    host_id TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    cwd TEXT,
                    PRIMARY KEY (host_id, thread_id)
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO local_thread_catalog (host_id, thread_id, cwd) VALUES ('local', ?1, ?2)",
                params![session_id, old_cwd.to_string_lossy()],
            )
            .unwrap();
        drop(connection);
        (old_cwd, target_cwd, vec![logical, compressed])
    }

    fn assert_rebind_cwd(
        root: &Path,
        session_id: &str,
        rollouts: &[PathBuf],
        expected: &Path,
    ) {
        for rollout in rollouts {
            let meta = first_rollout_value(rollout).unwrap().unwrap();
            assert_eq!(
                snapshot_cwd(&meta).as_deref(),
                Some(expected.to_string_lossy().as_ref())
            );
            assert!(locate_rollout_text(rollout, "keep me").unwrap().is_some());
        }
        for state in [root.join("sqlite/state_5.sqlite"), root.join("state_4.sqlite")] {
            assert_eq!(
                state_database_cwd(&state, session_id).unwrap().as_deref(),
                Some(expected.to_string_lossy().as_ref())
            );
        }
        assert_eq!(
            catalog_row_cwd(
                &root.join("sqlite/codex-dev.db"),
                session_id,
                "local"
            )
            .unwrap(),
            Some(Some(expected.to_string_lossy().into_owned()))
        );
    }

    #[test]
    fn thread_rebind_commits_rollouts_state_catalog_and_preserves_index() {
        let root = test_root("rebind-commit");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-rebind";
        let (_old, target, rollouts) = write_rebind_fixture(&root, session_id);
        let original_index = fs::read(root.join(INDEX_NAME)).unwrap();
        let prepared = prepare_thread_rebind(
            &root,
            session_id,
            target.to_string_lossy().as_ref(),
        )
        .unwrap();
        assert_eq!(prepared.files.len(), 2);
        assert_eq!(prepared.state_rows.len(), 2);
        assert_eq!(prepared.catalog_rows.len(), 1);
        let transaction_root = root.join("app-data/rebind");
        let report = execute_thread_rebind_transaction(
            &root,
            &transaction_root,
            prepared,
            || Ok(()),
        )
        .unwrap();
        assert_eq!(report.rollout_file_count, 2);
        assert_eq!(report.state_database_row_count, 2);
        assert_eq!(report.catalog_row_count, 1);
        assert_rebind_cwd(
            &root,
            session_id,
            &rollouts,
            &target.canonicalize().unwrap(),
        );
        assert_eq!(fs::read(root.join(INDEX_NAME)).unwrap(), original_index);
        assert_eq!(fs::read_dir(&transaction_root).unwrap().count(), 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_rebind_rolls_back_every_surface_when_admission_is_lost() {
        let root = test_root("rebind-rollback");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-rebind-rollback";
        let (old, target, rollouts) = write_rebind_fixture(&root, session_id);
        let prepared = prepare_thread_rebind(
            &root,
            session_id,
            target.to_string_lossy().as_ref(),
        )
        .unwrap();
        let mut calls = 0usize;
        let error = execute_thread_rebind_transaction(
            &root,
            &root.join("app-data/rebind"),
            prepared,
            || {
                calls += 1;
                (calls < 3)
                    .then_some(())
                    .ok_or_else(|| "injected lease loss".to_string())
            },
        )
        .unwrap_err();
        assert!(error.contains("已自动恢复"));
        assert_rebind_cwd(&root, session_id, &rollouts, &old);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_thread_rebind_recovers_after_interrupted_apply() {
        let root = test_root("rebind-crash");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-rebind-crash";
        let (old, target, rollouts) = write_rebind_fixture(&root, session_id);
        let prepared = prepare_thread_rebind(
            &root,
            session_id,
            target.to_string_lossy().as_ref(),
        )
        .unwrap();
        let transaction_root = root.join("app-data/rebind");
        let transaction =
            create_rebind_transaction(&transaction_root, &root, &prepared).unwrap();
        let first_file = &transaction.manifest.files[0];
        replace_rebind_file(
            &transaction
                .directory
                .join("replacements")
                .join(&first_file.replacement_name),
            &root.join(&first_file.source_relative),
        )
        .unwrap();
        let first_state = &transaction.manifest.state_rows[0];
        update_state_database_cwd(
            &root.join(&first_state.database_relative),
            session_id,
            &first_state.original_cwd,
            &transaction.manifest.target_cwd,
        )
        .unwrap();
        drop(transaction);
        fs::remove_dir_all(&target).unwrap();

        recover_rebind_transactions_with_revalidate(&root, &transaction_root, || Ok(()))
            .unwrap();
        assert_rebind_cwd(&root, session_id, &rollouts, &old);
        assert_eq!(fs::read_dir(&transaction_root).unwrap().count(), 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_rebind_confirmation_is_one_time_and_revision_bound() {
        let root = test_root("rebind-plan");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-rebind-plan";
        let (_old, target, rollouts) = write_rebind_fixture(&root, session_id);
        let prepared = prepare_thread_rebind(
            &root,
            session_id,
            target.to_string_lossy().as_ref(),
        )
        .unwrap();
        let mut plans = PendingThreadRebindPlans::default();
        let (token, _) = plans.issue(&prepared, Utc::now()).unwrap();
        let consumed = plans.consume(&token, Utc::now()).unwrap();
        assert_eq!(consumed.session_id, session_id);
        assert!(plans.consume(&token, Utc::now()).is_err());

        fs::OpenOptions::new()
            .append(true)
            .open(&rollouts[0])
            .unwrap()
            .write_all(b"{\"type\":\"event_msg\"}\n")
            .unwrap();
        let changed = prepare_thread_rebind(
            &root,
            session_id,
            target.to_string_lossy().as_ref(),
        )
        .unwrap();
        assert_ne!(changed.revision, consumed.expected_revision);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_rebind_rolls_back_its_surfaces_when_the_guard_index_changes() {
        let root = test_root("rebind-index-drift");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-rebind-index-drift";
        let (old, target, rollouts) = write_rebind_fixture(&root, session_id);
        let prepared = prepare_thread_rebind(
            &root,
            session_id,
            target.to_string_lossy().as_ref(),
        )
        .unwrap();
        let mut calls = 0usize;
        let error = execute_thread_rebind_transaction(
            &root,
            &root.join("app-data/rebind"),
            prepared,
            || {
                calls += 1;
                if calls == 3 {
                    fs::write(
                        root.join(INDEX_NAME),
                        format!(
                            "{{\"id\":\"{session_id}\",\"thread_name\":\"Concurrent title\"}}\n"
                        ),
                    )
                    .unwrap();
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.contains("已自动恢复"));
        assert_rebind_cwd(&root, session_id, &rollouts, &old);
        assert!(fs::read_to_string(root.join(INDEX_NAME))
            .unwrap()
            .contains("Concurrent title"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_thread_rebind_cleans_up_even_if_payload_or_target_disappeared() {
        let root = test_root("rebind-committed-cleanup");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-rebind-committed-cleanup";
        let (_old, target, _rollouts) = write_rebind_fixture(&root, session_id);
        let prepared = prepare_thread_rebind(
            &root,
            session_id,
            target.to_string_lossy().as_ref(),
        )
        .unwrap();
        let transaction_root = root.join("app-data/rebind");
        let mut transaction =
            create_rebind_transaction(&transaction_root, &root, &prepared).unwrap();
        transaction.manifest.state = RebindTransactionState::Committed;
        write_rebind_manifest(&transaction.directory, &transaction.manifest).unwrap();
        fs::remove_dir_all(transaction.directory.join("backups")).unwrap();
        fs::remove_dir_all(transaction.directory.join("replacements")).unwrap();
        fs::remove_dir_all(target).unwrap();
        drop(transaction);

        recover_rebind_transactions_with_revalidate(&root, &transaction_root, || Ok(()))
            .unwrap();
        assert_eq!(fs::read_dir(&transaction_root).unwrap().count(), 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn thread_rebind_rejects_a_rollout_that_is_still_open() {
        let root = test_root("rebind-open-file");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-rebind-open-file";
        let (old, target, rollouts) = write_rebind_fixture(&root, session_id);
        let prepared = prepare_thread_rebind(
            &root,
            session_id,
            target.to_string_lossy().as_ref(),
        )
        .unwrap();
        let _open_rollout = File::open(&rollouts[0]).unwrap();
        let error = execute_thread_rebind_transaction(
            &root,
            &root.join("app-data/rebind"),
            prepared,
            || Ok(()),
        )
        .unwrap_err();
        assert!(error.contains("仍被 Codex 使用"));
        assert_rebind_cwd(&root, session_id, &rollouts, &old);

        fs::remove_dir_all(root).unwrap();
    }

    fn write_visibility_state_database(
        path: &Path,
        session_id: &str,
        insert_stale_row: bool,
    ) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    source TEXT NOT NULL,
                    model_provider TEXT NOT NULL,
                    cwd TEXT NOT NULL,
                    title TEXT NOT NULL,
                    sandbox_policy TEXT NOT NULL,
                    approval_mode TEXT NOT NULL,
                    tokens_used INTEGER NOT NULL DEFAULT 0,
                    has_user_event INTEGER NOT NULL DEFAULT 0,
                    archived INTEGER NOT NULL DEFAULT 0,
                    archived_at INTEGER,
                    cli_version TEXT NOT NULL DEFAULT '',
                    first_user_message TEXT NOT NULL DEFAULT '',
                    preview TEXT NOT NULL DEFAULT ''
                );",
            )
            .unwrap();
        if insert_stale_row {
            connection
                .execute(
                    concat!(
                        "INSERT INTO threads (id, rollout_path, created_at, updated_at, source, ",
                        "model_provider, cwd, title, sandbox_policy, approval_mode, archived, ",
                        "archived_at) VALUES (?1, '/missing/old.jsonl', 1, 2, 'exec', 'stale', ",
                        "'/missing/cwd', 'Manual title', '{}', 'never', 1, 2)"
                    ),
                    params![session_id],
                )
                .unwrap();
        }
    }

    fn write_visibility_repair_fixture(
        root: &Path,
        session_id: &str,
        with_index: bool,
    ) -> (PathBuf, PathBuf, String) {
        let cwd = root.join("project");
        fs::create_dir_all(&cwd).unwrap();
        let session_dir = root.join("sessions/2026/09/04");
        fs::create_dir_all(&session_dir).unwrap();
        let rollout = session_dir.join(format!("rollout-{session_id}.jsonl"));
        fs::write(
            &rollout,
            format!(
                concat!(
                    "{{\"timestamp\":\"2026-09-04T01:02:03Z\",\"type\":\"session_meta\",",
                    "\"payload\":{{\"id\":\"{session_id}\",\"cwd\":{},\"source\":\"vscode\",",
                    "\"cli_version\":\"1.2.3\",\"model_provider\":\"openai\"}}}}\n",
                    "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",",
                    "\"info\":{{\"total_token_usage\":{{\"input_tokens\":7,",
                    "\"output_tokens\":5,\"total_tokens\":12}}}}}}}}\n",
                    "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",",
                    "\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",",
                    "\"text\":\"Repair this hidden session\"}}]}}}}\n"
                ),
                serde_json::to_string(&cwd.to_string_lossy()).unwrap(),
                session_id = session_id,
            ),
        )
        .unwrap();
        write_visibility_state_database(
            &root.join("sqlite/state_5.sqlite"),
            session_id,
            true,
        );
        write_visibility_state_database(&root.join("state_4.sqlite"), session_id, false);
        let catalog = root.join("sqlite/codex-dev.db");
        let connection = Connection::open(&catalog).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE local_thread_catalog (
                    host_id TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    display_title TEXT NOT NULL,
                    cwd TEXT,
                    model_provider TEXT,
                    missing_candidate INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (host_id, thread_id)
                );",
            )
            .unwrap();
        connection
            .execute(
                concat!(
                    "INSERT INTO local_thread_catalog ",
                    "(host_id, thread_id, display_title, cwd, model_provider, missing_candidate) ",
                    "VALUES ('local', ?1, 'Manual catalog title', '/missing/cwd', 'stale', 1)"
                ),
                params![session_id],
            )
            .unwrap();
        let index = if with_index {
            "{\"id\":\"unrelated\",\"thread_name\":\"Keep me\"}\n{malformed}\n".to_string()
        } else {
            String::new()
        };
        if with_index {
            fs::write(root.join(INDEX_NAME), &index).unwrap();
        }
        (rollout, cwd, index)
    }

    fn assert_visibility_repaired(root: &Path, session_id: &str, rollout: &Path, cwd: &Path) {
        for database in [root.join("sqlite/state_5.sqlite"), root.join("state_4.sqlite")] {
            let connection = Connection::open(database).unwrap();
            let row = snapshot_visibility_row(
                &connection,
                VisibilityDatabaseKind::State,
                session_id,
                None,
            )
            .unwrap()
            .unwrap();
            assert_eq!(row_text(&row, "rollout_path"), Some(rollout.to_string_lossy().as_ref()));
            assert_eq!(row_text(&row, "cwd"), Some(cwd.to_string_lossy().as_ref()));
            assert_eq!(row_text(&row, "model_provider"), Some("openai"));
            assert_eq!(row_cell(&row, "archived"), Some(&SqliteCell::Integer(0)));
            assert_eq!(row_cell(&row, "archived_at"), Some(&SqliteCell::Null));
            assert_eq!(
                row_text(&row, "preview"),
                Some("Repair this hidden session")
            );
            assert_eq!(
                row_text(&row, "first_user_message"),
                Some("Repair this hidden session")
            );
            assert_eq!(
                row_cell(&row, "has_user_event"),
                Some(&SqliteCell::Integer(1))
            );
        }
        let catalog = Connection::open(root.join("sqlite/codex-dev.db")).unwrap();
        let row = snapshot_visibility_row(
            &catalog,
            VisibilityDatabaseKind::Catalog,
            session_id,
            Some("local"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(row_text(&row, "cwd"), Some(cwd.to_string_lossy().as_ref()));
        assert_eq!(row_text(&row, "model_provider"), Some("openai"));
        assert_eq!(row_cell(&row, "missing_candidate"), Some(&SqliteCell::Integer(0)));
        assert_eq!(row_text(&row, "display_title"), Some("Manual catalog title"));
    }

    #[test]
    fn visibility_repair_commits_row_aware_state_catalog_and_additive_index() {
        let root = test_root("visibility-repair-commit");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-visibility-commit";
        let (rollout, cwd, original_index) =
            write_visibility_repair_fixture(&root, session_id, true);
        let original_rollout = fs::read(&rollout).unwrap();
        let prepared =
            prepare_visibility_repair(&root, VisibilityRepairMode::Deep, None).unwrap();
        let (updates, inserts, catalogs) = visibility_repair_counts(&prepared.databases);
        assert_eq!((updates, inserts, catalogs), (1, 1, 1));
        assert_eq!(prepared.index.as_ref().unwrap().add_count, 1);
        let transaction_root = root.join("app-data/visibility");

        let report = execute_visibility_repair_transaction(
            &root,
            &transaction_root,
            prepared,
            || Ok(()),
        )
        .unwrap();
        assert_eq!(report.repaired_session_count, 1);
        assert_visibility_repaired(&root, session_id, &rollout, &cwd);
        let canonical = Connection::open(root.join("sqlite/state_5.sqlite")).unwrap();
        let canonical_row = snapshot_visibility_row(
            &canonical,
            VisibilityDatabaseKind::State,
            session_id,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(row_text(&canonical_row, "title"), Some("Manual title"));
        let legacy = Connection::open(root.join("state_4.sqlite")).unwrap();
        let legacy_row = snapshot_visibility_row(
            &legacy,
            VisibilityDatabaseKind::State,
            session_id,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            row_text(&legacy_row, "title"),
            Some("Repair this hidden session")
        );
        let repaired_index = fs::read_to_string(root.join(INDEX_NAME)).unwrap();
        assert!(repaired_index.contains("Keep me"));
        assert!(repaired_index.contains("{malformed}"));
        assert!(repaired_index.contains(session_id));
        assert_ne!(repaired_index, original_index);
        assert_eq!(fs::read(&rollout).unwrap(), original_rollout);
        assert_eq!(fs::read_dir(&transaction_root).unwrap().count(), 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn visibility_repair_completes_existing_index_entry_without_losing_unknown_lines() {
        let root = test_root("visibility-repair-index-update");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-visibility-index-update";
        let (_rollout, _cwd, _index) =
            write_visibility_repair_fixture(&root, session_id, true);
        let original_index = format!(
            concat!(
                "{{\"id\":\"{session_id}\",\"thread_name\":\"\",",
                "\"title\":\"Friendly title\",\"future\":{{\"keep\":true}}}}\r\n",
                "{{malformed and preserved}}\r\n\r\n"
            ),
            session_id = session_id,
        );
        fs::write(root.join(INDEX_NAME), &original_index).unwrap();
        let prepared =
            prepare_visibility_repair(&root, VisibilityRepairMode::Deep, None).unwrap();
        let index = prepared.index.as_ref().unwrap();
        assert_eq!(index.add_count, 0);
        assert_eq!(index.update_count, 1);
        assert!(index.target_content.contains("Friendly title"));
        assert!(index.target_content.contains("\"future\":{\"keep\":true}"));
        assert!(index
            .target_content
            .ends_with("{malformed and preserved}\r\n\r\n"));

        execute_visibility_repair_transaction(
            &root,
            &root.join("app-data/visibility"),
            prepared,
            || Ok(()),
        )
        .unwrap();
        let repaired = fs::read_to_string(root.join(INDEX_NAME)).unwrap();
        let first: Value = serde_json::from_str(repaired.lines().next().unwrap()).unwrap();
        assert_eq!(
            first.get("thread_name").and_then(Value::as_str),
            Some("Friendly title")
        );
        assert_eq!(first.pointer("/future/keep").and_then(Value::as_bool), Some(true));
        assert!(repaired.ends_with("{malformed and preserved}\r\n\r\n"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn visibility_repair_uses_unique_state_selected_rollout_and_rejects_database_disagreement() {
        let root = test_root("visibility-repair-rollout-selection");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-visibility-rollout-selection";
        let (first_rollout, _cwd, _index) =
            write_visibility_repair_fixture(&root, session_id, true);
        let second_dir = root.join("sessions/2026/09/05");
        fs::create_dir_all(&second_dir).unwrap();
        let second_rollout = second_dir.join(format!("rollout-{session_id}.jsonl"));
        fs::copy(&first_rollout, &second_rollout).unwrap();

        let canonical = Connection::open(root.join("sqlite/state_5.sqlite")).unwrap();
        canonical
            .execute(
                "UPDATE threads SET rollout_path = ?1 WHERE id = ?2",
                params![first_rollout.to_string_lossy(), session_id],
            )
            .unwrap();
        drop(canonical);

        let prepared =
            prepare_visibility_repair(&root, VisibilityRepairMode::Quick, None).unwrap();
        assert_eq!(prepared.sources.len(), 1);
        assert_eq!(prepared.sources[0].rollout_path, first_rollout.to_string_lossy());
        assert_eq!(prepared.rollout_guards.len(), 1);
        assert!(prepared.rollout_guards[0]
            .source_relative
            .contains("sessions/2026/09/04"));

        let legacy = Connection::open(root.join("state_4.sqlite")).unwrap();
        legacy
            .execute(
                concat!(
                    "INSERT INTO threads (id, rollout_path, created_at, updated_at, source, ",
                    "model_provider, cwd, title, sandbox_policy, approval_mode, archived, ",
                    "archived_at) VALUES (?1, ?2, 1, 2, 'exec', 'openai', ",
                    "'/missing/cwd', '', '{}', 'never', 0, NULL)"
                ),
                params![session_id, second_rollout.to_string_lossy()],
            )
            .unwrap();
        drop(legacy);

        let error = prepare_visibility_repair(&root, VisibilityRepairMode::Quick, None)
            .unwrap_err();
        assert!(error.contains("canonical/legacy state DB 选择了不同 rollout"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn visibility_repair_rolls_back_updates_inserts_catalog_and_new_index() {
        let root = test_root("visibility-repair-rollback");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-visibility-rollback";
        let (rollout, _cwd, _index) =
            write_visibility_repair_fixture(&root, session_id, false);
        let canonical = Connection::open(root.join("sqlite/state_5.sqlite")).unwrap();
        let original_state = snapshot_visibility_row(
            &canonical,
            VisibilityDatabaseKind::State,
            session_id,
            None,
        )
        .unwrap();
        drop(canonical);
        let catalog = Connection::open(root.join("sqlite/codex-dev.db")).unwrap();
        let original_catalog = snapshot_visibility_row(
            &catalog,
            VisibilityDatabaseKind::Catalog,
            session_id,
            Some("local"),
        )
        .unwrap();
        drop(catalog);
        let prepared =
            prepare_visibility_repair(&root, VisibilityRepairMode::Deep, None).unwrap();
        let mut calls = 0usize;
        let error = execute_visibility_repair_transaction(
            &root,
            &root.join("app-data/visibility"),
            prepared,
            || {
                calls += 1;
                (calls < 4)
                    .then_some(())
                    .ok_or_else(|| "injected lease loss".to_string())
            },
        )
        .unwrap_err();
        assert!(error.contains("已自动恢复"));
        let canonical = Connection::open(root.join("sqlite/state_5.sqlite")).unwrap();
        assert_eq!(
            snapshot_visibility_row(
                &canonical,
                VisibilityDatabaseKind::State,
                session_id,
                None,
            )
            .unwrap(),
            original_state
        );
        let legacy = Connection::open(root.join("state_4.sqlite")).unwrap();
        assert!(snapshot_visibility_row(
            &legacy,
            VisibilityDatabaseKind::State,
            session_id,
            None,
        )
        .unwrap()
        .is_none());
        let catalog = Connection::open(root.join("sqlite/codex-dev.db")).unwrap();
        assert_eq!(
            snapshot_visibility_row(
                &catalog,
                VisibilityDatabaseKind::Catalog,
                session_id,
                Some("local"),
            )
            .unwrap(),
            original_catalog
        );
        assert!(!root.join(INDEX_NAME).exists());
        assert!(first_rollout_value(&rollout).unwrap().is_some());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn visibility_repair_preserves_unrelated_index_change_while_rolling_back_databases() {
        let root = test_root("visibility-repair-index-drift");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-visibility-index-drift";
        let (_rollout, _cwd, _index) =
            write_visibility_repair_fixture(&root, session_id, true);
        let canonical = Connection::open(root.join("sqlite/state_5.sqlite")).unwrap();
        let original_state = snapshot_visibility_row(
            &canonical,
            VisibilityDatabaseKind::State,
            session_id,
            None,
        )
        .unwrap();
        drop(canonical);
        let prepared =
            prepare_visibility_repair(&root, VisibilityRepairMode::Deep, None).unwrap();
        let error = execute_visibility_repair_transaction(
            &root,
            &root.join("app-data/visibility"),
            prepared,
            || {
                let index = fs::read_to_string(root.join(INDEX_NAME)).unwrap_or_default();
                if index.contains(session_id) {
                    fs::write(
                        root.join(INDEX_NAME),
                        "{\"id\":\"concurrent\",\"thread_name\":\"Preserve me\"}\n",
                    )
                    .unwrap();
                    return Err("injected concurrent index update".to_string());
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.contains("无法确认自动恢复结果"));
        let canonical = Connection::open(root.join("sqlite/state_5.sqlite")).unwrap();
        assert_eq!(
            snapshot_visibility_row(
                &canonical,
                VisibilityDatabaseKind::State,
                session_id,
                None,
            )
            .unwrap(),
            original_state
        );
        let legacy = Connection::open(root.join("state_4.sqlite")).unwrap();
        assert!(snapshot_visibility_row(
            &legacy,
            VisibilityDatabaseKind::State,
            session_id,
            None,
        )
        .unwrap()
        .is_none());
        assert_eq!(
            fs::read_to_string(root.join(INDEX_NAME)).unwrap(),
            "{\"id\":\"concurrent\",\"thread_name\":\"Preserve me\"}\n"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_visibility_repair_recovers_after_partial_database_commit() {
        let root = test_root("visibility-repair-crash");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-visibility-crash";
        let (_rollout, _cwd, original_index) =
            write_visibility_repair_fixture(&root, session_id, true);
        let prepared =
            prepare_visibility_repair(&root, VisibilityRepairMode::Deep, None).unwrap();
        let transaction_root = root.join("app-data/visibility");
        let transaction =
            create_visibility_repair_transaction(&transaction_root, &root, &prepared).unwrap();
        apply_visibility_database_plan(&root, &transaction.manifest.databases[0], true).unwrap();
        drop(transaction);

        recover_visibility_repair_transactions_with_revalidate(
            &root,
            &transaction_root,
            || Ok(()),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(root.join(INDEX_NAME)).unwrap(), original_index);
        let legacy = Connection::open(root.join("state_4.sqlite")).unwrap();
        assert!(snapshot_visibility_row(
            &legacy,
            VisibilityDatabaseKind::State,
            session_id,
            None,
        )
        .unwrap()
        .is_none());
        assert_eq!(fs::read_dir(&transaction_root).unwrap().count(), 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn visibility_repair_confirmation_is_one_time_and_revision_bound() {
        let root = test_root("visibility-repair-plan");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-visibility-plan";
        let (rollout, _cwd, _index) =
            write_visibility_repair_fixture(&root, session_id, true);
        let prepared =
            prepare_visibility_repair(&root, VisibilityRepairMode::Quick, None).unwrap();
        let mut plans = PendingVisibilityRepairPlans::default();
        let (token, _) = plans.issue(&prepared, Utc::now()).unwrap();
        let consumed = plans.consume(&token, Utc::now()).unwrap();
        assert!(plans.consume(&token, Utc::now()).is_err());
        fs::OpenOptions::new()
            .append(true)
            .open(rollout)
            .unwrap()
            .write_all(b"{\"type\":\"event_msg\"}\n")
            .unwrap();
        let changed =
            prepare_visibility_repair(&root, VisibilityRepairMode::Quick, None).unwrap();
        assert_ne!(changed.revision, consumed.expected_revision);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn visibility_repair_rejects_unknown_required_state_column_for_missing_row() {
        let root = test_root("visibility-repair-schema");
        fs::create_dir_all(&root).unwrap();
        let session_id = "thread-visibility-schema";
        let (_rollout, _cwd, _index) =
            write_visibility_repair_fixture(&root, session_id, true);
        let legacy = Connection::open(root.join("state_4.sqlite")).unwrap();
        legacy
            .execute_batch("ALTER TABLE threads ADD COLUMN future_required TEXT NOT NULL;")
            .unwrap();
        drop(legacy);

        let error = prepare_visibility_repair(&root, VisibilityRepairMode::Quick, None)
            .unwrap_err();
        assert!(error.contains("无法安全推导的必填列"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_paths_cannot_escape_codex_home() {
        assert!(safe_relative_path("sessions/2026/rollout-a.jsonl").is_some());
        assert!(safe_relative_path("").is_none());
        assert!(safe_relative_path("../auth.json").is_none());
        assert!(safe_relative_path("/outside/rollout.jsonl").is_none());
        assert!(safe_relative_path("C:/outside/rollout.jsonl").is_none());
        assert!(safe_relative_path(r"C:\outside\rollout.jsonl").is_none());
        assert!(safe_relative_path("C:outside/rollout.jsonl").is_none());
        assert!(safe_relative_path(r"\\server\share\rollout.jsonl").is_none());
    }

    #[test]
    fn scans_searches_and_rewrites_compressed_rollouts() {
        let root = test_root("compressed");
        let session_dir = root.join("sessions/2026/08/09");
        fs::create_dir_all(&session_dir).unwrap();
        let rollout = session_dir.join("rollout-thread-z.jsonl.zst");
        let source = concat!(
            r#"{"timestamp":"2026-08-09T10:00:00Z","type":"session_meta","payload":{"#,
            r#""id":"thread-z","cwd":"F:\\projects\\zeta","model_provider":"openai"}}"#,
            "\n",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"#,
            r#""total_token_usage":{"input_tokens":20,"output_tokens":7,"total_tokens":27}}}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"#,
            r#""type":"input_text","text":"Compressed Zeta history"}]}}"#,
            "\n",
        );
        let output = File::create(&rollout).unwrap();
        let mut encoder = zstd::stream::write::Encoder::new(output, 3).unwrap();
        encoder.write_all(source.as_bytes()).unwrap();
        encoder.finish().unwrap();

        let snapshots = gather_snapshots(&root).unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].path, rollout);
        assert_eq!(token_totals(&snapshots[0].path), Some((20, 7, 27)));
        assert_eq!(
            locate_rollout_text(&snapshots[0].path, "zeta")
                .unwrap()
                .as_deref(),
            Some("Compressed Zeta history")
        );

        let meta = first_rollout_value(&snapshots[0].path).unwrap().unwrap();
        assert_eq!(meta["payload"]["model_provider"], "openai");
        assert!(locate_rollout_text(&snapshots[0].path, "compressed")
            .unwrap()
            .is_some());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn recycle_visibility_roundtrips_the_codex_state_row() {
        let root = test_root("state-visibility");
        fs::create_dir_all(&root).unwrap();
        let state_db = root.join("state_5.sqlite");
        let connection = Connection::open(&state_db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT NOT NULL,
                    archived INTEGER NOT NULL,
                    archived_at INTEGER,
                    preview TEXT NOT NULL
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, rollout_path, archived, archived_at, preview) VALUES (?1, ?2, 0, NULL, ?3)",
                params!["thread-a", "sessions/rollout-thread-a.jsonl", "Visible thread"],
            )
            .unwrap();
        drop(connection);

        let visibility = state_visibility_snapshot(Some(&state_db), "thread-a")
            .unwrap()
            .unwrap();
        hide_thread_in_state(
            Some(&state_db),
            "thread-a",
            &root.join("bin/rollout-thread-a.jsonl"),
        )
        .unwrap();
        let hidden = state_visibility_snapshot(Some(&state_db), "thread-a")
            .unwrap()
            .unwrap();
        assert_eq!(hidden.archived, 1);
        assert!(hidden.preview.is_empty());

        restore_thread_visibility(Some(&state_db), "thread-a", Some(&visibility)).unwrap();
        let restored = state_visibility_snapshot(Some(&state_db), "thread-a")
            .unwrap()
            .unwrap();
        assert_eq!(restored.archived, 0);
        assert_eq!(restored.preview, "Visible thread");
        assert_eq!(restored.rollout_path, "sessions/rollout-thread-a.jsonl");

        fs::remove_dir_all(&root).unwrap();
    }

    fn write_thread_trash_fixture(root: &Path, session_id: &str) -> PathBuf {
        let session_dir = root.join("sessions/2026/09/02");
        fs::create_dir_all(&session_dir).unwrap();
        let rollout = session_dir.join(format!("rollout-{session_id}.jsonl"));
        fs::write(
            &rollout,
            format!(
                "{{\"timestamp\":\"2026-09-02T01:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"/tmp/project\",\"model_provider\":\"openai\"}}}}\n"
            ),
        )
        .unwrap();
        fs::write(
            root.join(INDEX_NAME),
            format!("{{\"id\":\"{session_id}\",\"thread_name\":\"Trash fixture\"}}\n"),
        )
        .unwrap();
        let state_db = root.join("state_5.sqlite");
        let connection = Connection::open(&state_db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT NOT NULL,
                    archived INTEGER NOT NULL,
                    archived_at INTEGER,
                    preview TEXT NOT NULL
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (id, rollout_path, archived, archived_at, preview)
                 VALUES (?1, ?2, 0, NULL, 'Visible fixture')",
                params![session_id, rollout.to_string_lossy()],
            )
            .unwrap();
        rollout
    }

    #[test]
    fn thread_trash_confirmation_tokens_are_one_time_and_expire() {
        let now = Utc::now();
        let mut plans = PendingThreadTrashPlans::default();
        let (token, _) = plans
            .issue(vec!["thread-a".to_string()], "revision-a".to_string(), now)
            .unwrap();
        let plan = plans.consume(&token, now).unwrap();
        assert_eq!(plan.session_ids, vec!["thread-a"]);
        assert_eq!(plan.expected_revision, "revision-a");
        assert!(plans.consume(&token, now).unwrap_err().contains("失效"));

        let (expired, _) = plans
            .issue(vec!["thread-b".to_string()], "revision-b".to_string(), now)
            .unwrap();
        assert!(plans
            .consume(
                &expired,
                now + chrono::Duration::seconds(THREAD_TRASH_PLAN_TTL_SECONDS + 1),
            )
            .unwrap_err()
            .contains("过期"));
        assert!(validate_thread_trash_plan_token("wrong-prefix").is_err());
    }

    #[test]
    fn thread_trash_legacy_manifests_ignore_the_redundant_absolute_path() {
        let manifest: BinManifest = serde_json::from_value(json!({
            "sessionId": "thread-legacy-bin",
            "title": "Legacy bin entry",
            "cwd": "/tmp/project",
            "originalRolloutPath": "/private/legacy/rollout.jsonl",
            "relativeRolloutPath": "sessions/rollout.jsonl",
            "sessionIndexEntry": { "id": "thread-legacy-bin" },
            "deletedAt": "2026-09-02T00:00:00Z",
            "stateVisibility": null
        }))
        .unwrap();
        assert_eq!(manifest.session_id, "thread-legacy-bin");
        assert_eq!(manifest.relative_rollout_path, "sessions/rollout.jsonl");
    }

    #[test]
    fn thread_trash_revision_detects_content_changes_after_preview() {
        let root = test_root("trash-revision");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-revision");
        let first = prepare_thread_trash(&root, vec!["thread-revision".to_string()]).unwrap();
        let first_revision = first.revision.clone();
        let mut file = fs::OpenOptions::new().append(true).open(&rollout).unwrap();
        file.write_all(b"{\"type\":\"event_msg\",\"payload\":{}}\n")
            .unwrap();
        file.flush().unwrap();
        let error = execute_thread_trash_transaction(
            &root,
            &root.join("trash"),
            first,
            || Ok(()),
        )
        .unwrap_err();
        assert!(error.contains("未移动任何会话"));
        assert!(rollout.is_file());
        let second = prepare_thread_trash(&root, vec!["thread-revision".to_string()]).unwrap();
        assert_ne!(first_revision, second.revision);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_trash_precondition_detects_index_removal_before_mutation() {
        let root = test_root("trash-index-race");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-index-race");
        let prepared =
            prepare_thread_trash(&root, vec!["thread-index-race".to_string()]).unwrap();
        fs::remove_file(root.join(INDEX_NAME)).unwrap();

        let error = execute_thread_trash_transaction(
            &root,
            &root.join("trash"),
            prepared,
            || Ok(()),
        )
        .unwrap_err();
        assert!(error.contains("未移动任何会话"));
        assert!(rollout.is_file());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_trash_transaction_commits_a_recoverable_manifest_without_redundant_path() {
        let root = test_root("trash-commit");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-commit");
        let trash_root = root.join("trash");
        let prepared = prepare_thread_trash(&root, vec!["thread-commit".to_string()]).unwrap();
        let report =
            execute_thread_trash_transaction(&root, &trash_root, prepared, || Ok(())).unwrap();

        assert_eq!(report.requested_count, 1);
        assert_eq!(report.affected_count, 1);
        assert!(!rollout.exists());
        assert!(!index_values(&root).unwrap().contains_key("thread-commit"));
        let hidden = state_visibility_snapshot(latest_state_db(&root).as_deref(), "thread-commit")
            .unwrap()
            .unwrap();
        assert_eq!(hidden.archived, 1);
        assert!(hidden.preview.is_empty());

        let batch = fs::read_dir(&trash_root)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let folder = fs::read_dir(batch).unwrap().next().unwrap().unwrap().path();
        let manifest: Value =
            serde_json::from_slice(&fs::read(folder.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["sessionId"], "thread-commit");
        assert!(manifest.get("originalRolloutPath").is_none());
        assert!(folder
            .join("files/sessions/2026/09/02/rollout-thread-commit.jsonl")
            .is_file());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_trash_transaction_rolls_back_all_surfaces_on_admission_loss() {
        let root = test_root("trash-rollback");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-rollback");
        let original_index = fs::read_to_string(root.join(INDEX_NAME)).unwrap();
        let original_visibility =
            state_visibility_snapshot(latest_state_db(&root).as_deref(), "thread-rollback")
                .unwrap();
        let prepared = prepare_thread_trash(&root, vec!["thread-rollback".to_string()]).unwrap();
        let trash_root = root.join("trash");
        let mut admissions = 0;
        let error = execute_thread_trash_transaction(&root, &trash_root, prepared, || {
            admissions += 1;
            if admissions == 3 {
                Err("injected lease loss".to_string())
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert!(error.contains("自动恢复"));
        assert!(rollout.is_file());
        assert_eq!(
            fs::read_to_string(root.join(INDEX_NAME)).unwrap(),
            original_index
        );
        assert_eq!(
            state_visibility_snapshot(latest_state_db(&root).as_deref(), "thread-rollback")
                .unwrap(),
            original_visibility
        );
        assert_eq!(fs::read_dir(&trash_root).unwrap().count(), 0);

        fs::remove_dir_all(root).unwrap();
    }

    fn move_thread_fixture_to_trash(
        root: &Path,
        trash_root: &Path,
        session_id: &str,
    ) -> (PathBuf, PathBuf, Option<StateVisibilitySnapshot>) {
        let rollout = write_thread_trash_fixture(root, session_id);
        let original_visibility =
            state_visibility_snapshot(latest_state_db(root).as_deref(), session_id).unwrap();
        let prepared = prepare_thread_trash(root, vec![session_id.to_string()]).unwrap();
        execute_thread_trash_transaction(root, trash_root, prepared, || Ok(())).unwrap();
        let entry = collect_bin_entries_from_root(trash_root)
            .unwrap()
            .into_iter()
            .find(|entry| entry.manifest.session_id == session_id)
            .unwrap();
        (rollout, entry.rollouts[0].clone(), original_visibility)
    }

    #[test]
    fn thread_restore_confirmation_tokens_are_one_time_and_expire() {
        let now = Utc::now();
        let mut plans = PendingThreadRestorePlans::default();
        let (token, _) = plans
            .issue(vec!["thread-a".to_string()], "revision-a".to_string(), now)
            .unwrap();
        let plan = plans.consume(&token, now).unwrap();
        assert_eq!(plan.session_ids, vec!["thread-a"]);
        assert_eq!(plan.expected_revision, "revision-a");
        assert!(plans.consume(&token, now).unwrap_err().contains("失效"));

        let (expired, _) = plans
            .issue(vec!["thread-b".to_string()], "revision-b".to_string(), now)
            .unwrap();
        assert!(plans
            .consume(
                &expired,
                now + chrono::Duration::seconds(THREAD_RESTORE_PLAN_TTL_SECONDS + 1),
            )
            .unwrap_err()
            .contains("过期"));
        assert!(validate_thread_restore_plan_token("wrong-prefix").is_err());
    }

    #[test]
    fn thread_restore_transaction_commits_files_index_state_and_removes_bin_entry() {
        let root = test_root("restore-commit");
        fs::create_dir_all(&root).unwrap();
        let trash_root = root.join("trash");
        let (rollout, source, original_visibility) =
            move_thread_fixture_to_trash(&root, &trash_root, "thread-restore-commit");
        assert!(!rollout.exists());
        assert!(source.is_file());

        let prepared = prepare_thread_restore(
            &root,
            &trash_root,
            vec!["thread-restore-commit".to_string()],
        )
        .unwrap();
        let report = execute_thread_restore_transaction(&root, prepared, || Ok(())).unwrap();

        assert_eq!(report.requested_count, 1);
        assert_eq!(report.affected_count, 1);
        assert!(rollout.is_file());
        assert!(!source.exists());
        assert_eq!(
            index_values(&root).unwrap()["thread-restore-commit"]["thread_name"],
            "Trash fixture"
        );
        assert_eq!(
            state_visibility_snapshot(latest_state_db(&root).as_deref(), "thread-restore-commit")
                .unwrap(),
            original_visibility
        );
        assert!(collect_bin_entries_from_root(&trash_root)
            .unwrap()
            .is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_restore_precondition_detects_bin_content_changes_without_overwrite() {
        let root = test_root("restore-revision");
        fs::create_dir_all(&root).unwrap();
        let trash_root = root.join("trash");
        let (rollout, source, _) =
            move_thread_fixture_to_trash(&root, &trash_root, "thread-restore-revision");
        let prepared = prepare_thread_restore(
            &root,
            &trash_root,
            vec!["thread-restore-revision".to_string()],
        )
        .unwrap();
        let mut file = fs::OpenOptions::new().append(true).open(&source).unwrap();
        file.write_all(b"{\"type\":\"event_msg\",\"payload\":{}}\n")
            .unwrap();
        file.flush().unwrap();

        let error = execute_thread_restore_transaction(&root, prepared, || Ok(())).unwrap_err();
        assert!(error.contains("未恢复任何会话"));
        assert!(!rollout.exists());
        assert!(source.is_file());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_restore_preview_rejects_existing_destination_without_overwrite() {
        let root = test_root("restore-conflict");
        fs::create_dir_all(&root).unwrap();
        let trash_root = root.join("trash");
        let (rollout, source, _) =
            move_thread_fixture_to_trash(&root, &trash_root, "thread-restore-conflict");
        fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        fs::write(&rollout, b"do-not-overwrite").unwrap();

        let error = prepare_thread_restore(
            &root,
            &trash_root,
            vec!["thread-restore-conflict".to_string()],
        )
        .unwrap_err();
        assert!(error.contains("避免覆盖"));
        assert_eq!(fs::read(&rollout).unwrap(), b"do-not-overwrite");
        assert!(source.is_file());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_restore_transaction_rolls_back_all_surfaces_on_admission_loss() {
        let root = test_root("restore-rollback");
        fs::create_dir_all(&root).unwrap();
        let trash_root = root.join("trash");
        let (rollout, source, _) =
            move_thread_fixture_to_trash(&root, &trash_root, "thread-restore-rollback");
        let hidden_visibility =
            state_visibility_snapshot(latest_state_db(&root).as_deref(), "thread-restore-rollback")
                .unwrap();
        let original_index = fs::read_to_string(root.join(INDEX_NAME)).unwrap();
        let prepared = prepare_thread_restore(
            &root,
            &trash_root,
            vec!["thread-restore-rollback".to_string()],
        )
        .unwrap();
        let mut admissions = 0;
        let error = execute_thread_restore_transaction(&root, prepared, || {
            admissions += 1;
            if admissions == 3 {
                Err("injected restore lease loss".to_string())
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert!(error.contains("自动还原"));
        assert!(!rollout.exists());
        assert!(source.is_file());
        assert_eq!(
            fs::read_to_string(root.join(INDEX_NAME)).unwrap(),
            original_index
        );
        assert_eq!(
            state_visibility_snapshot(latest_state_db(&root).as_deref(), "thread-restore-rollback")
                .unwrap(),
            hidden_visibility
        );
        assert_eq!(collect_bin_entries_from_root(&trash_root).unwrap().len(), 1);

        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn thread_archive_confirmation_tokens_are_one_time_and_expire() {
        let now = Utc::now();
        let mut plans = PendingThreadArchivePlans::default();
        let (token, _) = plans
            .issue(vec!["thread-a".to_string()], "revision-a".to_string(), now)
            .unwrap();
        let plan = plans.consume(&token, now).unwrap();
        assert_eq!(plan.session_ids, vec!["thread-a"]);
        assert_eq!(plan.expected_revision, "revision-a");
        assert!(plans.consume(&token, now).unwrap_err().contains("失效"));

        let (expired, _) = plans
            .issue(vec!["thread-b".to_string()], "revision-b".to_string(), now)
            .unwrap();
        assert!(plans
            .consume(
                &expired,
                now + chrono::Duration::seconds(THREAD_ARCHIVE_PLAN_TTL_SECONDS + 1),
            )
            .unwrap_err()
            .contains("过期"));
        assert!(validate_thread_archive_plan_token("wrong-prefix").is_err());
    }

    fn apply_archive_transaction_fixture(root: &Path, transaction: &ArchiveTransaction) {
        for item in &transaction.manifest.items {
            for file in &item.files {
                relocate_archive_file_without_replace(
                    root,
                    &root.join(&file.source_relative),
                    &root.join(&file.target_relative),
                    &file.content_revision,
                )
                .unwrap();
            }
            for visibility in &item.visibilities {
                restore_thread_visibility(
                    Some(&root.join(&visibility.state_database)),
                    &item.session_id,
                    visibility.archived.as_ref(),
                )
                .unwrap();
            }
        }
    }

    fn append_thread_archive_fixture(root: &Path, session_id: &str) -> PathBuf {
        let session_dir = root.join("sessions/2026/09/02");
        fs::create_dir_all(&session_dir).unwrap();
        let rollout = session_dir.join(format!("rollout-{session_id}.jsonl"));
        fs::write(
            &rollout,
            format!(
                "{{\"timestamp\":\"2026-09-02T01:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"/tmp/project\",\"model_provider\":\"openai\"}}}}\n"
            ),
        )
        .unwrap();
        let mut index = fs::OpenOptions::new()
            .append(true)
            .open(root.join(INDEX_NAME))
            .unwrap();
        writeln!(
            index,
            "{{\"id\":\"{session_id}\",\"thread_name\":\"Archive fixture\"}}"
        )
        .unwrap();
        Connection::open(root.join("state_5.sqlite"))
            .unwrap()
            .execute(
                "INSERT INTO threads (id, rollout_path, archived, archived_at, preview)
                 VALUES (?1, ?2, 0, NULL, 'Visible fixture')",
                params![session_id, rollout.to_string_lossy()],
            )
            .unwrap();
        rollout
    }

    fn add_sqlite_state_database_fixture(root: &Path) -> PathBuf {
        let sqlite_directory = root.join("sqlite");
        fs::create_dir_all(&sqlite_directory).unwrap();
        let state_database = sqlite_directory.join("state_5.sqlite");
        fs::copy(root.join("state_5.sqlite"), &state_database).unwrap();
        state_database
    }

    #[test]
    fn thread_archive_transaction_commits_files_state_and_preserves_index() {
        let root = test_root("archive-commit");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-archive-commit");
        let target = root.join("archived_sessions/2026/09/02/rollout-thread-archive-commit.jsonl");
        let original_index = fs::read_to_string(root.join(INDEX_NAME)).unwrap();
        let transaction_root = root.join("archive-transactions");
        let prepared =
            prepare_thread_archive(&root, vec!["thread-archive-commit".to_string()]).unwrap();

        let report =
            execute_thread_archive_transaction(&root, &transaction_root, prepared, || Ok(()))
                .unwrap();

        assert_eq!(report.requested_count, 1);
        assert_eq!(report.affected_count, 1);
        assert!(!rollout.exists());
        assert!(target.is_file());
        assert_eq!(
            fs::read_to_string(root.join(INDEX_NAME)).unwrap(),
            original_index
        );
        let archived =
            state_visibility_snapshot(latest_state_db(&root).as_deref(), "thread-archive-commit")
                .unwrap()
                .unwrap();
        assert_eq!(archived.archived, 1);
        assert!(archived.archived_at.is_some());
        assert_eq!(archived.rollout_path, target.to_string_lossy());
        assert_eq!(archived.preview, "Visible fixture");
        assert_eq!(
            rollout_status(&gather_snapshots(&root).unwrap()[0].relative_path),
            Some("archived")
        );
        assert!(fs::read_dir(&transaction_root).unwrap().next().is_none());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_archive_moves_a_compressed_rollout_and_keeps_the_logical_state_path() {
        let root = test_root("archive-compressed");
        fs::create_dir_all(&root).unwrap();
        let logical_source = write_thread_trash_fixture(&root, "thread-archive-compressed");
        let compressed_source = logical_source.with_extension("jsonl.zst");
        let output = File::create(&compressed_source).unwrap();
        let mut encoder = zstd::stream::write::Encoder::new(output, 3).unwrap();
        encoder.write_all(&fs::read(&logical_source).unwrap()).unwrap();
        encoder.finish().unwrap();
        fs::remove_file(&logical_source).unwrap();
        let compressed_target = root
            .join("archived_sessions/2026/09/02/rollout-thread-archive-compressed.jsonl.zst");
        let logical_target = root
            .join("archived_sessions/2026/09/02/rollout-thread-archive-compressed.jsonl");
        let transaction_root = root.join("archive-transactions");
        let prepared =
            prepare_thread_archive(&root, vec!["thread-archive-compressed".to_string()]).unwrap();

        execute_thread_archive_transaction(&root, &transaction_root, prepared, || Ok(())).unwrap();

        assert!(!compressed_source.exists());
        assert!(!logical_source.exists());
        assert!(compressed_target.is_file());
        let archived = state_visibility_snapshot(
            latest_state_db(&root).as_deref(),
            "thread-archive-compressed",
        )
        .unwrap()
        .unwrap();
        assert_eq!(archived.archived, 1);
        assert_eq!(archived.rollout_path, logical_target.to_string_lossy());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_archive_updates_current_and_legacy_state_databases_atomically() {
        let root = test_root("archive-multiple-state-databases");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-archive-multi-db");
        let sqlite_state_database = add_sqlite_state_database_fixture(&root);
        let legacy_state_database = root.join("state_5.sqlite");
        let target =
            root.join("archived_sessions/2026/09/02/rollout-thread-archive-multi-db.jsonl");
        let transaction_root = root.join("archive-transactions");
        let prepared =
            prepare_thread_archive(&root, vec!["thread-archive-multi-db".to_string()]).unwrap();

        assert_eq!(
            prepared.state_databases,
            vec![sqlite_state_database.clone(), legacy_state_database.clone()]
        );
        execute_thread_archive_transaction(&root, &transaction_root, prepared, || Ok(())).unwrap();

        assert!(!rollout.exists());
        assert!(target.is_file());
        for state_database in [sqlite_state_database, legacy_state_database] {
            let archived = state_visibility_snapshot(
                Some(&state_database),
                "thread-archive-multi-db",
            )
            .unwrap()
            .unwrap();
            assert_eq!(archived.archived, 1);
            assert_eq!(archived.rollout_path, target.to_string_lossy());
        }
        assert!(fs::read_dir(&transaction_root).unwrap().next().is_none());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_archive_precondition_detects_content_changes_without_mutation() {
        let root = test_root("archive-revision");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-archive-revision");
        let transaction_root = root.join("archive-transactions");
        let prepared =
            prepare_thread_archive(&root, vec!["thread-archive-revision".to_string()]).unwrap();
        let mut file = fs::OpenOptions::new().append(true).open(&rollout).unwrap();
        file.write_all(b"{\"type\":\"event_msg\",\"payload\":{}}\n")
            .unwrap();
        file.flush().unwrap();

        let error =
            execute_thread_archive_transaction(&root, &transaction_root, prepared, || Ok(()))
                .unwrap_err();

        assert!(error.contains("自动恢复"), "{error}");
        assert!(rollout.is_file());
        assert!(!root.join("archived_sessions").exists());
        assert_eq!(
            state_visibility_snapshot(
                latest_state_db(&root).as_deref(),
                "thread-archive-revision",
            )
            .unwrap()
            .unwrap()
            .archived,
            0
        );
        assert!(fs::read_dir(&transaction_root).unwrap().next().is_none());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_archive_preview_rejects_existing_destination_without_overwrite() {
        let root = test_root("archive-conflict");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-archive-conflict");
        let target =
            root.join("archived_sessions/2026/09/02/rollout-thread-archive-conflict.jsonl");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"do-not-overwrite").unwrap();

        let error =
            prepare_thread_archive(&root, vec!["thread-archive-conflict".to_string()]).unwrap_err();

        assert!(error.contains("避免覆盖"));
        assert!(rollout.is_file());
        assert_eq!(fs::read(&target).unwrap(), b"do-not-overwrite");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_archive_transaction_rolls_back_on_admission_loss() {
        let root = test_root("archive-rollback");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-archive-rollback");
        let sqlite_state_database = add_sqlite_state_database_fixture(&root);
        let original_visibility =
            state_visibility_snapshot(latest_state_db(&root).as_deref(), "thread-archive-rollback")
                .unwrap();
        let original_sqlite_visibility = state_visibility_snapshot(
            Some(&sqlite_state_database),
            "thread-archive-rollback",
        )
        .unwrap();
        let original_index = fs::read_to_string(root.join(INDEX_NAME)).unwrap();
        let transaction_root = root.join("archive-transactions");
        let prepared =
            prepare_thread_archive(&root, vec!["thread-archive-rollback".to_string()]).unwrap();
        let mut admissions = 0;

        let error = execute_thread_archive_transaction(&root, &transaction_root, prepared, || {
            admissions += 1;
            if admissions == 3 {
                Err("injected archive lease loss".to_string())
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert!(error.contains("自动恢复"), "{error}");
        assert!(rollout.is_file());
        assert!(!root
            .join("archived_sessions/2026/09/02/rollout-thread-archive-rollback.jsonl")
            .exists());
        assert_eq!(
            state_visibility_snapshot(
                latest_state_db(&root).as_deref(),
                "thread-archive-rollback",
            )
            .unwrap(),
            original_visibility
        );
        assert_eq!(
            state_visibility_snapshot(
                Some(&sqlite_state_database),
                "thread-archive-rollback",
            )
            .unwrap(),
            original_sqlite_visibility
        );
        assert_eq!(
            fs::read_to_string(root.join(INDEX_NAME)).unwrap(),
            original_index
        );
        assert!(fs::read_dir(&transaction_root).unwrap().next().is_none());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_archive_batch_rolls_back_every_item_when_admission_is_lost_mid_batch() {
        let root = test_root("archive-batch-rollback");
        fs::create_dir_all(&root).unwrap();
        let first = write_thread_trash_fixture(&root, "thread-archive-batch-a");
        let second = append_thread_archive_fixture(&root, "thread-archive-batch-b");
        let original_index = fs::read_to_string(root.join(INDEX_NAME)).unwrap();
        let transaction_root = root.join("archive-transactions");
        let prepared = prepare_thread_archive(
            &root,
            vec![
                "thread-archive-batch-a".to_string(),
                "thread-archive-batch-b".to_string(),
            ],
        )
        .unwrap();
        let mut admissions = 0;

        let error = execute_thread_archive_transaction(
            &root,
            &transaction_root,
            prepared,
            || {
                admissions += 1;
                if admissions == 3 {
                    Err("injected mid-batch lease loss".to_string())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();

        assert!(error.contains("自动恢复"), "{error}");
        assert!(first.is_file());
        assert!(second.is_file());
        assert!(!root
            .join("archived_sessions")
            .join("2026/09/02/rollout-thread-archive-batch-a.jsonl")
            .exists());
        assert!(!root
            .join("archived_sessions")
            .join("2026/09/02/rollout-thread-archive-batch-b.jsonl")
            .exists());
        assert_eq!(
            fs::read_to_string(root.join(INDEX_NAME)).unwrap(),
            original_index
        );
        assert!(fs::read_dir(&transaction_root).unwrap().next().is_none());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_thread_archive_is_rolled_back_after_restart() {
        let root = test_root("archive-prepared-recovery");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-archive-prepared");
        let original_visibility =
            state_visibility_snapshot(latest_state_db(&root).as_deref(), "thread-archive-prepared")
                .unwrap();
        let transaction_root = root.join("archive-transactions");
        let prepared =
            prepare_thread_archive(&root, vec!["thread-archive-prepared".to_string()]).unwrap();
        let transaction = create_archive_transaction(&transaction_root, &root, &prepared).unwrap();
        apply_archive_transaction_fixture(&root, &transaction);
        assert!(!rollout.exists());

        recover_incomplete_archive_transactions_with_revalidate(
            &root,
            &transaction_root,
            || Ok(()),
        )
        .unwrap();

        assert!(rollout.is_file());
        assert_eq!(
            state_visibility_snapshot(
                latest_state_db(&root).as_deref(),
                "thread-archive-prepared",
            )
            .unwrap(),
            original_visibility
        );
        assert!(fs::read_dir(&transaction_root).unwrap().next().is_none());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_thread_archive_recovers_a_crash_between_link_and_source_removal() {
        let root = test_root("archive-hardlink-recovery");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-archive-hardlink");
        let transaction_root = root.join("archive-transactions");
        let prepared =
            prepare_thread_archive(&root, vec!["thread-archive-hardlink".to_string()]).unwrap();
        let transaction =
            create_archive_transaction(&transaction_root, &root, &prepared).unwrap();
        let file = &transaction.manifest.items[0].files[0];
        let target = root.join(&file.target_relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::hard_link(&rollout, &target).unwrap();

        recover_incomplete_archive_transactions_with_revalidate(
            &root,
            &transaction_root,
            || Ok(()),
        )
        .unwrap();

        assert!(rollout.is_file());
        assert!(!target.exists());
        assert_eq!(
            state_visibility_snapshot(
                latest_state_db(&root).as_deref(),
                "thread-archive-hardlink",
            )
            .unwrap()
            .unwrap()
            .archived,
            0
        );
        assert!(fs::read_dir(&transaction_root).unwrap().next().is_none());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_thread_archive_preserves_both_files_when_recovery_finds_a_conflict() {
        let root = test_root("archive-recovery-conflict");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-archive-recovery-conflict");
        let transaction_root = root.join("archive-transactions");
        let prepared = prepare_thread_archive(
            &root,
            vec!["thread-archive-recovery-conflict".to_string()],
        )
        .unwrap();
        let transaction = create_archive_transaction(&transaction_root, &root, &prepared).unwrap();
        let file = &transaction.manifest.items[0].files[0];
        let target = root.join(&file.target_relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"independent archive content").unwrap();

        let error = recover_incomplete_archive_transactions_with_revalidate(
            &root,
            &transaction_root,
            || Ok(()),
        )
        .unwrap_err();

        assert!(error.contains("内容冲突"), "{error}");
        assert!(rollout.is_file());
        assert_eq!(fs::read(&target).unwrap(), b"independent archive content");
        assert!(transaction.directory.is_dir());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_thread_archive_finishes_cleanup_after_restart() {
        let root = test_root("archive-committed-recovery");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-archive-committed");
        let target =
            root.join("archived_sessions/2026/09/02/rollout-thread-archive-committed.jsonl");
        let transaction_root = root.join("archive-transactions");
        let prepared =
            prepare_thread_archive(&root, vec!["thread-archive-committed".to_string()]).unwrap();
        let mut transaction =
            create_archive_transaction(&transaction_root, &root, &prepared).unwrap();
        apply_archive_transaction_fixture(&root, &transaction);
        transaction.manifest.state = ArchiveTransactionState::Committed;
        write_archive_transaction_manifest(&transaction.directory, &transaction.manifest).unwrap();

        recover_incomplete_archive_transactions_with_revalidate(
            &root,
            &transaction_root,
            || Ok(()),
        )
        .unwrap();

        assert!(!rollout.exists());
        assert!(target.is_file());
        assert_eq!(
            state_visibility_snapshot(
                latest_state_db(&root).as_deref(),
                "thread-archive-committed",
            )
            .unwrap()
            .unwrap()
            .archived,
            1
        );
        assert!(fs::read_dir(&transaction_root).unwrap().next().is_none());

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn thread_archive_rejects_a_symlinked_archive_root() {
        use std::os::unix::fs::symlink;

        let root = test_root("archive-root-symlink");
        fs::create_dir_all(&root).unwrap();
        let rollout = write_thread_trash_fixture(&root, "thread-archive-symlink");
        let outside = test_root("archive-root-symlink-outside");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("archived_sessions")).unwrap();

        let error =
            prepare_thread_archive(&root, vec!["thread-archive-symlink".to_string()]).unwrap_err();

        assert!(error.contains("归档会话目录类型不安全"));
        assert!(rollout.is_file());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());

        fs::remove_file(root.join("archived_sessions")).unwrap();
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    fn thread_purge_test_paths(root: &Path) -> crate::storage::Paths {
        let app_data = root.join("app-data");
        crate::storage::Paths {
            codex_home: root.to_path_buf(),
            current_auth: root.join("auth.json"),
            current_config: root.join("config.toml"),
            accounts: app_data.join("accounts"),
            providers: app_data.join("providers"),
            config_backup: app_data.join("config-before-provider.toml"),
            state_file: app_data.join("state.json"),
        }
    }

    fn seed_thread_purge_ownership(paths: &crate::storage::Paths, session_id: &str) {
        let mut state = crate::models::ManagerStateFile::default();
        state
            .conversation_account_ids
            .insert(session_id.to_string(), "managed-account-a".to_string());
        state
            .observed_conversation_ids
            .insert(session_id.to_string());
        crate::storage::write_state(paths, &state).unwrap();
    }

    fn add_thread_purge_related_state(root: &Path, session_id: &str) {
        let state = Connection::open(root.join("state_5.sqlite")).unwrap();
        state
            .execute_batch(
                "CREATE TABLE thread_dynamic_tools (
                    thread_id TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    name TEXT NOT NULL,
                    PRIMARY KEY (thread_id, position)
                );",
            )
            .unwrap();
        state
            .execute(
                "INSERT INTO thread_dynamic_tools VALUES (?1, 0, 'fixture-tool')",
                params![session_id],
            )
            .unwrap();
        drop(state);

        let catalog_dir = root.join("sqlite");
        fs::create_dir_all(&catalog_dir).unwrap();
        let catalog = Connection::open(catalog_dir.join("codex.db")).unwrap();
        catalog
            .execute_batch(
                "CREATE TABLE local_thread_catalog (
                    thread_id TEXT PRIMARY KEY,
                    model_provider TEXT NOT NULL
                );",
            )
            .unwrap();
        catalog
            .execute(
                "INSERT INTO local_thread_catalog VALUES (?1, 'openai')",
                params![session_id],
            )
            .unwrap();
    }

    fn apply_thread_purge_mutation(paths: &crate::storage::Paths, prepared: &PreparedThreadPurge) {
        for item in &prepared.items {
            purge_thread_state(&paths.codex_home, &item.session_id).unwrap();
        }
        let mut state = crate::storage::read_state(paths);
        for item in &prepared.items {
            state.conversation_account_ids.remove(&item.session_id);
            state.observed_conversation_ids.remove(&item.session_id);
            for bin in &item.bins {
                fs::remove_dir_all(&bin.snapshot.folder).unwrap();
            }
        }
        crate::storage::write_state(paths, &state).unwrap();
    }

    #[derive(Default)]
    struct ThreadPurgeStoppedViewer;

    impl capacity_mutation::LegacyViewerProbe for ThreadPurgeStoppedViewer {
        fn legacy_viewer_state(&mut self) -> capacity_mutation::LegacyViewerState {
            capacity_mutation::LegacyViewerState::NotRunning
        }
    }

    fn thread_purge_lease(
        root: &Path,
        probe: &mut ThreadPurgeStoppedViewer,
    ) -> capacity_mutation::MutationLock {
        let owner = capacity_mutation::MutationLockOwner::new(
            capacity_mutation::MutationOwnerId::parse(
                "mutation-owner:v1:00000000-0000-4000-8000-000000000052",
            )
            .unwrap(),
            std::process::id(),
            capacity_domain::UtcTimestamp::parse("2026-09-02T00:00:00Z").unwrap(),
        )
        .unwrap();
        capacity_mutation::MutationLock::acquire(root.join("mutation-lock"), owner, probe).unwrap()
    }

    #[test]
    fn thread_purge_confirmation_tokens_are_one_time_and_expire() {
        let now = Utc::now();
        let mut plans = PendingThreadPurgePlans::default();
        let (token, _) = plans
            .issue(
                vec!["thread-a".to_string()],
                true,
                "revision-a".to_string(),
                now,
            )
            .unwrap();
        let plan = plans.consume(&token, now).unwrap();
        assert_eq!(plan.session_ids, vec!["thread-a"]);
        assert!(plan.empty_bin);
        assert_eq!(plan.expected_revision, "revision-a");
        assert!(plans.consume(&token, now).unwrap_err().contains("失效"));

        let (expired, _) = plans
            .issue(
                vec!["thread-b".to_string()],
                false,
                "revision-b".to_string(),
                now,
            )
            .unwrap();
        assert!(
            plans
                .consume(
                    &expired,
                    now + chrono::Duration::seconds(THREAD_PURGE_PLAN_TTL_SECONDS + 1),
                )
                .unwrap_err()
                .contains("过期")
        );
        assert!(validate_thread_purge_plan_token("wrong-prefix").is_err());
    }

    #[test]
    fn purge_only_database_tables_do_not_expand_import_permissions() {
        assert!(!valid_related_table("state", "threads"));
        assert!(!valid_related_table("catalog:codex.db", "local_thread_catalog"));
        assert!(valid_purge_related_table("state", "threads"));
        assert!(valid_purge_related_table(
            "catalog:codex.db",
            "local_thread_catalog"
        ));
    }

    #[test]
    fn thread_purge_transaction_commits_all_surfaces_and_destroys_restore_payload() {
        let root = test_root("purge-commit");
        fs::create_dir_all(&root).unwrap();
        let trash_root = root.join("trash");
        let restore_root = root.join("purge-restore-points");
        let session_id = "thread-purge-commit";
        let (_, source, _) = move_thread_fixture_to_trash(&root, &trash_root, session_id);
        add_thread_purge_related_state(&root, session_id);
        let paths = thread_purge_test_paths(&root);
        seed_thread_purge_ownership(&paths, session_id);
        let manager_state = crate::storage::read_state(&paths);
        let prepared = prepare_thread_purge(
            &root,
            &trash_root,
            &manager_state,
            vec![session_id.to_string()],
            false,
        )
        .unwrap();

        let report =
            execute_thread_purge_transaction(&paths, &trash_root, &restore_root, prepared, || {
                Ok(())
            })
            .unwrap();

        assert_eq!(report.requested_count, 1);
        assert_eq!(report.affected_count, 1);
        assert_eq!(report.outcomes.len(), 1);
        assert_eq!(report.outcomes[0].session_id, session_id);
        assert_eq!(report.outcomes[0].status, "purged");
        assert!(report.released_bytes > 0);
        assert!(report.restore_point_removed);
        assert!(!source.exists());
        assert!(
            snapshot_purge_related_state(&root, session_id)
                .unwrap()
                .is_empty()
        );
        assert!(
            !crate::storage::read_state(&paths)
                .conversation_account_ids
                .contains_key(session_id)
        );
        assert!(
            collect_bin_entries_from_root(&trash_root)
                .unwrap()
                .is_empty()
        );
        assert!(fs::read_dir(&restore_root).unwrap().next().is_none());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_purge_precondition_detects_bin_content_changes_without_mutation() {
        let root = test_root("purge-revision");
        fs::create_dir_all(&root).unwrap();
        let trash_root = root.join("trash");
        let restore_root = root.join("purge-restore-points");
        let session_id = "thread-purge-revision";
        let (_, source, _) = move_thread_fixture_to_trash(&root, &trash_root, session_id);
        let paths = thread_purge_test_paths(&root);
        let manager_state = crate::storage::read_state(&paths);
        let prepared = prepare_thread_purge(
            &root,
            &trash_root,
            &manager_state,
            vec![session_id.to_string()],
            false,
        )
        .unwrap();
        let mut file = fs::OpenOptions::new().append(true).open(&source).unwrap();
        file.write_all(b"{\"type\":\"event_msg\",\"payload\":{}}\n")
            .unwrap();
        file.flush().unwrap();

        let error =
            execute_thread_purge_transaction(&paths, &trash_root, &restore_root, prepared, || {
                Ok(())
            })
            .unwrap_err();

        assert!(error.contains("确认期间发生变化"));
        assert!(source.is_file());
        assert!(
            state_visibility_snapshot(latest_state_db(&root).as_deref(), session_id)
                .unwrap()
                .is_some()
        );
        assert!(!restore_root.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_purge_transaction_rolls_back_files_database_and_ownership_on_lease_loss() {
        let root = test_root("purge-rollback");
        fs::create_dir_all(&root).unwrap();
        let trash_root = root.join("trash");
        let restore_root = root.join("purge-restore-points");
        let session_id = "thread-purge-rollback";
        let (_, source, _) = move_thread_fixture_to_trash(&root, &trash_root, session_id);
        add_thread_purge_related_state(&root, session_id);
        let paths = thread_purge_test_paths(&root);
        seed_thread_purge_ownership(&paths, session_id);
        let original_state = snapshot_purge_related_state(&root, session_id).unwrap();
        let manager_state = crate::storage::read_state(&paths);
        let prepared = prepare_thread_purge(
            &root,
            &trash_root,
            &manager_state,
            vec![session_id.to_string()],
            false,
        )
        .unwrap();
        let mut admissions = 0;
        let error =
            execute_thread_purge_transaction(&paths, &trash_root, &restore_root, prepared, || {
                admissions += 1;
                if admissions == 4 {
                    Err("injected purge lease loss".to_string())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();

        assert!(error.contains("自动恢复"));
        assert!(source.is_file());
        assert_eq!(
            canonical_sqlite_snapshot_bytes(
                &snapshot_purge_related_state(&root, session_id).unwrap()
            )
            .unwrap(),
            canonical_sqlite_snapshot_bytes(&original_state).unwrap()
        );
        assert_eq!(
            crate::storage::read_state(&paths)
                .conversation_account_ids
                .get(session_id)
                .map(String::as_str),
            Some("managed-account-a")
        );
        assert!(
            crate::storage::read_state(&paths)
                .observed_conversation_ids
                .contains(session_id)
        );
        assert!(fs::read_dir(&restore_root).unwrap().next().is_none());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_thread_purge_is_recovered_after_interrupted_mutation() {
        let root = test_root("purge-prepared-recovery");
        fs::create_dir_all(&root).unwrap();
        let trash_root = root.join("trash");
        let restore_root = root.join("purge-restore-points");
        let session_id = "thread-purge-prepared-recovery";
        let (_, source, _) = move_thread_fixture_to_trash(&root, &trash_root, session_id);
        add_thread_purge_related_state(&root, session_id);
        let paths = thread_purge_test_paths(&root);
        seed_thread_purge_ownership(&paths, session_id);
        let original_state = snapshot_purge_related_state(&root, session_id).unwrap();
        let manager_state = crate::storage::read_state(&paths);
        let prepared = prepare_thread_purge(
            &root,
            &trash_root,
            &manager_state,
            vec![session_id.to_string()],
            false,
        )
        .unwrap();
        let _point = create_purge_restore_point(&restore_root, &trash_root, &prepared).unwrap();
        apply_thread_purge_mutation(&paths, &prepared);
        assert!(!source.exists());

        let mut probe = ThreadPurgeStoppedViewer;
        let lease = thread_purge_lease(&root, &mut probe);
        recover_incomplete_purge_transactions_with_lease(
            &paths,
            &trash_root,
            &restore_root,
            &lease,
            &mut probe,
        )
        .unwrap();

        assert!(source.is_file());
        assert_eq!(
            canonical_sqlite_snapshot_bytes(
                &snapshot_purge_related_state(&root, session_id).unwrap()
            )
            .unwrap(),
            canonical_sqlite_snapshot_bytes(&original_state).unwrap()
        );
        assert_eq!(
            crate::storage::read_state(&paths)
                .conversation_account_ids
                .get(session_id)
                .map(String::as_str),
            Some("managed-account-a")
        );
        assert!(fs::read_dir(&restore_root).unwrap().next().is_none());

        drop(lease);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_thread_purge_finishes_payload_cleanup_after_restart() {
        let root = test_root("purge-committed-recovery");
        fs::create_dir_all(&root).unwrap();
        let trash_root = root.join("trash");
        let restore_root = root.join("purge-restore-points");
        let session_id = "thread-purge-committed-recovery";
        let (_, source, _) = move_thread_fixture_to_trash(&root, &trash_root, session_id);
        let paths = thread_purge_test_paths(&root);
        seed_thread_purge_ownership(&paths, session_id);
        let manager_state = crate::storage::read_state(&paths);
        let prepared = prepare_thread_purge(
            &root,
            &trash_root,
            &manager_state,
            vec![session_id.to_string()],
            false,
        )
        .unwrap();
        let mut point = create_purge_restore_point(&restore_root, &trash_root, &prepared).unwrap();
        apply_thread_purge_mutation(&paths, &prepared);
        point.manifest.state = PurgeRestoreState::Committed;
        write_purge_restore_manifest(&point.directory, &point.manifest).unwrap();

        let mut probe = ThreadPurgeStoppedViewer;
        let lease = thread_purge_lease(&root, &mut probe);
        recover_incomplete_purge_transactions_with_lease(
            &paths,
            &trash_root,
            &restore_root,
            &lease,
            &mut probe,
        )
        .unwrap();

        assert!(!source.exists());
        assert!(
            snapshot_purge_related_state(&root, session_id)
                .unwrap()
                .is_empty()
        );
        assert!(fs::read_dir(&restore_root).unwrap().next().is_none());

        drop(lease);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thread_purge_rejects_a_target_with_an_unselected_parent_edge() {
        let root = test_root("purge-cross-edge");
        fs::create_dir_all(&root).unwrap();
        let trash_root = root.join("trash");
        let session_id = "thread-purge-child";
        move_thread_fixture_to_trash(&root, &trash_root, session_id);
        let state = Connection::open(root.join("state_5.sqlite")).unwrap();
        state
            .execute_batch(
                "CREATE TABLE thread_spawn_edges (
                    parent_thread_id TEXT NOT NULL,
                    child_thread_id TEXT PRIMARY KEY,
                    status TEXT NOT NULL
                );
                INSERT INTO thread_spawn_edges
                VALUES ('thread-unselected-parent', 'thread-purge-child', 'completed');",
            )
            .unwrap();
        drop(state);

        let error = prepare_thread_purge(
            &root,
            &trash_root,
            &crate::models::ManagerStateFile::default(),
            vec![session_id.to_string()],
            false,
        )
        .unwrap_err();

        assert!(error.contains("跨目标父子关系"));
        assert_eq!(collect_bin_entries_from_root(&trash_root).unwrap().len(), 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn thread_purge_rejects_a_symlinked_trash_root() {
        use std::os::unix::fs::symlink;

        let root = test_root("purge-trash-root-symlink");
        fs::create_dir_all(&root).unwrap();
        let trash_root = root.join("trash");
        let session_id = "thread-purge-symlink";
        move_thread_fixture_to_trash(&root, &trash_root, session_id);
        let actual_trash = root.join("actual-trash");
        fs::rename(&trash_root, &actual_trash).unwrap();
        symlink(&actual_trash, &trash_root).unwrap();

        let error = prepare_thread_purge(
            &root,
            &trash_root,
            &crate::models::ManagerStateFile::default(),
            vec![session_id.to_string()],
            false,
        )
        .unwrap_err();

        assert!(error.contains("回收站目录类型不安全"));

        fs::remove_file(&trash_root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn purging_a_thread_removes_its_codex_catalog_entry() {
        let root = test_root("catalog-purge");
        let catalog_dir = root.join("sqlite");
        fs::create_dir_all(&catalog_dir).unwrap();
        let catalog_path = catalog_dir.join("codex.db");
        let connection = Connection::open(&catalog_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE local_thread_catalog (
                    thread_id TEXT PRIMARY KEY,
                    model_provider TEXT NOT NULL
                );
                INSERT INTO local_thread_catalog (thread_id, model_provider)
                VALUES ('thread-a', 'openai'), ('thread-b', 'openai');",
            )
            .unwrap();
        drop(connection);

        purge_thread_catalogs(&root, "thread-a").unwrap();

        let connection = Connection::open(&catalog_path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM local_thread_catalog WHERE thread_id = 'thread-a'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM local_thread_catalog WHERE thread_id = 'thread-b'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );

        drop(connection);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn related_sqlite_state_roundtrips_for_import() {
        let source = test_root("related-state-source");
        let target = test_root("related-state-target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();

        for root in [&source, &target] {
            Connection::open(root.join("state_2.sqlite"))
                .unwrap()
                .execute_batch(
                    "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL);
                     CREATE TABLE thread_dynamic_tools (
                         thread_id TEXT NOT NULL, position INTEGER NOT NULL, name TEXT NOT NULL,
                         PRIMARY KEY (thread_id, position)
                     );
                     CREATE TABLE thread_spawn_edges (
                         parent_thread_id TEXT NOT NULL, child_thread_id TEXT PRIMARY KEY, status TEXT NOT NULL
                     );",
                )
                .unwrap();
            Connection::open(root.join("thread_history_3.sqlite"))
                .unwrap()
                .execute_batch(
                    "CREATE TABLE thread_turns (
                         thread_id TEXT NOT NULL, turn_id TEXT NOT NULL,
                         PRIMARY KEY (thread_id, turn_id)
                     );
                     CREATE TABLE thread_items (
                         thread_id TEXT NOT NULL, turn_id TEXT NOT NULL, item_id TEXT NOT NULL,
                         PRIMARY KEY (thread_id, turn_id, item_id)
                     );
                     CREATE TABLE thread_history_projection_state (
                         thread_id TEXT PRIMARY KEY, next_rollout_ordinal INTEGER NOT NULL
                     );",
                )
                .unwrap();
            Connection::open(root.join("queue_1.sqlite"))
                .unwrap()
                .execute_batch(
                    "CREATE TABLE queued_items (
                         id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, payload_json TEXT NOT NULL
                     );",
                )
                .unwrap();
        }

        Connection::open(source.join("state_2.sqlite"))
            .unwrap()
            .execute_batch(
                "INSERT INTO threads VALUES ('thread-a', 'sessions/source.jsonl');
                 INSERT INTO thread_dynamic_tools VALUES ('thread-a', 0, 'tool-a');
                 INSERT INTO thread_spawn_edges VALUES ('thread-a', 'thread-b', 'completed');",
            )
            .unwrap();
        Connection::open(source.join("thread_history_3.sqlite"))
            .unwrap()
            .execute_batch(
                "INSERT INTO thread_turns VALUES ('thread-a', 'turn-a');
                 INSERT INTO thread_items VALUES ('thread-a', 'turn-a', 'item-a');
                 INSERT INTO thread_history_projection_state VALUES ('thread-a', 7);",
            )
            .unwrap();
        Connection::open(source.join("queue_1.sqlite"))
            .unwrap()
            .execute(
                "INSERT INTO queued_items VALUES ('queue-a', 'thread-a', '{}')",
                [],
            )
            .unwrap();

        let thread_row =
            snapshot_thread_row(latest_state_db(&source).as_deref(), "thread-a").unwrap();
        restore_thread_row(
            latest_state_db(&target).as_deref(),
            thread_row.as_ref(),
            &target.join("sessions/rollout-thread-a.jsonl.zst"),
            "thread-a",
        )
        .unwrap();
        let included = HashSet::from(["thread-a".to_string(), "thread-b".to_string()]);
        let related = snapshot_related_state(&source, "thread-a", &included).unwrap();
        restore_related_state(&target, &related, "thread-a").unwrap();

        let state = Connection::open(target.join("state_2.sqlite")).unwrap();
        let restored_path: String = state
            .query_row(
                "SELECT rollout_path FROM threads WHERE id = 'thread-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(restored_path.ends_with("rollout-thread-a.jsonl"));
        assert_eq!(
            state
                .query_row(
                    "SELECT COUNT(*) FROM thread_dynamic_tools WHERE thread_id = 'thread-a'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            state
                .query_row(
                    "SELECT COUNT(*) FROM thread_spawn_edges WHERE parent_thread_id = 'thread-a'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            Connection::open(target.join("thread_history_3.sqlite"))
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM thread_items WHERE thread_id = 'thread-a'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            Connection::open(target.join("queue_1.sqlite"))
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM queued_items WHERE thread_id = 'thread-a'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );

        drop(state);
        fs::remove_dir_all(source).unwrap();
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn migration_rewrites_thread_ids_and_clears_old_parent_links() {
        let mut value = serde_json::json!({
            "type": "session_meta",
            "payload": {
                "id": "thread-old",
                "session_id": "thread-old",
                "parent_thread_id": "thread-old",
                "message": "thread-old should remain in user content"
            }
        });

        rewrite_thread_identifiers(&mut value, "thread-old", "thread-new");

        assert_eq!(value["payload"]["id"], "thread-new");
        assert_eq!(value["payload"]["session_id"], "thread-new");
        assert!(value["payload"]["parent_thread_id"].is_null());
        assert_eq!(value["payload"]["message"], "thread-old should remain in user content");
    }

    #[test]
    fn initial_ownership_scan_keeps_legacy_threads_unknown() {
        let mut state = crate::models::ManagerStateFile::default();
        let snapshots = vec![snapshot_for_ownership_test("legacy-thread")];

        assert!(observe_threads(&snapshots, &mut state, Some("current-account")));

        assert!(state.conversation_ownership_initialized);
        assert!(state.observed_conversation_ids.contains("legacy-thread"));
        assert!(!state.conversation_account_ids.contains_key("legacy-thread"));
    }

    #[test]
    fn later_ownership_scan_assigns_new_threads_to_the_current_account() {
        let mut state = crate::models::ManagerStateFile {
            conversation_ownership_initialized: true,
            ..crate::models::ManagerStateFile::default()
        };
        let snapshots = vec![snapshot_for_ownership_test("new-thread")];

        assert!(observe_threads(&snapshots, &mut state, Some("current-account")));

        assert_eq!(
            state.conversation_account_ids.get("new-thread").map(String::as_str),
            Some("current-account")
        );
    }

    fn snapshot_for_ownership_test(session_id: &str) -> RolloutSnapshot {
        RolloutSnapshot {
            session_id: session_id.to_string(),
            title: session_id.to_string(),
            cwd: "C:\\workspace".to_string(),
            updated_at: None,
            started_at: None,
            path: PathBuf::new(),
            physical_paths: Vec::new(),
            relative_path: PathBuf::new(),
            index_value: serde_json::json!({ "id": session_id }),
            size_bytes: 0,
            history_base_thread_id: None,
            parent_thread_id: None,
        }
    }
}
