use std::fmt;

use capacity_domain::{
    UtcTimestamp, VaultAccount, VaultOperation, VaultOperationCheckpoint, VaultOperationId,
    VaultOperationKind, VaultOperationStatus, VaultOperationTransition,
};
use capacity_store::{CapacityStore, VaultOperationAdvanceOutcome};
use capacity_vault::{
    AccountVaultError, ProtectedRecordBackend, VaultRecordState, VaultRecoveryAction,
    inspect_vault_recovery,
};

use crate::{LegacyViewerProbe, LockedVaultMutationError, MutationLock, MutationLockError};

const RECOVERY_POSTCONDITION_FAILED: &str = "recovery_postcondition_failed";
const RECOVERED_COMPENSATION: &str = "recovered_compensation";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultRecoveryDisposition {
    Succeeded,
    Compensated,
    RegistrationRequestRequired,
    ManualReviewRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultRecoveryExecutionItem {
    pub operation_id: VaultOperationId,
    pub action: VaultRecoveryAction,
    pub disposition: VaultRecoveryDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultRecoveryExecutionReport {
    pub scanned: u32,
    pub recovered: u32,
    pub deferred: u32,
    pub items: Vec<VaultRecoveryExecutionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultRecoveryExecutionError {
    Lock(MutationLockError),
    Vault(AccountVaultError),
    StoreUnavailable,
    OperationChanged,
    InvariantViolation,
}

impl fmt::Display for VaultRecoveryExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(formatter),
            Self::Vault(error) => error.fmt(formatter),
            Self::StoreUnavailable => formatter.write_str("account-vault metadata is unavailable"),
            Self::OperationChanged => {
                formatter.write_str("account-vault recovery operation changed during execution")
            }
            Self::InvariantViolation => {
                formatter.write_str("account-vault recovery invariant failed")
            }
        }
    }
}

impl std::error::Error for VaultRecoveryExecutionError {}

pub(crate) fn execute_vault_recovery_queue<B: ProtectedRecordBackend, P: LegacyViewerProbe>(
    lock: &MutationLock,
    legacy_probe: &mut P,
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    limit: u32,
    changed_at: &UtcTimestamp,
) -> Result<VaultRecoveryExecutionReport, VaultRecoveryExecutionError> {
    verify_admission(lock, legacy_probe)?;
    let inspection = inspect_vault_recovery(metadata, protected_backend, limit)
        .map_err(VaultRecoveryExecutionError::Vault)?;
    let mut execution = VaultRecoveryExecutionReport {
        scanned: inspection.scanned,
        recovered: 0,
        deferred: 0,
        items: Vec::with_capacity(inspection.items.len()),
    };

    for item in inspection.items {
        verify_admission(lock, legacy_probe)?;
        let operation = metadata
            .vault_operation(&item.operation_id)
            .map_err(|_| VaultRecoveryExecutionError::StoreUnavailable)?
            .ok_or(VaultRecoveryExecutionError::OperationChanged)?;
        if operation.kind != item.kind
            || operation.checkpoint != item.checkpoint
            || !status_matches_action(operation.status, item.action)
        {
            return Err(VaultRecoveryExecutionError::OperationChanged);
        }

        let disposition = match item.action {
            VaultRecoveryAction::RetryRegistrationWithOriginalRequest => {
                VaultRecoveryDisposition::RegistrationRequestRequired
            }
            VaultRecoveryAction::ManualReview => VaultRecoveryDisposition::ManualReviewRequired,
            VaultRecoveryAction::ResumeForget => resume_forget(
                lock,
                legacy_probe,
                metadata,
                protected_backend,
                &operation,
                changed_at,
            )?,
            VaultRecoveryAction::FinalizeSucceeded => {
                finalize_succeeded(metadata, protected_backend, &operation, changed_at)?
            }
            VaultRecoveryAction::FinalizeCompensated => {
                finalize_compensated(metadata, protected_backend, &operation, changed_at)?
            }
        };

        match disposition {
            VaultRecoveryDisposition::Succeeded | VaultRecoveryDisposition::Compensated => {
                execution.recovered = execution
                    .recovered
                    .checked_add(1)
                    .ok_or(VaultRecoveryExecutionError::InvariantViolation)?;
            }
            VaultRecoveryDisposition::RegistrationRequestRequired
            | VaultRecoveryDisposition::ManualReviewRequired => {
                execution.deferred = execution
                    .deferred
                    .checked_add(1)
                    .ok_or(VaultRecoveryExecutionError::InvariantViolation)?;
            }
        }
        execution.items.push(VaultRecoveryExecutionItem {
            operation_id: item.operation_id,
            action: item.action,
            disposition,
        });
        verify_admission(lock, legacy_probe)?;
    }

    let accounted = execution
        .recovered
        .checked_add(execution.deferred)
        .ok_or(VaultRecoveryExecutionError::InvariantViolation)?;
    if accounted != execution.scanned
        || usize::try_from(execution.scanned).ok() != Some(execution.items.len())
    {
        return Err(VaultRecoveryExecutionError::InvariantViolation);
    }
    verify_admission(lock, legacy_probe)?;
    Ok(execution)
}

fn status_matches_action(status: VaultOperationStatus, action: VaultRecoveryAction) -> bool {
    match action {
        VaultRecoveryAction::ManualReview => status == VaultOperationStatus::NeedsReview,
        VaultRecoveryAction::RetryRegistrationWithOriginalRequest
        | VaultRecoveryAction::ResumeForget
        | VaultRecoveryAction::FinalizeSucceeded
        | VaultRecoveryAction::FinalizeCompensated => status == VaultOperationStatus::InProgress,
    }
}

fn resume_forget<B: ProtectedRecordBackend, P: LegacyViewerProbe>(
    lock: &MutationLock,
    legacy_probe: &mut P,
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    operation: &VaultOperation,
    changed_at: &UtcTimestamp,
) -> Result<VaultRecoveryDisposition, VaultRecoveryExecutionError> {
    if operation.kind != VaultOperationKind::ForgetAccount
        || !matches!(
            operation.checkpoint,
            VaultOperationCheckpoint::Prepared | VaultOperationCheckpoint::RecordQuarantined
        )
    {
        return Err(VaultRecoveryExecutionError::InvariantViolation);
    }
    let expected = expected_forget_account(metadata, operation)?;
    if !operation_matches_account(operation, &expected) {
        return pause_for_manual_review(metadata, operation, changed_at);
    }

    match lock.journaled_forget_account(
        legacy_probe,
        metadata,
        protected_backend,
        &expected,
        changed_at,
    ) {
        Ok(receipt) => {
            if receipt.operation_id.as_ref() != Some(&operation.operation_id) {
                return Err(VaultRecoveryExecutionError::OperationChanged);
            }
        }
        Err(LockedVaultMutationError::Lock(error)) => {
            return Err(VaultRecoveryExecutionError::Lock(error));
        }
        Err(LockedVaultMutationError::Vault(error)) => {
            let current = load_same_operation(metadata, operation)?;
            return match current.status {
                VaultOperationStatus::Compensated => Ok(VaultRecoveryDisposition::Compensated),
                _ => Err(VaultRecoveryExecutionError::Vault(error)),
            };
        }
    }
    let current = load_same_operation(metadata, operation)?;
    match current.status {
        VaultOperationStatus::Succeeded => Ok(VaultRecoveryDisposition::Succeeded),
        VaultOperationStatus::Compensated => Ok(VaultRecoveryDisposition::Compensated),
        _ => Err(VaultRecoveryExecutionError::InvariantViolation),
    }
}

fn expected_forget_account(
    metadata: &CapacityStore,
    operation: &VaultOperation,
) -> Result<VaultAccount, VaultRecoveryExecutionError> {
    let account_id = operation
        .account_id
        .as_ref()
        .ok_or(VaultRecoveryExecutionError::InvariantViolation)?;
    if let Some(account) = metadata
        .vault_account(account_id)
        .map_err(|_| VaultRecoveryExecutionError::StoreUnavailable)?
    {
        return Ok(account);
    }
    Ok(VaultAccount {
        account_id: account_id.clone(),
        account_fingerprint: operation.account_fingerprint.clone(),
        protected_record_ref: operation.protected_record_ref.clone(),
        display_name: operation.display_name.clone(),
        auth_mode: operation.auth_mode,
        source: operation.source,
        lifecycle: operation.lifecycle,
        provider_id: operation.provider_id.clone(),
        model: operation.model.clone(),
        revision: operation
            .expected_account_revision
            .ok_or(VaultRecoveryExecutionError::InvariantViolation)?,
        display_order: 0,
        created_at: operation.created_at.clone(),
        updated_at: operation.updated_at.clone(),
        last_used_at: None,
    })
}

fn finalize_succeeded<B: ProtectedRecordBackend>(
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    operation: &VaultOperation,
    changed_at: &UtcTimestamp,
) -> Result<VaultRecoveryDisposition, VaultRecoveryExecutionError> {
    if !succeeded_postcondition(metadata, protected_backend, operation)? {
        return pause_for_manual_review(metadata, operation, changed_at);
    }
    advance_terminal(
        metadata,
        operation,
        VaultOperationTransition::Succeeded,
        changed_at,
    )?;
    Ok(VaultRecoveryDisposition::Succeeded)
}

fn finalize_compensated<B: ProtectedRecordBackend>(
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    operation: &VaultOperation,
    changed_at: &UtcTimestamp,
) -> Result<VaultRecoveryDisposition, VaultRecoveryExecutionError> {
    if !compensated_postcondition(metadata, protected_backend, operation)? {
        return pause_for_manual_review(metadata, operation, changed_at);
    }
    advance_terminal(
        metadata,
        operation,
        VaultOperationTransition::Compensated {
            reason_code: RECOVERED_COMPENSATION.into(),
        },
        changed_at,
    )?;
    Ok(VaultRecoveryDisposition::Compensated)
}

fn succeeded_postcondition<B: ProtectedRecordBackend>(
    metadata: &CapacityStore,
    protected_backend: &mut B,
    operation: &VaultOperation,
) -> Result<bool, VaultRecoveryExecutionError> {
    match operation.kind {
        VaultOperationKind::RegisterAccount
            if operation.checkpoint == VaultOperationCheckpoint::MetadataCommitted =>
        {
            let Some(account_id) = operation.account_id.as_ref() else {
                return Ok(false);
            };
            let account = metadata
                .vault_account(account_id)
                .map_err(|_| VaultRecoveryExecutionError::StoreUnavailable)?;
            let owner = metadata
                .vault_account_by_record_ref(&operation.protected_record_ref)
                .map_err(|_| VaultRecoveryExecutionError::StoreUnavailable)?;
            if !account
                .as_ref()
                .is_some_and(|account| operation_matches_account(operation, account))
                || owner.as_ref().map(|account| &account.account_id) != Some(account_id)
            {
                return Ok(false);
            }
            if protected_backend.record_state(&operation.protected_record_ref)
                != Ok(VaultRecordState::Present)
            {
                return Ok(false);
            }
            Ok(protected_backend
                .read_record(&operation.protected_record_ref)
                .is_ok())
        }
        VaultOperationKind::ForgetAccount
            if operation.checkpoint == VaultOperationCheckpoint::MetadataRemoved =>
        {
            let Some(account_id) = operation.account_id.as_ref() else {
                return Ok(false);
            };
            let account = metadata
                .vault_account(account_id)
                .map_err(|_| VaultRecoveryExecutionError::StoreUnavailable)?;
            let owner = metadata
                .vault_account_by_record_ref(&operation.protected_record_ref)
                .map_err(|_| VaultRecoveryExecutionError::StoreUnavailable)?;
            if account.is_some() || owner.is_some() {
                return Ok(false);
            }
            Ok(matches!(
                protected_backend.record_state(&operation.protected_record_ref),
                Ok(VaultRecordState::Missing | VaultRecordState::Quarantined)
            ))
        }
        _ => Ok(false),
    }
}

fn compensated_postcondition<B: ProtectedRecordBackend>(
    metadata: &CapacityStore,
    protected_backend: &mut B,
    operation: &VaultOperation,
) -> Result<bool, VaultRecoveryExecutionError> {
    match operation.kind {
        VaultOperationKind::RegisterAccount
            if operation.checkpoint == VaultOperationCheckpoint::RecordQuarantined =>
        {
            let owner = metadata
                .vault_account_by_record_ref(&operation.protected_record_ref)
                .map_err(|_| VaultRecoveryExecutionError::StoreUnavailable)?;
            Ok(owner.is_none()
                && protected_backend.record_state(&operation.protected_record_ref)
                    == Ok(VaultRecordState::Quarantined))
        }
        VaultOperationKind::ForgetAccount
            if operation.checkpoint == VaultOperationCheckpoint::RecordRestored =>
        {
            let Some(account_id) = operation.account_id.as_ref() else {
                return Ok(false);
            };
            let account = metadata
                .vault_account(account_id)
                .map_err(|_| VaultRecoveryExecutionError::StoreUnavailable)?;
            let owner = metadata
                .vault_account_by_record_ref(&operation.protected_record_ref)
                .map_err(|_| VaultRecoveryExecutionError::StoreUnavailable)?;
            if !account
                .as_ref()
                .is_some_and(|account| operation_matches_account(operation, account))
                || owner.as_ref().map(|account| &account.account_id) != Some(account_id)
            {
                return Ok(false);
            }
            if protected_backend.record_state(&operation.protected_record_ref)
                != Ok(VaultRecordState::Present)
            {
                return Ok(false);
            }
            Ok(protected_backend
                .read_record(&operation.protected_record_ref)
                .is_ok())
        }
        _ => Ok(false),
    }
}

fn pause_for_manual_review(
    metadata: &mut CapacityStore,
    operation: &VaultOperation,
    changed_at: &UtcTimestamp,
) -> Result<VaultRecoveryDisposition, VaultRecoveryExecutionError> {
    advance_terminal(
        metadata,
        operation,
        VaultOperationTransition::NeedsReview {
            reason_code: RECOVERY_POSTCONDITION_FAILED.into(),
        },
        changed_at,
    )?;
    Ok(VaultRecoveryDisposition::ManualReviewRequired)
}

fn advance_terminal(
    metadata: &mut CapacityStore,
    operation: &VaultOperation,
    transition: VaultOperationTransition,
    changed_at: &UtcTimestamp,
) -> Result<VaultOperation, VaultRecoveryExecutionError> {
    match metadata
        .advance_vault_operation(
            &operation.operation_id,
            operation.revision,
            transition,
            changed_at,
        )
        .map_err(|_| VaultRecoveryExecutionError::StoreUnavailable)?
    {
        VaultOperationAdvanceOutcome::Updated(operation) => Ok(operation),
        VaultOperationAdvanceOutcome::RevisionConflict(_)
        | VaultOperationAdvanceOutcome::Missing => {
            Err(VaultRecoveryExecutionError::OperationChanged)
        }
    }
}

fn load_same_operation(
    metadata: &CapacityStore,
    expected: &VaultOperation,
) -> Result<VaultOperation, VaultRecoveryExecutionError> {
    let operation = metadata
        .vault_operation(&expected.operation_id)
        .map_err(|_| VaultRecoveryExecutionError::StoreUnavailable)?
        .ok_or(VaultRecoveryExecutionError::OperationChanged)?;
    if operation.kind != expected.kind
        || operation.account_id != expected.account_id
        || operation.account_fingerprint != expected.account_fingerprint
        || operation.protected_record_ref != expected.protected_record_ref
    {
        return Err(VaultRecoveryExecutionError::OperationChanged);
    }
    Ok(operation)
}

fn operation_matches_account(operation: &VaultOperation, account: &VaultAccount) -> bool {
    operation.account_id.as_ref() == Some(&account.account_id)
        && operation.account_fingerprint == account.account_fingerprint
        && operation.protected_record_ref == account.protected_record_ref
        && operation.display_name == account.display_name
        && operation.auth_mode == account.auth_mode
        && operation.source == account.source
        && operation.lifecycle == account.lifecycle
        && operation.provider_id == account.provider_id
        && operation.model == account.model
        && operation
            .expected_account_revision
            .is_none_or(|revision| revision == account.revision)
}

fn verify_admission(
    lock: &MutationLock,
    legacy_probe: &mut impl LegacyViewerProbe,
) -> Result<(), VaultRecoveryExecutionError> {
    lock.verify_admission(legacy_probe)
        .map_err(VaultRecoveryExecutionError::Lock)
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use capacity_domain::{
        AccountFingerprint, VaultAccountAuthMode, VaultAccountLifecycle, VaultAccountRegistration,
        VaultAccountSource, VaultOperationTransition, VaultRecordRef,
    };
    use capacity_store::{
        VaultAccountRegistrationOutcome, VaultAccountRemovalOutcome, VaultOperationBeginOutcome,
    };
    use capacity_vault::{
        FileVault, ProtectedVaultRecord, VaultBackendError, VaultTransitionOutcome,
        journaled_register_account,
    };

    use super::*;
    use crate::{LegacyViewerState, MutationLockOwner, MutationOwnerId};

    const OWNER_UUID: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73601";
    const FINGERPRINT: &str = "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73401:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const RECORD_UUID: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73402";
    const TOKEN_CANARY: &str = "recovery-secret-token";
    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestRoot {
        base: PathBuf,
        lock: PathBuf,
    }

    impl TestRoot {
        fn new() -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock")
                .as_nanos();
            let base = std::env::temp_dir().join(format!(
                "capacity-recovery-test-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(&base).expect("create test root");
            let lock = base.join("lock");
            Self { base, lock }
        }

        fn store(&self) -> CapacityStore {
            CapacityStore::open(self.base.join("capacity.sqlite3")).expect("open store")
        }

        fn vault(&self) -> FileVault {
            FileVault::open(self.base.join("vault")).expect("open vault")
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let safe = self
                .base
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("capacity-recovery-test-"));
            if safe && self.base.starts_with(std::env::temp_dir()) {
                let _ = fs::remove_dir_all(&self.base);
            }
        }
    }

    #[derive(Default)]
    struct FakeLegacyProbe {
        states: VecDeque<LegacyViewerState>,
        calls: u32,
    }

    impl FakeLegacyProbe {
        fn with_states(states: impl IntoIterator<Item = LegacyViewerState>) -> Self {
            Self {
                states: states.into_iter().collect(),
                calls: 0,
            }
        }
    }

    impl LegacyViewerProbe for FakeLegacyProbe {
        fn legacy_viewer_state(&mut self) -> LegacyViewerState {
            self.calls += 1;
            self.states
                .pop_front()
                .unwrap_or(LegacyViewerState::NotRunning)
        }
    }

    fn timestamp(value: &str) -> UtcTimestamp {
        UtcTimestamp::parse(value).expect("timestamp")
    }

    fn owner() -> MutationLockOwner {
        MutationLockOwner::new(
            MutationOwnerId::parse(format!("mutation-owner:v1:{OWNER_UUID}")).expect("owner id"),
            std::process::id(),
            timestamp("2026-08-30T09:00:00Z"),
        )
        .expect("owner")
    }

    fn registration() -> VaultAccountRegistration {
        VaultAccountRegistration {
            account_fingerprint: AccountFingerprint::parse(FINGERPRINT).expect("fingerprint"),
            protected_record_ref: VaultRecordRef::parse(format!("vault-record:v1:{RECORD_UUID}"))
                .expect("record ref"),
            display_name: "Primary ChatGPT".into(),
            auth_mode: VaultAccountAuthMode::ChatGpt,
            source: VaultAccountSource::ManualChatGpt,
            lifecycle: VaultAccountLifecycle::Ready,
            provider_id: Some("official_codex".into()),
            model: Some("gpt-5.6-sol".into()),
        }
    }

    fn protected_record() -> ProtectedVaultRecord {
        ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_account_id",
            b"recovery-person@example.invalid".to_vec(),
            format!(r#"{{"auth_mode":"chatgpt","access_token":"{TOKEN_CANARY}"}}"#).into_bytes(),
            None,
        )
        .expect("protected record")
    }

    fn acquire(root: &TestRoot, probe: &mut FakeLegacyProbe) -> MutationLock {
        MutationLock::acquire(&root.lock, owner(), probe).expect("acquire lock")
    }

    fn begin_registration(store: &mut CapacityStore, changed_at: &str) -> VaultOperation {
        match store
            .begin_vault_registration_operation(&registration(), &timestamp(changed_at))
            .expect("begin registration")
        {
            VaultOperationBeginOutcome::Created(operation)
            | VaultOperationBeginOutcome::Existing(operation) => operation,
        }
    }

    fn begin_forget(
        store: &mut CapacityStore,
        account: &VaultAccount,
        changed_at: &str,
    ) -> VaultOperation {
        match store
            .begin_vault_forget_operation(account, &timestamp(changed_at))
            .expect("begin forget")
        {
            VaultOperationBeginOutcome::Created(operation)
            | VaultOperationBeginOutcome::Existing(operation) => operation,
        }
    }

    fn advance(
        store: &mut CapacityStore,
        operation: &VaultOperation,
        transition: VaultOperationTransition,
        changed_at: &str,
    ) -> VaultOperation {
        match store
            .advance_vault_operation(
                &operation.operation_id,
                operation.revision,
                transition,
                &timestamp(changed_at),
            )
            .expect("advance operation")
        {
            VaultOperationAdvanceOutcome::Updated(operation) => operation,
            outcome => panic!("unexpected advance outcome: {outcome:?}"),
        }
    }

    fn registered_account(store: &mut CapacityStore, vault: &mut FileVault) -> VaultAccount {
        journaled_register_account(
            store,
            vault,
            &registration(),
            &protected_record(),
            &timestamp("2026-08-30T09:01:00Z"),
        )
        .expect("register account")
        .receipt
        .account
    }

    #[test]
    fn registration_without_original_request_is_deferred_and_report_is_redacted() {
        let root = TestRoot::new();
        let mut store = root.store();
        let mut vault = root.vault();
        let operation = begin_registration(&mut store, "2026-08-30T09:01:00Z");
        let mut probe = FakeLegacyProbe::default();
        let lock = acquire(&root, &mut probe);

        let report = lock
            .recover_vault_operations(
                &mut probe,
                &mut store,
                &mut vault,
                16,
                &timestamp("2026-08-30T09:10:00Z"),
            )
            .expect("classify registration retry");
        assert_eq!(report.scanned, 1);
        assert_eq!(report.recovered, 0);
        assert_eq!(report.deferred, 1);
        assert_eq!(
            report.items[0].disposition,
            VaultRecoveryDisposition::RegistrationRequestRequired
        );
        assert_eq!(
            store
                .vault_operation(&operation.operation_id)
                .unwrap()
                .unwrap()
                .revision,
            operation.revision
        );
        let rendered = format!("{report:?}");
        for canary in [
            TOKEN_CANARY,
            FINGERPRINT,
            RECORD_UUID,
            OWNER_UUID,
            root.base.to_string_lossy().as_ref(),
        ] {
            assert!(!rendered.contains(canary));
        }
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn committed_registration_is_finalized_only_after_record_reverification() {
        let root = TestRoot::new();
        let mut store = root.store();
        let mut vault = root.vault();
        let operation = begin_registration(&mut store, "2026-08-30T09:01:00Z");
        vault
            .create_record(&registration().protected_record_ref, &protected_record())
            .unwrap();
        let operation = advance(
            &mut store,
            &operation,
            VaultOperationTransition::RecordReady,
            "2026-08-30T09:02:00Z",
        );
        let account = match store
            .register_vault_account(&registration(), &timestamp("2026-08-30T09:03:00Z"))
            .unwrap()
        {
            VaultAccountRegistrationOutcome::Created(account) => account,
            outcome => panic!("unexpected registration outcome: {outcome:?}"),
        };
        let operation = advance(
            &mut store,
            &operation,
            VaultOperationTransition::MetadataCommitted {
                account_id: account.account_id,
            },
            "2026-08-30T09:04:00Z",
        );
        let mut probe = FakeLegacyProbe::default();
        let lock = acquire(&root, &mut probe);

        let report = lock
            .recover_vault_operations(
                &mut probe,
                &mut store,
                &mut vault,
                16,
                &timestamp("2026-08-30T09:10:00Z"),
            )
            .unwrap();
        assert_eq!(report.recovered, 1);
        assert_eq!(
            report.items[0].disposition,
            VaultRecoveryDisposition::Succeeded
        );
        assert_eq!(
            store
                .vault_operation(&operation.operation_id)
                .unwrap()
                .unwrap()
                .status,
            VaultOperationStatus::Succeeded
        );
    }

    #[test]
    fn quarantined_registration_is_finalized_as_compensated() {
        let root = TestRoot::new();
        let mut store = root.store();
        let mut vault = root.vault();
        let operation = begin_registration(&mut store, "2026-08-30T09:01:00Z");
        vault
            .create_record(&registration().protected_record_ref, &protected_record())
            .unwrap();
        let operation = advance(
            &mut store,
            &operation,
            VaultOperationTransition::RecordReady,
            "2026-08-30T09:02:00Z",
        );
        vault
            .quarantine_record(&registration().protected_record_ref)
            .unwrap();
        let operation = advance(
            &mut store,
            &operation,
            VaultOperationTransition::RecordQuarantined,
            "2026-08-30T09:03:00Z",
        );
        let mut probe = FakeLegacyProbe::default();
        let lock = acquire(&root, &mut probe);

        let report = lock
            .recover_vault_operations(
                &mut probe,
                &mut store,
                &mut vault,
                16,
                &timestamp("2026-08-30T09:10:00Z"),
            )
            .unwrap();
        assert_eq!(report.recovered, 1);
        assert_eq!(
            report.items[0].disposition,
            VaultRecoveryDisposition::Compensated
        );
        let terminal = store
            .vault_operation(&operation.operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(terminal.status, VaultOperationStatus::Compensated);
        assert_eq!(
            terminal.last_error_code.as_deref(),
            Some(RECOVERED_COMPENSATION)
        );
    }

    #[test]
    fn prepared_forget_reconstructs_snapshot_after_metadata_disappears() {
        let root = TestRoot::new();
        let mut store = root.store();
        let mut vault = root.vault();
        let account = registered_account(&mut store, &mut vault);
        let operation = begin_forget(&mut store, &account, "2026-08-30T09:02:00Z");
        vault
            .quarantine_record(&account.protected_record_ref)
            .unwrap();
        assert!(matches!(
            store
                .remove_vault_account_metadata(
                    &account.account_id,
                    account.revision,
                    &timestamp("2026-08-30T09:03:00Z")
                )
                .unwrap(),
            VaultAccountRemovalOutcome::Removed(_)
        ));
        let mut probe = FakeLegacyProbe::default();
        let lock = acquire(&root, &mut probe);

        let report = lock
            .recover_vault_operations(
                &mut probe,
                &mut store,
                &mut vault,
                16,
                &timestamp("2026-08-30T09:10:00Z"),
            )
            .unwrap();
        assert_eq!(report.recovered, 1);
        assert_eq!(report.items[0].action, VaultRecoveryAction::ResumeForget);
        assert_eq!(
            report.items[0].disposition,
            VaultRecoveryDisposition::Succeeded
        );
        assert_eq!(
            store
                .vault_operation(&operation.operation_id)
                .unwrap()
                .unwrap()
                .status,
            VaultOperationStatus::Succeeded
        );
    }

    #[test]
    fn metadata_removed_forget_is_finalized_as_succeeded() {
        let root = TestRoot::new();
        let mut store = root.store();
        let mut vault = root.vault();
        let account = registered_account(&mut store, &mut vault);
        let operation = begin_forget(&mut store, &account, "2026-08-30T09:02:00Z");
        vault
            .quarantine_record(&account.protected_record_ref)
            .unwrap();
        let operation = advance(
            &mut store,
            &operation,
            VaultOperationTransition::RecordQuarantined,
            "2026-08-30T09:03:00Z",
        );
        store
            .remove_vault_account_metadata(
                &account.account_id,
                account.revision,
                &timestamp("2026-08-30T09:04:00Z"),
            )
            .unwrap();
        let operation = advance(
            &mut store,
            &operation,
            VaultOperationTransition::MetadataRemoved,
            "2026-08-30T09:05:00Z",
        );
        let mut probe = FakeLegacyProbe::default();
        let lock = acquire(&root, &mut probe);

        let report = lock
            .recover_vault_operations(
                &mut probe,
                &mut store,
                &mut vault,
                16,
                &timestamp("2026-08-30T09:10:00Z"),
            )
            .unwrap();
        assert_eq!(
            report.items[0].disposition,
            VaultRecoveryDisposition::Succeeded
        );
        assert_eq!(
            store
                .vault_operation(&operation.operation_id)
                .unwrap()
                .unwrap()
                .status,
            VaultOperationStatus::Succeeded
        );
    }

    #[test]
    fn restored_forget_is_finalized_as_compensated() {
        let root = TestRoot::new();
        let mut store = root.store();
        let mut vault = root.vault();
        let account = registered_account(&mut store, &mut vault);
        let operation = begin_forget(&mut store, &account, "2026-08-30T09:02:00Z");
        vault
            .quarantine_record(&account.protected_record_ref)
            .unwrap();
        let operation = advance(
            &mut store,
            &operation,
            VaultOperationTransition::RecordQuarantined,
            "2026-08-30T09:03:00Z",
        );
        vault.restore_record(&account.protected_record_ref).unwrap();
        let operation = advance(
            &mut store,
            &operation,
            VaultOperationTransition::RecordRestored,
            "2026-08-30T09:04:00Z",
        );
        let mut probe = FakeLegacyProbe::default();
        let lock = acquire(&root, &mut probe);

        let report = lock
            .recover_vault_operations(
                &mut probe,
                &mut store,
                &mut vault,
                16,
                &timestamp("2026-08-30T09:10:00Z"),
            )
            .unwrap();
        assert_eq!(
            report.items[0].disposition,
            VaultRecoveryDisposition::Compensated
        );
        assert_eq!(
            store
                .vault_operation(&operation.operation_id)
                .unwrap()
                .unwrap()
                .status,
            VaultOperationStatus::Compensated
        );
    }

    #[test]
    fn needs_review_operation_remains_deferred_without_revision_change() {
        let root = TestRoot::new();
        let mut store = root.store();
        let mut vault = root.vault();
        let operation = begin_registration(&mut store, "2026-08-30T09:01:00Z");
        let operation = advance(
            &mut store,
            &operation,
            VaultOperationTransition::NeedsReview {
                reason_code: "operator_review_required".into(),
            },
            "2026-08-30T09:02:00Z",
        );
        let mut probe = FakeLegacyProbe::default();
        let lock = acquire(&root, &mut probe);

        let report = lock
            .recover_vault_operations(
                &mut probe,
                &mut store,
                &mut vault,
                16,
                &timestamp("2026-08-30T09:10:00Z"),
            )
            .unwrap();
        assert_eq!(report.deferred, 1);
        assert_eq!(
            report.items[0].disposition,
            VaultRecoveryDisposition::ManualReviewRequired
        );
        assert_eq!(
            store
                .vault_operation(&operation.operation_id)
                .unwrap()
                .unwrap()
                .revision,
            operation.revision
        );
    }

    struct QuarantineBeforeFinalization {
        inner: FileVault,
        state_calls: u32,
    }

    impl ProtectedRecordBackend for QuarantineBeforeFinalization {
        fn record_state(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<VaultRecordState, VaultBackendError> {
            self.state_calls += 1;
            if self.state_calls == 2 {
                self.inner.quarantine_record(record_ref)?;
            }
            self.inner.record_state(record_ref)
        }

        fn create_record(
            &mut self,
            record_ref: &VaultRecordRef,
            record: &ProtectedVaultRecord,
        ) -> Result<(), VaultBackendError> {
            self.inner.create_record(record_ref, record)
        }

        fn read_record(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<ProtectedVaultRecord, VaultBackendError> {
            self.inner.read_record(record_ref)
        }

        fn quarantine_record(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<VaultTransitionOutcome, VaultBackendError> {
            self.inner.quarantine_record(record_ref)
        }

        fn restore_record(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<VaultTransitionOutcome, VaultBackendError> {
            self.inner.restore_record(record_ref)
        }
    }

    #[test]
    fn postcondition_drift_pauses_instead_of_claiming_success() {
        let root = TestRoot::new();
        let mut store = root.store();
        let vault = root.vault();
        let operation = begin_registration(&mut store, "2026-08-30T09:01:00Z");
        vault
            .create_record(&registration().protected_record_ref, &protected_record())
            .unwrap();
        let operation = advance(
            &mut store,
            &operation,
            VaultOperationTransition::RecordReady,
            "2026-08-30T09:02:00Z",
        );
        let account = match store
            .register_vault_account(&registration(), &timestamp("2026-08-30T09:03:00Z"))
            .unwrap()
        {
            VaultAccountRegistrationOutcome::Created(account) => account,
            outcome => panic!("unexpected registration outcome: {outcome:?}"),
        };
        let operation = advance(
            &mut store,
            &operation,
            VaultOperationTransition::MetadataCommitted {
                account_id: account.account_id,
            },
            "2026-08-30T09:04:00Z",
        );
        let mut changing = QuarantineBeforeFinalization {
            inner: vault,
            state_calls: 0,
        };
        let mut probe = FakeLegacyProbe::default();
        let lock = acquire(&root, &mut probe);

        let report = lock
            .recover_vault_operations(
                &mut probe,
                &mut store,
                &mut changing,
                16,
                &timestamp("2026-08-30T09:10:00Z"),
            )
            .unwrap();
        assert_eq!(report.recovered, 0);
        assert_eq!(report.deferred, 1);
        assert_eq!(
            report.items[0].disposition,
            VaultRecoveryDisposition::ManualReviewRequired
        );
        let paused = store
            .vault_operation(&operation.operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(paused.status, VaultOperationStatus::NeedsReview);
        assert_eq!(
            paused.last_error_code.as_deref(),
            Some(RECOVERY_POSTCONDITION_FAILED)
        );
    }

    #[test]
    fn invalid_bound_and_late_legacy_viewer_fail_closed() {
        let invalid_root = TestRoot::new();
        let mut invalid_store = invalid_root.store();
        let mut invalid_vault = invalid_root.vault();
        let mut invalid_probe = FakeLegacyProbe::default();
        let invalid_lock = acquire(&invalid_root, &mut invalid_probe);
        assert_eq!(
            invalid_lock.recover_vault_operations(
                &mut invalid_probe,
                &mut invalid_store,
                &mut invalid_vault,
                0,
                &timestamp("2026-08-30T09:10:00Z"),
            ),
            Err(VaultRecoveryExecutionError::Vault(
                AccountVaultError::OperationJournalUnavailable
            ))
        );

        let root = TestRoot::new();
        let mut store = root.store();
        let mut vault = root.vault();
        let operation = begin_registration(&mut store, "2026-08-30T09:01:00Z");
        let mut probe = FakeLegacyProbe::with_states([
            LegacyViewerState::NotRunning,
            LegacyViewerState::NotRunning,
            LegacyViewerState::NotRunning,
            LegacyViewerState::NotRunning,
            LegacyViewerState::Running,
        ]);
        let lock = acquire(&root, &mut probe);
        assert_eq!(
            lock.recover_vault_operations(
                &mut probe,
                &mut store,
                &mut vault,
                16,
                &timestamp("2026-08-30T09:10:00Z"),
            ),
            Err(VaultRecoveryExecutionError::Lock(
                MutationLockError::LegacyViewerRunning
            ))
        );
        assert_eq!(probe.calls, 5);
        assert_eq!(
            store
                .vault_operation(&operation.operation_id)
                .unwrap()
                .unwrap()
                .revision,
            operation.revision
        );
    }
}
