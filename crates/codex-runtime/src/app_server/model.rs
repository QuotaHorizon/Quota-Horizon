use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub platform_family: Option<String>,
    #[serde(default)]
    pub platform_os: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountReadResult {
    #[serde(default)]
    pub account: Option<AccountInfo>,
    #[serde(default)]
    pub requires_openai_auth: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    #[serde(rename = "type")]
    pub account_type: String,
    #[serde(default)]
    pub plan_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitsReadResult {
    pub rate_limits: RateLimitSnapshot,
    #[serde(default)]
    pub rate_limits_by_limit_id: Option<BTreeMap<String, RateLimitSnapshot>>,
    #[serde(default)]
    pub rate_limit_reset_credits: Option<RateLimitResetCreditSummary>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitSnapshot {
    #[serde(default)]
    pub limit_id: Option<String>,
    #[serde(default)]
    pub limit_name: Option<String>,
    #[serde(default)]
    pub plan_type: Option<String>,
    #[serde(default)]
    pub primary: Option<RateLimitWindow>,
    #[serde(default)]
    pub secondary: Option<RateLimitWindow>,
    #[serde(default)]
    pub credits: Option<CreditsSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditsSnapshot {
    pub has_credits: bool,
    pub unlimited: bool,
    #[serde(default)]
    pub balance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitWindow {
    pub used_percent: f64,
    #[serde(default)]
    pub window_duration_mins: Option<u64>,
    #[serde(default)]
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitResetCreditSummary {
    pub available_count: u64,
    #[serde(default)]
    pub credits: Option<Vec<RateLimitResetCredit>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitResetCredit {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub granted_at: Option<i64>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub reset_type: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReadResult {
    #[serde(default)]
    pub summary: Option<TokenUsageSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageSummary {
    #[serde(default)]
    pub lifetime_tokens: Option<u64>,
    #[serde(default)]
    pub peak_daily_tokens: Option<u64>,
    #[serde(default)]
    pub longest_running_turn_sec: Option<u64>,
    #[serde(default)]
    pub current_streak_days: Option<u64>,
    #[serde(default)]
    pub longest_streak_days: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageUnavailableReason {
    NotApplicable,
    MethodUnavailable,
    ReadFailed,
    InvalidResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionalUsageRead {
    Available(UsageReadResult),
    Unavailable(UsageUnavailableReason),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NotificationSummary {
    pub rate_limit_updates: u64,
    pub account_updates: u64,
    pub unknown_notifications: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CapacityRead {
    pub initialize: InitializeResult,
    pub account: AccountReadResult,
    pub rate_limits: Option<RateLimitsReadResult>,
    pub usage: OptionalUsageRead,
    pub notifications: NotificationSummary,
    pub stderr_captured_bytes: usize,
    pub stderr_truncated: bool,
}
