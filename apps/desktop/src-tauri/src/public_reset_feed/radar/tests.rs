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
    let mut stale = forecast("old_history", "cadence", 99.0);
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
    for report in [
        "Codex reset history shows two redemption entries; please verify whether this is correct.",
        "Codex usage displays 93% remaining and resets in 19h. Explain whether the countdown is correct.",
    ] {
        assert_eq!(community::classify(report).unwrap().0, "observation");
    }
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
        ("optimistic", "personal_inference", Some(24))
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
        partial: false,
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
        Some("method_unverified")
    );
    for p in &mut v.community.opinions {
        p.text.push_str(" Tibo says");
    }
    model::evaluate(&mut v, &at());
    assert_eq!(v.community.independent_authors, 0);
    assert_eq!(v.community.effects[0].eligible_authors, 3);
    assert!(v.community.effects[0].effective_authors <= 1.0);
    for p in &mut v.community.opinions {
        p.reason = "quoted_evidence".into();
    }
    model::evaluate(&mut v, &at());
    assert_eq!(v.estimate.as_ref().unwrap().community_adjustment_24h, 0.0);
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
        polls: vec![],
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
    assert!(uses_current_model(&first));
    assert_eq!(
        first.history[0].model_version.as_deref(),
        Some(MODEL_VERSION)
    );
    assert_eq!(first.forecasts[0].history.len(), 1);
    assert!(load_snapshot(&conn, &at()).unwrap().forecasts[0]
        .history
        .is_empty());
    let later = UtcTimestamp::parse("2026-10-03T08:15:00.000Z").unwrap();
    save(
        &conn,
        Collected {
            forecasts: vec![("reset_monitor", Err(SourceIssue::RequestFailed))],
            community: vec![],
            polls: vec![],
        },
        &insights::PublicInsights::default(),
        &later,
    )
    .unwrap();
    let latest = load(&conn, &later).unwrap();
    assert!(latest.estimate.is_none());
    assert!(uses_current_model(&latest));
    assert_eq!(latest.forecasts[0].probability_24h, Some(40.0));
    assert_eq!(latest.history.len(), 1);
    assert_eq!(latest.forecasts[0].history.len(), 1);
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
    assert!(collected
        .forecasts
        .iter()
        .any(|(id, r)| *id == "reset_monitor" && r.is_ok()));
    assert!(!collected.forecasts.iter().any(|(id, _)| *id == "reset_app"));
    assert!(collected
        .forecasts
        .iter()
        .any(|(id, r)| *id == "nextreset" && r.is_ok()));
    for (id, result) in &collected.polls {
        eprintln!(
            "{id}: {}",
            if result.is_ok() { "ok" } else { "unavailable" }
        );
        assert!(result.is_ok(), "public poll {id}: {result:?}");
    }
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

#[test]
fn admission_rejects_retired_opaque_and_undated_inputs_even_when_prices_look_plausible() {
    let mut undated = forecast("undated", "cadence", 60.0);
    undated.updated_at = None;
    let mut v = RadarView {
        forecasts: vec![
            forecast("codex_reset", "cadence", 20.0),
            forecast("reset_app", "mixed", 50.0),
            forecast("quota_cue", "mixed", 21.0),
            undated,
        ],
        ..Default::default()
    };
    model::evaluate(&mut v, &at());
    assert_eq!(v.estimate.unwrap().source_count, 1);
    assert_eq!(v.forecasts[1].exclusion.as_deref(), Some("source_retired"));
    assert_eq!(
        v.forecasts[2].exclusion.as_deref(),
        Some("method_unverified")
    );
    assert_eq!(
        v.forecasts[3].exclusion.as_deref(),
        Some("missing_timestamp")
    );
}

fn sampled_view(opinions: Vec<Opinion>) -> RadarView {
    let mut v = RadarView {
        forecasts: vec![forecast("codex_reset", "cadence", 30.0)],
        ..Default::default()
    };
    for id in ["github", "hacker_news"] {
        v.community.channels.push(Channel {
            id: id.into(),
            name: id.into(),
            url: "https://github.com".into(),
            query: "test".into(),
            attempted_at: at().as_str().into(),
            success_at: Some(at().as_str().into()),
            issue: None,
            scanned: opinions.len(),
            truncated: false,
            partial: false,
        });
    }
    v.community.opinions = opinions;
    v
}

#[test]
fn partial_community_reads_preserve_prior_samples_and_bound_repeated_merges() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_store(&dir.path().join("public.sqlite"), true).unwrap();
    let mut channel = sampled_view(vec![]).community.channels.remove(0);
    for (offset, partial) in [(0, false), (20, true), (40, true)] {
        channel.partial = partial;
        channel.issue = partial.then_some(SourceIssue::RequestFailed);
        save(
            &conn,
            Collected {
                forecasts: vec![],
                polls: vec![],
                community: vec![(
                    channel.clone(),
                    (offset..offset + 30)
                        .map(|i| opinion(&format!("author{i}"), &format!("independent text {i}")))
                        .collect(),
                )],
            },
            &insights::PublicInsights::default(),
            &at(),
        )
        .unwrap();
        let v = load_snapshot(&conn, &at()).unwrap();
        assert_eq!(v.community.opinions.len(), (offset + 30).min(64));
        assert_eq!(
            v.community
                .opinions
                .iter()
                .map(|p| &p.id)
                .collect::<HashSet<_>>()
                .len(),
            v.community.opinions.len()
        );
        assert_eq!(v.community.channels[0].truncated, offset + 30 > 64);
        assert_eq!(v.community.channels[0].partial, partial);
    }
}

#[test]
fn community_has_continuous_strength_and_respects_negative_horizons() {
    let mut v = sampled_view(vec![opinion("a", "first independent view")]);
    model::evaluate(&mut v, &at());
    let single = v.estimate.as_ref().unwrap().community_adjustment_24h;
    assert!(single > 0.0);
    v.community.opinions.extend((0..40).map(|i| {
        let mut p = opinion(&format!("person{i}"), &format!("different view {i}"));
        p.channel = if i % 2 == 0 { "github" } else { "hacker_news" }.into();
        p
    }));
    model::evaluate(&mut v, &at());
    let strong = v.estimate.as_ref().unwrap().community_adjustment_24h;
    assert!(strong > 15.0 && strong < 35.0);
    for p in &mut v.community.opinions {
        p.stance = "pessimistic".into();
    }
    model::evaluate(&mut v, &at());
    assert!(v.estimate.as_ref().unwrap().community_adjustment_24h < 0.0);
    assert_eq!(v.estimate.as_ref().unwrap().community_adjustment_48h, 0.0);
}

#[test]
fn shared_reference_sets_merge_transitively_and_quotes_are_not_new_forecasts() {
    let mut a = opinion("a", "My first personal inference from the announcement");
    let mut b = opinion("b", "My second inference after reviewing two announcements");
    let mut c = opinion("c", "My third inference after the second announcement");
    a.evidence_urls = vec!["https://x.com/thsottiaux/status/111".into()];
    b.evidence_urls = vec![
        "https://x.com/thsottiaux/status/111?ref=copy".into(),
        "https://x.com/thsottiaux/status/222".into(),
    ];
    c.evidence_urls = vec!["https://x.com/thsottiaux/status/222".into()];
    let mut v = sampled_view(vec![a, b, c]);
    model::evaluate(&mut v, &at());
    assert_eq!(v.community.effects[0].eligible_authors, 3);
    assert_eq!(v.community.effects[0].effective_authors, 1.0);
    assert_eq!(
        community::classify("Tibo says Codex will reset today")
            .unwrap()
            .0,
        "observation"
    );
    assert_eq!(
        community::classify("After Tibo's post, I think Codex will reset today")
            .unwrap()
            .0,
        "optimistic"
    );
}

#[test]
fn expired_and_pre_reset_predictions_do_not_drive_the_next_event() {
    let mut past = opinion("past", "old event prediction");
    past.published_at = "2026-10-02T20:00:00Z".into();
    let mut expired = opinion("expired", "expired window prediction");
    expired.published_at = "2026-10-02T07:00:00Z".into();
    let mut v = sampled_view(vec![past, expired]);
    model::evaluate(&mut v, &at());
    assert_eq!(v.community.effects[0].eligible_authors, 0);
    assert_eq!(v.estimate.unwrap().community_adjustment_24h, 0.0);
}

fn poll_time() -> UtcTimestamp {
    UtcTimestamp::parse("2026-10-03T12:20:00.000Z").unwrap()
}
#[test]
fn nextreset_admission_checks_probability_scale_expiry_and_recent_event() {
    let bytes = include_bytes!("fixtures/nextreset-forecast.json");
    let f = sources::parse("nextreset", bytes, &poll_time()).unwrap();
    assert!((f.probability_24h.unwrap() - 12.3859).abs() < 0.001);
    assert_eq!(f.method, "cadence");
    assert!(!f.uses_community);
    let mut v = RadarView {
        forecasts: vec![f.clone()],
        ..Default::default()
    };
    model::evaluate(&mut v, &poll_time());
    assert_eq!(v.estimate.as_ref().unwrap().source_count, 1);
    model::evaluate(
        &mut v,
        &UtcTimestamp::parse("2026-10-03T12:31:00Z").unwrap(),
    );
    assert_eq!(v.forecasts[0].exclusion.as_deref(), Some("stale"));
    let mut json: Value = serde_json::from_slice(bytes).unwrap();
    json["windows"][0]["probability"] = serde_json::json!(1.2);
    assert!(sources::parse(
        "nextreset",
        &serde_json::to_vec(&json).unwrap(),
        &poll_time()
    )
    .is_err());
    json["windows"][0]["probability"] = serde_json::json!(0.6);
    assert!(sources::parse(
        "nextreset",
        &serde_json::to_vec(&json).unwrap(),
        &poll_time()
    )
    .is_err());
    let mut reviewed = f;
    reviewed.method = "mixed".into();
    let mut v = RadarView {
        forecasts: vec![reviewed],
        ..Default::default()
    };
    model::evaluate(&mut v, &poll_time());
    assert!(v.forecasts[0].exclusion.is_none());
}
#[test]
fn repeats_use_generation_time_and_preserve_first_observation() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_store(&dir.path().join("radar.sqlite"), true).unwrap();
    for (i, at_str) in [
        "2026-10-03T08:00:00Z",
        "2026-10-03T08:15:00Z",
        "2026-10-03T08:30:00Z",
    ]
    .iter()
    .enumerate()
    {
        let time = UtcTimestamp::parse(*at_str).unwrap();
        let mut f = forecast(
            "reset_monitor",
            "statements",
            if i == 2 { 41.0 } else { 40.0 },
        );
        f.updated_at = Some(
            if i == 2 {
                "2026-10-03T08:25:00Z"
            } else {
                "2026-10-03T07:50:00Z"
            }
            .into(),
        );
        f.collected_at = time.as_str().into();
        save(
            &conn,
            Collected {
                forecasts: vec![("reset_monitor", Ok(f))],
                community: vec![],
                polls: vec![],
            },
            &insights::PublicInsights::default(),
            &time,
        )
        .unwrap();
    }
    let v = load(&conn, &UtcTimestamp::parse("2026-10-03T08:30:00Z").unwrap()).unwrap();
    assert_eq!(v.history.len(), 3);
    let p = &v.forecasts[0].history;
    assert_eq!(p.len(), 2);
    assert_eq!(p[0].at, "2026-10-03T07:50:00Z");
    assert_eq!(
        p[0].observed_at.as_ref().and_then(|t| seconds(t)),
        seconds("2026-10-03T08:00:00Z")
    );
    assert_eq!(p[1].at, "2026-10-03T08:25:00Z");
    assert!(
        load_snapshot(&conn, &UtcTimestamp::parse("2026-10-03T08:30:00Z").unwrap())
            .unwrap()
            .forecasts[0]
            .history
            .is_empty()
    );
}
#[test]
fn polls_preserve_deadlines_check_counts_and_limit_anonymous_influence() {
    let time = poll_time();
    let n = polls::parse(include_bytes!("fixtures/nextreset-poll.json"), &time).unwrap();
    assert_eq!(n.round.mean_probability, Some(30.0));
    assert_eq!(n.round.samples, 1);
    assert_eq!(n.round.ends_at, "2026-10-04T07:00:00.000Z");
    // A negative for the next 18h cannot imply a negative for the next 24h.
    assert_eq!(
        polls::signal(&[n], 24, seconds(time.as_str()).unwrap(), None),
        (0.0, 0.0, 0)
    );
    let p = polls::parse_timing(include_bytes!("fixtures/codex-reset-poll.json"), &time).unwrap();
    assert_eq!(p.round.samples, 8);
    let clock = seconds(time.as_str()).unwrap();
    let (d, e, n) = polls::signal(&[p.clone()], 24, clock, None);
    assert!(d < 0.0 && e > 0.0);
    assert_eq!(n, 7);
    let (_, e, _) = polls::signal(&vec![p.clone(); 100], 24, clock, None);
    assert!(e <= 1.0);
    assert_eq!(
        polls::signal(&[p.clone()], 24, clock, Some(clock - 1)),
        (0.0, 0.0, 0)
    );
    let mut stale = p.clone();
    stale.issue = Some(SourceIssue::HttpError);
    assert_eq!(polls::signal(&[stale], 24, clock, None), (0.0, 0.0, 0));
    let mut bad: Value =
        serde_json::from_slice(include_bytes!("fixtures/codex-reset-poll.json")).unwrap();
    bad["tally"]["total_votes"] = serde_json::json!(99);
    assert!(polls::parse_timing(&serde_json::to_vec(&bad).unwrap(), &time).is_err());
    bad["tally"]["unlocked"] = serde_json::json!(false);
    let hidden = polls::parse_timing(&serde_json::to_vec(&bad).unwrap(), &time).unwrap();
    assert!(hidden.round.distribution.is_empty());
    assert_eq!(polls::signal(&[hidden], 24, clock, None), (0.0, 0.0, 0));
    let mut v = RadarView {
        forecasts: vec![forecast("codex_reset", "cadence", 30.0)],
        ..Default::default()
    };
    v.forecasts[0].collected_at = time.as_str().into();
    v.forecasts[0].updated_at = Some(time.as_str().into());
    v.community.polls = vec![p];
    model::evaluate(&mut v, &time);
    assert!(v.estimate.as_ref().unwrap().community_adjustment_24h < 0.0);
    assert_eq!(v.community.authors, 0);
    assert_eq!(v.community.effects[0].poll_samples, 7);
}
#[test]
fn credit_issuance_predictions_inform_sentiment_but_redemption_and_wishes_do_not() {
    assert_eq!(community::classify("Codex usage unexpectedly changed to 2% used. I did not redeem a banked reset. The unexpected refill is not itself the problem.").unwrap().0, "observation");
    for prediction in [
        "I expect another banked reset for Codex tonight",
        "Tibo will give us a reset credit tomorrow",
        "我认为 Codex 今天大概率发卡",
    ] {
        assert_eq!(community::classify(prediction).unwrap().0, "optimistic");
    }
    assert_eq!(
        community::classify("I hope Codex will give another banked reset tonight")
            .unwrap()
            .0,
        "wish"
    );
    assert_eq!(
        community::classify("My Codex banked reset was received tonight")
            .unwrap()
            .0,
        "observation"
    );
    assert_eq!(
        community::classify("我认为 Codex 今天不会发卡").unwrap().0,
        "pessimistic"
    );
}
