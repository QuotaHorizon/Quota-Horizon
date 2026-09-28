use super::*;
use capacity_domain::{
    activity_quota::ActivityQuotaSample,
    pace::{PACE_HISTORY_HOURS, PaceCurrent, PaceEstimate, PaceSample, estimate_pace},
};
const MAX_PACE_ROWS: u32 = 25_000;

impl CapacityStore {
    /// Exact scope, original captures. Wider display-history families must not
    /// silently become calibrated or forecast inputs.
    pub fn pace_estimate(
        &self,
        fp: &AccountFingerprint,
        env: &str,
        current: &PaceCurrent,
        now: &UtcTimestamp,
    ) -> Result<PaceEstimate, StoreError> {
        let (samples, truncated) = self.pace_samples(fp, env, current, now)?;
        let mut estimate = estimate_pace(&samples, current, now, truncated);
        if self.has_pace_outcomes(fp, env, &current.limit_id)? {
            // Prospective checks have started, but no validated release
            // thresholds exist. Never promote to provisional/validated here.
            estimate.backtest_status = "insufficient_samples";
        }
        Ok(estimate)
    }

    pub(super) fn pace_samples(
        &self,
        fp: &AccountFingerprint,
        env: &str,
        current: &PaceCurrent,
        now: &UtcTimestamp,
    ) -> Result<(Vec<PaceSample>, bool), StoreError> {
        if !bounded_metadata(env, 256) || !bounded_metadata(&current.limit_id, 128) {
            return Err(StoreError::InvalidMetadata("pace_scope"));
        }
        let mut statement = self.connection.prepare(
            "SELECT s.snapshot_id,s.captured_at,w.window_minutes,w.remaining_basis_points,
                    w.resets_at,s.availability,c.compatibility,x.account_plan_type
             FROM quota_snapshots s
             JOIN quota_windows w ON w.snapshot_id=s.snapshot_id
             JOIN compatibility_observations c ON c.observation_id=s.compatibility_observation_id
                 AND c.environment_id=s.environment_id
             LEFT JOIN quota_snapshot_contexts x ON x.snapshot_id=s.snapshot_id
             WHERE s.environment_id=?1 AND s.account_fingerprint=?2 AND w.limit_id=?3
                 AND julianday(s.captured_at)>=julianday(?4)-?5/24.0
                 AND julianday(s.captured_at)<=julianday(?4)
             ORDER BY julianday(s.captured_at) DESC,s.snapshot_id LIMIT ?6",
        )?;
        let mut rows = statement.query(params![
            env,
            fp.as_str(),
            current.limit_id,
            now.as_str(),
            PACE_HISTORY_HOURS,
            MAX_PACE_ROWS + 1
        ])?;
        let mut samples = vec![];
        while let Some(row) = rows.next()? {
            let minutes: Option<i64> = row.get(2)?;
            let reset: Option<String> = row.get(4)?;
            samples.push(PaceSample {
                quota: ActivityQuotaSample {
                    snapshot_id: row.get(0)?,
                    captured_at: UtcTimestamp::parse(row.get::<_, String>(1)?)?,
                    limit_id: current.limit_id.clone(),
                    window_minutes: minutes
                        .map(|n| n.try_into().map_err(|_| StoreError::NumericOverflow))
                        .transpose()?,
                    remaining_percent: row.get::<_, i64>(3)? as f64 / 100.0,
                    resets_at: reset.map(UtcTimestamp::parse).transpose()?,
                    availability: availability_from_str(&row.get::<_, String>(5)?)?,
                    freshness: Freshness::Live,
                    compatibility: compatibility_from_str(&row.get::<_, String>(6)?)?,
                },
                account_plan_type: row.get(7)?,
            });
        }
        let truncated = samples.len() > MAX_PACE_ROWS as usize;
        samples.truncate(MAX_PACE_ROWS as usize);
        Ok((samples, truncated))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{FINGERPRINT, FINGERPRINT_2, observation, snapshot, stable_binding};
    fn at(h: u32, m: u32) -> UtcTimestamp {
        UtcTimestamp::parse(format!("2026-08-30T{h:02}:{m:02}:00Z")).unwrap()
    }
    fn fixture() -> (
        CapacityStore,
        AccountFingerprint,
        String,
        PaceCurrent,
        UtcTimestamp,
    ) {
        let mut store = CapacityStore::open_in_memory().unwrap();
        let fp = AccountFingerprint::parse(FINGERPRINT).unwrap();
        let now = at(12, 0);
        let mut last = None;
        for step in 0..=24 {
            let time = at(6 + step / 4, step % 4 * 15);
            let mut s = snapshot(time.as_str());
            s.quota.windows[0].window_minutes = Some(10080);
            s.quota.windows[0].remaining_percent = 50.0 - step as f64 / 4.0;
            s.quota.windows[0].used_percent = 100.0 - s.quota.windows[0].remaining_percent;
            store
                .record_snapshot(&stable_binding(), &s, &observation(time.as_str()))
                .unwrap();
            last = Some(s);
        }
        let last = last.unwrap();
        let w = &last.quota.windows[0];
        let current = PaceCurrent {
            limit_id: w.limit_id.clone(),
            window_minutes: w.window_minutes,
            remaining_percent: w.remaining_percent,
            resets_at: w.resets_at.clone(),
            observed_at: now.clone(),
            account_plan_type: last.account.as_ref().and_then(|a| a.plan_type.clone()),
            availability: last.data_status.availability,
            freshness: Freshness::Live,
            compatibility: last.data_status.compatibility,
        };
        (store, fp, last.environment.environment_id, current, now)
    }
    #[test]
    fn captures_context_and_estimates_only_exact_account_environment() {
        let (store, fp, env, current, now) = fixture();
        let changes_before = store.connection.total_changes();
        let value = store.pace_estimate(&fp, &env, &current, &now).unwrap();
        assert_eq!(store.connection.total_changes(), changes_before);
        assert_eq!(value.state, "pace_only");
        assert_eq!(value.balance_at_horizon.lower, Some(38.0));
        assert_eq!(value.input_snapshot_ids.len(), 25);
        assert_eq!(
            table_count(&store.connection, "quota_snapshot_contexts").unwrap(),
            25
        );
        let other = AccountFingerprint::parse(FINGERPRINT_2).unwrap();
        for (account, scope) in [(&other, env.as_str()), (&fp, "another-reader")] {
            let value = store.pace_estimate(account, scope, &current, &now).unwrap();
            assert_eq!(value.state, "abstained");
            assert!(value.input_snapshot_ids.is_empty());
        }
    }
    #[test]
    fn failed_context_insert_rolls_back_the_entire_new_capture() {
        let (mut store, _, _, _, _) = fixture();
        store.connection.execute_batch("CREATE TEMP TRIGGER reject_context BEFORE INSERT ON quota_snapshot_contexts BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(
            store
                .record_snapshot(
                    &stable_binding(),
                    &snapshot("2026-08-30T13:00:00Z"),
                    &observation("2026-08-30T13:00:00Z"),
                )
                .is_err()
        );
        for table in [
            "quota_snapshots",
            "quota_windows",
            "quota_snapshot_contexts",
            "compatibility_observations",
        ] {
            assert_eq!(
                table_count(&store.connection, table).unwrap(),
                25,
                "{table}"
            );
        }
    }
    #[test]
    fn missing_or_changed_plan_context_does_not_borrow_the_current_type() {
        let (store, fp, env, current, now) = fixture();
        store
            .connection
            .execute("DELETE FROM quota_snapshot_contexts", [])
            .unwrap();
        assert_eq!(
            store
                .pace_estimate(&fp, &env, &current, &now)
                .unwrap()
                .reason_code,
            "plan_context_missing"
        );
        let (store, fp, env, mut current, now) = fixture();
        current.account_plan_type = Some("plus".into());
        assert_eq!(
            store
                .pace_estimate(&fp, &env, &current, &now)
                .unwrap()
                .reason_code,
            "plan_type_changed"
        );
    }
    #[test]
    fn v8_migration_is_atomic_preserves_old_snapshots_and_does_not_backfill() {
        for fail in [false, true] {
            let (mut store, fp, env, current, now) = fixture();
            store.connection.execute_batch("DROP TABLE pace_trial_outcomes; DROP TABLE pace_trials; DROP TABLE quota_snapshot_contexts; DELETE FROM schema_migrations WHERE version>=8; PRAGMA user_version=7;").unwrap();
            if fail {
                store.connection.execute_batch("CREATE TEMP TRIGGER reject_v8 BEFORE INSERT ON schema_migrations WHEN NEW.version=8 BEGIN SELECT RAISE(ABORT,'test v8 failure'); END;").unwrap();
            }
            assert_eq!(store.migrate().is_err(), fail);
            assert_eq!(
                store.schema_version().unwrap(),
                if fail { 7 } else { STORE_SCHEMA_VERSION }
            );
            assert_eq!(
                table_count(&store.connection, "quota_snapshots").unwrap(),
                25
            );
            if fail {
                let tables: i64 = store
                    .connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_schema WHERE name='quota_snapshot_contexts'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(tables, 0);
            } else {
                assert_eq!(
                    table_count(&store.connection, "quota_snapshot_contexts").unwrap(),
                    0
                );
                assert_eq!(
                    store
                        .pace_estimate(&fp, &env, &current, &now)
                        .unwrap()
                        .reason_code,
                    "plan_context_missing"
                );
                store.verify_schema().unwrap();
            }
        }
    }
    #[test]
    fn snapshot_retention_and_delete_cascade_context_rows() {
        let (mut store, _, _, _, _) = fixture();
        store.connection.execute("DELETE FROM quota_snapshots WHERE snapshot_id=(SELECT snapshot_id FROM quota_snapshots LIMIT 1)",[]).unwrap();
        assert_eq!(
            table_count(&store.connection, "quota_snapshot_contexts").unwrap(),
            24
        );
        assert_eq!(
            store
                .delete_all_database_data()
                .unwrap()
                .quota_snapshot_contexts,
            24
        );
        assert_eq!(
            table_count(&store.connection, "quota_snapshot_contexts").unwrap(),
            0
        );
    }
}
