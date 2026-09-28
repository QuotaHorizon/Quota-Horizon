//! Public evidence only. No account data, network access, or quota side effects.
//!
//! Trackers discover candidates; their labels do not establish official facts.
//! The ledger records when an assessment became available so later corrections
//! cannot leak into an earlier backtest. A signature authenticates the publisher,
//! not the truth of a claim: source review remains a separate requirement.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::UtcTimestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PublicEventKind {
    GlobalFullReset,
    GlobalBankedResetGrant,
    QuotaPolicyChange,
    ServiceIncident,
    Unclassified,
}

impl PublicEventKind {
    pub fn is_forecast_target(self) -> bool {
        matches!(self, Self::GlobalFullReset | Self::GlobalBankedResetGrant)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PublicSourceClass {
    OfficialAnnouncement,
    OfficialDocumentation,
    OfficialSocial,
    OfficialStatus,
    IndependentTracker,
    ProviderUiObservation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PublicEvidenceReview {
    PrimaryReviewed,
    Indirect,
    Unreviewed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PublicEvidenceScope {
    BroadCodex,
    Personal,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PublicSignalSemantics {
    ContextOnly,
    PossibleSignal,
    ExplicitFutureReset,
    ExplicitTimedReset,
    ConfirmedReset,
    NegativeSignal,
    Corrected,
    Retracted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PublicEvidenceDisposition {
    Confirmed,
    Announced,
    Possible,
    Negative,
    Context,
    NeedsReview,
    Corrected,
    Retracted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicEvidenceSource {
    /// Canonical public item, not a tracker mirror or a private account URL.
    pub canonical_url: String,
    pub author: String,
    pub source_class: PublicSourceClass,
    pub review: PublicEvidenceReview,
    /// Unknown publication time stays unknown, never replaced by collection time.
    pub published_at: Option<UtcTimestamp>,
    pub collected_at: UtcTimestamp,
    /// SHA-256 of the normalized evidence record, not an asserted full-page hash.
    pub content_sha256: String,
    pub parser_version: String,
    pub discovered_via: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicAnnouncementTiming {
    /// A tracker's interpretation of a calendar date, never an exact official
    /// deadline or a confirmed occurrence. Conversion belongs to presentation.
    pub expected_on: String,
    pub time_zone: String,
    pub source_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicResetSignal {
    pub signal_id: String,
    pub revision: u32,
    /// One original statement = one family, regardless of mirror count.
    pub evidence_family_id: String,
    /// Curated identity of the underlying event; announcements and confirmation
    /// can be distinct signals belonging to the same event.
    pub event_id: Option<String>,
    pub kind: PublicEventKind,
    pub scope: PublicEvidenceScope,
    pub semantics: PublicSignalSemantics,
    pub source: PublicEvidenceSource,
    /// The time this exact revision was recorded, including later review.
    pub recorded_at: UtcTimestamp,
    /// Exact occurrence time only if supported, never a guessed receipt time.
    pub occurred_at: Option<UtcTimestamp>,
    pub title: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announcement_timing: Option<PublicAnnouncementTiming>,
}

impl PublicResetSignal {
    pub fn disposition(&self) -> PublicEvidenceDisposition {
        use PublicEvidenceDisposition as D;
        use PublicSignalSemantics as S;
        if self.semantics == S::Retracted {
            return D::Retracted;
        }
        if self.semantics == S::Corrected {
            return D::Corrected;
        }
        if self.semantics == S::ContextOnly || !self.kind.is_forecast_target() {
            return D::Context;
        }
        // The official usage dashboard describes an account, not a global event.
        let public_primary = matches!(
            self.source.source_class,
            PublicSourceClass::OfficialAnnouncement
                | PublicSourceClass::OfficialDocumentation
                | PublicSourceClass::OfficialSocial
                | PublicSourceClass::OfficialStatus
        ) && self.source.review == PublicEvidenceReview::PrimaryReviewed;
        if !public_primary || self.scope != PublicEvidenceScope::BroadCodex {
            return D::NeedsReview;
        }
        match self.semantics {
            S::ConfirmedReset => D::Confirmed,
            S::ExplicitFutureReset | S::ExplicitTimedReset => D::Announced,
            S::PossibleSignal => D::Possible,
            S::NegativeSignal => D::Negative,
            S::ContextOnly => D::Context,
            S::Corrected => D::Corrected,
            S::Retracted => D::Retracted,
        }
    }

    /// A confirmed grant never becomes a full reset. Unknown exact event times
    /// remain visible facts but are not automatically usable as backtest labels.
    pub fn is_dated_confirmation_for(&self, kind: PublicEventKind) -> bool {
        kind.is_forecast_target()
            && self.kind == kind
            && self.disposition() == PublicEvidenceDisposition::Confirmed
            && self.event_id.is_some()
            && self.occurred_at.is_some()
    }

    pub fn validate(&self) -> Result<(), PublicLedgerError> {
        if self.announcement_timing.as_ref().is_some_and(|timing| {
            timing.expected_on.len() != 10
                || chrono::NaiveDate::parse_from_str(&timing.expected_on, "%Y-%m-%d").is_err()
                || !text_field(&timing.time_zone, 64)
                || !timing.time_zone.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'+')
                })
                || !public_url_shape(&timing.source_url)
        }) {
            return Err(PublicLedgerError::InvalidRecord);
        }
        if !identifier(&self.signal_id)
            || !identifier(&self.evidence_family_id)
            || self.event_id.as_ref().is_some_and(|id| !identifier(id))
            || self.revision == 0
            || !text_field(&self.title, 256)
            || !text_field(&self.summary, 4096)
            || !text_field(&self.source.author, 128)
            || !identifier(&self.source.parser_version)
            || self.source.content_sha256.len() != 64
            || !self
                .source
                .content_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.source.discovered_via.len() > 16
        {
            return Err(PublicLedgerError::InvalidRecord);
        }
        // Full URL parsing / host policy belongs to the source adapter. Reject
        // obvious non-public forms here too; never permit local paths or HTML.
        if !public_url_shape(&self.source.canonical_url)
            || self
                .source
                .discovered_via
                .iter()
                .any(|url| !public_url_shape(url))
        {
            return Err(PublicLedgerError::InvalidRecord);
        }
        if !self.recorded_at.is_not_before(&self.source.collected_at)
            || self
                .source
                .published_at
                .as_ref()
                .is_some_and(|time| !self.source.collected_at.is_not_before(time))
            || self
                .occurred_at
                .as_ref()
                .is_some_and(|time| !self.recorded_at.is_not_before(time))
        {
            return Err(PublicLedgerError::InvalidTimeOrder);
        }
        Ok(())
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 192
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':' | b'/'))
}

fn text_field(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max
        && value
            .chars()
            .all(|c| !c.is_control() || matches!(c, '\n' | '\t'))
}

fn public_url_shape(value: &str) -> bool {
    value.starts_with("https://")
        && value.len() <= 2048
        && !value.chars().any(|c| c.is_whitespace() || c.is_control())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PublicLedgerError {
    #[error("invalid public evidence record")]
    InvalidRecord,
    #[error("public evidence timestamps are out of order")]
    InvalidTimeOrder,
    #[error("public evidence revision conflicts with its history")]
    RevisionConflict,
    #[error("public evidence ledger is full")]
    LimitExceeded,
}

/// Append-only in-memory reducer. Persistence and signed transport wrap this
/// reducer; neither may replace old revisions when a source changes its story.
#[derive(Debug, Clone, Default)]
pub struct PublicResetLedger {
    revisions: BTreeMap<String, Vec<PublicResetSignal>>,
    count: usize,
}

impl PublicResetLedger {
    pub const MAX_REVISIONS: usize = 16_384;

    /// Exact retries are idempotent. A changed record needs a new revision and
    /// cannot claim it was observed earlier than the version it replaces.
    pub fn append(&mut self, signal: PublicResetSignal) -> Result<bool, PublicLedgerError> {
        signal.validate()?;
        let history = self.revisions.get(&signal.signal_id);
        if let Some(previous) =
            history.and_then(|items| items.iter().find(|item| item.revision == signal.revision))
        {
            return if previous == &signal {
                Ok(false)
            } else {
                Err(PublicLedgerError::RevisionConflict)
            };
        }
        match history.and_then(|items| items.last()) {
            None if signal.revision != 1 => return Err(PublicLedgerError::RevisionConflict),
            Some(previous)
                if previous.revision.checked_add(1) != Some(signal.revision)
                    || !signal.recorded_at.is_not_before(&previous.recorded_at)
                    || signal.evidence_family_id != previous.evidence_family_id =>
            {
                return Err(PublicLedgerError::RevisionConflict);
            }
            _ => {}
        }
        if self.count >= Self::MAX_REVISIONS {
            return Err(PublicLedgerError::LimitExceeded);
        }
        self.revisions
            .entry(signal.signal_id.clone())
            .or_default()
            .push(signal);
        self.count += 1;
        Ok(true)
    }

    /// Cut off by observation/review time, not merely original publication time.
    pub fn as_of(&self, cutoff: &UtcTimestamp) -> Vec<&PublicResetSignal> {
        self.revisions
            .values()
            .filter_map(|history| {
                history
                    .iter()
                    .rev()
                    .find(|item| cutoff.is_not_before(&item.recorded_at))
            })
            .collect()
    }

    pub fn evidence_family_count_at(&self, cutoff: &UtcTimestamp) -> usize {
        self.as_of(cutoff)
            .iter()
            .map(|signal| &signal.evidence_family_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }

    /// Multiple confirmations/mirrors of one reset never become extra resets.
    /// A correction or retraction in an evidence family suppresses that family
    /// even when a stale mirror still carries the original confirmation.
    pub fn dated_confirmations_at(
        &self,
        cutoff: &UtcTimestamp,
        kind: PublicEventKind,
    ) -> Vec<&PublicResetSignal> {
        let visible = self.as_of(cutoff);
        let withdrawn: std::collections::BTreeSet<_> = visible
            .iter()
            .filter(|item| {
                matches!(
                    item.disposition(),
                    PublicEvidenceDisposition::Corrected | PublicEvidenceDisposition::Retracted
                )
            })
            .map(|item| item.evidence_family_id.as_str())
            .collect();
        let mut by_event: BTreeMap<&str, &PublicResetSignal> = BTreeMap::new();
        let mut conflicting_events = std::collections::BTreeSet::new();
        for signal in visible.iter().copied().filter(|item| {
            item.is_dated_confirmation_for(kind)
                && !withdrawn.contains(item.evidence_family_id.as_str())
        }) {
            let id = signal
                .event_id
                .as_deref()
                .expect("dated confirmation has event ID");
            if let Some(previous) = by_event.get(id) {
                let equal_time = match (&previous.occurred_at, &signal.occurred_at) {
                    (Some(left), Some(right)) => {
                        left.is_not_before(right) && right.is_not_before(left)
                    }
                    _ => false,
                };
                if !equal_time {
                    conflicting_events.insert(id);
                }
            }
            by_event.entry(id).or_insert(signal);
        }
        by_event
            .into_iter()
            .filter(|(id, _)| !conflicting_events.contains(id))
            .map(|(_, signal)| signal)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time(hour: u32) -> UtcTimestamp {
        UtcTimestamp::parse(format!("2026-09-07T{hour:02}:00:00Z")).unwrap()
    }

    fn signal() -> PublicResetSignal {
        PublicResetSignal {
            signal_id: "signal:1".into(),
            revision: 1,
            evidence_family_id: "statement:1".into(),
            event_id: Some("reset:1".into()),
            kind: PublicEventKind::GlobalFullReset,
            scope: PublicEvidenceScope::BroadCodex,
            semantics: PublicSignalSemantics::ConfirmedReset,
            source: PublicEvidenceSource {
                canonical_url: "https://openai.com/index/example-reset".into(),
                author: "OpenAI".into(),
                source_class: PublicSourceClass::OfficialAnnouncement,
                review: PublicEvidenceReview::PrimaryReviewed,
                published_at: Some(time(1)),
                collected_at: time(2),
                content_sha256: "a".repeat(64),
                parser_version: "review-v1".into(),
                discovered_via: vec![],
            },
            recorded_at: time(2),
            occurred_at: Some(time(1)),
            title: "Synthetic broad reset".into(),
            summary: "Synthetic test evidence, not a real event.".into(),
            announcement_timing: None,
        }
    }

    #[test]
    fn tracker_confirmed_and_dashboard_observation_are_not_official_confirmation() {
        for class in [
            PublicSourceClass::IndependentTracker,
            PublicSourceClass::ProviderUiObservation,
        ] {
            let mut candidate = signal();
            candidate.source.source_class = class;
            assert_eq!(
                candidate.disposition(),
                PublicEvidenceDisposition::NeedsReview
            );
            assert!(!candidate.is_dated_confirmation_for(PublicEventKind::GlobalFullReset));
        }
    }

    #[test]
    fn canonical_official_link_without_primary_review_is_still_indirect() {
        let mut candidate = signal();
        candidate.source.review = PublicEvidenceReview::Indirect;
        assert_eq!(
            candidate.disposition(),
            PublicEvidenceDisposition::NeedsReview
        );
        candidate.source.review = PublicEvidenceReview::PrimaryReviewed;
        candidate.scope = PublicEvidenceScope::Personal;
        assert_eq!(
            candidate.disposition(),
            PublicEvidenceDisposition::NeedsReview
        );
    }

    #[test]
    fn grants_incidents_and_unknown_receipt_times_never_become_full_reset_labels() {
        let mut candidate = signal();
        candidate.kind = PublicEventKind::GlobalBankedResetGrant;
        assert_eq!(
            candidate.disposition(),
            PublicEvidenceDisposition::Confirmed
        );
        assert!(!candidate.is_dated_confirmation_for(PublicEventKind::GlobalFullReset));
        candidate.occurred_at = None;
        assert!(!candidate.is_dated_confirmation_for(PublicEventKind::GlobalBankedResetGrant));
        candidate.kind = PublicEventKind::ServiceIncident;
        assert_eq!(candidate.disposition(), PublicEvidenceDisposition::Context);
    }

    #[test]
    fn review_and_retraction_do_not_rewrite_earlier_backtests() {
        let mut ledger = PublicResetLedger::default();
        let mut candidate = signal();
        candidate.source.review = PublicEvidenceReview::Indirect;
        ledger.append(candidate.clone()).unwrap();
        candidate.revision = 2;
        candidate.recorded_at = time(3);
        candidate.source.review = PublicEvidenceReview::PrimaryReviewed;
        ledger.append(candidate.clone()).unwrap();
        candidate.revision = 3;
        candidate.recorded_at = time(4);
        candidate.semantics = PublicSignalSemantics::Retracted;
        ledger.append(candidate).unwrap();
        assert!(ledger.as_of(&time(1)).is_empty());
        assert!(
            ledger
                .dated_confirmations_at(&time(2), PublicEventKind::GlobalFullReset)
                .is_empty()
        );
        assert_eq!(
            ledger
                .dated_confirmations_at(&time(3), PublicEventKind::GlobalFullReset)
                .len(),
            1
        );
        assert!(
            ledger
                .dated_confirmations_at(&time(4), PublicEventKind::GlobalFullReset)
                .is_empty()
        );
    }

    #[test]
    fn retries_are_idempotent_but_edits_need_ordered_revisions() {
        let mut ledger = PublicResetLedger::default();
        let mut candidate = signal();
        assert_eq!(ledger.append(candidate.clone()), Ok(true));
        assert_eq!(ledger.append(candidate.clone()), Ok(false));
        candidate.summary = "Changed assessment".into();
        assert_eq!(
            ledger.append(candidate.clone()),
            Err(PublicLedgerError::RevisionConflict)
        );
        candidate.revision = 3;
        assert_eq!(
            ledger.append(candidate.clone()),
            Err(PublicLedgerError::RevisionConflict)
        );
        candidate.revision = 2;
        candidate.recorded_at = time(1);
        assert_eq!(
            ledger.append(candidate),
            Err(PublicLedgerError::InvalidTimeOrder)
        );
    }

    #[test]
    fn mirrors_and_multiple_statements_do_not_multiply_events() {
        let mut ledger = PublicResetLedger::default();
        let candidate = signal();
        ledger.append(candidate.clone()).unwrap();
        let mut mirror = candidate.clone();
        mirror.signal_id = "mirror:1".into();
        ledger.append(mirror.clone()).unwrap();
        assert_eq!(ledger.evidence_family_count_at(&time(3)), 1);
        assert_eq!(
            ledger
                .dated_confirmations_at(&time(3), candidate.kind)
                .len(),
            1
        );
        let mut second_statement = candidate;
        second_statement.signal_id = "statement:2".into();
        second_statement.evidence_family_id = "family:2".into();
        ledger.append(second_statement.clone()).unwrap();
        assert_eq!(
            ledger
                .dated_confirmations_at(&time(3), second_statement.kind)
                .len(),
            1
        );
        mirror.revision = 2;
        mirror.recorded_at = time(4);
        mirror.semantics = PublicSignalSemantics::Retracted;
        ledger.append(mirror).unwrap();
        let confirmations = ledger.dated_confirmations_at(&time(4), second_statement.kind);
        assert_eq!(confirmations.len(), 1);
        assert_eq!(confirmations[0].signal_id, "statement:2");
    }

    #[test]
    fn optional_tracker_date_is_backward_compatible_and_never_an_occurrence() {
        let original = signal();
        let encoded = serde_json::to_string(&original).unwrap();
        assert!(!encoded.contains("announcementTiming"));
        let mut decoded: PublicResetSignal = serde_json::from_str(&encoded).unwrap();
        assert!(decoded.announcement_timing.is_none());
        decoded.announcement_timing = Some(PublicAnnouncementTiming {
            expected_on: "2026-09-11".into(),
            time_zone: "America/Los_Angeles".into(),
            source_url: "https://quotaresets.com/api/v1/events.json".into(),
        });
        assert!(decoded.validate().is_ok());
        assert_eq!(decoded.occurred_at, original.occurred_at);
        assert_eq!(decoded.disposition(), original.disposition());
        decoded.announcement_timing.as_mut().unwrap().expected_on = "2026-02-30".into();
        assert_eq!(decoded.validate(), Err(PublicLedgerError::InvalidRecord));
    }

    #[test]
    fn malformed_provenance_and_future_publication_are_rejected() {
        let mut candidate = signal();
        candidate.source.published_at = Some(time(3));
        assert_eq!(
            candidate.validate(),
            Err(PublicLedgerError::InvalidTimeOrder)
        );
        candidate.source.published_at = None;
        candidate.source.canonical_url = "javascript:alert(1)".into();
        assert_eq!(candidate.validate(), Err(PublicLedgerError::InvalidRecord));
        candidate.source.canonical_url = "https://openai.com/".into();
        candidate.source.content_sha256 = "not-a-digest".into();
        assert_eq!(candidate.validate(), Err(PublicLedgerError::InvalidRecord));
    }

    #[test]
    fn conflicting_occurrence_times_need_review_not_an_arbitrary_training_label() {
        let mut ledger = PublicResetLedger::default();
        let first = signal();
        let mut second = first.clone();
        second.signal_id = "other:1".into();
        second.evidence_family_id = "other-family:1".into();
        second.occurred_at = Some(time(2));
        ledger.append(first).unwrap();
        ledger.append(second).unwrap();
        assert!(
            ledger
                .dated_confirmations_at(&time(3), PublicEventKind::GlobalFullReset)
                .is_empty()
        );
    }
}
