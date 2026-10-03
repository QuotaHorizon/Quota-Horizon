fn public_client_builder() -> reqwest::blocking::ClientBuilder {
    // Dedicated client: no cookies, default authorization, account headers or
    // redirected requests. Only the existing user-configured proxy is reused.
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("QuotaHorizon/PublicSources (+https://github.com/QuotaHorizon/Quota-Horizon)")
}

fn public_client() -> Result<reqwest::blocking::Client, SourceIssue> {
    crate::system_proxy::apply(public_client_builder())
        .build()
        .map_err(|_| SourceIssue::RequestFailed)
}

fn read_response(response: reqwest::blocking::Response) -> Result<Vec<u8>, SourceIssue> {
    if !response.status().is_success() {
        return Err(SourceIssue::HttpError);
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if content_type != "application/json"
        && !(content_type.starts_with("application/") && content_type.ends_with("+json"))
    {
        return Err(SourceIssue::InvalidResponse);
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES)
    {
        return Err(SourceIssue::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SourceIssue::RequestFailed)?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(SourceIssue::ResponseTooLarge);
    }
    Ok(bytes)
}

fn fetch_source(
    client: &reqwest::blocking::Client,
    source: PublicTimelineSource,
) -> Result<PublicCandidateBatch, SourceIssue> {
    let response = client
        .get(if source == PublicTimelineSource::CodexResetPosts {
            insights::POSTS_URL
        } else {
            source.url()
        })
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .map_err(|_| SourceIssue::RequestFailed)?;
    let bytes = read_response(response)?;
    decode_public_timeline(source, &bytes, now()).map_err(|error| match error {
        "public_source_schema_changed" => SourceIssue::SchemaChanged,
        "public_source_upstream_stale" => SourceIssue::UpstreamStale,
        "public_source_too_large" | "public_source_too_many_records" => {
            SourceIssue::ResponseTooLarge
        }
        _ => SourceIssue::InvalidResponse,
    })
}

type SourceOutcome = (
    PublicTimelineSource,
    Result<PublicCandidateBatch, SourceIssue>,
);

fn collect_sources() -> Vec<SourceOutcome> {
    let client = match public_client() {
        Ok(client) => client,
        Err(issue) => {
            return PublicTimelineSource::ALL
                .into_iter()
                .map(|source| (source, Err(issue)))
                .collect()
        }
    };
    std::thread::scope(|scope| {
        let workers: Vec<_> = PublicTimelineSource::ALL
            .into_iter()
            .map(|source| {
                let client = client.clone();
                (source, scope.spawn(move || fetch_source(&client, source)))
            })
            .collect();
        workers
            .into_iter()
            .map(|(source, worker)| {
                (
                    source,
                    worker.join().unwrap_or(Err(SourceIssue::RequestFailed)),
                )
            })
            .collect()
    })
}

#[cfg(test)]
fn save_refresh(
    path: &Path,
    outcomes: Vec<SourceOutcome>,
    at: &UtcTimestamp,
) -> Result<PublicResetTimeline, String> {
    save_refresh_with_insights(path, outcomes, None, at)
}

fn save_refresh_with_insights(
    path: &Path,
    outcomes: Vec<SourceOutcome>,
    forecast: Option<Result<insights::ExternalForecast, SourceIssue>>,
    at: &UtcTimestamp,
) -> Result<PublicResetTimeline, String> {
    save_refresh_with_radar(path, outcomes, forecast, None, at)
}

fn save_refresh_with_radar(
    path: &Path,
    outcomes: Vec<SourceOutcome>,
    forecast: Option<Result<insights::ExternalForecast, SourceIssue>>,
    radar: Option<radar::Collected>,
    at: &UtcTimestamp,
) -> Result<PublicResetTimeline, String> {
    let mut conn = open_store(path, true)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let mut stored = load_ledger(&tx, None)?;
    let mut statuses = read_sources(&tx)?;
    let mut insight_cache = insights::read_cache(&tx)?;
    if let Some(result) = forecast {
        insight_cache.forecast.attempted_at = Some(at.as_str().into());
        match result {
            Ok(value) => {
                insight_cache.forecast.value = Some(value);
                insight_cache.forecast.issue = None;
            }
            Err(issue) => insight_cache.forecast.issue = Some(issue),
        }
    }
    for (source, outcome) in outcomes {
        let status = statuses
            .iter_mut()
            .find(|item| item.source_id == source.id())
            .ok_or_else(|| db_error("unknown source"))?;
        status.last_attempt_at = Some(at.clone());
        match outcome {
            Err(issue) => status.issue = Some(issue),
            Ok(batch)
                if batch.source_url != source.url()
                    || batch
                        .candidates
                        .len()
                        .saturating_add(batch.rejected_records)
                        .saturating_add(batch.skipped_records)
                        > 2048
                    || !at.is_not_before(&batch.collected_at) =>
            {
                status.issue = Some(SourceIssue::InvalidResponse)
            }
            Ok(batch) => {
                if source == PublicTimelineSource::CodexResetPosts {
                    if let Some(posts) = batch.posts {
                        insight_cache.posts = Some(posts);
                    }
                }
                let mut rejected = batch.rejected_records;
                let mut accepted = 0;
                for mut signal in batch.candidates {
                    if !validate_collected_signal(source, &signal) {
                        rejected += 1;
                        continue;
                    }
                    if let Some(previous) = stored.latest.get(&signal.signal_id) {
                        if previous.source.content_sha256 == signal.source.content_sha256
                            && previous.source.parser_version == signal.source.parser_version
                        {
                            accepted += 1;
                            continue;
                        }
                        let Some(revision) = previous.revision.checked_add(1) else {
                            rejected += 1;
                            continue;
                        };
                        signal.revision = revision;
                    }
                    signal.recorded_at = at.clone();
                    let json = serde_json::to_string(&signal).map_err(db_error)?;
                    if json.len() > MAX_SIGNAL_BYTES
                        || stored.bytes.saturating_add(json.len() as u64)
                            > MAX_JOURNAL_PAYLOAD_BYTES
                        || stored.ledger.append(signal.clone()).is_err()
                    {
                        rejected += 1;
                        continue;
                    }
                    tx.execute("INSERT INTO public_signals (signal_id, revision, source_id, signal_json) VALUES (?1, ?2, ?3, ?4)",
                        params![signal.signal_id, signal.revision, source.id(), json]).map_err(db_error)?;
                    stored.bytes += json.len() as u64;
                    stored.count += 1;
                    stored.latest.insert(signal.signal_id.clone(), signal);
                    accepted += 1;
                }
                status.accepted_records = accepted;
                status.rejected_records = rejected;
                status.skipped_records = batch.skipped_records;
                if accepted == 0 && rejected > 0 {
                    status.issue = Some(SourceIssue::InvalidResponse);
                } else {
                    status.issue = None;
                    status.last_success_at = Some(at.clone());
                }
            }
        }
        let issue = status
            .issue
            .map(|value| serde_json::to_string(&value))
            .transpose()
            .map_err(db_error)?;
        tx.execute("INSERT INTO public_sources (source_id, last_attempt_at, last_success_at, issue_json, accepted_records, rejected_records, skipped_records)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(source_id) DO UPDATE SET
            last_attempt_at=excluded.last_attempt_at, last_success_at=excluded.last_success_at, issue_json=excluded.issue_json,
            accepted_records=excluded.accepted_records, rejected_records=excluded.rejected_records, skipped_records=excluded.skipped_records",
            params![status.source_id, at.as_str(), status.last_success_at.as_ref().map(UtcTimestamp::as_str), issue,
                status.accepted_records as i64, status.rejected_records as i64, status.skipped_records as i64]).map_err(db_error)?;
    }
    insights::save_cache(&tx, &insight_cache)?;
    if let Some(collected) = radar {
        radar::save(&tx, collected, &insight_cache, at)?;
    }
    read_state::baseline_if_needed(&tx)?;
    tx.commit().map_err(db_error)?;
    load_timeline(path, at)
}

fn sources_recent(timeline: &PublicResetTimeline, at: &UtcTimestamp) -> bool {
    let Ok(now) = DateTime::parse_from_rfc3339(at.as_str()) else {
        return false;
    };
    timeline.sources.iter().all(|source| {
        source
            .last_attempt_at
            .as_ref()
            .and_then(|time| DateTime::parse_from_rfc3339(time.as_str()).ok())
            .is_some_and(|time| {
                (0..SOURCE_INTERVAL_SECONDS)
                    .contains(&now.signed_duration_since(time).num_seconds())
            })
    })
}

fn data_path<R: Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(db_error)?.join(DB_NAME))
}

#[tauri::command]
pub(crate) async fn get_public_reset_timeline<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
) -> Result<PublicResetTimeline, String> {
    let path = data_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || load_timeline(&path, &now()))
        .await
        .map_err(db_error)?
}

#[tauri::command]
pub(crate) async fn refresh_public_reset_timeline<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    force: bool,
) -> Result<PublicResetTimeline, String> {
    let path = data_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (timeline, collected) = refresh_at_path(&path, force)?;
        if collected {
            let _ = app.emit("public-reset-timeline-changed", ());
        }
        Ok(timeline)
    })
    .await
    .map_err(db_error)?
}

fn refresh_at_path(path: &Path, force: bool) -> Result<(PublicResetTimeline, bool), String> {
    static REFRESH: Mutex<Option<Instant>> = Mutex::new(None);
    let mut gate = REFRESH.lock().map_err(db_error)?;
    let at = now();
    let cached = load_timeline(path, &at)?;
    if gate.is_some_and(|last| last.elapsed() < Duration::from_secs(5))
        || (!force
            && sources_recent(&cached, &at)
            && cached.radar.updated_at.is_some()
            && radar::uses_current_model(&cached.radar)
            && cached.insights.forecast.attempted_at.is_some())
    {
        return Ok((cached, false));
    }
    *gate = Some(Instant::now());
    let (mut outcomes, forecast, radar) = std::thread::scope(|scope| {
        let radar = scope.spawn(|| radar::collect(&at));
        let forecast = scope.spawn(|| {
            let client = public_client()?;
            let response = client
                .get(insights::FORECAST_URL)
                .send()
                .map_err(|_| SourceIssue::RequestFailed)?;
            insights::parse_forecast(&read_response(response)?, &now())
        });
        (
            collect_sources(),
            forecast.join().unwrap_or(Err(SourceIssue::RequestFailed)),
            radar.join().map_err(|_| db_error("radar collector failed")),
        )
    });
    if let Ok(client) = public_client() {
        for (_, result) in &mut outcomes {
            if let Ok(batch) = result {
                if let Some(feed) = batch.posts.as_mut() {
                    insights::enrich_parents(&client, feed, cached.insights.posts.as_ref(), &now());
                }
            }
        }
    }
    save_refresh_with_radar(path, outcomes, Some(forecast), Some(radar?), &now())
        .map(|timeline| (timeline, true))
}
