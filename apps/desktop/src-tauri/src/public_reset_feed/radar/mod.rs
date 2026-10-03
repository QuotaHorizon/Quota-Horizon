//! Public forecast inputs, bounded community sampling and replayable estimates.
//! No credentials, account observations or private conversations enter this model.
use super::*;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

mod community;
mod model;
mod sources;
#[cfg(test)]
mod tests;

const MODEL_VERSION: &str = "horizon-pool-v1";
const MAX_STATE: usize = 256 * 1024;
const MAX_HISTORY_BYTES: i64 = 24 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Forecast {
    id: String,
    name: String,
    url: String,
    method: String,
    probability_24h: Option<f64>,
    probability_48h: Option<f64>,
    baseline_24h: Option<f64>,
    baseline_48h: Option<f64>,
    updated_at: Option<String>,
    collected_at: String,
    last_reset_at: Option<String>,
    evidence_urls: Vec<String>,
    uses_community: bool,
    issue: Option<SourceIssue>,
    exclusion: Option<String>,
    weight: f64,
    history: Vec<Point>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Point {
    at: String,
    probability_24h: f64,
    probability_48h: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Channel {
    id: String,
    name: String,
    url: String,
    query: String,
    attempted_at: String,
    success_at: Option<String>,
    issue: Option<SourceIssue>,
    scanned: usize,
    truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Opinion {
    id: String,
    channel: String,
    author: String,
    url: String,
    text: String,
    published_at: String,
    stance: String,
    reason: String,
    evidence_urls: Vec<String>,
    horizon_hours: Option<u32>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Community {
    channels: Vec<Channel>,
    opinions: Vec<Opinion>,
    optimistic: usize,
    uncertain: usize,
    pessimistic: usize,
    wishes: usize,
    observations: usize,
    duplicates: usize,
    authors: usize,
    communities: usize,
    independent_authors: usize,
    optimistic_share: Option<f64>,
    previous_share: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Estimate {
    probability_24h: f64,
    probability_48h: f64,
    pooled_24h: f64,
    pooled_48h: f64,
    community_adjustment_24h: f64,
    community_adjustment_48h: f64,
    source_count: usize,
    spread_24h: f64,
    spread_48h: f64,
    evidence_groups: usize,
    model_version: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RadarView {
    pub updated_at: Option<String>,
    forecasts: Vec<Forecast>,
    community: Community,
    estimate: Option<Estimate>,
    #[serde(default)]
    history: Vec<Point>,
}

pub(super) struct Collected {
    forecasts: Vec<(&'static str, Result<Forecast, SourceIssue>)>,
    community: Vec<(Channel, Vec<Opinion>)>,
}

fn seconds(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|t| t.timestamp())
}
fn number(value: &Value) -> Result<f64, SourceIssue> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
        .ok_or(SourceIssue::SchemaChanged)
}
fn stamp(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|v| v.len() <= 64 && seconds(v).is_some())
        .map(str::to_owned)
}
fn short(value: &Value, limit: usize) -> Option<String> {
    value
        .as_str()
        .filter(|v| !v.trim().is_empty() && v.len() <= limit)
        .map(|v| {
            v.chars()
                .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
                .collect()
        })
}
fn public_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|u| {
        u.scheme() == "https"
            && u.username().is_empty()
            && u.password().is_none()
            && u.port().is_none()
            && matches!(
                u.host_str(),
                Some(
                    "x.com"
                        | "github.com"
                        | "news.ycombinator.com"
                        | "codex-reset.com"
                        | "codexreset.org"
                        | "codexreset.app"
                        | "quotacue.com"
                )
            )
    })
}
fn urls(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) if public_url(s) => vec![s.clone()],
        Value::Array(a) => a.iter().flat_map(urls).take(40).collect(),
        Value::Object(o) => o.values().flat_map(urls).take(40).collect(),
        _ => vec![],
    }
}

pub(super) fn collect(at: &UtcTimestamp) -> Collected {
    std::thread::scope(|scope| {
        let forecasts = scope.spawn(|| sources::collect(at));
        let community = community::collect(at);
        Collected {
            forecasts: forecasts.join().unwrap_or_default(),
            community,
        }
    })
}

pub(super) fn migrate(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS radar_snapshots (at TEXT PRIMARY KEY, payload TEXT NOT NULL);
        PRAGMA user_version=4; COMMIT;",
    )
    .map_err(db_error)
}

fn table_exists(conn: &Connection) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='radar_snapshots')",
        [],
        |r| r.get(0),
    )
    .map_err(db_error)
}

pub(super) fn load(conn: &Connection, at: &UtcTimestamp) -> Result<RadarView, String> {
    if !table_exists(conn)? {
        return Ok(RadarView::default());
    }
    let mut stmt = conn
        .prepare("SELECT payload FROM radar_snapshots WHERE at <= ?1 ORDER BY at DESC LIMIT 1")
        .map_err(db_error)?;
    let mut rows = stmt.query([at.as_str()]).map_err(db_error)?;
    let Some(row) = rows.next().map_err(db_error)? else {
        return Ok(RadarView::default());
    };
    let json: String = row.get(0).map_err(db_error)?;
    if json.len() > MAX_STATE {
        return Err(db_error("radar cache limit"));
    }
    let mut view: RadarView = serde_json::from_str(&json).map_err(db_error)?;
    let start = DateTime::parse_from_rfc3339(at.as_str()).unwrap() - chrono::Duration::hours(24);
    let mut stmt = conn.prepare("SELECT at, json_extract(payload, '$.estimate.probability24h'), json_extract(payload, '$.estimate.probability48h') FROM radar_snapshots WHERE at >= ?1 AND at <= ?2 AND json_extract(payload, '$.estimate') IS NOT NULL ORDER BY at").map_err(db_error)?;
    let rows = stmt
        .query_map(
            params![
                start.to_rfc3339_opts(SecondsFormat::Millis, true),
                at.as_str()
            ],
            |r| {
                Ok(Point {
                    at: r.get(0)?,
                    probability_24h: r.get(1)?,
                    probability_48h: r.get(2)?,
                })
            },
        )
        .map_err(db_error)?;
    view.history = rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?;
    Ok(view)
}

pub(super) fn save(
    conn: &Connection,
    collected: Collected,
    insights: &insights::PublicInsights,
    at: &UtcTimestamp,
) -> Result<(), String> {
    let mut view = load(conn, at)?;
    view.history.clear();
    view.updated_at = Some(at.as_str().into());
    for (id, outcome) in collected.forecasts {
        match outcome {
            Ok(f) => {
                view.forecasts.retain(|v| v.id != id);
                view.forecasts.push(f);
            }
            Err(issue) => {
                if let Some(v) = view.forecasts.iter_mut().find(|v| v.id == id) {
                    v.issue = Some(issue);
                } else {
                    view.forecasts.push(sources::unavailable(id, issue, at));
                }
            }
        }
    }
    if let Some(f) = insights.forecast.value.as_ref() {
        view.forecasts.retain(|v| v.id != "codex_reset");
        view.forecasts.insert(
            0,
            Forecast {
                id: "codex_reset".into(),
                name: "Codex Reset".into(),
                url: "https://codex-reset.com".into(),
                method: if f.mode == "model" {
                    "cadence"
                } else {
                    "statements"
                }
                .into(),
                probability_24h: Some(f.probability_24h),
                probability_48h: Some(f.probability_48h),
                baseline_24h: f.model_probability_24h,
                baseline_48h: f.model_probability_48h,
                updated_at: Some(f.updated_at.clone()),
                collected_at: f.checked_at.clone(),
                last_reset_at: f.last_reset_at.clone(),
                evidence_urls: f.evidence_urls.clone(),
                uses_community: false,
                issue: insights.forecast.issue,
                exclusion: None,
                weight: 0.0,
                history: vec![],
            },
        );
    }
    for (channel, opinions) in collected.community {
        if channel.issue.is_none() {
            view.community.opinions.retain(|v| v.channel != channel.id);
            view.community.opinions.extend(opinions);
        }
        let prior_success = view
            .community
            .channels
            .iter()
            .find(|v| v.id == channel.id)
            .and_then(|v| v.success_at.clone());
        view.community.channels.retain(|v| v.id != channel.id);
        view.community.channels.push(Channel {
            success_at: channel.success_at.clone().or(prior_success),
            ..channel
        });
    }
    model::evaluate(&mut view, at);
    let json = serde_json::to_string(&view).map_err(db_error)?;
    if json.len() > MAX_STATE {
        return Err(db_error("radar snapshot too large"));
    }
    // Keep replay inputs and outputs together. Only this new forecast series is
    // retained for 30 days / 24 MiB; the existing event journal is untouched.
    conn.execute(
        "INSERT OR REPLACE INTO radar_snapshots(at, payload) VALUES (?1, ?2)",
        params![at.as_str(), json],
    )
    .map_err(db_error)?;
    let cutoff = DateTime::parse_from_rfc3339(at.as_str()).unwrap() - chrono::Duration::days(30);
    conn.execute(
        "DELETE FROM radar_snapshots WHERE at < ?1",
        [cutoff.to_rfc3339_opts(SecondsFormat::Millis, true)],
    )
    .map_err(db_error)?;
    conn.execute("DELETE FROM radar_snapshots WHERE at IN (SELECT at FROM (SELECT at, SUM(length(CAST(payload AS BLOB))) OVER (ORDER BY at DESC) AS bytes FROM radar_snapshots) WHERE bytes > ?1)", [MAX_HISTORY_BYTES]).map_err(db_error)?;
    Ok(())
}
