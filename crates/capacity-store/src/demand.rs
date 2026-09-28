use super::*;
use capacity_domain::demand::{CapacityDemandInput, DemandKind};

pub use capacity_domain::planning_archive::CapacityWorkPlan;

pub struct CapacityWorkPlanUpdate {
    pub expected_revision: u32,
    pub enabled: bool,
    pub demand: CapacityDemandInput,
}

#[derive(Debug, PartialEq)]
pub enum CapacityWorkPlanUpdateOutcome {
    Updated(CapacityWorkPlan),
    RevisionConflict(Option<CapacityWorkPlan>),
}

impl CapacityStore {
    pub fn has_capacity_work_plan_in_other_environment(
        &self,
        fingerprint: &AccountFingerprint,
        environment: &str,
    ) -> Result<bool, StoreError> {
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM capacity_work_plans WHERE account_fingerprint=?1 AND environment_id<>?2)", params![fingerprint.as_str(), environment], |row| row.get(0))?)
    }
    pub fn capacity_work_plans(
        &self,
        fingerprint: &AccountFingerprint,
        environment: &str,
    ) -> Result<Vec<CapacityWorkPlan>, StoreError> {
        read_plans(&self.connection, fingerprint, environment)
    }

    /// A single account/environment has one intent lineage. Every edit, pause,
    /// and restart appends a new immutable plan+demand revision atomically.
    pub fn update_capacity_work_plan(
        &mut self,
        fingerprint: &AccountFingerprint,
        environment: &str,
        request: &CapacityWorkPlanUpdate,
        now: &UtcTimestamp,
    ) -> Result<CapacityWorkPlanUpdateOutcome, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous = read_plans(&transaction, fingerprint, environment)?
            .into_iter()
            .next();
        if previous.as_ref().map_or(0, |plan| plan.revision) != request.expected_revision {
            return Ok(CapacityWorkPlanUpdateOutcome::RevisionConflict(previous));
        }
        if request.enabled {
            request.demand.validate(now)?;
        } else if previous
            .as_ref()
            .is_none_or(|plan| plan.demand != request.demand)
        {
            // Pausing an expired plan is allowed; smuggling an invalid edited
            // demand through a disabled revision is not.
            return Err(StoreError::InvalidMetadata("capacity_demand"));
        }
        if previous
            .as_ref()
            .is_some_and(|plan| !now.is_not_before(&plan.created_at))
        {
            return Err(StoreError::InvalidMetadata("capacity_plan_clock"));
        }
        let revision = request
            .expected_revision
            .checked_add(1)
            .ok_or(StoreError::NumericOverflow)?;
        let work_plan_id = previous
            .as_ref()
            .map(|plan| plan.work_plan_id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let demand_id = previous
            .as_ref()
            .map(|plan| plan.demand_id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let kind = match request.demand.demand_kind {
            DemandKind::ActiveHours => "active_hours",
            DemandKind::MaintainRecentPace => "maintain_recent_pace",
        };
        transaction.execute("INSERT INTO capacity_demands(environment_id, account_fingerprint, demand_id, revision, horizon_end, demand_kind, planned_codex_active_hours, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![environment, fingerprint.as_str(), demand_id, revision, request.demand.horizon_end.as_str(), kind, request.demand.planned_codex_active_hours, now.as_str()])?;
        transaction.execute("INSERT INTO capacity_work_plans(environment_id, account_fingerprint, work_plan_id, revision, demand_id, demand_revision, enabled, created_at) VALUES (?1,?2,?3,?4,?5,?4,?6,?7)", params![environment, fingerprint.as_str(), work_plan_id, revision, demand_id, request.enabled, now.as_str()])?;
        transaction.commit()?;
        Ok(CapacityWorkPlanUpdateOutcome::Updated(CapacityWorkPlan {
            work_plan_id,
            revision,
            demand_id,
            demand_revision: revision,
            enabled: request.enabled,
            demand: request.demand.clone(),
            created_at: now.clone(),
        }))
    }
}

pub(super) fn read_plans(
    connection: &Connection,
    fingerprint: &AccountFingerprint,
    environment: &str,
) -> Result<Vec<CapacityWorkPlan>, StoreError> {
    if !bounded_metadata(environment, 256) {
        return Err(StoreError::InvalidMetadata("environment_id"));
    }
    let mut statement = connection.prepare("SELECT p.work_plan_id, p.revision, p.demand_id, p.demand_revision, p.enabled, p.created_at, d.horizon_end, d.demand_kind, d.planned_codex_active_hours FROM capacity_work_plans p JOIN capacity_demands d USING(environment_id, account_fingerprint, demand_id) WHERE p.environment_id=?1 AND p.account_fingerprint=?2 AND d.revision=p.demand_revision ORDER BY p.revision DESC LIMIT 5")?;
    let rows = statement.query_map(params![environment, fingerprint.as_str()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, u32>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, u32>(3)?,
            row.get::<_, bool>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, Option<f64>>(8)?,
        ))
    })?;
    rows.map(|row| {
        let (
            work_plan_id,
            revision,
            demand_id,
            demand_revision,
            enabled,
            created_at,
            horizon,
            kind,
            hours,
        ) = row?;
        let demand_kind = match kind.as_str() {
            "active_hours" => DemandKind::ActiveHours,
            "maintain_recent_pace" => DemandKind::MaintainRecentPace,
            _ => return Err(StoreError::CorruptEnum("demand_kind")),
        };
        Ok(CapacityWorkPlan {
            work_plan_id,
            revision,
            demand_id,
            demand_revision,
            enabled,
            created_at: UtcTimestamp::parse(created_at)?,
            demand: CapacityDemandInput {
                horizon_end: UtcTimestamp::parse(horizon)?,
                demand_kind,
                planned_codex_active_hours: hours,
            },
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{
        FINGERPRINT, FINGERPRINT_2, observation, snapshot, stable_binding, v4_connection,
    };
    fn at(value: &str) -> UtcTimestamp {
        UtcTimestamp::parse(value).unwrap()
    }
    fn request(revision: u32) -> CapacityWorkPlanUpdate {
        CapacityWorkPlanUpdate {
            expected_revision: revision,
            enabled: true,
            demand: CapacityDemandInput {
                horizon_end: at("2026-08-31T12:00:00Z"),
                demand_kind: DemandKind::ActiveHours,
                planned_codex_active_hours: Some(4.0),
            },
        }
    }
    fn fixture() -> (CapacityStore, AccountFingerprint, String) {
        let mut store = CapacityStore::open_in_memory().unwrap();
        let snapshot = snapshot("2026-08-30T02:55:53Z");
        store
            .record_snapshot(
                &stable_binding(),
                &snapshot,
                &observation("2026-08-30T02:55:53Z"),
            )
            .unwrap();
        (
            store,
            AccountFingerprint::parse(FINGERPRINT).unwrap(),
            snapshot.environment.environment_id,
        )
    }
    #[test]
    fn revisions_are_atomic_immutable_and_bound_to_account_environment() {
        let (mut store, fp, env) = fixture();
        let now = at("2026-08-30T03:00:00Z");
        let first = store
            .update_capacity_work_plan(&fp, &env, &request(0), &now)
            .unwrap();
        assert!(matches!(first, CapacityWorkPlanUpdateOutcome::Updated(_)));
        let mut edited = request(1);
        edited.demand.planned_codex_active_hours = Some(6.0);
        store
            .update_capacity_work_plan(&fp, &env, &edited, &now)
            .unwrap();
        let rows = store.capacity_work_plans(&fp, &env).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].work_plan_id, rows[1].work_plan_id);
        assert_eq!(rows[0].demand.planned_codex_active_hours, Some(6.0));
        assert_eq!(rows[1].demand.planned_codex_active_hours, Some(4.0));
        assert!(matches!(
            store
                .update_capacity_work_plan(&fp, &env, &request(1), &now)
                .unwrap(),
            CapacityWorkPlanUpdateOutcome::RevisionConflict(Some(_))
        ));
        assert!(
            store
                .capacity_work_plans(&fp, "other-environment")
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .has_capacity_work_plan_in_other_environment(&fp, "other-environment")
                .unwrap()
        );
        let other = AccountFingerprint::parse(FINGERPRINT_2).unwrap();
        // A different stable fingerprint cannot read the existing plan.
        assert!(store.capacity_work_plans(&other, &env).unwrap().is_empty());
        assert!(
            !store
                .has_capacity_work_plan_in_other_environment(&other, &env)
                .unwrap()
        );
        let report = store.delete_all_database_data().unwrap();
        assert_eq!(report.capacity_demands, 2);
        assert_eq!(report.capacity_work_plans, 2);
        assert!(store.capacity_work_plans(&fp, &env).unwrap().is_empty());
    }
    #[test]
    fn pause_after_expiry_is_allowed_but_invalid_or_missing_scope_does_not_write() {
        let (mut store, fp, env) = fixture();
        let now = at("2026-08-30T03:00:00Z");
        assert!(
            store
                .update_capacity_work_plan(&fp, "unknown", &request(0), &now)
                .is_err()
        );
        store
            .update_capacity_work_plan(&fp, &env, &request(0), &now)
            .unwrap();
        let mut paused = request(1);
        paused.enabled = false;
        store
            .update_capacity_work_plan(&fp, &env, &paused, &at("2026-09-01T00:00:00Z"))
            .unwrap();
        assert!(!store.capacity_work_plans(&fp, &env).unwrap()[0].enabled);
        let mut invalid = request(2);
        invalid.demand.planned_codex_active_hours = Some(f64::NAN);
        assert!(
            store
                .update_capacity_work_plan(&fp, &env, &invalid, &now)
                .is_err()
        );
        assert_eq!(
            table_count(&store.connection, "capacity_demands").unwrap(),
            2
        );
    }
    #[test]
    fn v5_migration_preserves_history_and_rolls_back_both_tables_on_failure() {
        let connection = v4_connection();
        connection.execute_batch(MIGRATION_V5).unwrap();
        connection
            .execute(
                "INSERT INTO schema_migrations VALUES(5, '2026-08-30T00:00:00Z')",
                [],
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 5).unwrap();
        let mut store = CapacityStore {
            connection,
            file_backed: false,
        };
        crate::tests::persist_legacy_snapshot(
            &mut store,
            &snapshot("2026-08-30T02:55:53Z"),
            &observation("2026-08-30T02:55:53Z"),
        )
        .unwrap();
        store.connection.execute_batch("CREATE TEMP TRIGGER reject_v6 BEFORE INSERT ON schema_migrations WHEN NEW.version=6 BEGIN SELECT RAISE(ABORT, 'test'); END;").unwrap();
        assert!(store.migrate().is_err());
        assert_eq!(store.schema_version().unwrap(), 5);
        assert_eq!(
            table_count(&store.connection, "quota_snapshots").unwrap(),
            1
        );
        store
            .verify_schema_version(5, &SCHEMA_COLUMNS[..SCHEMA_V5_TABLE_COUNT])
            .unwrap();
        store
            .connection
            .execute_batch("DROP TRIGGER reject_v6")
            .unwrap();
        store.migrate().unwrap();
        store.verify_schema().unwrap();
        assert_eq!(
            table_count(&store.connection, "quota_snapshots").unwrap(),
            1
        );
        assert_eq!(
            table_count(&store.connection, "capacity_demands").unwrap(),
            0
        );
    }

    #[test]
    fn failed_plan_insert_rolls_back_demand_and_history_cascade_keeps_intent() {
        let (mut store, fp, env) = fixture();
        let now = at("2026-08-30T03:00:00Z");
        store.connection.execute_batch("CREATE TEMP TRIGGER reject_plan BEFORE INSERT ON capacity_work_plans BEGIN SELECT RAISE(ABORT, 'test'); END;").unwrap();
        assert!(
            store
                .update_capacity_work_plan(&fp, &env, &request(0), &now)
                .is_err()
        );
        assert_eq!(
            table_count(&store.connection, "capacity_demands").unwrap(),
            0
        );
        store
            .connection
            .execute_batch("DROP TRIGGER reject_plan")
            .unwrap();
        for revision in 0..7 {
            store
                .update_capacity_work_plan(&fp, &env, &request(revision), &now)
                .unwrap();
        }
        let recent = store.capacity_work_plans(&fp, &env).unwrap();
        assert_eq!(recent.len(), 5);
        assert_eq!(recent[0].revision, 7);
        assert_eq!(recent[4].revision, 3);
        assert_eq!(
            table_count(&store.connection, "capacity_work_plans").unwrap(),
            7
        );
        let other = AccountFingerprint::parse(FINGERPRINT_2).unwrap();
        store
            .connection
            .execute(
                "UPDATE account_bindings SET account_fingerprint=?1 WHERE account_fingerprint=?2",
                params![other.as_str(), fp.as_str()],
            )
            .unwrap();
        assert!(store.capacity_work_plans(&fp, &env).unwrap().is_empty());
        assert_eq!(store.capacity_work_plans(&other, &env).unwrap(), recent);
        store.verify_schema().unwrap();
    }

    #[test]
    fn explicit_plan_survives_reopen_without_changing_old_schedule() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capacity.sqlite3");
        let mut store = CapacityStore::open(&path).unwrap();
        let snapshot = snapshot("2026-08-30T02:55:53Z");
        store
            .record_snapshot(
                &stable_binding(),
                &snapshot,
                &observation("2026-08-30T02:55:53Z"),
            )
            .unwrap();
        let fp = AccountFingerprint::parse(FINGERPRINT).unwrap();
        let schedule = store.work_schedule().unwrap();
        store
            .update_capacity_work_plan(
                &fp,
                &snapshot.environment.environment_id,
                &request(0),
                &at("2026-08-30T03:00:00Z"),
            )
            .unwrap();
        drop(store);
        let reopened = CapacityStore::open(&path).unwrap();
        assert_eq!(
            reopened
                .capacity_work_plans(&fp, &snapshot.environment.environment_id)
                .unwrap()[0]
                .demand,
            request(0).demand
        );
        assert_eq!(reopened.work_schedule().unwrap(), schedule);
    }
}
