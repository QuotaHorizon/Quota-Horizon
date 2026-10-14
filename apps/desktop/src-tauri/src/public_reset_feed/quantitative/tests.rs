use super::*;
use serde_json::{json, Value};

fn fixture() -> Value {
    let forecasts: Vec<_> = ["broad_quota_relief", "automatic_reset"].iter().map(|target| json!({
        "target": target, "status": "current", "issued_at": "2026-10-04T00:00:00Z", "valid_until": "2026-10-04T00:15:00Z",
        "model_version": "test-model", "training_state": "cold_start_prior", "evaluation_windows": 0, "event_count": 50,
        "curve": (1..=72).map(|h| json!({"hours":h,"probability": h as f64 / 100.0,"baseline":0.5,"component_range":[0.1,0.9]})).collect::<Vec<_>>(),
        "sources": [{"id":"private_signal","probabilities":vec![0.5;72],"source_points":null,"origin":null,"source_coverage_hours":null}],
        "community":{"probabilities":vec![0.3;72],"weight":0.5}, "raw_secret_provenance": "never serialize this"
    })).collect();
    json!({"schema_version":"reset-intelligence-consumer/1","forecasts":forecasts})
}
#[test]
fn separate_comment_score_crosses_boundary_without_overwriting_source_points() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("consumer.json");
    let mut value = fixture();
    value["forecasts"][0]["sources"] = json!([{
        "id":"codex_reset", "probabilities": vec![0.83;72], "source_points":[[24.,0.45],[48.,0.70]],
        "origin":1791072000., "source_coverage_hours":48.,
        "comment":{"probability":0.83,"url":"https://x.com/i/status/1234","at":1791068400.,
            "included":true,"state":"included","deadline":1791075600.,
            "timing_sources":["nextreset","private_signal"],"adjustment_24h":0.38,"adjustment_48h":0.13}
    }]);
    fs::write(&path, value.to_string()).unwrap();
    let parsed = parse_forecast(&path).unwrap();
    let source = &parsed.forecasts[0].sources[0];
    assert_eq!(source.source_points.as_ref().unwrap()[0], [24., 0.45]);
    assert_eq!(source.comment.as_ref().unwrap().probability, 0.83);
    assert_eq!(
        source.comment.as_ref().unwrap().timing_sources,
        ["nextreset"]
    );
    value["forecasts"][0]["sources"][0]["comment"]["url"] = json!("https://private.example/hidden");
    fs::write(&path, value.to_string()).unwrap();
    assert!(parse_forecast(&path).unwrap().forecasts[0].sources[0]
        .comment
        .is_none());
}
#[test]
fn announcement_context_retains_deadline_without_changing_probability() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("consumer.json");
    let mut value = fixture();
    value["forecasts"][0]["response"] = json!({
        "at": 1791068400., "url": "https://x.com/i/status/1234",
        "kind": "announcement", "deadline": 1791075600.,
        "reviewed": ["monitor", "nextreset", "private_signal"], "changed": []
    });
    fs::write(&path, value.to_string()).unwrap();
    let parsed = parse_forecast(&path).unwrap();
    let f = &parsed.forecasts[0];
    assert_eq!(f.curve[23].probability, 0.24);
    let context = f.response.as_ref().unwrap();
    assert_eq!(context.kind.as_deref(), Some("announcement"));
    assert_eq!(context.deadline, Some(1791075600.));
    assert_eq!(context.reviewed, ["monitor", "nextreset"]);
    value["forecasts"][0]["response"]["deadline"] = json!(1.);
    fs::write(&path, value.to_string()).unwrap();
    let parsed = parse_forecast(&path).unwrap();
    assert!(parsed.forecasts[0]
        .response
        .as_ref()
        .unwrap()
        .deadline
        .is_none());
}
#[test]
fn revision_breakdown_must_reconcile_to_the_published_probability() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("consumer.json");
    let mut value = fixture();
    let f = &mut value["forecasts"][0];
    f["combination_method"] = json!("baseline_plus_revisions");
    for p in f["curve"].as_array_mut().unwrap() {
        let value = p["probability"].as_f64().unwrap();
        p["baseline"] = json!(value * 0.4);
        p["radar_adjustment"] = json!(value * 0.6);
        p["community_adjustment"] = json!(0.);
        p["coherence_adjustment"] = json!(0.);
    }
    fs::write(&path, value.to_string()).unwrap();
    assert!(parse_forecast(&path).is_ok());
    value["forecasts"][0]["curve"][23]["radar_adjustment"] = json!(0.9);
    fs::write(&path, value.to_string()).unwrap();
    assert!(parse_forecast(&path).is_err());
}
#[test]
fn readonly_disconnected_does_not_create_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("not-created");
    assert!(!load(&path, &now()).connected);
    assert!(!path.exists());
}
#[test]
fn exports_are_validated_filtered_and_retained_on_partial_failure() {
    let root = tempfile::tempdir().unwrap();
    let f = root.path().join("consumer.json");
    let report = root.path().join("report.json");
    fs::write(&f, fixture().to_string()).unwrap();
    fs::write(
        root.path().join("reset-radar-connection.json"),
        json!({"forecastPath": f, "evaluationPath": report}).to_string(),
    )
    .unwrap();
    let good = load(root.path(), &now());
    assert!(good.connected);
    assert!(good.forecast_issue.is_none());
    assert_eq!(good.evaluation_issue.as_deref(), Some("missing"));
    let serialized = serde_json::to_string(&good).unwrap();
    assert!(!serialized.contains("private_signal") && !serialized.contains("provenance"));
    fs::write(&f, "{").unwrap();
    let retained = load(root.path(), &now());
    assert_eq!(retained.forecast_issue.as_deref(), Some("invalid"));
    assert!(retained.forecast.is_some());
    let mut invalid = fixture();
    invalid["forecasts"][0]["curve"][23]["probability"] = json!(1.1);
    fs::write(&f, invalid.to_string()).unwrap();
    assert!(parse_forecast(&f).is_err());
    invalid = fixture();
    invalid["forecasts"][0]["curve"][23]["probability"] = json!(0.01);
    fs::write(&f, invalid.to_string()).unwrap();
    assert!(parse_forecast(&f).is_err());
    invalid = fixture();
    invalid["schema_version"] = json!("next-schema");
    fs::write(&f, invalid.to_string()).unwrap();
    assert!(parse_forecast(&f).is_err());
}
#[test]
fn output_identity_and_links_are_allowlisted() {
    for id in [
        "private_signal",
        "internal_signal_abcdef",
        "date_poll",
        "without_private_signal",
    ] {
        assert!(!identity(&Identity {
            source: id.into(),
            version: "v1".into(),
            track: "community".into()
        }));
    }
    assert!(identity(&Identity {
        source: "without_community".into(),
        version: "v1".into(),
        track: "shadow".into()
    }));
    assert!(public_reference("https://x.com/i/status/123"));
    assert!(!public_reference("https://unknown.example/market"));
    assert!(!public_reference("https://user:secret@x.com/i/status/123"));
}

#[test]
fn applied_weights_and_sample_coverage_cross_only_the_public_boundary() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("consumer.json");
    let mut value = fixture();
    let f = &mut value["forecasts"][0];
    f["family_weights"] = json!({"baseline":0.15,"radars":0.75,"community":0.1});
    f["community_inputs"] = json!([
        {"id":"date_poll","weight":0.1,"votes":70,"effective_votes":8.2,"coverage_hours":36,
         "probability_24h":0.3,"probability_48h":0.5,"raw_source":"secret venue"},
        {"id":"secret venue","weight":0.,"votes":null,"effective_votes":null,"coverage_hours":null,
         "probability_24h":0.3,"probability_48h":0.5}]);
    f["community_coverage"] = json!({"included":1,"unavailable":1,"supplementary_included":0,
        "supplementary_checked":1,"supplementary_weight":0.,"url":"https://private.invalid"});
    fs::write(&path, value.to_string()).unwrap();
    let parsed = parse_forecast(&path).unwrap();
    assert_eq!(parsed.forecasts[0].community_inputs.len(), 1);
    assert_eq!(parsed.forecasts[0].community_inputs[0].votes, Some(70));
    let public = serde_json::to_string(&parsed).unwrap();
    assert!(!public.contains("secret venue") && !public.contains("private.invalid"));
    value["forecasts"][0]["community_inputs"][0]["effective_votes"] = json!(71.);
    fs::write(&path, value.to_string()).unwrap();
    assert!(parse_forecast(&path).is_err());
}

#[test]
fn scoring_progress_is_optional_bounded_and_keeps_only_aggregate_counts() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("report.json");
    let mut report = json!({
        "schema_version":"reset-observer/1", "generated_at":"2026-10-05T15:00:00Z",
        "started_at":"2026-10-03T16:00:00Z", "state":"scoring",
        "policy":{"reporting_grace_seconds":10800,"minimum_rank_windows":30,"minimum_rank_events":5},
        "summary":{"forecast_snapshots":500,"frozen_trials":264,"scored_nonoverlapping_trials":37,
            "settlements":{"settled":264},"next_maturity":null,"settlement_revisions":300},
        "latest":[],"curves":[],"scorecards":[],"comparisons":[],"outcomes":[],"outcome_providers":[]
    });
    fs::write(&path, report.to_string()).unwrap();
    assert!(parse_report(&path)
        .unwrap()
        .summary
        .scoring_by_horizon
        .is_none());
    report["summary"]["scoring_by_horizon"] = json!([{
        "hours":6,"settled_windows":12,"scored_windows":2,"scored_trials":37,
        "eligible_rolling_trials":218,"missing_forecast_trials":46,"private_source":"hidden"
    }]);
    fs::write(&path, report.to_string()).unwrap();
    let parsed = parse_report(&path).unwrap();
    let progress = parsed.summary.scoring_by_horizon.as_ref().unwrap();
    assert_eq!(progress[0].scored_trials, 37);
    assert_eq!(progress[0].scored_windows, 2);
    assert!(!serde_json::to_string(&parsed)
        .unwrap()
        .contains("private_source"));
    let mut duplicate = report.clone();
    let first = duplicate["summary"]["scoring_by_horizon"][0].clone();
    duplicate["summary"]["scoring_by_horizon"]
        .as_array_mut()
        .unwrap()
        .push(first);
    fs::write(&path, duplicate.to_string()).unwrap();
    assert!(parse_report(&path).is_err());
    for (field, invalid) in [
        ("hours", 5),
        ("scored_windows", 13),
        ("scored_trials", 219),
        ("missing_forecast_trials", 265),
    ] {
        let mut value = report.clone();
        value["summary"]["scoring_by_horizon"][0][field] = json!(invalid);
        fs::write(&path, value.to_string()).unwrap();
        assert!(parse_report(&path).is_err(), "accepted invalid {field}");
    }
}
#[test]
#[ignore = "explicit read-only installed public journal and independent exports"]
fn capture_installed_connection() {
    let path = PathBuf::from(
        std::env::var("HORIZON_PUBLIC_LEDGER_TEST_PATH").expect("explicit public journal path"),
    );
    assert_eq!(path.file_name().unwrap(), DB_NAME);
    let timeline = load_timeline(&path, &now()).unwrap();
    let q = &timeline.quantitative;
    assert!(
        q.connected && q.forecast_issue.is_none() && q.evaluation_issue.is_none(),
        "{q:?}"
    );
    let f = q.forecast.as_ref().unwrap();
    let r = q.evaluation.as_ref().unwrap();
    assert_eq!(f.forecasts.len(), 2);
    assert!(!r.latest.is_empty());
    println!(
        "connected: targets={}, forecasts={}, scorecards={}, outcomes={}, report={}",
        f.forecasts.len(),
        r.latest.len(),
        r.scorecards.len(),
        r.outcomes.len(),
        r.generated_at
    );
    if let Ok(output) = std::env::var("HORIZON_PUBLIC_CAPTURE_PATH") {
        fs::write(output, serde_json::to_vec(&timeline).unwrap()).unwrap();
    }
}
