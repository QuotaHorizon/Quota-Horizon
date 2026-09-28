use super::*;
use capacity_domain::{
    active_time::ActiveTimeObservation,
    activity_quota::{ActivityQuotaComparison, ActivityQuotaSample, compare_activity_quota},
};

const MAX_ACTIVITY_QUOTA_ROWS: u32 = 25_000;

#[derive(Debug, Clone)]
pub struct ActivityQuotaEvidence {
    pub record: ActiveTimeObservation,
    pub comparison: ActivityQuotaComparison,
}

impl CapacityStore {
    /// Demand and activity use exact environments. Do NOT use the wider native
    /// history-family query here or silently migrate this record to a new reader.
    pub fn active_time_quota_evidence(
        &self,
        fp: &AccountFingerprint,
        env: &str,
        observation_id: &str,
    ) -> Result<Option<ActivityQuotaEvidence>, StoreError> {
        self.active_time_quota_evidence_bounded(fp, env, observation_id, MAX_ACTIVITY_QUOTA_ROWS)
    }
    fn active_time_quota_evidence_bounded(
        &self,
        fp: &AccountFingerprint,
        env: &str,
        observation_id: &str,
        maximum: u32,
    ) -> Result<Option<ActivityQuotaEvidence>, StoreError> {
        if !bounded_metadata(env, 256) {
            return Err(StoreError::InvalidMetadata("environment_id"));
        }
        super::active_time::valid_id(observation_id)?;
        let Some(record) =
            super::active_time::read_observation(&self.connection, fp, env, observation_id)?
        else {
            return Ok(None);
        };
        let mut statement=self.connection.prepare("SELECT s.snapshot_id,s.captured_at,w.limit_id,w.window_minutes,w.remaining_basis_points,w.resets_at,s.availability,s.freshness,c.compatibility FROM quota_snapshots s JOIN quota_windows w ON w.snapshot_id=s.snapshot_id JOIN compatibility_observations c ON c.observation_id=s.compatibility_observation_id AND c.environment_id=s.environment_id WHERE s.account_fingerprint=?1 AND s.environment_id=?2 AND (w.limit_id='codex' OR w.limit_id GLOB 'codex:*') AND julianday(s.captured_at)>=julianday(?3) AND julianday(s.captured_at)<=julianday(?4) ORDER BY julianday(s.captured_at),s.snapshot_id,w.limit_id LIMIT ?5")?;
        let mut rows = statement.query(params![
            fp.as_str(),
            env,
            record.observation.started_at.as_str(),
            record.observation.ended_at.as_str(),
            i64::from(maximum) + 1
        ])?;
        let mut samples = Vec::new();
        while let Some(row) = rows.next()? {
            let captured: String = row.get(1)?;
            let remaining: i64 = row.get(4)?;
            let reset: Option<String> = row.get(5)?;
            let availability: String = row.get(6)?;
            let freshness: String = row.get(7)?;
            let compatibility: String = row.get(8)?;
            let minutes: Option<i64> = row.get(3)?;
            samples.push(ActivityQuotaSample {
                snapshot_id: row.get(0)?,
                captured_at: UtcTimestamp::parse(captured)?,
                limit_id: row.get(2)?,
                window_minutes: minutes
                    .map(|m| m.try_into().map_err(|_| StoreError::NumericOverflow))
                    .transpose()?,
                remaining_percent: remaining as f64 / 100.0,
                resets_at: reset.map(UtcTimestamp::parse).transpose()?,
                availability: availability_from_str(&availability)?,
                freshness: match freshness.as_str() {
                    "live" => Freshness::Live,
                    "stale" => Freshness::Stale,
                    "not_applicable" => Freshness::NotApplicable,
                    _ => return Err(StoreError::CorruptEnum("freshness")),
                },
                compatibility: compatibility_from_str(&compatibility)?,
            });
        }
        let truncated = samples.len() > maximum as usize;
        samples.truncate(maximum as usize);
        let comparison = compare_activity_quota(&record.observation, &samples, truncated);
        Ok(Some(ActivityQuotaEvidence { record, comparison }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{FINGERPRINT, FINGERPRINT_2, observation, snapshot, stable_binding};
    use capacity_domain::active_time::{ActiveTimeAction, ActiveTimeInput};
    fn at(time: &str) -> UtcTimestamp {
        UtcTimestamp::parse(format!("2026-08-30T{time}Z")).unwrap()
    }
    fn capture(
        store: &mut CapacityStore,
        time: &str,
        remaining: f64,
        binding: &AccountBinding,
        env: Option<&str>,
    ) {
        let mut snap = snapshot(at(time).as_str());
        snap.quota.windows[0].remaining_percent = remaining;
        snap.quota.windows[0].used_percent = 100.0 - remaining;
        if let Some(env) = env {
            snap.environment.environment_id = env.into();
        }
        let mut compatibility = observation(at(time).as_str());
        compatibility.environment_id = snap.environment.environment_id.clone();
        store
            .record_snapshot(binding, &snap, &compatibility)
            .unwrap();
    }
    fn fixture() -> (CapacityStore, AccountFingerprint, String, String) {
        let mut store = CapacityStore::open_in_memory().unwrap();
        let binding = stable_binding();
        for (time, remaining) in [
            ("01:59:59", 70.0),
            ("02:10:00", 40.0),
            ("02:25:00", 39.0),
            ("02:40:00", 38.0),
            ("02:50:00", 37.0),
            ("03:00:01", 1.0),
        ] {
            capture(&mut store, time, remaining, &binding, None);
        }
        let fp = AccountFingerprint::parse(FINGERPRINT).unwrap();
        let env = "macos-arm64-native".to_owned();
        let id = Uuid::new_v4().to_string();
        store
            .update_active_time(
                &fp,
                &env,
                "owner",
                &ActiveTimeAction::Record {
                    observation_id: id.clone(),
                    observation: ActiveTimeInput {
                        started_at: at("02:00:00"),
                        ended_at: at("03:00:00"),
                        duration_seconds: 1800,
                    },
                },
                &at("04:00:00"),
            )
            .unwrap();
        (store, fp, env, id)
    }
    #[test]
    fn raw_exact_scope_is_used_without_outside_endpoints_or_other_account_history() {
        let (mut s, fp, env, id) = fixture();
        let other = AccountFingerprint::parse(FINGERPRINT_2).unwrap();
        capture(
            &mut s,
            "02:20:00",
            95.0,
            &AccountBinding::Stable(other.clone()),
            None,
        );
        capture(
            &mut s,
            "02:30:00",
            99.0,
            &stable_binding(),
            Some("other-environment"),
        );
        let v = s
            .active_time_quota_evidence(&fp, &env, &id)
            .unwrap()
            .unwrap();
        assert_eq!(v.comparison.windows[0].sample_count, 4);
        assert_eq!(v.comparison.windows[0].observed_decrease_percent, Some(3.0));
        assert!(
            s.active_time_quota_evidence(&other, &env, &id)
                .unwrap()
                .is_none()
        );
        assert!(
            s.active_time_quota_evidence(&fp, "other-environment", &id)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn interior_rise_is_not_lost_to_chart_downsampling_and_exclusion_is_preserved() {
        let (mut s, fp, env, id) = fixture();
        capture(&mut s, "02:30:00", 41.0, &stable_binding(), None);
        s.update_active_time(
            &fp,
            &env,
            "owner",
            &ActiveTimeAction::SetIncluded {
                observation_id: id.clone(),
                expected_revision: 1,
                included: false,
            },
            &at("04:00:00"),
        )
        .unwrap();
        let v = s
            .active_time_quota_evidence(&fp, &env, &id)
            .unwrap()
            .unwrap();
        assert!(!v.record.included);
        assert_eq!(v.comparison.windows[0].reason_code, "balance_increased");
        assert!(v.comparison.windows[0].observed_decrease_percent.is_none());
    }
    #[test]
    fn query_cap_is_explicit_and_no_history_is_not_a_zero_consumption_record() {
        let (s, fp, env, id) = fixture();
        let capped = s
            .active_time_quota_evidence_bounded(&fp, &env, &id, 2)
            .unwrap()
            .unwrap();
        assert!(capped.comparison.query_truncated);
        assert_eq!(capped.comparison.windows[0].reason_code, "query_truncated");
        s.connection
            .execute("DELETE FROM quota_snapshots", [])
            .unwrap();
        let empty = s
            .active_time_quota_evidence(&fp, &env, &id)
            .unwrap()
            .unwrap();
        assert!(empty.comparison.windows.is_empty());
        assert_eq!(empty.record.observation.duration_seconds, 1800);
        assert_eq!(s.schema_version().unwrap(), STORE_SCHEMA_VERSION);
    }
}
