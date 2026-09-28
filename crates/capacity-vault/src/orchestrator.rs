use std::fmt;

use capacity_domain::{
    UtcTimestamp, VaultAccount, VaultAccountRegistration, VaultRecordRef, VaultValidationError,
};
use capacity_store::{
    CapacityStore, StoreError, VaultAccountRegistrationOutcome, VaultAccountRemoval,
    VaultAccountRemovalOutcome,
};

use crate::{
    FileVault, ProtectedVaultRecord, VaultBackendError, VaultRecordState, VaultTransitionOutcome,
};

/// Narrow seam shared by the permission-mode fallback and future platform
/// credential-store adapters. Implementations must keep errors payload-free.
pub trait ProtectedRecordBackend {
    fn record_state(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultRecordState, VaultBackendError>;

    fn create_record(
        &mut self,
        record_ref: &VaultRecordRef,
        record: &ProtectedVaultRecord,
    ) -> Result<(), VaultBackendError>;

    fn read_record(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<ProtectedVaultRecord, VaultBackendError>;

    fn quarantine_record(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultTransitionOutcome, VaultBackendError>;

    fn restore_record(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultTransitionOutcome, VaultBackendError>;
}

impl ProtectedRecordBackend for FileVault {
    fn record_state(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultRecordState, VaultBackendError> {
        FileVault::record_state(self, record_ref)
    }

    fn create_record(
        &mut self,
        record_ref: &VaultRecordRef,
        record: &ProtectedVaultRecord,
    ) -> Result<(), VaultBackendError> {
        FileVault::create_record(self, record_ref, record)
    }

    fn read_record(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<ProtectedVaultRecord, VaultBackendError> {
        FileVault::read_record(self, record_ref)
    }

    fn quarantine_record(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultTransitionOutcome, VaultBackendError> {
        FileVault::quarantine_record(self, record_ref)
    }

    fn restore_record(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultTransitionOutcome, VaultBackendError> {
        FileVault::restore_record(self, record_ref)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompensationOutcome {
    NotRequired,
    Quarantined,
    Restored,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataFailureReason {
    Unavailable,
    CapacityExceeded,
    IdentityRecordConflict,
    RevisionConflict,
    InvariantViolation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRegistrationReceipt {
    pub account: VaultAccount,
    pub metadata_created: bool,
    pub protected_record_created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgetAccountOutcome {
    Forgotten {
        removal: Box<VaultAccountRemoval>,
        record_was_already_quarantined: bool,
    },
    AlreadyForgotten {
        recovery_record_present: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountVaultError {
    InvalidRegistration,
    ProtectedBackend(VaultBackendError),
    ProtectedRecordContentsConflict,
    ProtectedRecordState(VaultRecordState),
    MetadataLookupFailed,
    MetadataSnapshotStale,
    MetadataRegistrationFailed {
        reason: MetadataFailureReason,
        compensation: CompensationOutcome,
    },
    MetadataRemovalFailed {
        reason: MetadataFailureReason,
        compensation: CompensationOutcome,
    },
    OperationJournalConflict,
    OperationJournalFull,
    OperationJournalUnavailable,
    OrphanedProtectedRecord,
}

impl fmt::Display for AccountVaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidRegistration => "account-vault registration is invalid",
            Self::ProtectedBackend(_) => "protected-record backend operation failed",
            Self::ProtectedRecordContentsConflict => {
                "protected-record contents conflict with the requested account"
            }
            Self::ProtectedRecordState(_) => "protected record is not in the required state",
            Self::MetadataLookupFailed => "account metadata lookup failed",
            Self::MetadataSnapshotStale => "account metadata snapshot is stale",
            Self::MetadataRegistrationFailed { .. } => "account metadata registration failed",
            Self::MetadataRemovalFailed { .. } => "account metadata removal failed",
            Self::OperationJournalConflict => "account-vault operation conflicts with active work",
            Self::OperationJournalFull => "account-vault operation journal is full",
            Self::OperationJournalUnavailable => "account-vault operation journal is unavailable",
            Self::OrphanedProtectedRecord => {
                "live protected record has no matching account metadata"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for AccountVaultError {}

/// Makes one protected record and one metadata account converge on the same
/// fingerprint/reference pair. Exact retries are idempotent. The caller must
/// retain the same request and record reference across crash recovery; durable
/// pending-operation discovery belongs to a later operation-journal package.
pub fn register_account<B: ProtectedRecordBackend>(
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    registration: &VaultAccountRegistration,
    protected_record: &ProtectedVaultRecord,
    created_at: &UtcTimestamp,
) -> Result<AccountRegistrationReceipt, AccountVaultError> {
    registration
        .validate()
        .map_err(|_error: VaultValidationError| AccountVaultError::InvalidRegistration)?;

    if let Some(existing) = metadata
        .vault_account_by_fingerprint(&registration.account_fingerprint)
        .map_err(|_| AccountVaultError::MetadataLookupFailed)?
    {
        if existing.protected_record_ref != registration.protected_record_ref {
            return Err(AccountVaultError::MetadataRegistrationFailed {
                reason: MetadataFailureReason::IdentityRecordConflict,
                compensation: CompensationOutcome::NotRequired,
            });
        }
        let protected_record_created = ensure_live_record(
            protected_backend,
            &registration.protected_record_ref,
            protected_record,
        )?;
        return Ok(AccountRegistrationReceipt {
            account: existing,
            metadata_created: false,
            protected_record_created,
        });
    }

    let protected_record_created = ensure_live_record(
        protected_backend,
        &registration.protected_record_ref,
        protected_record,
    )?;

    match metadata.register_vault_account(registration, created_at) {
        Ok(VaultAccountRegistrationOutcome::Created(account)) => finish_registration(
            metadata,
            protected_backend,
            registration,
            account,
            true,
            protected_record_created,
        ),
        Ok(VaultAccountRegistrationOutcome::Existing(account)) => finish_registration(
            metadata,
            protected_backend,
            registration,
            account,
            false,
            protected_record_created,
        ),
        Err(error) => {
            match metadata.vault_account_by_fingerprint(&registration.account_fingerprint) {
                Ok(Some(account)) => finish_registration(
                    metadata,
                    protected_backend,
                    registration,
                    account,
                    false,
                    protected_record_created,
                ),
                Ok(None) | Err(_) => Err(AccountVaultError::MetadataRegistrationFailed {
                    reason: metadata_failure_reason(&error),
                    compensation: quarantine_new_record(
                        metadata,
                        protected_backend,
                        &registration.protected_record_ref,
                        protected_record_created,
                    ),
                }),
            }
        }
    }
}

/// Quarantines a verified protected record before removing its revisioned
/// metadata. A stale preflight never touches the record. If removal fails while
/// metadata still exists, the record is restored to the live set.
pub fn forget_account<B: ProtectedRecordBackend>(
    metadata: &mut CapacityStore,
    protected_backend: &mut B,
    expected_account: &VaultAccount,
    removed_at: &UtcTimestamp,
) -> Result<ForgetAccountOutcome, AccountVaultError> {
    let current = metadata
        .vault_account(&expected_account.account_id)
        .map_err(|_| AccountVaultError::MetadataLookupFailed)?;
    let Some(current) = current else {
        return already_forgotten_or_orphaned(
            protected_backend,
            &expected_account.protected_record_ref,
        );
    };
    if current.revision != expected_account.revision
        || current.account_fingerprint != expected_account.account_fingerprint
        || current.protected_record_ref != expected_account.protected_record_ref
    {
        return Err(AccountVaultError::MetadataSnapshotStale);
    }

    let record_was_already_quarantined = match protected_backend
        .record_state(&current.protected_record_ref)
        .map_err(AccountVaultError::ProtectedBackend)?
    {
        VaultRecordState::Present => false,
        VaultRecordState::Quarantined => true,
        state @ (VaultRecordState::Missing | VaultRecordState::Conflict) => {
            return Err(AccountVaultError::ProtectedRecordState(state));
        }
    };
    let transition = protected_backend
        .quarantine_record(&current.protected_record_ref)
        .map_err(AccountVaultError::ProtectedBackend)?;
    let record_was_already_quarantined = record_was_already_quarantined
        || transition == VaultTransitionOutcome::AlreadyAtDestination;

    match metadata.remove_vault_account_metadata(&current.account_id, current.revision, removed_at)
    {
        Ok(VaultAccountRemovalOutcome::Removed(removal)) => Ok(ForgetAccountOutcome::Forgotten {
            removal: Box::new(removal),
            record_was_already_quarantined,
        }),
        Ok(VaultAccountRemovalOutcome::Missing) => metadata_missing_after_quarantine(
            metadata,
            protected_backend,
            &current.protected_record_ref,
        ),
        Ok(VaultAccountRemovalOutcome::RevisionConflict(_)) => {
            Err(AccountVaultError::MetadataRemovalFailed {
                reason: MetadataFailureReason::RevisionConflict,
                compensation: restore_quarantined_record(
                    protected_backend,
                    &current.protected_record_ref,
                ),
            })
        }
        Err(error) => match metadata.vault_account(&current.account_id) {
            Ok(None) => metadata_missing_after_quarantine(
                metadata,
                protected_backend,
                &current.protected_record_ref,
            ),
            Ok(Some(_)) | Err(_) => Err(AccountVaultError::MetadataRemovalFailed {
                reason: metadata_failure_reason(&error),
                compensation: restore_quarantined_record(
                    protected_backend,
                    &current.protected_record_ref,
                ),
            }),
        },
    }
}

pub(crate) fn ensure_live_record<B: ProtectedRecordBackend>(
    protected_backend: &mut B,
    record_ref: &VaultRecordRef,
    requested: &ProtectedVaultRecord,
) -> Result<bool, AccountVaultError> {
    match protected_backend
        .record_state(record_ref)
        .map_err(AccountVaultError::ProtectedBackend)?
    {
        VaultRecordState::Missing => {
            protected_backend
                .create_record(record_ref, requested)
                .map_err(AccountVaultError::ProtectedBackend)?;
            Ok(true)
        }
        VaultRecordState::Present => {
            let existing = protected_backend
                .read_record(record_ref)
                .map_err(AccountVaultError::ProtectedBackend)?;
            if protected_records_equal(&existing, requested) {
                Ok(false)
            } else {
                Err(AccountVaultError::ProtectedRecordContentsConflict)
            }
        }
        state @ (VaultRecordState::Quarantined | VaultRecordState::Conflict) => {
            Err(AccountVaultError::ProtectedRecordState(state))
        }
    }
}

fn finish_registration<B: ProtectedRecordBackend>(
    metadata: &CapacityStore,
    protected_backend: &mut B,
    registration: &VaultAccountRegistration,
    account: VaultAccount,
    metadata_created: bool,
    protected_record_created: bool,
) -> Result<AccountRegistrationReceipt, AccountVaultError> {
    if account.account_fingerprint != registration.account_fingerprint
        || account.protected_record_ref != registration.protected_record_ref
    {
        return Err(AccountVaultError::MetadataRegistrationFailed {
            reason: MetadataFailureReason::IdentityRecordConflict,
            compensation: quarantine_new_record(
                metadata,
                protected_backend,
                &registration.protected_record_ref,
                protected_record_created,
            ),
        });
    }
    Ok(AccountRegistrationReceipt {
        account,
        metadata_created,
        protected_record_created,
    })
}

fn already_forgotten_or_orphaned<B: ProtectedRecordBackend>(
    protected_backend: &mut B,
    record_ref: &VaultRecordRef,
) -> Result<ForgetAccountOutcome, AccountVaultError> {
    match protected_backend
        .record_state(record_ref)
        .map_err(AccountVaultError::ProtectedBackend)?
    {
        VaultRecordState::Missing => Ok(ForgetAccountOutcome::AlreadyForgotten {
            recovery_record_present: false,
        }),
        VaultRecordState::Quarantined => {
            protected_backend
                .quarantine_record(record_ref)
                .map_err(AccountVaultError::ProtectedBackend)?;
            Ok(ForgetAccountOutcome::AlreadyForgotten {
                recovery_record_present: true,
            })
        }
        VaultRecordState::Present => Err(AccountVaultError::OrphanedProtectedRecord),
        VaultRecordState::Conflict => Err(AccountVaultError::ProtectedRecordState(
            VaultRecordState::Conflict,
        )),
    }
}

fn quarantine_new_record<B: ProtectedRecordBackend>(
    metadata: &CapacityStore,
    protected_backend: &mut B,
    record_ref: &VaultRecordRef,
    protected_record_created: bool,
) -> CompensationOutcome {
    if !protected_record_created {
        return CompensationOutcome::NotRequired;
    }
    match metadata.vault_account_by_record_ref(record_ref) {
        Ok(Some(_)) => return CompensationOutcome::NotRequired,
        Err(_) => return CompensationOutcome::Failed,
        Ok(None) => {}
    }
    match protected_backend.quarantine_record(record_ref) {
        Ok(VaultTransitionOutcome::Moved | VaultTransitionOutcome::AlreadyAtDestination) => {
            CompensationOutcome::Quarantined
        }
        Err(_) => CompensationOutcome::Failed,
    }
}

fn metadata_missing_after_quarantine<B: ProtectedRecordBackend>(
    metadata: &CapacityStore,
    protected_backend: &mut B,
    record_ref: &VaultRecordRef,
) -> Result<ForgetAccountOutcome, AccountVaultError> {
    match metadata.vault_account_by_record_ref(record_ref) {
        Ok(None) => Ok(ForgetAccountOutcome::AlreadyForgotten {
            recovery_record_present: true,
        }),
        Ok(Some(_)) => Err(AccountVaultError::MetadataRemovalFailed {
            reason: MetadataFailureReason::IdentityRecordConflict,
            compensation: restore_quarantined_record(protected_backend, record_ref),
        }),
        Err(_) => Err(AccountVaultError::MetadataRemovalFailed {
            reason: MetadataFailureReason::Unavailable,
            compensation: restore_quarantined_record(protected_backend, record_ref),
        }),
    }
}

fn restore_quarantined_record<B: ProtectedRecordBackend>(
    protected_backend: &mut B,
    record_ref: &VaultRecordRef,
) -> CompensationOutcome {
    match protected_backend.restore_record(record_ref) {
        Ok(VaultTransitionOutcome::Moved | VaultTransitionOutcome::AlreadyAtDestination) => {
            CompensationOutcome::Restored
        }
        Err(_) => CompensationOutcome::Failed,
    }
}

pub(crate) fn protected_records_equal(
    left: &ProtectedVaultRecord,
    right: &ProtectedVaultRecord,
) -> bool {
    left.adapter_namespace() == right.adapter_namespace()
        && left.identity_kind() == right.identity_kind()
        && left.upstream_identity() == right.upstream_identity()
        && left.auth_json() == right.auth_json()
        && left.config_toml() == right.config_toml()
}

pub(crate) fn metadata_failure_reason(error: &StoreError) -> MetadataFailureReason {
    match error {
        StoreError::VaultCatalogFull => MetadataFailureReason::CapacityExceeded,
        StoreError::VaultRecordConflict => MetadataFailureReason::IdentityRecordConflict,
        StoreError::UnsupportedSchema { .. }
        | StoreError::CorruptEnum(_)
        | StoreError::SchemaInvariant => MetadataFailureReason::InvariantViolation,
        _ => MetadataFailureReason::Unavailable,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use capacity_domain::{
        AccountFingerprint, EnvironmentSnapshot, VaultAccountAuthMode, VaultAccountLifecycle,
        VaultAccountSource,
    };
    use capacity_store::{VaultAccountMutationOutcome, VaultEnvironmentStateUpdateOutcome};

    use super::*;

    const FINGERPRINT: &str = "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73331:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const FINGERPRINT_2: &str = "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73331:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    const RECORD_UUID: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73332";
    const RECORD_UUID_2: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73333";
    const IDENTITY_CANARY: &str = "orchestrator-person@example.invalid";
    const TOKEN_CANARY: &str = "orchestrator-secret-token";
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
                "codex-capacity-orchestrator-test-{}-{epoch_nanos}-{sequence}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(&path).expect("create test root");
            Self { path }
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
                .is_some_and(|name| name.starts_with("codex-capacity-orchestrator-test-"));
            if self.path.parent() == Some(std::env::temp_dir().as_path())
                && safe_name
                && let Ok(metadata) = fs::symlink_metadata(&self.path)
                && metadata.is_dir()
                && !metadata.file_type().is_symlink()
            {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }

    fn timestamp(value: &str) -> UtcTimestamp {
        UtcTimestamp::parse(value).expect("valid UTC timestamp")
    }

    fn record_ref(uuid: &str) -> VaultRecordRef {
        VaultRecordRef::parse(format!("vault-record:v1:{uuid}")).expect("valid record reference")
    }

    fn registration(
        fingerprint: &str,
        record_uuid: &str,
        display_name: &str,
    ) -> VaultAccountRegistration {
        VaultAccountRegistration {
            account_fingerprint: AccountFingerprint::parse(fingerprint).expect("fingerprint"),
            protected_record_ref: record_ref(record_uuid),
            display_name: display_name.into(),
            auth_mode: VaultAccountAuthMode::ChatGpt,
            source: VaultAccountSource::ManualChatGpt,
            lifecycle: VaultAccountLifecycle::Ready,
            provider_id: None,
            model: None,
        }
    }

    fn protected_record(token: &str) -> ProtectedVaultRecord {
        ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_account_id",
            IDENTITY_CANARY.as_bytes().to_vec(),
            format!(r#"{{"auth_mode":"chatgpt","access_token":"{token}"}}"#).into_bytes(),
            None,
        )
        .expect("valid protected record")
    }

    fn environment() -> EnvironmentSnapshot {
        EnvironmentSnapshot {
            environment_id: "macos-arm64-native".into(),
            platform: "macos".into(),
            architecture: "arm64".into(),
            boundary: "native".into(),
        }
    }

    #[test]
    fn registration_is_ordered_and_exact_retry_is_idempotent() {
        let root = TestRoot::new();
        let mut backend = root.file_vault();
        let mut metadata = CapacityStore::open_in_memory().expect("open metadata");
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");

        let created = register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(TOKEN_CANARY),
            &timestamp("2026-08-30T07:00:00Z"),
        )
        .expect("register account");
        assert!(created.metadata_created);
        assert!(created.protected_record_created);
        assert_eq!(
            backend.record_state(&registration.protected_record_ref),
            Ok(VaultRecordState::Present)
        );

        let retried = register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(TOKEN_CANARY),
            &timestamp("2026-08-30T07:01:00Z"),
        )
        .expect("retry exact registration");
        assert!(!retried.metadata_created);
        assert!(!retried.protected_record_created);
        assert_eq!(retried.account, created.account);
        assert_eq!(
            metadata
                .vault_catalog("macos-arm64-native")
                .unwrap()
                .accounts
                .len(),
            1
        );
    }

    #[test]
    fn registration_resumes_after_record_create_before_metadata_commit() {
        let root = TestRoot::new();
        let mut backend = root.file_vault();
        let mut metadata = CapacityStore::open_in_memory().expect("open metadata");
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        backend
            .create_record(
                &registration.protected_record_ref,
                &protected_record(TOKEN_CANARY),
            )
            .expect("simulate pre-crash record create");

        let receipt = register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(TOKEN_CANARY),
            &timestamp("2026-08-30T07:00:00Z"),
        )
        .expect("resume registration");
        assert!(receipt.metadata_created);
        assert!(!receipt.protected_record_created);
    }

    #[test]
    fn registration_rejects_existing_record_with_different_secret_bytes() {
        let root = TestRoot::new();
        let mut backend = root.file_vault();
        let mut metadata = CapacityStore::open_in_memory().expect("open metadata");
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        backend
            .create_record(
                &registration.protected_record_ref,
                &protected_record("first-secret-canary"),
            )
            .expect("create conflicting record");

        let error = register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record("second-secret-canary"),
            &timestamp("2026-08-30T07:00:00Z"),
        )
        .expect_err("different bytes must fail closed");
        assert_eq!(error, AccountVaultError::ProtectedRecordContentsConflict);
        assert!(
            metadata
                .vault_catalog("macos-arm64-native")
                .unwrap()
                .accounts
                .is_empty()
        );
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("first-secret-canary"));
        assert!(!rendered.contains("second-secret-canary"));
    }

    #[test]
    fn invalid_registration_has_no_protected_record_side_effect() {
        let root = TestRoot::new();
        let mut backend = root.file_vault();
        let mut metadata = CapacityStore::open_in_memory().expect("open metadata");
        let mut invalid = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        invalid.display_name = " Primary ChatGPT ".into();

        assert_eq!(
            register_account(
                &mut metadata,
                &mut backend,
                &invalid,
                &protected_record(TOKEN_CANARY),
                &timestamp("2026-08-30T07:00:00Z"),
            ),
            Err(AccountVaultError::InvalidRegistration)
        );
        assert_eq!(
            backend.record_state(&invalid.protected_record_ref),
            Ok(VaultRecordState::Missing)
        );
    }

    #[test]
    fn existing_identity_with_another_record_ref_has_no_backend_side_effect() {
        let root = TestRoot::new();
        let mut backend = root.file_vault();
        let mut metadata = CapacityStore::open_in_memory().expect("open metadata");
        let existing = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        assert!(matches!(
            metadata.register_vault_account(&existing, &timestamp("2026-08-30T07:00:00Z")),
            Ok(VaultAccountRegistrationOutcome::Created(_))
        ));
        let candidate = registration(FINGERPRINT, RECORD_UUID_2, "Duplicate ChatGPT");

        assert_eq!(
            register_account(
                &mut metadata,
                &mut backend,
                &candidate,
                &protected_record(TOKEN_CANARY),
                &timestamp("2026-08-30T07:01:00Z"),
            ),
            Err(AccountVaultError::MetadataRegistrationFailed {
                reason: MetadataFailureReason::IdentityRecordConflict,
                compensation: CompensationOutcome::NotRequired,
            })
        );
        assert_eq!(
            backend.record_state(&candidate.protected_record_ref),
            Ok(VaultRecordState::Missing)
        );
    }

    #[test]
    fn metadata_capacity_failure_quarantines_only_the_new_record() {
        let root = TestRoot::new();
        let mut backend = root.file_vault();
        let mut metadata = CapacityStore::open_in_memory().expect("open metadata");
        let key_id = "018f47a2-8a71-7f4a-9c35-1f4234a73340";
        for index in 0_u16..1024 {
            let filled = registration(
                &format!("hmac-sha256:v1:{key_id}:{index:064x}"),
                &format!("00000000-0000-0000-0000-{index:012x}"),
                &format!("Synthetic account {index}"),
            );
            assert!(matches!(
                metadata.register_vault_account(&filled, &timestamp("2026-08-30T07:00:00Z")),
                Ok(VaultAccountRegistrationOutcome::Created(_))
            ));
        }
        let candidate = registration(
            &format!("hmac-sha256:v1:{key_id}:{:064x}", 1024_u16),
            RECORD_UUID,
            "Overflow account",
        );

        assert_eq!(
            register_account(
                &mut metadata,
                &mut backend,
                &candidate,
                &protected_record(TOKEN_CANARY),
                &timestamp("2026-08-30T07:01:00Z"),
            ),
            Err(AccountVaultError::MetadataRegistrationFailed {
                reason: MetadataFailureReason::CapacityExceeded,
                compensation: CompensationOutcome::Quarantined,
            })
        );
        assert_eq!(
            backend.record_state(&candidate.protected_record_ref),
            Ok(VaultRecordState::Quarantined)
        );
    }

    #[test]
    fn forget_quarantines_before_metadata_removal_and_is_idempotent() {
        let root = TestRoot::new();
        let mut backend = root.file_vault();
        let mut metadata = CapacityStore::open_in_memory().expect("open metadata");
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let account = register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(TOKEN_CANARY),
            &timestamp("2026-08-30T07:00:00Z"),
        )
        .unwrap()
        .account;
        assert!(matches!(
            metadata.set_vault_environment_state(
                &environment(),
                0,
                Some(&account.account_id),
                Some(&account.account_id),
                &timestamp("2026-08-30T07:01:00Z"),
            ),
            Ok(VaultEnvironmentStateUpdateOutcome::Updated(_))
        ));

        let forgotten = forget_account(
            &mut metadata,
            &mut backend,
            &account,
            &timestamp("2026-08-30T07:02:00Z"),
        )
        .expect("forget account");
        let ForgetAccountOutcome::Forgotten {
            removal,
            record_was_already_quarantined,
        } = forgotten
        else {
            panic!("first forget must remove metadata");
        };
        assert!(!record_was_already_quarantined);
        assert_eq!(removal.cleared_environment_states, 1);
        assert_eq!(
            backend.record_state(&registration.protected_record_ref),
            Ok(VaultRecordState::Quarantined)
        );
        assert!(
            metadata
                .vault_account(&account.account_id)
                .unwrap()
                .is_none()
        );

        assert_eq!(
            forget_account(
                &mut metadata,
                &mut backend,
                &account,
                &timestamp("2026-08-30T07:03:00Z"),
            ),
            Ok(ForgetAccountOutcome::AlreadyForgotten {
                recovery_record_present: true,
            })
        );
    }

    #[test]
    fn stale_forget_request_never_touches_the_live_record() {
        let root = TestRoot::new();
        let mut backend = root.file_vault();
        let mut metadata = CapacityStore::open_in_memory().expect("open metadata");
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let account = register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(TOKEN_CANARY),
            &timestamp("2026-08-30T07:00:00Z"),
        )
        .unwrap()
        .account;
        assert!(matches!(
            metadata.rename_vault_account(
                &account.account_id,
                account.revision,
                "Renamed account",
                &timestamp("2026-08-30T07:01:00Z"),
            ),
            Ok(VaultAccountMutationOutcome::Updated(_))
        ));

        assert_eq!(
            forget_account(
                &mut metadata,
                &mut backend,
                &account,
                &timestamp("2026-08-30T07:02:00Z"),
            ),
            Err(AccountVaultError::MetadataSnapshotStale)
        );
        assert_eq!(
            backend.record_state(&registration.protected_record_ref),
            Ok(VaultRecordState::Present)
        );
    }

    #[test]
    fn missing_record_blocks_forget_and_preserves_metadata() {
        let root = TestRoot::new();
        let mut backend = root.file_vault();
        let mut metadata = CapacityStore::open_in_memory().expect("open metadata");
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let account = register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(TOKEN_CANARY),
            &timestamp("2026-08-30T07:00:00Z"),
        )
        .unwrap()
        .account;
        fs::remove_dir_all(backend.records_root.join(RECORD_UUID)).expect("remove test record");

        assert_eq!(
            forget_account(
                &mut metadata,
                &mut backend,
                &account,
                &timestamp("2026-08-30T07:01:00Z"),
            ),
            Err(AccountVaultError::ProtectedRecordState(
                VaultRecordState::Missing
            ))
        );
        assert_eq!(
            metadata.vault_account(&account.account_id).unwrap(),
            Some(account)
        );
    }

    #[test]
    fn corrupt_quarantined_record_blocks_metadata_removal() {
        let root = TestRoot::new();
        let mut backend = root.file_vault();
        let mut metadata = CapacityStore::open_in_memory().expect("open metadata");
        let registration = registration(FINGERPRINT, RECORD_UUID, "Primary ChatGPT");
        let account = register_account(
            &mut metadata,
            &mut backend,
            &registration,
            &protected_record(TOKEN_CANARY),
            &timestamp("2026-08-30T07:00:00Z"),
        )
        .unwrap()
        .account;
        backend
            .quarantine_record(&registration.protected_record_ref)
            .expect("quarantine record");
        let auth_path = backend.quarantine_root.join(RECORD_UUID).join("auth.json");
        fs::set_permissions(&auth_path, fs::Permissions::from_mode(0o644))
            .expect("inject permission drift");

        assert_eq!(
            forget_account(
                &mut metadata,
                &mut backend,
                &account,
                &timestamp("2026-08-30T07:01:00Z"),
            ),
            Err(AccountVaultError::ProtectedBackend(
                VaultBackendError::InsecurePermissions
            ))
        );
        assert_eq!(
            metadata.vault_account(&account.account_id).unwrap(),
            Some(account)
        );
    }

    struct ClaimOnCreateBackend {
        inner: FileVault,
        competing_store: CapacityStore,
        competing_registration: VaultAccountRegistration,
        fired: bool,
    }

    impl ProtectedRecordBackend for ClaimOnCreateBackend {
        fn record_state(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<VaultRecordState, VaultBackendError> {
            self.inner.record_state(record_ref)
        }

        fn create_record(
            &mut self,
            record_ref: &VaultRecordRef,
            record: &ProtectedVaultRecord,
        ) -> Result<(), VaultBackendError> {
            self.inner.create_record(record_ref, record)?;
            if !self.fired {
                self.fired = true;
                assert!(matches!(
                    self.competing_store.register_vault_account(
                        &self.competing_registration,
                        &timestamp("2026-08-30T07:00:30Z"),
                    ),
                    Ok(VaultAccountRegistrationOutcome::Created(_))
                ));
            }
            Ok(())
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
    fn compensation_never_quarantines_a_record_claimed_during_registration() {
        let root = TestRoot::new();
        let database_path = root.path.join("metadata.sqlite");
        let mut metadata = CapacityStore::open(&database_path).expect("open primary metadata");
        let competing_store = CapacityStore::open(&database_path).expect("open competing metadata");
        let requested = registration(FINGERPRINT, RECORD_UUID, "Requested account");
        let competing_registration = registration(FINGERPRINT_2, RECORD_UUID, "Competing account");
        let mut backend = ClaimOnCreateBackend {
            inner: root.file_vault(),
            competing_store,
            competing_registration: competing_registration.clone(),
            fired: false,
        };

        assert_eq!(
            register_account(
                &mut metadata,
                &mut backend,
                &requested,
                &protected_record(TOKEN_CANARY),
                &timestamp("2026-08-30T07:01:00Z"),
            ),
            Err(AccountVaultError::MetadataRegistrationFailed {
                reason: MetadataFailureReason::IdentityRecordConflict,
                compensation: CompensationOutcome::NotRequired,
            })
        );
        assert_eq!(
            backend.inner.record_state(&requested.protected_record_ref),
            Ok(VaultRecordState::Present)
        );
        let owner = metadata
            .vault_account_by_record_ref(&requested.protected_record_ref)
            .unwrap()
            .expect("competing metadata owns the record");
        assert_eq!(
            owner.account_fingerprint,
            competing_registration.account_fingerprint
        );
    }

    struct RenameOnQuarantineBackend {
        inner: FileVault,
        competing_store: CapacityStore,
        expected_account: VaultAccount,
        fired: bool,
    }

    impl ProtectedRecordBackend for RenameOnQuarantineBackend {
        fn record_state(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<VaultRecordState, VaultBackendError> {
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
            let outcome = self.inner.quarantine_record(record_ref)?;
            if !self.fired {
                self.fired = true;
                let renamed = self
                    .competing_store
                    .rename_vault_account(
                        &self.expected_account.account_id,
                        self.expected_account.revision,
                        "Concurrent rename",
                        &timestamp("2026-08-30T07:02:30Z"),
                    )
                    .expect("competing metadata write");
                assert!(matches!(renamed, VaultAccountMutationOutcome::Updated(_)));
            }
            Ok(outcome)
        }

        fn restore_record(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<VaultTransitionOutcome, VaultBackendError> {
            self.inner.restore_record(record_ref)
        }
    }

    #[test]
    fn metadata_race_after_quarantine_restores_the_record() {
        let root = TestRoot::new();
        let database_path = root.path.join("metadata.sqlite");
        let mut metadata = CapacityStore::open(&database_path).expect("open primary metadata");
        let competing_store = CapacityStore::open(&database_path).expect("open competing metadata");
        let mut file_vault = root.file_vault();
        let registration = registration(FINGERPRINT, RECORD_UUID_2, "Primary ChatGPT");
        let account = register_account(
            &mut metadata,
            &mut file_vault,
            &registration,
            &protected_record(TOKEN_CANARY),
            &timestamp("2026-08-30T07:00:00Z"),
        )
        .unwrap()
        .account;
        let mut backend = RenameOnQuarantineBackend {
            inner: file_vault,
            competing_store,
            expected_account: account.clone(),
            fired: false,
        };

        assert_eq!(
            forget_account(
                &mut metadata,
                &mut backend,
                &account,
                &timestamp("2026-08-30T07:03:00Z"),
            ),
            Err(AccountVaultError::MetadataRemovalFailed {
                reason: MetadataFailureReason::RevisionConflict,
                compensation: CompensationOutcome::Restored,
            })
        );
        assert_eq!(
            backend
                .inner
                .record_state(&registration.protected_record_ref),
            Ok(VaultRecordState::Present)
        );
        let current = metadata
            .vault_account(&account.account_id)
            .unwrap()
            .expect("metadata remains");
        assert_eq!(current.display_name, "Concurrent rename");
        assert_eq!(current.revision, account.revision + 1);
    }

    #[test]
    fn debug_and_error_surfaces_do_not_expose_orchestrator_secrets_or_paths() {
        let root = TestRoot::new();
        let error = AccountVaultError::MetadataRegistrationFailed {
            reason: MetadataFailureReason::Unavailable,
            compensation: CompensationOutcome::Failed,
        };
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains(TOKEN_CANARY));
        assert!(!rendered.contains(IDENTITY_CANARY));
        assert!(!rendered.contains(Path::new(&root.path).to_string_lossy().as_ref()));
    }
}
