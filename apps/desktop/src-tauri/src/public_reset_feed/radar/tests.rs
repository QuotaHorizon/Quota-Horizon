use super::*;
fn at() -> UtcTimestamp {
    UtcTimestamp::parse("2026-10-03T08:00:00.000Z").unwrap()
}
fn forecast(id: &str, method: &str, p: f64) -> Forecast {
    let mut f = sources::unavailable(id, SourceIssue::InvalidResponse, &at());
    f.id = id.into();
    f.method = method.into();
    f.issue = None;
    f.probability_24h = Some(p);
    f.probability_48h = Some((p + 20.0).min(100.0));
    f.updated_at = Some(at().as_str().into());
    f.last_reset_at = Some("2026-10-02T21:18:48Z".into());
    f
}
#[test]
fn pooling_is_monotone_and_duplicate_methods_do_not_multiply_evidence() {
    let a = forecast("codex_reset", "cadence", 20.0);
    let b = forecast("reset_monitor", "statements", 60.0);
    let mut v = RadarView {
        forecasts: vec![a.clone(), b.clone()],
        ..Default::default()
    };
    model::evaluate(&mut v, &at());
    let p = v.estimate.as_ref().unwrap().probability_24h;
    assert!((p - 36.5).abs() < 0.1);
    let mut clone = a;
    clone.id = "copy".into();
    v.forecasts.push(clone);
    model::evaluate(&mut v, &at());
    assert_eq!(v.estimate.as_ref().unwrap().probability_24h, p);
    assert!(v.estimate.as_ref().unwrap().probability_48h >= p);
    v.forecasts
        .push(forecast("different_model", "cadence", 40.0));
    model::evaluate(&mut v, &at());
    let before = v.estimate.as_ref().unwrap().probability_24h;
    let mut repeated = v.forecasts[0].clone();
    repeated.id = "another_copy".into();
    v.forecasts.push(repeated);
    model::evaluate(&mut v, &at());
    assert_eq!(before, v.estimate.as_ref().unwrap().probability_24h);
}
#[test]
fn failures_and_missed_resets_never_become_zero_votes() {
    let mut stale = forecast("reset_app", "mixed", 99.0);
    stale.last_reset_at = Some("2026-07-25T03:37:00Z".into());
    let mut v = RadarView {
        forecasts: vec![forecast("codex_reset", "cadence", 20.0), stale],
        ..Default::default()
    };
    model::evaluate(&mut v, &at());
    assert_eq!(v.forecasts[1].exclusion.as_deref(), Some("reset_mismatch"));
    assert_eq!(v.estimate.as_ref().unwrap().probability_24h, 20.0);
    v.forecasts[0].issue = Some(SourceIssue::RequestFailed);
    model::evaluate(&mut v, &at());
    assert!(v.estimate.is_none());
    assert_eq!(v.forecasts[0].probability_24h, Some(20.0));
}
#[test]
fn community_distinguishes_wishes_quotes_scope_and_prediction() {
    for t in [
        "I hope Codex will reset today",
        "Please reset Codex quota tonight",
    ] {
        assert_eq!(community::classify(t).unwrap().0, "wish");
    }
    assert_eq!(
        community::classify("Tibo says Codex will reset today")
            .unwrap()
            .0,
        "observation"
    );
    assert_eq!(
        community::classify("My weekly Codex quota will reset today")
            .unwrap()
            .0,
        "observation"
    );
    assert_eq!(
        community::classify("Codex quota won't reset tomorrow").unwrap(),
        ("pessimistic", "explicit_prediction", Some(48))
    );
    assert_eq!(
        community::classify("I don't expect another Codex quota reset today")
            .unwrap()
            .0,
        "pessimistic"
    );
    assert_eq!(
        community::classify("Codex 今天大概率重置额度").unwrap(),
        ("optimistic", "explicit_prediction", Some(24))
    );
    assert!(community::classify("git reset fixed the Codex limit bug").is_none());
}
fn opinion(author: &str, text: &str) -> Opinion {
    Opinion {
        id: author.into(),
        channel: "github".into(),
        author: author.into(),
        url: "https://github.com/openai/codex/issues/1".into(),
        text: text.into(),
        published_at: "2026-10-03T07:00:00Z".into(),
        stance: "optimistic".into(),
        reason: "explicit_prediction".into(),
        evidence_urls: vec![],
        horizon_hours: Some(24),
    }
}
#[test]
fn community_is_author_capped_time_bounded_and_deduplicated() {
    let mut v = RadarView {
        forecasts: vec![forecast("codex_reset", "cadence", 20.0)],
        ..Default::default()
    };
    v.community.opinions = vec![
        opinion("a", "Codex will reset today"),
        opinion("a", "I expect a Codex reset today"),
        opinion("b", "Codex will reset today"),
    ];
    model::evaluate(&mut v, &at());
    assert_eq!(v.community.authors, 1);
    assert_eq!(v.community.duplicates, 2);
    assert_eq!(v.estimate.as_ref().unwrap().community_adjustment_24h, 0.0);
}
#[test]
fn fresh_independent_community_moves_the_estimate_without_recounting_mixed_forecasts() {
    let mut v = RadarView {
        forecasts: vec![
            forecast("codex_reset", "cadence", 30.0),
            forecast("quota_cue", "mixed", 90.0),
        ],
        ..Default::default()
    };
    v.community.channels.push(Channel {
        id: "github".into(),
        name: "GitHub".into(),
        url: "https://github.com".into(),
        query: "test".into(),
        attempted_at: at().as_str().into(),
        success_at: Some(at().as_str().into()),
        issue: None,
        scanned: 3,
        truncated: false,
    });
    v.community.opinions = vec![
        opinion("a", "first prediction"),
        opinion("b", "second prediction"),
        opinion("c", "third prediction"),
    ];
    model::evaluate(&mut v, &at());
    let e = v.estimate.as_ref().unwrap();
    assert!(e.probability_24h > 30.0 && e.probability_24h < 35.0);
    assert_eq!(
        v.forecasts[1].exclusion.as_deref(),
        Some("community_overlap")
    );
    for p in &mut v.community.opinions {
        p.text.push_str(" Tibo says");
    }
    model::evaluate(&mut v, &at());
    assert_eq!(v.community.independent_authors, 0);
}
#[test]
fn parser_rejects_impossible_windows_and_keeps_unknown_generation_time() {
    let f = sources::parse("quota_cue", br#"{"pred_24h":{"probability":16.6},"pred_48h":{"probability":30},"last_reset":"2026-10-02T21:18:48Z"}"#, &at()).unwrap();
    assert_eq!(f.updated_at, None);
    assert_eq!(f.probability_24h, Some(16.6));
    assert!(sources::parse(
        "quota_cue",
        br#"{"pred_24h":{"probability":60},"pred_48h":{"probability":30}}"#,
        &at()
    )
    .is_err());
}
#[test]
fn snapshots_replay_inputs_and_failures_without_rewriting_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("public.sqlite");
    let conn = open_store(&path, true).unwrap();
    let collected = Collected {
        forecasts: vec![(
            "reset_monitor",
            Ok(forecast("reset_monitor", "statements", 40.0)),
        )],
        community: vec![],
    };
    save(
        &conn,
        collected,
        &insights::PublicInsights::default(),
        &at(),
    )
    .unwrap();
    let first = load(&conn, &at()).unwrap();
    assert_eq!(first.history.len(), 1);
    let later = UtcTimestamp::parse("2026-10-03T08:15:00.000Z").unwrap();
    save(
        &conn,
        Collected {
            forecasts: vec![("reset_monitor", Err(SourceIssue::RequestFailed))],
            community: vec![],
        },
        &insights::PublicInsights::default(),
        &later,
    )
    .unwrap();
    let latest = load(&conn, &later).unwrap();
    assert!(latest.estimate.is_none());
    assert_eq!(latest.forecasts[0].probability_24h, Some(40.0));
    assert_eq!(latest.history.len(), 1);
    assert_eq!(
        load(&conn, &at())
            .unwrap()
            .estimate
            .unwrap()
            .probability_24h,
        40.0
    );
}

#[test]
fn v3_upgrade_adds_forecast_storage_without_replacing_existing_tables() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("public.sqlite");
    let conn = open_store(&path, true).unwrap();
    conn.execute_batch("DROP TABLE radar_snapshots; PRAGMA user_version=3;")
        .unwrap();
    let before: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='public_signals'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(conn);
    let conn = open_store(&path, true).unwrap();
    assert!(table_exists(&conn).unwrap());
    let after: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='public_signals'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        4
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}
#[test]
#[ignore = "explicit public network verification; no account data or existing database writes"]
fn live_radar_collectors_and_snapshot() {
    let time = now();
    let collected = collect(&time);
    for (id, result) in &collected.forecasts {
        eprintln!(
            "{id}: {}",
            if result.is_ok() { "ok" } else { "unavailable" }
        );
    }
    assert!(
        collected
            .forecasts
            .iter()
            .filter(|(_, r)| r.is_ok())
            .count()
            >= 2
    );
    for (c, _) in &collected.community {
        eprintln!("{}: scanned={} issue={:?}", c.id, c.scanned, c.issue);
    }
    let dir = tempfile::tempdir().unwrap();
    let conn = open_store(&dir.path().join("radar.sqlite"), true).unwrap();
    let client = public_client().unwrap();
    let bytes = read_response(client.get(insights::FORECAST_URL).send().unwrap()).unwrap();
    let mut insights = insights::PublicInsights::default();
    insights.forecast.value = Some(insights::parse_forecast(&bytes, &time).unwrap());
    save(&conn, collected, &insights, &time).unwrap();
    let view = load(&conn, &time).unwrap();
    assert!(view.estimate.is_some());
    if let Ok(path) = std::env::var("QH_RADAR_REVIEW_OUTPUT") {
        fs::write(path, serde_json::to_vec_pretty(&view).unwrap()).unwrap();
    }
    eprintln!(
        "estimates={} community_authors={} independent={}",
        view.estimate.as_ref().unwrap().source_count,
        view.community.authors,
        view.community.independent_authors
    );
}
