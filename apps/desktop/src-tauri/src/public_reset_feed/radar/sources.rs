use super::*;

const ORG: &str = "https://codexreset.org/_serverFn/265792b9fbf2f0d07fea84fe2c15432450c2afaf3552f5e75c04dc9540f99dbf";
const CUE: &str = "https://quotacue.com/api/dashboard";
const APP: &str = "https://codexreset.app/api/signal";

pub(super) fn unavailable(id: &str, issue: SourceIssue, at: &UtcTimestamp) -> Forecast {
    let (name, url, method) = match id {
        "reset_monitor" => (
            "Codex Reset Monitor",
            "https://codexreset.org",
            "statements",
        ),
        "quota_cue" => ("QuotaCue", "https://quotacue.com", "mixed"),
        _ => ("CodexReset.app", "https://codexreset.app", "mixed"),
    };
    Forecast {
        id: id.into(),
        name: name.into(),
        url: url.into(),
        method: method.into(),
        probability_24h: None,
        probability_48h: None,
        baseline_24h: None,
        baseline_48h: None,
        updated_at: None,
        collected_at: at.as_str().into(),
        last_reset_at: None,
        evidence_urls: vec![],
        uses_community: id == "reset_app",
        issue: Some(issue),
        exclusion: Some("unavailable".into()),
        weight: 0.0,
        history: vec![],
    }
}

// The monitor exposes its public page loader as Seroval JSON. Decode only inert
// numbers, strings, constants, arrays and objects; never evaluate page scripts.
// Unknown protocol types fail the adapter instead of executing source content.
fn inert_json(v: &Value, depth: usize) -> Result<Value, SourceIssue> {
    if depth > 32 {
        return Err(SourceIssue::InvalidResponse);
    }
    match v["t"].as_u64() {
        Some(0 | 1) => Ok(v["s"].clone()),
        Some(2) => Ok(Value::Null),
        Some(9) => v["a"]
            .as_array()
            .ok_or(SourceIssue::SchemaChanged)?
            .iter()
            .map(|v| inert_json(v, depth + 1))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Some(10 | 11) => {
            let k = v["p"]["k"].as_array().ok_or(SourceIssue::SchemaChanged)?;
            let vals = v["p"]["v"].as_array().ok_or(SourceIssue::SchemaChanged)?;
            if k.len() != vals.len() {
                return Err(SourceIssue::SchemaChanged);
            }
            let mut result = serde_json::Map::new();
            for (k, v) in k.iter().zip(vals) {
                result.insert(
                    k.as_str().ok_or(SourceIssue::SchemaChanged)?.into(),
                    inert_json(v, depth + 1)?,
                );
            }
            Ok(Value::Object(result))
        }
        // Repeated references are not required by the forecast contract. Leave
        // optional telemetry references absent rather than copying private data.
        Some(4) => Ok(Value::Null),
        _ => Err(SourceIssue::SchemaChanged),
    }
}

pub(super) fn parse(id: &str, bytes: &[u8], at: &UtcTimestamp) -> Result<Forecast, SourceIssue> {
    let mut v: Value = serde_json::from_slice(bytes).map_err(|_| SourceIssue::InvalidResponse)?;
    let mut f = unavailable(id, SourceIssue::InvalidResponse, at);
    f.issue = None;
    f.exclusion = None;
    match id {
        "reset_monitor" => {
            if v.get("t").is_some() {
                v = inert_json(&v, 0)?["result"].take();
            }
            let p = &v["forecast"];
            f.probability_24h = Some(number(&p["score24h"])?);
            f.probability_48h = Some(number(&p["score48h"])?);
            f.baseline_24h = number(&p["baseScore24h"]).ok();
            f.baseline_48h = number(&p["baseScore48h"]).ok();
            f.updated_at = Some(stamp(&v["updatedAt"]).ok_or(SourceIssue::SchemaChanged)?);
            f.last_reset_at = v["history"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| r["type"] == "forced-reset")
                .filter_map(|r| stamp(&r["dateTime"]))
                .max_by_key(|v| seconds(v));
            f.evidence_urls = urls(&p["semanticSignals"]);
            f.history = v["forecastHistory"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| {
                    let time = stamp(&row["calculatedAt"])?;
                    let age = seconds(at.as_str())? - seconds(&time)?;
                    if !(0..=86400).contains(&age) {
                        return None;
                    }
                    Some(Point {
                        at: time,
                        probability_24h: number(&row["score24h"]).ok()?,
                        probability_48h: number(&row["score48h"]).ok()?,
                    })
                })
                .take(192)
                .collect();
        }
        "quota_cue" => {
            f.probability_24h = Some(number(&v["pred_24h"]["probability"])?);
            f.probability_48h = Some(number(&v["pred_48h"]["probability"])?);
            f.last_reset_at = stamp(&v["last_reset"]);
            // This endpoint publishes no generation timestamp. Preserve that
            // absence; a successful HTTP read is only collectedAt.
            f.updated_at = stamp(&v["updated_at"]);
        }
        "reset_app" => {
            f.probability_24h = Some(number(&v["forecast"]["probability24h"])?);
            f.probability_48h = Some(number(&v["forecast"]["probability48h"])?);
            f.updated_at = Some(stamp(&v["dataAsOf"]).ok_or(SourceIssue::SchemaChanged)?);
            f.last_reset_at = stamp(&v["lastConfirmedReset"]["timestamp"])
                .or_else(|| stamp(&v["lastConfirmedReset"]["date"]));
            f.evidence_urls = urls(&v["recentEvents"]);
        }
        _ => return Err(SourceIssue::SchemaChanged),
    }
    if f.probability_48h < f.probability_24h
        || f.updated_at
            .as_ref()
            .is_some_and(|v| seconds(v).unwrap() > seconds(at.as_str()).unwrap() + 60)
    {
        return Err(SourceIssue::InvalidResponse);
    }
    f.evidence_urls.sort();
    f.evidence_urls.dedup();
    Ok(f)
}

pub(super) fn collect(at: &UtcTimestamp) -> Vec<(&'static str, Result<Forecast, SourceIssue>)> {
    std::thread::scope(|scope| {
        let jobs: Vec<_> = [
            ("reset_monitor", ORG),
            ("quota_cue", CUE),
            ("reset_app", APP),
        ]
        .into_iter()
        .map(|(id, url)| {
            (
                id,
                scope.spawn(move || {
                    let client = public_client()?;
                    let mut req = client
                        .get(url)
                        .header(reqwest::header::ACCEPT, "application/json");
                    if id == "reset_monitor" {
                        req = req.header("x-tsr-serverFn", "true");
                    }
                    let bytes = read_response(req.send().map_err(|_| SourceIssue::RequestFailed)?)?;
                    parse(id, &bytes, at)
                }),
            )
        })
        .collect();
        jobs.into_iter()
            .map(|(id, job)| (id, job.join().unwrap_or(Err(SourceIssue::RequestFailed))))
            .collect()
    })
}
