use std::collections::HashSet;

use chrono::DateTime;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

pub const STATUS_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct ReasonCode(String);

impl ReasonCode {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 64
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'.'
            });
        if !valid {
            return Err(ValidationError::InvalidReasonCode(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for ReasonCode {
    type Error = ValidationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<String> for ReasonCode {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for ReasonCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct UtcTimestamp(String);

impl UtcTimestamp {
    pub fn parse(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let parsed = DateTime::parse_from_rfc3339(&value)
            .map_err(|_| ValidationError::InvalidUtcTimestamp(value.clone()))?;
        if parsed.offset().local_minus_utc() != 0 {
            return Err(ValidationError::InvalidUtcTimestamp(value));
        }
        Ok(Self(value))
    }

    pub(crate) fn new_unchecked(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_not_before(&self, reference: &Self) -> bool {
        matches!(
            (
                DateTime::parse_from_rfc3339(&self.0),
                DateTime::parse_from_rfc3339(&reference.0),
            ),
            (Ok(candidate), Ok(reference)) if candidate >= reference
        )
    }
}

impl<'de> Deserialize<'de> for UtcTimestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Complete,
    Partial,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Live,
    Stale,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    Tested,
    ExpectedCompatible,
    NotTested,
    Unsupported,
    KnownBroken,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DataStatus {
    pub availability: Availability,
    pub freshness: Freshness,
    pub compatibility: Compatibility,
    pub reason_codes: Vec<ReasonCode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Diagnostic {
    pub code: ReasonCode,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentSnapshot {
    pub environment_id: String,
    pub platform: String,
    pub architecture: String,
    pub boundary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CodexExecutableSnapshot {
    pub executable_id: String,
    pub source: String,
    pub canonical_path: Option<String>,
    pub file_identity: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccountBindingStatus {
    Stable,
    Ephemeral,
    Unavailable,
}

pub const ACCOUNT_FINGERPRINT_PREFIX: &str = "hmac-sha256:v1:";
pub const EPHEMERAL_SESSION_ID_PREFIX: &str = "session:v1:";

/// A non-reversible account key produced by the account-vault boundary.
///
/// The domain accepts only the versioned HMAC representation so callers cannot
/// accidentally pass an email address, display name, or upstream identifier to
/// persistence as though it were already protected.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AccountFingerprint(String);

impl std::fmt::Debug for AccountFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("AccountFingerprint")
            .field(&"<redacted>")
            .finish()
    }
}

impl AccountFingerprint {
    pub fn parse(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let Some(payload) = value.strip_prefix(ACCOUNT_FINGERPRINT_PREFIX) else {
            return Err(ValidationError::InvalidAccountFingerprint);
        };
        let Some((key_id, digest)) = payload.split_once(':') else {
            return Err(ValidationError::InvalidAccountFingerprint);
        };
        if !is_lowercase_uuid(key_id)
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ValidationError::InvalidAccountFingerprint);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the non-secret installation-key identifier embedded in the
    /// versioned HMAC fingerprint. The digest remains opaque.
    pub fn key_id(&self) -> &str {
        // `parse` accepted only the exact `prefix + UUID + ':' + digest`
        // representation, so this split is infallible for a constructed value.
        self.0[ACCOUNT_FINGERPRINT_PREFIX.len()..]
            .split_once(':')
            .map(|(key_id, _)| key_id)
            .expect("validated account fingerprint")
    }
}

/// A process-scoped identifier which is intentionally not serializable.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct EphemeralSessionId(String);

impl std::fmt::Debug for EphemeralSessionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("EphemeralSessionId")
            .field(&"<redacted>")
            .finish()
    }
}

impl EphemeralSessionId {
    pub fn parse(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let Some(identifier) = value.strip_prefix(EPHEMERAL_SESSION_ID_PREFIX) else {
            return Err(ValidationError::InvalidEphemeralSessionId);
        };
        if !is_lowercase_uuid(identifier) {
            return Err(ValidationError::InvalidEphemeralSessionId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountBinding {
    Stable(AccountFingerprint),
    Ephemeral(EphemeralSessionId),
    Unavailable,
}

impl AccountBinding {
    pub fn status(&self) -> AccountBindingStatus {
        match self {
            Self::Stable(_) => AccountBindingStatus::Stable,
            Self::Ephemeral(_) => AccountBindingStatus::Ephemeral,
            Self::Unavailable => AccountBindingStatus::Unavailable,
        }
    }

    pub fn stable_fingerprint(&self) -> Option<&AccountFingerprint> {
        match self {
            Self::Stable(fingerprint) => Some(fingerprint),
            Self::Ephemeral(_) | Self::Unavailable => None,
        }
    }
}

fn is_lowercase_uuid(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    value.bytes().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => byte == b'-',
        _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccountSnapshot {
    pub auth_mode: Option<String>,
    pub plan_type: Option<String>,
    pub binding_status: AccountBindingStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QuotaWindow {
    pub limit_id: String,
    pub label: Option<String>,
    pub window_minutes: Option<u64>,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub resets_at: Option<UtcTimestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResetCredit {
    pub opaque_id: String,
    pub reset_type: String,
    pub status: String,
    pub granted_at: Option<UtcTimestamp>,
    pub expires_at: Option<UtcTimestamp>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResetCreditDetailsStatus {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummaryStatus {
    Available,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResetCreditSummary {
    pub summary_status: SummaryStatus,
    pub available_count: Option<u64>,
    pub details_status: ResetCreditDetailsStatus,
    pub credits: Option<Vec<ResetCredit>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageCreditEntry {
    pub limit_id: String,
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageCreditSummary {
    pub summary_status: SummaryStatus,
    pub entries: Vec<UsageCreditEntry>,
    pub captured_at: UtcTimestamp,
    pub upstream_schema_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QuotaSnapshot {
    pub windows: Vec<QuotaWindow>,
    pub reset_credit_summary: ResetCreditSummary,
    pub usage_credit_summary: UsageCreditSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenUsageSummary {
    pub lifetime_tokens: Option<u64>,
    pub peak_daily_tokens: Option<u64>,
    pub longest_running_turn_sec: Option<u64>,
    pub current_streak_days: Option<u64>,
    pub longest_streak_days: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageSnapshot {
    pub availability: Availability,
    pub summary: Option<TokenUsageSummary>,
    pub reason_codes: Vec<ReasonCode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StatusSnapshot {
    pub schema_version: String,
    pub captured_at: UtcTimestamp,
    pub environment: EnvironmentSnapshot,
    pub codex_executable: Option<CodexExecutableSnapshot>,
    pub account: Option<AccountSnapshot>,
    pub quota: QuotaSnapshot,
    pub usage: UsageSnapshot,
    pub data_status: DataStatus,
    pub diagnostics: Vec<Diagnostic>,
}

impl StatusSnapshot {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != STATUS_SCHEMA_VERSION {
            return Err(ValidationError::UnsupportedSchemaVersion(
                self.schema_version.clone(),
            ));
        }
        if self.environment.environment_id.trim().is_empty() {
            return Err(ValidationError::EmptyEnvironmentId);
        }

        let mut limit_ids = HashSet::new();
        for window in &self.quota.windows {
            if window.limit_id.trim().is_empty() || !limit_ids.insert(&window.limit_id) {
                return Err(ValidationError::InvalidLimitId(window.limit_id.clone()));
            }
            validate_percentage(window.used_percent)?;
            validate_percentage(window.remaining_percent)?;
            if (window.used_percent + window.remaining_percent - 100.0).abs() > 0.01 {
                return Err(ValidationError::InconsistentPercentages {
                    used: window.used_percent,
                    remaining: window.remaining_percent,
                });
            }
        }

        validate_reset_credit_summary(&self.quota.reset_credit_summary)?;
        validate_usage_credit_summary(&self.quota.usage_credit_summary)?;

        Ok(())
    }
}

fn validate_reset_credit_summary(summary: &ResetCreditSummary) -> Result<(), ValidationError> {
    if summary.summary_status == SummaryStatus::Unavailable {
        if summary.available_count.is_some()
            || summary.details_status != ResetCreditDetailsStatus::Unavailable
            || summary.credits.is_some()
        {
            return Err(ValidationError::InvalidSummaryState);
        }
        return Ok(());
    }

    let available_count = summary
        .available_count
        .ok_or(ValidationError::InvalidSummaryState)?;
    let detail_count = summary.credits.as_ref().map_or(0, Vec::len) as u64;
    match summary.details_status {
        ResetCreditDetailsStatus::Complete if detail_count != available_count => {
            return Err(ValidationError::ResetCreditCountMismatch);
        }
        ResetCreditDetailsStatus::Partial if detail_count > available_count => {
            return Err(ValidationError::ResetCreditCountMismatch);
        }
        ResetCreditDetailsStatus::Unavailable if summary.credits.is_some() => {
            return Err(ValidationError::ResetCreditCountMismatch);
        }
        _ => {}
    }

    let mut credit_ids = HashSet::new();
    if let Some(credits) = &summary.credits {
        for credit in credits {
            if credit.opaque_id.trim().is_empty()
                || !credit_ids.insert(&credit.opaque_id)
                || !safe_metadata(&credit.reset_type, 64)
                || !safe_metadata(&credit.status, 64)
                || credit
                    .title
                    .as_deref()
                    .is_some_and(|value| !safe_metadata(value, 512))
                || credit
                    .description
                    .as_deref()
                    .is_some_and(|value| !safe_metadata(value, 512))
            {
                return Err(ValidationError::InvalidResetCredit);
            }
        }
    }
    Ok(())
}

fn validate_usage_credit_summary(summary: &UsageCreditSummary) -> Result<(), ValidationError> {
    match summary.summary_status {
        SummaryStatus::Unavailable if !summary.entries.is_empty() => {
            return Err(ValidationError::InvalidSummaryState);
        }
        SummaryStatus::Available | SummaryStatus::Partial if summary.entries.is_empty() => {
            return Err(ValidationError::InvalidSummaryState);
        }
        _ => {}
    }

    let mut limit_ids = HashSet::new();
    for entry in &summary.entries {
        if !safe_metadata(&entry.limit_id, 128)
            || !limit_ids.insert(&entry.limit_id)
            || entry
                .balance
                .as_deref()
                .is_some_and(|value| !safe_metadata(value, 128))
        {
            return Err(ValidationError::InvalidUsageCredit);
        }
    }

    if summary
        .upstream_schema_fingerprint
        .as_deref()
        .is_some_and(|value| !safe_metadata(value, 128))
    {
        return Err(ValidationError::InvalidUsageCredit);
    }
    Ok(())
}

fn safe_metadata(value: &str, maximum_length: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= maximum_length
        && !value.chars().any(char::is_control)
}

fn validate_percentage(value: f64) -> Result<(), ValidationError> {
    if value.is_finite() && (0.0..=100.0).contains(&value) {
        Ok(())
    } else {
        Err(ValidationError::InvalidPercentage(value))
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum ValidationError {
    #[error("invalid reason code: {0}")]
    InvalidReasonCode(String),
    #[error("timestamp must be RFC 3339 UTC: {0}")]
    InvalidUtcTimestamp(String),
    #[error("unsupported status schema version: {0}")]
    UnsupportedSchemaVersion(String),
    #[error("environment_id must not be empty")]
    EmptyEnvironmentId,
    #[error("account fingerprint must be a versioned HMAC-SHA256 digest")]
    InvalidAccountFingerprint,
    #[error("ephemeral session ID must be a versioned lowercase UUID")]
    InvalidEphemeralSessionId,
    #[error("limit_id must be non-empty and unique: {0}")]
    InvalidLimitId(String),
    #[error("percentage must be finite and within 0..=100: {0}")]
    InvalidPercentage(f64),
    #[error("used and remaining percentages must sum to 100: {used} + {remaining}")]
    InconsistentPercentages { used: f64, remaining: f64 },
    #[error("reset credit details do not match the authoritative count")]
    ResetCreditCountMismatch,
    #[error("summary status conflicts with its fields")]
    InvalidSummaryState,
    #[error("reset-credit detail fields are invalid")]
    InvalidResetCredit,
    #[error("usage-credit summary fields are invalid")]
    InvalidUsageCredit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_are_safe_ascii_identifiers() {
        assert!(ReasonCode::new("codex_not_found").is_ok());
        assert!(ReasonCode::new("contains space").is_err());
        assert!(ReasonCode::new("UPPERCASE").is_err());
        assert!(ReasonCode::new("含中文").is_err());
    }

    #[test]
    fn utc_timestamp_rejects_non_utc_offsets() {
        let timestamp = UtcTimestamp::parse("2026-08-30T00:00:00Z").expect("UTC timestamp");
        let later = UtcTimestamp::parse("2026-08-30T00:00:01Z").expect("later timestamp");
        assert!(later.is_not_before(&timestamp));
        assert!(timestamp.is_not_before(&timestamp));
        assert!(!timestamp.is_not_before(&later));
        assert!(UtcTimestamp::parse("2026-08-30T08:00:00+08:00").is_err());
    }

    #[test]
    fn account_fingerprint_accepts_only_versioned_hmac_digest() {
        let valid = format!(
            "{ACCOUNT_FINGERPRINT_PREFIX}018f47a2-8a71-7f4a-9c35-1f4234a73311:{}",
            "a3".repeat(32)
        );
        let fingerprint = AccountFingerprint::parse(valid.clone()).expect("valid fingerprint");
        assert_eq!(fingerprint.key_id(), "018f47a2-8a71-7f4a-9c35-1f4234a73311");
        assert!(!format!("{fingerprint:?}").contains(&valid));
        assert!(AccountFingerprint::parse("person@example.com").is_err());
        assert!(
            AccountFingerprint::parse(format!(
                "{ACCOUNT_FINGERPRINT_PREFIX}018f47a2-8a71-7f4a-9c35-1f4234a73311:{}",
                "G0".repeat(32)
            ))
            .is_err()
        );
    }

    #[test]
    fn ephemeral_binding_is_typed_but_not_a_persistence_key() {
        let session = EphemeralSessionId::parse("session:v1:018f47a2-8a71-7f4a-9c35-1f4234a73311")
            .expect("valid session ID");
        let binding = AccountBinding::Ephemeral(session);
        assert_eq!(binding.status(), AccountBindingStatus::Ephemeral);
        assert!(binding.stable_fingerprint().is_none());
        assert!(!format!("{binding:?}").contains("018f47a2"));
        assert!(EphemeralSessionId::parse("person@example.com").is_err());
    }
}
