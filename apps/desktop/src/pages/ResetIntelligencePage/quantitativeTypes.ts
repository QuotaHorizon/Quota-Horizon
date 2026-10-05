/** Product view of the two versioned module contracts. Probabilities use [0, 1]. */
export type ResetTarget = "broad_quota_relief" | "automatic_reset";
export interface QuantChanges {
  since: string; until: string; new_statement_ids: string[];
  windows: { hours: number; before: number; after: number; delta: number; coherence_effect: number;
    drivers: { id: string; before: number | null; after: number | null; weight_before: number; weight_after: number;
      raw_before: number | null; raw_after: number | null; input_effect: number; weight_effect: number; effect: number }[] }[];
}
export interface QuantForecast {
  target: ResetTarget; status: "current" | "stale" | "unavailable";
  issued_at: string; valid_until: string | null; model_version: string;
  training_state: string | null; evaluation_windows: number; event_count: number;
  curve: { hours: number; probability: number; baseline: number; component_range: [number, number] }[];
  sources: { id: string; probabilities: number[]; source_points: [number, number][] | null; origin: number | null; source_coverage_hours: number | null }[];
  community: { probabilities: number[]; weight: number } | null;
  changes?: QuantChanges | null;
  statements?: { id: string; at: number; url: string; original: string; translation_zh: string | null;
    stage: string | null; event_type: string | null; collector?: string | null; context: { text: string; url: string } | null }[];
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
  comparisons: { left: ForecastIdentity; right: ForecastIdentity; target: ResetTarget; hours: number; n: number;
    unique_events: number; brier_skill: number | null; loss_difference: Interval; error_correlation: number | null }[];
  outcomes: { id: string; status: string; reason?: string | null; kind: string; from: number | null; to: number | null;
    announced_at: number | null; known_at: number; references: string[]; primary_references: number; completion_reports: number }[];
  outcome_providers: { provider: string; last_attempt: string; generated_at: string | null; healthy: boolean }[];
}
export interface QuantitativeView {
  connected: boolean; checkedAt: string; forecastIssue: string | null; evaluationIssue: string | null;
  runtime?: { managed: boolean; state: "external" | "checking" | "starting" | "running" | "retrying";
    checkedAt: string | null; retryAt: string | null; issue: string | null };
  forecast: { schema_version: "reset-intelligence-consumer/1"; forecasts: QuantForecast[] } | null;
  evaluation: QuantReport | null;
}
