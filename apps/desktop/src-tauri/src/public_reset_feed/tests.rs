#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(hour: u8) -> UtcTimestamp {
        UtcTimestamp::parse(format!("2026-09-12T{hour:02}:00:00Z")).unwrap()
    }

    pub(super) fn batch(hour: u8, state: &str) -> PublicCandidateBatch {
        decode_public_timeline(PublicTimelineSource::QuotaResets, &serde_json::to_vec(&json!({"data":[{
            "provider":"openai", "slug":"synthetic-reset-1", "type":"hard_reset", "state":state,
            "title":"Synthetic tracker statement", "summary":"Fixture only, not a real announcement.",
            "source":{"kind":"authorized_social", "url":"https://x.com/thsottiaux/status/123456",
                "publishedAt":"2026-09-12T00:00:00Z"}
        }]})).unwrap(), at(hour)).unwrap()
    }

    fn save(path: &Path, hour: u8, state: &str) -> PublicResetTimeline {
        save_refresh(
            path,
            vec![(PublicTimelineSource::QuotaResets, Ok(batch(hour, state)))],
            &at(hour),
        )
        .unwrap()
    }

    #[test]
    fn cache_read_before_first_collection_does_not_create_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-created").join(DB_NAME);
        let timeline = load_timeline(&path, &at(1)).unwrap();
        assert_eq!(timeline.sources.len(), 4);
        assert!(timeline.entries.is_empty());
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    #[ignore = "explicit read-only existing public journal check; aggregate output only"]
    fn readonly_existing_public_change_history() {
        let path = PathBuf::from(
            std::env::var("HORIZON_PUBLIC_LEDGER_TEST_PATH").expect("explicit public ledger path"),
        );
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(DB_NAME)
        );
        assert!(path.is_file());
        let timeline = load_timeline(&path, &now()).unwrap();
        let payload = serde_json::to_value(&timeline.changes).unwrap();
        let items = payload["items"].as_array().unwrap();
        let transitions = items
            .iter()
            .filter(|item| !item["previous"].is_null())
            .count();
        println!("public revisions={}, current records={}, meaningful revisions={}, recent changes={}, before/after pairs={}",
            timeline.revision_count, timeline.entries.len(), payload["totalCount"], items.len(), transitions);
        assert!(items.len() <= 128);
        assert!(timeline.revision_count > 0);
    }

    #[test]
    fn public_v1_migration_preserves_journal_and_creates_one_recoverable_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        save(&path, 1, "likely");
        Connection::open(&path)
            .unwrap()
            .execute_batch("DROP TABLE public_insights; DROP TABLE public_reads; DROP TABLE public_read_state; PRAGMA user_version=1;")
            .unwrap();
        let prior = load_timeline(&path, &at(1)).unwrap();
        assert!(prior.insights.forecast.value.is_none());
        assert!(!path.with_extension("before-insights.sqlite3").exists());
        let next = save(&path, 2, "likely");
        assert_eq!(next.revision_count, prior.revision_count);
        let backup = path.with_extension("before-insights.sqlite3");
        assert_eq!(
            load_timeline(&backup, &at(1)).unwrap().revision_count,
            prior.revision_count
        );
        let bytes = fs::read(&backup).unwrap();
        save(&path, 3, "confirmed");
        assert_eq!(fs::read(&backup).unwrap(), bytes);
        assert_eq!(
            Connection::open(&path)
                .unwrap()
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            4
        );
    }

    #[test]
    fn external_forecast_cache_retains_last_success_on_failure_without_rewriting_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        save(&path, 1, "likely");
        let forecast = insights::parse_forecast(
            &serde_json::to_vec(&json!({
                "updated_at":at(1).as_str(),"mode":"model","confidence":"low",
                "probabilities":{"rounded_24h":20,"rounded_48h":35}
            }))
            .unwrap(),
            &at(1),
        )
        .unwrap();
        let first =
            save_refresh_with_insights(&path, Vec::new(), Some(Ok(forecast)), &at(1)).unwrap();
        let failed = save_refresh_with_insights(
            &path,
            Vec::new(),
            Some(Err(SourceIssue::HttpError)),
            &at(2),
        )
        .unwrap();
        assert_eq!(failed.revision_count, first.revision_count);
        assert_eq!(
            failed.insights.forecast.value.unwrap().probability_24h,
            20.0
        );
        assert_eq!(failed.insights.forecast.issue, Some(SourceIssue::HttpError));
        assert_eq!(
            failed.insights.forecast.attempted_at.as_deref(),
            Some(at(2).as_str())
        );
    }

    #[test]
    fn persisted_tracker_confirmation_is_not_a_reviewed_announcement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        save(&path, 1, "confirmed");
        let reloaded = load_timeline(&path, &at(2)).unwrap();
        assert_eq!(reloaded.revision_count, 1);
        assert_eq!(
            reloaded.entries[0].disposition,
            PublicEvidenceDisposition::NeedsReview
        );
        assert_eq!(
            reloaded.entries[0].signal.scope,
            PublicEvidenceScope::Unknown
        );
        assert_eq!(
            reloaded.entries[0].signal.source.review,
            PublicEvidenceReview::Indirect
        );
    }

    #[test]
    fn timed_announcement_survives_refresh_and_reload_with_source_probability_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        let bytes = serde_json::to_vec(&json!({"data":[{
            "provider":"openai", "slug":"synthetic-timed-reset", "type":"hard_reset", "state":"likely",
            "title":"Synthetic timed announcement", "summary":"Full reset announced.",
            "expectedAt":at(10).as_str(), "cohort":"all paid ChatGPT accounts",
            "timingSource":{"url":"https://x.com/thsottiaux/status/123456", "timeZone":"America/Los_Angeles"},
            "source":{"kind":"authorized_social", "url":"https://x.com/thsottiaux/status/123456",
                "publishedAt":at(0).as_str(), "excerpt":"Global reset landing tomorrow 10am PST for all paid ChatGPT accounts."}
        }]})).unwrap();
        let decode = |hour| {
            decode_public_timeline(PublicTimelineSource::QuotaResets, &bytes, at(hour)).unwrap()
        };
        let mut legacy = decode(1);
        legacy.candidates[0].announcement_timing = None;
        legacy.candidates[0].kind = capacity_domain::public_reset::PublicEventKind::Unclassified;
        legacy.candidates[0].semantics =
            capacity_domain::public_reset::PublicSignalSemantics::PossibleSignal;
        legacy.candidates[0].source.parser_version = "public-candidate-v5".into();
        save_refresh(
            &path,
            vec![(PublicTimelineSource::QuotaResets, Ok(legacy))],
            &at(1),
        )
        .unwrap();
        let forecast = insights::parse_forecast(
            &serde_json::to_vec(&json!({
                "updated_at":at(2).as_str(), "mode":"announced", "confidence":"low",
                "probabilities":{"rounded_24h":17,"rounded_48h":31}
            }))
            .unwrap(),
            &at(2),
        )
        .unwrap();
        save_refresh_with_insights(
            &path,
            vec![(PublicTimelineSource::QuotaResets, Ok(decode(2)))],
            Some(Ok(forecast)),
            &at(2),
        )
        .unwrap();
        let latest = load_timeline(&path, &at(2)).unwrap();
        assert_eq!(latest.revision_count, 2);
        let timing = latest.entries[0]
            .signal
            .announcement_timing
            .as_ref()
            .unwrap();
        assert_eq!(
            timing.expected_at.as_ref().unwrap().as_str(),
            "2026-09-12T10:00:00.000Z"
        );
        assert_eq!(timing.cohort.as_deref(), Some("all paid ChatGPT accounts"));
        assert!(latest.entries[0].signal.occurred_at.is_none());
        assert_eq!(
            latest.insights.forecast.value.unwrap().probability_24h,
            17.0
        );
        assert!(load_timeline(&path, &at(1)).unwrap().entries[0]
            .signal
            .announcement_timing
            .is_none());
        let repeated = save_refresh(
            &path,
            vec![(PublicTimelineSource::QuotaResets, Ok(decode(3)))],
            &at(3),
        )
        .unwrap();
        assert_eq!(repeated.revision_count, 2);
    }

    #[test]
    fn identical_content_updates_source_freshness_without_rewriting_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        save(&path, 1, "likely");
        let next = save(&path, 2, "likely");
        assert_eq!(next.revision_count, 1);
        assert_eq!(next.entries[0].signal.source.collected_at, at(1));
        assert_eq!(next.entries[0].signal.recorded_at, at(1));
        assert_eq!(next.sources[0].last_success_at, Some(at(2)));
        let changes = serde_json::to_value(&next.changes).unwrap();
        assert_eq!(changes["totalCount"], 1);
        assert_eq!(changes["items"][0]["current"]["recordedAt"], at(1).as_str());
    }

    #[test]
    fn public_changes_ignore_parser_and_whitespace_but_retain_real_source_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        save(&path, 1, "confirmed");
        let mut parser = batch(2, "confirmed");
        parser.candidates[0].source.parser_version = "fixture-next".into();
        parser.candidates[0].summary = "Fixture only,  not a real announcement.\n".into();
        parser.candidates[0].source.published_at =
            Some(UtcTimestamp::parse("2026-09-12T00:00:00.000+00:00").unwrap());
        let second = save_refresh(
            &path,
            vec![(PublicTimelineSource::QuotaResets, Ok(parser))],
            &at(2),
        )
        .unwrap();
        assert_eq!(second.revision_count, 2);
        assert_eq!(
            serde_json::to_value(second.changes).unwrap()["totalCount"],
            1
        );
        let mut edited = batch(3, "confirmed");
        edited.candidates[0].summary = "An actual changed source statement in this fixture.".into();
        edited.candidates[0].source.content_sha256 = "b".repeat(64);
        let third = save_refresh(
            &path,
            vec![(PublicTimelineSource::QuotaResets, Ok(edited))],
            &at(3),
        )
        .unwrap();
        let changes = serde_json::to_value(third.changes).unwrap();
        assert_eq!(changes["totalCount"], 2);
        assert_eq!(changes["items"][0]["previous"]["revision"], 2);
        assert_eq!(changes["items"][0]["current"]["revision"], 3);
        let historical = load_timeline(&path, &at(2)).unwrap();
        assert_eq!(
            serde_json::to_value(historical.changes).unwrap()["totalCount"],
            1
        );
    }

    #[test]
    fn correction_and_retraction_append_revisions_and_preserve_prior_views() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        save(&path, 1, "likely");
        save(&path, 2, "corrected");
        let latest = save(&path, 3, "retracted");
        assert_eq!(latest.revision_count, 3);
        assert_eq!(latest.entries.len(), 1);
        assert_eq!(latest.entries[0].signal.revision, 3);
        assert_eq!(
            latest.entries[0].disposition,
            PublicEvidenceDisposition::Retracted
        );
        assert_eq!(
            load_timeline(&path, &at(2)).unwrap().entries[0].disposition,
            PublicEvidenceDisposition::Corrected
        );
        assert_eq!(
            load_timeline(&path, &at(1)).unwrap().entries[0].disposition,
            PublicEvidenceDisposition::Context
        );
        let changes = serde_json::to_value(latest.changes).unwrap();
        assert_eq!(changes["totalCount"], 3);
        assert_eq!(changes["items"][0]["current"]["semantics"], "retracted");
        assert_eq!(changes["items"][0]["previous"]["semantics"], "corrected");
        assert!(changes["items"][2]["previous"].is_null());
        let historical = load_timeline(&path, &at(1)).unwrap();
        assert_eq!(
            serde_json::to_value(historical.changes).unwrap()["totalCount"],
            1
        );
    }

    #[test]
    fn failures_and_later_omissions_do_not_erase_previous_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        save(&path, 1, "likely");
        for issue in [SourceIssue::RequestFailed, SourceIssue::SchemaChanged] {
            let failed = save_refresh(
                &path,
                vec![(PublicTimelineSource::QuotaResets, Err(issue))],
                &at(2),
            )
            .unwrap();
            assert_eq!(failed.entries.len(), 1);
            assert_eq!(failed.sources[0].last_success_at, Some(at(1)));
            assert_eq!(failed.sources[0].last_attempt_at, Some(at(2)));
            assert_eq!(failed.sources[0].issue, Some(issue));
        }
        let mut empty = batch(3, "likely");
        empty.candidates.clear();
        let next = save_refresh(
            &path,
            vec![(PublicTimelineSource::QuotaResets, Ok(empty))],
            &at(3),
        )
        .unwrap();
        assert_eq!(next.entries.len(), 1);
        assert_eq!(next.revision_count, 1);
        assert_eq!(next.sources[0].last_success_at, Some(at(3)));
        assert!(next.sources[0].issue.is_none());
    }

    #[test]
    fn primary_review_scope_and_url_tampering_are_rejected_without_cache_loss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        save(&path, 1, "likely");
        for variant in 0..4 {
            let mut next = batch(2, "confirmed");
            let signal = &mut next.candidates[0];
            match variant {
                0 => signal.source.review = PublicEvidenceReview::PrimaryReviewed,
                1 => signal.scope = PublicEvidenceScope::BroadCodex,
                2 => signal.source.canonical_url = "https://localhost/private".into(),
                _ => {
                    signal.source.discovered_via =
                        vec![PublicTimelineSource::CodexReset.url().into()]
                }
            }
            let result = save_refresh(
                &path,
                vec![(PublicTimelineSource::QuotaResets, Ok(next))],
                &at(2),
            )
            .unwrap();
            assert_eq!(result.revision_count, 1);
            assert_eq!(result.sources[0].issue, Some(SourceIssue::InvalidResponse));
            assert_eq!(result.sources[0].last_success_at, Some(at(1)));
        }
    }

    #[test]
    fn source_identity_changes_are_not_silently_rebased() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        save(&path, 1, "likely");
        let mut next = batch(2, "confirmed");
        next.candidates[0].evidence_family_id = "x:999999".into();
        let result = save_refresh(
            &path,
            vec![(PublicTimelineSource::QuotaResets, Ok(next))],
            &at(2),
        )
        .unwrap();
        assert_eq!(result.revision_count, 1);
        assert_eq!(result.sources[0].rejected_records, 1);
    }

    #[test]
    fn mirror_records_share_one_evidence_family() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        let second = decode_public_timeline(
            PublicTimelineSource::CodexReset,
            &serde_json::to_vec(&json!({"events":[{
                "id":"mirror-1", "url":"https://twitter.com/thsottiaux/status/123456?s=20",
                "type":"reset", "reset_kind":"hard", "summary":"Synthetic mirror."
            }]}))
            .unwrap(),
            at(1),
        )
        .unwrap();
        let result = save_refresh(
            &path,
            vec![
                (PublicTimelineSource::QuotaResets, Ok(batch(1, "likely"))),
                (PublicTimelineSource::CodexReset, Ok(second)),
            ],
            &at(1),
        )
        .unwrap();
        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.evidence_family_count, 1);
    }

    #[test]
    fn unknown_schema_and_corrupt_records_are_not_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE unrelated (value TEXT); INSERT INTO unrelated VALUES ('keep');",
        )
        .unwrap();
        assert!(save_refresh(&path, vec![], &at(1)).is_err());
        assert_eq!(
            conn.query_row("SELECT value FROM unrelated", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        let second = dir.path().join("corrupt.sqlite3");
        save(&second, 1, "likely");
        let corrupt = Connection::open(&second).unwrap();
        corrupt
            .execute("UPDATE public_signals SET signal_json='not-json'", [])
            .unwrap();
        assert!(save_refresh(&second, vec![], &at(2)).is_err());
        assert_eq!(
            corrupt
                .query_row("SELECT signal_json FROM public_signals", [], |row| row
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
            "not-json"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_store_is_rejected_and_target_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.sqlite3");
        let link = dir.path().join(DB_NAME);
        save(&target, 1, "likely");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(load_timeline(&link, &at(2)).is_err());
        assert!(save_refresh(&link, vec![], &at(2)).is_err());
        assert_eq!(load_timeline(&target, &at(2)).unwrap().revision_count, 1);
    }

    #[test]
    fn failed_sql_transaction_does_not_leave_partial_revisions_or_freshness() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        save(&path, 1, "likely");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TRIGGER reject_status BEFORE UPDATE ON public_sources BEGIN SELECT RAISE(ABORT, 'fixture'); END;").unwrap();
        assert!(save_refresh(
            &path,
            vec![(PublicTimelineSource::QuotaResets, Ok(batch(2, "retracted")))],
            &at(2)
        )
        .is_err());
        let previous = load_timeline(&path, &at(2)).unwrap();
        assert_eq!(previous.revision_count, 1);
        assert_eq!(previous.sources[0].last_success_at, Some(at(1)));
    }

    #[test]
    fn refresh_backoff_covers_failures_but_not_future_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let timeline = save_refresh(
            &dir.path().join(DB_NAME),
            PublicTimelineSource::ALL
                .into_iter()
                .map(|source| (source, Err(SourceIssue::RequestFailed)))
                .collect(),
            &at(2),
        )
        .unwrap();
        assert!(sources_recent(&timeline, &at(2)));
        assert!(!sources_recent(&timeline, &at(1)));
        assert!(!sources_recent(&timeline, &at(3)));
        assert!(!sources_recent(
            &timeline,
            &UtcTimestamp::parse("2026-09-12T02:15:00Z").unwrap()
        ));
    }

    fn http_fixture(status: u16, body: Vec<u8>, chunked: bool) -> Result<Vec<u8>, SourceIssue> {
        http_fixture_type(status, body, chunked, "application/json; charset=utf-8")
    }

    fn http_fixture_type(
        status: u16,
        body: Vec<u8>,
        chunked: bool,
        content_type: &'static str,
    ) -> Result<Vec<u8>, SourceIssue> {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", server.server_addr());
        let worker = std::thread::spawn(move || {
            let request = server
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            for header in request.headers() {
                assert!(!header.field.equiv("Authorization"));
                assert!(!header.field.equiv("Cookie"));
            }
            let size = (!chunked).then_some(body.len());
            let response = tiny_http::Response::new(
                tiny_http::StatusCode(status),
                vec![],
                std::io::Cursor::new(body),
                size,
                None,
            )
            .with_header(tiny_http::Header::from_bytes("Content-Type", content_type).unwrap())
            .with_header(
                tiny_http::Header::from_bytes("Location", "http://127.0.0.1:1/not-followed")
                    .unwrap(),
            );
            let _ = request.respond(response);
        });
        let response = public_client_builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(url)
            .send()
            .unwrap();
        let result = read_response(response);
        worker.join().unwrap();
        result
    }

    #[test]
    fn http_client_has_no_credentials_rejects_redirects_and_bounds_bodies() {
        assert_eq!(http_fixture(200, b"{}".to_vec(), false).unwrap(), b"{}");
        assert_eq!(
            http_fixture(302, vec![], false),
            Err(SourceIssue::HttpError)
        );
        assert_eq!(
            http_fixture(503, vec![], false),
            Err(SourceIssue::HttpError)
        );
        assert_eq!(
            http_fixture_type(200, b"{}".to_vec(), false, "text/html"),
            Err(SourceIssue::InvalidResponse)
        );
        for chunked in [false, true] {
            assert_eq!(
                http_fixture(200, vec![b'x'; MAX_RESPONSE_BYTES as usize + 1], chunked),
                Err(SourceIssue::ResponseTooLarge)
            );
        }
    }

    #[test]
    #[ignore = "explicit read-only HTTPS smoke; fixed public sources, aggregate output only"]
    fn readonly_public_sources_smoke() {
        let outcomes = collect_sources();
        for (source, result) in &outcomes {
            match result {
                Ok(batch) => eprintln!(
                    "source={} accepted={} rejected={} skipped={} undated={}",
                    source.id(),
                    batch.candidates.len(),
                    batch.rejected_records,
                    batch.skipped_records,
                    batch
                        .candidates
                        .iter()
                        .filter(|item| item.source.published_at.is_none())
                        .count()
                ),
                Err(issue) => eprintln!("source={} issue={issue:?}", source.id()),
            }
        }
        assert!(
            outcomes.iter().any(|(_, result)| result
                .as_ref()
                .is_ok_and(|batch| !batch.candidates.is_empty())),
            "No source provided usable evidence"
        );
        let dir = tempfile::tempdir().unwrap();
        let timeline = save_refresh(&dir.path().join(DB_NAME), outcomes, &now()).unwrap();
        assert!(timeline.entries.iter().all(|entry| !matches!(
            entry.disposition,
            PublicEvidenceDisposition::Confirmed | PublicEvidenceDisposition::Announced
        )));
        eprintln!(
            "persisted={} revisions={} evidence_families={}",
            timeline.entries.len(),
            timeline.revision_count,
            timeline.evidence_family_count
        );
    }
}
