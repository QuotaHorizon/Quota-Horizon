use std::fmt;

use capacity_domain::{VaultOperation, VaultRecordRef};
use capacity_store::CapacityStore;
use capacity_vault::{
    ProtectedRecordInventoryBackend, VaultBackendError, VaultInventoryRecord, VaultRecordState,
    VaultStagingResidueId,
};

use crate::{LegacyViewerProbe, MutationLock, MutationLockError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultReconciliationArtifact {
    Record {
        record_ref: VaultRecordRef,
        observed_state: VaultRecordState,
    },
    StagingResidue {
        residue_id: VaultStagingResidueId,
        record_ref: VaultRecordRef,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultReconciliationDisposition {
    HealthyOwned,
    OperationManaged,
    OrphanQuarantined,
    RecoveryRecordRetained,
    StagingResidueRetained,
    ClaimAppearedRestored,
    ManualReviewRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultReconciliationItem {
    pub artifact: VaultReconciliationArtifact,
    pub disposition: VaultReconciliationDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultReconciliationReport {
    pub scanned: u32,
    pub quarantined_orphans: u32,
    pub retained: u32,
    pub review_required: u32,
    pub items: Vec<VaultReconciliationItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultReconciliationError {
    Lock(MutationLockError),
    Backend(VaultBackendError),
    StoreUnavailable,
    InvariantViolation,
}

impl fmt::Display for VaultReconciliationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(formatter),
            Self::Backend(error) => error.fmt(formatter),
            Self::StoreUnavailable => formatter.write_str("account-vault metadata is unavailable"),
            Self::InvariantViolation => {
                formatter.write_str("account-vault reconciliation invariant failed")
            }
        }
    }
}

impl std::error::Error for VaultReconciliationError {}

pub(crate) fn reconcile_vault_artifacts<
    B: ProtectedRecordInventoryBackend,
    P: LegacyViewerProbe,
>(
    lock: &MutationLock,
    legacy_probe: &mut P,
    metadata: &CapacityStore,
    protected_backend: &mut B,
    limit: u32,
) -> Result<VaultReconciliationReport, VaultReconciliationError> {
    verify_admission(lock, legacy_probe)?;
    let inventory = protected_backend
        .inventory(limit)
        .map_err(VaultReconciliationError::Backend)?;
    let capacity = inventory
        .records
        .len()
        .checked_add(inventory.staging_residues.len())
        .ok_or(VaultReconciliationError::InvariantViolation)?;
    let mut report = VaultReconciliationReport {
        scanned: 0,
        quarantined_orphans: 0,
        retained: 0,
        review_required: 0,
        items: Vec::with_capacity(capacity),
    };

    for record in inventory.records {
        verify_admission(lock, legacy_probe)?;
        let disposition = reconcile_record(metadata, protected_backend, &record)?;
        push_item(
            &mut report,
            VaultReconciliationArtifact::Record {
                record_ref: record.record_ref,
                observed_state: record.state,
            },
            disposition,
        )?;
        verify_admission(lock, legacy_probe)?;
    }

    for residue in inventory.staging_residues {
        verify_admission(lock, legacy_probe)?;
        let (_, operation) = claims(metadata, &residue.record_ref)?;
        let disposition = if operation.is_some() {
            VaultReconciliationDisposition::OperationManaged
        } else {
            VaultReconciliationDisposition::StagingResidueRetained
        };
        push_item(
            &mut report,
            VaultReconciliationArtifact::StagingResidue {
                residue_id: residue.residue_id,
                record_ref: residue.record_ref,
            },
            disposition,
        )?;
        verify_admission(lock, legacy_probe)?;
    }

    let accounted = report
        .quarantined_orphans
        .checked_add(report.retained)
        .and_then(|value| value.checked_add(report.review_required))
        .ok_or(VaultReconciliationError::InvariantViolation)?;
    if accounted != report.scanned
        || usize::try_from(report.scanned).ok() != Some(report.items.len())
        || report.scanned as usize != capacity
    {
        return Err(VaultReconciliationError::InvariantViolation);
    }
    verify_admission(lock, legacy_probe)?;
    Ok(report)
}

fn reconcile_record<B: ProtectedRecordInventoryBackend>(
    metadata: &CapacityStore,
    protected_backend: &mut B,
    inventory_record: &VaultInventoryRecord,
) -> Result<VaultReconciliationDisposition, VaultReconciliationError> {
    let current_state = protected_backend
        .record_state(&inventory_record.record_ref)
        .map_err(VaultReconciliationError::Backend)?;
    if current_state != inventory_record.state {
        return Ok(VaultReconciliationDisposition::ManualReviewRequired);
    }
    let (owner, operation) = claims(metadata, &inventory_record.record_ref)?;
    if current_state == VaultRecordState::Conflict {
        return Ok(VaultReconciliationDisposition::ManualReviewRequired);
    }
    if operation.is_some() {
        return Ok(VaultReconciliationDisposition::OperationManaged);
    }
    if owner {
        return Ok(
            if current_state == VaultRecordState::Present
                && protected_backend
                    .read_record(&inventory_record.record_ref)
                    .is_ok()
            {
                VaultReconciliationDisposition::HealthyOwned
            } else {
                VaultReconciliationDisposition::ManualReviewRequired
            },
        );
    }
    match current_state {
        VaultRecordState::Present => {
            quarantine_unclaimed_record(metadata, protected_backend, &inventory_record.record_ref)
        }
        VaultRecordState::Quarantined => Ok(VaultReconciliationDisposition::RecoveryRecordRetained),
        VaultRecordState::Conflict | VaultRecordState::Missing => {
            Ok(VaultReconciliationDisposition::ManualReviewRequired)
        }
    }
}

fn quarantine_unclaimed_record<B: ProtectedRecordInventoryBackend>(
    metadata: &CapacityStore,
    protected_backend: &mut B,
    record_ref: &VaultRecordRef,
) -> Result<VaultReconciliationDisposition, VaultReconciliationError> {
    let (owner, operation) = claims(metadata, record_ref)?;
    if operation.is_some() {
        return Ok(VaultReconciliationDisposition::OperationManaged);
    }
    if owner {
        return Ok(VaultReconciliationDisposition::HealthyOwned);
    }
    if protected_backend
        .record_state(record_ref)
        .map_err(VaultReconciliationError::Backend)?
        != VaultRecordState::Present
    {
        return Ok(VaultReconciliationDisposition::ManualReviewRequired);
    }
    if protected_backend.read_record(record_ref).is_err() {
        return Ok(VaultReconciliationDisposition::ManualReviewRequired);
    }
    protected_backend
        .quarantine_record(record_ref)
        .map_err(VaultReconciliationError::Backend)?;
    if protected_backend
        .record_state(record_ref)
        .map_err(VaultReconciliationError::Backend)?
        != VaultRecordState::Quarantined
    {
        return Err(VaultReconciliationError::InvariantViolation);
    }

    let (owner, operation) = claims(metadata, record_ref)?;
    if operation.is_some() {
        return Ok(VaultReconciliationDisposition::OperationManaged);
    }
    if !owner {
        return Ok(VaultReconciliationDisposition::OrphanQuarantined);
    }

    protected_backend
        .restore_record(record_ref)
        .map_err(VaultReconciliationError::Backend)?;
    if protected_backend
        .record_state(record_ref)
        .map_err(VaultReconciliationError::Backend)?
        != VaultRecordState::Present
    {
        return Err(VaultReconciliationError::InvariantViolation);
    }
    let (owner, operation) = claims(metadata, record_ref)?;
    if operation.is_some() {
        return Ok(VaultReconciliationDisposition::OperationManaged);
    }
    if owner {
        return Ok(VaultReconciliationDisposition::ClaimAppearedRestored);
    }

    protected_backend
        .quarantine_record(record_ref)
        .map_err(VaultReconciliationError::Backend)?;
    if protected_backend
        .record_state(record_ref)
        .map_err(VaultReconciliationError::Backend)?
        != VaultRecordState::Quarantined
    {
        return Err(VaultReconciliationError::InvariantViolation);
    }
    Ok(VaultReconciliationDisposition::OrphanQuarantined)
}

fn claims(
    metadata: &CapacityStore,
    record_ref: &VaultRecordRef,
) -> Result<(bool, Option<VaultOperation>), VaultReconciliationError> {
    let owner = metadata
        .vault_account_by_record_ref(record_ref)
        .map_err(|_| VaultReconciliationError::StoreUnavailable)?
        .is_some();
    let operation = metadata
        .active_vault_operation_by_record_ref(record_ref)
        .map_err(|_| VaultReconciliationError::StoreUnavailable)?;
    Ok((owner, operation))
}

fn push_item(
    report: &mut VaultReconciliationReport,
    artifact: VaultReconciliationArtifact,
    disposition: VaultReconciliationDisposition,
) -> Result<(), VaultReconciliationError> {
    report.scanned = report
        .scanned
        .checked_add(1)
        .ok_or(VaultReconciliationError::InvariantViolation)?;
    match disposition {
        VaultReconciliationDisposition::OrphanQuarantined => {
            report.quarantined_orphans = report
                .quarantined_orphans
                .checked_add(1)
                .ok_or(VaultReconciliationError::InvariantViolation)?;
        }
        VaultReconciliationDisposition::ManualReviewRequired => {
            report.review_required = report
                .review_required
                .checked_add(1)
                .ok_or(VaultReconciliationError::InvariantViolation)?;
        }
        VaultReconciliationDisposition::HealthyOwned
        | VaultReconciliationDisposition::OperationManaged
        | VaultReconciliationDisposition::RecoveryRecordRetained
        | VaultReconciliationDisposition::StagingResidueRetained
        | VaultReconciliationDisposition::ClaimAppearedRestored => {
            report.retained = report
                .retained
                .checked_add(1)
                .ok_or(VaultReconciliationError::InvariantViolation)?;
        }
    }
    report.items.push(VaultReconciliationItem {
        artifact,
        disposition,
    });
    Ok(())
}

fn verify_admission(
    lock: &MutationLock,
    legacy_probe: &mut impl LegacyViewerProbe,
) -> Result<(), VaultReconciliationError> {
    lock.verify_admission(legacy_probe)
        .map_err(VaultReconciliationError::Lock)
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use capacity_domain::{
        AccountFingerprint, UtcTimestamp, VaultAccountAuthMode, VaultAccountLifecycle,
        VaultAccountRegistration, VaultAccountSource,
    };
    use capacity_store::{VaultAccountRegistrationOutcome, VaultOperationBeginOutcome};
    use capacity_vault::{
        FileVault, ProtectedRecordBackend, ProtectedVaultRecord, VaultBackendInventory,
        VaultTransitionOutcome, journaled_register_account,
    };

    use super::*;
    use crate::{LegacyViewerState, MutationLockOwner, MutationOwnerId};

    const OWNER_UUID: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73701";
    const TOKEN_CANARY: &str = "reconciliation-secret-token";
    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestRoot {
        base: PathBuf,
        lock: PathBuf,
        store: PathBuf,
        vault: PathBuf,
    }

    impl TestRoot {
        fn new() -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock")
                .as_nanos();
            let base = std::env::temp_dir().join(format!(
                "capacity-reconciliation-test-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(&base).expect("create test root");
            Self {
                lock: base.join("lock"),
                store: base.join("capacity.sqlite3"),
                vault: base.join("vault"),
                base,
            }
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let safe = self
                .base
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("capacity-reconciliation-test-"));
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
            timestamp("2026-08-30T10:00:00Z"),
        )
        .expect("owner")
    }

    fn acquire(root: &TestRoot, probe: &mut FakeLegacyProbe) -> MutationLock {
        MutationLock::acquire(&root.lock, owner(), probe).expect("acquire lock")
    }

    fn record_uuid(index: u16) -> String {
        format!("018f47a2-8a71-7f4a-9c35-1f4234a7{index:04x}")
    }

    fn registration(index: u16) -> VaultAccountRegistration {
        VaultAccountRegistration {
            account_fingerprint: AccountFingerprint::parse(format!(
                "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73401:{index:064x}"
            ))
            .expect("fingerprint"),
            protected_record_ref: VaultRecordRef::parse(format!(
                "vault-record:v1:{}",
                record_uuid(index)
            ))
            .expect("record ref"),
            display_name: format!("Synthetic account {index}"),
            auth_mode: VaultAccountAuthMode::ChatGpt,
            source: VaultAccountSource::ManualChatGpt,
            lifecycle: VaultAccountLifecycle::Ready,
            provider_id: Some("official_codex".into()),
            model: Some("gpt-5.6-sol".into()),
        }
    }

    fn protected_record(index: u16) -> ProtectedVaultRecord {
        ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_account_id",
            format!("reconciliation-{index}@example.invalid").into_bytes(),
            format!(r#"{{"auth_mode":"chatgpt","access_token":"{TOKEN_CANARY}-{index}"}}"#)
                .into_bytes(),
            None,
        )
        .expect("protected record")
    }

    fn create_record(vault: &FileVault, index: u16) {
        vault
            .create_record(
                &registration(index).protected_record_ref,
                &protected_record(index),
            )
            .expect("create record");
    }

    fn create_staging_residue(vault_root: &Path, index: u16, sequence: u64) -> PathBuf {
        let path = vault_root.join("staging-v1").join(format!(
            ".staging-{}-1234-1788070000000000000-{sequence}",
            record_uuid(index)
        ));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&path).expect("create staging residue");
        path
    }

    fn disposition_for_record(
        report: &VaultReconciliationReport,
        record_ref: &VaultRecordRef,
    ) -> VaultReconciliationDisposition {
        report
            .items
            .iter()
            .find_map(|item| match &item.artifact {
                VaultReconciliationArtifact::Record {
                    record_ref: current,
                    ..
                } if current == record_ref => Some(item.disposition),
                VaultReconciliationArtifact::Record { .. }
                | VaultReconciliationArtifact::StagingResidue { .. } => None,
            })
            .expect("record disposition")
    }

    fn staging_dispositions(
        report: &VaultReconciliationReport,
        record_ref: &VaultRecordRef,
    ) -> Vec<VaultReconciliationDisposition> {
        report
            .items
            .iter()
            .filter_map(|item| match &item.artifact {
                VaultReconciliationArtifact::StagingResidue {
                    record_ref: current,
                    ..
                } if current == record_ref => Some(item.disposition),
                VaultReconciliationArtifact::Record { .. }
                | VaultReconciliationArtifact::StagingResidue { .. } => None,
            })
            .collect()
    }

    #[test]
    fn reconciliation_quarantines_only_unclaimed_live_records() {
        let root = TestRoot::new();
        let mut store = CapacityStore::open(&root.store).expect("open store");
        let mut vault = FileVault::open(&root.vault).expect("open vault");

        let owned = registration(1);
        journaled_register_account(
            &mut store,
            &mut vault,
            &owned,
            &protected_record(1),
            &timestamp("2026-08-30T10:01:00Z"),
        )
        .expect("register owned account");

        let managed = registration(2);
        create_record(&vault, 2);
        assert!(matches!(
            store
                .begin_vault_registration_operation(&managed, &timestamp("2026-08-30T10:02:00Z"))
                .unwrap(),
            VaultOperationBeginOutcome::Created(_)
        ));

        let orphan = registration(3);
        create_record(&vault, 3);
        let recovery = registration(4);
        create_record(&vault, 4);
        vault
            .quarantine_record(&recovery.protected_record_ref)
            .expect("quarantine recovery record");
        let managed_residue = create_staging_residue(&root.vault, 2, 1);
        let orphan_residue = create_staging_residue(&root.vault, 5, 2);

        let mut probe = FakeLegacyProbe::default();
        let lock = acquire(&root, &mut probe);
        let report = lock
            .reconcile_vault_artifacts(&mut probe, &store, &mut vault, 16)
            .expect("reconcile inventory");

        assert_eq!(report.scanned, 6);
        assert_eq!(report.quarantined_orphans, 1);
        assert_eq!(report.retained, 5);
        assert_eq!(report.review_required, 0);
        assert_eq!(
            disposition_for_record(&report, &owned.protected_record_ref),
            VaultReconciliationDisposition::HealthyOwned
        );
        assert_eq!(
            disposition_for_record(&report, &managed.protected_record_ref),
            VaultReconciliationDisposition::OperationManaged
        );
        assert_eq!(
            disposition_for_record(&report, &orphan.protected_record_ref),
            VaultReconciliationDisposition::OrphanQuarantined
        );
        assert_eq!(
            disposition_for_record(&report, &recovery.protected_record_ref),
            VaultReconciliationDisposition::RecoveryRecordRetained
        );
        assert_eq!(
            staging_dispositions(&report, &managed.protected_record_ref),
            vec![VaultReconciliationDisposition::OperationManaged]
        );
        assert_eq!(
            staging_dispositions(&report, &registration(5).protected_record_ref),
            vec![VaultReconciliationDisposition::StagingResidueRetained]
        );
        assert_eq!(
            vault.record_state(&orphan.protected_record_ref),
            Ok(VaultRecordState::Quarantined)
        );
        assert_eq!(
            vault.record_state(&owned.protected_record_ref),
            Ok(VaultRecordState::Present)
        );
        assert!(managed_residue.exists());
        assert!(orphan_residue.exists());

        let rendered = format!("{report:?}");
        for canary in [
            TOKEN_CANARY,
            OWNER_UUID,
            record_uuid(1).as_str(),
            record_uuid(2).as_str(),
            record_uuid(3).as_str(),
            record_uuid(4).as_str(),
            record_uuid(5).as_str(),
            root.base.to_string_lossy().as_ref(),
        ] {
            assert!(!rendered.contains(canary));
        }
        assert!(rendered.contains("<redacted>"));
    }

    fn copy_record_directory(source: &Path, destination: &Path) {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(destination).expect("create destination");
        for entry in fs::read_dir(source).expect("read source") {
            let entry = entry.expect("source entry");
            let destination_file = destination.join(entry.file_name());
            fs::copy(entry.path(), &destination_file).expect("copy record file");
            fs::set_permissions(&destination_file, fs::Permissions::from_mode(0o600))
                .expect("private copied file");
        }
    }

    #[test]
    fn conflicts_and_corrupt_orphans_require_review_without_mutation() {
        let root = TestRoot::new();
        let mut store = CapacityStore::open(&root.store).expect("open store");
        let mut vault = FileVault::open(&root.vault).expect("open vault");

        let conflict = registration(1);
        create_record(&vault, 1);
        copy_record_directory(
            &root.vault.join("records-v1").join(record_uuid(1)),
            &root.vault.join("quarantine-v1").join(record_uuid(1)),
        );
        let corrupt = registration(2);
        let corrupt_path = root.vault.join("records-v1").join(record_uuid(2));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(&corrupt_path)
            .expect("create corrupt record");
        assert!(matches!(
            store
                .register_vault_account(&corrupt, &timestamp("2026-08-30T10:01:00Z"))
                .expect("register corrupt metadata owner"),
            VaultAccountRegistrationOutcome::Created(_)
        ));

        let mut probe = FakeLegacyProbe::default();
        let lock = acquire(&root, &mut probe);
        let report = lock
            .reconcile_vault_artifacts(&mut probe, &store, &mut vault, 8)
            .expect("classify review items");
        assert_eq!(report.scanned, 2);
        assert_eq!(report.quarantined_orphans, 0);
        assert_eq!(report.retained, 0);
        assert_eq!(report.review_required, 2);
        assert_eq!(
            disposition_for_record(&report, &conflict.protected_record_ref),
            VaultReconciliationDisposition::ManualReviewRequired
        );
        assert_eq!(
            disposition_for_record(&report, &corrupt.protected_record_ref),
            VaultReconciliationDisposition::ManualReviewRequired
        );
        assert_eq!(
            vault.record_state(&conflict.protected_record_ref),
            Ok(VaultRecordState::Conflict)
        );
        assert!(corrupt_path.exists());
    }

    struct ClaimOnQuarantine {
        inner: FileVault,
        competing_store: CapacityStore,
        registration: VaultAccountRegistration,
        claimed: bool,
    }

    impl ProtectedRecordBackend for ClaimOnQuarantine {
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
            let transition = self.inner.quarantine_record(record_ref)?;
            if !self.claimed {
                assert!(matches!(
                    self.competing_store
                        .register_vault_account(
                            &self.registration,
                            &timestamp("2026-08-30T10:05:00Z")
                        )
                        .expect("concurrent claim"),
                    VaultAccountRegistrationOutcome::Created(_)
                ));
                self.claimed = true;
            }
            Ok(transition)
        }

        fn restore_record(
            &mut self,
            record_ref: &VaultRecordRef,
        ) -> Result<VaultTransitionOutcome, VaultBackendError> {
            self.inner.restore_record(record_ref)
        }
    }

    impl ProtectedRecordInventoryBackend for ClaimOnQuarantine {
        fn inventory(&mut self, limit: u32) -> Result<VaultBackendInventory, VaultBackendError> {
            self.inner.inventory(limit)
        }
    }

    #[test]
    fn metadata_claim_appearing_after_quarantine_is_restored() {
        let root = TestRoot::new();
        let store = CapacityStore::open(&root.store).expect("open store");
        let competing_store = CapacityStore::open(&root.store).expect("open competing store");
        let vault = FileVault::open(&root.vault).expect("open vault");
        let registration = registration(1);
        create_record(&vault, 1);
        let mut backend = ClaimOnQuarantine {
            inner: vault,
            competing_store,
            registration: registration.clone(),
            claimed: false,
        };
        let mut probe = FakeLegacyProbe::default();
        let lock = acquire(&root, &mut probe);

        let report = lock
            .reconcile_vault_artifacts(&mut probe, &store, &mut backend, 4)
            .expect("reconcile concurrent claim");
        assert_eq!(report.scanned, 1);
        assert_eq!(report.quarantined_orphans, 0);
        assert_eq!(report.retained, 1);
        assert_eq!(
            report.items[0].disposition,
            VaultReconciliationDisposition::ClaimAppearedRestored
        );
        assert_eq!(
            backend
                .inner
                .record_state(&registration.protected_record_ref),
            Ok(VaultRecordState::Present)
        );
        assert!(
            store
                .vault_account_by_record_ref(&registration.protected_record_ref)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn invalid_limit_and_late_viewer_fail_closed() {
        let invalid_root = TestRoot::new();
        let invalid_store = CapacityStore::open(&invalid_root.store).expect("open store");
        let mut invalid_vault = FileVault::open(&invalid_root.vault).expect("open vault");
        let mut invalid_probe = FakeLegacyProbe::default();
        let invalid_lock = acquire(&invalid_root, &mut invalid_probe);
        assert_eq!(
            invalid_lock.reconcile_vault_artifacts(
                &mut invalid_probe,
                &invalid_store,
                &mut invalid_vault,
                0,
            ),
            Err(VaultReconciliationError::Backend(
                VaultBackendError::InvalidInventoryLimit
            ))
        );

        let root = TestRoot::new();
        let store = CapacityStore::open(&root.store).expect("open store");
        let mut vault = FileVault::open(&root.vault).expect("open vault");
        let orphan = registration(1);
        create_record(&vault, 1);
        let mut probe = FakeLegacyProbe::with_states([
            LegacyViewerState::NotRunning,
            LegacyViewerState::NotRunning,
            LegacyViewerState::NotRunning,
            LegacyViewerState::NotRunning,
            LegacyViewerState::Running,
        ]);
        let lock = acquire(&root, &mut probe);
        assert_eq!(
            lock.reconcile_vault_artifacts(&mut probe, &store, &mut vault, 4),
            Err(VaultReconciliationError::Lock(
                MutationLockError::LegacyViewerRunning
            ))
        );
        assert_eq!(probe.calls, 5);
        assert_eq!(
            vault.record_state(&orphan.protected_record_ref),
            Ok(VaultRecordState::Quarantined),
            "completed reversible quarantine remains, but no later item may run"
        );
    }
}
