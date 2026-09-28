use super::*;
use capacity_domain::{
    pace::{PACE_ALGORITHM_VERSION, PaceCurrent, normalize_plan_type},
    pace_evidence::{PaceEvidenceAccumulator, PaceEvidenceSummary},
    pace_trial::{PaceTrialOutcome, PaceTrialSeed, PaceTrialView, assess, epoch, trial_view},
};

impl CapacityStore {
    /// One read transaction keeps the page and its scope-wide summary aligned.
    /// Bounded streaming avoids loading hundreds of full frozen inputs at once.
    pub fn pace_trial_report(
        &self,
        fp: &AccountFingerprint,
        env: &str,
        offset: u32,
        now: &UtcTimestamp,
    ) -> Result<(Vec<PaceTrialView>, PaceEvidenceSummary), StoreError> {
        let tx = self.connection.unchecked_transaction()?;
        let page = self.pace_trials(fp, env, offset)?;
        let summary = self.pace_evidence(fp, env, now, 500, 32 * 1024 * 1024)?;
        tx.commit()?;
        Ok((page, summary))
    }
    fn pace_evidence(
        &self,
        fp: &AccountFingerprint,
        env: &str,
        now: &UtcTimestamp,
        record_limit: usize,
        byte_limit: usize,
    ) -> Result<PaceEvidenceSummary, StoreError> {
        let mut query = self.connection.prepare("SELECT t.inputs_json,o.outcome_json FROM pace_trials t LEFT JOIN pace_trial_outcomes o ON o.trial_id=t.trial_id WHERE t.environment_id=?1 AND t.account_fingerprint=?2 ORDER BY julianday(t.issued_at) DESC,t.trial_id LIMIT ?3")?;
        let mut rows = query.query(params![env, fp.as_str(), (record_limit + 1) as i64])?;
        let mut evidence = PaceEvidenceAccumulator::new(now);
        let mut bytes = 0usize;
        let mut count = 0usize;
        let mut limited = false;
        while let Some(row) = rows.next()? {
            if count == record_limit {
                limited = true;
                break;
            }
            let json: String = row.get(0)?;
            let outcome: Option<String> = row.get(1)?;
            bytes = bytes
                .saturating_add(json.len())
                .saturating_add(outcome.as_ref().map_or(0, String::len));
            if bytes > byte_limit {
                limited = true;
                break;
            }
            let seed = PaceTrialSeed::decode(&json)
                .ok_or(StoreError::InvalidMetadata("pace_trial_encoding"))?;
            let outcome = outcome
                .map(|value| {
                    PaceTrialOutcome::decode(&value)
                        .ok_or(StoreError::InvalidMetadata("pace_outcome_encoding"))
                })
                .transpose()?;
            evidence.add(&seed, outcome.as_ref().map(|v| &v.summary));
            count += 1;
        }
        Ok(evidence.finish(limited))
    }

    pub(super) fn has_pace_outcomes(
        &self,
        fp: &AccountFingerprint,
        env: &str,
        limit: &str,
    ) -> Result<bool, StoreError> {
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM pace_trials t JOIN pace_trial_outcomes o ON o.trial_id=t.trial_id WHERE t.environment_id=?1 AND t.account_fingerprint=?2 AND t.limit_id=?3 AND t.algorithm_version=?4)",params![env,fp.as_str(),limit,PACE_ALGORITHM_VERSION],|r|r.get(0))?)
    }
    /// Called only after an actual live capture has been persisted, not by UI
    /// reads. Frozen inputs and terminal outcomes are append-only.
    pub fn maintain_pace_trials(
        &mut self,
        fp: &AccountFingerprint,
        snapshot: &StatusSnapshot,
        now: &UtcTimestamp,
    ) -> Result<(), StoreError> {
        snapshot.validate()?;
        let env = &snapshot.environment.environment_id;
        if snapshot.account.as_ref().map(|a| a.binding_status) != Some(AccountBindingStatus::Stable)
        {
            return Err(StoreError::BindingStatusMismatch);
        }
        self.settle_pace_trials(fp, env, now)?;
        if snapshot.data_status.freshness != Freshness::Live {
            return Ok(());
        }
        let plan = snapshot.account.as_ref().and_then(|a| a.plan_type.clone());
        let pro = normalize_plan_type(plan.as_deref())
            .is_some_and(|p| matches!(p.as_str(), "pro" | "prolite"));
        let windows: Vec<_> = snapshot
            .quota
            .windows
            .iter()
            .filter(|w| {
                (w.limit_id == "codex" || w.limit_id.starts_with("codex:"))
                    && !(pro && w.window_minutes.is_some_and(|m| m <= 1440))
            })
            .collect();
        if windows.len() > 8 {
            return Ok(());
        }
        for window in windows {
            let already: bool = self.connection.query_row("SELECT EXISTS(SELECT 1 FROM pace_trials WHERE environment_id=?1 AND account_fingerprint=?2 AND limit_id=?3 AND algorithm_version=?4 AND julianday(forecast_horizon)>julianday(?5))", params![env,fp.as_str(),window.limit_id,PACE_ALGORITHM_VERSION,now.as_str()],|r|r.get(0))?;
            if already {
                continue;
            }
            let current = PaceCurrent {
                limit_id: window.limit_id.clone(),
                window_minutes: window.window_minutes,
                remaining_percent: window.remaining_percent,
                resets_at: window.resets_at.clone(),
                observed_at: snapshot.captured_at.clone(),
                account_plan_type: plan.clone(),
                availability: snapshot.data_status.availability,
                freshness: snapshot.data_status.freshness,
                compatibility: snapshot.data_status.compatibility,
            };
            let (samples, truncated) = self.pace_samples(fp, env, &current, now)?;
            let Some(seed) = PaceTrialSeed::freeze(&samples, &current, now, truncated) else {
                continue;
            };
            let json = seed
                .encode()
                .ok_or(StoreError::InvalidMetadata("pace_trial_encoding"))?;
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Recheck under the writer lock: simultaneous app processes may
            // have prepared the same candidate before either started writing.
            tx.execute("INSERT INTO pace_trials(trial_id,environment_id,account_fingerprint,limit_id,algorithm_version,issued_at,forecast_horizon,inputs_json) SELECT ?1,?2,?3,?4,?5,?6,?7,?8 WHERE NOT EXISTS(SELECT 1 FROM pace_trials WHERE environment_id=?2 AND account_fingerprint=?3 AND limit_id=?4 AND algorithm_version=?5 AND julianday(forecast_horizon)>julianday(?6))", params![Uuid::new_v4().to_string(),env,fp.as_str(),current.limit_id,seed.algorithm_version,now.as_str(),seed.horizon.as_str(),json])?;
            tx.commit()?;
        }
        let days = read_monitor_settings(&self.connection)?.history_retention_days;
        self.connection.execute("DELETE FROM pace_trials WHERE environment_id=?1 AND account_fingerprint=?2 AND julianday(issued_at)<julianday(?3)-?4",params![env,fp.as_str(),now.as_str(),days])?;
        Ok(())
    }

    fn settle_pace_trials(
        &mut self,
        fp: &AccountFingerprint,
        env: &str,
        now: &UtcTimestamp,
    ) -> Result<(), StoreError> {
        let rows: Vec<(String, String)> = {
            let mut query = self.connection.prepare("SELECT t.trial_id,t.inputs_json FROM pace_trials t LEFT JOIN pace_trial_outcomes o ON o.trial_id=t.trial_id WHERE t.environment_id=?1 AND t.account_fingerprint=?2 AND o.trial_id IS NULL AND julianday(t.forecast_horizon)<=julianday(?3) ORDER BY julianday(t.issued_at) LIMIT 32")?;
            query
                .query_map(params![env, fp.as_str(), now.as_str()], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<Result<_, _>>()?
        };
        for (id, json) in rows {
            let seed = PaceTrialSeed::decode(&json)
                .ok_or(StoreError::InvalidMetadata("pace_trial_encoding"))?;
            let (future, truncated) =
                self.pace_samples(fp, env, &seed.current, &seed.query_until(now))?;
            if let Some(outcome) = assess(&seed, &future, now, truncated) {
                let encoded = outcome
                    .encode()
                    .ok_or(StoreError::InvalidMetadata("pace_outcome_encoding"))?;
                self.connection.execute("INSERT INTO pace_trial_outcomes(trial_id,assessed_at,outcome_json) SELECT ?1,?2,?3 WHERE EXISTS(SELECT 1 FROM pace_trials WHERE trial_id=?1 AND environment_id=?4 AND account_fingerprint=?5) ON CONFLICT(trial_id) DO NOTHING",params![id,now.as_str(),encoded,env,fp.as_str()])?;
            }
        }
        Ok(())
    }

    /// Read-only scoped page. Frozen raw inputs remain native-side.
    pub fn pace_trials(
        &self,
        fp: &AccountFingerprint,
        env: &str,
        offset: u32,
    ) -> Result<Vec<PaceTrialView>, StoreError> {
        if !bounded_metadata(env, 256) || offset > 100_000 {
            return Err(StoreError::InvalidMetadata("pace_trial_page"));
        }
        let mut query = self.connection.prepare("SELECT t.trial_id,t.inputs_json,o.outcome_json FROM pace_trials t LEFT JOIN pace_trial_outcomes o ON o.trial_id=t.trial_id WHERE t.environment_id=?1 AND t.account_fingerprint=?2 ORDER BY julianday(t.issued_at) DESC,t.trial_id LIMIT 21 OFFSET ?3")?;
        let rows = query.query_map(params![env, fp.as_str(), offset], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (id, json, outcome) = row?;
            let seed = PaceTrialSeed::decode(&json)
                .ok_or(StoreError::InvalidMetadata("pace_trial_encoding"))?;
            let outcome = outcome
                .map(|s| {
                    PaceTrialOutcome::decode(&s)
                        .ok_or(StoreError::InvalidMetadata("pace_outcome_encoding"))
                })
                .transpose()?;
            if epoch(&seed.horizon) <= epoch(&seed.issued_at) {
                return Err(StoreError::InvalidMetadata("pace_trial_time"));
            }
            Ok(trial_view(id, seed, outcome))
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{FINGERPRINT, FINGERPRINT_2, observation, snapshot, stable_binding};
    fn at(step: u32) -> UtcTimestamp {
        UtcTimestamp::parse(format!(
            "2026-08-30T{:02}:{:02}:00Z",
            step / 4,
            step % 4 * 15
        ))
        .unwrap()
    }
    fn capture(store: &mut CapacityStore, step: u32, maintain: bool) -> StatusSnapshot {
        let time = at(step);
        let mut s = snapshot(time.as_str());
        s.account.as_mut().unwrap().plan_type = Some("plus".into());
        s.quota.windows[0].remaining_percent = 80.0 - step as f64 / 2.0;
        s.quota.windows[0].used_percent = 100.0 - s.quota.windows[0].remaining_percent;
        store
            .record_snapshot(&stable_binding(), &s, &observation(time.as_str()))
            .unwrap();
        if maintain {
            store.maintain_pace_trials(&fp(), &s, &time).unwrap();
        }
        s
    }
    fn fp() -> AccountFingerprint {
        AccountFingerprint::parse(FINGERPRINT).unwrap()
    }
    fn fixture() -> (CapacityStore, StatusSnapshot) {
        let mut store = CapacityStore::open_in_memory().unwrap();
        for step in 0..24 {
            capture(&mut store, step, false);
        }
        let s = capture(&mut store, 24, true);
        (store, s)
    }
    #[test]
    fn automatic_forward_check_is_immutable_scoped_and_nonoverlapping() {
        let (mut store, s) = fixture();
        let env = &s.environment.environment_id;
        let first = store.pace_trials(&fp(), env, 0).unwrap().remove(0);
        assert!(
            !store
                .has_pace_outcomes(&fp(), env, "codex:primary")
                .unwrap()
        );
        assert!(first.outcome.is_none());
        assert_eq!(first.balance_range, [56.0, 56.0]);
        let frozen: String = store
            .connection
            .query_row(
                "SELECT inputs_json FROM pace_trials WHERE trial_id=?1",
                [&first.trial_id],
                |r| r.get(0),
            )
            .unwrap();
        store
            .maintain_pace_trials(&fp(), &s, &s.captured_at)
            .unwrap();
        assert_eq!(table_count(&store.connection, "pace_trials").unwrap(), 1);
        assert!(
            store
                .connection
                .execute("UPDATE pace_trials SET inputs_json='{}'", [])
                .is_err()
        );
        // Removing pre-issue raw history does not rewrite the frozen inputs.
        store
            .prune_history_before(env, &fp(), &s.captured_at)
            .unwrap();
        assert_eq!(table_count(&store.connection, "pace_trials").unwrap(), 1);
        for step in 25..=48 {
            capture(&mut store, step, true);
        }
        let rows = store.pace_trials(&fp(), env, 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            store
                .has_pace_outcomes(&fp(), env, "codex:primary")
                .unwrap()
        );
        assert_eq!(
            rows.iter()
                .find(|v| v.trial_id == first.trial_id)
                .unwrap()
                .outcome
                .as_ref()
                .unwrap()
                .classification,
            "within_band"
        );
        let again: String = store
            .connection
            .query_row(
                "SELECT inputs_json FROM pace_trials WHERE trial_id=?1",
                [&first.trial_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(frozen, again);
        assert!(
            store
                .connection
                .execute("UPDATE pace_trial_outcomes SET outcome_json='{}'", [])
                .is_err()
        );
        let terminal: String = store
            .connection
            .query_row(
                "SELECT outcome_json FROM pace_trial_outcomes WHERE trial_id=?1",
                [&first.trial_id],
                |r| r.get(0),
            )
            .unwrap();
        capture(&mut store, 49, true);
        let after: String = store
            .connection
            .query_row(
                "SELECT outcome_json FROM pace_trial_outcomes WHERE trial_id=?1",
                [&first.trial_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(terminal, after);
        let changes = store.connection.total_changes();
        store.pace_trials(&fp(), env, 0).unwrap();
        assert_eq!(changes, store.connection.total_changes());
        assert!(
            store
                .pace_trials(&AccountFingerprint::parse(FINGERPRINT_2).unwrap(), env, 0)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .pace_trials(&fp(), "another-reader", 0)
                .unwrap()
                .is_empty()
        );
        assert!(store.pace_trials(&fp(), env, 100001).is_err());
    }
    #[test]
    fn reopen_preserves_pending_input_and_missing_future_is_unscorable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trials.sqlite3");
        let mut store = CapacityStore::open(&path).unwrap();
        for step in 0..=24 {
            capture(&mut store, step, true);
        }
        let env = snapshot(at(24).as_str()).environment.environment_id;
        let id = store.pace_trials(&fp(), &env, 0).unwrap()[0]
            .trial_id
            .clone();
        drop(store);
        let mut store = CapacityStore::open(&path).unwrap();
        capture(&mut store, 52, true);
        let old = store
            .pace_trials(&fp(), &env, 0)
            .unwrap()
            .into_iter()
            .find(|v| v.trial_id == id)
            .unwrap();
        assert_eq!(old.outcome.unwrap().reason_code, "target_samples_missing");
    }
    #[test]
    fn retention_and_delete_all_cover_trials_and_outcomes() {
        let (mut store, s) = fixture();
        for step in 25..=48 {
            capture(&mut store, step, true);
        }
        store
            .prune_history_before(&s.environment.environment_id, &fp(), &at(25))
            .unwrap();
        assert_eq!(table_count(&store.connection, "pace_trials").unwrap(), 1);
        assert_eq!(
            table_count(&store.connection, "pace_trial_outcomes").unwrap(),
            0
        );
        let report = store.delete_all_database_data().unwrap();
        assert_eq!(report.pace_trials, 1);
        assert_eq!(table_count(&store.connection, "pace_trials").unwrap(), 0);
        assert_eq!(
            table_count(&store.connection, "pace_trial_outcomes").unwrap(),
            0
        );
    }
    #[test]
    fn key_rotation_cascades_scope_without_changing_frozen_payload() {
        let (store, s) = fixture();
        let before: String = store
            .connection
            .query_row("SELECT inputs_json FROM pace_trials", [], |r| r.get(0))
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE account_bindings SET account_fingerprint=?1 WHERE account_fingerprint=?2",
                params![FINGERPRINT_2, FINGERPRINT],
            )
            .unwrap();
        assert!(
            store
                .pace_trials(&fp(), &s.environment.environment_id, 0)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .pace_trials(
                    &AccountFingerprint::parse(FINGERPRINT_2).unwrap(),
                    &s.environment.environment_id,
                    0
                )
                .unwrap()
                .len(),
            1
        );
        let after: String = store
            .connection
            .query_row("SELECT inputs_json FROM pace_trials", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, after);
    }
    #[test]
    fn v9_migration_preserves_context_and_rolls_back_both_tables() {
        for fail in [true, false] {
            let (mut store, _) = fixture();
            store.connection.execute_batch("DROP TABLE pace_trial_outcomes; DROP TABLE pace_trials; DELETE FROM schema_migrations WHERE version=9; PRAGMA user_version=8;").unwrap();
            if fail {
                store.connection.execute_batch("CREATE TEMP TRIGGER reject_v9 BEFORE INSERT ON schema_migrations WHEN NEW.version=9 BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
            }
            assert_eq!(store.migrate().is_err(), fail);
            assert_eq!(
                store.schema_version().unwrap(),
                if fail { 8 } else { STORE_SCHEMA_VERSION }
            );
            assert_eq!(
                table_count(&store.connection, "quota_snapshot_contexts").unwrap(),
                25
            );
            if fail {
                store
                    .verify_schema_version(8, &SCHEMA_COLUMNS[..SCHEMA_V8_TABLE_COUNT])
                    .unwrap();
            } else {
                store.verify_schema().unwrap();
                assert_eq!(table_count(&store.connection, "pace_trials").unwrap(), 0);
            }
        }
    }
    #[test]
    fn recorder_failure_preserves_capture_and_retry_creates_one_trial() {
        let mut store = CapacityStore::open_in_memory().unwrap();
        for step in 0..=23 {
            capture(&mut store, step, false);
        }
        let snapshot = capture(&mut store, 24, false);
        store.connection.execute_batch("CREATE TEMP TRIGGER reject_trial BEFORE INSERT ON pace_trials BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(
            store
                .maintain_pace_trials(&fp(), &snapshot, &snapshot.captured_at)
                .is_err()
        );
        assert_eq!(
            table_count(&store.connection, "quota_snapshots").unwrap(),
            25
        );
        assert_eq!(table_count(&store.connection, "pace_trials").unwrap(), 0);
        store
            .connection
            .execute_batch("DROP TRIGGER reject_trial")
            .unwrap();
        store
            .maintain_pace_trials(&fp(), &snapshot, &snapshot.captured_at)
            .unwrap();
        store
            .maintain_pace_trials(&fp(), &snapshot, &snapshot.captured_at)
            .unwrap();
        assert_eq!(table_count(&store.connection, "pace_trials").unwrap(), 1);
    }
    #[test]
    fn evidence_is_page_independent_read_only_scoped_and_explicitly_bounded() {
        let (mut store, s) = fixture();
        for step in 25..=48 {
            capture(&mut store, step, true);
        }
        let env = &s.environment.environment_id;
        let before = store.connection.total_changes();
        let (page, summary) = store.pace_trial_report(&fp(), env, 0, &at(48)).unwrap();
        let (empty, later_summary) = store.pace_trial_report(&fp(), env, 20, &at(48)).unwrap();
        assert_eq!(page.len(), 2);
        assert!(empty.is_empty());
        assert_eq!(summary.counts.total, 2);
        assert_eq!(summary.counts.within, 1);
        assert_eq!(summary.counts.pending, 1);
        assert_eq!(later_summary.counts.total, summary.counts.total);
        assert!(!summary.limited);
        assert_eq!(
            summary.groups[0].training_maximum_gap_seconds,
            Some([900, 900])
        );
        assert_eq!(store.connection.total_changes(), before);
        for (fp, env) in [
            (fp(), "other-reader"),
            (
                AccountFingerprint::parse(FINGERPRINT_2).unwrap(),
                env.as_str(),
            ),
        ] {
            let (_, report) = store.pace_trial_report(&fp, env, 0, &at(48)).unwrap();
            assert_eq!(report.counts.total, 0);
        }
        let limited = store
            .pace_evidence(&fp(), env, &at(48), 1, usize::MAX)
            .unwrap();
        assert!(limited.limited);
        assert_eq!(limited.counts.total, 1);
        assert_eq!(limited.counts.pending, 1);
        let limited = store.pace_evidence(&fp(), env, &at(48), 500, 1).unwrap();
        assert!(limited.limited);
        assert_eq!(limited.counts.total, 0);
    }
    #[test]
    fn evidence_does_not_silently_drop_corrupt_records() {
        let (store, s) = fixture();
        store
            .connection
            .execute_batch("DROP TRIGGER pace_trial_immutable;")
            .unwrap();
        store
            .connection
            .execute("UPDATE pace_trials SET inputs_json='{}'", [])
            .unwrap();
        assert!(
            store
                .pace_trial_report(&fp(), &s.environment.environment_id, 0, &at(48))
                .is_err()
        );
    }
    #[test]
    fn old_reader_trials_are_discoverable_without_plans_and_remain_read_only() {
        use capacity_domain::planning_archive::{
            PlanningArchiveQuery as Query, PlanningArchiveRef as Ref,
            PlanningArchiveResult as ResultView,
        };
        let (mut store, s) = fixture();
        for step in 25..=48 {
            capture(&mut store, step, true);
        }
        let changes = store.connection.total_changes();
        let ResultView::List { entries, .. } = store
            .planning_archive(&fp(), "new-reader", &Query::List { offset: 0 })
            .unwrap()
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].plan_revisions, 0);
        assert_eq!(entries[0].record_count, 0);
        assert_eq!(entries[0].trial_count, 2);
        assert!(matches!(entries[0].source, Ref::Trial(_)));
        let query = Query::Pace {
            source: entries[0].source.clone(),
            offset: 0,
        };
        let ResultView::Pace { trials, has_more } = store
            .planning_archive(&fp(), "new-reader", &query)
            .unwrap()
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(trials.len(), 2);
        assert!(!has_more);
        assert_eq!(trials.iter().filter(|t| t.outcome.is_some()).count(), 1);
        assert_eq!(store.connection.total_changes(), changes);
        assert!(
            store
                .planning_archive(&fp(), &s.environment.environment_id, &query)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .planning_archive(
                    &AccountFingerprint::parse(FINGERPRINT_2).unwrap(),
                    "new-reader",
                    &query
                )
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .planning_archive(
                    &fp(),
                    "new-reader",
                    &Query::Pace {
                        source: Ref::Trial("bad-id".into()),
                        offset: 0
                    }
                )
                .is_err()
        );
        assert!(
            store
                .planning_archive(
                    &fp(),
                    "new-reader",
                    &Query::Pace {
                        source: entries[0].source.clone(),
                        offset: 100001
                    }
                )
                .is_err()
        );
        let ResultView::Pace { trials, .. } = store
            .planning_archive(
                &fp(),
                "new-reader",
                &Query::Pace {
                    source: entries[0].source.clone(),
                    offset: 20,
                },
            )
            .unwrap()
            .unwrap()
        else {
            panic!()
        };
        assert!(trials.is_empty());
        store.delete_all_database_data().unwrap();
        assert!(
            store
                .planning_archive(&fp(), "new-reader", &query)
                .unwrap()
                .is_none()
        );
    }
}
