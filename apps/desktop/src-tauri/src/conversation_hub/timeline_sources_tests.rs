#[cfg(test)]
mod timeline_source_tests {
    use super::*;

    const ROOT_ID: &str = "00000000-0000-4000-8000-000000000001";
    const CHILD_ID: &str = "00000000-0000-4000-8000-000000000002";

    fn fixture_root() -> PathBuf {
        std::env::temp_dir().join(format!("horizon-history-links-{}", Uuid::new_v4()))
    }

    fn segment(
        root: &Path,
        suffix: &str,
        day: u8,
        base: Option<(&str, u64)>,
        events: &[Value],
    ) -> PathBuf {
        let path = root
            .join("sessions/2026/09/07")
            .join(format!("rollout-{ROOT_ID}{suffix}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let timestamp = format!("2026-09-{day:02}T00:00:00Z");
        let mut meta = json!({"type":"session_meta","timestamp":timestamp,"payload":{
            "id":ROOT_ID,"cwd":"/tmp/fixture","timestamp":timestamp,"history_mode":"paginated"}});
        if let Some((id, end)) = base {
            meta["payload"]["history_base"] =
                json!({"thread_id":id,"end_byte_offset":end,"end_ordinal_exclusive":2});
        }
        let mut text = format!("{meta}\n");
        for event in events {
            text.push_str(&format!("{event}\n"));
        }
        fs::write(&path, text).unwrap();
        path
    }

    fn message(text: &str) -> Value {
        json!({"type":"event_msg","timestamp":"2026-09-07T01:00:00Z","payload":{"type":"agent_message","message":text}})
    }

    fn read(root: &Path) -> Result<ParsedThreadDetail, String> {
        let (head, sources) = logical_thread_sources(root, ROOT_ID)?;
        parse_logical_thread_detail(&head, &sources)
    }

    #[test]
    fn explicit_history_links_obey_prefix_boundaries_and_pair_tools_across_segments() {
        let root = fixture_root();
        let parent = segment(
            &root,
            "",
            1,
            None,
            &[
                message("earlier"),
                json!({"type":"response_item","timestamp":"2026-09-07T01:00:01Z","payload":{"type":"function_call","call_id":"cross-segment","name":"shell","arguments":"fixture"}}),
            ],
        );
        let cutoff = fs::metadata(&parent).unwrap().len();
        fs::OpenOptions::new()
            .append(true)
            .open(&parent)
            .unwrap()
            .write_all(format!("{}\n", message("excluded parent append")).as_bytes())
            .unwrap();
        let child = segment(
            &root,
            &format!("_{CHILD_ID}"),
            7,
            Some((ROOT_ID, cutoff)),
            &[
                json!({"type":"response_item","timestamp":"2026-09-07T01:00:02Z","payload":{"type":"function_call_output","call_id":"cross-segment","output":"fixture result"}}),
                message("latest"),
            ],
        );
        let before_parent = fs::read(&parent).unwrap();
        let before_child = fs::read(&child).unwrap();
        let parsed = read(&root).unwrap();
        assert_eq!(parsed.summary.segment_count, 2);
        assert_eq!(
            parsed.summary.size_bytes,
            cutoff + before_child.len() as u64
        );
        assert_eq!(parsed.timeline.len(), 3);
        assert!(!parsed
            .timeline
            .iter()
            .any(|item| item.text.as_deref() == Some("excluded parent append")));
        assert_eq!(
            parsed
                .timeline
                .iter()
                .find(|item| item.kind == "tool_call")
                .unwrap()
                .tool_output
                .as_deref(),
            Some("fixture result")
        );
        assert_eq!(fs::read(&parent).unwrap(), before_parent);
        assert_eq!(fs::read(&child).unwrap(), before_child);
        assert!(!root.join("state_5.sqlite").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn history_does_not_merge_unlinked_same_id_files() {
        let root = fixture_root();
        segment(&root, "", 1, None, &[message("unrelated old copy")]);
        segment(
            &root,
            &format!("_{CHILD_ID}"),
            7,
            None,
            &[message("current")],
        );
        let parsed = read(&root).unwrap();
        assert_eq!(parsed.summary.segment_count, 1);
        assert_eq!(parsed.timeline.len(), 1);
        assert_eq!(parsed.timeline[0].text.as_deref(), Some("current"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn history_rejects_missing_ambiguous_cyclic_and_incomplete_sources() {
        let root = fixture_root();
        let parent = segment(&root, "", 1, None, &[message("parent")]);
        let end = fs::metadata(&parent).unwrap().len();
        segment(
            &root,
            &format!("_{CHILD_ID}"),
            7,
            Some(("missing", end)),
            &[],
        );
        assert!(read(&root).err().unwrap().contains("暂不可用"));
        segment(
            &root,
            &format!("_{CHILD_ID}"),
            7,
            Some((CHILD_ID, end)),
            &[],
        );
        assert!(read(&root).err().unwrap().contains("循环"));
        segment(
            &root,
            &format!("_{CHILD_ID}"),
            7,
            Some((ROOT_ID, end - 1)),
            &[],
        );
        assert!(read(&root).err().unwrap().contains("完整记录"));
        segment(
            &root,
            &format!("_{CHILD_ID}"),
            7,
            Some((ROOT_ID, end + 1)),
            &[],
        );
        assert!(read(&root).err().unwrap().contains("边界无效"));
        segment(&root, &format!("_{CHILD_ID}"), 7, Some((ROOT_ID, end)), &[]);
        fs::copy(
            &parent,
            parent.with_file_name(format!("rollout-duplicate-{ROOT_ID}.jsonl")),
        )
        .unwrap();
        assert!(read(&root).err().unwrap().contains("多个来源"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn frozen_read_survives_append_until_explicit_refresh() {
        let root = fixture_root();
        let path = segment(&root, "", 7, None, &[message("first"), message("second")]);
        let cache = std::sync::Mutex::new(None);
        let first = read_logical_thread_detail_cached(&root, ROOT_ID, None, &cache).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(format!("{}\n", message("third")).as_bytes())
            .unwrap();
        let frozen =
            read_logical_thread_detail_cached(&root, ROOT_ID, Some(&first.revision), &cache)
                .unwrap();
        assert!(std::sync::Arc::ptr_eq(&first, &frozen));
        let page =
            page_thread_detail(&frozen, Some(1), Some(1), Some(first.revision.clone())).unwrap();
        assert_eq!(page.offset, 1);
        assert_eq!(page.previous_offset, Some(0));
        assert_eq!(page.next_offset, None);
        assert_eq!(page.total, 2);
        let latest = read_logical_thread_detail_cached(&root, ROOT_ID, None, &cache).unwrap();
        assert_ne!(first.revision, latest.revision);
        assert_eq!(latest.timeline.len(), 3);
        assert_eq!(
            page_thread_detail(&latest, Some(1), Some(1), Some(first.revision.clone()))
                .unwrap_err(),
            "session_revision_changed"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compressed_single_segment_remains_readable() {
        let root = fixture_root();
        let path = segment(&root, "", 7, None, &[message("compressed fixture")]);
        let compressed = zstd::stream::encode_all(File::open(&path).unwrap(), 1).unwrap();
        fs::write(compressed_rollout_path(&path), compressed).unwrap();
        fs::remove_file(path).unwrap();
        let parsed = read(&root).unwrap();
        assert_eq!(parsed.timeline.len(), 1);
        assert_eq!(parsed.summary.segment_count, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_record_is_disclosed_without_hiding_later_messages_or_weakening_bounds() {
        let root = fixture_root();
        let path = segment(
            &root,
            "",
            7,
            None,
            &[
                message("before"),
                message(&"x".repeat(MAX_JSONL_LINE_BYTES)),
                message("after"),
            ],
        );
        let parsed = read(&root).unwrap();
        assert_eq!(parsed.summary.skipped_record_count, 1);
        assert_eq!(parsed.timeline.len(), 2);
        assert_eq!(parsed.timeline[1].text.as_deref(), Some("after"));
        let mut reader = std::io::Cursor::new(vec![b'x'; MAX_JSONL_LINE_BYTES + 1]);
        assert!(
            read_timeline_record(&mut reader, &mut Vec::new(), &mut Sha256::new(), 16).is_err()
        );
        let mut strict = std::io::Cursor::new(vec![b'x'; MAX_JSONL_LINE_BYTES + 1]);
        assert!(bounded_read_line(&mut strict, &mut String::new()).is_err());
        assert!(fs::metadata(path).unwrap().len() > MAX_JSONL_LINE_BYTES as u64);
        fs::remove_dir_all(root).unwrap();
    }

    // Explicit opt-in smoke check: reads only the named logical history, emits
    // aggregate counts/times, and never calls recovery or any mutation command.
    #[test]
    #[ignore = "Requires explicit HORIZON_READONLY_HISTORY_ROOT and HORIZON_READONLY_HISTORY_ID"]
    fn readonly_local_history_smoke() {
        let root =
            PathBuf::from(std::env::var("HORIZON_READONLY_HISTORY_ROOT").expect("explicit root"));
        let id = std::env::var("HORIZON_READONLY_HISTORY_ID").expect("explicit session");
        let started = std::time::Instant::now();
        let (head, sources) = logical_thread_sources(&root, &id).unwrap();
        let parsed = parse_logical_thread_detail(&head, &sources).unwrap();
        eprintln!("Read-only history: segments={}, bytes={}, records={}, items={}, started={:?}, updated={:?}, elapsed_ms={}",
            parsed.summary.segment_count, parsed.summary.size_bytes, parsed.summary.line_count,
            parsed.timeline.len(), parsed.summary.started_at, parsed.summary.updated_at, started.elapsed().as_millis());
        assert!(!parsed.timeline.is_empty());
    }
}
