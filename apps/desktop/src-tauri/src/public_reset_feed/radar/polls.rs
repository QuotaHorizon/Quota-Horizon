use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Poll {
    pub source_id: String,
    pub name: String,
    pub url: String,
    pub updated_at: Option<String>,
    pub collected_at: String,
    pub issue: Option<SourceIssue>,
    pub round: Round,
    pub recent: Vec<Round>,
    #[serde(default)]
    pub history: Vec<PollPoint>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Round {
    pub kind: String,
    pub id: String,
    pub starts_at: String,
    pub ends_at: String,
    pub samples: usize,
    pub mean_probability: Option<f64>,
    pub distribution: Vec<Bin>,
    pub accepting: bool,
    pub coverage_gap: bool,
    pub outcome: Option<u8>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Bin {
    pub probability: Option<f64>,
    pub count: usize,
    #[serde(default, rename = "deadlineAt")]
    pub deadline_at: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PollPoint {
    pub at: String,
    pub round_id: String,
    pub mean_probability: Option<f64>,
    pub samples: usize,
}

fn millis(v: &Value) -> Result<String, SourceIssue> {
    v.as_i64()
        .and_then(DateTime::from_timestamp_millis)
        .map(|t| t.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or(SourceIssue::SchemaChanged)
}
fn round(v: &Value) -> Result<Round, SourceIssue> {
    if v["kind"] != "pacific-day" {
        return Err(SourceIssue::SchemaChanged);
    }
    let starts_at = millis(&v["startsAt"])?;
    let ends_at = millis(&v["endsAt"])?;
    // Pacific calendar days can be 23 or 25 hours at a DST transition.
    if !(23 * 3600..=25 * 3600)
        .contains(&(seconds(&ends_at).unwrap() - seconds(&starts_at).unwrap()))
    {
        return Err(SourceIssue::SchemaChanged);
    }
    let samples = v["total"]
        .as_u64()
        .filter(|n| *n <= 100_000)
        .ok_or(SourceIssue::SchemaChanged)? as usize;
    let mut distribution = vec![];
    let mut seen = HashSet::new();
    for b in v["distribution"]
        .as_array()
        .ok_or(SourceIssue::SchemaChanged)?
    {
        let p = b["probability"]
            .as_u64()
            .filter(|p| [10, 30, 50, 70, 90].contains(p))
            .ok_or(SourceIssue::SchemaChanged)?;
        let count = b["count"]
            .as_u64()
            .filter(|n| *n <= 100_000)
            .ok_or(SourceIssue::SchemaChanged)? as usize;
        if !seen.insert(p) {
            return Err(SourceIssue::SchemaChanged);
        }
        distribution.push(Bin {
            probability: Some(p as f64),
            count,
            deadline_at: None,
        });
    }
    if distribution.len() != 5 || distribution.iter().map(|b| b.count).sum::<usize>() != samples {
        return Err(SourceIssue::SchemaChanged);
    }
    distribution.sort_by(|a, b| a.probability.unwrap().total_cmp(&b.probability.unwrap()));
    let mean_probability = (samples > 0).then(|| {
        distribution
            .iter()
            .map(|b| b.probability.unwrap() * b.count as f64)
            .sum::<f64>()
            / samples as f64
    });
    if mean_probability
        .is_some_and(|mean| number(&v["average"]).map_or(true, |p| (p - mean).abs() > 0.11))
    {
        return Err(SourceIssue::SchemaChanged);
    }
    let outcome = match v["outcome"].as_u64() {
        Some(n @ 0..=1) => Some(n as u8),
        _ if v["outcome"].is_null() => None,
        _ => return Err(SourceIssue::SchemaChanged),
    };
    Ok(Round {
        kind: "probability".into(),
        id: short(&v["id"], 64).ok_or(SourceIssue::SchemaChanged)?,
        starts_at,
        ends_at,
        samples,
        mean_probability,
        distribution,
        outcome,
        accepting: v["accepting"] == true && v["lockAt"].is_null() && outcome.is_none(),
        coverage_gap: v["coverageGap"]
            .as_bool()
            .ok_or(SourceIssue::SchemaChanged)?,
    })
}

pub(super) fn parse(bytes: &[u8], at: &UtcTimestamp) -> Result<Poll, SourceIssue> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| SourceIssue::InvalidResponse)?;
    if v["available"] != true {
        return Err(SourceIssue::InvalidResponse);
    }
    let updated_at = stamp(&v["asOf"]).ok_or(SourceIssue::SchemaChanged)?;
    if !(-60..1800).contains(&(seconds(at.as_str()).unwrap() - seconds(&updated_at).unwrap())) {
        return Err(SourceIssue::InvalidResponse);
    }
    let current = round(&v["round"])?;
    if seconds(&current.starts_at).unwrap() > seconds(at.as_str()).unwrap() + 60 {
        return Err(SourceIssue::InvalidResponse);
    }
    let mut recent = vec![];
    for r in v["recent"].as_array().into_iter().flatten().take(7) {
        let r = round(r)?;
        if seconds(&r.ends_at).unwrap() <= seconds(&current.starts_at).unwrap() {
            recent.push(r);
        }
    }
    Ok(Poll {
        source_id: "nextreset_poll".into(),
        name: "NextReset".into(),
        url: "https://nextreset.ai/#community".into(),
        updated_at: Some(updated_at),
        collected_at: at.as_str().into(),
        issue: None,
        round: current,
        recent,
        history: vec![],
    })
}

pub(super) fn parse_timing(bytes: &[u8], at: &UtcTimestamp) -> Result<Poll, SourceIssue> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| SourceIssue::InvalidResponse)?;
    let starts_at = stamp(&v["opened_at"]).ok_or(SourceIssue::SchemaChanged)?;
    let ends_at = stamp(&v["expires_at"]).ok_or(SourceIssue::SchemaChanged)?;
    if seconds(&starts_at).unwrap() > seconds(at.as_str()).unwrap() + 60
        || seconds(&ends_at).unwrap() - seconds(&starts_at).unwrap() != 7 * 86400
    {
        return Err(SourceIssue::SchemaChanged);
    }
    let samples = v["tally"]["total_votes"]
        .as_u64()
        .filter(|n| *n <= 100_000)
        .ok_or(SourceIssue::SchemaChanged)? as usize;
    // The provider intentionally hides small tallies. Do not infer hidden votes.
    let unlocked = v["tally"]["unlocked"] == true;
    let mut distribution = vec![];
    if unlocked {
        let buckets = v["tally"]["buckets"]
            .as_array()
            .ok_or(SourceIssue::SchemaChanged)?;
        let windows = v["windows"].as_array().ok_or(SourceIssue::SchemaChanged)?;
        for (id, hours) in [
            ("within_24h", Some(24)),
            ("in_1_3_days", Some(72)),
            ("in_4_7_days", Some(168)),
            ("not_soon", None),
        ] {
            let b = buckets
                .iter()
                .find(|b| b["id"] == id)
                .ok_or(SourceIssue::SchemaChanged)?;
            let w = windows
                .iter()
                .find(|b| b["id"] == id)
                .ok_or(SourceIssue::SchemaChanged)?;
            let deadline_at = stamp(&w["deadline_at"]);
            if deadline_at
                .as_ref()
                .and_then(|d| seconds(d))
                .map(|t| t - seconds(&starts_at).unwrap())
                != hours.map(|h| h * 3600)
            {
                return Err(SourceIssue::SchemaChanged);
            }
            let count = b["votes"]
                .as_u64()
                .filter(|n| *n <= 100_000)
                .ok_or(SourceIssue::SchemaChanged)? as usize;
            distribution.push(Bin {
                probability: None,
                count,
                deadline_at,
            });
        }
        if distribution.iter().map(|b| b.count).sum::<usize>() != samples {
            return Err(SourceIssue::SchemaChanged);
        }
    }
    Ok(Poll {
        source_id: "codex_reset_poll".into(),
        name: "Codex Reset".into(),
        url: "https://codex-reset.com/codex-usage#reset-poll".into(),
        updated_at: None,
        collected_at: at.as_str().into(),
        issue: None,
        round: Round {
            kind: "timing".into(),
            id: short(&v["poll_id"], 64).ok_or(SourceIssue::SchemaChanged)?,
            starts_at,
            ends_at,
            samples,
            mean_probability: None,
            distribution,
            accepting: v["open"] == true,
            coverage_gap: false,
            outcome: None,
        },
        recent: vec![],
        history: vec![],
    })
}

pub(super) fn collect(at: &UtcTimestamp) -> Vec<(&'static str, Result<Poll, SourceIssue>)> {
    std::thread::scope(|scope| {
        let jobs: Vec<_> = [
            ("nextreset_poll", "https://nextreset.ai/api/community"),
            ("codex_reset_poll", "https://codex-reset.com/api/reset-poll"),
        ]
        .into_iter()
        .map(|(id, url)| {
            (
                id,
                scope.spawn(move || {
                    let client = public_client()?;
                    // GET only. Never vote, send a browser identity or create an account.
                    let response = client
                        .get(url)
                        .send()
                        .map_err(|_| SourceIssue::RequestFailed)?;
                    let bytes = read_response(response)?;
                    if id == "nextreset_poll" {
                        parse(&bytes, at)
                    } else {
                        parse_timing(&bytes, at)
                    }
                }),
            )
        })
        .collect();
        jobs.into_iter()
            .map(|(id, job)| (id, job.join().unwrap_or(Err(SourceIssue::RequestFailed))))
            .collect()
    })
}

// A bounded outlook feature, not a conversion into a rolling event probability.
// A near-term positive can inform a wider window; a near-term negative cannot.
pub(super) fn signal(
    polls: &[Poll],
    hours: u32,
    now: i64,
    last_reset: Option<i64>,
) -> (f64, f64, usize) {
    let mut direction = 0.0;
    let mut effective = 0.0;
    let mut samples = 0;
    let target = now + i64::from(hours) * 3600;
    for poll in polls {
        let r = &poll.round;
        let start = seconds(&r.starts_at).unwrap_or(0);
        let end = seconds(&r.ends_at).unwrap_or(0);
        if poll.issue.is_some()
            || !r.accepting
            || r.coverage_gap
            || r.samples == 0
            || end <= now
            || last_reset.is_some_and(|reset| reset > start)
            || seconds(&poll.collected_at).is_none_or(|t| !(-60..1800).contains(&(now - t)))
            || poll
                .updated_at
                .as_ref()
                .and_then(|t| seconds(t))
                .is_some_and(|t| !(-60..1800).contains(&(now - t)))
        {
            continue;
        }
        let (score, used) = if r.kind == "probability" {
            let Some(p) = r.mean_probability else {
                continue;
            };
            if (p > 50.0 && end <= target) || (p < 50.0 && end >= target) {
                (2.0 * p / 100.0 - 1.0, r.samples)
            } else {
                (0.0, 0)
            }
        } else {
            let mut previous = start;
            let mut net = 0.0;
            let mut used = 0;
            for b in &r.distribution {
                let deadline = b.deadline_at.as_ref().and_then(|t| seconds(t));
                let sign = if deadline.is_some_and(|t| t > now && t <= target) {
                    1.0
                } else if previous >= target {
                    -1.0
                } else {
                    0.0
                };
                if sign != 0.0 {
                    used += b.count;
                    net += sign * b.count as f64;
                }
                previous = deadline.unwrap_or(i64::MAX);
            }
            (net / r.samples as f64, used)
        };
        if used == 0 {
            continue;
        }
        // No per-vote timestamps are supplied, so decay from the opening time.
        let weight = r.samples as f64 / (r.samples as f64 + 12.0)
            * 2.0_f64.powf(-((now - start).max(0) as f64) / (12.0 * 3600.0));
        direction += weight * score;
        effective += weight;
        samples += used;
    }
    let cap = effective.max(1.0);
    (direction / cap, effective / cap, samples)
}
