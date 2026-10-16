/** Product view of the two versioned module contracts. Probabilities use [0, 1]. */
export type ResetTarget = "broad_quota_relief" | "automatic_reset";
export interface CommentEstimate {
  probability: number; url: string; at: number; included: boolean; state: string;
  deadline: number | null; timing_sources: string[]; adjustment_24h: number; adjustment_48h: number;
}
export interface QuantChanges {
  basis?: string | null; since: string; until: string; new_statement_ids: string[];
  windows: { hours: number; before: number; after: number; delta: number; coherence_effect: number;
    drivers: { id: string; before: number | null; after: number | null; weight_before: number; weight_after: number;
      raw_before: number | null; raw_after: number | null; input_effect: number; weight_effect: number; effect: number }[] }[];
}
export interface QuantForecast {
  target: ResetTarget; status: "current" | "stale" | "unavailable";
  issued_at: string; valid_until: string | null; model_version: string;
  combination_method?: string | null;
  training_state: string | null; evaluation_windows: number; event_count: number;
  last_event_at?: number | null;
  statements_checked_at?: number | null;
  response?: { at: number; url: string; reviewed: string[]; changed: string[]; kind?: "announcement" | "revision" | null; deadline?: number | null } | null;
  curve: { hours: number; probability: number; baseline: number; component_range: [number, number]; radar_adjustment?: number | null; comment_adjustment?: number | null; community_adjustment?: number | null; coherence_adjustment?: number | null }[];
  sources: { id: string; probabilities: number[]; source_points: [number, number][] | null; origin: number | null; expires_at?: number | null; source_coverage_hours: number | null; weight?: number | null; baseline_points?: [number, number][] | null; role?: "information" | "background" | "comparison" | null; response_state?: string | null; signal_baseline?: number[] | null; baseline_weight?: number | null; comment?: CommentEstimate | null }[];
  community: { probabilities: number[]; weight: number } | null;
  family_weights?: Record<string, number>;
  community_inputs?: { id: string; weight: number; votes: number | null; effective_votes: number | null; coverage_hours: number | null; probability_24h: number; probability_48h: number; total_votes?: number | null; recent_votes?: number | null; observation_window_hours?: number | null; latest_vote_at?: number | null; freshness_weight?: number | null }[];
  community_coverage?: { included: number; unavailable: number; supplementary_included: number; supplementary_checked: number; supplementary_weight: number } | null;
  changes?: QuantChanges | null;
  statements?: { id: string; at: number; url: string; original: string; translation_zh: string | null;
    stage: string | null; event_type: string | null; collector?: string | null; context: { text: string; url: string } | null;
    relevance?: string | null; is_reply?: boolean | null; parent?: {id: string; author: string; url: string; text: string | null} | null }[];
}
export interface ForecastIdentity { source: string; version: string; track: "native" | "adapted" | "model" | "shadow" }
export interface QuantLatest extends ForecastIdentity {
  target: ResetTarget; origin: string; received_at: string; valid_until: string;
  state: string; probabilities_at_publication: Record<string, number | null>;
}
export interface QuantCurve extends ForecastIdentity {
  target: ResetTarget; hours: number; points: { at: string; p: number | null }[];
}
interface Interval { mean: number | null; ci95: [number, number] | null; blocks: number }
export interface QuantScorecard extends ForecastIdentity {
  target: ResetTarget; hours: number; scheduled_windows: number; forecast_coverage: number | null;
  settlement_states: Record<string, number>;
  scores: { n: number; unique_events: number; brier: number | null; log_loss: number | null; state: string;
    brier_interval: Interval; calibration: { n: number; mean_probability: number; event_frequency: number }[] };
  observed_event_scores?: QuantScorecard["scores"] | null;
  alerts: { threshold: number; episodes: number; false_alerts: number; precision: number | null; event_recall: number | null; median_lead_hours: number | null };
}
export interface QuantReport {
  schema_version: "reset-observer/1"; generated_at: string; started_at: string; state: string;
  policy: { reporting_grace_seconds: number; minimum_rank_windows: number; minimum_rank_events: number };
  summary: { forecast_snapshots: number; frozen_trials: number; scored_nonoverlapping_trials: number;
    settlements: Record<string, number>; next_maturity: string | null; settlement_revisions: number;
    scoring_by_horizon?: { hours: number; settled_windows: number; scored_windows: number;
      scored_trials: number; eligible_rolling_trials: number; missing_forecast_trials: number }[] };
  latest: QuantLatest[]; curves: QuantCurve[]; scorecards: QuantScorecard[];
  curve_basis?: string | null;
  comparisons: { left: ForecastIdentity; right: ForecastIdentity; target: ResetTarget; hours: number; n: number;
    unique_events: number; brier_skill: number | null; loss_difference: Interval; error_correlation: number | null }[];
  outcomes: { id: string; status: string; reason?: string | null; kind: string; from: number | null; to: number | null;
    announced_at: number | null; known_at: number; references: string[]; primary_references: number; completion_reports: number }[];
  outcome_providers: { provider: string; last_attempt: string; generated_at: string | null; healthy: boolean;
    state?: "healthy" | "collector_overdue" | "request_or_decode_failure" | "stale_source_content" | "incomplete_source_evidence" | null }[];
}
export interface QuantitativeView {
  connected: boolean; checkedAt: string; forecastIssue: string | null; evaluationIssue: string | null;
  runtime?: { managed: boolean; state: "external" | "checking" | "starting" | "running" | "retrying";
    checkedAt: string | null; retryAt: string | null; issue: string | null };
  forecast: { schema_version: "reset-intelligence-consumer/1"; forecasts: QuantForecast[] } | null;
  evaluation: QuantReport | null;
}
