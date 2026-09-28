use super::*;

/// Each saved account gets its own observation, with one schedule revision and
/// clock for the whole batch. Never consult or change the active runtime here.
pub async fn work_plans_for_saved_accounts(
    app: AppHandle,
    observations: Vec<(String, Option<DesktopWorkPlanQuotaObservation>)>,
) -> Result<Vec<(String, DesktopWorkPlanEnvelope)>, DesktopIssue> {
    let state = app.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    let schedule = state.work_schedule().await;
    Ok(plans_for_observations(
        schedule,
        observations,
        &SystemClock.now(),
    ))
}

fn plans_for_observations(
    schedule: DesktopWorkScheduleEnvelope,
    observations: Vec<(String, Option<DesktopWorkPlanQuotaObservation>)>,
    now: &UtcTimestamp,
) -> Vec<(String, DesktopWorkPlanEnvelope)> {
    // An empty status is intentional: an account without usable quota must
    // never borrow the currently signed-in account's window or reset time.
    let empty = envelope_from_discovery(None, None, None);
    observations
        .into_iter()
        .map(|(id, observation)| {
            (
                id,
                work_plan_envelope(schedule.clone(), &empty, now, observation.as_ref()),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule() -> DesktopWorkScheduleEnvelope {
        DesktopWorkScheduleEnvelope {
            schema_version: "1.0",
            status: DesktopWorkScheduleStatus::Available,
            reason_code: "work_schedule_available",
            schedule: Some(DesktopWorkScheduleView {
                revision: 4,
                enabled: true,
                off_periods: vec![],
                updated_at: "2026-01-01T00:00:00Z".into(),
            }),
        }
    }

    fn observation(remaining: f64, reset: &str) -> Option<DesktopWorkPlanQuotaObservation> {
        DesktopWorkPlanQuotaObservation::from_managed_account_weekly(
            "2026-01-01T11:59:00Z".into(),
            10080,
            remaining,
            reset.into(),
        )
    }

    #[test]
    fn two_accounts_use_independent_quota_and_reset_with_one_schedule() {
        let plans = plans_for_observations(
            schedule(),
            vec![
                ("a".into(), observation(28.0, "2026-01-03T12:00:00Z")),
                ("b".into(), observation(81.0, "2026-01-07T12:00:00Z")),
            ],
            &UtcTimestamp::parse("2026-01-01T12:00:00Z").unwrap(),
        );
        let first = plans[0].1.baseline.as_ref().unwrap();
        let second = plans[1].1.baseline.as_ref().unwrap();
        assert_eq!(first.comparison.actual_remaining_percent, 28.0);
        assert_eq!(second.comparison.actual_remaining_percent, 81.0);
        assert_ne!(
            first.comparison.expected_remaining_percent,
            second.comparison.expected_remaining_percent
        );
        assert_eq!(plans[0].1.schedule.as_ref().unwrap().revision, 4);
        assert_eq!(plans[1].1.schedule.as_ref().unwrap().revision, 4);
        assert_eq!(first.source, "managed_account_cache");
    }

    #[test]
    fn missing_or_expired_account_never_borrows_another_accounts_plan() {
        let plans = plans_for_observations(
            schedule(),
            vec![
                ("valid".into(), observation(28.0, "2026-01-03T12:00:00Z")),
                ("missing".into(), None),
                ("expired".into(), observation(99.0, "2026-01-01T12:00:00Z")),
            ],
            &UtcTimestamp::parse("2026-01-01T12:00:00Z").unwrap(),
        );
        assert!(plans[0].1.baseline.is_some());
        assert!(plans[1].1.baseline.is_none());
        assert!(plans[2].1.baseline.is_none());
    }
}
