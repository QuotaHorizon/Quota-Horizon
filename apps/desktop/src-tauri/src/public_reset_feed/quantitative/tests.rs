use super::*;
use serde_json::{json, Value};

// load() keeps one process-wide saved connection; retention tests must not
// replace each other's cache between successive reads.
static LOAD_TEST: Mutex<()> = Mutex::new(());

// Synthetic history, not a projection of production growth: four historical
// versions of twelve public identities, sampled every fifteen minutes over
// fourteen days. Additional days extend native sources and introduce one daily
// model revision, with real points, day windows, scores and pair comparisons.
fn content_report(extra_days: usize) -> Value {
    let start = DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z").unwrap();
    let at = |quarter: usize| {
        (start + chrono::Duration::minutes((quarter * 15) as i64))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    };
    let sources = [
        ("horizon", "model"),
        ("codex_reset", "native"),
        ("nextreset", "native"),
        ("observatory", "native"),
        ("lunar", "native"),
        ("dreaife", "native"),
        ("survival_baseline", "shadow"),
        ("without_community", "shadow"),
        ("without_radars", "shadow"),
        ("without_information", "shadow"),
        ("without_comment_estimates", "shadow"),
        ("equal_components", "shadow"),
    ];
    let mut identities = Vec::new();
    let mut curves = Vec::new();
    let mut latest = Vec::new();
    let mut scorecards = Vec::new();
    let mut comparisons = Vec::new();
    let end = (14 + extra_days) * 96;
    for index in 0..48 + extra_days {
        let (source, track) = if index < 48 {
            sources[index % 12]
        } else {
            sources[0]
        };
        let version = if index < 48 {
            index / 12
        } else {
            index - 48 + 4
        };
        let identity =
            json!({"source":source,"track":track,"version":format!("synthetic-v{version}")});
        let first = if index < 48 {
            version * 336
        } else {
            14 * 96 + (index - 48) * 96
        };
        let last = if index < 36 {
            first + 336
        } else if index < 48 && source != "horizon" {
            end
        } else if index < 48 {
            14 * 96
        } else {
            first + 96
        };
        for target in ["broad_quota_relief", "automatic_reset"] {
            let p = 0.125 + (index % 12) as f64 / 32.;
            let mut item = identity.clone();
            item["target"] = json!(target);
            item["origin"] = json!(at(last - 1));
            item["received_at"] = json!(at(last - 1));
            item["valid_until"] = json!(at(last));
            item["state"] = json!("current");
            item["probabilities_at_publication"] = json!({"24":p,"48":p + 0.125});
            latest.push(item);
            for hours in [24, 48] {
                let probability = p + if hours == 48 { 0.125 } else { 0. };
                let mut curve = identity.clone();
                curve["target"] = json!(target);
                curve["hours"] = json!(hours);
                curve["points"] = json!((first..last)
                    .map(|q| json!({
                        "at":at(q), "p":if q % 97 == 1 { None } else { Some(probability) }
                    }))
                    .collect::<Vec<_>>());
                curves.push(curve);
            }
            for hours in [6, 12, 24, 48] {
                let n = (last - first) / (hours * 4);
                let mut card = identity.clone();
                card["target"] = json!(target);
                card["hours"] = json!(hours);
                card["scheduled_windows"] = json!(n);
                card["forecast_coverage"] = json!(1.);
                card["settlement_states"] = json!({"settled":n});
                card["scores"] = json!({"n":n,"unique_events":0,"brier":p*p,
                    "log_loss":-(1.-p).ln(),"state":"insufficient_events",
                    "brier_interval":{"mean":p*p,"ci95":null,"blocks":n},
                    "calibration":[{"n":n,"mean_probability":p,"event_frequency":0.}]});
                card["observed_event_scores"] = Value::Null;
                card["alerts"] = json!({"threshold":0.5,"episodes":0,"false_alerts":0,
                    "precision":null,"event_recall":null,"median_lead_hours":null});
                scorecards.push(card);
            }
        }
        identities.push(identity);
    }
    for (i, left) in identities.iter().enumerate() {
        for right in identities.iter().skip(i + 1) {
            // Different historical versions have no shared evaluation window.
            // Keep every zero-sample comparison rather than pruning empty pairs.
            for target in ["broad_quota_relief", "automatic_reset"] {
                for hours in [6, 12, 24, 48] {
                    let shared = left["version"] == right["version"];
                    let left_p = 0.125 + (i % 12) as f64 / 32.;
                    let right_index = sources
                        .iter()
                        .position(|(s, _)| *s == right["source"].as_str().unwrap())
                        .unwrap();
                    let right_p = 0.125 + right_index as f64 / 32.;
                    let difference = if shared {
                        Some(left_p * left_p - right_p * right_p)
                    } else {
                        None
                    };
                    comparisons.push(json!({"left":left,"right":right,"target":target,
                        "hours":hours,"n":usize::from(shared),"unique_events":0,"brier_skill":null,
                        "loss_difference":{"mean":difference,"ci95":null,"blocks":usize::from(shared)},
                        "error_correlation":null}));
                }
            }
        }
    }
    let mut private_curve = curves[0].clone();
    private_curve["source"] = json!("private_signal");
    curves.push(private_curve);
    json!({"schema_version":"reset-observer/1","generated_at":at(end),"started_at":at(0),
        "state":"scoring","policy":{"reporting_grace_seconds":10800,"minimum_rank_windows":30,"minimum_rank_events":5},
        "summary":{"forecast_snapshots":end*12,"frozen_trials":end,"scored_nonoverlapping_trials":14+extra_days,
            "settlements":{"settled":14+extra_days},"next_maturity":null,"settlement_revisions":end},
        "latest":latest,"curve_basis":"received_publication_values","curves":curves,
        "scorecards":scorecards,"comparisons":comparisons,
        "outcomes":[{"id":"synthetic-合成-🌅","status":"unconfirmed","reason":null,
            "kind":"automatic_reset","from":null,"to":null,"announced_at":null,
            "known_at":1790812800.,"references":["https://x.com/i/status/123","https://private.invalid/hidden"],
            "primary_references":0,"completion_reports":0}],"outcome_providers":[],
        "calendar_sentiment":{"version":"calendar-preference-increments-1","production_weight":0.,
            "automatic_promotion":false,"vote_units":"clicks_not_unique_people",
            "trials":(0..14+extra_days).map(|day| json!({"id":format!("synthetic-day-{day}"),
                "features":{"window_start":1790812800+day*86400,"window_end":1790812800+(day+1)*86400,
                    "unique_participants":null,"production_weight":0.},
                "candidates":{"raw_click_share":0.375,"repeat_sensitive_share":0.5},
                "settlement":null})).collect::<Vec<_>>()},
        "diagnostics":{"note":"合成内容 / café / 🌅","private_provenance":"never serialize this"}})
}

#[test]
fn pretty_and_compact_reports_preserve_filtered_content_and_cold_growth() {
    let _guard = LOAD_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let mut previous_size = 0;
    for extra_days in [0, 1, 7] {
        let value = content_report(extra_days);
        assert_eq!(
            value["calendar_sentiment"]["trials"]
                .as_array()
                .unwrap()
                .len(),
            14 + extra_days
        );
        let mut pretty = serde_json::to_vec_pretty(&value).unwrap();
        let mut compact = serde_json::to_vec(&value).unwrap();
        pretty.push(b'\n');
        compact.push(b'\n');
        assert!(
            serde_json::from_slice::<Value>(&pretty).unwrap()
                == serde_json::from_slice::<Value>(&compact).unwrap(),
            "pretty/compact decoded content differs at +{extra_days} days"
        );
        assert!(compact.len() > previous_size && compact.len() < pretty.len());
        assert!(pretty.len() < REPORT_LIMIT as usize);
        previous_size = compact.len();
        let mut expected = None;
        for (encoding, bytes) in [("pretty", &pretty), ("compact", &compact)] {
            // New connection paths guarantee no saved evaluation for this load.
            let root = tempfile::tempdir().unwrap();
            let report = root.path().join("report.json");
            let forecast = root.path().join("consumer.json");
            fs::write(&forecast, fixture().to_string()).unwrap();
            fs::write(
                root.path().join("reset-radar-connection.json"),
                json!({"forecastPath":forecast,"evaluationPath":report}).to_string(),
            )
            .unwrap();
            let missing = load(root.path(), &now());
            assert_eq!(missing.evaluation_issue.as_deref(), Some("missing"));
            assert!(missing.evaluation.is_none());
            fs::write(&report, bytes).unwrap();
            let started = std::time::Instant::now();
            let loaded = load(root.path(), &now());
            let load_elapsed = started.elapsed();
            assert!(loaded.connected && loaded.forecast_issue.is_none());
            assert_eq!(loaded.evaluation_issue, None);
            let parsed = loaded.evaluation.unwrap();
            assert_eq!(parsed.curves.len(), (48 + extra_days) * 4);
            assert_eq!(parsed.curves[0].points[0].p, Some(0.125));
            assert_eq!(parsed.curves[0].points[1].p, None);
            assert_eq!(parsed.curves[0].points[2].p, Some(0.125));
            assert_eq!(parsed.curves[4].points[0].p, Some(0.15625));
            assert_eq!(parsed.curves[0].identity.version, "synthetic-v0");
            assert_eq!(parsed.curves[48].identity.version, "synthetic-v1");
            assert_eq!(parsed.scorecards.len(), (48 + extra_days) * 8);
            assert_eq!(
                parsed.comparisons.len(),
                (48 + extra_days) * (47 + extra_days) * 4
            );
            assert!(parsed.comparisons.iter().any(|c| c.n == 0));
            assert!(parsed.comparisons.iter().any(|c| c.n > 0));
            assert_eq!(parsed.outcomes[0].id, "synthetic-合成-🌅");
            assert_eq!(parsed.outcomes[0].references.len(), 1);
            let filtered = serde_json::to_value(&parsed).unwrap();
            let serialized = serde_json::to_string(&filtered).unwrap();
            assert!(
                !serialized.contains("private_signal")
                    && !serialized.contains("private_provenance")
            );
            if let Some(expected) = &expected {
                assert!(&filtered == expected, "filtered encodings differ");
            } else {
                expected = Some(filtered.clone());
            }
            let started = std::time::Instant::now();
            let directly_parsed = parse_report(&report).unwrap();
            let parse_elapsed = started.elapsed();
            assert!(serde_json::to_value(directly_parsed).unwrap() == filtered);
            println!("content-growth extra_days={extra_days} encoding={encoding} bytes={} curves={} scorecards={} comparisons={} cold_connection_load_ms={:.3} repeat_parse_report_ms={:.3}",
                bytes.len(), parsed.curves.len(), parsed.scorecards.len(), parsed.comparisons.len(),
                load_elapsed.as_secs_f64()*1000., parse_elapsed.as_secs_f64()*1000.);
        }
    }
}

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
fn outcome_provider_states_are_optional_and_allowlisted() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("report.json");
    let mut report = json!({
        "schema_version":"reset-observer/1", "generated_at":"2026-10-17T00:00:00Z",
        "started_at":"2026-10-03T00:00:00Z", "state":"scoring",
        "policy":{"reporting_grace_seconds":10800,"minimum_rank_windows":30,"minimum_rank_events":5},
        "summary":{"forecast_snapshots":0,"frozen_trials":0,"scored_nonoverlapping_trials":0,
            "settlements":{},"next_maturity":null,"settlement_revisions":0},
        "latest":[],"curves":[],"scorecards":[],"comparisons":[],
        "outcomes":[{"id":"completed-scope-pending","status":"pending","kind":"automatic",
            "from":1792045693.,"to":1792045693.,"announced_at":1792045693.,"known_at":1792053933.,
            "references":["https://x.com/i/status/2110618732666257557"],
            "primary_references":1,"completion_reports":1}],
        "outcome_providers":[{"provider":"quota_events","last_attempt":"2026-10-17T00:00:00Z",
            "generated_at":"2026-10-13T00:00:00Z","healthy":true}]
    });
    for state in [
        None,
        Some("healthy"),
        Some("positive_evidence"),
        Some("collector_overdue"),
        Some("request_or_decode_failure"),
        Some("stale_source_content"),
        Some("incomplete_source_evidence"),
        Some("private_diagnostic_content"),
    ] {
        report["outcome_providers"][0]["state"] = json!(state);
        fs::write(&path, report.to_string()).unwrap();
        let parsed = parse_report(&path).unwrap();
        let expected = state.filter(|s| *s != "private_diagnostic_content");
        assert_eq!(parsed.outcome_providers[0].state.as_deref(), expected);
        assert!(parsed.outcome_providers[0].healthy);
        assert!(!serde_json::to_string(&parsed)
            .unwrap()
            .contains("private_diagnostic_content"));
    }
    for reason in [
        None,
        Some("population_unspecified"),
        Some("conditional_pledge"),
        Some("insufficient_primary_confirmation"),
        Some("completion_withdrawn"),
        Some("private_diagnostic_content"),
    ] {
        report["outcomes"][0]["reason"] = json!(reason);
        fs::write(&path, report.to_string()).unwrap();
        let parsed = parse_report(&path).unwrap();
        assert_eq!(
            parsed.outcomes[0].reason.as_deref(),
            reason.filter(|r| *r != "private_diagnostic_content")
        );
        assert_eq!(parsed.outcomes[0].status, "pending");
        assert_eq!(parsed.outcomes[0].completion_reports, 1);
        assert_eq!(parsed.outcomes[0].to, Some(1792045693.));
    }
    report["outcome_providers"][0]
        .as_object_mut()
        .unwrap()
        .remove("state");
    fs::write(&path, report.to_string()).unwrap();
    assert!(parse_report(&path).unwrap().outcome_providers[0]
        .state
        .is_none());
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
    assert!(source.comment.as_ref().unwrap().timing_evidence.is_empty());
    assert!(serde_json::to_value(source).unwrap()["comment"]
        .get("timing_evidence")
        .is_none());
    value["forecasts"][0]["sources"][0]["comment"]["url"] = json!("https://private.example/hidden");
    fs::write(&path, value.to_string()).unwrap();
    assert!(parse_forecast(&path).unwrap().forecasts[0].sources[0]
        .comment
        .is_none());
}
#[test]
fn comment_timing_evidence_retains_public_links_and_filters_private_provenance() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("consumer.json");
    let mut value = fixture();
    let direct = json!({
        "source":"nextreset", "deadline":1791075600., "relationship":"direct_reply",
        "estimate_url":"https://x.com/i/status/1234", "timing_url":"https://x.com/thsottiaux/status/5678",
        "links":[{"child_url":"https://x.com/thsottiaux/status/5678", "parent_url":"https://x.com/i/status/1234",
            "at":1791070200., "source":"tibo_posts", "private_provenance":"never serialize this"}],
        "private_provenance":"never serialize this"
    });
    let mut same = direct.clone();
    same["relationship"] = json!("same_reference");
    same["timing_url"] = same["estimate_url"].clone();
    same["links"] = json!([]);
    let mut invalid = Vec::new();
    for key in ["source", "relationship", "estimate_url", "timing_url"] {
        let mut row = direct.clone();
        row[key] = json!("https://private.invalid/hidden");
        invalid.push(row);
    }
    for key in ["source", "child_url", "parent_url"] {
        let mut row = direct.clone();
        row["links"][0][key] = json!("https://private.invalid/hidden");
        invalid.push(row);
    }
    for url in [
        "http://x.com/i/status/1234",
        "https://user:secret@x.com/i/status/1234",
        "javascript:alert(1)",
    ] {
        let mut row = direct.clone();
        row["links"][0]["child_url"] = json!(url);
        invalid.push(row);
    }
    let mut missing_source = direct.clone();
    missing_source["links"][0]["source"] = Value::Null;
    invalid.push(missing_source);
    let mut bad_time = direct.clone();
    bad_time["deadline"] = json!(-1.);
    invalid.push(bad_time);
    let mut future_link = direct.clone();
    future_link["links"][0]["at"] = json!(1791075601.);
    invalid.push(future_link);
    let mut mixed = direct.clone();
    mixed["links"].as_array_mut().unwrap().push(json!({
        "child_url":"https://private.invalid/hidden", "parent_url":"https://x.com/i/status/1234",
        "at":1791070200., "source":"private_signal"
    }));
    value["forecasts"][0]["sources"] = json!([{
        "id":"codex_reset", "probabilities":vec![0.93;72], "source_points":[[24.,0.45],[48.,0.70]],
        "origin":1791072000., "source_coverage_hours":48.,
        "comment":{"probability":0.93,"url":"https://x.com/i/status/1234","at":1791068400.,
            "included":true,"state":"included","deadline":1791075600.,
            "timing_sources":["nextreset"],"adjustment_24h":0.48,"adjustment_48h":0.23,
            "timing_evidence":[mixed,same]}
    }]);
    value["forecasts"][0]["sources"][0]["comment"]["timing_evidence"]
        .as_array_mut()
        .unwrap()
        .extend(invalid);
    fs::write(&path, value.to_string()).unwrap();
    let parsed = parse_forecast(&path).unwrap();
    let comment = parsed.forecasts[0].sources[0].comment.as_ref().unwrap();
    assert_eq!(comment.probability, 0.93);
    assert_eq!(comment.timing_evidence.len(), 2);
    let evidence = &comment.timing_evidence[0];
    assert_eq!(evidence.source, "nextreset");
    assert_eq!(evidence.relationship, "direct_reply");
    assert_eq!(evidence.links.len(), 1);
    assert_eq!(evidence.links[0].source.as_deref(), Some("tibo_posts"));
    assert_eq!(evidence.links[0].at, 1791070200.);
    assert_eq!(comment.timing_evidence[1].relationship, "same_reference");
    let serialized = serde_json::to_string(&parsed).unwrap();
    for private in [
        "private.invalid",
        "private_signal",
        "private_provenance",
        "secret",
        "javascript:",
    ] {
        assert!(!serialized.contains(private), "leaked {private}");
    }
}
#[test]
fn calendar_sentiment_compact_view_is_optional_and_filters_research_details() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("report.json");
    let mut value = json!({
        "schema_version":"reset-observer/1", "generated_at":"2026-10-17T00:00:00Z",
        "started_at":"2026-10-03T00:00:00Z", "state":"scoring",
        "policy":{"reporting_grace_seconds":10800,"minimum_rank_windows":30,"minimum_rank_events":5},
        "summary":{"forecast_snapshots":0,"frozen_trials":0,"scored_nonoverlapping_trials":0,
            "settlements":{},"next_maturity":null,"settlement_revisions":0},
        "latest":[],"curves":[],"scorecards":[],"comparisons":[],"outcomes":[],"outcome_providers":[]
    });
    fs::write(&path, value.to_string()).unwrap();
    assert!(parse_report(&path).unwrap().calendar_sentiment.is_none());
    // The research shape in older reports remains readable without suggesting
    // that missing compact counts mean no stored trials.
    value["calendar_sentiment"] = json!({"production_weight":0.,"scored_days":1,
        "trials":[{"private":"hidden"}],"latest":[]});
    fs::write(&path, value.to_string()).unwrap();
    assert!(parse_report(&path).unwrap().calendar_sentiment.is_none());
    let sample = json!({"source":"didcodex_poll","date":"2026-10-16",
        "window_start":1792123200., "window_end":1792209600., "observed_at":1792188000.,
        "recent_votes":512,"total_clicks":2048,"weighted_reset_share":0.72,"reset_share":0.75,
        "latest_vote_at":1792187940.,"state":"ready_for_evaluation",
        "private_provenance":"never serialize this"});
    let mut samples = vec![sample.clone()];
    for (key, invalid) in [
        ("source", json!("private_signal")),
        ("date", json!("2026-99-99")),
        ("date", json!("private arbitrary label")),
        ("state", json!("unreviewed")),
        ("window_start", json!(-1.)),
        ("window_end", json!(1792123200.)),
        ("observed_at", json!(1992188000.)),
        ("latest_vote_at", json!(1992188000.)),
        ("recent_votes", json!(2049)),
        ("reset_share", json!(1.1)),
        ("weighted_reset_share", json!(-0.1)),
    ] {
        let mut invalid_sample = sample.clone();
        invalid_sample[key] = invalid;
        samples.push(invalid_sample);
    }
    value["calendar_sentiment"] = json!({"production_weight":0.,"frozen_days":3,"frozen_trials":5,
        "scored_days":1,"scored_trials":2,"latest":samples,"trials":[{"private":"hidden"}]});
    fs::write(&path, value.to_string()).unwrap();
    let parsed = parse_report(&path).unwrap();
    let calendar = parsed.calendar_sentiment.as_ref().unwrap();
    assert_eq!(calendar.latest.len(), 1);
    assert_eq!(calendar.latest[0].recent_votes, 512);
    assert_eq!(calendar.latest[0].weighted_reset_share, Some(0.72));
    assert_eq!(calendar.production_weight, 0.);
    assert_eq!((calendar.frozen_days, calendar.frozen_trials), (3, 5));
    let generated = DateTime::parse_from_rfc3339("2026-10-17T00:00:00Z")
        .unwrap()
        .timestamp() as f64;
    let bins: Vec<_> = (0..6)
        .map(|i| {
            json!({
                "start": generated - (6-i) as f64 * 3600., "end": generated - (5-i) as f64 * 3600.,
                "new_clicks":100,"reset_clicks":70,"feature_clicks":30,"coverage_fraction":0.98,
                "reset_share":0.7,"clicks_per_hour":102.04,"state":"available"
            })
        })
        .collect();
    let mut with_trend = value.clone();
    with_trend["calendar_sentiment"]["latest"][0]["trend"] = json!({
        "window_hours":6,"bin_hours":1,"minimum_bin_votes":50,"minimum_coverage":0.9,
        "bins":bins,"change":{"state":"available","reset_share_delta":0.,"click_rate_ratio":1.}
    });
    fs::write(&path, with_trend.to_string()).unwrap();
    let trend_report = parse_report(&path).unwrap();
    assert!(trend_report.calendar_sentiment.unwrap().latest[0]
        .trend
        .is_some());
    for (field, invalid) in [
        ("new_clicks", json!(49)),
        ("coverage_fraction", json!(0.89)),
        ("end", json!(generated + 3600.)),
    ] {
        let mut bad = with_trend.clone();
        bad["calendar_sentiment"]["latest"][0]["trend"]["bins"][5][field] = invalid;
        fs::write(&path, bad.to_string()).unwrap();
        // A malformed optional curve must not discard the valid community
        // observation, existing scores, or the rest of the evaluation report.
        let calendar = parse_report(&path).unwrap().calendar_sentiment.unwrap();
        assert_eq!(calendar.latest.len(), 1);
        assert!(calendar.latest[0].trend.is_none(), "{field}");
    }
    let serialized = serde_json::to_string(&parsed).unwrap();
    for private in [
        "private_signal",
        "private_provenance",
        "hidden",
        "arbitrary",
    ] {
        assert!(!serialized.contains(private), "leaked {private}");
    }
    for (field, invalid) in [
        ("production_weight", json!(0.2)),
        ("scored_days", json!(4)),
        ("scored_trials", json!(6)),
        ("frozen_days", json!(6)),
        ("frozen_trials", json!(-1)),
    ] {
        let mut invalid_section = value.clone();
        invalid_section["calendar_sentiment"][field] = invalid;
        fs::write(&path, invalid_section.to_string()).unwrap();
        assert!(
            parse_report(&path).unwrap().calendar_sentiment.is_none(),
            "{field}"
        );
    }
}
#[test]
fn consumer_preserves_exact_evaluation_identity_and_legacy_reports() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("consumer.json");
    let mut value = fixture();
    fs::write(&path, value.to_string()).unwrap();
    assert!(parse_forecast(&path).unwrap().forecasts[0]
        .evaluation_version
        .is_none());
    let model = value["forecasts"][0]["model_version"]
        .as_str()
        .unwrap()
        .to_owned();
    value["forecasts"][0]["evaluation_version"] = json!(format!("{model}:abc123"));
    fs::write(&path, value.to_string()).unwrap();
    assert_eq!(
        parse_forecast(&path).unwrap().forecasts[0]
            .evaluation_version
            .as_deref(),
        Some(format!("{model}:abc123").as_str())
    );
    for invalid in [
        "wrong-model:abc123".to_owned(),
        format!("{model}:"),
        format!("{model}:invalid hash"),
    ] {
        value["forecasts"][0]["evaluation_version"] = json!(invalid);
        fs::write(&path, value.to_string()).unwrap();
        assert!(parse_forecast(&path).is_err());
    }
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
fn forecast_community_ablation_is_optional_and_preserves_valid_probabilities() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("consumer.json");
    let mut value = fixture();
    fs::write(&path, value.to_string()).unwrap();
    assert!(parse_forecast(&path).unwrap().forecasts[0].curve[23]
        .without_community_probability
        .is_none());
    for probability in [None, Some(0.), Some(0.24), Some(0.8), Some(1.)] {
        value["forecasts"][0]["curve"][23]["without_community_probability"] = json!(probability);
        fs::write(&path, value.to_string()).unwrap();
        let parsed = parse_forecast(&path).unwrap();
        assert_eq!(
            parsed.forecasts[0].curve[23].without_community_probability,
            probability
        );
        assert_eq!(parsed.forecasts[0].curve[23].probability, 0.24);
        let exported = serde_json::to_value(parsed).unwrap();
        assert_eq!(
            exported["forecasts"][0]["curve"][23]["without_community_probability"],
            json!(probability)
        );
    }
    for probability in [-0.1, 1.1] {
        value["forecasts"][0]["curve"][23]["without_community_probability"] = json!(probability);
        fs::write(&path, value.to_string()).unwrap();
        assert!(parse_forecast(&path).is_err());
    }
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
    let _guard = LOAD_TEST.lock().unwrap_or_else(|e| e.into_inner());
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
fn large_report_loads_cold_and_retains_curves_after_invalid_or_oversized_updates() {
    let _guard = LOAD_TEST.lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().unwrap();
    let forecast = root.path().join("consumer.json");
    let report = root.path().join("report.json");
    fs::write(&forecast, fixture().to_string()).unwrap();
    fs::write(
        root.path().join("reset-radar-connection.json"),
        json!({"forecastPath":forecast,"evaluationPath":report}).to_string(),
    )
    .unwrap();
    let value = json!({
        "schema_version":"reset-observer/1", "generated_at":"2026-10-15T00:00:00Z",
        "started_at":"2026-10-03T16:00:00Z", "state":"scoring",
        "policy":{"reporting_grace_seconds":10800,"minimum_rank_windows":30,"minimum_rank_events":5},
        "summary":{"forecast_snapshots":500,"frozen_trials":264,"scored_nonoverlapping_trials":37,
            "settlements":{"settled":264},"next_maturity":null,"settlement_revisions":300},
        "latest":[],"curve_basis":"received_publication_values",
        "curves":[{"source":"horizon","version":"test-model","track":"model",
            "target":"broad_quota_relief","hours":24,
            "points":[{"at":"2026-10-14T23:00:00Z","p":0.24},{"at":"2026-10-15T00:00:00Z","p":0.25}]}],
        "scorecards":[],"comparisons":[],"outcomes":[],"outcome_providers":[]
    });
    // Whitespace increases the valid export size without inventing model data.
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.resize(9 * 1024 * 1024, b' ');
    fs::write(&report, bytes).unwrap();
    let good = load(root.path(), &now());
    assert!(good.connected);
    assert!(good.forecast_issue.is_none());
    assert_eq!(good.evaluation_issue, None);
    let saved = good.evaluation.unwrap();
    assert_eq!(saved.curves.len(), 1);
    assert_eq!(saved.curves[0].points.len(), 2);
    assert_eq!(saved.curves[0].points[1].p, Some(0.25));
    let expected = serde_json::to_value(&saved).unwrap();
    for invalid in [json!(1.1), json!(-0.1)] {
        let mut bad = value.clone();
        bad["curves"][0]["points"][0]["p"] = invalid;
        fs::write(&report, bad.to_string()).unwrap();
        let retained = load(root.path(), &now());
        assert_eq!(retained.evaluation_issue.as_deref(), Some("invalid"));
        assert_eq!(
            serde_json::to_value(retained.evaluation.unwrap()).unwrap(),
            expected
        );
    }
    let mut bad = value.clone();
    bad["curves"][0]["points"][0]["at"] = json!("invalid-time");
    for invalid in [bad.to_string(), "{".into()] {
        fs::write(&report, invalid).unwrap();
        let retained = load(root.path(), &now());
        assert_eq!(retained.evaluation_issue.as_deref(), Some("invalid"));
        assert_eq!(
            serde_json::to_value(retained.evaluation.unwrap()).unwrap(),
            expected
        );
    }
    fs::File::create(&report)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    let retained = load(root.path(), &now());
    assert_eq!(retained.evaluation_issue.as_deref(), Some("too_large"));
    assert_eq!(
        serde_json::to_value(retained.evaluation.unwrap()).unwrap(),
        expected
    );
    let mut recovered = value;
    recovered["curves"][0]["points"][1]["p"] = json!(0.375);
    fs::write(&report, serde_json::to_vec(&recovered).unwrap()).unwrap();
    let restored = load(root.path(), &now());
    assert_eq!(restored.evaluation_issue, None);
    assert_eq!(
        restored.evaluation.unwrap().curves[0].points[1].p,
        Some(0.375)
    );
}

#[test]
fn forecast_and_configuration_keep_the_eight_mib_limit() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("consumer.json");
    let mut bytes = serde_json::to_vec(&fixture()).unwrap();
    bytes.resize(8 * 1024 * 1024, b' ');
    fs::write(&path, &bytes).unwrap();
    assert!(parse_forecast(&path).is_ok());
    bytes.push(b' ');
    fs::write(&path, bytes).unwrap();
    assert_eq!(parse_forecast(&path).unwrap_err(), "too_large");
    let config = root.path().join("reset-radar-connection.json");
    let mut bytes = serde_json::to_vec(
        &json!({"forecastPath":path,"evaluationPath":root.path().join("report.json")}),
    )
    .unwrap();
    bytes.resize(8 * 1024 * 1024, b' ');
    fs::write(&config, &bytes).unwrap();
    assert!(read_json::<ConnectionConfig>(&config).is_ok());
    bytes.push(b' ');
    fs::write(&config, bytes).unwrap();
    assert_eq!(
        read_json::<ConnectionConfig>(&config).err().as_deref(),
        Some("too_large")
    );
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
fn public_status_can_supply_commitment_timing_without_becoming_a_forecast() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("consumer.json");
    let mut value = fixture();
    let forecast = json!({
        "id":"codex_reset", "probabilities":vec![0.83;72], "source_points":[[24.,0.45],[48.,0.70]],
        "origin":1791072000., "source_coverage_hours":48.,
        "comment":{"probability":0.83,"url":"https://x.com/i/status/1234","at":1791068400.,
            "included":true,"state":"included","deadline":1791075600.,
            "timing_sources":["didcodex_status","private_signal"],"adjustment_24h":0.38,"adjustment_48h":0.13,
            "timing_evidence":[{"source":"didcodex_status","deadline":1791075600.,
                "relationship":"same_reference","estimate_url":"https://x.com/i/status/1234",
                "timing_url":"https://x.com/i/status/1234","links":[]}]}
    });
    let mut unsupported = forecast.clone();
    unsupported["id"] = json!("didcodex_status");
    value["forecasts"][0]["sources"] = json!([forecast, unsupported]);
    fs::write(&path, value.to_string()).unwrap();
    let parsed = parse_forecast(&path).unwrap();
    assert_eq!(parsed.forecasts[0].sources.len(), 1);
    let comment = parsed.forecasts[0].sources[0].comment.as_ref().unwrap();
    assert_eq!(comment.timing_sources, ["didcodex_status"]);
    assert_eq!(comment.timing_evidence.len(), 1);
    assert_eq!(comment.timing_evidence[0].source, "didcodex_status");
    assert!(!serde_json::to_string(&parsed)
        .unwrap()
        .contains("private_signal"));
}

#[test]
fn calendar_innovation_preserves_clicks_without_claiming_people() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("consumer.json");
    let mut value = fixture();
    let input = json!({"id":"didcodex_poll","weight":0.08,"votes":80,"total_votes":2000,
        "recent_votes":80,"effective_votes":null,"coverage_hours":8.,
        "probability_24h":0.65,"probability_48h":0.8,"observation_window_hours":6.,
        "mapping_kind":"sentiment_innovation","mapping_version":"calendar-innovation-1",
        "reference_share":0.6,"recent_share":0.75,"log_odds_change":0.2,
        "repeat_sensitivity_units":0.7,"window_start":1791158400.,"window_end":1791244800.,
        "private_source":"never expose"});
    value["forecasts"][0]["community_inputs"] = json!([input]);
    fs::write(&path, value.to_string()).unwrap();
    let parsed = parse_forecast(&path).unwrap();
    let c = &parsed.forecasts[0].community_inputs[0];
    assert_eq!(c.mapping_kind.as_deref(), Some("sentiment_innovation"));
    assert_eq!(c.recent_votes, Some(80));
    assert_eq!(c.effective_votes, None);
    assert_eq!(c.repeat_sensitivity_units, Some(0.7));
    assert!(!serde_json::to_string(&parsed)
        .unwrap()
        .contains("private_source"));
    for (key, invalid) in [
        ("effective_votes", json!(0.7)),
        ("recent_votes", json!(49)),
        ("mapping_kind", json!("private_mapping")),
        ("mapping_version", json!("unreviewed")),
        ("reference_share", json!(1.1)),
        ("repeat_sensitivity_units", json!(-1.)),
        ("window_end", json!(0.)),
        ("log_odds_change", Value::Null),
    ] {
        let mut bad = value.clone();
        bad["forecasts"][0]["community_inputs"][0][key] = invalid;
        fs::write(&path, bad.to_string()).unwrap();
        assert!(parse_forecast(&path).is_err(), "{key}");
    }
}

#[test]
fn calendar_consensus_preserves_the_original_window_estimate_and_validates_version() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("consumer.json");
    let mut value = fixture();
    value["forecasts"][0]["community_inputs"] = json!([{
        "id":"didcodex_poll","weight":0.78,"votes":6960,"total_votes":30447,
        "recent_votes":6960,"effective_votes":null,"coverage_hours":3.,
        "probability_24h":0.98,"probability_48h":0.99,"observation_window_hours":6.,
        "mapping_kind":"sentiment_level","mapping_version":"calendar-consensus-2",
        "reference_share":0.9944,"recent_share":0.9934,"log_odds_change":-0.046,
        "repeat_sensitivity_units":29.8,"window_start":1791158400.,"window_end":1791244800.,
        "terminal_probability":0.9773
    }]);
    fs::write(&path, value.to_string()).unwrap();
    let parsed = parse_forecast(&path).unwrap();
    let c = &parsed.forecasts[0].community_inputs[0];
    assert_eq!(c.mapping_kind.as_deref(), Some("sentiment_level"));
    assert_eq!(c.mapping_version.as_deref(), Some("calendar-consensus-2"));
    assert_eq!(c.terminal_probability, Some(0.9773));
    assert_eq!(c.log_odds_change, Some(-0.046));
    for (key, invalid) in [
        ("terminal_probability", json!(1.01)),
        ("terminal_probability", json!(-0.01)),
        ("terminal_probability", Value::Null),
        ("mapping_kind", json!("sentiment_innovation")),
        ("mapping_version", json!("calendar-innovation-1")),
    ] {
        let mut bad = value.clone();
        bad["forecasts"][0]["community_inputs"][0][key] = invalid;
        fs::write(&path, bad.to_string()).unwrap();
        assert!(parse_forecast(&path).is_err(), "{key}");
    }
}

#[test]
fn forecast_cycle_retains_completion_identity_without_turning_publication_into_occurrence() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("consumer.json");
    let mut value = fixture();
    let issued = DateTime::parse_from_rfc3339(value["forecasts"][0]["issued_at"].as_str().unwrap())
        .unwrap()
        .timestamp() as f64;
    let confirmation = json!({"time_basis":"completion_confirmation","occurred_at":null,
        "confirmation_at":issued-100.,"invalidation_at":issued-100.,"identity":"a".repeat(64),
        "references":["https://x.com/i/status/123","https://private.invalid/event"],"observations":["hidden"]});
    value["forecasts"][0]["cycle"] = confirmation;
    fs::write(&path, value.to_string()).unwrap();
    let parsed = parse_forecast(&path).unwrap();
    let cycle = parsed.forecasts[0].cycle.as_ref().unwrap();
    assert_eq!(cycle.occurred_at, None);
    assert_eq!(cycle.confirmation_at, Some(issued - 100.));
    assert_eq!(cycle.references, vec!["https://x.com/i/status/123"]);
    assert!(!serde_json::to_string(&parsed)
        .unwrap()
        .contains("private.invalid"));
    for (key, invalid) in [
        ("occurred_at", json!(issued - 100.)),
        ("time_basis", json!("announcement_proxy")),
        ("confirmation_at", json!(issued + 3600.)),
        ("invalidation_at", Value::Null),
        ("identity", json!("private-id")),
    ] {
        let mut bad = value.clone();
        bad["forecasts"][0]["cycle"][key] = invalid;
        fs::write(&path, bad.to_string()).unwrap();
        assert!(
            parse_forecast(&path).unwrap().forecasts[0].cycle.is_none(),
            "{key}"
        );
    }
}

#[test]
fn rolling_preference_qualifies_six_hours_and_keeps_hourly_volume_separate() {
    let generated = 1792195200.;
    let bins: Vec<_> = (0..6)
        .map(|i| {
            json!({
                "start":generated-(6-i) as f64*3600.,"end":generated-(5-i) as f64*3600.,
                "new_clicks":4,"reset_clicks":3,"feature_clicks":1,"coverage_fraction":0.5,
                "sample_votes":80,"sample_coverage_fraction":0.6,"reset_share":0.7,
                "clicks_per_hour":null,"state":"available"
            })
        })
        .collect();
    let value = json!({"window_hours":6,"bin_hours":1,"minimum_bin_votes":50,"minimum_coverage":0.9,
        "estimate_window_hours":6,"estimate_kind":"rolling_weighted_preference","bins":bins,
        "change":{"state":"available","reset_share_delta":0.,"click_rate_ratio":null}});
    let trend: CalendarTrend = serde_json::from_value(value.clone()).unwrap();
    assert!(trend.valid(generated));
    let mut sparse = value.clone();
    for bin in sparse["bins"].as_array_mut().unwrap() {
        bin["sample_votes"] = json!(49);
        bin["reset_share"] = Value::Null;
        bin["state"] = json!("insufficient_recent_votes");
        bin["coverage_fraction"] = json!(1.);
        bin["clicks_per_hour"] = json!(4.);
    }
    sparse["change"] =
        json!({"state":"awaiting_comparable_hours","reset_share_delta":null,"click_rate_ratio":1.});
    assert!(serde_json::from_value::<CalendarTrend>(sparse)
        .unwrap()
        .valid(generated));
    for (key, invalid) in [
        ("sample_votes", json!(49)),
        ("sample_coverage_fraction", json!(1.1)),
        ("sample_votes", Value::Null),
        ("end", json!(generated + 3600.)),
        ("reset_share", Value::Null),
    ] {
        let mut bad = value.clone();
        bad["bins"][5][key] = invalid;
        assert!(
            !serde_json::from_value::<CalendarTrend>(bad)
                .unwrap()
                .valid(generated),
            "{key}"
        );
    }
    let mut unknown = value;
    unknown["estimate_kind"] = json!("unknown_model");
    assert!(!serde_json::from_value::<CalendarTrend>(unknown)
        .unwrap()
        .valid(generated));
}

#[test]
fn reaction_export_is_optional_bounded_and_excludes_private_or_unmeasured_clocks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("report.json");
    let mut value = json!({"schema_version":"reset-observer/1","generated_at":"2026-10-17T00:00:00Z",
        "started_at":"2026-10-03T00:00:00Z","state":"scoring",
        "policy":{"reporting_grace_seconds":10800,"minimum_rank_windows":30,"minimum_rank_events":5},
        "summary":{"forecast_snapshots":0,"frozen_trials":0,"scored_nonoverlapping_trials":0,
            "settlements":{},"next_maturity":null,"settlement_revisions":0},
        "latest":[],"curves":[],"scorecards":[],"comparisons":[],"outcomes":[],
        "outcome_providers":[{"provider":"completion_page","last_attempt":"2026-10-17T00:00:00Z",
            "generated_at":null,"healthy":false,"state":"positive_evidence","source_family":"codex_reset_fm","coverage_role":"positive_only"}]});
    fs::write(&path, value.to_string()).unwrap();
    assert!(parse_report(&path).unwrap().reaction.is_none());
    let series = json!({"source":"horizon","version":"model-1","target":"broad_quota_relief","track":"model",
        "state":"updated","published_at":1792195100.,"source_received_at":1792195105.,"observer_received_at":1792195110.,
        "publication_to_capture_seconds":5.,"completion_to_update_seconds":100.,"known_to_update_seconds":90.,
        "old_cycle_residual_seconds":100.,"residual_censored":false,"before_probability_24h":0.8,"after_probability_24h":0.2,
        "ui_received_at":1792195115.,"ui_delay_seconds":5.,"private_provenance":"hidden"});
    let mut private = series.clone();
    private["source"] = json!("private_market");
    let mut invalid = series.clone();
    invalid["completion_to_update_seconds"] = json!(-1.);
    let evidence = json!({"provider":"completion_page","source_family":"codex_reset_fm","published_at":1792195000.,
        "first_received_at":1792195010.,"published_to_capture_seconds":10.,"raw_url":"hidden"});
    value["reaction"] = json!({"version":"reset-reaction-1","initialized_at":1792195190.,"history_since":1791590400.,
        "generated_at":1792195200.,"events":[{"id":"a".repeat(64),"kind":"automatic","status":"confirmed",
        "time_basis":"public_completion_notice","completed_from":1792195000.,"completed_to":1792195000.,
        "first_received_at":1792195010.,"evidence":[evidence],"series":[series,private,invalid]}]});
    fs::write(&path, value.to_string()).unwrap();
    let parsed = parse_report(&path).unwrap();
    let reaction = parsed.reaction.as_ref().unwrap();
    assert_eq!(reaction.events.len(), 1);
    assert_eq!(reaction.events[0].series.len(), 1);
    assert_eq!(
        reaction.events[0].series[0].completion_to_update_seconds,
        Some(100.)
    );
    assert_eq!(reaction.events[0].series[0].ui_received_at, None);
    assert_eq!(reaction.events[0].series[0].ui_delay_seconds, None);
    assert_eq!(
        parsed.outcome_providers[0].coverage_role.as_deref(),
        Some("positive_only")
    );
    assert_eq!(
        parsed.outcome_providers[0].state.as_deref(),
        Some("positive_evidence")
    );
    assert!(!parsed.outcome_providers[0].healthy);
    let serialized = serde_json::to_string(&parsed).unwrap();
    for hidden in ["private_market", "private_provenance", "raw_url", "hidden"] {
        assert!(!serialized.contains(hidden));
    }
    for (key, invalid) in [
        ("version", json!("future-format")),
        ("generated_at", json!(1992195200.)),
        ("history_since", json!(-1.)),
        ("events", json!({"private":"shape"})),
    ] {
        let mut bad = value.clone();
        bad["reaction"][key] = invalid;
        fs::write(&path, bad.to_string()).unwrap();
        assert!(parse_report(&path).unwrap().reaction.is_none(), "{key}");
    }
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
    let started = std::time::Instant::now();
    let timeline = load_timeline(&path, &now()).unwrap();
    let cold_ms = started.elapsed().as_secs_f64() * 1000.;
    let q = &timeline.quantitative;
    assert!(
        q.connected && q.forecast_issue.is_none() && q.evaluation_issue.is_none(),
        "connected={}, forecast_issue={:?}, evaluation_issue={:?}",
        q.connected,
        q.forecast_issue,
        q.evaluation_issue
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
    let started = std::time::Instant::now();
    let repeated = load(path.parent().unwrap(), &now());
    let warm_ms = started.elapsed().as_secs_f64() * 1000.;
    assert!(repeated.forecast_issue.is_none() && repeated.evaluation_issue.is_none());
    println!(
        "installed-read cold_timeline_ms={cold_ms:.3} warm_exports_ms={warm_ms:.3} same_publication={}",
        repeated.evaluation.as_ref().unwrap().generated_at == r.generated_at
    );
    if let Ok(output) = std::env::var("HORIZON_PUBLIC_CAPTURE_PATH") {
        fs::write(output, serde_json::to_vec(&timeline).unwrap()).unwrap();
    }
}

#[test]
fn candle_projection_contract_survives_filtering_and_accepts_legacy_lines() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("report.json");
    let mut report = content_report(0);
    report["curves"][0]["candle_interval_minutes"] = json!(30);
    fs::write(&path, report.to_string()).unwrap();
    let parsed = parse_report(&path).unwrap();
    assert_eq!(parsed.curves[0].candle_interval_minutes, Some(30));
    assert_eq!(parsed.curves[1].candle_interval_minutes, None);
    let exported = serde_json::to_value(&parsed).unwrap();
    assert_eq!(exported["curves"][0]["candle_interval_minutes"], 30);
    assert!(exported["curves"][1]
        .get("candle_interval_minutes")
        .is_none());
    for (field, value) in [
        ("candle_interval_minutes", json!(15)),
        ("source", json!("monitor")),
        ("track", json!("native")),
        ("hours", json!(48)),
    ] {
        let mut invalid = report.clone();
        invalid["curves"][0][field] = value;
        fs::write(&path, invalid.to_string()).unwrap();
        // Disallowed identities are removed before value validation. A report
        // may survive that filter, but no invalid candle contract may escape.
        if let Ok(parsed) = parse_report(&path) {
            assert!(
                parsed.curves.iter().all(|curve| {
                    curve.candle_interval_minutes.is_none_or(|minutes| {
                        minutes == 30
                            && curve.identity.source == "horizon"
                            && curve.identity.track == "model"
                            && curve.hours == 24
                    })
                }),
                "invalid candle {field}"
            );
        }
    }
}
