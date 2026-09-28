//! Read-only projection of the existing journal. Repeated checks, parser-only
//! revisions and irrelevant context must not look like new public reset news.
use super::*;
use capacity_domain::public_reset::{PublicEventKind, PublicSignalSemantics};
use std::collections::BTreeMap;

const MAX_RECENT_CHANGES: usize = 128;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PublicRevisionChange {
    previous: Option<PublicResetSignal>,
    pub(super) current: PublicResetSignal,
    pub(super) unread: bool,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PublicRevisionHistory {
    pub(super) items: Vec<PublicRevisionChange>,
    total_count: usize,
    pub(super) read_version: u32,
}

#[derive(Default)]
pub(super) struct RecentChanges {
    // Sorted by observation time, with deterministic ties; bounded while loading,
    // not after cloning the whole journal a second time.
    items: BTreeMap<(DateTime<chrono::FixedOffset>, String, u32), PublicRevisionChange>,
    count: usize,
}

fn relevant(signal: &PublicResetSignal) -> bool {
    signal.scope != PublicEvidenceScope::Personal
        && signal.source.source_class != PublicSourceClass::ProviderUiObservation
        && (matches!(
            signal.kind,
            PublicEventKind::GlobalFullReset | PublicEventKind::GlobalBankedResetGrant
        ) || (signal.kind == PublicEventKind::Unclassified
            && signal.semantics != PublicSignalSemantics::ContextOnly))
}

fn normalized_words(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn same_time(before: Option<&UtcTimestamp>, after: Option<&UtcTimestamp>) -> bool {
    match (before, after) {
        (None, None) => true,
        (Some(before), Some(after)) => before.is_not_before(after) && after.is_not_before(before),
        _ => false,
    }
}

fn same_content(before: &PublicResetSignal, after: &PublicResetSignal) -> bool {
    // Recorded/collected times, content hash and parser version are bookkeeping,
    // not a source changing its claim. A changed public fact still survives.
    before.kind == after.kind
        && before.scope == after.scope
        && before.semantics == after.semantics
        && same_time(before.occurred_at.as_ref(), after.occurred_at.as_ref())
        && before.announcement_timing == after.announcement_timing
        && before.source.canonical_url == after.source.canonical_url
        && same_time(
            before.source.published_at.as_ref(),
            after.source.published_at.as_ref(),
        )
        && before.source.author == after.source.author
        && before.source.source_class == after.source.source_class
        && before.source.review == after.source.review
        && normalized_words(&before.title) == normalized_words(&after.title)
        && normalized_words(&before.summary) == normalized_words(&after.summary)
}

impl RecentChanges {
    pub(super) fn record(
        &mut self,
        previous: Option<&PublicResetSignal>,
        current: &PublicResetSignal,
    ) {
        if (!relevant(current) && !previous.is_some_and(relevant))
            || previous.is_some_and(|before| same_content(before, current))
        {
            return;
        }
        // Validation has already checked timestamps. Do not turn an invalid one
        // into a current-looking change if this helper is reused elsewhere.
        let Ok(recorded) = DateTime::parse_from_rfc3339(current.recorded_at.as_str()) else {
            return;
        };
        self.count += 1;
        self.items.insert(
            (recorded, current.signal_id.clone(), current.revision),
            PublicRevisionChange {
                previous: previous.cloned(),
                current: current.clone(),
                unread: false,
            },
        );
        if self.items.len() > MAX_RECENT_CHANGES {
            self.items.pop_first();
        }
    }

    pub(super) fn finish(self) -> PublicRevisionHistory {
        PublicRevisionHistory {
            items: self.items.into_values().rev().collect(),
            total_count: self.count,
            read_version: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_projection_is_bounded_in_observation_order_not_signal_order() {
        let mut signal = super::super::tests::batch(1, "confirmed")
            .candidates
            .remove(0);
        let mut history = RecentChanges::default();
        for index in (0..300).rev() {
            signal.signal_id = format!("bounded-{index:03}");
            signal.recorded_at = UtcTimestamp::parse(format!(
                "2026-09-12T01:{:02}:{:02}Z",
                index / 60,
                index % 60
            ))
            .unwrap();
            history.record(None, &signal);
            assert!(history.items.len() <= MAX_RECENT_CHANGES);
        }
        let result = history.finish();
        assert_eq!(result.total_count, 300);
        assert_eq!(result.items.len(), 128);
        assert_eq!(result.items[0].current.signal_id, "bounded-299");
        assert_eq!(result.items[127].current.signal_id, "bounded-172");
    }
}
