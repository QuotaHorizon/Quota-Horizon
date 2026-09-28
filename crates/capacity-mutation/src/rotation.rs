use std::fmt;

use capacity_domain::{
    InstallationKeyId, UtcTimestamp, VaultKeyRotation, VaultKeyRotationCheckpoint,
    VaultKeyRotationId, VaultKeyRotationStatus, VaultKeyRotationTransition, VaultValidationError,
};
use capacity_store::{
    CapacityStore, StoreError, VaultFingerprintCascadeOutcome, VaultKeyRotationAdvanceOutcome,
    VaultKeyRotationBeginOutcome,
};
use capacity_vault::{
    AuthenticatedVault, IdentityError, InstallationKey, InstallationKeyRingMutationOutcome,
    InstallationKeyRingRecord, InstallationKeyRingStore, MAX_VAULT_INVENTORY_ITEMS,
    ProtectedRecordInventoryBackend, RawAuthenticatedRecordMutationBackend, VaultBackendError,
    VaultRecordRetagOutcome,
};

use crate::{LegacyViewerProbe, MutationLock, MutationLockError};

const DEFAULT_ROTATION_RECOVERY_LIMIT: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultKeyRotationDisposition {
    Succeeded,
    Compensated,
    NeedsReview,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultKeyRotationExecutionReceipt {
    pub rotation_id: VaultKeyRotationId,
    pub disposition: VaultKeyRotationDisposition,
    pub converged_accounts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultKeyRotationExecutionItem {
    pub rotation_id: VaultKeyRotationId,
    pub disposition: VaultKeyRotationDisposition,
    pub converged_accounts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultKeyRotationRecoveryReport {
    pub scanned: u32,
    pub items: Vec<VaultKeyRotationExecutionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultKeyRotationRollbackRequest {
    pub rotation_id: VaultKeyRotationId,
    pub reason_code: String,
}

#[derive(Debug)]
pub enum LockedVaultKeyRotationError {
    Lock(MutationLockError),
    Store(StoreError),
    KeyStore(IdentityError),
    Vault(VaultBackendError),
    RotationMissing,
    RotationStateMismatch,
    RotationRevisionConflict,
    FingerprintRevisionConflict,
    FingerprintBindingConflict,
    DependencyBlocked,
    RollbackUnavailable,
    BoundExceeded,
}

impl LockedVaultKeyRotationError {
    fn reason_code(&self) -> &'static str {
        match self {
            Self::Lock(_) => "rotation_lock_lost",
            Self::Store(_) => "rotation_metadata_failed",
            Self::KeyStore(_) => "rotation_key_store_failed",
            Self::Vault(_) => "rotation_record_store_failed",
            Self::RotationMissing => "rotation_journal_missing",
            Self::RotationStateMismatch => "rotation_state_mismatch",
            Self::RotationRevisionConflict => "rotation_revision_conflict",
            Self::FingerprintRevisionConflict => "rotation_account_revision_conflict",
            Self::FingerprintBindingConflict => "rotation_account_binding_conflict",
            Self::DependencyBlocked => "rotation_dependency_blocked",
            Self::RollbackUnavailable => "rotation_rollback_unavailable",
            Self::BoundExceeded => "rotation_bound_exceeded",
        }
    }

    fn can_mark_review(&self) -> bool {
        !matches!(self, Self::Lock(_))
    }
}

impl fmt::Display for LockedVaultKeyRotationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Lock(error) => return error.fmt(formatter),
            Self::Store(_) => "key rotation metadata operation failed",
            Self::KeyStore(_) => "installation key-ring operation failed",
            Self::Vault(_) => "protected-record rotation operation failed",
            Self::RotationMissing => "key rotation journal entry is missing",
            Self::RotationStateMismatch => "key rotation state does not match its journal",
            Self::RotationRevisionConflict => "key rotation journal revision changed",
            Self::FingerprintRevisionConflict => "account changed during key rotation",
            Self::FingerprintBindingConflict => "rotated account fingerprint conflicts",
            Self::DependencyBlocked => "key retirement still has dependencies",
            Self::RollbackUnavailable => "key rotation has crossed the rollback boundary",
            Self::BoundExceeded => "key rotation exceeded a bounded collection",
        })
    }
}

impl std::error::Error for LockedVaultKeyRotationError {}

impl From<StoreError> for LockedVaultKeyRotationError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<IdentityError> for LockedVaultKeyRotationError {
    fn from(error: IdentityError) -> Self {
        Self::KeyStore(error)
    }
}

impl From<VaultBackendError> for LockedVaultKeyRotationError {
    fn from(error: VaultBackendError) -> Self {
        Self::Vault(error)
    }
}

impl MutationLock {
    /// Starts and fully converges one installation-key rotation. The durable
    /// intent is committed before key material reaches the ring; every later
    /// side effect is recoverable from the journal under this same lease.
    pub fn rotate_installation_key<K, B, P>(
        &self,
        legacy_probe: &mut P,
        metadata: &mut CapacityStore,
        key_store: &mut K,
        protected_backend: &mut B,
        changed_at: &UtcTimestamp,
    ) -> Result<VaultKeyRotationExecutionReceipt, LockedVaultKeyRotationError>
    where
        K: InstallationKeyRingStore,
        B: RawAuthenticatedRecordMutationBackend + ProtectedRecordInventoryBackend,
        P: LegacyViewerProbe,
    {
        self.verify_admission(legacy_probe)
            .map_err(LockedVaultKeyRotationError::Lock)?;
        let ring = key_store.load_or_create_key_ring()?;
        let source_key_id = InstallationKeyId::parse(ring.active_key_id().to_owned())
            .map_err(|_| LockedVaultKeyRotationError::RotationStateMismatch)?;
        let expected_key_ring_revision = ring.revision();
        let target_key = InstallationKey::generate()?;
        let target_key_id = InstallationKeyId::parse(target_key.key_id().to_owned())
            .map_err(|_| LockedVaultKeyRotationError::RotationStateMismatch)?;
        let rotation = match metadata.begin_vault_key_rotation(
            &source_key_id,
            &target_key_id,
            expected_key_ring_revision,
            changed_at,
        )? {
            VaultKeyRotationBeginOutcome::Created(rotation) => rotation,
            VaultKeyRotationBeginOutcome::Existing(rotation) => rotation,
        };
        drop(ring);

        let result = drive_forward_rotation(
            self,
            legacy_probe,
            RotationDriveContext {
                metadata,
                key_store,
                protected_backend,
                changed_at,
            },
            rotation,
            Some(target_key),
        );
        let receipt = finish_or_mark_review(self, legacy_probe, metadata, result, changed_at)?;
        self.verify_admission(legacy_probe)
            .map_err(LockedVaultKeyRotationError::Lock)?;
        Ok(receipt)
    }

    /// Resumes every active forward/rollback journal entry. Needs-review rows
    /// are reported but never silently resumed because the external condition
    /// that caused them may still be present.
    pub fn recover_installation_key_rotations<K, B, P>(
        &self,
        legacy_probe: &mut P,
        metadata: &mut CapacityStore,
        key_store: &mut K,
        protected_backend: &mut B,
        limit: u32,
        changed_at: &UtcTimestamp,
    ) -> Result<VaultKeyRotationRecoveryReport, LockedVaultKeyRotationError>
    where
        K: InstallationKeyRingStore,
        B: RawAuthenticatedRecordMutationBackend + ProtectedRecordInventoryBackend,
        P: LegacyViewerProbe,
    {
        self.verify_admission(legacy_probe)
            .map_err(LockedVaultKeyRotationError::Lock)?;
        if limit == 0 || limit > DEFAULT_ROTATION_RECOVERY_LIMIT {
            return Err(LockedVaultKeyRotationError::BoundExceeded);
        }
        let rotations = metadata.recoverable_vault_key_rotations(limit)?;
        let scanned = u32::try_from(rotations.len())
            .map_err(|_| LockedVaultKeyRotationError::BoundExceeded)?;
        let mut items = Vec::with_capacity(rotations.len());
        for rotation in rotations {
            self.verify_admission(legacy_probe)
                .map_err(LockedVaultKeyRotationError::Lock)?;
            if rotation.status == VaultKeyRotationStatus::NeedsReview {
                items.push(VaultKeyRotationExecutionItem {
                    rotation_id: rotation.rotation_id,
                    disposition: VaultKeyRotationDisposition::NeedsReview,
                    converged_accounts: 0,
                });
                continue;
            }
            let result = match rotation.status {
                VaultKeyRotationStatus::InProgress => drive_forward_rotation(
                    self,
                    legacy_probe,
                    RotationDriveContext {
                        metadata,
                        key_store,
                        protected_backend,
                        changed_at,
                    },
                    rotation,
                    None,
                ),
                VaultKeyRotationStatus::RollingBack => drive_rollback_rotation(
                    self,
                    legacy_probe,
                    RotationDriveContext {
                        metadata,
                        key_store,
                        protected_backend,
                        changed_at,
                    },
                    rotation,
                ),
                VaultKeyRotationStatus::Succeeded | VaultKeyRotationStatus::Compensated => {
                    return Err(LockedVaultKeyRotationError::RotationStateMismatch);
                }
                VaultKeyRotationStatus::NeedsReview => unreachable!(),
            };
            let receipt = finish_or_mark_review(self, legacy_probe, metadata, result, changed_at)?;
            items.push(VaultKeyRotationExecutionItem {
                rotation_id: receipt.rotation_id,
                disposition: receipt.disposition,
                converged_accounts: receipt.converged_accounts,
            });
        }
        self.verify_admission(legacy_probe)
            .map_err(LockedVaultKeyRotationError::Lock)?;
        Ok(VaultKeyRotationRecoveryReport { scanned, items })
    }

    /// Explicitly resumes one needs-review rotation after the caller has
    /// resolved its bounded reason code.
    pub fn resume_installation_key_rotation<K, B, P>(
        &self,
        legacy_probe: &mut P,
        metadata: &mut CapacityStore,
        key_store: &mut K,
        protected_backend: &mut B,
        rotation_id: &VaultKeyRotationId,
        changed_at: &UtcTimestamp,
    ) -> Result<VaultKeyRotationExecutionReceipt, LockedVaultKeyRotationError>
    where
        K: InstallationKeyRingStore,
        B: RawAuthenticatedRecordMutationBackend + ProtectedRecordInventoryBackend,
        P: LegacyViewerProbe,
    {
        self.verify_admission(legacy_probe)
            .map_err(LockedVaultKeyRotationError::Lock)?;
        let current = metadata
            .vault_key_rotation(rotation_id)?
            .ok_or(LockedVaultKeyRotationError::RotationMissing)?;
        if current.status != VaultKeyRotationStatus::NeedsReview {
            return Err(LockedVaultKeyRotationError::RotationStateMismatch);
        }
        let resumed = advance_rotation(
            metadata,
            &current,
            VaultKeyRotationTransition::Resume,
            changed_at,
        )?;
        let result = match resumed.status {
            VaultKeyRotationStatus::InProgress => drive_forward_rotation(
                self,
                legacy_probe,
                RotationDriveContext {
                    metadata,
                    key_store,
                    protected_backend,
                    changed_at,
                },
                resumed,
                None,
            ),
            VaultKeyRotationStatus::RollingBack => drive_rollback_rotation(
                self,
                legacy_probe,
                RotationDriveContext {
                    metadata,
                    key_store,
                    protected_backend,
                    changed_at,
                },
                resumed,
            ),
            _ => Err(LockedVaultKeyRotationError::RotationStateMismatch),
        };
        finish_or_mark_review(self, legacy_probe, metadata, result, changed_at)
    }

    /// Begins or resumes an explicit rollback while the predecessor is still
    /// provably present. A crash after physical predecessor retirement is
    /// recognized even if the journal transition was not yet committed.
    pub fn rollback_installation_key_rotation<K, B, P>(
        &self,
        legacy_probe: &mut P,
        metadata: &mut CapacityStore,
        key_store: &mut K,
        protected_backend: &mut B,
        request: &VaultKeyRotationRollbackRequest,
        changed_at: &UtcTimestamp,
    ) -> Result<VaultKeyRotationExecutionReceipt, LockedVaultKeyRotationError>
    where
        K: InstallationKeyRingStore,
        B: RawAuthenticatedRecordMutationBackend + ProtectedRecordInventoryBackend,
        P: LegacyViewerProbe,
    {
        self.verify_admission(legacy_probe)
            .map_err(LockedVaultKeyRotationError::Lock)?;
        let mut current = metadata
            .vault_key_rotation(&request.rotation_id)?
            .ok_or(LockedVaultKeyRotationError::RotationMissing)?;
        if current.status == VaultKeyRotationStatus::NeedsReview {
            let preview = current
                .apply_transition(VaultKeyRotationTransition::Resume, changed_at.clone())
                .map_err(rotation_validation_error)?;
            if preview.status == VaultKeyRotationStatus::InProgress {
                ensure_rollback_available(key_store, &preview)?;
                preview
                    .apply_transition(
                        VaultKeyRotationTransition::BeginRollback {
                            reason_code: request.reason_code.clone(),
                        },
                        changed_at.clone(),
                    )
                    .map_err(rotation_validation_error)?;
            }
            current = advance_rotation(
                metadata,
                &current,
                VaultKeyRotationTransition::Resume,
                changed_at,
            )?;
        }
        if current.status == VaultKeyRotationStatus::InProgress {
            ensure_rollback_available(key_store, &current)?;
            current = advance_rotation(
                metadata,
                &current,
                VaultKeyRotationTransition::BeginRollback {
                    reason_code: request.reason_code.clone(),
                },
                changed_at,
            )?;
        }
        if current.status != VaultKeyRotationStatus::RollingBack {
            return Err(LockedVaultKeyRotationError::RotationStateMismatch);
        }
        let result = drive_rollback_rotation(
            self,
            legacy_probe,
            RotationDriveContext {
                metadata,
                key_store,
                protected_backend,
                changed_at,
            },
            current,
        );
        finish_or_mark_review(self, legacy_probe, metadata, result, changed_at)
    }
}

fn rotation_validation_error(error: VaultValidationError) -> LockedVaultKeyRotationError {
    LockedVaultKeyRotationError::Store(StoreError::VaultValidation(error))
}

struct RotationDriveContext<'a, K, B> {
    metadata: &'a mut CapacityStore,
    key_store: &'a mut K,
    protected_backend: &'a mut B,
    changed_at: &'a UtcTimestamp,
}

fn drive_forward_rotation<K, B, P>(
    lock: &MutationLock,
    legacy_probe: &mut P,
    context: RotationDriveContext<'_, K, B>,
    mut rotation: VaultKeyRotation,
    mut prepared_target_key: Option<InstallationKey>,
) -> Result<VaultKeyRotationExecutionReceipt, LockedVaultKeyRotationError>
where
    K: InstallationKeyRingStore,
    B: RawAuthenticatedRecordMutationBackend + ProtectedRecordInventoryBackend,
    P: LegacyViewerProbe,
{
    if rotation.status != VaultKeyRotationStatus::InProgress {
        return Err(LockedVaultKeyRotationError::RotationStateMismatch);
    }
    let mut converged_accounts = 0_u32;
    loop {
        lock.verify_admission(legacy_probe)
            .map_err(LockedVaultKeyRotationError::Lock)?;
        rotation = match rotation.checkpoint {
            VaultKeyRotationCheckpoint::Prepared => {
                ensure_rotation_started(context.key_store, &rotation, &mut prepared_target_key)?;
                advance_rotation(
                    context.metadata,
                    &rotation,
                    VaultKeyRotationTransition::KeyRingStarted,
                    context.changed_at,
                )?
            }
            VaultKeyRotationCheckpoint::KeyRingStarted => {
                converged_accounts = converged_accounts
                    .checked_add(converge_accounts(
                        context.metadata,
                        context.key_store,
                        context.protected_backend,
                        &rotation,
                        RotationDirection::Forward,
                        context.changed_at,
                    )?)
                    .ok_or(LockedVaultKeyRotationError::BoundExceeded)?;
                advance_rotation(
                    context.metadata,
                    &rotation,
                    VaultKeyRotationTransition::AccountsMigrated,
                    context.changed_at,
                )?
            }
            VaultKeyRotationCheckpoint::AccountsMigrated => {
                audit_retirement_dependencies(
                    context.metadata,
                    context.key_store,
                    context.protected_backend,
                    &rotation,
                    &rotation.source_key_id,
                )?;
                advance_rotation(
                    context.metadata,
                    &rotation,
                    VaultKeyRotationTransition::DependenciesCleared,
                    context.changed_at,
                )?
            }
            VaultKeyRotationCheckpoint::DependenciesCleared => {
                ensure_forward_predecessor_retired(context.key_store, &rotation)?;
                advance_rotation(
                    context.metadata,
                    &rotation,
                    VaultKeyRotationTransition::PredecessorRetired,
                    context.changed_at,
                )?
            }
            VaultKeyRotationCheckpoint::PredecessorRetired => {
                let completed = advance_rotation(
                    context.metadata,
                    &rotation,
                    VaultKeyRotationTransition::Succeeded,
                    context.changed_at,
                )?;
                lock.verify_admission(legacy_probe)
                    .map_err(LockedVaultKeyRotationError::Lock)?;
                return Ok(VaultKeyRotationExecutionReceipt {
                    rotation_id: completed.rotation_id,
                    disposition: VaultKeyRotationDisposition::Succeeded,
                    converged_accounts,
                });
            }
            VaultKeyRotationCheckpoint::RollbackStarted
            | VaultKeyRotationCheckpoint::AccountsRestored
            | VaultKeyRotationCheckpoint::NewKeyRetired => {
                return Err(LockedVaultKeyRotationError::RotationStateMismatch);
            }
        };
    }
}

fn drive_rollback_rotation<K, B, P>(
    lock: &MutationLock,
    legacy_probe: &mut P,
    context: RotationDriveContext<'_, K, B>,
    mut rotation: VaultKeyRotation,
) -> Result<VaultKeyRotationExecutionReceipt, LockedVaultKeyRotationError>
where
    K: InstallationKeyRingStore,
    B: RawAuthenticatedRecordMutationBackend + ProtectedRecordInventoryBackend,
    P: LegacyViewerProbe,
{
    if rotation.status != VaultKeyRotationStatus::RollingBack {
        return Err(LockedVaultKeyRotationError::RotationStateMismatch);
    }
    let mut converged_accounts = 0_u32;
    loop {
        lock.verify_admission(legacy_probe)
            .map_err(LockedVaultKeyRotationError::Lock)?;
        rotation = match rotation.checkpoint {
            VaultKeyRotationCheckpoint::RollbackStarted => {
                ensure_rollback_source_active(context.key_store, &rotation)?;
                converged_accounts = converged_accounts
                    .checked_add(converge_accounts(
                        context.metadata,
                        context.key_store,
                        context.protected_backend,
                        &rotation,
                        RotationDirection::Rollback,
                        context.changed_at,
                    )?)
                    .ok_or(LockedVaultKeyRotationError::BoundExceeded)?;
                audit_retirement_dependencies(
                    context.metadata,
                    context.key_store,
                    context.protected_backend,
                    &rotation,
                    &rotation.target_key_id,
                )?;
                advance_rotation(
                    context.metadata,
                    &rotation,
                    VaultKeyRotationTransition::AccountsRestored,
                    context.changed_at,
                )?
            }
            VaultKeyRotationCheckpoint::AccountsRestored => {
                ensure_rollback_target_retired(context.key_store, &rotation)?;
                advance_rotation(
                    context.metadata,
                    &rotation,
                    VaultKeyRotationTransition::NewKeyRetired,
                    context.changed_at,
                )?
            }
            VaultKeyRotationCheckpoint::NewKeyRetired => {
                let completed = advance_rotation(
                    context.metadata,
                    &rotation,
                    VaultKeyRotationTransition::Compensated,
                    context.changed_at,
                )?;
                lock.verify_admission(legacy_probe)
                    .map_err(LockedVaultKeyRotationError::Lock)?;
                return Ok(VaultKeyRotationExecutionReceipt {
                    rotation_id: completed.rotation_id,
                    disposition: VaultKeyRotationDisposition::Compensated,
                    converged_accounts,
                });
            }
            VaultKeyRotationCheckpoint::Prepared
            | VaultKeyRotationCheckpoint::KeyRingStarted
            | VaultKeyRotationCheckpoint::AccountsMigrated
            | VaultKeyRotationCheckpoint::DependenciesCleared
            | VaultKeyRotationCheckpoint::PredecessorRetired => {
                return Err(LockedVaultKeyRotationError::RotationStateMismatch);
            }
        };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RingPhase {
    Original,
    Rotating,
    ForwardRetired,
    RollbackPromoted,
    RollbackRetired,
}

fn classify_ring(
    ring: &InstallationKeyRingRecord,
    rotation: &VaultKeyRotation,
) -> Result<RingPhase, LockedVaultKeyRotationError> {
    let revision = ring.revision();
    let expected = rotation.expected_key_ring_revision;
    let forward_revision = expected
        .checked_add(1)
        .ok_or(LockedVaultKeyRotationError::RotationStateMismatch)?;
    let terminal_revision = expected
        .checked_add(2)
        .ok_or(LockedVaultKeyRotationError::RotationStateMismatch)?;
    let rollback_terminal_revision = expected
        .checked_add(3)
        .ok_or(LockedVaultKeyRotationError::RotationStateMismatch)?;
    let source = rotation.source_key_id.as_str();
    let target = rotation.target_key_id.as_str();
    let verification = ring.verification_key_ids().collect::<Vec<_>>();
    let source_present = verification.contains(&source);
    let target_present = verification.contains(&target);

    match (revision, ring.active_key_id()) {
        (value, active)
            if value == expected && active == source && !source_present && !target_present =>
        {
            Ok(RingPhase::Original)
        }
        (value, active) if value == forward_revision && active == target && source_present => {
            Ok(RingPhase::Rotating)
        }
        (value, active) if value == terminal_revision && active == target && !source_present => {
            Ok(RingPhase::ForwardRetired)
        }
        (value, active) if value == terminal_revision && active == source && target_present => {
            Ok(RingPhase::RollbackPromoted)
        }
        (value, active)
            if value == rollback_terminal_revision && active == source && !target_present =>
        {
            Ok(RingPhase::RollbackRetired)
        }
        _ => Err(LockedVaultKeyRotationError::RotationStateMismatch),
    }
}

fn load_ring<K: InstallationKeyRingStore>(
    key_store: &mut K,
) -> Result<InstallationKeyRingRecord, LockedVaultKeyRotationError> {
    key_store
        .load_key_ring()?
        .ok_or(LockedVaultKeyRotationError::RotationStateMismatch)
}

fn ensure_rotation_started<K: InstallationKeyRingStore>(
    key_store: &mut K,
    rotation: &VaultKeyRotation,
    prepared_target_key: &mut Option<InstallationKey>,
) -> Result<(), LockedVaultKeyRotationError> {
    let current = load_ring(key_store)?;
    match classify_ring(&current, rotation)? {
        RingPhase::Original => {
            let expected = current.revision_token();
            let target_key = match prepared_target_key.take() {
                Some(key) if key.key_id() == rotation.target_key_id.as_str() => key,
                Some(_) => return Err(LockedVaultKeyRotationError::RotationStateMismatch),
                None => InstallationKey::generate_for_id(rotation.target_key_id.as_str())?,
            };
            let replacement = current.start_rotation(target_key)?;
            match key_store.replace_key_ring(&expected, &replacement)? {
                InstallationKeyRingMutationOutcome::Applied => Ok(()),
                InstallationKeyRingMutationOutcome::RevisionConflict => {
                    ensure_ring_phase(key_store, rotation, RingPhase::Rotating)
                }
                InstallationKeyRingMutationOutcome::Missing => {
                    Err(LockedVaultKeyRotationError::RotationStateMismatch)
                }
            }
        }
        RingPhase::Rotating => Ok(()),
        RingPhase::ForwardRetired | RingPhase::RollbackPromoted | RingPhase::RollbackRetired => {
            Err(LockedVaultKeyRotationError::RotationStateMismatch)
        }
    }
}

fn ensure_forward_predecessor_retired<K: InstallationKeyRingStore>(
    key_store: &mut K,
    rotation: &VaultKeyRotation,
) -> Result<(), LockedVaultKeyRotationError> {
    let current = load_ring(key_store)?;
    match classify_ring(&current, rotation)? {
        RingPhase::Rotating => {
            let expected = current.revision_token();
            let replacement = current.retire_verification_key(rotation.source_key_id.as_str())?;
            match key_store.replace_key_ring(&expected, &replacement)? {
                InstallationKeyRingMutationOutcome::Applied => Ok(()),
                InstallationKeyRingMutationOutcome::RevisionConflict => {
                    ensure_ring_phase(key_store, rotation, RingPhase::ForwardRetired)
                }
                InstallationKeyRingMutationOutcome::Missing => {
                    Err(LockedVaultKeyRotationError::RotationStateMismatch)
                }
            }
        }
        RingPhase::ForwardRetired => Ok(()),
        RingPhase::Original | RingPhase::RollbackPromoted | RingPhase::RollbackRetired => {
            Err(LockedVaultKeyRotationError::RotationStateMismatch)
        }
    }
}

fn ensure_rollback_source_active<K: InstallationKeyRingStore>(
    key_store: &mut K,
    rotation: &VaultKeyRotation,
) -> Result<(), LockedVaultKeyRotationError> {
    let current = load_ring(key_store)?;
    match classify_ring(&current, rotation)? {
        RingPhase::Original | RingPhase::RollbackPromoted | RingPhase::RollbackRetired => Ok(()),
        RingPhase::Rotating => {
            let expected = current.revision_token();
            let replacement = current.promote_verification_key(rotation.source_key_id.as_str())?;
            match key_store.replace_key_ring(&expected, &replacement)? {
                InstallationKeyRingMutationOutcome::Applied => Ok(()),
                InstallationKeyRingMutationOutcome::RevisionConflict => {
                    ensure_ring_phase(key_store, rotation, RingPhase::RollbackPromoted)
                }
                InstallationKeyRingMutationOutcome::Missing => {
                    Err(LockedVaultKeyRotationError::RotationStateMismatch)
                }
            }
        }
        RingPhase::ForwardRetired => Err(LockedVaultKeyRotationError::RollbackUnavailable),
    }
}

fn ensure_rollback_available<K: InstallationKeyRingStore>(
    key_store: &mut K,
    rotation: &VaultKeyRotation,
) -> Result<(), LockedVaultKeyRotationError> {
    match classify_ring(&load_ring(key_store)?, rotation)? {
        RingPhase::Original | RingPhase::Rotating => Ok(()),
        RingPhase::ForwardRetired => Err(LockedVaultKeyRotationError::RollbackUnavailable),
        RingPhase::RollbackPromoted | RingPhase::RollbackRetired => {
            Err(LockedVaultKeyRotationError::RotationStateMismatch)
        }
    }
}

fn ensure_rollback_target_retired<K: InstallationKeyRingStore>(
    key_store: &mut K,
    rotation: &VaultKeyRotation,
) -> Result<(), LockedVaultKeyRotationError> {
    let current = load_ring(key_store)?;
    match classify_ring(&current, rotation)? {
        RingPhase::Original | RingPhase::RollbackRetired => Ok(()),
        RingPhase::RollbackPromoted => {
            let expected = current.revision_token();
            let replacement = current.retire_verification_key(rotation.target_key_id.as_str())?;
            match key_store.replace_key_ring(&expected, &replacement)? {
                InstallationKeyRingMutationOutcome::Applied => Ok(()),
                InstallationKeyRingMutationOutcome::RevisionConflict => {
                    ensure_ring_phase(key_store, rotation, RingPhase::RollbackRetired)
                }
                InstallationKeyRingMutationOutcome::Missing => {
                    Err(LockedVaultKeyRotationError::RotationStateMismatch)
                }
            }
        }
        RingPhase::Rotating | RingPhase::ForwardRetired => {
            Err(LockedVaultKeyRotationError::RotationStateMismatch)
        }
    }
}

fn ensure_ring_phase<K: InstallationKeyRingStore>(
    key_store: &mut K,
    rotation: &VaultKeyRotation,
    expected: RingPhase,
) -> Result<(), LockedVaultKeyRotationError> {
    let current = load_ring(key_store)?;
    if classify_ring(&current, rotation)? == expected {
        Ok(())
    } else {
        Err(LockedVaultKeyRotationError::RotationStateMismatch)
    }
}

#[derive(Debug, Clone, Copy)]
enum RotationDirection {
    Forward,
    Rollback,
}

fn converge_accounts<K, B>(
    metadata: &mut CapacityStore,
    key_store: &mut K,
    protected_backend: &mut B,
    rotation: &VaultKeyRotation,
    direction: RotationDirection,
    changed_at: &UtcTimestamp,
) -> Result<u32, LockedVaultKeyRotationError>
where
    K: InstallationKeyRingStore,
    B: RawAuthenticatedRecordMutationBackend + ProtectedRecordInventoryBackend,
{
    let ring_record = load_ring(key_store)?;
    let phase = classify_ring(&ring_record, rotation)?;
    let (from_key, to_key, valid_phase) = match direction {
        RotationDirection::Forward => (
            &rotation.source_key_id,
            &rotation.target_key_id,
            phase == RingPhase::Rotating,
        ),
        RotationDirection::Rollback => (
            &rotation.target_key_id,
            &rotation.source_key_id,
            matches!(phase, RingPhase::Original | RingPhase::RollbackPromoted),
        ),
    };
    if !valid_phase {
        return Err(LockedVaultKeyRotationError::RotationStateMismatch);
    }
    let mut vault = AuthenticatedVault::new(protected_backend, ring_record.into_key_ring());
    let accounts = metadata.vault_accounts()?;
    let mut converged = 0_u32;
    for account in accounts {
        let current_key_id = account.account_fingerprint.key_id();
        if current_key_id != from_key.as_str() && current_key_id != to_key.as_str() {
            continue;
        }
        let record = vault.read_record_in_current_state(&account.protected_record_ref)?;
        let replacement = vault
            .account_fingerprint_with_key_id(&record, to_key.as_str())
            .map_err(LockedVaultKeyRotationError::KeyStore)?;
        if current_key_id == to_key.as_str() && account.account_fingerprint != replacement {
            return Err(LockedVaultKeyRotationError::RotationStateMismatch);
        }
        match vault.retag_record_with_active_key(&account.protected_record_ref)? {
            VaultRecordRetagOutcome::Applied | VaultRecordRetagOutcome::AlreadyApplied => {}
            VaultRecordRetagOutcome::RevisionConflict => {
                return Err(LockedVaultKeyRotationError::FingerprintRevisionConflict);
            }
        }
        if current_key_id == from_key.as_str() {
            match metadata.cascade_vault_account_fingerprint(&account, &replacement, changed_at)? {
                VaultFingerprintCascadeOutcome::Updated(_)
                | VaultFingerprintCascadeOutcome::AlreadyApplied(_) => {}
                VaultFingerprintCascadeOutcome::RevisionConflict(_) => {
                    return Err(LockedVaultKeyRotationError::FingerprintRevisionConflict);
                }
                VaultFingerprintCascadeOutcome::BindingConflict => {
                    return Err(LockedVaultKeyRotationError::FingerprintBindingConflict);
                }
                VaultFingerprintCascadeOutcome::Missing => {
                    return Err(LockedVaultKeyRotationError::FingerprintRevisionConflict);
                }
            }
            converged = converged
                .checked_add(1)
                .ok_or(LockedVaultKeyRotationError::BoundExceeded)?;
        }
    }
    Ok(converged)
}

fn audit_retirement_dependencies<K, B>(
    metadata: &CapacityStore,
    key_store: &mut K,
    protected_backend: &mut B,
    rotation: &VaultKeyRotation,
    retiring_key: &InstallationKeyId,
) -> Result<(), LockedVaultKeyRotationError>
where
    K: InstallationKeyRingStore,
    B: RawAuthenticatedRecordMutationBackend + ProtectedRecordInventoryBackend,
{
    let metadata_audit =
        metadata.vault_key_dependency_audit_for_rotation(retiring_key, &rotation.rotation_id)?;
    if metadata_audit.has_dependencies() {
        return Err(LockedVaultKeyRotationError::DependencyBlocked);
    }
    let ring_record = load_ring(key_store)?;
    let retiring_key_available = ring_record
        .verification_key_ids()
        .chain(std::iter::once(ring_record.active_key_id()))
        .any(|key_id| key_id == retiring_key.as_str());
    let mut vault = AuthenticatedVault::new(protected_backend, ring_record.into_key_ring());
    let record_audit = if retiring_key_available {
        vault.record_tag_dependency_audit(retiring_key.as_str(), MAX_VAULT_INVENTORY_ITEMS)?
    } else {
        vault.record_tag_reference_audit(retiring_key.as_str(), MAX_VAULT_INVENTORY_ITEMS)?
    };
    if record_audit.has_retirement_blockers() {
        return Err(LockedVaultKeyRotationError::DependencyBlocked);
    }
    Ok(())
}

fn advance_rotation(
    metadata: &mut CapacityStore,
    current: &VaultKeyRotation,
    transition: VaultKeyRotationTransition,
    changed_at: &UtcTimestamp,
) -> Result<VaultKeyRotation, LockedVaultKeyRotationError> {
    match metadata.advance_vault_key_rotation(
        &current.rotation_id,
        current.revision,
        transition,
        changed_at,
    )? {
        VaultKeyRotationAdvanceOutcome::Updated(rotation) => Ok(rotation),
        VaultKeyRotationAdvanceOutcome::RevisionConflict(_) => {
            Err(LockedVaultKeyRotationError::RotationRevisionConflict)
        }
        VaultKeyRotationAdvanceOutcome::Missing => {
            Err(LockedVaultKeyRotationError::RotationMissing)
        }
    }
}

fn finish_or_mark_review<P: LegacyViewerProbe>(
    lock: &MutationLock,
    legacy_probe: &mut P,
    metadata: &mut CapacityStore,
    result: Result<VaultKeyRotationExecutionReceipt, LockedVaultKeyRotationError>,
    changed_at: &UtcTimestamp,
) -> Result<VaultKeyRotationExecutionReceipt, LockedVaultKeyRotationError> {
    let error = match result {
        Ok(receipt) => return Ok(receipt),
        Err(error) => error,
    };
    if error.can_mark_review()
        && lock.verify_admission(legacy_probe).is_ok()
        && let Ok(rotations) = metadata.recoverable_vault_key_rotations(1)
        && let Some(rotation) = rotations.into_iter().next()
        && matches!(
            rotation.status,
            VaultKeyRotationStatus::InProgress | VaultKeyRotationStatus::RollingBack
        )
    {
        let _ = advance_rotation(
            metadata,
            &rotation,
            VaultKeyRotationTransition::NeedsReview {
                reason_code: error.reason_code().to_owned(),
            },
            changed_at,
        );
    }
    Err(error)
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use capacity_domain::{
        UtcTimestamp, VaultAccountAuthMode, VaultAccountLifecycle, VaultAccountSource,
        VaultKeyRotationStatus, VaultRecordRef,
    };
    use capacity_store::{CapacityStore, VaultAccountRegistrationOutcome};
    use capacity_vault::{
        AccountRegistrationDraft, AuthenticatedVault, FileInstallationKeyStore, FileVault,
        ProtectedRecordBackend, ProtectedVaultRecord,
    };

    use super::*;
    use crate::{
        LegacyViewerState, MutationLockOwner, MutationOwnerId, OWNER_ID_PREFIX,
        PRIVATE_DIRECTORY_MODE,
    };

    const OWNER_UUID: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73101";
    const OWNER_UUID_2: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73102";
    const RECORD_UUID: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73103";
    const RECORD_UUID_2: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73104";
    const ROTATION_CHILD_MODE: &str = "CAPACITY_MUTATION_ROTATION_CHILD";
    const ROTATION_CHILD_BASE: &str = "CAPACITY_MUTATION_ROTATION_BASE";
    const ROTATION_CHILD_SCENARIO: &str = "CAPACITY_MUTATION_ROTATION_SCENARIO";
    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "capacity-rotation-test-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            builder.mode(PRIVATE_DIRECTORY_MODE);
            builder.create(&path).expect("create test root");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            if self
                .0
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("capacity-rotation-test-"))
                && self.0.starts_with(std::env::temp_dir())
            {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }

    #[derive(Default)]
    struct FakeLegacyProbe {
        states: VecDeque<LegacyViewerState>,
    }

    impl LegacyViewerProbe for FakeLegacyProbe {
        fn legacy_viewer_state(&mut self) -> LegacyViewerState {
            self.states
                .pop_front()
                .unwrap_or(LegacyViewerState::NotRunning)
        }
    }

    struct ChildGuard(Option<Child>);

    impl ChildGuard {
        fn kill_and_wait(&mut self) {
            if let Some(mut child) = self.0.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            self.kill_and_wait();
        }
    }

    fn timestamp(value: &str) -> UtcTimestamp {
        UtcTimestamp::parse(value).expect("timestamp")
    }

    fn owner(uuid: &str) -> MutationLockOwner {
        MutationLockOwner::new(
            MutationOwnerId::parse(format!("{OWNER_ID_PREFIX}{uuid}")).expect("owner id"),
            std::process::id(),
            timestamp("2026-08-30T14:00:00Z"),
        )
        .expect("owner")
    }

    fn protected_record() -> ProtectedVaultRecord {
        ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_account_id",
            b"rotation-person@example.invalid".to_vec(),
            br#"{"auth_mode":"chatgpt","access_token":"rotation-secret-canary"}"#.to_vec(),
            None,
        )
        .expect("protected record")
    }

    fn record_ref() -> VaultRecordRef {
        VaultRecordRef::parse(format!("vault-record:v1:{RECORD_UUID}")).expect("record ref")
    }

    fn quarantined_record_ref() -> VaultRecordRef {
        VaultRecordRef::parse(format!("vault-record:v1:{RECORD_UUID_2}"))
            .expect("quarantined record ref")
    }

    struct Fixture {
        metadata: CapacityStore,
        key_store: FileInstallationKeyStore,
        backend: FileVault,
        source_key_id: InstallationKeyId,
    }

    fn fixture(base: &Path) -> Fixture {
        let mut metadata = CapacityStore::open(base.join("capacity.sqlite3")).expect("metadata");
        let key_store = FileInstallationKeyStore::open_explicit_fallback(base.join("key-store"))
            .expect("key store");
        let ring = key_store.load_or_create_key_ring().expect("initial ring");
        let source_key_id =
            InstallationKeyId::parse(ring.active_key_id().to_owned()).expect("source key id");
        let mut backend = FileVault::open(base.join("vault")).expect("vault");
        let record = protected_record();
        let draft = AccountRegistrationDraft {
            protected_record_ref: record_ref(),
            display_name: "Primary ChatGPT".to_owned(),
            auth_mode: VaultAccountAuthMode::ChatGpt,
            source: VaultAccountSource::ManualChatGpt,
            lifecycle: VaultAccountLifecycle::Ready,
            provider_id: None,
            model: None,
        };
        let mut authenticated = AuthenticatedVault::new(&mut backend, ring.into_key_ring());
        let registration = authenticated
            .bind_registration(&draft, &record)
            .expect("bind registration");
        authenticated
            .create_record(&draft.protected_record_ref, &record)
            .expect("create protected record");
        drop(authenticated);
        assert!(matches!(
            metadata
                .register_vault_account(&registration, &timestamp("2026-08-30T14:00:00Z"))
                .expect("register metadata"),
            VaultAccountRegistrationOutcome::Created(_)
        ));
        Fixture {
            metadata,
            key_store,
            backend,
            source_key_id,
        }
    }

    fn begin_rotation(fixture: &mut Fixture) -> (VaultKeyRotation, InstallationKey) {
        let ring = fixture
            .key_store
            .load_key_ring()
            .expect("load ring")
            .expect("ring exists");
        let target = InstallationKey::generate().expect("target key");
        let target_id = InstallationKeyId::parse(target.key_id().to_owned()).expect("target id");
        let rotation = match fixture
            .metadata
            .begin_vault_key_rotation(
                &fixture.source_key_id,
                &target_id,
                ring.revision(),
                &timestamp("2026-08-30T14:01:00Z"),
            )
            .expect("begin rotation")
        {
            VaultKeyRotationBeginOutcome::Created(rotation) => rotation,
            VaultKeyRotationBeginOutcome::Existing(_) => panic!("unexpected existing rotation"),
        };
        (rotation, target)
    }

    fn advance(
        fixture: &mut Fixture,
        rotation: &VaultKeyRotation,
        transition: VaultKeyRotationTransition,
    ) -> VaultKeyRotation {
        advance_rotation(
            &mut fixture.metadata,
            rotation,
            transition,
            &timestamp("2026-08-30T14:02:00Z"),
        )
        .expect("advance rotation")
    }

    fn assert_final_state(
        fixture: &mut Fixture,
        rotation: &VaultKeyRotation,
        expected_status: VaultKeyRotationStatus,
    ) {
        let persisted = fixture
            .metadata
            .vault_key_rotation(&rotation.rotation_id)
            .expect("read rotation")
            .expect("rotation exists");
        assert_eq!(persisted.status, expected_status);
        let ring = fixture
            .key_store
            .load_key_ring()
            .expect("load final ring")
            .expect("final ring");
        let expected_key = if expected_status == VaultKeyRotationStatus::Succeeded {
            &rotation.target_key_id
        } else {
            &rotation.source_key_id
        };
        let retired_key = if expected_status == VaultKeyRotationStatus::Succeeded {
            &rotation.source_key_id
        } else {
            &rotation.target_key_id
        };
        assert_eq!(ring.active_key_id(), expected_key.as_str());
        assert!(
            !ring
                .verification_key_ids()
                .any(|key_id| key_id == retired_key.as_str())
        );
        let accounts = fixture.metadata.vault_accounts().expect("accounts");
        assert_eq!(accounts.len(), 1);
        assert_eq!(
            accounts[0].account_fingerprint.key_id(),
            expected_key.as_str()
        );
        let mut authenticated = AuthenticatedVault::new(&mut fixture.backend, ring.into_key_ring());
        assert!(authenticated.read_record(&record_ref()).is_ok());
    }

    #[test]
    fn locked_forward_rotation_converges_metadata_record_and_ring() {
        let root = TestRoot::new();
        let mut fixture = fixture(root.path());
        let mut probe = FakeLegacyProbe::default();
        let lock = MutationLock::acquire(root.path().join("lock"), owner(OWNER_UUID), &mut probe)
            .expect("lock");
        let receipt = lock
            .rotate_installation_key(
                &mut probe,
                &mut fixture.metadata,
                &mut fixture.key_store,
                &mut fixture.backend,
                &timestamp("2026-08-30T14:03:00Z"),
            )
            .expect("forward rotation");
        assert_eq!(receipt.disposition, VaultKeyRotationDisposition::Succeeded);
        assert_eq!(receipt.converged_accounts, 1);
        let rotation = fixture
            .metadata
            .vault_key_rotation(&receipt.rotation_id)
            .unwrap()
            .unwrap();
        assert_final_state(&mut fixture, &rotation, VaultKeyRotationStatus::Succeeded);
    }

    #[test]
    fn forward_rotation_converges_multiple_live_and_quarantined_accounts() {
        let root = TestRoot::new();
        let mut fixture = fixture(root.path());
        let ring = fixture
            .key_store
            .load_key_ring()
            .unwrap()
            .expect("source ring");
        let second_record = ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_account_id",
            b"quarantined-rotation-person@example.invalid".to_vec(),
            br#"{"auth_mode":"chatgpt","access_token":"quarantined-secret-canary"}"#.to_vec(),
            None,
        )
        .expect("second record");
        let second_draft = AccountRegistrationDraft {
            protected_record_ref: quarantined_record_ref(),
            display_name: "Saved ChatGPT".to_owned(),
            auth_mode: VaultAccountAuthMode::ChatGpt,
            source: VaultAccountSource::ManualChatGpt,
            lifecycle: VaultAccountLifecycle::Ready,
            provider_id: None,
            model: None,
        };
        let mut authenticated = AuthenticatedVault::new(&mut fixture.backend, ring.into_key_ring());
        let second_registration = authenticated
            .bind_registration(&second_draft, &second_record)
            .expect("bind second account");
        authenticated
            .create_record(&second_draft.protected_record_ref, &second_record)
            .expect("create second record");
        authenticated
            .quarantine_record(&second_draft.protected_record_ref)
            .expect("quarantine second record");
        drop(authenticated);
        assert!(matches!(
            fixture
                .metadata
                .register_vault_account(&second_registration, &timestamp("2026-08-30T14:00:01Z"))
                .expect("register second account"),
            VaultAccountRegistrationOutcome::Created(_)
        ));

        let mut probe = FakeLegacyProbe::default();
        let lock = MutationLock::acquire(root.path().join("lock"), owner(OWNER_UUID), &mut probe)
            .expect("lock");
        let receipt = lock
            .rotate_installation_key(
                &mut probe,
                &mut fixture.metadata,
                &mut fixture.key_store,
                &mut fixture.backend,
                &timestamp("2026-08-30T14:03:00Z"),
            )
            .expect("multi-account rotation");
        assert_eq!(receipt.converged_accounts, 2);
        let rotation = fixture
            .metadata
            .vault_key_rotation(&receipt.rotation_id)
            .unwrap()
            .unwrap();
        let accounts = fixture.metadata.vault_accounts().expect("accounts");
        assert_eq!(accounts.len(), 2);
        assert!(accounts.iter().all(|account| {
            account.account_fingerprint.key_id() == rotation.target_key_id.as_str()
        }));
        assert_eq!(
            fixture
                .backend
                .record_state(&quarantined_record_ref())
                .expect("quarantined state"),
            capacity_vault::VaultRecordState::Quarantined
        );
        let ring = fixture
            .key_store
            .load_key_ring()
            .unwrap()
            .expect("final ring");
        let mut authenticated = AuthenticatedVault::new(&mut fixture.backend, ring.into_key_ring());
        assert!(
            authenticated
                .read_record_in_current_state(&quarantined_record_ref())
                .is_ok()
        );
    }

    #[test]
    fn prepared_recovery_recreates_only_unpersisted_target_material() {
        let root = TestRoot::new();
        let mut fixture = fixture(root.path());
        let (rotation, target) = begin_rotation(&mut fixture);
        let target_id = target.key_id().to_owned();
        drop(target);
        let mut probe = FakeLegacyProbe::default();
        let lock = MutationLock::acquire(root.path().join("lock"), owner(OWNER_UUID), &mut probe)
            .expect("lock");
        let report = lock
            .recover_installation_key_rotations(
                &mut probe,
                &mut fixture.metadata,
                &mut fixture.key_store,
                &mut fixture.backend,
                1,
                &timestamp("2026-08-30T14:04:00Z"),
            )
            .expect("recover prepared");
        assert_eq!(report.scanned, 1);
        assert_eq!(
            report.items[0].disposition,
            VaultKeyRotationDisposition::Succeeded
        );
        assert_eq!(rotation.target_key_id.as_str(), target_id);
        assert_final_state(&mut fixture, &rotation, VaultKeyRotationStatus::Succeeded);
    }

    #[test]
    fn explicit_rollback_restores_fingerprint_tag_and_source_key() {
        let root = TestRoot::new();
        let mut fixture = fixture(root.path());
        let (mut rotation, target) = begin_rotation(&mut fixture);
        ensure_rotation_started(&mut fixture.key_store, &rotation, &mut Some(target))
            .expect("start ring");
        rotation = advance(
            &mut fixture,
            &rotation,
            VaultKeyRotationTransition::KeyRingStarted,
        );
        assert_eq!(
            converge_accounts(
                &mut fixture.metadata,
                &mut fixture.key_store,
                &mut fixture.backend,
                &rotation,
                RotationDirection::Forward,
                &timestamp("2026-08-30T14:02:00Z"),
            )
            .expect("forward accounts"),
            1
        );
        rotation = advance(
            &mut fixture,
            &rotation,
            VaultKeyRotationTransition::AccountsMigrated,
        );
        let mut probe = FakeLegacyProbe::default();
        let lock = MutationLock::acquire(root.path().join("lock"), owner(OWNER_UUID), &mut probe)
            .expect("lock");
        let receipt = lock
            .rollback_installation_key_rotation(
                &mut probe,
                &mut fixture.metadata,
                &mut fixture.key_store,
                &mut fixture.backend,
                &VaultKeyRotationRollbackRequest {
                    rotation_id: rotation.rotation_id.clone(),
                    reason_code: "operator_requested_rollback".to_owned(),
                },
                &timestamp("2026-08-30T14:05:00Z"),
            )
            .expect("rollback");
        assert_eq!(
            receipt.disposition,
            VaultKeyRotationDisposition::Compensated
        );
        assert_eq!(receipt.converged_accounts, 1);
        assert_final_state(&mut fixture, &rotation, VaultKeyRotationStatus::Compensated);
    }

    #[test]
    fn prepared_rollback_never_creates_the_journaled_target_key() {
        let root = TestRoot::new();
        let mut fixture = fixture(root.path());
        let (rotation, target) = begin_rotation(&mut fixture);
        drop(target);
        let original_revision = rotation.expected_key_ring_revision;
        let mut probe = FakeLegacyProbe::default();
        let lock = MutationLock::acquire(root.path().join("lock"), owner(OWNER_UUID), &mut probe)
            .expect("lock");
        let receipt = lock
            .rollback_installation_key_rotation(
                &mut probe,
                &mut fixture.metadata,
                &mut fixture.key_store,
                &mut fixture.backend,
                &VaultKeyRotationRollbackRequest {
                    rotation_id: rotation.rotation_id.clone(),
                    reason_code: "operator_cancelled_prepared_rotation".to_owned(),
                },
                &timestamp("2026-08-30T14:05:00Z"),
            )
            .expect("prepared rollback");
        assert_eq!(
            receipt.disposition,
            VaultKeyRotationDisposition::Compensated
        );
        assert_eq!(receipt.converged_accounts, 0);
        let ring = fixture.key_store.load_key_ring().unwrap().expect("ring");
        assert_eq!(ring.revision(), original_revision);
        assert_eq!(ring.active_key_id(), rotation.source_key_id.as_str());
        assert!(
            !ring
                .verification_key_ids()
                .any(|key_id| key_id == rotation.target_key_id.as_str())
        );
        assert_final_state(&mut fixture, &rotation, VaultKeyRotationStatus::Compensated);
    }

    #[test]
    fn staging_blocker_marks_review_and_explicit_resume_finishes() {
        let root = TestRoot::new();
        let mut fixture = fixture(root.path());
        let residue = root
            .path()
            .join("vault/staging-v1")
            .join(format!(".staging-{RECORD_UUID}-1-1-0"));
        let mut builder = fs::DirBuilder::new();
        builder.mode(PRIVATE_DIRECTORY_MODE);
        builder.create(&residue).expect("create staging residue");
        let mut probe = FakeLegacyProbe::default();
        let lock = MutationLock::acquire(root.path().join("lock"), owner(OWNER_UUID), &mut probe)
            .expect("lock");
        let error = lock
            .rotate_installation_key(
                &mut probe,
                &mut fixture.metadata,
                &mut fixture.key_store,
                &mut fixture.backend,
                &timestamp("2026-08-30T14:03:00Z"),
            )
            .expect_err("staging must block retirement");
        assert!(matches!(
            error,
            LockedVaultKeyRotationError::DependencyBlocked
        ));
        let pending = fixture
            .metadata
            .recoverable_vault_key_rotations(1)
            .expect("pending rotation");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, VaultKeyRotationStatus::NeedsReview);
        assert_eq!(
            pending[0].checkpoint,
            VaultKeyRotationCheckpoint::AccountsMigrated
        );
        let ring = fixture
            .key_store
            .load_key_ring()
            .unwrap()
            .expect("rotating ring");
        assert_eq!(ring.active_key_id(), pending[0].target_key_id.as_str());
        assert!(
            ring.verification_key_ids()
                .any(|key_id| key_id == pending[0].source_key_id.as_str())
        );

        assert!(matches!(
            lock.rollback_installation_key_rotation(
                &mut probe,
                &mut fixture.metadata,
                &mut fixture.key_store,
                &mut fixture.backend,
                &VaultKeyRotationRollbackRequest {
                    rotation_id: pending[0].rotation_id.clone(),
                    reason_code: "INVALID REASON".to_owned(),
                },
                &timestamp("2026-08-30T14:03:30Z"),
            ),
            Err(LockedVaultKeyRotationError::Store(
                StoreError::VaultValidation(_)
            ))
        ));
        assert_eq!(
            fixture
                .metadata
                .vault_key_rotation(&pending[0].rotation_id)
                .unwrap()
                .unwrap()
                .status,
            VaultKeyRotationStatus::NeedsReview,
            "invalid rollback request must not consume the review gate"
        );

        fs::remove_dir(&residue).expect("resolve staging residue");
        let receipt = lock
            .resume_installation_key_rotation(
                &mut probe,
                &mut fixture.metadata,
                &mut fixture.key_store,
                &mut fixture.backend,
                &pending[0].rotation_id,
                &timestamp("2026-08-30T14:04:00Z"),
            )
            .expect("resume after explicit resolution");
        assert_eq!(receipt.disposition, VaultKeyRotationDisposition::Succeeded);
        assert_final_state(&mut fixture, &pending[0], VaultKeyRotationStatus::Succeeded);
    }

    fn prepare_checkpoint(scenario: &str, fixture: &mut Fixture) -> VaultKeyRotation {
        let (mut rotation, target) = begin_rotation(fixture);
        if scenario == "prepared" {
            drop(target);
            return rotation;
        }
        ensure_rotation_started(&mut fixture.key_store, &rotation, &mut Some(target))
            .expect("start ring");
        rotation = advance(
            fixture,
            &rotation,
            VaultKeyRotationTransition::KeyRingStarted,
        );
        if scenario == "key_ring_started" {
            return rotation;
        }
        converge_accounts(
            &mut fixture.metadata,
            &mut fixture.key_store,
            &mut fixture.backend,
            &rotation,
            RotationDirection::Forward,
            &timestamp("2026-08-30T14:02:00Z"),
        )
        .expect("converge forward accounts");
        rotation = advance(
            fixture,
            &rotation,
            VaultKeyRotationTransition::AccountsMigrated,
        );
        if scenario == "accounts_migrated" {
            return rotation;
        }
        if scenario.starts_with("rollback_") {
            rotation = advance(
                fixture,
                &rotation,
                VaultKeyRotationTransition::BeginRollback {
                    reason_code: "kill_matrix_rollback".to_owned(),
                },
            );
            if scenario == "rollback_started" {
                return rotation;
            }
            ensure_rollback_source_active(&mut fixture.key_store, &rotation)
                .expect("promote source");
            converge_accounts(
                &mut fixture.metadata,
                &mut fixture.key_store,
                &mut fixture.backend,
                &rotation,
                RotationDirection::Rollback,
                &timestamp("2026-08-30T14:02:00Z"),
            )
            .expect("restore accounts");
            audit_retirement_dependencies(
                &fixture.metadata,
                &mut fixture.key_store,
                &mut fixture.backend,
                &rotation,
                &rotation.target_key_id,
            )
            .expect("audit rollback");
            rotation = advance(
                fixture,
                &rotation,
                VaultKeyRotationTransition::AccountsRestored,
            );
            if scenario == "rollback_accounts_restored" {
                return rotation;
            }
            assert_eq!(scenario, "rollback_new_key_retired");
            ensure_rollback_target_retired(&mut fixture.key_store, &rotation)
                .expect("retire target");
            return advance(
                fixture,
                &rotation,
                VaultKeyRotationTransition::NewKeyRetired,
            );
        }
        audit_retirement_dependencies(
            &fixture.metadata,
            &mut fixture.key_store,
            &mut fixture.backend,
            &rotation,
            &rotation.source_key_id,
        )
        .expect("audit forward");
        rotation = advance(
            fixture,
            &rotation,
            VaultKeyRotationTransition::DependenciesCleared,
        );
        if scenario == "dependencies_cleared" {
            return rotation;
        }
        assert_eq!(scenario, "predecessor_retired");
        ensure_forward_predecessor_retired(&mut fixture.key_store, &rotation)
            .expect("retire predecessor");
        advance(
            fixture,
            &rotation,
            VaultKeyRotationTransition::PredecessorRetired,
        )
    }

    fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("condition did not become true before timeout");
    }

    #[test]
    fn subprocess_rotation_checkpoint_holder() {
        if std::env::var_os(ROTATION_CHILD_MODE).is_none() {
            return;
        }
        let base = PathBuf::from(std::env::var_os(ROTATION_CHILD_BASE).expect("child base"));
        let scenario = std::env::var(ROTATION_CHILD_SCENARIO).expect("child scenario");
        let mut fixture = fixture(&base);
        let _lock = MutationLock::acquire(
            base.join("lock"),
            owner(OWNER_UUID),
            &mut FakeLegacyProbe::default(),
        )
        .expect("child lock");
        let _ = prepare_checkpoint(&scenario, &mut fixture);
        fs::write(base.join("rotation-ready"), b"ready\n").expect("ready marker");
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }

    #[test]
    fn real_process_kill_matrix_recovers_every_rotation_checkpoint() {
        if std::env::var_os(ROTATION_CHILD_MODE).is_some() {
            return;
        }
        let scenarios = [
            ("prepared", VaultKeyRotationStatus::Succeeded),
            ("key_ring_started", VaultKeyRotationStatus::Succeeded),
            ("accounts_migrated", VaultKeyRotationStatus::Succeeded),
            ("dependencies_cleared", VaultKeyRotationStatus::Succeeded),
            ("predecessor_retired", VaultKeyRotationStatus::Succeeded),
            ("rollback_started", VaultKeyRotationStatus::Compensated),
            (
                "rollback_accounts_restored",
                VaultKeyRotationStatus::Compensated,
            ),
            (
                "rollback_new_key_retired",
                VaultKeyRotationStatus::Compensated,
            ),
        ];
        let test_executable = std::env::current_exe().expect("test executable");
        for (scenario, expected_status) in scenarios {
            let root = TestRoot::new();
            let child = Command::new(&test_executable)
                .args([
                    "--exact",
                    "rotation::tests::subprocess_rotation_checkpoint_holder",
                    "--nocapture",
                ])
                .env(ROTATION_CHILD_MODE, "1")
                .env(ROTATION_CHILD_BASE, root.path())
                .env(ROTATION_CHILD_SCENARIO, scenario)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn rotation child");
            let mut child = ChildGuard(Some(child));
            wait_until(Duration::from_secs(5), || {
                root.path().join("rotation-ready").exists()
            });
            child.kill_and_wait();
            wait_until(Duration::from_secs(5), || {
                MutationLock::inspect(root.path().join("lock"))
                    .is_ok_and(|inspection| inspection.state == crate::MutationLockState::Stale)
            });

            let mut fixture = Fixture {
                metadata: CapacityStore::open(root.path().join("capacity.sqlite3"))
                    .expect("recovery metadata"),
                key_store: FileInstallationKeyStore::open_explicit_fallback(
                    root.path().join("key-store"),
                )
                .expect("recovery key store"),
                backend: FileVault::open(root.path().join("vault")).expect("recovery vault"),
                source_key_id: InstallationKeyId::parse("018f47a2-8a71-7f4a-9c35-1f4234a73199")
                    .unwrap(),
            };
            let pending = fixture
                .metadata
                .recoverable_vault_key_rotations(1)
                .expect("pending rotation");
            assert_eq!(pending.len(), 1, "scenario={scenario}");
            let rotation = pending[0].clone();
            let mut probe = FakeLegacyProbe::default();
            let replacement =
                MutationLock::acquire(root.path().join("lock"), owner(OWNER_UUID_2), &mut probe)
                    .expect("replacement lock");
            let report = replacement
                .recover_installation_key_rotations(
                    &mut probe,
                    &mut fixture.metadata,
                    &mut fixture.key_store,
                    &mut fixture.backend,
                    1,
                    &timestamp("2026-08-30T14:10:00Z"),
                )
                .expect("recover rotation");
            assert_eq!(report.scanned, 1, "scenario={scenario}");
            assert_eq!(report.items.len(), 1, "scenario={scenario}");
            assert_final_state(&mut fixture, &rotation, expected_status);
        }
    }

    #[test]
    fn error_and_debug_surfaces_do_not_expose_ids_or_secret_canaries() {
        let target = InstallationKey::generate_for_id("018f47a2-8a71-7f4a-9c35-1f4234a73188")
            .expect("fixed-id key");
        assert_eq!(target.key_id(), "018f47a2-8a71-7f4a-9c35-1f4234a73188");
        assert!(InstallationKey::generate_for_id("ROTATION-SECRET-CANARY").is_err());
        let rendered = format!(
            "{:?} {}",
            LockedVaultKeyRotationError::RotationStateMismatch,
            LockedVaultKeyRotationError::RotationStateMismatch
        );
        assert!(!rendered.contains("rotation-secret-canary"));
        assert!(!rendered.contains(RECORD_UUID));
    }
}
