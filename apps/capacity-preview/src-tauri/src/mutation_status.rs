//! Read-only desktop boundary for mutation coordination and recovery state.
//!
//! The WebView receives aggregate counts and fixed enums only. It never
//! receives account, operation, rotation, key, record, process, or path data,
//! and this module exposes no mutation capability.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use capacity_mutation::{
    LegacyViewerProbe, LegacyViewerState, MutationLock, MutationLockState, SystemLegacyViewerProbe,
};
use capacity_store::VaultMutationRecoverySummary;
use serde::Serialize;

use crate::persistence::DesktopPersistence;

const MUTATION_STATUS_SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DesktopVaultMutationStatus {
    Available,
    RecoveryRequired,
    Blocked,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DesktopMutationLockState {
    Unlocked,
    Active,
    Stale,
    Unverifiable,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DesktopLegacyViewerState {
    NotRunning,
    Running,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopMutationCoordination {
    pub(crate) lock_state: DesktopMutationLockState,
    pub(crate) legacy_viewer_state: DesktopLegacyViewerState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopVaultCatalogSummary {
    pub(crate) managed_account_count: u32,
    pub(crate) pending_account_operation_count: u32,
    pub(crate) pending_key_rotation_count: u32,
    pub(crate) needs_review_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopVaultMutationStatusEnvelope {
    pub(crate) schema_version: &'static str,
    pub(crate) status: DesktopVaultMutationStatus,
    pub(crate) reason_code: &'static str,
    pub(crate) coordination: DesktopMutationCoordination,
    pub(crate) catalog: Option<DesktopVaultCatalogSummary>,
    pub(crate) mutation_commands_enabled: bool,
    pub(crate) identifiers_redacted: bool,
    pub(crate) paths_redacted: bool,
}

pub(crate) fn inspect_vault_mutation_status(
    persistence: Arc<Mutex<DesktopPersistence>>,
    lock_root: Option<PathBuf>,
) -> DesktopVaultMutationStatusEnvelope {
    let catalog = persistence
        .lock()
        .map_err(|_| "vault_store_unavailable")
        .and_then(|persistence| persistence.vault_mutation_recovery_summary());
    let lock_state = lock_root
        .ok_or("mutation_lock_root_unavailable")
        .and_then(|root| MutationLock::inspect(root).map_err(|_| "mutation_lock_inspection_failed"))
        .map(|inspection| inspection.state);
    let mut legacy_probe = SystemLegacyViewerProbe;
    build_envelope(catalog, lock_state, legacy_probe.legacy_viewer_state())
}

pub(crate) fn failed_vault_mutation_status(
    reason_code: &'static str,
) -> DesktopVaultMutationStatusEnvelope {
    DesktopVaultMutationStatusEnvelope {
        schema_version: MUTATION_STATUS_SCHEMA_VERSION,
        status: DesktopVaultMutationStatus::Failed,
        reason_code,
        coordination: DesktopMutationCoordination {
            lock_state: DesktopMutationLockState::Unavailable,
            legacy_viewer_state: DesktopLegacyViewerState::Unavailable,
        },
        catalog: None,
        mutation_commands_enabled: false,
        identifiers_redacted: true,
        paths_redacted: true,
    }
}

fn build_envelope(
    catalog: Result<VaultMutationRecoverySummary, &'static str>,
    lock_state: Result<MutationLockState, &'static str>,
    legacy_viewer_state: LegacyViewerState,
) -> DesktopVaultMutationStatusEnvelope {
    let legacy_viewer_state = DesktopLegacyViewerState::from(legacy_viewer_state);
    let coordination = DesktopMutationCoordination {
        lock_state: lock_state
            .as_ref()
            .map_or(DesktopMutationLockState::Unavailable, |state| {
                DesktopMutationLockState::from(*state)
            }),
        legacy_viewer_state,
    };
    let catalog_view = catalog.as_ref().ok().copied().map(Into::into);

    let (status, reason_code) = match (&catalog, &lock_state) {
        (Err(reason_code), _) => (DesktopVaultMutationStatus::Failed, *reason_code),
        (_, Err(reason_code)) => (DesktopVaultMutationStatus::Failed, *reason_code),
        (Ok(summary), Ok(lock_state)) => match (legacy_viewer_state, lock_state) {
            (DesktopLegacyViewerState::Running, _) => {
                (DesktopVaultMutationStatus::Blocked, "legacy_viewer_running")
            }
            (DesktopLegacyViewerState::Unavailable, _) => (
                DesktopVaultMutationStatus::Blocked,
                "legacy_viewer_state_unavailable",
            ),
            (_, MutationLockState::Active) => {
                (DesktopVaultMutationStatus::Blocked, "mutation_lock_active")
            }
            (_, MutationLockState::Stale) => (
                DesktopVaultMutationStatus::RecoveryRequired,
                "mutation_lock_stale",
            ),
            (_, MutationLockState::Unverifiable) => (
                DesktopVaultMutationStatus::RecoveryRequired,
                "mutation_lock_unverifiable",
            ),
            (_, MutationLockState::Unlocked) if summary.needs_review_count() > 0 => (
                DesktopVaultMutationStatus::RecoveryRequired,
                "vault_review_required",
            ),
            (_, MutationLockState::Unlocked) if summary.has_pending_work() => (
                DesktopVaultMutationStatus::RecoveryRequired,
                "vault_recovery_required",
            ),
            (_, MutationLockState::Unlocked) => (
                DesktopVaultMutationStatus::Available,
                "mutation_status_available",
            ),
        },
    };

    DesktopVaultMutationStatusEnvelope {
        schema_version: MUTATION_STATUS_SCHEMA_VERSION,
        status,
        reason_code,
        coordination,
        catalog: catalog_view,
        // This package exposes inspection only. User-triggered mutation
        // commands remain absent from the Tauri allowlist.
        mutation_commands_enabled: false,
        identifiers_redacted: true,
        paths_redacted: true,
    }
}

impl From<MutationLockState> for DesktopMutationLockState {
    fn from(value: MutationLockState) -> Self {
        match value {
            MutationLockState::Unlocked => Self::Unlocked,
            MutationLockState::Active => Self::Active,
            MutationLockState::Stale => Self::Stale,
            MutationLockState::Unverifiable => Self::Unverifiable,
        }
    }
}

impl From<LegacyViewerState> for DesktopLegacyViewerState {
    fn from(value: LegacyViewerState) -> Self {
        match value {
            LegacyViewerState::NotRunning => Self::NotRunning,
            LegacyViewerState::Running => Self::Running,
            LegacyViewerState::Unavailable => Self::Unavailable,
        }
    }
}

impl From<VaultMutationRecoverySummary> for DesktopVaultCatalogSummary {
    fn from(value: VaultMutationRecoverySummary) -> Self {
        Self {
            managed_account_count: value.managed_accounts,
            pending_account_operation_count: value.pending_account_operations,
            pending_key_rotation_count: value.pending_key_rotations,
            needs_review_count: value.needs_review_count(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(
        managed_accounts: u32,
        pending_account_operations: u32,
        account_operations_needing_review: u32,
        pending_key_rotations: u32,
        key_rotations_needing_review: u32,
    ) -> VaultMutationRecoverySummary {
        VaultMutationRecoverySummary {
            managed_accounts,
            pending_account_operations,
            account_operations_needing_review,
            pending_key_rotations,
            key_rotations_needing_review,
        }
    }

    #[test]
    fn recovery_envelope_matches_fixture_and_redacts_identifiers_and_paths() {
        let envelope = build_envelope(
            Ok(summary(3, 1, 1, 0, 0)),
            Ok(MutationLockState::Unlocked),
            LegacyViewerState::NotRunning,
        );
        let actual = serde_json::to_value(&envelope).expect("serialize envelope");
        let expected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../fixtures/desktop/v1/vault-mutation-recovery-envelope.json"),
            )
            .expect("read fixture"),
        )
        .expect("parse fixture");
        assert_eq!(actual, expected);

        let serialized = serde_json::to_string(&envelope).expect("serialize envelope");
        for forbidden in [
            "account:v1:",
            "vault-operation:v1:",
            "vault-key-rotation:v1:",
            "installation-key:v1:",
            "vault-record:v1:",
            "/Users/",
            "process_id",
            "owner_id",
            "fingerprint",
        ] {
            assert!(!serialized.contains(forbidden), "leaked {forbidden}");
        }
    }

    #[test]
    fn status_precedence_fails_closed_without_enabling_mutations() {
        let empty = summary(0, 0, 0, 0, 0);
        let cases = [
            (
                build_envelope(
                    Err("vault_store_unavailable"),
                    Ok(MutationLockState::Unlocked),
                    LegacyViewerState::NotRunning,
                ),
                DesktopVaultMutationStatus::Failed,
                "vault_store_unavailable",
            ),
            (
                build_envelope(
                    Ok(empty),
                    Err("mutation_lock_inspection_failed"),
                    LegacyViewerState::NotRunning,
                ),
                DesktopVaultMutationStatus::Failed,
                "mutation_lock_inspection_failed",
            ),
            (
                build_envelope(
                    Ok(empty),
                    Ok(MutationLockState::Unlocked),
                    LegacyViewerState::Running,
                ),
                DesktopVaultMutationStatus::Blocked,
                "legacy_viewer_running",
            ),
            (
                build_envelope(
                    Ok(empty),
                    Ok(MutationLockState::Unlocked),
                    LegacyViewerState::Unavailable,
                ),
                DesktopVaultMutationStatus::Blocked,
                "legacy_viewer_state_unavailable",
            ),
            (
                build_envelope(
                    Ok(empty),
                    Ok(MutationLockState::Active),
                    LegacyViewerState::NotRunning,
                ),
                DesktopVaultMutationStatus::Blocked,
                "mutation_lock_active",
            ),
            (
                build_envelope(
                    Ok(empty),
                    Ok(MutationLockState::Stale),
                    LegacyViewerState::NotRunning,
                ),
                DesktopVaultMutationStatus::RecoveryRequired,
                "mutation_lock_stale",
            ),
            (
                build_envelope(
                    Ok(empty),
                    Ok(MutationLockState::Unverifiable),
                    LegacyViewerState::NotRunning,
                ),
                DesktopVaultMutationStatus::RecoveryRequired,
                "mutation_lock_unverifiable",
            ),
            (
                build_envelope(
                    Ok(summary(1, 1, 0, 0, 0)),
                    Ok(MutationLockState::Unlocked),
                    LegacyViewerState::NotRunning,
                ),
                DesktopVaultMutationStatus::RecoveryRequired,
                "vault_recovery_required",
            ),
            (
                build_envelope(
                    Ok(summary(1, 1, 1, 0, 0)),
                    Ok(MutationLockState::Unlocked),
                    LegacyViewerState::NotRunning,
                ),
                DesktopVaultMutationStatus::RecoveryRequired,
                "vault_review_required",
            ),
            (
                build_envelope(
                    Ok(empty),
                    Ok(MutationLockState::Unlocked),
                    LegacyViewerState::NotRunning,
                ),
                DesktopVaultMutationStatus::Available,
                "mutation_status_available",
            ),
        ];

        for (envelope, status, reason_code) in cases {
            assert_eq!(envelope.status, status);
            assert_eq!(envelope.reason_code, reason_code);
            assert!(!envelope.mutation_commands_enabled);
            assert!(envelope.identifiers_redacted);
            assert!(envelope.paths_redacted);
        }
    }
}
