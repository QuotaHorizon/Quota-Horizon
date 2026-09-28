//! Public, account-free views of earlier explicit plans and activity. Selecting
//! the stable account and resolving source resources belongs to the host/store.
use crate::{
    UtcTimestamp, active_time::ActiveTimeObservation, activity_quota::ActivityQuotaComparison,
    demand::CapacityDemandInput,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacityWorkPlan {
    pub work_plan_id: String,
    pub revision: u32,
    pub demand_id: String,
    pub demand_revision: u32,
    pub enabled: bool,
    pub demand: CapacityDemandInput,
    pub created_at: UtcTimestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PlanningArchiveRef {
    Plan(String),
    Record(String),
    Timer(String),
    Trial(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PlanningArchiveQuery {
    List {
        offset: u32,
    },
    Details {
        source: PlanningArchiveRef,
        offset: u32,
    },
    Quota {
        source: PlanningArchiveRef,
        observation_id: String,
    },
    Pace {
        source: PlanningArchiveRef,
        offset: u32,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanningArchiveEntry {
    pub source: PlanningArchiveRef,
    pub platform: String,
    pub architecture: String,
    pub boundary: String,
    pub codex_version: Option<String>,
    pub last_recorded_at: UtcTimestamp,
    pub plan_revisions: u64,
    pub record_count: u64,
    pub trial_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanningArchiveDraft {
    pub started_at: UtcTimestamp,
    pub last_checkpoint: UtcTimestamp,
    pub suggested_seconds: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PlanningArchiveResult {
    List {
        entries: Vec<PlanningArchiveEntry>,
        has_more: bool,
    },
    Details {
        plans: Vec<CapacityWorkPlan>,
        records: Vec<ActiveTimeObservation>,
        draft: Option<PlanningArchiveDraft>,
        has_more: bool,
    },
    Quota {
        record: ActiveTimeObservation,
        comparison: ActivityQuotaComparison,
    },
    Pace {
        trials: Vec<crate::pace_trial::PaceTrialView>,
        has_more: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_contract_accepts_only_typed_resource_refs_not_scope_overrides() {
        for text in [
            r#"{"kind":"list","offset":0,"environmentId":"other"}"#,
            r#"{"kind":"list","offset":-1}"#,
            r#"{"kind":"details","offset":0,"source":{"kind":"record","id":"id","account":"other"}}"#,
        ] {
            assert!(serde_json::from_str::<PlanningArchiveQuery>(text).is_err());
        }
        let query = PlanningArchiveQuery::Quota {
            source: PlanningArchiveRef::Record("anchor".into()),
            observation_id: "record".into(),
        };
        let json = serde_json::to_value(&query).unwrap();
        assert_eq!(json["observationId"], "record");
        assert_eq!(
            serde_json::from_value::<PlanningArchiveQuery>(json).unwrap(),
            query
        );
        let result = PlanningArchiveResult::Details {
            plans: vec![],
            records: vec![],
            draft: None,
            has_more: false,
        };
        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["hasMore"], false);
    }
}
