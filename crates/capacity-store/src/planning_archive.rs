//! Read-only access to this account's prior reader scopes. Resource UUIDs are
//! resolved here; the WebView never chooses an account or environment ID.
use super::*;
use capacity_domain::planning_archive::{
    PlanningArchiveDraft, PlanningArchiveEntry, PlanningArchiveRef,
};
pub use capacity_domain::planning_archive::{PlanningArchiveQuery, PlanningArchiveResult};

const PAGE_SIZE: u32 = 20;

fn page(offset: u32) -> Result<(), StoreError> {
    // Bounded per read. Offset is not an authorization input.
    if offset > i32::MAX as u32 {
        return Err(StoreError::InvalidMetadata("archive_offset"));
    }
    Ok(())
}

impl CapacityStore {
    pub fn planning_archive(
        &self,
        fp: &AccountFingerprint,
        current_env: &str,
        query: &PlanningArchiveQuery,
    ) -> Result<Option<PlanningArchiveResult>, StoreError> {
        if !bounded_metadata(current_env, 256) {
            return Err(StoreError::InvalidMetadata("environment_id"));
        }
        match query {
            PlanningArchiveQuery::List { offset } => {
                page(*offset)?;
                // One deterministic resource anchor per scope; UUID remains a
                // reference, never a capability to bypass fingerprint checks.
                let mut statement = self.connection.prepare("WITH items AS (
                    SELECT environment_id, 'plan' AS kind, work_plan_id AS id, created_at AS recorded FROM capacity_work_plans WHERE account_fingerprint=?1
                    UNION ALL SELECT environment_id, 'record', observation_id, created_at FROM active_time_observations WHERE account_fingerprint=?1
                    UNION ALL SELECT environment_id, 'timer', timer_id, updated_at FROM active_time_timers WHERE account_fingerprint=?1 AND state IN ('running','paused','review')
                    UNION ALL SELECT environment_id, 'trial', trial_id, issued_at FROM pace_trials WHERE account_fingerprint=?1
                ), ranked AS (SELECT *,ROW_NUMBER() OVER (PARTITION BY environment_id ORDER BY julianday(recorded) DESC, kind, id) AS n FROM items WHERE environment_id<>?2)
                SELECT r.environment_id,r.kind,r.id,r.recorded,e.platform,e.architecture,e.boundary,
                    (SELECT c.codex_version FROM quota_snapshots s JOIN compatibility_observations c ON c.observation_id=s.compatibility_observation_id AND c.environment_id=s.environment_id WHERE s.account_fingerprint=?1 AND s.environment_id=r.environment_id ORDER BY julianday(s.captured_at) DESC,s.snapshot_id LIMIT 1),
                    (SELECT COUNT(*) FROM capacity_work_plans p WHERE p.account_fingerprint=?1 AND p.environment_id=r.environment_id),
                    (SELECT COUNT(*) FROM active_time_observations a WHERE a.account_fingerprint=?1 AND a.environment_id=r.environment_id),
                    (SELECT COUNT(*) FROM pace_trials t WHERE t.account_fingerprint=?1 AND t.environment_id=r.environment_id)
                FROM ranked r JOIN environments e ON e.environment_id=r.environment_id WHERE r.n=1
                ORDER BY julianday(r.recorded) DESC,r.environment_id LIMIT ?3 OFFSET ?4")?;
                let mut rows =
                    statement.query(params![fp.as_str(), current_env, PAGE_SIZE + 1, offset])?;
                let mut entries = vec![];
                while let Some(row) = rows.next()? {
                    let kind: String = row.get(1)?;
                    let id = row.get(2)?;
                    let source = match kind.as_str() {
                        "plan" => PlanningArchiveRef::Plan(id),
                        "record" => PlanningArchiveRef::Record(id),
                        "trial" => PlanningArchiveRef::Trial(id),
                        _ => PlanningArchiveRef::Timer(id),
                    };
                    entries.push(PlanningArchiveEntry {
                        source,
                        platform: row.get(4)?,
                        architecture: row.get(5)?,
                        boundary: row.get(6)?,
                        codex_version: row.get(7)?,
                        last_recorded_at: UtcTimestamp::parse(row.get::<_, String>(3)?)?,
                        plan_revisions: row
                            .get::<_, i64>(8)?
                            .try_into()
                            .map_err(|_| StoreError::NumericOverflow)?,
                        record_count: row
                            .get::<_, i64>(9)?
                            .try_into()
                            .map_err(|_| StoreError::NumericOverflow)?,
                        trial_count: row
                            .get::<_, i64>(10)?
                            .try_into()
                            .map_err(|_| StoreError::NumericOverflow)?,
                    });
                }
                let has_more = entries.len() > PAGE_SIZE as usize;
                entries.truncate(PAGE_SIZE as usize);
                Ok(Some(PlanningArchiveResult::List { entries, has_more }))
            }
            PlanningArchiveQuery::Details { source, offset } => {
                page(*offset)?;
                let Some(env) = self.archive_environment(fp, current_env, source)? else {
                    return Ok(None);
                };
                let plans = super::demand::read_plans(&self.connection, fp, &env)?;
                let timer = super::active_time::read_timer(&self.connection, fp, &env, None)?;
                // Never checkpoint, resume, complete or copy an archived timer.
                let draft = timer.map(|timer| PlanningArchiveDraft {
                    started_at: timer.started_at,
                    last_checkpoint: timer.updated_at,
                    suggested_seconds: timer.suggested_seconds,
                });
                let mut statement = self.connection.prepare("SELECT observation_id FROM active_time_observations WHERE account_fingerprint=?1 AND environment_id=?2 ORDER BY julianday(ended_at) DESC,observation_id LIMIT ?3 OFFSET ?4")?;
                let ids = statement
                    .query_map(params![fp.as_str(), env, PAGE_SIZE + 1, offset], |r| {
                        r.get::<_, String>(0)
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                let has_more = ids.len() > PAGE_SIZE as usize;
                let records = ids
                    .into_iter()
                    .take(PAGE_SIZE as usize)
                    .map(|id| {
                        super::active_time::read_observation(&self.connection, fp, &env, &id)?
                            .ok_or(StoreError::InvalidMetadata("archive_record_missing"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Some(PlanningArchiveResult::Details {
                    plans,
                    records,
                    draft,
                    has_more,
                }))
            }
            PlanningArchiveQuery::Pace { source, offset } => {
                if *offset > 100_000 {
                    return Err(StoreError::InvalidMetadata("archive_offset"));
                }
                let Some(env) = self.archive_environment(fp, current_env, source)? else {
                    return Ok(None);
                };
                let mut trials = self.pace_trials(fp, &env, *offset)?;
                let has_more = trials.len() > PAGE_SIZE as usize;
                trials.truncate(PAGE_SIZE as usize);
                Ok(Some(PlanningArchiveResult::Pace { trials, has_more }))
            }
            PlanningArchiveQuery::Quota {
                source,
                observation_id,
            } => {
                super::active_time::valid_id(observation_id)?;
                let Some(env) = self.archive_environment(fp, current_env, source)? else {
                    return Ok(None);
                };
                Ok(self
                    .active_time_quota_evidence(fp, &env, observation_id)?
                    .map(|evidence| PlanningArchiveResult::Quota {
                        record: evidence.record,
                        comparison: evidence.comparison,
                    }))
            }
        }
    }

    fn archive_environment(
        &self,
        fp: &AccountFingerprint,
        current: &str,
        source: &PlanningArchiveRef,
    ) -> Result<Option<String>, StoreError> {
        let (sql, id) = match source {
            PlanningArchiveRef::Plan(id) => (
                "SELECT DISTINCT environment_id FROM capacity_work_plans WHERE account_fingerprint=?1 AND environment_id<>?2 AND work_plan_id=?3",
                id,
            ),
            PlanningArchiveRef::Record(id) => (
                "SELECT environment_id FROM active_time_observations WHERE account_fingerprint=?1 AND environment_id<>?2 AND observation_id=?3",
                id,
            ),
            PlanningArchiveRef::Timer(id) => (
                "SELECT environment_id FROM active_time_timers WHERE account_fingerprint=?1 AND environment_id<>?2 AND timer_id=?3 AND state IN ('running','paused','review')",
                id,
            ),
            PlanningArchiveRef::Trial(id) => (
                "SELECT environment_id FROM pace_trials WHERE account_fingerprint=?1 AND environment_id<>?2 AND trial_id=?3",
                id,
            ),
        };
        super::active_time::valid_id(id)?;
        let mut statement = self.connection.prepare(sql)?;
        let mut rows = statement.query(params![fp.as_str(), current, id])?;
        let env: Option<String> = rows.next()?.map(|r| r.get(0)).transpose()?;
        // Ambiguous imported/corrupt plan IDs cannot choose a scope arbitrarily.
        if rows.next()?.is_some() {
            return Err(StoreError::InvalidMetadata("archive_ambiguous"));
        }
        Ok(env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{FINGERPRINT, FINGERPRINT_2, observation, snapshot, stable_binding};
    use capacity_domain::{
        active_time::{ActiveTimeAction, ActiveTimeInput},
        demand::{CapacityDemandInput, DemandKind},
    };
    fn at(value: &str) -> UtcTimestamp {
        UtcTimestamp::parse(value).unwrap()
    }
    fn capture(store: &mut CapacityStore, env: &str, when: &str, remaining: f64) {
        let mut snap = snapshot(when);
        snap.environment.environment_id = env.into();
        snap.quota.windows[0].remaining_percent = remaining;
        snap.quota.windows[0].used_percent = 100.0 - remaining;
        let mut obs = observation(when);
        obs.environment_id = env.into();
        store
            .record_snapshot(&stable_binding(), &snap, &obs)
            .unwrap();
    }
    fn fixture() -> (
        CapacityStore,
        AccountFingerprint,
        PlanningArchiveRef,
        String,
    ) {
        let mut store = CapacityStore::open_in_memory().unwrap();
        capture(&mut store, "old-reader", "2026-08-30T02:10:00Z", 40.0);
        capture(&mut store, "old-reader", "2026-08-30T02:30:00Z", 39.0);
        capture(&mut store, "new-reader", "2026-08-30T02:30:00Z", 10.0);
        let fp = AccountFingerprint::parse(FINGERPRINT).unwrap();
        let now = at("2026-08-30T03:00:00Z");
        let id = Uuid::new_v4().to_string();
        store
            .update_active_time(
                &fp,
                "old-reader",
                "owner",
                &ActiveTimeAction::Record {
                    observation_id: id.clone(),
                    observation: ActiveTimeInput {
                        started_at: at("2026-08-30T02:00:00Z"),
                        ended_at: at("2026-08-30T02:40:00Z"),
                        duration_seconds: 600,
                    },
                },
                &now,
            )
            .unwrap();
        store
            .update_capacity_work_plan(
                &fp,
                "old-reader",
                &CapacityWorkPlanUpdate {
                    expected_revision: 0,
                    enabled: true,
                    demand: CapacityDemandInput {
                        horizon_end: at("2026-09-01T00:00:00Z"),
                        demand_kind: DemandKind::ActiveHours,
                        planned_codex_active_hours: Some(4.0),
                    },
                },
                &now,
            )
            .unwrap();
        let plan = store
            .capacity_work_plans(&fp, "old-reader")
            .unwrap()
            .remove(0);
        (store, fp, PlanningArchiveRef::Plan(plan.work_plan_id), id)
    }
    #[test]
    fn archives_resolve_resources_only_within_current_account_and_old_scope() {
        let (store, fp, source, id) = fixture();
        let list = store
            .planning_archive(&fp, "new-reader", &PlanningArchiveQuery::List { offset: 0 })
            .unwrap()
            .unwrap();
        let PlanningArchiveResult::List { entries, has_more } = list else {
            panic!()
        };
        assert_eq!(entries.len(), 1);
        assert!(!has_more);
        assert_eq!(entries[0].plan_revisions, 1);
        assert_eq!(entries[0].record_count, 1);
        let query = PlanningArchiveQuery::Details {
            source: source.clone(),
            offset: 0,
        };
        let PlanningArchiveResult::Details { plans, records, .. } = store
            .planning_archive(&fp, "new-reader", &query)
            .unwrap()
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(plans.len(), 1);
        assert_eq!(records[0].observation_id, id);
        let other = AccountFingerprint::parse(FINGERPRINT_2).unwrap();
        assert!(
            store
                .planning_archive(&other, "new-reader", &query)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .planning_archive(&fp, "old-reader", &query)
                .unwrap()
                .is_none()
        );
        let PlanningArchiveResult::List { entries, .. } = store
            .planning_archive(
                &other,
                "new-reader",
                &PlanningArchiveQuery::List { offset: 0 },
            )
            .unwrap()
            .unwrap()
        else {
            panic!()
        };
        assert!(entries.is_empty());
    }
    #[test]
    fn original_quota_and_timer_read_do_not_merge_current_balance_or_checkpoint() {
        let (mut store, fp, source, id) = fixture();
        let timer = Uuid::new_v4().to_string();
        let now = at("2026-08-30T03:00:00Z");
        store
            .update_active_time(
                &fp,
                "old-reader",
                "owner",
                &ActiveTimeAction::Start {
                    timer_id: timer.clone(),
                },
                &now,
            )
            .unwrap();
        let before = store.connection.total_changes();
        let PlanningArchiveResult::Details { draft, .. } = store
            .planning_archive(
                &fp,
                "new-reader",
                &PlanningArchiveQuery::Details {
                    source: source.clone(),
                    offset: 0,
                },
            )
            .unwrap()
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(draft.unwrap().last_checkpoint, now);
        let PlanningArchiveResult::Quota { comparison, .. } = store
            .planning_archive(
                &fp,
                "new-reader",
                &PlanningArchiveQuery::Quota {
                    source,
                    observation_id: id,
                },
            )
            .unwrap()
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(comparison.windows[0].observed_decrease_percent, Some(1.0));
        assert_eq!(comparison.windows[0].first_remaining_percent, Some(40.0));
        assert_eq!(store.connection.total_changes(), before);
        let current = super::super::active_time::read_timer(
            &store.connection,
            &fp,
            "old-reader",
            Some(&timer),
        )
        .unwrap()
        .unwrap();
        assert_eq!(current.revision, 1);
        assert_eq!(current.suggested_seconds, 0);
    }
    #[test]
    fn list_and_activity_pagination_are_bounded_without_hiding_next_page() {
        let (mut store, fp, source, _) = fixture();
        // Distinct, adjacent activity intervals; old stored rows stay unchanged.
        for n in 0..25 {
            let from = format!("2026-08-29T00:{n:02}:00Z");
            let to = format!("2026-08-29T00:{:02}:00Z", n + 1);
            store
                .update_active_time(
                    &fp,
                    "old-reader",
                    "owner",
                    &ActiveTimeAction::Record {
                        observation_id: Uuid::new_v4().to_string(),
                        observation: ActiveTimeInput {
                            started_at: at(&from),
                            ended_at: at(&to),
                            duration_seconds: 30,
                        },
                    },
                    &at("2026-08-30T03:00:00Z"),
                )
                .unwrap();
        }
        for (offset, count, more) in [(0, 20, true), (20, 6, false)] {
            let PlanningArchiveResult::Details {
                records, has_more, ..
            } = store
                .planning_archive(
                    &fp,
                    "new-reader",
                    &PlanningArchiveQuery::Details {
                        source: source.clone(),
                        offset,
                    },
                )
                .unwrap()
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(records.len(), count);
            assert_eq!(has_more, more);
        }
        for n in 0..22 {
            let env = format!("prior-{n}");
            capture(&mut store, &env, "2026-08-30T02:30:00Z", 50.0);
            store
                .update_capacity_work_plan(
                    &fp,
                    &env,
                    &CapacityWorkPlanUpdate {
                        expected_revision: 0,
                        enabled: true,
                        demand: CapacityDemandInput {
                            horizon_end: at("2026-09-01T00:00:00Z"),
                            demand_kind: DemandKind::MaintainRecentPace,
                            planned_codex_active_hours: None,
                        },
                    },
                    &at("2026-08-30T03:00:00Z"),
                )
                .unwrap();
        }
        let mut ids = BTreeSet::new();
        for (offset, count, more) in [(0, 20, true), (20, 3, false)] {
            let PlanningArchiveResult::List { entries, has_more } = store
                .planning_archive(&fp, "new-reader", &PlanningArchiveQuery::List { offset })
                .unwrap()
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(entries.len(), count);
            assert_eq!(has_more, more);
            for entry in entries {
                assert!(ids.insert(entry.source));
            }
        }
    }
    #[test]
    fn malformed_missing_cross_scope_and_deleted_anchors_cannot_read_history() {
        let (mut store, fp, source, id) = fixture();
        assert!(
            store
                .planning_archive(
                    &fp,
                    "new-reader",
                    &PlanningArchiveQuery::Details {
                        source: PlanningArchiveRef::Plan("not-an-id".into()),
                        offset: 0
                    }
                )
                .is_err()
        );
        assert!(
            store
                .planning_archive(
                    &fp,
                    "new-reader",
                    &PlanningArchiveQuery::List { offset: u32::MAX }
                )
                .is_err()
        );
        assert!(
            store
                .planning_archive(
                    &fp,
                    "new-reader",
                    &PlanningArchiveQuery::Quota {
                        source: PlanningArchiveRef::Record(Uuid::new_v4().to_string()),
                        observation_id: id.clone()
                    }
                )
                .unwrap()
                .is_none()
        );
        let current_id = Uuid::new_v4().to_string();
        store
            .update_active_time(
                &fp,
                "new-reader",
                "owner",
                &ActiveTimeAction::Record {
                    observation_id: current_id.clone(),
                    observation: ActiveTimeInput {
                        started_at: at("2026-08-30T02:00:00Z"),
                        ended_at: at("2026-08-30T02:40:00Z"),
                        duration_seconds: 600,
                    },
                },
                &at("2026-08-30T03:00:00Z"),
            )
            .unwrap();
        assert!(
            store
                .planning_archive(
                    &fp,
                    "new-reader",
                    &PlanningArchiveQuery::Quota {
                        source: source.clone(),
                        observation_id: current_id,
                    }
                )
                .unwrap()
                .is_none()
        );
        let query = PlanningArchiveQuery::Details { source, offset: 0 };
        store.delete_all_database_data().unwrap();
        assert!(
            store
                .planning_archive(&fp, "new-reader", &query)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn explicit_template_save_creates_a_new_lineage_without_moving_old_records() {
        let (mut store, fp, source, id) = fixture();
        let old = store
            .capacity_work_plans(&fp, "old-reader")
            .unwrap()
            .remove(0);
        assert!(
            store
                .capacity_work_plans(&fp, "new-reader")
                .unwrap()
                .is_empty()
        );
        let request = CapacityWorkPlanUpdate {
            expected_revision: 0,
            enabled: true,
            demand: old.demand.clone(),
        };
        let CapacityWorkPlanUpdateOutcome::Updated(new) = store
            .update_capacity_work_plan(&fp, "new-reader", &request, &at("2026-08-30T03:10:00Z"))
            .unwrap()
        else {
            panic!()
        };
        assert_ne!(new.work_plan_id, old.work_plan_id);
        assert_eq!(new.revision, 1);
        assert_eq!(new.demand, old.demand);
        assert_eq!(
            store.capacity_work_plans(&fp, "old-reader").unwrap()[0],
            old
        );
        let PlanningArchiveResult::Details { records, .. } = store
            .planning_archive(
                &fp,
                "new-reader",
                &PlanningArchiveQuery::Details { source, offset: 0 },
            )
            .unwrap()
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].observation_id, id);
        assert!(
            super::super::active_time::read_observation(&store.connection, &fp, "new-reader", &id)
                .unwrap()
                .is_none()
        );
    }
}
