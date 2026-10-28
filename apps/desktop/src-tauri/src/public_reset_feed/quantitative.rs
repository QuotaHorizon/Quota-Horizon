//! Read-only consumer of the independently operated forecasting and evaluation
//! modules. Never opens their databases or runs their model inside the desktop.
use super::*;
use std::collections::BTreeMap;

pub(crate) mod runtime;

const LIMIT: u64 = 8 * 1024 * 1024;
// Evaluation exports include accumulated curves and scoring history, unlike
// the bounded current forecast/configuration documents.
const REPORT_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Forecast {
    target: String,
    status: String,
    issued_at: String,
    valid_until: Option<String>,
    model_version: String,
    #[serde(default)]
    evaluation_version: Option<String>,
    combination_method: Option<String>,
    training_state: Option<String>,
    evaluation_windows: usize,
    event_count: usize,
    last_event_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cycle: Option<ForecastCycle>,
    response: Option<NewsResponse>,
    statements_checked_at: Option<f64>,
    curve: Vec<Prediction>,
    sources: Vec<Input>,
    community: Option<Community>,
    #[serde(default)]
    family_weights: BTreeMap<String, f64>,
    #[serde(default)]
    community_inputs: Vec<CommunityInput>,
    #[serde(default)]
    community_coverage: Option<CommunityCoverage>,
    #[serde(default)]
    statements: Vec<Statement>,
    #[serde(default)]
    changes: Option<ForecastChanges>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ForecastCycle {
    time_basis: String,
    occurred_at: Option<f64>,
    confirmation_at: Option<f64>,
    invalidation_at: Option<f64>,
    references: Vec<String>,
    identity: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ForecastChanges {
    basis: Option<String>,
    since: String,
    until: String,
    windows: Vec<ChangedWindow>,
    new_statement_ids: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ChangedWindow {
    hours: u32,
    before: f64,
    after: f64,
    delta: f64,
    coherence_effect: f64,
    drivers: Vec<ChangeDriver>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ChangeDriver {
    id: String,
    before: Option<f64>,
    after: Option<f64>,
    weight_before: f64,
    weight_after: f64,
    raw_before: Option<f64>,
    raw_after: Option<f64>,
    input_effect: f64,
    weight_effect: f64,
    effect: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Statement {
    id: String,
    at: f64,
    url: String,
    original: String,
    translation_zh: Option<String>,
    stage: Option<String>,
    event_type: Option<String>,
    context: Option<StatementContext>,
    collector: Option<String>,
    relevance: Option<String>,
    is_reply: Option<bool>,
    parent: Option<StatementParent>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct StatementParent {
    id: String,
    author: String,
    url: String,
    text: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct NewsResponse {
    at: f64,
    url: String,
    reviewed: Vec<String>,
    changed: Vec<String>,
    kind: Option<String>,
    deadline: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct StatementContext {
    text: String,
    url: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Prediction {
    hours: u32,
    probability: f64,
    baseline: f64,
    component_range: [f64; 2],
    radar_adjustment: Option<f64>,
    comment_adjustment: Option<f64>,
    community_adjustment: Option<f64>,
    coherence_adjustment: Option<f64>,
    without_community_probability: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Input {
    id: String,
    probabilities: Vec<f64>,
    source_points: Option<Vec<[f64; 2]>>,
    origin: Option<f64>,
    expires_at: Option<f64>,
    source_coverage_hours: Option<f64>,
    weight: Option<f64>,
    baseline_points: Option<Vec<[f64; 2]>>,
    role: Option<String>,
    signal_baseline: Option<Vec<f64>>,
    baseline_weight: Option<f64>,
    response_state: Option<String>,
    comment: Option<CommentEstimate>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CommentEstimate {
    probability: f64,
    url: String,
    at: f64,
    included: bool,
    state: String,
    deadline: Option<f64>,
    timing_sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    timing_evidence: Vec<CommentTimingEvidence>,
    adjustment_24h: f64,
    adjustment_48h: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CommentTimingEvidence {
    source: String,
    deadline: f64,
    relationship: String,
    estimate_url: String,
    timing_url: String,
    links: Vec<CommentReplyLink>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CommentReplyLink {
    child_url: String,
    parent_url: String,
    at: f64,
    source: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CommunityInput {
    id: String,
    weight: f64,
    votes: Option<usize>,
    effective_votes: Option<f64>,
    coverage_hours: Option<f64>,
    probability_24h: f64,
    probability_48h: f64,
    total_votes: Option<usize>,
    recent_votes: Option<usize>,
    observation_window_hours: Option<f64>,
    latest_vote_at: Option<f64>,
    freshness_weight: Option<f64>,
    mapping_kind: Option<String>,
    mapping_version: Option<String>,
    reference_share: Option<f64>,
    recent_share: Option<f64>,
    log_odds_change: Option<f64>,
    repeat_sensitivity_units: Option<f64>,
    window_start: Option<f64>,
    window_end: Option<f64>,
    terminal_probability: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CommunityCoverage {
    included: usize,
    unavailable: usize,
    supplementary_included: usize,
    supplementary_checked: usize,
    supplementary_weight: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Community {
    probabilities: Vec<f64>,
    weight: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Consumer {
    schema_version: String,
    forecasts: Vec<Forecast>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Identity {
    source: String,
    version: String,
    track: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Latest {
    #[serde(flatten)]
    identity: Identity,
    target: String,
    origin: String,
    received_at: String,
    valid_until: String,
    state: String,
    probabilities_at_publication: BTreeMap<String, Option<f64>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Point {
    at: String,
    p: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Curve {
    #[serde(flatten)]
    identity: Identity,
    target: String,
    hours: u32,
    points: Vec<Point>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Interval {
    mean: Option<f64>,
    ci95: Option<[f64; 2]>,
    blocks: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Calibration {
    n: usize,
    mean_probability: f64,
    event_frequency: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Scores {
    n: usize,
    unique_events: usize,
    brier: Option<f64>,
    log_loss: Option<f64>,
    state: String,
    brier_interval: Interval,
    calibration: Vec<Calibration>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Alerts {
    threshold: f64,
    episodes: usize,
    false_alerts: usize,
    precision: Option<f64>,
    event_recall: Option<f64>,
    median_lead_hours: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Scorecard {
    #[serde(flatten)]
    identity: Identity,
    target: String,
    hours: u32,
    scheduled_windows: usize,
    forecast_coverage: Option<f64>,
    settlement_states: BTreeMap<String, usize>,
    scores: Scores,
    observed_event_scores: Option<Scores>,
    alerts: Alerts,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Comparison {
    left: Identity,
    right: Identity,
    target: String,
    hours: u32,
    n: usize,
    #[serde(default)]
    unique_events: usize,
    brier_skill: Option<f64>,
    loss_difference: Interval,
    error_correlation: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Outcome {
    id: String,
    status: String,
    reason: Option<String>,
    kind: String,
    from: Option<f64>,
    to: Option<f64>,
    announced_at: Option<f64>,
    known_at: f64,
    references: Vec<String>,
    primary_references: usize,
    completion_reports: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Provider {
    provider: String,
    last_attempt: String,
    generated_at: Option<String>,
    healthy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    coverage_role: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Summary {
    forecast_snapshots: usize,
    frozen_trials: usize,
    scored_nonoverlapping_trials: usize,
    settlements: BTreeMap<String, usize>,
    next_maturity: Option<String>,
    settlement_revisions: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scoring_by_horizon: Option<Vec<ScoringProgress>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ScoringProgress {
    hours: u32,
    settled_windows: usize,
    scored_windows: usize,
    scored_trials: usize,
    eligible_rolling_trials: usize,
    missing_forecast_trials: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Policy {
    reporting_grace_seconds: u32,
    minimum_rank_windows: usize,
    minimum_rank_events: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CalendarSentiment {
    production_weight: f64,
    frozen_days: usize,
    frozen_trials: usize,
    scored_days: usize,
    scored_trials: usize,
    latest: Vec<CalendarSentimentSample>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CalendarSentimentSample {
    source: String,
    date: String,
    window_start: f64,
    window_end: f64,
    observed_at: f64,
    recent_votes: usize,
    total_clicks: usize,
    weighted_reset_share: Option<f64>,
    reset_share: Option<f64>,
    latest_vote_at: Option<f64>,
    state: String,
    #[serde(default, deserialize_with = "optional_calendar_trend")]
    trend: Option<CalendarTrend>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CalendarTrend {
    window_hours: usize,
    bin_hours: usize,
    minimum_bin_votes: usize,
    minimum_coverage: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    estimate_window_hours: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    estimate_kind: Option<String>,
    bins: Vec<CalendarTrendBin>,
    change: CalendarTrendChange,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CalendarTrendBin {
    start: f64,
    end: f64,
    new_clicks: usize,
    reset_clicks: usize,
    feature_clicks: usize,
    coverage_fraction: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sample_votes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sample_coverage_fraction: Option<f64>,
    reset_share: Option<f64>,
    clicks_per_hour: Option<f64>,
    state: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CalendarTrendChange {
    state: String,
    reset_share_delta: Option<f64>,
    click_rate_ratio: Option<f64>,
}
fn optional_calendar_trend<'de, D>(deserializer: D) -> Result<Option<CalendarTrend>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}
impl CalendarTrend {
    fn valid(&self, generated: f64) -> bool {
        let rolling = self.estimate_kind.as_deref() == Some("rolling_weighted_preference")
            && self.estimate_window_hours == Some(6);
        let legacy = self.estimate_kind.is_none() && self.estimate_window_hours.is_none();
        (rolling || legacy)
            && self.window_hours == 6
            && self.bin_hours == 1
            && self.minimum_bin_votes == 50
            && self.minimum_coverage == 0.9
            && self.bins.len() == 6
            && self.bins.iter().all(|b| {
                epoch(b.start)
                    && epoch(b.end)
                    && (b.end - b.start - 3600.).abs() < 1.
                    && b.end <= generated + 60.
                    && b.start >= generated - 7. * 3600.
                    && b.reset_clicks.checked_add(b.feature_clicks) == Some(b.new_clicks)
                    && probability(b.coverage_fraction)
                    && b.reset_share.is_none_or(probability)
                    && b.clicks_per_hour.is_none_or(|v| v.is_finite() && v >= 0.)
                    && if rolling {
                        b.sample_votes.is_some()
                            && b.sample_coverage_fraction.is_some_and(probability)
                    } else {
                        b.sample_votes.is_none() && b.sample_coverage_fraction.is_none()
                    }
                    && match b.state.as_str() {
                        "available" => {
                            b.reset_share.is_some()
                                && if rolling {
                                    b.sample_votes.is_some_and(|n| n >= 50)
                                } else {
                                    b.new_clicks >= 50
                                        && b.coverage_fraction >= 0.9
                                        && b.clicks_per_hour.is_some()
                                }
                        }
                        "insufficient_recent_votes" | "incomplete_coverage" => {
                            b.reset_share.is_none()
                        }
                        _ => false,
                    }
            })
            && self
                .bins
                .windows(2)
                .all(|b| (b[0].end - b[1].start).abs() < 1.)
            && self
                .change
                .reset_share_delta
                .is_none_or(|v| v.is_finite() && (-1. ..=1.).contains(&v))
            && self
                .change
                .click_rate_ratio
                .is_none_or(|v| v.is_finite() && v >= 0.)
            && match self.change.state.as_str() {
                "available" => {
                    self.bins[4..].iter().all(|b| b.state == "available")
                        && self.change.reset_share_delta.is_some()
                        && (rolling || self.change.click_rate_ratio.is_some())
                }
                "awaiting_comparable_hours" => {
                    self.change.reset_share_delta.is_none()
                        && (rolling || self.change.click_rate_ratio.is_none())
                }
                _ => false,
            }
    }
}
fn optional_calendar_sentiment<'de, D>(
    deserializer: D,
) -> Result<Option<CalendarSentiment>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Earlier reports contain the research-only shape. Missing compact fields
    // or an invalid optional section must not discard the whole scored report.
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Reaction {
    version: String,
    initialized_at: Option<f64>,
    history_since: Option<f64>,
    generated_at: f64,
    events: Vec<ReactionEvent>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ReactionEvent {
    id: String,
    kind: String,
    status: String,
    time_basis: String,
    completed_from: f64,
    completed_to: f64,
    first_received_at: Option<f64>,
    evidence: Vec<ReactionEvidence>,
    series: Vec<ReactionSeries>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ReactionEvidence {
    provider: String,
    source_family: String,
    published_at: Option<f64>,
    first_received_at: f64,
    published_to_capture_seconds: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ReactionSeries {
    source: String,
    version: String,
    target: String,
    track: String,
    state: String,
    published_at: Option<f64>,
    source_received_at: Option<f64>,
    observer_received_at: Option<f64>,
    publication_to_capture_seconds: Option<f64>,
    completion_to_update_seconds: Option<f64>,
    known_to_update_seconds: Option<f64>,
    old_cycle_residual_seconds: Option<f64>,
    residual_censored: bool,
    before_probability_24h: Option<f64>,
    after_probability_24h: Option<f64>,
    ui_received_at: Option<f64>,
    ui_presented_at: Option<f64>,
    ui_delay_seconds: Option<f64>,
}
fn optional_reaction<'de, D>(deserializer: D) -> Result<Option<Reaction>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    schema_version: String,
    generated_at: String,
    started_at: String,
    state: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_calendar_sentiment"
    )]
    calendar_sentiment: Option<CalendarSentiment>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_reaction"
    )]
    reaction: Option<Reaction>,
    policy: Policy,
    summary: Summary,
    latest: Vec<Latest>,
    curves: Vec<Curve>,
    #[serde(default)]
    curve_basis: Option<String>,
    scorecards: Vec<Scorecard>,
    comparisons: Vec<Comparison>,
    outcomes: Vec<Outcome>,
    outcome_providers: Vec<Provider>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct View {
    pub connected: bool,
    runtime: runtime::Status,
    checked_at: String,
    forecast_issue: Option<String>,
    evaluation_issue: Option<String>,
    forecast: Option<Consumer>,
    evaluation: Option<Report>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectionConfig {
    forecast_path: PathBuf,
    evaluation_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExportFileState {
    Present {
        modified: Option<std::time::SystemTime>,
        bytes: u64,
    },
    Unavailable(std::io::ErrorKind),
}

impl ExportFileState {
    fn at(path: &Path) -> Self {
        match fs::metadata(path) {
            Ok(metadata) => Self::Present {
                modified: metadata.modified().ok(),
                bytes: metadata.len(),
            },
            Err(error) => Self::Unavailable(error.kind()),
        }
    }
}

/// Watch publication metadata independently of public-source HTTP requests.
/// Only a changed connection file is read; forecast/report contents and their
/// databases remain untouched until the existing consumer handles the event.
pub(crate) struct OutputWatcher {
    directory: PathBuf,
    connection: Option<ExportFileState>,
    paths: Vec<PathBuf>,
    previous: Option<(ExportFileState, Vec<ExportFileState>)>,
}

impl OutputWatcher {
    pub(crate) fn new(directory: &Path) -> Self {
        let mut watcher = Self {
            directory: directory.into(),
            connection: None,
            paths: Vec::new(),
            previous: None,
        };
        watcher.changed();
        watcher
    }

    pub(crate) fn changed(&mut self) -> bool {
        let connection_path = self.directory.join("reset-radar-connection.json");
        let connection = ExportFileState::at(&connection_path);
        if self.connection.as_ref() != Some(&connection) {
            self.paths = match &connection {
                ExportFileState::Unavailable(std::io::ErrorKind::NotFound) => vec![
                    self.directory.join("reset-intelligence/consumer.json"),
                    self.directory.join("reset-observer/report.json"),
                ],
                ExportFileState::Present { .. } => read_json::<ConnectionConfig>(&connection_path)
                    .ok()
                    .filter(|paths| {
                        paths.forecast_path.is_absolute() && paths.evaluation_path.is_absolute()
                    })
                    .map(|paths| vec![paths.forecast_path, paths.evaluation_path])
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            self.connection = Some(connection.clone());
        }
        let current = (
            connection,
            self.paths
                .iter()
                .map(|path| ExportFileState::at(path))
                .collect(),
        );
        let changed = self
            .previous
            .as_ref()
            .is_some_and(|previous| previous != &current);
        self.previous = Some(current);
        changed
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    read_json_with_limit(path, LIMIT)
}

fn read_json_with_limit<T: serde::de::DeserializeOwned>(
    path: &Path,
    limit: u64,
) -> Result<T, String> {
    let file = fs::File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "missing"
        } else {
            "unreadable"
        }
        .to_owned()
    })?;
    let metadata = file.metadata().map_err(|_| "invalid")?;
    if !metadata.is_file() {
        return Err("invalid".into());
    }
    if metadata.len() > limit {
        return Err("too_large".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "unreadable")?;
    if bytes.len() as u64 > limit {
        return Err("too_large".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid".into())
}
fn timestamp(s: &str) -> bool {
    DateTime::parse_from_rfc3339(s).is_ok()
}
fn probability(p: f64) -> bool {
    p.is_finite() && (0.0..=1.0).contains(&p)
}
fn epoch(s: f64) -> bool {
    s.is_finite() && (0.0..=253_402_300_799.0).contains(&s)
}
fn target(s: &str) -> bool {
    matches!(s, "broad_quota_relief" | "automatic_reset")
}
fn source(s: &str) -> bool {
    matches!(
        s,
        "codex_reset" | "monitor" | "nextreset" | "observatory" | "lunar" | "dreaife"
    )
}
fn timing_source(s: &str) -> bool {
    source(s) || s == "didcodex_status"
}
fn outcome_provider(s: &str) -> bool {
    matches!(
        s,
        "quota_events"
            | "completion_feed"
            | "public_catalogue"
            | "codex_reset_status"
            | "didcodex_status"
            | "completion_page"
    )
}
fn outcome_family(s: &str) -> bool {
    outcome_provider(s) || s == "codex_reset_fm"
}
fn provider_family(s: &str) -> &str {
    if matches!(s, "completion_feed" | "completion_page") {
        "codex_reset_fm"
    } else {
        s
    }
}
fn nonnegative(v: f64) -> bool {
    v.is_finite() && v >= 0.
}
impl Reaction {
    fn sanitize(&mut self, generated: f64) -> bool {
        let clock = |v| epoch(v) && v <= generated + 60.;
        if self.version != "reset-reaction-1"
            || !clock(self.generated_at)
            || !self.initialized_at.is_none_or(clock)
            || !self.history_since.is_none_or(clock)
            || self
                .history_since
                .zip(self.initialized_at)
                .is_some_and(|(a, b)| a > b)
        {
            return false;
        }
        self.events.retain(|event| {
            event.id.len() == 64
                && event.id.bytes().all(|c| c.is_ascii_hexdigit())
                && matches!(event.kind.as_str(), "automatic" | "banked" | "unknown")
                && matches!(event.status.as_str(), "confirmed" | "pending")
                && matches!(
                    event.time_basis.as_str(),
                    "operator_observed" | "public_completion_notice"
                )
                && clock(event.completed_from)
                && clock(event.completed_to)
                && event.completed_from <= event.completed_to
                && event.first_received_at.is_none_or(clock)
        });
        self.events.truncate(8);
        for event in &mut self.events {
            event.evidence.retain(|e| {
                outcome_provider(&e.provider)
                    && outcome_family(&e.source_family)
                    && e.source_family == provider_family(&e.provider)
                    && e.published_at.is_none_or(clock)
                    && clock(e.first_received_at)
                    && e.published_to_capture_seconds.is_none_or(nonnegative)
            });
            event.evidence.truncate(16);
            event.series.retain(|s| {
                ((source(&s.source) && s.track == "native")
                    || (s.source == "horizon" && s.track == "model"))
                    && target(&s.target)
                    && !s.version.is_empty()
                    && s.version.len() <= 256
                    && matches!(
                        s.state.as_str(),
                        "updated" | "awaiting_update" | "anchor_unknown" | "no_prior_observation"
                    )
                    && [s.published_at, s.source_received_at, s.observer_received_at]
                        .into_iter()
                        .flatten()
                        .all(clock)
                    && [
                        s.publication_to_capture_seconds,
                        s.completion_to_update_seconds,
                        s.known_to_update_seconds,
                        s.old_cycle_residual_seconds,
                    ]
                    .into_iter()
                    .flatten()
                    .all(nonnegative)
                    && s.before_probability_24h.is_none_or(probability)
                    && s.after_probability_24h.is_none_or(probability)
            });
            event.series.truncate(16);
            for s in &mut event.series {
                // Collector exports cannot claim a desktop receipt. The local
                // view measures its own receipt after loading this report.
                s.ui_received_at = None;
                s.ui_presented_at = None;
                s.ui_delay_seconds = None;
            }
        }
        true
    }
}
fn identity(i: &Identity) -> bool {
    // Private community inputs, URLs and aliases never enter the webview.
    (source(&i.source) && matches!(i.track.as_str(), "native" | "adapted"))
        || (i.source == "horizon" && i.track == "model")
        || (matches!(
            i.source.as_str(),
            "survival_baseline"
                | "without_community"
                | "without_radars"
                | "without_information"
                | "without_response_cohort"
                | "without_comment_estimates"
                | "without_baseline"
                | "equal_components"
        ) && i.track == "shadow")
}
fn public_reference(s: &str) -> bool {
    url::Url::parse(s).is_ok_and(|u| {
        u.scheme() == "https"
            && u.username().is_empty()
            && u.password().is_none()
            && matches!(
                u.host_str(),
                Some(
                    "x.com"
                        | "twitter.com"
                        | "github.com"
                        | "quotaresets.com"
                        | "codex-resets.com"
                        | "codex-reset.pro"
                        | "codex-reset.com"
                        | "didcodexreset.com"
                )
            )
    })
}
fn parse_forecast(path: &Path) -> Result<Consumer, String> {
    let mut c: Consumer = read_json(path)?;
    if c.schema_version != "reset-intelligence-consumer/1" || c.forecasts.len() != 2 {
        return Err("invalid".into());
    }
    if c.forecasts[0].target == c.forecasts[1].target {
        return Err("invalid".into());
    }
    for f in &mut c.forecasts {
        if let Some(cycle) = &mut f.cycle {
            cycle.references.retain(|url| public_reference(url));
            cycle.references.truncate(32);
            let issued = DateTime::parse_from_rfc3339(&f.issued_at)
                .map(|at| at.timestamp() as f64)
                .unwrap_or(0.);
            if cycle.identity.len() != 64
                || !cycle.identity.bytes().all(|b| b.is_ascii_hexdigit())
                || [
                    cycle.occurred_at,
                    cycle.confirmation_at,
                    cycle.invalidation_at,
                ]
                .into_iter()
                .flatten()
                .any(|at| !epoch(at) || at > issued + 60.)
                || !match cycle.time_basis.as_str() {
                    "observed_reset" => {
                        cycle.occurred_at.is_some()
                            && cycle.confirmation_at.is_none()
                            && cycle.invalidation_at == cycle.occurred_at
                    }
                    "completion_confirmation" => {
                        cycle.confirmation_at.is_some()
                            && cycle.occurred_at.is_none()
                            && cycle.invalidation_at == cycle.confirmation_at
                    }
                    "historical_background" => {
                        cycle.occurred_at.is_none()
                            && cycle.confirmation_at.is_none()
                            && cycle.invalidation_at.is_none()
                    }
                    _ => false,
                }
            {
                f.cycle = None;
            }
        }
        if f.combination_method.as_deref() != Some("baseline_plus_revisions") {
            f.combination_method = None;
        }
        if !target(&f.target)
            || !timestamp(&f.issued_at)
            || f.valid_until.as_ref().is_some_and(|s| !timestamp(s))
            || !matches!(f.status.as_str(), "current" | "stale" | "unavailable")
            || f.evaluation_version.as_ref().is_some_and(|v| {
                !v.starts_with(&format!("{}:", f.model_version))
                    || v.len() <= f.model_version.len() + 1
                    || v.len() > 256
                    || !v
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b))
            })
            || (f.status == "current" && (f.curve.len() != 72 || f.valid_until.is_none()))
            || f.curve.len() > 72
            || f.curve.iter().enumerate().any(|(i, p)| {
                p.hours as usize != i + 1
                    || !probability(p.probability)
                    || !probability(p.baseline)
                    || p.without_community_probability
                        .is_some_and(|v| !probability(v))
                    || p.comment_adjustment.is_some_and(|v| !probability(v))
                    || p.component_range.iter().any(|v| !probability(*v))
                    || [
                        p.radar_adjustment,
                        p.community_adjustment,
                        p.coherence_adjustment,
                    ]
                    .into_iter()
                    .flatten()
                    .any(|v| !v.is_finite() || v.abs() > 2.0000001)
                    || (f.combination_method.is_some()
                        && (p.radar_adjustment.is_none()
                            || p.community_adjustment.is_none()
                            || p.coherence_adjustment.is_none()
                            || (p.baseline
                                + p.radar_adjustment.unwrap_or(0.)
                                + p.community_adjustment.unwrap_or(0.)
                                + p.coherence_adjustment.unwrap_or(0.)
                                - p.probability)
                                .abs()
                                > 1e-7))
            })
            || f.curve
                .windows(2)
                .any(|p| p[1].probability < p[0].probability)
        {
            return Err("invalid".into());
        }
        f.sources
            .retain(|s| source(&s.id) || s.id == "survival_baseline");
        for s in &mut f.sources {
            if let Some(c) = &mut s.comment {
                c.timing_sources.retain(|id| timing_source(id));
                for evidence in &mut c.timing_evidence {
                    evidence.links.retain(|link| {
                        public_reference(&link.child_url)
                            && public_reference(&link.parent_url)
                            && epoch(link.at)
                            && link.at < evidence.deadline
                            && link.source.as_deref().is_some_and(|id| {
                                timing_source(id) || matches!(id, "tibo_posts" | "statements")
                            })
                    });
                    evidence.links.truncate(16);
                }
                c.timing_evidence.retain(|evidence| {
                    timing_source(&evidence.source)
                        && epoch(evidence.deadline)
                        && evidence.deadline > c.at
                        && public_reference(&evidence.estimate_url)
                        && public_reference(&evidence.timing_url)
                        && match evidence.relationship.as_str() {
                            "same_reference" => evidence.links.is_empty(),
                            "direct_reply" => !evidence.links.is_empty(),
                            _ => false,
                        }
                });
                c.timing_evidence.truncate(16);
                if !probability(c.probability)
                    || !public_reference(&c.url)
                    || !epoch(c.at)
                    || c.deadline.is_some_and(|d| !epoch(d) || d <= c.at)
                    || (c.included && (c.deadline.is_none() || c.timing_sources.is_empty()))
                    || !probability(c.adjustment_24h)
                    || !probability(c.adjustment_48h)
                    || !matches!(
                        c.state.as_str(),
                        "included"
                            | "closed_or_superseded"
                            | "awaiting_event_window"
                            | "awaiting_source_baseline"
                    )
                {
                    s.comment = None;
                }
            }
            if !matches!(s.response_state.as_deref(), Some("reviewed" | "awaiting")) {
                s.response_state = None;
            }
            if !matches!(
                s.role.as_deref(),
                Some("information" | "background" | "comparison")
            ) {
                s.role = None;
            }
        }
        f.statements
            .retain(|s| public_reference(&s.url) && s.original.len() <= 32_768 && epoch(s.at));
        f.statements.truncate(512);
        if let Some(c) = &mut f.changes {
            let increments =
                c.basis.as_deref() == Some("increment_terms") && f.combination_method.is_some();
            if !increments {
                c.basis = None;
            }
            let signed =
                |n: f64| n.is_finite() && n.abs() <= if increments { 4.0000001 } else { 1.0000001 };
            if !timestamp(&c.since)
                || c.until != f.issued_at
                || c.since >= c.until
                || c.windows.len() != 2
                || c.windows[0].hours != 24
                || c.windows[1].hours != 48
                || c.windows.iter().any(|w| {
                    !probability(w.before)
                        || !probability(w.after)
                        || !signed(w.delta)
                        || (w.after - w.before - w.delta).abs() > 1e-7
                        || !signed(w.coherence_effect)
                        || w.drivers.len() > 16
                        || (w.drivers.iter().map(|d| d.effect).sum::<f64>() + w.coherence_effect
                            - w.delta)
                            .abs()
                            > 1e-7
                        || w.drivers.iter().any(|d| {
                            !(source(&d.id)
                                || matches!(
                                    d.id.as_str(),
                                    "community" | "survival_baseline" | "radar_revision"
                                ))
                                || [d.before, d.after, d.raw_before, d.raw_after]
                                    .into_iter()
                                    .flatten()
                                    .any(|p| {
                                        if increments {
                                            !p.is_finite() || p.abs() > 1.0000001
                                        } else {
                                            !probability(p)
                                        }
                                    })
                                || !probability(d.weight_before)
                                || !probability(d.weight_after)
                                || [d.input_effect, d.weight_effect, d.effect]
                                    .into_iter()
                                    .any(|p| !signed(p))
                                || (d.input_effect + d.weight_effect - d.effect).abs() > 1e-7
                        })
                })
            {
                return Err("invalid".into());
            }
            c.new_statement_ids
                .retain(|id| f.statements.iter().any(|s| &s.id == id));
        }
        for s in &mut f.statements {
            if !matches!(
                s.collector.as_deref(),
                Some("Codex Reset" | "Reset Monitor" | "Codex Reset FM")
            ) {
                s.collector = None;
            }
            if s.context
                .as_ref()
                .is_some_and(|c| !public_reference(&c.url) || c.text.len() > 32_768)
            {
                s.context = None;
            }
            if s.parent.as_ref().is_some_and(|p| {
                !public_reference(&p.url)
                    || p.id.len() > 24
                    || !p.id.bytes().all(|b| b.is_ascii_digit())
                    || p.author.len() > 64
                    || p.text.as_ref().is_some_and(|s| s.len() > 32_768)
            }) {
                s.parent = None;
            }
            if !matches!(
                s.relevance.as_deref(),
                Some("reset_related" | "conditional_pledge" | "context")
            ) {
                s.relevance = None;
            }
        }
        if f.last_event_at.is_some_and(|t| {
            !t.is_finite()
                || t < 0.
                || t > DateTime::parse_from_rfc3339(&f.issued_at)
                    .map(|t| t.timestamp() as f64 + 60.)
                    .unwrap_or(0.)
        }) {
            return Err("invalid".into());
        }
        if let Some(r) = &mut f.response {
            r.reviewed.retain(|id| source(id));
            r.changed.retain(|id| source(id));
            if !matches!(r.kind.as_deref(), Some("announcement" | "revision")) {
                r.kind = None;
            }
            if r.deadline.is_some_and(|t| !t.is_finite() || t <= r.at) {
                r.deadline = None;
                r.kind = None;
            }
            if !r.at.is_finite() || r.at < 0. || !public_reference(&r.url) {
                f.response = None;
            }
        }
        if f.statements_checked_at
            .is_some_and(|t| !t.is_finite() || t < 0.)
        {
            f.statements_checked_at = None;
        }
        f.family_weights
            .retain(|k, _| matches!(k.as_str(), "history" | "baseline" | "radars" | "community"));
        f.community_inputs.retain(|c| {
            matches!(
                c.id.as_str(),
                "date_poll" | "community_poll" | "didcodex_poll"
            )
        });
        for c in &mut f.community_inputs {
            if c.id != "didcodex_poll" {
                c.mapping_kind = None;
                c.mapping_version = None;
                c.reference_share = None;
                c.recent_share = None;
                c.log_odds_change = None;
                c.repeat_sensitivity_units = None;
                c.window_start = None;
                c.window_end = None;
                c.terminal_probability = None;
            }
        }
        if f.family_weights.values().any(|w| !probability(*w))
            || (!f.family_weights.is_empty()
                && (f.family_weights.values().sum::<f64>() - 1.).abs() > 1e-6)
            || f.community_inputs.len() > 3
            || f.community_inputs.iter().any(|c| {
                !probability(c.weight)
                    || !probability(c.probability_24h)
                    || !probability(c.probability_48h)
                    || c.probability_24h > c.probability_48h
                    || c.effective_votes.is_some_and(|n| {
                        !n.is_finite() || n < 0. || n > c.votes.unwrap_or(0) as f64
                    })
                    || c.coverage_hours.is_some_and(|h| !h.is_finite() || h < 0.)
                    || c.recent_votes
                        .is_some_and(|n| n > c.total_votes.unwrap_or(n))
                    || c.observation_window_hours
                        .is_some_and(|h| !h.is_finite() || !(0.0..=6.0001).contains(&h))
                    || c.latest_vote_at
                        .is_some_and(|at| !at.is_finite() || at < 0.)
                    || c.freshness_weight.is_some_and(|w| !probability(w))
                    || c.terminal_probability.is_some_and(|p| !probability(p))
                    || (c.id == "didcodex_poll"
                        && (!matches!(
                            (c.mapping_kind.as_deref(), c.mapping_version.as_deref()),
                            (Some("sentiment_innovation"), Some("calendar-innovation-1"))
                                | (Some("sentiment_level"), Some("calendar-consensus-2"))
                        ) || (c.mapping_kind.as_deref() == Some("sentiment_level")
                            && !c.terminal_probability.is_some_and(probability))
                            || c.effective_votes.is_some()
                            || !c.recent_votes.is_some_and(|n| n >= 50)
                            || !c.reference_share.is_some_and(probability)
                            || !c.recent_share.is_some_and(probability)
                            || !c.log_odds_change.is_some_and(|v| v.is_finite())
                            || !c.repeat_sensitivity_units.is_some_and(nonnegative)
                            || !c.window_start.is_some_and(epoch)
                            || !c.window_end.is_some_and(epoch)
                            || c.window_start >= c.window_end))
            })
            || f.community_coverage.as_ref().is_some_and(|c| {
                !probability(c.supplementary_weight)
                    || c.included > 64
                    || c.unavailable > 64
                    || c.supplementary_checked > 64
                    || c.supplementary_included > c.supplementary_checked
                    || c.supplementary_included > c.included
            })
        {
            return Err("invalid".into());
        }
        if f.sources.iter().any(|s| {
            s.probabilities.len() != 72
                || s.weight.is_some_and(|w| !probability(w))
                || s.baseline_weight.is_some_and(|w| !probability(w))
                || s.signal_baseline
                    .as_ref()
                    .is_some_and(|ps| ps.len() != 72 || ps.iter().any(|p| !probability(*p)))
                || s.baseline_points.as_ref().is_some_and(|ps| {
                    ps.len() > 200
                        || ps
                            .iter()
                            .any(|p| !p[0].is_finite() || p[0] < 0. || !probability(p[1]))
                })
                || s.probabilities.iter().any(|p| !probability(*p))
                || s.source_points.as_ref().is_some_and(|ps| {
                    ps.len() > 200 || ps.iter().any(|p| p[0] < 0. || !probability(p[1]))
                })
        }) || f.community.as_ref().is_some_and(|c| {
            c.probabilities.len() != 72
                || !probability(c.weight)
                || c.probabilities.iter().any(|p| !probability(*p))
        }) {
            return Err("invalid".into());
        }
    }
    Ok(c)
}
fn parse_report(path: &Path) -> Result<Report, String> {
    let mut r: Report = read_json_with_limit(path, REPORT_LIMIT)?;
    if r.curve_basis.as_deref() != Some("received_publication_values") {
        r.curve_basis = None;
    }
    if r.schema_version != "reset-observer/1"
        || !timestamp(&r.generated_at)
        || !timestamp(&r.started_at)
    {
        return Err("invalid".into());
    }
    let generated = DateTime::parse_from_rfc3339(&r.generated_at)
        .map(|t| t.timestamp() as f64)
        .unwrap_or(0.);
    if r.reaction
        .as_mut()
        .is_some_and(|reaction| !reaction.sanitize(generated))
    {
        r.reaction = None;
    }
    if let Some(calendar) = &mut r.calendar_sentiment {
        let generated = DateTime::parse_from_rfc3339(&r.generated_at)
            .map(|t| t.timestamp() as f64)
            .unwrap_or(0.);
        calendar.latest.retain(|sample| {
            sample.source == "didcodex_poll"
                && chrono::NaiveDate::parse_from_str(&sample.date, "%Y-%m-%d").is_ok()
                && sample.date.len() == 10
                && epoch(sample.window_start)
                && epoch(sample.window_end)
                && sample.window_start < sample.window_end
                && epoch(sample.observed_at)
                && sample.observed_at <= generated + 60.
                && sample.recent_votes <= sample.total_clicks
                && sample.weighted_reset_share.is_none_or(probability)
                && sample.reset_share.is_none_or(probability)
                && sample
                    .latest_vote_at
                    .is_none_or(|at| epoch(at) && at <= sample.observed_at)
                && matches!(
                    sample.state.as_str(),
                    "ready_for_evaluation"
                        | "insufficient_recent_votes"
                        | "upcoming"
                        | "closed"
                        | "stale"
                )
        });
        calendar.latest.truncate(128);
        for sample in &mut calendar.latest {
            if sample
                .trend
                .as_ref()
                .is_some_and(|trend| !trend.valid(generated))
            {
                sample.trend = None;
            }
        }
        if calendar.production_weight != 0.
            || calendar.scored_days > calendar.frozen_days
            || calendar.frozen_days > calendar.frozen_trials
            || calendar.scored_trials > calendar.frozen_trials
            || calendar.scored_days > calendar.scored_trials
        {
            r.calendar_sentiment = None;
        }
    }
    if let Some(progress) = &r.summary.scoring_by_horizon {
        let mut horizons = std::collections::BTreeSet::new();
        if progress.len() > 4
            || progress.iter().any(|p| {
                !matches!(p.hours, 6 | 12 | 24 | 48)
                    || !horizons.insert(p.hours)
                    || p.scored_windows > p.settled_windows
                    || p.scored_windows > p.scored_trials
                    || p.scored_trials > p.eligible_rolling_trials
                    || p.eligible_rolling_trials > r.summary.frozen_trials
                    || p.missing_forecast_trials > r.summary.frozen_trials
                    || p.settled_windows > r.summary.frozen_trials
            })
        {
            return Err("invalid".into());
        }
    }
    r.latest
        .retain(|f| identity(&f.identity) && target(&f.target));
    r.curves
        .retain(|f| identity(&f.identity) && target(&f.target) && matches!(f.hours, 24 | 48));
    r.scorecards.retain(|f| {
        identity(&f.identity) && target(&f.target) && matches!(f.hours, 6 | 12 | 24 | 48)
    });
    r.comparisons
        .retain(|f| identity(&f.left) && identity(&f.right) && target(&f.target));
    r.outcome_providers.retain(|p| {
        outcome_provider(&p.provider)
            && timestamp(&p.last_attempt)
            && p.generated_at.as_deref().is_none_or(timestamp)
    });
    r.outcome_providers.truncate(16);
    for provider in &mut r.outcome_providers {
        if !matches!(
            provider.coverage_role.as_deref(),
            Some("complete_inventory" | "positive_only")
        ) {
            provider.coverage_role = None;
        }
        if provider.source_family.as_deref().is_some_and(|family| {
            !outcome_family(family) || family != provider_family(&provider.provider)
        }) {
            provider.source_family = None;
        }
        if !matches!(
            provider.state.as_deref(),
            Some(
                "healthy"
                    | "positive_evidence"
                    | "collector_overdue"
                    | "request_or_decode_failure"
                    | "stale_source_content"
                    | "incomplete_source_evidence"
            )
        ) {
            provider.state = None;
        }
    }
    for o in &mut r.outcomes {
        o.references.retain(|s| public_reference(s));
        if !matches!(
            o.reason.as_deref(),
            Some(
                "conditional_pledge"
                    | "population_unspecified"
                    | "insufficient_primary_confirmation"
                    | "completion_withdrawn"
            )
        ) {
            o.reason = None;
        }
    }
    if r.latest.iter().any(|f| {
        !timestamp(&f.origin)
            || !timestamp(&f.received_at)
            || !timestamp(&f.valid_until)
            || f.probabilities_at_publication
                .values()
                .flatten()
                .any(|p| !probability(*p))
    }) || r.curves.iter().any(|c| {
        c.points.len() > 2000
            || c.points
                .iter()
                .any(|p| !timestamp(&p.at) || p.p.is_some_and(|p| !probability(p)))
    }) || r.outcomes.iter().any(|o| {
        !epoch(o.known_at)
            || [o.from, o.to, o.announced_at]
                .into_iter()
                .flatten()
                .any(|at| !epoch(at))
    }) {
        return Err("invalid".into());
    }
    Ok(r)
}

pub(crate) fn load(directory: &Path, at: &UtcTimestamp) -> View {
    let config_path = directory.join("reset-radar-connection.json");
    let mut view = View {
        checked_at: at.as_str().into(),
        runtime: runtime::status(directory),
        ..View::default()
    };
    let paths = if config_path.exists() {
        view.connected = true;
        match read_json::<ConnectionConfig>(&config_path) {
            Ok(c) if c.forecast_path.is_absolute() && c.evaluation_path.is_absolute() => c,
            _ => {
                view.forecast_issue = Some("connection_invalid".into());
                view.evaluation_issue = view.forecast_issue.clone();
                return view;
            }
        }
    } else {
        ConnectionConfig {
            forecast_path: directory.join("reset-intelligence/consumer.json"),
            evaluation_path: directory.join("reset-observer/report.json"),
        }
    };
    view.connected |=
        view.runtime.managed || paths.forecast_path.exists() || paths.evaluation_path.exists();
    if !view.connected {
        return view;
    }
    // Retain independently the last valid export through a partial write or
    // transient file failure. Cache keys include paths, so reconnecting cannot
    // accidentally display another producer's saved results.
    static SAVED: Mutex<Option<(PathBuf, PathBuf, Option<Consumer>, Option<Report>)>> =
        Mutex::new(None);
    let mut saved = SAVED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((f, e, forecast, evaluation)) = saved.as_ref() {
        if *f == paths.forecast_path && *e == paths.evaluation_path {
            view.forecast = forecast.clone();
            view.evaluation = evaluation.clone();
        }
    }
    match parse_forecast(&paths.forecast_path) {
        Ok(f) => view.forecast = Some(f),
        Err(e) => view.forecast_issue = Some(e),
    }
    match parse_report(&paths.evaluation_path) {
        Ok(r) => view.evaluation = Some(r),
        Err(e) => view.evaluation_issue = Some(e),
    }
    *saved = Some((
        paths.forecast_path,
        paths.evaluation_path,
        view.forecast.clone(),
        view.evaluation.clone(),
    ));
    view
}

// Public changes and the independent report have different clocks. Notify all
// webviews when either output changes, even while the HTTP collector is gated.
pub(crate) fn signature(view: &View) -> String {
    let mut v = view.clone();
    v.checked_at.clear();
    serde_json::to_string(&v).unwrap_or_default()
}

#[cfg(test)]
mod tests;
