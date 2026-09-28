use capacity_domain::{
    UtcTimestamp, VaultAccount, VaultAccountRegistration, VaultOperation, VaultOperationCheckpoint,
    VaultOperationId, VaultOperationKind, VaultOperationStatus, VaultOperationTransition,
};
use capacity_store::{
    CapacityStore, StoreError, VaultAccountRegistrationOutcome, VaultAccountRemovalOutcome,
    VaultOperationAdvanceOutcome, VaultOperationBeginOutcome,
};

use crate::orchestrator::{ensure_live_record, metadata_failure_reason};
use crate::{
    AccountRegistrationReceipt, AccountVaultError, CompensationOutcome, ForgetAccountOutcome,
    MetadataFailureReason, ProtectedRecordBackend, ProtectedVaultRecord, VaultRecordState,
    VaultTransitionOutcome,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournaledAccountRegistrationReceipt {
    pub operation_id: VaultOperationId,
    pub receipt: AccountRegistrationReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournaledForgetAccountReceipt {
    pub operation_id: Option<VaultOperationId>,
    pub outcome: ForgetAccountOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultRecoveryAction {
    RetryRegistrationWithOriginalRequest,
    ResumeForget,
    FinalizeSucceeded,
    FinalizeCompensated,
    ManualReview,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultRecoveryItem {
    pub operation_id: VaultOperationId,
    pub kind: VaultOperationKind,
    pub checkpoint: VaultOperationCheckpoint,
    pub action: VaultRecoveryAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultRecoveryReport {
    pub scanned: u32,
    pub items: Vec<VaultRecoveryItem>,
}

/// Registration entry point whose intent and every completed side-effect
/// boundary are persisted before success is returned. An exact retry supplies
/// the original protected request again, allowing record bytes to be verified
/// without ever copying them into the journal.
pub fn journaled_register_account<B: ProtectedRecordBackend>(
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    registration: &VaultAccountRegistration,
    protected_record: &ProtectedVaultRecord,
    changed_at: &UtcTimestamp,
) -> Result<JournaledAccountRegistrationReceipt, AccountVaultError> {
    registration
        .validate()
        .map_err(|_| AccountVaultError::InvalidRegistration)?;
    let mut operation = begin_registration_operation(metadata, registration, changed_at)?;

    if operation.status == VaultOperationStatus::NeedsReview {
        operation = advance_operation(
            metadata,
            &operation,
            VaultOperationTransition::Resume,
            changed_at,
        )?;
    }

    match operation.checkpoint {
        VaultOperationCheckpoint::MetadataCommitted => {
            return finish_committed_registration(
                metadata,
                protected_backend,
                registration,
                protected_record,
                operation,
                changed_at,
            );
        }
        VaultOperationCheckpoint::RecordQuarantined => {
            ensure_registration_compensation_postcondition(
                metadata,
                protected_backend,
                &operation,
                changed_at,
            )?;
            let operation = advance_operation(
                metadata,
                &operation,
                VaultOperationTransition::Compensated {
                    reason_code: "metadata_registration_failed".into(),
                },
                changed_at,
            )?;
            return Err(AccountVaultError::MetadataRegistrationFailed {
                reason: MetadataFailureReason::Unavailable,
                compensation: if operation.status == VaultOperationStatus::Compensated {
                    CompensationOutcome::Quarantined
                } else {
                    CompensationOutcome::Failed
                },
            });
        }
        VaultOperationCheckpoint::Prepared | VaultOperationCheckpoint::RecordReady => {}
        VaultOperationCheckpoint::MetadataRemoved | VaultOperationCheckpoint::RecordRestored => {
            return Err(AccountVaultError::OperationJournalUnavailable);
        }
    }

    let protected_record_created = match ensure_live_record(
        protected_backend,
        &registration.protected_record_ref,
        protected_record,
    ) {
        Ok(created) => created,
        Err(error) => {
            pause_operation(
                metadata,
                &operation,
                registration_error_code(&error),
                changed_at,
            );
            return Err(error);
        }
    };
    if operation.checkpoint == VaultOperationCheckpoint::Prepared {
        operation = advance_operation(
            metadata,
            &operation,
            VaultOperationTransition::RecordReady,
            changed_at,
        )?;
    }

    let registration_outcome = metadata.register_vault_account(registration, changed_at);
    let (account, metadata_created) = match registration_outcome {
        Ok(VaultAccountRegistrationOutcome::Created(account)) => (account, true),
        Ok(VaultAccountRegistrationOutcome::Existing(account)) => (account, false),
        Err(error) => {
            let reason = metadata_failure_reason(&error);
            match metadata.vault_account_by_fingerprint(&registration.account_fingerprint) {
                Ok(Some(account)) if registration_matches_account(registration, &account) => {
                    (account, false)
                }
                Ok(_) | Err(_) => {
                    let compensation = compensate_registration(
                        metadata,
                        protected_backend,
                        &operation,
                        protected_record_created,
                        reason,
                        changed_at,
                    )?;
                    return Err(AccountVaultError::MetadataRegistrationFailed {
                        reason,
                        compensation,
                    });
                }
            }
        }
    };

    if !registration_matches_account(registration, &account) {
        let compensation = compensate_registration(
            metadata,
            protected_backend,
            &operation,
            protected_record_created,
            MetadataFailureReason::IdentityRecordConflict,
            changed_at,
        )?;
        return Err(AccountVaultError::MetadataRegistrationFailed {
            reason: MetadataFailureReason::IdentityRecordConflict,
            compensation,
        });
    }

    operation = advance_operation(
        metadata,
        &operation,
        VaultOperationTransition::MetadataCommitted {
            account_id: account.account_id.clone(),
        },
        changed_at,
    )?;
    let operation = advance_operation(
        metadata,
        &operation,
        VaultOperationTransition::Succeeded,
        changed_at,
    )?;
    Ok(JournaledAccountRegistrationReceipt {
        operation_id: operation.operation_id,
        receipt: AccountRegistrationReceipt {
            account,
            metadata_created,
            protected_record_created,
        },
    })
}

/// Quarantine-first forget entry point with durable checkpoints. Missing
/// metadata plus a missing record is an already-complete no-op and does not
/// manufacture a journal row; a quarantined recovery record is reconciled
/// through the normal journal path.
pub fn journaled_forget_account<B: ProtectedRecordBackend>(
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    expected_account: &VaultAccount,
    changed_at: &UtcTimestamp,
) -> Result<JournaledForgetAccountReceipt, AccountVaultError> {
    let current = metadata
        .vault_account(&expected_account.account_id)
        .map_err(|_| AccountVaultError::MetadataLookupFailed)?;
    if let Some(current) = current.as_ref() {
        if !forget_snapshot_matches(expected_account, current) {
            return Err(AccountVaultError::MetadataSnapshotStale);
        }
    } else {
        match protected_backend
            .record_state(&expected_account.protected_record_ref)
            .map_err(AccountVaultError::ProtectedBackend)?
        {
            VaultRecordState::Missing => {
                return Ok(JournaledForgetAccountReceipt {
                    operation_id: None,
                    outcome: ForgetAccountOutcome::AlreadyForgotten {
                        recovery_record_present: false,
                    },
                });
            }
            VaultRecordState::Quarantined => {}
            VaultRecordState::Present => return Err(AccountVaultError::OrphanedProtectedRecord),
            VaultRecordState::Conflict => {
                return Err(AccountVaultError::ProtectedRecordState(
                    VaultRecordState::Conflict,
                ));
            }
        }
    }

    let mut operation = begin_forget_operation(metadata, expected_account, changed_at)?;
    if operation.status == VaultOperationStatus::NeedsReview {
        operation = advance_operation(
            metadata,
            &operation,
            VaultOperationTransition::Resume,
            changed_at,
        )?;
    }

    match operation.checkpoint {
        VaultOperationCheckpoint::MetadataRemoved => {
            ensure_no_record_owner(metadata, &operation)?;
            let recovery_record_present = recovery_record_present(protected_backend, &operation)?;
            let operation = advance_operation(
                metadata,
                &operation,
                VaultOperationTransition::Succeeded,
                changed_at,
            )?;
            return Ok(JournaledForgetAccountReceipt {
                operation_id: Some(operation.operation_id),
                outcome: ForgetAccountOutcome::AlreadyForgotten {
                    recovery_record_present,
                },
            });
        }
        VaultOperationCheckpoint::RecordRestored => {
            ensure_forget_compensation_postcondition(
                metadata,
                protected_backend,
                &operation,
                changed_at,
            )?;
            advance_operation(
                metadata,
                &operation,
                VaultOperationTransition::Compensated {
                    reason_code: "metadata_revision_conflict".into(),
                },
                changed_at,
            )?;
            return Err(AccountVaultError::MetadataRemovalFailed {
                reason: MetadataFailureReason::RevisionConflict,
                compensation: CompensationOutcome::Restored,
            });
        }
        VaultOperationCheckpoint::Prepared | VaultOperationCheckpoint::RecordQuarantined => {}
        VaultOperationCheckpoint::RecordReady | VaultOperationCheckpoint::MetadataCommitted => {
            return Err(AccountVaultError::OperationJournalUnavailable);
        }
    }

    let mut record_was_already_quarantined = true;
    if operation.checkpoint == VaultOperationCheckpoint::Prepared {
        record_was_already_quarantined = match protected_backend
            .record_state(&operation.protected_record_ref)
            .map_err(AccountVaultError::ProtectedBackend)?
        {
            VaultRecordState::Present => false,
            VaultRecordState::Quarantined => true,
            state @ (VaultRecordState::Missing | VaultRecordState::Conflict) => {
                pause_operation(
                    metadata,
                    &operation,
                    "forget_record_unavailable",
                    changed_at,
                );
                return Err(AccountVaultError::ProtectedRecordState(state));
            }
        };
        let transition = match protected_backend.quarantine_record(&operation.protected_record_ref)
        {
            Ok(transition) => transition,
            Err(error) => {
                pause_operation(metadata, &operation, "protected_backend_failed", changed_at);
                return Err(AccountVaultError::ProtectedBackend(error));
            }
        };
        record_was_already_quarantined = record_was_already_quarantined
            || transition == VaultTransitionOutcome::AlreadyAtDestination;
        operation = advance_operation(
            metadata,
            &operation,
            VaultOperationTransition::RecordQuarantined,
            changed_at,
        )?;
    } else if let Err(error) = protected_backend.quarantine_record(&operation.protected_record_ref)
    {
        pause_operation(metadata, &operation, "protected_backend_failed", changed_at);
        return Err(AccountVaultError::ProtectedBackend(error));
    }

    let account_id = operation
        .account_id
        .as_ref()
        .ok_or(AccountVaultError::OperationJournalUnavailable)?;
    let expected_revision = operation
        .expected_account_revision
        .ok_or(AccountVaultError::OperationJournalUnavailable)?;
    match metadata.remove_vault_account_metadata(account_id, expected_revision, changed_at) {
        Ok(VaultAccountRemovalOutcome::Removed(removal)) => {
            recovery_record_present(protected_backend, &operation)?;
            let operation = finish_forget_success(metadata, operation, changed_at)?;
            Ok(JournaledForgetAccountReceipt {
                operation_id: Some(operation.operation_id),
                outcome: ForgetAccountOutcome::Forgotten {
                    removal: Box::new(removal),
                    record_was_already_quarantined,
                },
            })
        }
        Ok(VaultAccountRemovalOutcome::Missing) => {
            finish_missing_forget(metadata, protected_backend, operation, changed_at)
        }
        Ok(VaultAccountRemovalOutcome::RevisionConflict(_)) => compensate_forget(
            metadata,
            protected_backend,
            operation,
            MetadataFailureReason::RevisionConflict,
            changed_at,
        ),
        Err(error) => {
            let reason = metadata_failure_reason(&error);
            match metadata.vault_account(account_id) {
                Ok(None) => {
                    finish_missing_forget(metadata, protected_backend, operation, changed_at)
                }
                Ok(Some(_)) | Err(_) => {
                    compensate_forget(metadata, protected_backend, operation, reason, changed_at)
                }
            }
        }
    }
}

/// Read-only startup classification. It never receives protected bytes and
/// never mutates the backend or journal. A later driver may execute only the
/// explicitly returned action under the global mutation lock.
pub fn inspect_vault_recovery<B: ProtectedRecordBackend>(
    metadata: &CapacityStore,
    protected_backend: &mut B,
    limit: u32,
) -> Result<VaultRecoveryReport, AccountVaultError> {
    let operations = metadata
        .recoverable_vault_operations(limit)
        .map_err(journal_store_error)?;
    let mut items = Vec::with_capacity(operations.len());
    for operation in operations {
        let action = classify_recovery(metadata, protected_backend, &operation)?;
        items.push(VaultRecoveryItem {
            operation_id: operation.operation_id,
            kind: operation.kind,
            checkpoint: operation.checkpoint,
            action,
        });
    }
    Ok(VaultRecoveryReport {
        scanned: u32::try_from(items.len())
            .map_err(|_| AccountVaultError::OperationJournalUnavailable)?,
        items,
    })
}

fn begin_registration_operation(
    metadata: &mut CapacityStore,
    registration: &VaultAccountRegistration,
    changed_at: &UtcTimestamp,
) -> Result<VaultOperation, AccountVaultError> {
    match metadata
        .begin_vault_registration_operation(registration, changed_at)
        .map_err(journal_store_error)?
    {
        VaultOperationBeginOutcome::Created(operation)
        | VaultOperationBeginOutcome::Existing(operation) => Ok(operation),
    }
}

fn begin_forget_operation(
    metadata: &mut CapacityStore,
    account: &VaultAccount,
    changed_at: &UtcTimestamp,
) -> Result<VaultOperation, AccountVaultError> {
    match metadata
        .begin_vault_forget_operation(account, changed_at)
        .map_err(journal_store_error)?
    {
        VaultOperationBeginOutcome::Created(operation)
        | VaultOperationBeginOutcome::Existing(operation) => Ok(operation),
    }
}

fn advance_operation(
    metadata: &mut CapacityStore,
    operation: &VaultOperation,
    transition: VaultOperationTransition,
    changed_at: &UtcTimestamp,
) -> Result<VaultOperation, AccountVaultError> {
    match metadata
        .advance_vault_operation(
            &operation.operation_id,
            operation.revision,
            transition,
            changed_at,
        )
        .map_err(journal_store_error)?
    {
        VaultOperationAdvanceOutcome::Updated(operation) => Ok(operation),
        VaultOperationAdvanceOutcome::RevisionConflict(_)
        | VaultOperationAdvanceOutcome::Missing => {
            Err(AccountVaultError::OperationJournalUnavailable)
        }
    }
}

fn finish_committed_registration<B: ProtectedRecordBackend>(
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    registration: &VaultAccountRegistration,
    protected_record: &ProtectedVaultRecord,
    operation: VaultOperation,
    changed_at: &UtcTimestamp,
) -> Result<JournaledAccountRegistrationReceipt, AccountVaultError> {
    let account_id = operation
        .account_id
        .as_ref()
        .ok_or(AccountVaultError::OperationJournalUnavailable)?;
    let account = metadata
        .vault_account(account_id)
        .map_err(|_| AccountVaultError::MetadataLookupFailed)?
        .ok_or(AccountVaultError::MetadataLookupFailed)?;
    if !registration_matches_account(registration, &account) {
        pause_operation(
            metadata,
            &operation,
            "metadata_identity_conflict",
            changed_at,
        );
        return Err(AccountVaultError::MetadataRegistrationFailed {
            reason: MetadataFailureReason::IdentityRecordConflict,
            compensation: CompensationOutcome::NotRequired,
        });
    }
    if let Err(error) = ensure_live_record(
        protected_backend,
        &registration.protected_record_ref,
        protected_record,
    ) {
        pause_operation(
            metadata,
            &operation,
            registration_error_code(&error),
            changed_at,
        );
        return Err(error);
    }
    let operation = advance_operation(
        metadata,
        &operation,
        VaultOperationTransition::Succeeded,
        changed_at,
    )?;
    Ok(JournaledAccountRegistrationReceipt {
        operation_id: operation.operation_id,
        receipt: AccountRegistrationReceipt {
            account,
            metadata_created: false,
            protected_record_created: false,
        },
    })
}

fn compensate_registration<B: ProtectedRecordBackend>(
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    operation: &VaultOperation,
    protected_record_created: bool,
    reason: MetadataFailureReason,
    changed_at: &UtcTimestamp,
) -> Result<CompensationOutcome, AccountVaultError> {
    if !protected_record_created {
        pause_operation(
            metadata,
            operation,
            metadata_reason_code(reason),
            changed_at,
        );
        return Ok(CompensationOutcome::NotRequired);
    }
    match metadata.vault_account_by_record_ref(&operation.protected_record_ref) {
        Ok(Some(_)) => {
            pause_operation(metadata, operation, "record_claimed", changed_at);
            return Ok(CompensationOutcome::NotRequired);
        }
        Err(_) => {
            pause_operation(metadata, operation, "metadata_lookup_failed", changed_at);
            return Ok(CompensationOutcome::Failed);
        }
        Ok(None) => {}
    }
    match protected_backend.quarantine_record(&operation.protected_record_ref) {
        Ok(VaultTransitionOutcome::Moved | VaultTransitionOutcome::AlreadyAtDestination) => {
            let operation = advance_operation(
                metadata,
                operation,
                VaultOperationTransition::RecordQuarantined,
                changed_at,
            )?;
            advance_operation(
                metadata,
                &operation,
                VaultOperationTransition::Compensated {
                    reason_code: metadata_reason_code(reason).into(),
                },
                changed_at,
            )?;
            Ok(CompensationOutcome::Quarantined)
        }
        Err(_) => {
            pause_operation(metadata, operation, "protected_backend_failed", changed_at);
            Ok(CompensationOutcome::Failed)
        }
    }
}

fn finish_forget_success(
    metadata: &mut CapacityStore,
    operation: VaultOperation,
    changed_at: &UtcTimestamp,
) -> Result<VaultOperation, AccountVaultError> {
    let operation = advance_operation(
        metadata,
        &operation,
        VaultOperationTransition::MetadataRemoved,
        changed_at,
    )?;
    advance_operation(
        metadata,
        &operation,
        VaultOperationTransition::Succeeded,
        changed_at,
    )
}

fn finish_missing_forget<B: ProtectedRecordBackend>(
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    operation: VaultOperation,
    changed_at: &UtcTimestamp,
) -> Result<JournaledForgetAccountReceipt, AccountVaultError> {
    match metadata.vault_account_by_record_ref(&operation.protected_record_ref) {
        Ok(None) => {
            let recovery_record_present = recovery_record_present(protected_backend, &operation)?;
            let operation = finish_forget_success(metadata, operation, changed_at)?;
            Ok(JournaledForgetAccountReceipt {
                operation_id: Some(operation.operation_id),
                outcome: ForgetAccountOutcome::AlreadyForgotten {
                    recovery_record_present,
                },
            })
        }
        Ok(Some(_)) => compensate_forget(
            metadata,
            protected_backend,
            operation,
            MetadataFailureReason::IdentityRecordConflict,
            changed_at,
        ),
        Err(_) => compensate_forget(
            metadata,
            protected_backend,
            operation,
            MetadataFailureReason::Unavailable,
            changed_at,
        ),
    }
}

fn compensate_forget<B: ProtectedRecordBackend>(
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    operation: VaultOperation,
    reason: MetadataFailureReason,
    changed_at: &UtcTimestamp,
) -> Result<JournaledForgetAccountReceipt, AccountVaultError> {
    let compensation = match protected_backend.restore_record(&operation.protected_record_ref) {
        Ok(VaultTransitionOutcome::Moved | VaultTransitionOutcome::AlreadyAtDestination) => {
            let operation = advance_operation(
                metadata,
                &operation,
                VaultOperationTransition::RecordRestored,
                changed_at,
            )?;
            advance_operation(
                metadata,
                &operation,
                VaultOperationTransition::Compensated {
                    reason_code: metadata_reason_code(reason).into(),
                },
                changed_at,
            )?;
            CompensationOutcome::Restored
        }
        Err(_) => {
            pause_operation(metadata, &operation, "protected_backend_failed", changed_at);
            CompensationOutcome::Failed
        }
    };
    Err(AccountVaultError::MetadataRemovalFailed {
        reason,
        compensation,
    })
}

fn recovery_record_present<B: ProtectedRecordBackend>(
    protected_backend: &mut B,
    operation: &VaultOperation,
) -> Result<bool, AccountVaultError> {
    match protected_backend
        .record_state(&operation.protected_record_ref)
        .map_err(AccountVaultError::ProtectedBackend)?
    {
        VaultRecordState::Quarantined => Ok(true),
        VaultRecordState::Missing => Ok(false),
        state @ (VaultRecordState::Present | VaultRecordState::Conflict) => {
            Err(AccountVaultError::ProtectedRecordState(state))
        }
    }
}

fn ensure_registration_compensation_postcondition<B: ProtectedRecordBackend>(
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    operation: &VaultOperation,
    changed_at: &UtcTimestamp,
) -> Result<(), AccountVaultError> {
    match metadata.vault_account_by_record_ref(&operation.protected_record_ref) {
        Ok(None) => {}
        Ok(Some(_)) => {
            pause_operation(metadata, operation, "record_claimed", changed_at);
            return Err(AccountVaultError::MetadataRegistrationFailed {
                reason: MetadataFailureReason::IdentityRecordConflict,
                compensation: CompensationOutcome::NotRequired,
            });
        }
        Err(_) => {
            pause_operation(metadata, operation, "metadata_lookup_failed", changed_at);
            return Err(AccountVaultError::MetadataLookupFailed);
        }
    }
    match protected_backend.record_state(&operation.protected_record_ref) {
        Ok(VaultRecordState::Quarantined) => Ok(()),
        Ok(state) => {
            pause_operation(
                metadata,
                operation,
                "compensation_postcondition_failed",
                changed_at,
            );
            Err(AccountVaultError::ProtectedRecordState(state))
        }
        Err(error) => {
            pause_operation(metadata, operation, "protected_backend_failed", changed_at);
            Err(AccountVaultError::ProtectedBackend(error))
        }
    }
}

fn ensure_forget_compensation_postcondition<B: ProtectedRecordBackend>(
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    operation: &VaultOperation,
    changed_at: &UtcTimestamp,
) -> Result<(), AccountVaultError> {
    match protected_backend.record_state(&operation.protected_record_ref) {
        Ok(VaultRecordState::Present) => Ok(()),
        Ok(state) => {
            pause_operation(
                metadata,
                operation,
                "compensation_postcondition_failed",
                changed_at,
            );
            Err(AccountVaultError::ProtectedRecordState(state))
        }
        Err(error) => {
            pause_operation(metadata, operation, "protected_backend_failed", changed_at);
            Err(AccountVaultError::ProtectedBackend(error))
        }
    }
}

fn ensure_no_record_owner(
    metadata: &CapacityStore,
    operation: &VaultOperation,
) -> Result<(), AccountVaultError> {
    match metadata.vault_account_by_record_ref(&operation.protected_record_ref) {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err(AccountVaultError::MetadataRemovalFailed {
            reason: MetadataFailureReason::IdentityRecordConflict,
            compensation: CompensationOutcome::NotRequired,
        }),
        Err(_) => Err(AccountVaultError::MetadataLookupFailed),
    }
}

fn pause_operation(
    metadata: &mut CapacityStore,
    operation: &VaultOperation,
    reason_code: &'static str,
    changed_at: &UtcTimestamp,
) {
    if operation.status != VaultOperationStatus::InProgress {
        return;
    }
    let _ = metadata.advance_vault_operation(
        &operation.operation_id,
        operation.revision,
        VaultOperationTransition::NeedsReview {
            reason_code: reason_code.into(),
        },
        changed_at,
    );
}

fn classify_recovery<B: ProtectedRecordBackend>(
    metadata: &CapacityStore,
    protected_backend: &mut B,
    operation: &VaultOperation,
) -> Result<VaultRecoveryAction, AccountVaultError> {
    if operation.status == VaultOperationStatus::NeedsReview {
        return Ok(VaultRecoveryAction::ManualReview);
    }
    let record_state = match protected_backend.record_state(&operation.protected_record_ref) {
        Ok(state) => state,
        Err(_) => return Ok(VaultRecoveryAction::ManualReview),
    };
    match operation.kind {
        VaultOperationKind::RegisterAccount => {
            classify_registration_recovery(metadata, operation, record_state)
        }
        VaultOperationKind::ForgetAccount => {
            classify_forget_recovery(metadata, operation, record_state)
        }
    }
}

fn classify_registration_recovery(
    metadata: &CapacityStore,
    operation: &VaultOperation,
    record_state: VaultRecordState,
) -> Result<VaultRecoveryAction, AccountVaultError> {
    let account = metadata
        .vault_account_by_fingerprint(&operation.account_fingerprint)
        .map_err(|_| AccountVaultError::MetadataLookupFailed)?;
    let exact_account = account
        .as_ref()
        .is_some_and(|account| operation_matches_account(operation, account));
    match operation.checkpoint {
        VaultOperationCheckpoint::Prepared | VaultOperationCheckpoint::RecordReady => {
            if operation.checkpoint == VaultOperationCheckpoint::RecordReady
                && record_state == VaultRecordState::Quarantined
                && account.is_none()
            {
                Ok(VaultRecoveryAction::FinalizeCompensated)
            } else if matches!(
                record_state,
                VaultRecordState::Missing | VaultRecordState::Present
            ) && (account.is_none() || exact_account)
            {
                Ok(VaultRecoveryAction::RetryRegistrationWithOriginalRequest)
            } else {
                Ok(VaultRecoveryAction::ManualReview)
            }
        }
        VaultOperationCheckpoint::MetadataCommitted
            if exact_account && record_state == VaultRecordState::Present =>
        {
            Ok(VaultRecoveryAction::FinalizeSucceeded)
        }
        VaultOperationCheckpoint::RecordQuarantined => {
            let owner = metadata
                .vault_account_by_record_ref(&operation.protected_record_ref)
                .map_err(|_| AccountVaultError::MetadataLookupFailed)?;
            if owner.is_none() && record_state == VaultRecordState::Quarantined {
                Ok(VaultRecoveryAction::FinalizeCompensated)
            } else {
                Ok(VaultRecoveryAction::ManualReview)
            }
        }
        VaultOperationCheckpoint::MetadataCommitted
        | VaultOperationCheckpoint::MetadataRemoved
        | VaultOperationCheckpoint::RecordRestored => Ok(VaultRecoveryAction::ManualReview),
    }
}

fn classify_forget_recovery(
    metadata: &CapacityStore,
    operation: &VaultOperation,
    record_state: VaultRecordState,
) -> Result<VaultRecoveryAction, AccountVaultError> {
    let account_id = operation
        .account_id
        .as_ref()
        .ok_or(AccountVaultError::OperationJournalUnavailable)?;
    let account = metadata
        .vault_account(account_id)
        .map_err(|_| AccountVaultError::MetadataLookupFailed)?;
    let exact_account = account
        .as_ref()
        .is_some_and(|account| operation_matches_account(operation, account));
    match operation.checkpoint {
        VaultOperationCheckpoint::Prepared
            if (exact_account
                && matches!(
                    record_state,
                    VaultRecordState::Present | VaultRecordState::Quarantined
                ))
                || (account.is_none() && record_state == VaultRecordState::Quarantined) =>
        {
            Ok(VaultRecoveryAction::ResumeForget)
        }
        VaultOperationCheckpoint::RecordQuarantined
            if (exact_account || account.is_none())
                && record_state == VaultRecordState::Quarantined =>
        {
            Ok(VaultRecoveryAction::ResumeForget)
        }
        VaultOperationCheckpoint::MetadataRemoved
            if account.is_none()
                && matches!(
                    record_state,
                    VaultRecordState::Missing | VaultRecordState::Quarantined
                ) =>
        {
            let owner = metadata
                .vault_account_by_record_ref(&operation.protected_record_ref)
                .map_err(|_| AccountVaultError::MetadataLookupFailed)?;
            if owner.is_none() {
                Ok(VaultRecoveryAction::FinalizeSucceeded)
            } else {
                Ok(VaultRecoveryAction::ManualReview)
            }
        }
        VaultOperationCheckpoint::RecordRestored if record_state == VaultRecordState::Present => {
            let owner = metadata
                .vault_account_by_record_ref(&operation.protected_record_ref)
                .map_err(|_| AccountVaultError::MetadataLookupFailed)?;
            if owner
                .as_ref()
                .is_some_and(|account| operation_matches_identity(operation, account))
            {
                Ok(VaultRecoveryAction::FinalizeCompensated)
            } else {
                Ok(VaultRecoveryAction::ManualReview)
            }
        }
        VaultOperationCheckpoint::Prepared
        | VaultOperationCheckpoint::RecordQuarantined
        | VaultOperationCheckpoint::MetadataRemoved
        | VaultOperationCheckpoint::RecordRestored
        | VaultOperationCheckpoint::RecordReady
        | VaultOperationCheckpoint::MetadataCommitted => Ok(VaultRecoveryAction::ManualReview),
    }
}

fn registration_matches_account(
    registration: &VaultAccountRegistration,
    account: &VaultAccount,
) -> bool {
    registration.account_fingerprint == account.account_fingerprint
        && registration.protected_record_ref == account.protected_record_ref
}

fn forget_snapshot_matches(expected: &VaultAccount, current: &VaultAccount) -> bool {
    expected.account_id == current.account_id
        && expected.revision == current.revision
        && expected.account_fingerprint == current.account_fingerprint
        && expected.protected_record_ref == current.protected_record_ref
}

fn operation_matches_account(operation: &VaultOperation, account: &VaultAccount) -> bool {
    operation
        .account_id
        .as_ref()
        .is_none_or(|id| id == &account.account_id)
        && operation_matches_identity(operation, account)
        && operation
            .expected_account_revision
            .is_none_or(|revision| revision == account.revision)
}

fn operation_matches_identity(operation: &VaultOperation, account: &VaultAccount) -> bool {
    operation.account_fingerprint == account.account_fingerprint
        && operation.protected_record_ref == account.protected_record_ref
}

fn registration_error_code(error: &AccountVaultError) -> &'static str {
    match error {
        AccountVaultError::ProtectedRecordContentsConflict
        | AccountVaultError::ProtectedRecordState(_) => "protected_record_conflict",
        _ => "protected_backend_failed",
    }
}

fn metadata_reason_code(reason: MetadataFailureReason) -> &'static str {
    match reason {
        MetadataFailureReason::Unavailable => "metadata_unavailable",
        MetadataFailureReason::CapacityExceeded => "metadata_capacity_exceeded",
        MetadataFailureReason::IdentityRecordConflict => "metadata_identity_conflict",
        MetadataFailureReason::RevisionConflict => "metadata_revision_conflict",
        MetadataFailureReason::InvariantViolation => "metadata_invariant_violation",
    }
}

fn journal_store_error(error: StoreError) -> AccountVaultError {
    match error {
        StoreError::VaultOperationConflict => AccountVaultError::OperationJournalConflict,
        StoreError::VaultOperationJournalFull => AccountVaultError::OperationJournalFull,
        _ => AccountVaultError::OperationJournalUnavailable,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use capacity_domain::{
        AccountFingerprint, VaultAccountAuthMode, VaultAccountLifecycle, VaultAccountSource,
        VaultRecordRef,
    };

    use super::*;
    use crate::FileVault;

    const FINGERPRINT: &str = "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73401:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const FINGERPRINT_2: &str = "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73401:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    const RECORD_UUID: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73402";
    const RECORD_UUID_2: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73403";
    const TOKEN_CANARY: &str = "journaled-secret-token";
    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new() -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let epoch_nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "capacity-journaled-vault-test-{}-{epoch_nanos}-{sequence}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(&path).expect("create test root");
            Self { path }
        }

        fn metadata_path(&self) -> PathBuf {
            self.path.join("capacity.sqlite3")
        }

        fn file_vault(&self) -> FileVault {
            FileVault::open(self.path.join("vault")).expect("open file vault")
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let safe_name = self
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("capacity-journaled-vault-test-"));
            if safe_name && self.path.starts_with(std::env::temp_dir()) {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }

    fn timestamp(value: &str) -> UtcTimestamp {
        UtcTimestamp::parse(value).expect("timestamp")
    }

    fn registration(
        fingerprint: &str,
        record_uuid: &str,
        display_name: &str,
    ) -> VaultAccountRegistration {
        VaultAccountRegistration {
            account_fingerprint: AccountFingerprint::parse(fingerprint).expect("fingerprint"),
            protected_record_ref: VaultRecordRef::parse(format!("vault-record:v1:{record_uuid}"))
                .expect("record reference"),
            display_name: display_name.into(),
            auth_mode: VaultAccountAuthMode::ChatGpt,
            source: VaultAccountSource::ManualChatGpt,
            lifecycle: VaultAccountLifecycle::Ready,
            provider_id: None,
            model: None,
        }
    }

    fn protected_record() -> ProtectedVaultRecord {
        ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_account_id",
            b"journaled-person@example.invalid".to_vec(),
            format!(r#"{{"auth_mode":"chatgpt","access_token":"{TOKEN_CANARY}"}}"#).into_bytes(),
            None,
        )
        .expect("protected record")
    }

    fn created_operation(outcome: VaultOperationBeginOutcome) -> VaultOperation {
        let VaultOperationBeginOutcome::Created(operation) = outcome else {
            panic!("new operation must be created");
        };
        operation
    }

    fn advanced_operation(outcome: VaultOperationAdvanceOutcome) -> VaultOperation {
        let VaultOperationAdvanceOutcome::Updated(operation) = outcome else {
            panic!("operation must advance");
        };
        operation
    }

    struct RestoreBeforeForgetPostflight {
        inner: FileVault,
        state_reads: u32,
    }

    impl ProtectedRecordBackend for RestoreBeforeForgetPostflight {
        fn record_state(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<VaultRecordState, crate::VaultBackendError> {
            self.state_reads += 1;
            if self.state_reads == 2 {
                self.inner.restore_record(record_ref)?;
            }
            self.inner.record_state(record_ref)
        }

        fn create_record(
            &mut self,
            record_ref: &VaultRecordRef,
            record: &ProtectedVaultRecord,
        ) -> Result<(), crate::VaultBackendError> {
            self.inner.create_record(record_ref, record)
        }

        fn read_record(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<ProtectedVaultRecord, crate::VaultBackendError> {
            self.inner.read_record(record_ref)
        }

        fn quarantine_record(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<VaultTransitionOutcome, crate::VaultBackendError> {
            self.inner.quarantine_record(record_ref)
        }

        fn restore_record(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<VaultTransitionOutcome, crate::VaultBackendError> {
            self.inner.restore_record(record_ref)
        }
    }

    #[test]
    fn journaled_registration_persists_every_checkpoint_to_terminal_success() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.metadata_path()).expect("open metadata");
        let mut backend = root.file_vault();
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let receipt = journaled_register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(),
            &timestamp("2026-08-30T08:00:00Z"),
        )
        .expect("journaled registration");

        assert!(receipt.receipt.metadata_created);
        assert!(receipt.receipt.protected_record_created);
        let operation = metadata
            .vault_operation(&receipt.operation_id)
            .unwrap()
            .expect("terminal operation");
        assert_eq!(operation.status, VaultOperationStatus::Succeeded);
        assert_eq!(
            operation.checkpoint,
            VaultOperationCheckpoint::MetadataCommitted
        );
        assert_eq!(operation.revision, 4);
        assert!(
            metadata
                .recoverable_vault_operations(256)
                .unwrap()
                .is_empty()
        );
        let rendered = format!("{receipt:?}");
        assert!(!rendered.contains(TOKEN_CANARY));
        assert!(!rendered.contains(Path::new(&root.path).to_string_lossy().as_ref()));
    }

    #[test]
    fn prepared_registration_survives_restart_and_requires_original_request() {
        let root = TestRoot::new();
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let operation = {
            let mut metadata = CapacityStore::open(root.metadata_path()).expect("open metadata");
            let backend = root.file_vault();
            let operation = created_operation(
                metadata
                    .begin_vault_registration_operation(
                        &registration,
                        &timestamp("2026-08-30T08:00:00Z"),
                    )
                    .expect("begin operation"),
            );
            backend
                .create_record(&registration.protected_record_ref, &protected_record())
                .expect("simulate create before checkpoint");
            operation
        };

        let mut metadata = CapacityStore::open(root.metadata_path()).expect("reopen metadata");
        let mut backend = root.file_vault();
        let report =
            inspect_vault_recovery(&metadata, &mut backend, 256).expect("inspect recovery");
        assert_eq!(report.scanned, 1);
        assert_eq!(
            report.items[0].action,
            VaultRecoveryAction::RetryRegistrationWithOriginalRequest
        );
        assert_eq!(
            report.items[0].checkpoint,
            VaultOperationCheckpoint::Prepared
        );
        assert_eq!(
            metadata
                .vault_operation(&operation.operation_id)
                .unwrap()
                .expect("unchanged operation")
                .revision,
            operation.revision,
            "inspection must remain read-only"
        );

        let receipt = journaled_register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(),
            &timestamp("2026-08-30T08:01:00Z"),
        )
        .expect("resume registration");
        assert_eq!(receipt.operation_id, operation.operation_id);
        assert!(!receipt.receipt.protected_record_created);
    }

    #[test]
    fn registration_reconciles_metadata_commit_before_checkpoint() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.metadata_path()).expect("open metadata");
        let mut backend = root.file_vault();
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let operation = created_operation(
            metadata
                .begin_vault_registration_operation(
                    &registration,
                    &timestamp("2026-08-30T08:00:00Z"),
                )
                .expect("begin operation"),
        );
        backend
            .create_record(&registration.protected_record_ref, &protected_record())
            .expect("create record");
        let operation = advanced_operation(
            metadata
                .advance_vault_operation(
                    &operation.operation_id,
                    operation.revision,
                    VaultOperationTransition::RecordReady,
                    &timestamp("2026-08-30T08:01:00Z"),
                )
                .expect("record ready"),
        );
        let account = match metadata
            .register_vault_account(&registration, &timestamp("2026-08-30T08:02:00Z"))
            .expect("simulate metadata commit")
        {
            VaultAccountRegistrationOutcome::Created(account) => account,
            VaultAccountRegistrationOutcome::Existing(_) => panic!("account must be new"),
        };

        let report =
            inspect_vault_recovery(&metadata, &mut backend, 256).expect("inspect recovery");
        assert_eq!(
            report.items[0].action,
            VaultRecoveryAction::RetryRegistrationWithOriginalRequest
        );
        let receipt = journaled_register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(),
            &timestamp("2026-08-30T08:03:00Z"),
        )
        .expect("reconcile metadata commit");
        assert_eq!(receipt.operation_id, operation.operation_id);
        assert_eq!(receipt.receipt.account, account);
        assert!(!receipt.receipt.metadata_created);
    }

    #[test]
    fn metadata_committed_checkpoint_is_classified_for_safe_finalization() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.metadata_path()).expect("open metadata");
        let mut backend = root.file_vault();
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let operation = created_operation(
            metadata
                .begin_vault_registration_operation(
                    &registration,
                    &timestamp("2026-08-30T08:00:00Z"),
                )
                .expect("begin operation"),
        );
        backend
            .create_record(&registration.protected_record_ref, &protected_record())
            .expect("create record");
        let operation = advanced_operation(
            metadata
                .advance_vault_operation(
                    &operation.operation_id,
                    operation.revision,
                    VaultOperationTransition::RecordReady,
                    &timestamp("2026-08-30T08:01:00Z"),
                )
                .expect("record ready"),
        );
        let account = match metadata
            .register_vault_account(&registration, &timestamp("2026-08-30T08:02:00Z"))
            .expect("register metadata")
        {
            VaultAccountRegistrationOutcome::Created(account) => account,
            VaultAccountRegistrationOutcome::Existing(_) => panic!("account must be new"),
        };
        let operation = advanced_operation(
            metadata
                .advance_vault_operation(
                    &operation.operation_id,
                    operation.revision,
                    VaultOperationTransition::MetadataCommitted {
                        account_id: account.account_id,
                    },
                    &timestamp("2026-08-30T08:03:00Z"),
                )
                .expect("metadata checkpoint"),
        );

        let report =
            inspect_vault_recovery(&metadata, &mut backend, 256).expect("inspect recovery");
        assert_eq!(
            report.items[0].action,
            VaultRecoveryAction::FinalizeSucceeded
        );
        assert_eq!(report.items[0].operation_id, operation.operation_id);
    }

    #[test]
    fn journaled_forget_reaches_terminal_success_and_keeps_recovery_record() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.metadata_path()).expect("open metadata");
        let mut backend = root.file_vault();
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let account = journaled_register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(),
            &timestamp("2026-08-30T08:00:00Z"),
        )
        .expect("register")
        .receipt
        .account;

        let receipt = journaled_forget_account(
            &mut metadata,
            &mut backend,
            &account,
            &timestamp("2026-08-30T08:01:00Z"),
        )
        .expect("forget");
        let operation_id = receipt.operation_id.expect("journal operation");
        assert!(matches!(
            receipt.outcome,
            ForgetAccountOutcome::Forgotten {
                record_was_already_quarantined: false,
                ..
            }
        ));
        let operation = metadata
            .vault_operation(&operation_id)
            .unwrap()
            .expect("terminal operation");
        assert_eq!(operation.status, VaultOperationStatus::Succeeded);
        assert_eq!(
            operation.checkpoint,
            VaultOperationCheckpoint::MetadataRemoved
        );
        assert_eq!(
            backend.record_state(&registration.protected_record_ref),
            Ok(VaultRecordState::Quarantined)
        );
    }

    #[test]
    fn quarantine_before_checkpoint_is_classified_and_resumed_after_restart() {
        let root = TestRoot::new();
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let (account, operation) = {
            let mut metadata = CapacityStore::open(root.metadata_path()).expect("open metadata");
            let mut backend = root.file_vault();
            let account = journaled_register_account(
                &mut metadata,
                &mut backend,
                &registration,
                &protected_record(),
                &timestamp("2026-08-30T08:00:00Z"),
            )
            .expect("register")
            .receipt
            .account;
            let operation = created_operation(
                metadata
                    .begin_vault_forget_operation(&account, &timestamp("2026-08-30T08:01:00Z"))
                    .expect("begin forget"),
            );
            backend
                .quarantine_record(&registration.protected_record_ref)
                .expect("simulate quarantine before checkpoint");
            (account, operation)
        };

        let mut metadata = CapacityStore::open(root.metadata_path()).expect("reopen metadata");
        let mut backend = root.file_vault();
        let report =
            inspect_vault_recovery(&metadata, &mut backend, 256).expect("inspect recovery");
        assert_eq!(report.items[0].action, VaultRecoveryAction::ResumeForget);
        assert_eq!(
            report.items[0].checkpoint,
            VaultOperationCheckpoint::Prepared
        );
        let receipt = journaled_forget_account(
            &mut metadata,
            &mut backend,
            &account,
            &timestamp("2026-08-30T08:02:00Z"),
        )
        .expect("resume forget");
        assert_eq!(receipt.operation_id, Some(operation.operation_id));
        assert!(
            metadata
                .vault_account(&account.account_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn metadata_removal_before_checkpoint_is_reconciled_without_a_live_record() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.metadata_path()).expect("open metadata");
        let mut backend = root.file_vault();
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let account = journaled_register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(),
            &timestamp("2026-08-30T08:00:00Z"),
        )
        .expect("register")
        .receipt
        .account;
        let operation = created_operation(
            metadata
                .begin_vault_forget_operation(&account, &timestamp("2026-08-30T08:01:00Z"))
                .expect("begin forget"),
        );
        backend
            .quarantine_record(&registration.protected_record_ref)
            .expect("quarantine");
        let operation = advanced_operation(
            metadata
                .advance_vault_operation(
                    &operation.operation_id,
                    operation.revision,
                    VaultOperationTransition::RecordQuarantined,
                    &timestamp("2026-08-30T08:02:00Z"),
                )
                .expect("quarantine checkpoint"),
        );
        assert!(matches!(
            metadata
                .remove_vault_account_metadata(
                    &account.account_id,
                    account.revision,
                    &timestamp("2026-08-30T08:03:00Z"),
                )
                .expect("remove metadata"),
            VaultAccountRemovalOutcome::Removed(_)
        ));

        let report =
            inspect_vault_recovery(&metadata, &mut backend, 256).expect("inspect recovery");
        assert_eq!(report.items[0].action, VaultRecoveryAction::ResumeForget);
        let receipt = journaled_forget_account(
            &mut metadata,
            &mut backend,
            &account,
            &timestamp("2026-08-30T08:04:00Z"),
        )
        .expect("finish missing metadata");
        assert_eq!(receipt.operation_id, Some(operation.operation_id));
        assert_eq!(
            receipt.outcome,
            ForgetAccountOutcome::AlreadyForgotten {
                recovery_record_present: true
            }
        );
    }

    #[test]
    fn stale_forget_preflight_creates_no_active_journal_or_record_side_effect() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.metadata_path()).expect("open metadata");
        let mut backend = root.file_vault();
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let account = journaled_register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(),
            &timestamp("2026-08-30T08:00:00Z"),
        )
        .expect("register")
        .receipt
        .account;
        metadata
            .rename_vault_account(
                &account.account_id,
                account.revision,
                "Renamed account",
                &timestamp("2026-08-30T08:01:00Z"),
            )
            .expect("rename account");

        assert_eq!(
            journaled_forget_account(
                &mut metadata,
                &mut backend,
                &account,
                &timestamp("2026-08-30T08:02:00Z"),
            ),
            Err(AccountVaultError::MetadataSnapshotStale)
        );
        assert!(
            metadata
                .recoverable_vault_operations(256)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            backend.record_state(&registration.protected_record_ref),
            Ok(VaultRecordState::Present)
        );
    }

    #[test]
    fn forget_postflight_refuses_terminal_success_if_record_returns_live() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.metadata_path()).expect("open metadata");
        let mut file_vault = root.file_vault();
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let account = journaled_register_account(
            &mut metadata,
            &mut file_vault,
            &registration,
            &protected_record(),
            &timestamp("2026-08-30T08:00:00Z"),
        )
        .expect("register")
        .receipt
        .account;
        let mut backend = RestoreBeforeForgetPostflight {
            inner: file_vault,
            state_reads: 0,
        };

        assert_eq!(
            journaled_forget_account(
                &mut metadata,
                &mut backend,
                &account,
                &timestamp("2026-08-30T08:01:00Z"),
            ),
            Err(AccountVaultError::ProtectedRecordState(
                VaultRecordState::Present
            ))
        );
        let pending = metadata.recoverable_vault_operations(256).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].checkpoint,
            VaultOperationCheckpoint::RecordQuarantined
        );
        let report =
            inspect_vault_recovery(&metadata, &mut backend, 256).expect("inspect recovery");
        assert_eq!(report.items[0].action, VaultRecoveryAction::ManualReview);
    }

    #[test]
    fn registration_compensation_refuses_terminal_state_if_record_returns_live() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.metadata_path()).expect("open metadata");
        let mut backend = root.file_vault();
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let operation = created_operation(
            metadata
                .begin_vault_registration_operation(
                    &registration,
                    &timestamp("2026-08-30T08:00:00Z"),
                )
                .expect("begin registration"),
        );
        backend
            .create_record(&registration.protected_record_ref, &protected_record())
            .expect("create record");
        let operation = advanced_operation(
            metadata
                .advance_vault_operation(
                    &operation.operation_id,
                    operation.revision,
                    VaultOperationTransition::RecordReady,
                    &timestamp("2026-08-30T08:01:00Z"),
                )
                .expect("record ready"),
        );
        backend
            .quarantine_record(&registration.protected_record_ref)
            .expect("quarantine record");
        let operation = advanced_operation(
            metadata
                .advance_vault_operation(
                    &operation.operation_id,
                    operation.revision,
                    VaultOperationTransition::RecordQuarantined,
                    &timestamp("2026-08-30T08:02:00Z"),
                )
                .expect("quarantine checkpoint"),
        );
        backend
            .restore_record(&registration.protected_record_ref)
            .expect("simulate record returning live");

        assert_eq!(
            journaled_register_account(
                &mut metadata,
                &mut backend,
                &registration,
                &protected_record(),
                &timestamp("2026-08-30T08:03:00Z"),
            ),
            Err(AccountVaultError::ProtectedRecordState(
                VaultRecordState::Present
            ))
        );
        let pending = metadata.recoverable_vault_operations(256).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation_id, operation.operation_id);
        assert_eq!(pending[0].status, VaultOperationStatus::NeedsReview);
        let report =
            inspect_vault_recovery(&metadata, &mut backend, 256).expect("inspect recovery");
        assert_eq!(report.items[0].action, VaultRecoveryAction::ManualReview);
    }

    #[test]
    fn forget_compensation_refuses_terminal_state_if_record_is_requarantined() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.metadata_path()).expect("open metadata");
        let mut backend = root.file_vault();
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let account = journaled_register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(),
            &timestamp("2026-08-30T08:00:00Z"),
        )
        .expect("register")
        .receipt
        .account;
        let operation = created_operation(
            metadata
                .begin_vault_forget_operation(&account, &timestamp("2026-08-30T08:01:00Z"))
                .expect("begin forget"),
        );
        backend
            .quarantine_record(&registration.protected_record_ref)
            .expect("quarantine record");
        let operation = advanced_operation(
            metadata
                .advance_vault_operation(
                    &operation.operation_id,
                    operation.revision,
                    VaultOperationTransition::RecordQuarantined,
                    &timestamp("2026-08-30T08:02:00Z"),
                )
                .expect("quarantine checkpoint"),
        );
        backend
            .restore_record(&registration.protected_record_ref)
            .expect("restore record");
        let operation = advanced_operation(
            metadata
                .advance_vault_operation(
                    &operation.operation_id,
                    operation.revision,
                    VaultOperationTransition::RecordRestored,
                    &timestamp("2026-08-30T08:03:00Z"),
                )
                .expect("restore checkpoint"),
        );
        backend
            .quarantine_record(&registration.protected_record_ref)
            .expect("simulate recovery record moving again");

        assert_eq!(
            journaled_forget_account(
                &mut metadata,
                &mut backend,
                &account,
                &timestamp("2026-08-30T08:04:00Z"),
            ),
            Err(AccountVaultError::ProtectedRecordState(
                VaultRecordState::Quarantined
            ))
        );
        let pending = metadata.recoverable_vault_operations(256).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation_id, operation.operation_id);
        assert_eq!(pending[0].status, VaultOperationStatus::NeedsReview);
        let report =
            inspect_vault_recovery(&metadata, &mut backend, 256).expect("inspect recovery");
        assert_eq!(report.items[0].action, VaultRecoveryAction::ManualReview);
    }

    #[test]
    fn recovery_classification_is_bounded_redacted_and_marks_review_states() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.metadata_path()).expect("open metadata");
        let mut backend = root.file_vault();
        let registration = registration(FINGERPRINT_2, RECORD_UUID_2, "Backup ChatGPT");
        let operation = created_operation(
            metadata
                .begin_vault_registration_operation(
                    &registration,
                    &timestamp("2026-08-30T08:00:00Z"),
                )
                .expect("begin operation"),
        );
        let paused = advanced_operation(
            metadata
                .advance_vault_operation(
                    &operation.operation_id,
                    operation.revision,
                    VaultOperationTransition::NeedsReview {
                        reason_code: "registration_request_required".into(),
                    },
                    &timestamp("2026-08-30T08:01:00Z"),
                )
                .expect("pause operation"),
        );

        let report = inspect_vault_recovery(&metadata, &mut backend, 1).expect("inspect recovery");
        assert_eq!(report.scanned, 1);
        assert_eq!(report.items[0].action, VaultRecoveryAction::ManualReview);
        assert_eq!(report.items[0].operation_id, paused.operation_id);
        let rendered = format!("{report:?}");
        assert!(!rendered.contains(FINGERPRINT_2));
        assert!(!rendered.contains(TOKEN_CANARY));
        assert!(!rendered.contains(Path::new(&root.path).to_string_lossy().as_ref()));
    }
}
