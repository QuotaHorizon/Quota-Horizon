use capacity_domain::{ResetCreditDetailsStatus, ResetCreditSummary, SummaryStatus, UtcTimestamp};
use chrono::DateTime;

use crate::DesktopResetCreditView;

/// Summarize the captured card inventory without replacing the source's available count.
pub(crate) fn reset_credit_view(
    summary: &ResetCreditSummary,
    captured_at: &UtcTimestamp,
) -> DesktopResetCreditView {
    let captured = DateTime::parse_from_rfc3339(captured_at.as_str()).ok();
    let mut known = Vec::new();
    if let (Some(captured), Some(credits)) = (captured, summary.credits.as_ref()) {
        for credit in credits
            .iter()
            .filter(|credit| credit.status.eq_ignore_ascii_case("available"))
        {
            if let Some(expires) = credit
                .expires_at
                .as_ref()
                .and_then(|time| DateTime::parse_from_rfc3339(time.as_str()).ok())
                .filter(|expires| *expires > captured)
            {
                known.push(expires);
            }
        }
    }
    known.sort();
    let known_expiry_count = known.len() as u64;
    DesktopResetCreditView {
        summary_status: summary.summary_status,
        available_count: summary.available_count,
        details_status: summary.details_status,
        next_expires_at: known.first().map(|value| value.to_rfc3339()),
        known_expiry_count,
        expiry_details_complete: summary.summary_status == SummaryStatus::Available
            && summary.details_status == ResetCreditDetailsStatus::Complete
            && summary.credits.is_some()
            && summary.available_count == Some(known_expiry_count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capacity_domain::ResetCredit;

    fn credit(status: &str, expiry: Option<&str>) -> ResetCredit {
        ResetCredit {
            opaque_id: "fixture".into(),
            reset_type: "codex".into(),
            status: status.into(),
            granted_at: None,
            expires_at: expiry.map(|value| UtcTimestamp::parse(value).unwrap()),
            title: None,
            description: None,
        }
    }
    fn summary(count: u64, credits: Vec<ResetCredit>) -> ResetCreditSummary {
        ResetCreditSummary {
            summary_status: SummaryStatus::Available,
            available_count: Some(count),
            details_status: ResetCreditDetailsStatus::Complete,
            credits: Some(credits),
        }
    }
    fn now() -> UtcTimestamp {
        UtcTimestamp::parse("2026-10-30T00:00:00Z").unwrap()
    }

    #[test]
    fn uses_only_available_future_cards_and_compares_absolute_times() {
        let result = reset_credit_view(
            &summary(
                2,
                vec![
                    credit("available", Some("2026-11-01T10:00:00Z")),
                    credit("AVAILABLE", Some("2026-10-31T04:00:00+00:00")),
                    credit("used", Some("2026-10-30T01:00:00Z")),
                    credit("expired", Some("2026-10-29T00:00:00Z")),
                ],
            ),
            &now(),
        );
        assert_eq!(result.known_expiry_count, 2);
        assert!(result.expiry_details_complete);
        assert_eq!(
            result.next_expires_at.as_deref(),
            Some("2026-10-31T04:00:00+00:00")
        );
        assert_eq!(result.available_count, Some(2));
    }

    #[test]
    fn missing_or_elapsed_dates_do_not_become_complete_or_renewed_cards() {
        let result = reset_credit_view(
            &summary(
                3,
                vec![
                    credit("available", None),
                    credit("available", Some("2026-10-30T00:00:00Z")),
                    credit("available", Some("2026-10-31T00:00:00Z")),
                ],
            ),
            &now(),
        );
        assert_eq!(result.available_count, Some(3));
        assert_eq!(result.known_expiry_count, 1);
        assert!(!result.expiry_details_complete);
        assert_eq!(
            result.next_expires_at.as_deref(),
            Some("2026-10-31T00:00:00+00:00")
        );
        let empty = reset_credit_view(&summary(0, vec![]), &now());
        assert_eq!(empty.next_expires_at, None);
        assert!(empty.expiry_details_complete);
    }

    #[test]
    fn partial_or_absent_inventory_keeps_expiry_coverage_unknown() {
        let mut data = summary(1, vec![credit("available", Some("2026-10-31T00:00:00Z"))]);
        data.details_status = ResetCreditDetailsStatus::Partial;
        assert!(!reset_credit_view(&data, &now()).expiry_details_complete);
        data.credits = None;
        let result = reset_credit_view(&data, &now());
        assert_eq!(result.next_expires_at, None);
        assert_eq!(result.known_expiry_count, 0);
        assert!(!result.expiry_details_complete);
    }
}
