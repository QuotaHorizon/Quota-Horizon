use std::fmt;

use capacity_domain::{
    AccountFingerprint, UtcTimestamp, VaultAccountAuthMode, VaultAccountLifecycle,
    VaultAccountRegistration, VaultAccountSource, VaultRecordRef,
};
use capacity_store::CapacityStore;

use crate::{
    AccountVaultError, FileVault, IdentityError, InstallationKeyRing,
    JournaledAccountRegistrationReceipt, ProtectedRecordBackend, ProtectedRecordInventoryBackend,
    ProtectedVaultRecord, RecordAuthenticationTag, VaultBackendError, VaultBackendInventory,
    VaultRecordRetagOutcome, VaultRecordState, VaultRecordTagDependencyAudit,
    VaultTransitionOutcome, journaled_register_account, read_record_at, record_directory_name,
};

/// Non-secret onboarding metadata. The stable fingerprint is deliberately
/// absent and can only be derived from the authenticated vault's active key and
/// the protected record's exact identity tuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRegistrationDraft {
    pub protected_record_ref: VaultRecordRef,
    pub display_name: String,
    pub auth_mode: VaultAccountAuthMode,
    pub source: VaultAccountSource,
    pub lifecycle: VaultAccountLifecycle,
    pub provider_id: Option<String>,
    pub model: Option<String>,
}

/// Storage seam needed by the authentication wrapper to verify both live and
/// quarantined bytes before a state transition. The raw record, including its
/// tag, never leaves the wrapper through `ProtectedRecordBackend`.
pub trait RawAuthenticatedRecordBackend: ProtectedRecordBackend {
    fn read_raw_record(
        &mut self,
        record_ref: &VaultRecordRef,
        state: VaultRecordState,
    ) -> Result<ProtectedVaultRecord, VaultBackendError>;
}

/// Mutation seam kept below [`AuthenticatedVault`] so callers cannot submit an
/// unauthenticated replacement payload. Implementations may replace only the
/// tag after checking the complete expected raw record.
pub trait RawAuthenticatedRecordMutationBackend: RawAuthenticatedRecordBackend {
    fn replace_raw_authentication_tag(
        &mut self,
        record_ref: &VaultRecordRef,
        state: VaultRecordState,
        expected: &ProtectedVaultRecord,
        replacement: &RecordAuthenticationTag,
    ) -> Result<VaultRecordRetagOutcome, VaultBackendError>;
}

impl RawAuthenticatedRecordBackend for FileVault {
    fn read_raw_record(
        &mut self,
        record_ref: &VaultRecordRef,
        state: VaultRecordState,
    ) -> Result<ProtectedVaultRecord, VaultBackendError> {
        self.verify_roots()?;
        let name = record_directory_name(record_ref)?;
        match (self.record_state(record_ref)?, state) {
            (VaultRecordState::Present, VaultRecordState::Present) => {
                read_record_at(&self.records_root.join(name))
            }
            (VaultRecordState::Quarantined, VaultRecordState::Quarantined) => {
                read_record_at(&self.quarantine_root.join(name))
            }
            (VaultRecordState::Missing, _) => Err(VaultBackendError::RecordMissing),
            (VaultRecordState::Conflict, _) => Err(VaultBackendError::RecordStateConflict),
            _ => Err(VaultBackendError::RecordChangedDuringRead),
        }
    }
}

impl RawAuthenticatedRecordMutationBackend for FileVault {
    fn replace_raw_authentication_tag(
        &mut self,
        record_ref: &VaultRecordRef,
        state: VaultRecordState,
        expected: &ProtectedVaultRecord,
        replacement: &RecordAuthenticationTag,
    ) -> Result<VaultRecordRetagOutcome, VaultBackendError> {
        self.replace_record_authentication_tag(record_ref, state, expected, replacement)
    }
}

/// Enforces installation-key authentication around a raw protected-record
/// backend. Callers submit and receive logical records without tags; this layer
/// creates, verifies, and strips the persisted authentication metadata.
pub struct AuthenticatedVault<B> {
    backend: B,
    keys: InstallationKeyRing,
}

impl<B> AuthenticatedVault<B> {
    pub fn new(backend: B, keys: InstallationKeyRing) -> Self {
        Self { backend, keys }
    }

    pub fn active_key_id(&self) -> &str {
        self.keys.active_key_id()
    }

    pub fn account_fingerprint(
        &self,
        record: &ProtectedVaultRecord,
    ) -> Result<AccountFingerprint, IdentityError> {
        self.keys.account_fingerprint(record)
    }

    pub fn account_fingerprint_with_key_id(
        &self,
        record: &ProtectedVaultRecord,
        key_id: &str,
    ) -> Result<AccountFingerprint, IdentityError> {
        self.keys.account_fingerprint_with_key_id(record, key_id)
    }

    pub fn bind_registration(
        &self,
        draft: &AccountRegistrationDraft,
        record: &ProtectedVaultRecord,
    ) -> Result<VaultAccountRegistration, AccountVaultError> {
        let account_fingerprint = self
            .account_fingerprint(record)
            .map_err(|_| AccountVaultError::InvalidRegistration)?;
        let registration = VaultAccountRegistration {
            account_fingerprint,
            protected_record_ref: draft.protected_record_ref.clone(),
            display_name: draft.display_name.clone(),
            auth_mode: draft.auth_mode,
            source: draft.source,
            lifecycle: draft.lifecycle,
            provider_id: draft.provider_id.clone(),
            model: draft.model.clone(),
        };
        registration
            .validate()
            .map_err(|_| AccountVaultError::InvalidRegistration)?;
        Ok(registration)
    }

    pub fn into_parts(self) -> (B, InstallationKeyRing) {
        (self.backend, self.keys)
    }
}

/// Durable registration entry point for authenticated account onboarding. It
/// derives the fingerprint and writes the authenticated record through one
/// vault capability, removing the caller-controlled fingerprint seam.
pub fn journaled_register_bound_account<B: RawAuthenticatedRecordBackend>(
    metadata: &mut CapacityStore,
    vault: &mut AuthenticatedVault<B>,
    draft: &AccountRegistrationDraft,
    protected_record: &ProtectedVaultRecord,
    changed_at: &UtcTimestamp,
) -> Result<JournaledAccountRegistrationReceipt, AccountVaultError> {
    let registration = vault.bind_registration(draft, protected_record)?;
    journaled_register_account(metadata, vault, &registration, protected_record, changed_at)
}

impl<B> fmt::Debug for AuthenticatedVault<B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedVault")
            .field("backend", &"<redacted>")
            .field("keys", &"<redacted>")
            .finish()
    }
}

impl<B: RawAuthenticatedRecordBackend> AuthenticatedVault<B> {
    fn verify_raw_record(
        &mut self,
        record_ref: &VaultRecordRef,
        state: VaultRecordState,
    ) -> Result<ProtectedVaultRecord, VaultBackendError> {
        let record = self.backend.read_raw_record(record_ref, state)?;
        let tag = record
            .authentication_tag()
            .ok_or(VaultBackendError::RecordAuthenticationMissing)?;
        self.keys
            .verify_record(record_ref, &record, tag)
            .map_err(authentication_error_to_backend)?;
        Ok(record.into_unauthenticated())
    }

    /// Reads and authenticates either the live or quarantined copy. Rotation
    /// must preserve both states and cannot assume every managed account is
    /// currently live.
    pub fn read_record_in_current_state(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<ProtectedVaultRecord, VaultBackendError> {
        match self.backend.record_state(record_ref)? {
            state @ (VaultRecordState::Present | VaultRecordState::Quarantined) => {
                self.verify_raw_record(record_ref, state)
            }
            VaultRecordState::Missing => Err(VaultBackendError::RecordMissing),
            VaultRecordState::Conflict => Err(VaultBackendError::RecordStateConflict),
        }
    }
}

impl<T: ProtectedRecordBackend + ?Sized> ProtectedRecordBackend for &mut T {
    fn record_state(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultRecordState, VaultBackendError> {
        (**self).record_state(record_ref)
    }

    fn create_record(
        &mut self,
        record_ref: &VaultRecordRef,
        record: &ProtectedVaultRecord,
    ) -> Result<(), VaultBackendError> {
        (**self).create_record(record_ref, record)
    }

    fn read_record(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<ProtectedVaultRecord, VaultBackendError> {
        (**self).read_record(record_ref)
    }

    fn quarantine_record(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultTransitionOutcome, VaultBackendError> {
        (**self).quarantine_record(record_ref)
    }

    fn restore_record(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultTransitionOutcome, VaultBackendError> {
        (**self).restore_record(record_ref)
    }
}

impl<T: RawAuthenticatedRecordBackend + ?Sized> RawAuthenticatedRecordBackend for &mut T {
    fn read_raw_record(
        &mut self,
        record_ref: &VaultRecordRef,
        state: VaultRecordState,
    ) -> Result<ProtectedVaultRecord, VaultBackendError> {
        (**self).read_raw_record(record_ref, state)
    }
}

impl<T: RawAuthenticatedRecordMutationBackend + ?Sized> RawAuthenticatedRecordMutationBackend
    for &mut T
{
    fn replace_raw_authentication_tag(
        &mut self,
        record_ref: &VaultRecordRef,
        state: VaultRecordState,
        expected: &ProtectedVaultRecord,
        replacement: &RecordAuthenticationTag,
    ) -> Result<VaultRecordRetagOutcome, VaultBackendError> {
        (**self).replace_raw_authentication_tag(record_ref, state, expected, replacement)
    }
}

impl<T: ProtectedRecordInventoryBackend + ?Sized> ProtectedRecordInventoryBackend for &mut T {
    fn inventory(&mut self, limit: u32) -> Result<VaultBackendInventory, VaultBackendError> {
        (**self).inventory(limit)
    }
}

impl<B: RawAuthenticatedRecordMutationBackend> AuthenticatedVault<B> {
    /// Re-authenticates one live or quarantined record with the ring's active
    /// key while the predecessor remains available for verification and
    /// rollback.
    pub fn retag_record_with_active_key(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultRecordRetagOutcome, VaultBackendError> {
        let state = match self.backend.record_state(record_ref)? {
            state @ (VaultRecordState::Present | VaultRecordState::Quarantined) => state,
            VaultRecordState::Missing => return Err(VaultBackendError::RecordMissing),
            VaultRecordState::Conflict => return Err(VaultBackendError::RecordStateConflict),
        };
        let raw = self.backend.read_raw_record(record_ref, state)?;
        let current_tag = raw
            .authentication_tag()
            .ok_or(VaultBackendError::RecordAuthenticationMissing)?;
        self.keys
            .verify_record(record_ref, &raw, current_tag)
            .map_err(authentication_error_to_backend)?;
        let replacement = self.keys.authenticate_record(record_ref, &raw);
        if current_tag == &replacement {
            return Ok(VaultRecordRetagOutcome::AlreadyApplied);
        }
        let outcome =
            self.backend
                .replace_raw_authentication_tag(record_ref, state, &raw, &replacement)?;
        if matches!(
            outcome,
            VaultRecordRetagOutcome::Applied | VaultRecordRetagOutcome::AlreadyApplied
        ) {
            let installed = self.backend.read_raw_record(record_ref, state)?;
            let installed_tag = installed
                .authentication_tag()
                .ok_or(VaultBackendError::RecordAuthenticationMissing)?;
            if installed_tag != &replacement {
                return Ok(VaultRecordRetagOutcome::RevisionConflict);
            }
            self.keys
                .verify_record(record_ref, &installed, installed_tag)
                .map_err(authentication_error_to_backend)?;
        }
        Ok(outcome)
    }
}

impl<B> AuthenticatedVault<B>
where
    B: RawAuthenticatedRecordBackend + ProtectedRecordInventoryBackend,
{
    /// Inventories every authenticated live/quarantined record twice and
    /// verifies each tag before counting references to `key_id`.
    pub fn record_tag_dependency_audit(
        &mut self,
        key_id: &str,
        limit: u32,
    ) -> Result<VaultRecordTagDependencyAudit, VaultBackendError> {
        if !self.keys.contains_key_id(key_id) {
            return Err(VaultBackendError::InstallationKeyUnavailable);
        }
        self.record_tag_reference_audit(key_id, limit)
    }

    /// Counts references to an exact key ID even when that key was never
    /// persisted. Tags using available keys are authenticated; a tag naming
    /// the absent key is conservatively counted as a blocker instead of being
    /// trusted or ignored. This closes the Prepared-to-rollback crash window.
    pub fn record_tag_reference_audit(
        &mut self,
        key_id: &str,
        limit: u32,
    ) -> Result<VaultRecordTagDependencyAudit, VaultBackendError> {
        let before = self.backend.inventory(limit)?;
        let mut observed_tags = Vec::with_capacity(before.records.len());
        let mut live_dependencies = 0_u32;
        let mut quarantined_dependencies = 0_u32;
        for item in &before.records {
            let state = match item.state {
                state @ (VaultRecordState::Present | VaultRecordState::Quarantined) => state,
                VaultRecordState::Missing => return Err(VaultBackendError::InventoryChanged),
                VaultRecordState::Conflict => return Err(VaultBackendError::RecordStateConflict),
            };
            let raw = self.backend.read_raw_record(&item.record_ref, state)?;
            let tag = raw
                .authentication_tag()
                .ok_or(VaultBackendError::RecordAuthenticationMissing)?;
            if tag.key_id() != key_id || self.keys.contains_key_id(key_id) {
                self.keys
                    .verify_record(&item.record_ref, &raw, tag)
                    .map_err(authentication_error_to_backend)?;
            }
            observed_tags.push(tag.clone());
            if tag.key_id() == key_id {
                let counter = match state {
                    VaultRecordState::Present => &mut live_dependencies,
                    VaultRecordState::Quarantined => &mut quarantined_dependencies,
                    VaultRecordState::Missing | VaultRecordState::Conflict => unreachable!(),
                };
                *counter = counter
                    .checked_add(1)
                    .ok_or(VaultBackendError::BoundExceeded("tag_dependencies"))?;
            }
        }
        let after = self.backend.inventory(limit)?;
        if before != after {
            return Err(VaultBackendError::InventoryChanged);
        }
        for (item, observed_tag) in before.records.iter().zip(&observed_tags) {
            let raw = self.backend.read_raw_record(&item.record_ref, item.state)?;
            let tag = raw
                .authentication_tag()
                .ok_or(VaultBackendError::RecordAuthenticationMissing)?;
            if tag.key_id() != key_id || self.keys.contains_key_id(key_id) {
                self.keys
                    .verify_record(&item.record_ref, &raw, tag)
                    .map_err(authentication_error_to_backend)?;
            }
            if tag != observed_tag {
                return Err(VaultBackendError::InventoryChanged);
            }
        }
        Ok(VaultRecordTagDependencyAudit {
            inspected_records: u32::try_from(before.records.len())
                .map_err(|_| VaultBackendError::BoundExceeded("inventory_items"))?,
            live_dependencies,
            quarantined_dependencies,
            staging_residues: u32::try_from(before.staging_residues.len())
                .map_err(|_| VaultBackendError::BoundExceeded("inventory_items"))?,
        })
    }
}

impl<B: RawAuthenticatedRecordBackend> ProtectedRecordBackend for AuthenticatedVault<B> {
    fn record_state(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultRecordState, VaultBackendError> {
        self.backend.record_state(record_ref)
    }

    fn create_record(
        &mut self,
        record_ref: &VaultRecordRef,
        record: &ProtectedVaultRecord,
    ) -> Result<(), VaultBackendError> {
        let tag = self.keys.authenticate_record(record_ref, record);
        let authenticated = record.authenticated_copy(tag)?;
        self.backend.create_record(record_ref, &authenticated)
    }

    fn read_record(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<ProtectedVaultRecord, VaultBackendError> {
        self.verify_raw_record(record_ref, VaultRecordState::Present)
    }

    fn quarantine_record(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultTransitionOutcome, VaultBackendError> {
        match self.backend.record_state(record_ref)? {
            state @ (VaultRecordState::Present | VaultRecordState::Quarantined) => {
                drop(self.verify_raw_record(record_ref, state)?);
            }
            VaultRecordState::Missing => return Err(VaultBackendError::RecordMissing),
            VaultRecordState::Conflict => return Err(VaultBackendError::RecordStateConflict),
        }
        self.backend.quarantine_record(record_ref)
    }

    fn restore_record(
        &mut self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultTransitionOutcome, VaultBackendError> {
        match self.backend.record_state(record_ref)? {
            state @ (VaultRecordState::Present | VaultRecordState::Quarantined) => {
                drop(self.verify_raw_record(record_ref, state)?);
            }
            VaultRecordState::Missing => return Err(VaultBackendError::RecordMissing),
            VaultRecordState::Conflict => return Err(VaultBackendError::RecordStateConflict),
        }
        self.backend.restore_record(record_ref)
    }
}

impl<B> ProtectedRecordInventoryBackend for AuthenticatedVault<B>
where
    B: RawAuthenticatedRecordBackend + ProtectedRecordInventoryBackend,
{
    fn inventory(&mut self, limit: u32) -> Result<VaultBackendInventory, VaultBackendError> {
        self.backend.inventory(limit)
    }
}

fn authentication_error_to_backend(error: IdentityError) -> VaultBackendError {
    match error {
        IdentityError::UnknownKeyId => VaultBackendError::InstallationKeyUnavailable,
        IdentityError::InvalidRecordAuthenticationTag
        | IdentityError::RecordAuthenticationFailed => {
            VaultBackendError::RecordAuthenticationFailed
        }
        _ => VaultBackendError::RecordAuthenticationFailed,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::{INSTALLATION_SECRET_BYTES, InstallationKey};

    use super::*;

    const KEY_ID: &str = "018f47a2-8a71-4f4a-9c35-1f4234a73601";
    const KEY_ID_2: &str = "018f47a2-8a71-4f4a-9c35-1f4234a73602";
    const KEY_ID_3: &str = "018f47a2-8a71-4f4a-9c35-1f4234a73603";
    const RECORD_UUID: &str = "018f47a2-8a71-4f4a-9c35-1f4234a73611";
    const RECORD_UUID_2: &str = "018f47a2-8a71-4f4a-9c35-1f4234a73612";
    const IDENTITY_CANARY: &str = "authenticated-vault@example.invalid";
    const TOKEN_CANARY: &str = "authenticated-vault-token";
    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            Self(std::env::temp_dir().join(format!(
                "capacity-authenticated-vault-test-{}-{nanos}-{sequence}",
                std::process::id()
            )))
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            if self.0.parent() == Some(std::env::temp_dir().as_path())
                && self
                    .0
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("capacity-authenticated-vault-test-"))
            {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }

    fn key(id: &str, byte: u8) -> InstallationKey {
        InstallationKey::from_parts(id, [byte; INSTALLATION_SECRET_BYTES]).unwrap()
    }

    fn reference(uuid: &str) -> VaultRecordRef {
        VaultRecordRef::parse(format!("vault-record:v1:{uuid}")).unwrap()
    }

    fn record() -> ProtectedVaultRecord {
        ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_account_id",
            IDENTITY_CANARY.as_bytes().to_vec(),
            format!(r#"{{"access_token":"{TOKEN_CANARY}"}}"#).into_bytes(),
            None,
        )
        .unwrap()
    }

    struct RetagOnSecondRawReadBackend {
        raw: FileVault,
        replacement: RecordAuthenticationTag,
        raw_reads: u8,
    }

    impl ProtectedRecordBackend for RetagOnSecondRawReadBackend {
        fn record_state(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<VaultRecordState, VaultBackendError> {
            self.raw.record_state(record_ref)
        }

        fn create_record(
            &mut self,
            record_ref: &VaultRecordRef,
            record: &ProtectedVaultRecord,
        ) -> Result<(), VaultBackendError> {
            self.raw.create_record(record_ref, record)
        }

        fn read_record(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<ProtectedVaultRecord, VaultBackendError> {
            self.raw.read_record(record_ref)
        }

        fn quarantine_record(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<VaultTransitionOutcome, VaultBackendError> {
            self.raw.quarantine_record(record_ref)
        }

        fn restore_record(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<VaultTransitionOutcome, VaultBackendError> {
            self.raw.restore_record(record_ref)
        }
    }

    impl RawAuthenticatedRecordBackend for RetagOnSecondRawReadBackend {
        fn read_raw_record(
            &mut self,
            record_ref: &VaultRecordRef,
            state: VaultRecordState,
        ) -> Result<ProtectedVaultRecord, VaultBackendError> {
            self.raw_reads = self
                .raw_reads
                .checked_add(1)
                .ok_or(VaultBackendError::BoundExceeded("test_raw_reads"))?;
            if self.raw_reads == 2 {
                let current = <FileVault as RawAuthenticatedRecordBackend>::read_raw_record(
                    &mut self.raw,
                    record_ref,
                    state,
                )?;
                let replacement = self.replacement.clone();
                match self.raw.replace_record_authentication_tag(
                    record_ref,
                    state,
                    &current,
                    &replacement,
                )? {
                    VaultRecordRetagOutcome::Applied | VaultRecordRetagOutcome::AlreadyApplied => {}
                    VaultRecordRetagOutcome::RevisionConflict => {
                        return Err(VaultBackendError::RecordChangedDuringRead);
                    }
                }
            }
            <FileVault as RawAuthenticatedRecordBackend>::read_raw_record(
                &mut self.raw,
                record_ref,
                state,
            )
        }
    }

    impl ProtectedRecordInventoryBackend for RetagOnSecondRawReadBackend {
        fn inventory(&mut self, limit: u32) -> Result<VaultBackendInventory, VaultBackendError> {
            self.raw.inventory(limit)
        }
    }

    fn draft(uuid: &str) -> AccountRegistrationDraft {
        AccountRegistrationDraft {
            protected_record_ref: reference(uuid),
            display_name: "Primary ChatGPT".to_owned(),
            auth_mode: VaultAccountAuthMode::ChatGpt,
            source: VaultAccountSource::CurrentRuntime,
            lifecycle: VaultAccountLifecycle::Ready,
            provider_id: None,
            model: None,
        }
    }

    #[test]
    fn bound_journal_registration_derives_the_only_persisted_fingerprint() {
        let root = TestRoot::new();
        let raw = FileVault::open(&root.0).unwrap();
        let mut vault = AuthenticatedVault::new(raw, InstallationKeyRing::new(key(KEY_ID, 0x19)));
        let protected_record = record();
        let expected = vault.account_fingerprint(&protected_record).unwrap();
        let mut metadata = CapacityStore::open_in_memory().unwrap();
        let changed_at = UtcTimestamp::parse("2026-08-30T10:10:00Z").unwrap();

        let receipt = journaled_register_bound_account(
            &mut metadata,
            &mut vault,
            &draft(RECORD_UUID),
            &protected_record,
            &changed_at,
        )
        .expect("authenticated durable registration");
        assert_eq!(receipt.receipt.account.account_fingerprint, expected);
        assert_eq!(
            metadata
                .vault_account_by_fingerprint(&expected)
                .unwrap()
                .unwrap()
                .protected_record_ref,
            reference(RECORD_UUID)
        );
        assert!(vault.read_record(&reference(RECORD_UUID)).is_ok());
    }

    #[test]
    fn authenticated_wrapper_persists_v2_and_returns_only_logical_payload() {
        let root = TestRoot::new();
        let raw = FileVault::open(&root.0).unwrap();
        let mut vault = AuthenticatedVault::new(raw, InstallationKeyRing::new(key(KEY_ID, 0x11)));
        let reference = reference(RECORD_UUID);

        vault.create_record(&reference, &record()).unwrap();
        let loaded = vault.read_record(&reference).unwrap();
        assert!(!loaded.is_authenticated());
        assert_eq!(loaded.upstream_identity(), IDENTITY_CANARY.as_bytes());
        assert_eq!(loaded.auth_json(), record().auth_json());

        let (raw, _) = vault.into_parts();
        let raw_record = raw.read_record(&reference).unwrap();
        assert!(raw_record.is_authenticated());
        let record_path = raw.records_root.join(RECORD_UUID);
        assert_eq!(
            fs::read(record_path.join(crate::RECORD_FORMAT_FILE)).unwrap(),
            crate::RECORD_FORMAT_V2
        );
        assert_eq!(
            fs::symlink_metadata(record_path.join(crate::RECORD_AUTHENTICATION_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn tamper_and_record_reference_swap_fail_authentication() {
        let root = TestRoot::new();
        let raw = FileVault::open(&root.0).unwrap();
        let mut vault = AuthenticatedVault::new(raw, InstallationKeyRing::new(key(KEY_ID, 0x22)));
        let first = reference(RECORD_UUID);
        vault.create_record(&first, &record()).unwrap();
        let (raw, keys) = vault.into_parts();
        let first_path = raw.records_root.join(RECORD_UUID);
        fs::write(first_path.join(crate::AUTH_FILE), b"tampered-auth").unwrap();
        fs::set_permissions(
            first_path.join(crate::AUTH_FILE),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let mut vault = AuthenticatedVault::new(raw, keys);
        assert!(matches!(
            vault.read_record(&first),
            Err(VaultBackendError::RecordAuthenticationFailed)
        ));

        let (raw, keys) = vault.into_parts();
        fs::rename(
            raw.records_root.join(RECORD_UUID),
            raw.records_root.join(RECORD_UUID_2),
        )
        .unwrap();
        let mut vault = AuthenticatedVault::new(raw, keys);
        assert!(matches!(
            vault.read_record(&reference(RECORD_UUID_2)),
            Err(VaultBackendError::RecordAuthenticationFailed)
        ));
    }

    #[test]
    fn legacy_v1_remains_readable_raw_but_never_silently_authenticates() {
        let root = TestRoot::new();
        let raw = FileVault::open(&root.0).unwrap();
        let reference = reference(RECORD_UUID);
        raw.create_record(&reference, &record()).unwrap();
        assert!(!raw.read_record(&reference).unwrap().is_authenticated());

        let mut authenticated =
            AuthenticatedVault::new(raw, InstallationKeyRing::new(key(KEY_ID, 0x33)));
        assert!(matches!(
            authenticated.read_record(&reference),
            Err(VaultBackendError::RecordAuthenticationMissing)
        ));
        assert_eq!(
            authenticated.quarantine_record(&reference),
            Err(VaultBackendError::RecordAuthenticationMissing)
        );
        assert_eq!(
            authenticated.record_state(&reference).unwrap(),
            VaultRecordState::Present
        );
    }

    #[test]
    fn rotation_ring_reads_old_records_and_new_records_use_active_key() {
        let root = TestRoot::new();
        let raw = FileVault::open(&root.0).unwrap();
        let first = reference(RECORD_UUID);
        let second = reference(RECORD_UUID_2);
        let mut old = AuthenticatedVault::new(raw, InstallationKeyRing::new(key(KEY_ID, 0x44)));
        old.create_record(&first, &record()).unwrap();
        let (raw, _) = old.into_parts();

        let ring = InstallationKeyRing::with_verification_keys(
            key(KEY_ID_2, 0x55),
            vec![key(KEY_ID, 0x44)],
        )
        .unwrap();
        let mut rotating = AuthenticatedVault::new(raw, ring);
        rotating.read_record(&first).expect("old record readable");
        rotating.create_record(&second, &record()).unwrap();
        let (raw, _) = rotating.into_parts();
        let old_tag = raw
            .read_record(&first)
            .unwrap()
            .authentication_tag()
            .unwrap()
            .key_id()
            .to_owned();
        let new_tag = raw
            .read_record(&second)
            .unwrap()
            .authentication_tag()
            .unwrap()
            .key_id()
            .to_owned();
        assert_eq!(old_tag, KEY_ID);
        assert_eq!(new_tag, KEY_ID_2);
    }

    #[test]
    fn rotation_retags_live_record_and_clears_old_dependency_idempotently() {
        let root = TestRoot::new();
        let raw = FileVault::open(&root.0).unwrap();
        let reference = reference(RECORD_UUID);
        let mut old = AuthenticatedVault::new(raw, InstallationKeyRing::new(key(KEY_ID, 0x49)));
        old.create_record(&reference, &record()).unwrap();
        let (raw, _) = old.into_parts();

        let ring = InstallationKeyRing::with_verification_keys(
            key(KEY_ID_2, 0x59),
            vec![key(KEY_ID, 0x49)],
        )
        .unwrap();
        let mut rotating = AuthenticatedVault::new(raw, ring);
        let before = rotating
            .record_tag_dependency_audit(KEY_ID, 8)
            .expect("old-key dependency audit");
        assert_eq!(before.inspected_records, 1);
        assert_eq!(before.live_dependencies, 1);
        assert_eq!(before.quarantined_dependencies, 0);
        assert_eq!(before.staging_residues, 0);
        assert!(before.has_retirement_blockers());

        assert_eq!(
            rotating
                .retag_record_with_active_key(&reference)
                .expect("retag live record"),
            VaultRecordRetagOutcome::Applied
        );
        assert_eq!(
            rotating
                .retag_record_with_active_key(&reference)
                .expect("exact retag retry"),
            VaultRecordRetagOutcome::AlreadyApplied
        );
        let loaded = rotating.read_record(&reference).expect("new tag verifies");
        assert_eq!(loaded.upstream_identity(), IDENTITY_CANARY.as_bytes());
        assert_eq!(loaded.auth_json(), record().auth_json());

        let after = rotating
            .record_tag_dependency_audit(KEY_ID, 8)
            .expect("cleared old-key audit");
        assert_eq!(after.inspected_records, 1);
        assert_eq!(after.live_dependencies, 0);
        assert_eq!(after.quarantined_dependencies, 0);
        assert_eq!(after.staging_residues, 0);
        assert!(!after.has_retirement_blockers());

        let (raw, _) = rotating.into_parts();
        let installed = raw.read_record(&reference).unwrap();
        assert_eq!(installed.authentication_tag().unwrap().key_id(), KEY_ID_2);
        assert_eq!(raw.staging_residue_count().unwrap(), 0);
    }

    #[test]
    fn absent_journaled_key_tag_is_counted_as_a_conservative_blocker() {
        let root = TestRoot::new();
        let raw = FileVault::open(&root.0).unwrap();
        let reference = reference(RECORD_UUID);
        let mut target_writer =
            AuthenticatedVault::new(raw, InstallationKeyRing::new(key(KEY_ID_2, 0x5a)));
        target_writer
            .create_record(&reference, &record())
            .expect("target-tagged record");
        let (raw, _) = target_writer.into_parts();

        let mut original =
            AuthenticatedVault::new(raw, InstallationKeyRing::new(key(KEY_ID, 0x49)));
        assert!(matches!(
            original.record_tag_dependency_audit(KEY_ID_2, 8),
            Err(VaultBackendError::InstallationKeyUnavailable)
        ));
        let audit = original
            .record_tag_reference_audit(KEY_ID_2, 8)
            .expect("absent-key reference audit");
        assert_eq!(audit.inspected_records, 1);
        assert_eq!(audit.live_dependencies, 1);
        assert_eq!(audit.quarantined_dependencies, 0);
        assert!(audit.has_retirement_blockers());
    }

    #[test]
    fn dependency_audit_counts_quarantine_and_staging_as_retirement_blockers() {
        let root = TestRoot::new();
        let raw = FileVault::open(&root.0).unwrap();
        let live = reference(RECORD_UUID);
        let quarantined = reference(RECORD_UUID_2);
        let mut old = AuthenticatedVault::new(raw, InstallationKeyRing::new(key(KEY_ID, 0x4a)));
        old.create_record(&live, &record()).unwrap();
        old.create_record(&quarantined, &record()).unwrap();
        assert_eq!(
            old.quarantine_record(&quarantined).unwrap(),
            VaultTransitionOutcome::Moved
        );
        let (raw, _) = old.into_parts();
        let residue = raw.allocate_staging_directory(RECORD_UUID).unwrap();
        assert!(residue.exists());

        let ring = InstallationKeyRing::with_verification_keys(
            key(KEY_ID_2, 0x5a),
            vec![key(KEY_ID, 0x4a)],
        )
        .unwrap();
        let mut rotating = AuthenticatedVault::new(raw, ring);
        let audit = rotating
            .record_tag_dependency_audit(KEY_ID, 8)
            .expect("bounded dependency audit");
        assert_eq!(audit.inspected_records, 2);
        assert_eq!(audit.live_dependencies, 1);
        assert_eq!(audit.quarantined_dependencies, 1);
        assert_eq!(audit.staging_residues, 1);
        assert!(audit.has_retirement_blockers());

        assert_eq!(
            rotating
                .retag_record_with_active_key(&quarantined)
                .expect("retag quarantined record in place"),
            VaultRecordRetagOutcome::Applied
        );
        assert_eq!(
            rotating.record_state(&quarantined).unwrap(),
            VaultRecordState::Quarantined
        );
        let audit = rotating
            .record_tag_dependency_audit(KEY_ID, 8)
            .expect("post-retag dependency audit");
        assert_eq!(audit.live_dependencies, 1);
        assert_eq!(audit.quarantined_dependencies, 0);
        assert_eq!(audit.staging_residues, 1);
        assert!(audit.has_retirement_blockers());
    }

    #[test]
    fn raw_retag_rejects_stale_revision_without_overwriting_installed_tag() {
        let root = TestRoot::new();
        let raw = FileVault::open(&root.0).unwrap();
        let reference = reference(RECORD_UUID);
        let old_ring = InstallationKeyRing::new(key(KEY_ID, 0x4b));
        let authenticated = record()
            .authenticated_copy(old_ring.authenticate_record(&reference, &record()))
            .unwrap();
        raw.create_record(&reference, &authenticated).unwrap();
        let expected = raw.read_record(&reference).unwrap();
        let new_ring = InstallationKeyRing::new(key(KEY_ID_2, 0x5b));
        let replacement = new_ring.authenticate_record(&reference, &expected);
        assert_eq!(
            raw.replace_record_authentication_tag(
                &reference,
                VaultRecordState::Present,
                &expected,
                &replacement,
            )
            .unwrap(),
            VaultRecordRetagOutcome::Applied
        );
        assert_eq!(
            raw.replace_record_authentication_tag(
                &reference,
                VaultRecordState::Present,
                &expected,
                &replacement,
            )
            .unwrap(),
            VaultRecordRetagOutcome::AlreadyApplied
        );
        assert_eq!(
            raw.replace_record_authentication_tag(
                &reference,
                VaultRecordState::Present,
                &record(),
                &replacement,
            ),
            Err(VaultBackendError::RecordAuthenticationMissing)
        );

        let third_ring = InstallationKeyRing::new(key(KEY_ID_3, 0x6b));
        let conflicting = third_ring.authenticate_record(&reference, &expected);
        assert_eq!(
            raw.replace_record_authentication_tag(
                &reference,
                VaultRecordState::Present,
                &expected,
                &conflicting,
            )
            .unwrap(),
            VaultRecordRetagOutcome::RevisionConflict
        );
        let installed = raw.read_record(&reference).unwrap();
        assert_eq!(installed.authentication_tag().unwrap(), &replacement);
        assert!(installed.same_payload(&expected));
        assert_eq!(raw.staging_residue_count().unwrap(), 0);
    }

    #[test]
    fn dependency_audit_fails_closed_for_legacy_and_unknown_tags() {
        let legacy_root = TestRoot::new();
        let legacy_raw = FileVault::open(&legacy_root.0).unwrap();
        let reference = reference(RECORD_UUID);
        legacy_raw.create_record(&reference, &record()).unwrap();
        let mut legacy =
            AuthenticatedVault::new(legacy_raw, InstallationKeyRing::new(key(KEY_ID, 0x4c)));
        assert_eq!(
            legacy.record_tag_dependency_audit(KEY_ID, 8),
            Err(VaultBackendError::RecordAuthenticationMissing)
        );

        let unknown_root = TestRoot::new();
        let unknown_raw = FileVault::open(&unknown_root.0).unwrap();
        let unknown_ring = InstallationKeyRing::new(key(KEY_ID_3, 0x6c));
        let authenticated = record()
            .authenticated_copy(unknown_ring.authenticate_record(&reference, &record()))
            .unwrap();
        unknown_raw
            .create_record(&reference, &authenticated)
            .unwrap();
        let mut unknown =
            AuthenticatedVault::new(unknown_raw, InstallationKeyRing::new(key(KEY_ID, 0x4c)));
        assert_eq!(
            unknown.record_tag_dependency_audit(KEY_ID, 8),
            Err(VaultBackendError::InstallationKeyUnavailable)
        );
    }

    #[test]
    fn dependency_audit_detects_retag_even_when_namespace_is_unchanged() {
        let root = TestRoot::new();
        let raw = FileVault::open(&root.0).unwrap();
        let reference = reference(RECORD_UUID);
        let mut old = AuthenticatedVault::new(raw, InstallationKeyRing::new(key(KEY_ID, 0x4d)));
        old.create_record(&reference, &record()).unwrap();
        let (raw, _) = old.into_parts();
        let ring = InstallationKeyRing::with_verification_keys(
            key(KEY_ID_2, 0x5d),
            vec![key(KEY_ID, 0x4d)],
        )
        .unwrap();
        let current = raw.read_record(&reference).unwrap();
        let replacement = ring.authenticate_record(&reference, &current);
        let backend = RetagOnSecondRawReadBackend {
            raw,
            replacement,
            raw_reads: 0,
        };
        let mut rotating = AuthenticatedVault::new(backend, ring);

        assert_eq!(
            rotating.record_tag_dependency_audit(KEY_ID, 8),
            Err(VaultBackendError::InventoryChanged)
        );
        let (backend, ring) = rotating.into_parts();
        let installed = backend.raw.read_record(&reference).unwrap();
        let installed_tag = installed.authentication_tag().unwrap();
        assert_eq!(installed_tag.key_id(), KEY_ID_2);
        ring.verify_record(&reference, &installed, installed_tag)
            .unwrap();
        assert_eq!(backend.raw.staging_residue_count().unwrap(), 0);
    }

    #[test]
    fn wrong_or_retired_key_fails_closed_before_transition() {
        let root = TestRoot::new();
        let raw = FileVault::open(&root.0).unwrap();
        let reference = reference(RECORD_UUID);
        let mut writer = AuthenticatedVault::new(raw, InstallationKeyRing::new(key(KEY_ID, 0x66)));
        writer.create_record(&reference, &record()).unwrap();
        let (raw, _) = writer.into_parts();
        let mut wrong = AuthenticatedVault::new(raw, InstallationKeyRing::new(key(KEY_ID_2, 0x77)));
        assert!(matches!(
            wrong.read_record(&reference),
            Err(VaultBackendError::InstallationKeyUnavailable)
        ));
        assert_eq!(
            wrong.quarantine_record(&reference),
            Err(VaultBackendError::InstallationKeyUnavailable)
        );
        assert_eq!(
            wrong.record_state(&reference).unwrap(),
            VaultRecordState::Present
        );
    }

    #[test]
    fn wrapper_debug_and_errors_are_redacted() {
        let root = TestRoot::new();
        let raw = FileVault::open(&root.0).unwrap();
        let vault = AuthenticatedVault::new(raw, InstallationKeyRing::new(key(KEY_ID, 0x88)));
        let path_canary = root.0.to_string_lossy();
        let redacted_tag = InstallationKeyRing::new(key(KEY_ID, 0x89))
            .authenticate_record(&reference(RECORD_UUID), &record());
        for rendered in [
            format!("{vault:?}"),
            format!("{redacted_tag:?}"),
            format!(
                "{:?}",
                VaultRecordTagDependencyAudit {
                    inspected_records: 1,
                    live_dependencies: 1,
                    quarantined_dependencies: 0,
                    staging_residues: 0,
                }
            ),
            VaultBackendError::RecordAuthenticationFailed.to_string(),
            format!("{:?}", VaultBackendError::InstallationKeyUnavailable),
        ] {
            assert!(!rendered.contains(KEY_ID));
            assert!(!rendered.contains(IDENTITY_CANARY));
            assert!(!rendered.contains(TOKEN_CANARY));
            assert!(!rendered.contains(path_canary.as_ref()));
        }
    }
}
