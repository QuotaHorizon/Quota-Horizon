use super::*;
use capacity_domain::active_time::{
    ActiveTimeAction, ActiveTimeInput, ActiveTimeObservation, ActiveTimer, ActiveTimerState,
    TimerTransition, whole_second,
};

#[derive(Debug, Clone)]
pub struct ActiveTimeSnapshot {
    pub revision: u64,
    pub timer: Option<ActiveTimer>,
    pub observations: Vec<ActiveTimeObservation>,
    pub included_count_30_days: u64,
    pub included_seconds_30_days: u64,
    pub has_other_environment_records: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTimeWriteOutcome {
    Updated,
    RevisionConflict,
    Overlap,
    TimerOpen,
}

impl CapacityStore {
    /// No elapsed time after the last checkpoint is credited when identity,
    /// authorization, reader environment, or process ownership is lost.
    pub fn interrupt_active_time(&mut self) -> Result<(), StoreError> {
        self.connection.execute("UPDATE active_time_timers SET state='review', ended_at=updated_at, interrupted=1, revision=revision+1 WHERE state IN ('running','paused')", [])?;
        Ok(())
    }

    pub fn active_time(
        &mut self,
        fp: &AccountFingerprint,
        env: &str,
        owner: &str,
        now: &UtcTimestamp,
    ) -> Result<ActiveTimeSnapshot, StoreError> {
        validate_scope(env, owner)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let timer = checkpoint(&tx, fp, env, owner, now)?;
        let observations = read_observations(&tx, fp, env)?;
        let (count, seconds) = tx.query_row("SELECT COUNT(*), COALESCE(SUM(duration_seconds),0) FROM active_time_observations WHERE account_fingerprint=?1 AND environment_id=?2 AND included=1 AND julianday(ended_at)>julianday(?3)-30 AND julianday(ended_at)<=julianday(?3)", params![fp.as_str(),env,now.as_str()], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?)))?;
        // Retained terminal drafts make this monotonic across every mutation.
        let revision = tx.query_row("SELECT (SELECT COALESCE(SUM(revision),0) FROM active_time_timers WHERE account_fingerprint=?1 AND environment_id=?2) + (SELECT COALESCE(SUM(revision),0) FROM active_time_observations WHERE account_fingerprint=?1 AND environment_id=?2)", params![fp.as_str(),env], |r| r.get::<_,i64>(0))?;
        let other = tx.query_row("SELECT EXISTS(SELECT 1 FROM active_time_observations WHERE account_fingerprint=?1 AND environment_id<>?2 UNION ALL SELECT 1 FROM active_time_timers WHERE account_fingerprint=?1 AND environment_id<>?2 AND state IN ('running','paused','review'))", params![fp.as_str(),env], |r| r.get(0))?;
        tx.commit()?;
        Ok(ActiveTimeSnapshot {
            revision: revision
                .try_into()
                .map_err(|_| StoreError::NumericOverflow)?,
            timer,
            observations,
            included_count_30_days: count.try_into().map_err(|_| StoreError::NumericOverflow)?,
            included_seconds_30_days: seconds
                .try_into()
                .map_err(|_| StoreError::NumericOverflow)?,
            has_other_environment_records: other,
        })
    }

    /// Recovery, revision guards and the mutation share a transaction. Callers
    /// do not need a prior read to avoid resuming an old process's draft.
    pub fn update_active_time(
        &mut self,
        fp: &AccountFingerprint,
        env: &str,
        owner: &str,
        action: &ActiveTimeAction,
        now: &UtcTimestamp,
    ) -> Result<ActiveTimeWriteOutcome, StoreError> {
        validate_scope(env, owner)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        checkpoint(&tx, fp, env, owner, now)?;
        let outcome = apply(&tx, fp, env, owner, action, &whole_second(now))?;
        tx.commit()?;
        Ok(outcome)
    }
}

fn checkpoint(
    conn: &Connection,
    fp: &AccountFingerprint,
    env: &str,
    owner: &str,
    now: &UtcTimestamp,
) -> Result<Option<ActiveTimer>, StoreError> {
    conn.execute("UPDATE active_time_timers SET state='review', ended_at=updated_at, interrupted=1, revision=revision+1 WHERE owner_context_id<>?1 AND state IN ('running','paused')", [owner])?;
    let mut timer = read_timer(conn, fp, env, None)?;
    if let Some(timer) = timer.as_mut() {
        let previous = timer.clone();
        timer.checkpoint(now, true);
        if *timer != previous {
            write_timer(conn, timer)?;
        }
    }
    Ok(timer)
}

fn validate_scope(env: &str, owner: &str) -> Result<(), StoreError> {
    if !bounded_metadata(env, 256) || !bounded_metadata(owner, 128) {
        return Err(StoreError::InvalidMetadata("activity_scope"));
    }
    Ok(())
}
pub(super) fn valid_id(id: &str) -> Result<(), StoreError> {
    if !Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id) {
        return Err(StoreError::InvalidMetadata("activity_id"));
    }
    Ok(())
}

fn apply(
    tx: &Transaction<'_>,
    fp: &AccountFingerprint,
    env: &str,
    owner: &str,
    action: &ActiveTimeAction,
    now: &UtcTimestamp,
) -> Result<ActiveTimeWriteOutcome, StoreError> {
    use ActiveTimeAction as A;
    use ActiveTimeWriteOutcome as O;
    match action {
        A::Start { timer_id } => {
            valid_id(timer_id)?;
            if let Some(timer) = read_timer(tx, fp, env, Some(timer_id))? {
                return Ok(if timer.state.is_open() {
                    O::Updated
                } else {
                    O::RevisionConflict
                });
            }
            if read_timer(tx, fp, env, None)?.is_some() {
                return Ok(O::TimerOpen);
            }
            let collision: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM active_time_timers WHERE timer_id=?1 OR state='running')", [timer_id], |r| r.get(0))?;
            if collision {
                return Ok(O::RevisionConflict);
            }
            tx.execute("INSERT INTO active_time_timers(timer_id,environment_id,account_fingerprint,owner_context_id,revision,state,started_at,ended_at,suggested_seconds,updated_at,interrupted) VALUES (?1,?2,?3,?4,1,'running',?5,NULL,0,?5,0)", params![timer_id,env,fp.as_str(),owner,now.as_str()])?;
            Ok(O::Updated)
        }
        A::Record {
            observation_id,
            observation,
        } => {
            valid_id(observation_id)?;
            let input = observation.validated(now)?;
            if let Some(previous) = read_observation(tx, fp, env, observation_id)? {
                return Ok(
                    if previous.observation == input && previous.source == "user_reported" {
                        O::Updated
                    } else {
                        O::RevisionConflict
                    },
                );
            }
            if read_timer(tx, fp, env, None)?.is_some() {
                return Ok(O::TimerOpen);
            }
            if overlaps(tx, fp, env, &input, None)? {
                return Ok(O::Overlap);
            }
            insert_observation(tx, fp, env, observation_id, &input, "user_reported", now)?;
            Ok(O::Updated)
        }
        A::SetIncluded {
            observation_id,
            expected_revision,
            included,
        } => {
            valid_id(observation_id)?;
            let Some(previous) = read_observation(tx, fp, env, observation_id)? else {
                return Ok(O::RevisionConflict);
            };
            if previous.revision != *expected_revision {
                return Ok(O::RevisionConflict);
            }
            if *included && overlaps(tx, fp, env, &previous.observation, Some(observation_id))? {
                return Ok(O::Overlap);
            }
            if previous.included != *included {
                tx.execute("UPDATE active_time_observations SET included=?1, revision=revision+1 WHERE observation_id=?2",params![included,observation_id])?;
            }
            Ok(O::Updated)
        }
        A::Pause {
            timer_id,
            expected_revision,
        }
        | A::Resume {
            timer_id,
            expected_revision,
        }
        | A::Finish {
            timer_id,
            expected_revision,
        }
        | A::Discard {
            timer_id,
            expected_revision,
        }
        | A::Confirm {
            timer_id,
            expected_revision,
            ..
        } => {
            valid_id(timer_id)?;
            let Some(mut timer) = read_timer(tx, fp, env, Some(timer_id))? else {
                return Ok(O::RevisionConflict);
            };
            if let A::Confirm { observation, .. } = action {
                let input = observation.validated(now)?;
                if timer.state == ActiveTimerState::Saved {
                    return Ok(
                        if read_observation(tx, fp, env, timer_id)?
                            .is_some_and(|row| row.observation == input)
                        {
                            O::Updated
                        } else {
                            O::RevisionConflict
                        },
                    );
                }
                if timer.revision != *expected_revision || timer.state != ActiveTimerState::Review {
                    return Ok(O::RevisionConflict);
                }
                if overlaps(tx, fp, env, &input, None)? {
                    return Ok(O::Overlap);
                }
                let source = if !timer.interrupted
                    && input.started_at == timer.started_at
                    && Some(&input.ended_at) == timer.ended_at.as_ref()
                    && input.duration_seconds == timer.suggested_seconds
                {
                    "user_timer"
                } else {
                    "user_reported"
                };
                insert_observation(tx, fp, env, timer_id, &input, source, now)?;
                timer.state = ActiveTimerState::Saved;
                timer.revision += 1;
            } else {
                if matches!(action, A::Discard { .. }) && timer.state == ActiveTimerState::Discarded
                {
                    return Ok(O::Updated);
                }
                if timer.revision != *expected_revision {
                    return Ok(O::RevisionConflict);
                }
                let transition = match action {
                    A::Pause { .. } => TimerTransition::Pause,
                    A::Resume { .. } => TimerTransition::Resume,
                    A::Finish { .. } => TimerTransition::Finish,
                    _ => TimerTransition::Discard,
                };
                timer.transition(transition, now)?;
            }
            write_timer(tx, &timer)?;
            Ok(O::Updated)
        }
    }
}

fn overlaps(
    conn: &Connection,
    fp: &AccountFingerprint,
    env: &str,
    input: &ActiveTimeInput,
    except: Option<&str>,
) -> Result<bool, StoreError> {
    // Intervals are conservative bounds, not continuous activity segments.
    // Reject any overlapping included record rather than double count gaps.
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM active_time_observations WHERE account_fingerprint=?1 AND environment_id=?2 AND included=1 AND (?5 IS NULL OR observation_id<>?5) AND julianday(started_at)<julianday(?4) AND julianday(ended_at)>julianday(?3))",params![fp.as_str(),env,input.started_at.as_str(),input.ended_at.as_str(),except],|r| r.get(0))?)
}
fn insert_observation(
    conn: &Connection,
    fp: &AccountFingerprint,
    env: &str,
    id: &str,
    input: &ActiveTimeInput,
    source: &str,
    now: &UtcTimestamp,
) -> Result<(), StoreError> {
    conn.execute("INSERT INTO active_time_observations(observation_id,environment_id,account_fingerprint,source,quality,started_at,ended_at,duration_seconds,included,revision,created_at) VALUES (?1,?2,?3,?4,'user_confirmed',?5,?6,?7,1,1,?8)",params![id,env,fp.as_str(),source,input.started_at.as_str(),input.ended_at.as_str(),input.duration_seconds,now.as_str()])?;
    Ok(())
}
pub(super) fn read_timer(
    conn: &Connection,
    fp: &AccountFingerprint,
    env: &str,
    id: Option<&str>,
) -> Result<Option<ActiveTimer>, StoreError> {
    let raw = conn.query_row("SELECT timer_id,revision,state,started_at,ended_at,suggested_seconds,updated_at,interrupted FROM active_time_timers WHERE account_fingerprint=?1 AND environment_id=?2 AND ((?3 IS NULL AND state IN ('running','paused','review')) OR timer_id=?3)",params![fp.as_str(),env,id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,u32>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,u32>(5)?,r.get::<_,String>(6)?,r.get::<_,bool>(7)?))).optional()?;
    raw.map(
        |(id, revision, state, start, end, seconds, updated, interrupted)| {
            let state = match state.as_str() {
                "running" => ActiveTimerState::Running,
                "paused" => ActiveTimerState::Paused,
                "review" => ActiveTimerState::Review,
                "saved" => ActiveTimerState::Saved,
                "discarded" => ActiveTimerState::Discarded,
                _ => return Err(StoreError::CorruptEnum("active_timer_state")),
            };
            Ok(ActiveTimer {
                timer_id: id,
                revision,
                state,
                started_at: UtcTimestamp::parse(start)?,
                ended_at: end.map(UtcTimestamp::parse).transpose()?,
                suggested_seconds: seconds,
                updated_at: UtcTimestamp::parse(updated)?,
                interrupted,
            })
        },
    )
    .transpose()
}
fn write_timer(conn: &Connection, timer: &ActiveTimer) -> Result<(), StoreError> {
    conn.execute("UPDATE active_time_timers SET revision=?1,state=?2,ended_at=?3,suggested_seconds=?4,updated_at=?5,interrupted=?6 WHERE timer_id=?7",params![timer.revision,timer.state.as_str(),timer.ended_at.as_ref().map(UtcTimestamp::as_str),timer.suggested_seconds,timer.updated_at.as_str(),timer.interrupted,timer.timer_id])?;
    Ok(())
}
fn observation_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, String, String, u32, bool, u32, String)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}
fn decode_observation(
    raw: (String, String, String, String, u32, bool, u32, String),
) -> Result<ActiveTimeObservation, StoreError> {
    let (id, source, start, end, seconds, included, revision, created) = raw;
    let source = match source.as_str() {
        "user_timer" => "user_timer",
        "user_reported" => "user_reported",
        _ => return Err(StoreError::CorruptEnum("active_time_source")),
    };
    Ok(ActiveTimeObservation {
        observation_id: id,
        source,
        quality: "user_confirmed",
        observation: ActiveTimeInput {
            started_at: UtcTimestamp::parse(start)?,
            ended_at: UtcTimestamp::parse(end)?,
            duration_seconds: seconds,
        },
        included,
        revision,
        created_at: UtcTimestamp::parse(created)?,
    })
}
fn read_observations(
    conn: &Connection,
    fp: &AccountFingerprint,
    env: &str,
) -> Result<Vec<ActiveTimeObservation>, StoreError> {
    let mut statement = conn.prepare("SELECT observation_id,source,started_at,ended_at,duration_seconds,included,revision,created_at FROM active_time_observations WHERE account_fingerprint=?1 AND environment_id=?2 ORDER BY ended_at DESC,observation_id LIMIT 20")?;
    statement
        .query_map(params![fp.as_str(), env], observation_row)?
        .map(|r| decode_observation(r?))
        .collect()
}
pub(super) fn read_observation(
    conn: &Connection,
    fp: &AccountFingerprint,
    env: &str,
    id: &str,
) -> Result<Option<ActiveTimeObservation>, StoreError> {
    conn.query_row("SELECT observation_id,source,started_at,ended_at,duration_seconds,included,revision,created_at FROM active_time_observations WHERE account_fingerprint=?1 AND environment_id=?2 AND observation_id=?3",params![fp.as_str(),env,id],observation_row).optional()?.map(decode_observation).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{
        FINGERPRINT, FINGERPRINT_2, observation, snapshot, stable_binding, v4_connection,
    };
    fn at(time: &str) -> UtcTimestamp {
        UtcTimestamp::parse(format!("2026-08-30T{time}Z")).unwrap()
    }
    fn fixture() -> (CapacityStore, AccountFingerprint, String) {
        let mut s = CapacityStore::open_in_memory().unwrap();
        let snap = snapshot("2026-08-30T02:00:00Z");
        s.record_snapshot(
            &stable_binding(),
            &snap,
            &observation("2026-08-30T02:00:00Z"),
        )
        .unwrap();
        (
            s,
            AccountFingerprint::parse(FINGERPRINT).unwrap(),
            snap.environment.environment_id,
        )
    }
    fn input() -> ActiveTimeInput {
        ActiveTimeInput {
            started_at: at("02:00:00"),
            ended_at: at("03:00:00"),
            duration_seconds: 1800,
        }
    }
    fn mutate(
        s: &mut CapacityStore,
        fp: &AccountFingerprint,
        env: &str,
        action: ActiveTimeAction,
        time: &str,
    ) -> ActiveTimeWriteOutcome {
        s.active_time(fp, env, "owner", &at(time)).unwrap();
        s.update_active_time(fp, env, "owner", &action, &at(time))
            .unwrap()
    }
    #[test]
    fn timer_pause_review_confirmation_and_retry_are_atomic_and_explicit() {
        let (mut s, fp, env) = fixture();
        let id = Uuid::new_v4().to_string();
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Start {
                timer_id: id.clone(),
            },
            "02:00:00",
        );
        let empty = s.active_time(&fp, &env, "owner", &at("02:10:00")).unwrap();
        assert_eq!(empty.included_count_30_days, 0);
        assert!(empty.observations.is_empty());
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Pause {
                timer_id: id.clone(),
                expected_revision: 1,
            },
            "02:10:00",
        );
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Resume {
                timer_id: id.clone(),
                expected_revision: 2,
            },
            "02:50:00",
        );
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Finish {
                timer_id: id.clone(),
                expected_revision: 3,
            },
            "03:00:00",
        );
        let view = s.active_time(&fp, &env, "owner", &at("03:00:00")).unwrap();
        assert_eq!(view.timer.as_ref().unwrap().suggested_seconds, 1200);
        assert!(view.observations.is_empty());
        let action = ActiveTimeAction::Confirm {
            timer_id: id,
            expected_revision: 4,
            observation: ActiveTimeInput {
                duration_seconds: 1200,
                ..input()
            },
        };
        assert_eq!(
            mutate(&mut s, &fp, &env, action.clone(), "03:00:00"),
            ActiveTimeWriteOutcome::Updated
        );
        assert_eq!(
            mutate(&mut s, &fp, &env, action, "03:00:01"),
            ActiveTimeWriteOutcome::Updated
        );
        let view = s.active_time(&fp, &env, "owner", &at("03:00:02")).unwrap();
        assert!(view.timer.is_none());
        assert_eq!(view.included_count_30_days, 1);
        assert_eq!(view.included_seconds_30_days, 1200);
        assert_eq!(view.observations[0].source, "user_timer");
    }
    #[test]
    fn overlap_exclusion_restore_and_stale_revision_do_not_double_count() {
        let (mut s, fp, env) = fixture();
        let id = Uuid::new_v4().to_string();
        let second = Uuid::new_v4().to_string();
        let record = ActiveTimeAction::Record {
            observation_id: id.clone(),
            observation: input(),
        };
        assert_eq!(
            mutate(&mut s, &fp, &env, record.clone(), "03:00:00"),
            ActiveTimeWriteOutcome::Updated
        );
        assert_eq!(
            mutate(&mut s, &fp, &env, record, "03:00:01"),
            ActiveTimeWriteOutcome::Updated
        );
        let overlapping = ActiveTimeAction::Record {
            observation_id: second,
            observation: input(),
        };
        assert_eq!(
            mutate(&mut s, &fp, &env, overlapping.clone(), "03:00:02"),
            ActiveTimeWriteOutcome::Overlap
        );
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::SetIncluded {
                observation_id: id.clone(),
                expected_revision: 1,
                included: false,
            },
            "03:00:03",
        );
        assert_eq!(
            mutate(&mut s, &fp, &env, overlapping, "03:00:04"),
            ActiveTimeWriteOutcome::Updated
        );
        assert_eq!(
            mutate(
                &mut s,
                &fp,
                &env,
                ActiveTimeAction::SetIncluded {
                    observation_id: id.clone(),
                    expected_revision: 1,
                    included: true
                },
                "03:00:05"
            ),
            ActiveTimeWriteOutcome::RevisionConflict
        );
        assert_eq!(
            mutate(
                &mut s,
                &fp,
                &env,
                ActiveTimeAction::SetIncluded {
                    observation_id: id,
                    expected_revision: 2,
                    included: true
                },
                "03:00:06"
            ),
            ActiveTimeWriteOutcome::Overlap
        );
        assert_eq!(
            s.active_time(&fp, &env, "owner", &at("03:00:07"))
                .unwrap()
                .included_count_30_days,
            1
        );
    }
    #[test]
    fn reopen_and_context_loss_never_resume_or_claim_elapsed_time() {
        let (mut s, fp, env) = fixture();
        let id = Uuid::new_v4().to_string();
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Start {
                timer_id: id.clone(),
            },
            "02:00:00",
        );
        s.active_time(&fp, &env, "owner", &at("02:10:00")).unwrap();
        let view = s
            .active_time(&fp, &env, "restarted-owner", &at("05:00:00"))
            .unwrap();
        let timer = view.timer.unwrap();
        assert!(timer.interrupted);
        assert_eq!(timer.suggested_seconds, 600);
        assert_eq!(timer.ended_at, Some(at("02:10:00")));
        assert!(view.observations.is_empty());
        assert_eq!(
            s.update_active_time(
                &fp,
                &env,
                "restarted-owner",
                &ActiveTimeAction::Pause {
                    timer_id: id,
                    expected_revision: 1
                },
                &at("05:00:00")
            )
            .unwrap(),
            ActiveTimeWriteOutcome::RevisionConflict
        );
    }
    #[test]
    fn account_environment_guards_missing_bindings_and_delete_all() {
        let (mut s, fp, env) = fixture();
        let id = Uuid::new_v4().to_string();
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Record {
                observation_id: id,
                observation: input(),
            },
            "03:00:00",
        );
        let other = AccountFingerprint::parse(FINGERPRINT_2).unwrap();
        assert!(
            s.active_time(&other, &env, "owner", &at("03:00:00"))
                .unwrap()
                .observations
                .is_empty()
        );
        let changed = s
            .active_time(&fp, "other-environment", "owner", &at("03:00:00"))
            .unwrap();
        assert!(changed.observations.is_empty());
        assert!(changed.has_other_environment_records);
        assert!(
            s.update_active_time(
                &other,
                &env,
                "owner",
                &ActiveTimeAction::Start {
                    timer_id: Uuid::new_v4().to_string()
                },
                &at("03:00:00")
            )
            .is_err()
        );
        let report = s.delete_all_database_data().unwrap();
        assert_eq!(report.active_time_observations, 1);
    }
    #[test]
    fn failed_confirmation_keeps_draft_and_no_observation() {
        let (mut s, fp, env) = fixture();
        let id = Uuid::new_v4().to_string();
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Start {
                timer_id: id.clone(),
            },
            "02:00:00",
        );
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Finish {
                timer_id: id.clone(),
                expected_revision: 1,
            },
            "03:00:00",
        );
        s.connection.execute_batch("CREATE TEMP TRIGGER fail_timer_save BEFORE UPDATE ON active_time_timers WHEN NEW.state='saved' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(
            s.update_active_time(
                &fp,
                &env,
                "owner",
                &ActiveTimeAction::Confirm {
                    timer_id: id,
                    expected_revision: 2,
                    observation: input()
                },
                &at("03:00:00")
            )
            .is_err()
        );
        let view = s.active_time(&fp, &env, "owner", &at("03:00:01")).unwrap();
        assert!(view.observations.is_empty());
        assert_eq!(view.timer.unwrap().state, ActiveTimerState::Review);
    }
    #[test]
    fn v6_migration_preserves_rows_and_failure_rolls_back() {
        for fail in [true, false] {
            let mut conn = v4_connection();
            let tx = conn.transaction().unwrap();
            tx.execute_batch(MIGRATION_V5).unwrap();
            record_schema_migration(&tx, 5).unwrap();
            tx.execute_batch(MIGRATION_V6).unwrap();
            record_schema_migration(&tx, 6).unwrap();
            tx.pragma_update(None, "user_version", 6).unwrap();
            tx.commit().unwrap();
            let mut s = CapacityStore {
                connection: conn,
                file_backed: false,
            };
            let snap = snapshot("2026-08-30T02:00:00Z");
            crate::tests::persist_legacy_snapshot(
                &mut s,
                &snap,
                &observation("2026-08-30T02:00:00Z"),
            )
            .unwrap();
            if fail {
                s.connection.execute_batch("CREATE TEMP TRIGGER reject_v7 BEFORE INSERT ON schema_migrations WHEN NEW.version=7 BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
            }
            assert_eq!(s.migrate().is_err(), fail);
            assert_eq!(
                s.schema_version().unwrap(),
                if fail { 6 } else { STORE_SCHEMA_VERSION }
            );
            assert_eq!(table_count(&s.connection, "quota_snapshots").unwrap(), 1);
            if !fail {
                s.verify_schema().unwrap();
            }
        }
    }
    #[test]
    fn file_reopen_retains_records_and_recovers_draft_at_last_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("activity.sqlite3");
        let mut s = CapacityStore::open(&path).unwrap();
        let snap = snapshot("2026-08-30T02:00:00Z");
        s.record_snapshot(
            &stable_binding(),
            &snap,
            &observation("2026-08-30T02:00:00Z"),
        )
        .unwrap();
        let fp = AccountFingerprint::parse(FINGERPRINT).unwrap();
        let env = snap.environment.environment_id;
        let record = Uuid::new_v4().to_string();
        let timer = Uuid::new_v4().to_string();
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Record {
                observation_id: record,
                observation: input(),
            },
            "03:00:00",
        );
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Start { timer_id: timer },
            "03:00:00",
        );
        s.active_time(&fp, &env, "owner", &at("03:10:00")).unwrap();
        drop(s);
        let mut s = CapacityStore::open(&path).unwrap();
        let view = s
            .active_time(&fp, &env, "new-process", &at("05:00:00"))
            .unwrap();
        assert_eq!(view.observations.len(), 1);
        let timer = view.timer.unwrap();
        assert_eq!(timer.suggested_seconds, 600);
        assert_eq!(timer.ended_at, Some(at("03:10:00")));
        assert!(timer.interrupted);
    }
    #[test]
    fn binding_rekey_cascades_activity_and_interrupted_confirmations_are_user_reports() {
        let (mut s, fp, env) = fixture();
        let id = Uuid::new_v4().to_string();
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Start {
                timer_id: id.clone(),
            },
            "02:00:00",
        );
        s.active_time(&fp, &env, "owner", &at("03:00:00")).unwrap();
        s.interrupt_active_time().unwrap();
        let other = AccountFingerprint::parse(FINGERPRINT_2).unwrap();
        s.connection
            .execute(
                "UPDATE account_bindings SET account_fingerprint=?1 WHERE account_fingerprint=?2",
                params![other.as_str(), fp.as_str()],
            )
            .unwrap();
        let view = s
            .active_time(&other, &env, "new-owner", &at("03:10:00"))
            .unwrap();
        let revision = view.timer.unwrap().revision;
        let input = ActiveTimeInput {
            duration_seconds: 3600,
            ..input()
        };
        let result = s
            .update_active_time(
                &other,
                &env,
                "new-owner",
                &ActiveTimeAction::Confirm {
                    timer_id: id,
                    expected_revision: revision,
                    observation: input,
                },
                &at("03:10:00"),
            )
            .unwrap();
        assert_eq!(result, ActiveTimeWriteOutcome::Updated);
        let view = s
            .active_time(&other, &env, "new-owner", &at("03:10:00"))
            .unwrap();
        assert_eq!(view.observations[0].source, "user_reported");
        s.connection
            .execute(
                "UPDATE account_bindings SET account_fingerprint=?1 WHERE account_fingerprint=?2",
                params![fp.as_str(), other.as_str()],
            )
            .unwrap();
        assert_eq!(
            s.active_time(&fp, &env, "owner", &at("03:10:00"))
                .unwrap()
                .observations
                .len(),
            1
        );
        assert!(
            s.active_time(&other, &env, "owner", &at("03:10:00"))
                .unwrap()
                .observations
                .is_empty()
        );
    }
    #[test]
    fn adjacent_intervals_are_allowed_but_an_open_timer_blocks_manual_recording() {
        let (mut s, fp, env) = fixture();
        let id = Uuid::new_v4().to_string();
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Start {
                timer_id: id.clone(),
            },
            "02:00:00",
        );
        assert_eq!(
            mutate(
                &mut s,
                &fp,
                &env,
                ActiveTimeAction::Record {
                    observation_id: Uuid::new_v4().to_string(),
                    observation: input()
                },
                "03:00:00"
            ),
            ActiveTimeWriteOutcome::TimerOpen
        );
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Discard {
                timer_id: id,
                expected_revision: 1,
            },
            "03:00:00",
        );
        mutate(
            &mut s,
            &fp,
            &env,
            ActiveTimeAction::Record {
                observation_id: Uuid::new_v4().to_string(),
                observation: input(),
            },
            "03:00:00",
        );
        let adjacent = ActiveTimeInput {
            started_at: at("03:00:00"),
            ended_at: at("04:00:00"),
            duration_seconds: 1800,
        };
        assert_eq!(
            mutate(
                &mut s,
                &fp,
                &env,
                ActiveTimeAction::Record {
                    observation_id: Uuid::new_v4().to_string(),
                    observation: adjacent
                },
                "04:00:00"
            ),
            ActiveTimeWriteOutcome::Updated
        );
        assert_eq!(
            s.active_time(&fp, &env, "owner", &at("04:00:00"))
                .unwrap()
                .included_seconds_30_days,
            3600
        );
    }
    #[test]
    fn direct_mutation_recovers_context_and_cannot_bypass_checkpoint_with_stale_revision() {
        let (mut s, fp, env) = fixture();
        let id = Uuid::new_v4().to_string();
        s.update_active_time(
            &fp,
            &env,
            "owner",
            &ActiveTimeAction::Start {
                timer_id: id.clone(),
            },
            &at("02:00:00"),
        )
        .unwrap();
        s.update_active_time(
            &fp,
            &env,
            "owner",
            &ActiveTimeAction::Pause {
                timer_id: id.clone(),
                expected_revision: 1,
            },
            &at("02:10:00"),
        )
        .unwrap();
        assert_eq!(
            s.update_active_time(
                &fp,
                &env,
                "new-owner",
                &ActiveTimeAction::Resume {
                    timer_id: id,
                    expected_revision: 2
                },
                &at("03:00:00")
            )
            .unwrap(),
            ActiveTimeWriteOutcome::RevisionConflict
        );
        let t = s
            .active_time(&fp, &env, "new-owner", &at("03:00:00"))
            .unwrap()
            .timer
            .unwrap();
        assert!(t.interrupted);
        assert_eq!(t.suggested_seconds, 600);
        assert_eq!(t.state, ActiveTimerState::Review);
        assert!(valid_id("018F47A2-8A71-7F4A-9C35-1F4234A73311").is_err());
    }
}
