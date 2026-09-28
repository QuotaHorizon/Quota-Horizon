//! Account identity, authentication, and protected-record primitives.
//!
//! This crate is a mutation-plane building block, not a default desktop
//! credential backend. It derives installation-secret HMAC bindings and stores
//! auth/config plus exact upstream identity outside ordinary SQLite behind a
//! [`VaultRecordRef`]. File adapters are explicit Unix fallbacks until platform
//! credential-store and Windows ACL evidence exists; active workflows should
//! wrap raw record storage in [`AuthenticatedVault`].

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::path::Component;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use capacity_domain::{VAULT_RECORD_REF_PREFIX, VaultRecordRef};

mod authenticated;
mod identity;
mod journaled;
mod orchestrator;

pub use authenticated::{
    AccountRegistrationDraft, AuthenticatedVault, RawAuthenticatedRecordBackend,
    RawAuthenticatedRecordMutationBackend, journaled_register_bound_account,
};
#[cfg(target_os = "macos")]
pub use identity::MacOsKeychainInstallationKeyStore;
pub use identity::{
    ACCOUNT_BINDING_DOMAIN, FileInstallationKeyStore, INSTALLATION_SECRET_BYTES, IdentityError,
    InstallationKey, InstallationKeyBackendRequest, InstallationKeyRing,
    InstallationKeyRingMutationOutcome, InstallationKeyRingRecord, InstallationKeyRingRevision,
    InstallationKeyRingStore, InstallationKeyStorageVersion, InstallationKeyStore,
    RECORD_AUTHENTICATION_PREFIX, RECORD_BINDING_DOMAIN, RecordAuthenticationTag,
    SelectedInstallationKeyStore, derive_account_fingerprint, select_installation_key_store,
};

pub use journaled::{
    JournaledAccountRegistrationReceipt, JournaledForgetAccountReceipt, VaultRecoveryAction,
    VaultRecoveryItem, VaultRecoveryReport, inspect_vault_recovery, journaled_forget_account,
    journaled_register_account,
};
pub use orchestrator::{
    AccountRegistrationReceipt, AccountVaultError, CompensationOutcome, ForgetAccountOutcome,
    MetadataFailureReason, ProtectedRecordBackend, forget_account, register_account,
};

const RECORDS_DIRECTORY: &str = "records-v1";
const QUARANTINE_DIRECTORY: &str = "quarantine-v1";
const STAGING_DIRECTORY: &str = "staging-v1";
const RECORD_FORMAT_FILE: &str = "record-format";
const ADAPTER_NAMESPACE_FILE: &str = "adapter-namespace";
const IDENTITY_KIND_FILE: &str = "identity-kind";
const IDENTITY_VALUE_FILE: &str = "identity-value";
const AUTH_FILE: &str = "auth.json";
const CONFIG_FILE: &str = "config.toml";
const RECORD_AUTHENTICATION_FILE: &str = "record-authentication";
const RECORD_FORMAT_V1: &[u8] = b"capacity-vault-record-v1\n";
const RECORD_FORMAT_V2: &[u8] = b"capacity-vault-record-v2\n";
const MAX_RECORD_AUTHENTICATION_BYTES: u64 =
    (RECORD_AUTHENTICATION_PREFIX.len() + 36 + 1 + 64) as u64;

const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const MAX_ADAPTER_NAMESPACE_BYTES: u64 = 128;
const MAX_IDENTITY_KIND_BYTES: u64 = 128;
const MAX_IDENTITY_VALUE_BYTES: u64 = 4 * 1024;
const MAX_AUTH_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_RECORD_ENTRIES: usize = 7;
const MAX_STAGING_ATTEMPTS: u64 = 16;
const MAX_STAGING_RESIDUES: u32 = 1024;
pub const MAX_VAULT_INVENTORY_ITEMS: u32 = 4096;

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Secret-bearing bytes with deliberately redacted debug output and no Clone
/// or serialization implementation.
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn expose_secret(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretBytes(<redacted>)")
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        // Best-effort only. A dedicated audited zeroization dependency remains
        // a future hardening item; this type does not claim guaranteed memory
        // erasure across compiler copies.
        self.0.fill(0);
    }
}

/// Protected material required to restore an account and re-derive its stable
/// fingerprint after installation-secret rotation.
pub struct ProtectedVaultRecord {
    adapter_namespace: String,
    identity_kind: String,
    upstream_identity: SecretBytes,
    auth_json: SecretBytes,
    config_toml: Option<SecretBytes>,
    authentication_tag: Option<RecordAuthenticationTag>,
}

impl ProtectedVaultRecord {
    pub fn new(
        adapter_namespace: impl Into<String>,
        identity_kind: impl Into<String>,
        upstream_identity: Vec<u8>,
        auth_json: Vec<u8>,
        config_toml: Option<Vec<u8>>,
    ) -> Result<Self, VaultBackendError> {
        Self::from_secret_parts(
            adapter_namespace.into(),
            identity_kind.into(),
            SecretBytes::new(upstream_identity),
            SecretBytes::new(auth_json),
            config_toml.map(SecretBytes::new),
            None,
        )
    }

    fn from_secret_parts(
        adapter_namespace: String,
        identity_kind: String,
        upstream_identity: SecretBytes,
        auth_json: SecretBytes,
        config_toml: Option<SecretBytes>,
        authentication_tag: Option<RecordAuthenticationTag>,
    ) -> Result<Self, VaultBackendError> {
        validate_identifier(
            &adapter_namespace,
            MAX_ADAPTER_NAMESPACE_BYTES as usize,
            "adapter_namespace",
        )?;
        validate_identifier(
            &identity_kind,
            MAX_IDENTITY_KIND_BYTES as usize,
            "identity_kind",
        )?;
        validate_identity(upstream_identity.expose_secret())?;
        validate_secret_payload(auth_json.expose_secret(), MAX_AUTH_BYTES, "auth_json")?;
        if let Some(config_toml) = &config_toml {
            validate_secret_payload(config_toml.expose_secret(), MAX_CONFIG_BYTES, "config_toml")?;
        }
        Ok(Self {
            adapter_namespace,
            identity_kind,
            upstream_identity,
            auth_json,
            config_toml,
            authentication_tag,
        })
    }

    pub fn adapter_namespace(&self) -> &str {
        &self.adapter_namespace
    }

    pub fn identity_kind(&self) -> &str {
        &self.identity_kind
    }

    pub fn upstream_identity(&self) -> &[u8] {
        self.upstream_identity.expose_secret()
    }

    pub fn auth_json(&self) -> &[u8] {
        self.auth_json.expose_secret()
    }

    pub fn config_toml(&self) -> Option<&[u8]> {
        self.config_toml.as_ref().map(SecretBytes::expose_secret)
    }

    pub fn is_authenticated(&self) -> bool {
        self.authentication_tag.is_some()
    }

    pub(crate) fn authentication_tag(&self) -> Option<&RecordAuthenticationTag> {
        self.authentication_tag.as_ref()
    }

    pub(crate) fn authenticated_copy(
        &self,
        authentication_tag: RecordAuthenticationTag,
    ) -> Result<Self, VaultBackendError> {
        Self::from_secret_parts(
            self.adapter_namespace.clone(),
            self.identity_kind.clone(),
            SecretBytes::new(self.upstream_identity().to_vec()),
            SecretBytes::new(self.auth_json().to_vec()),
            self.config_toml()
                .map(|config| SecretBytes::new(config.to_vec())),
            Some(authentication_tag),
        )
    }

    pub(crate) fn same_material(&self, other: &Self) -> bool {
        self.adapter_namespace == other.adapter_namespace
            && self.identity_kind == other.identity_kind
            && self.upstream_identity() == other.upstream_identity()
            && self.auth_json() == other.auth_json()
            && self.config_toml() == other.config_toml()
            && self.authentication_tag == other.authentication_tag
    }

    pub(crate) fn same_payload(&self, other: &Self) -> bool {
        self.adapter_namespace == other.adapter_namespace
            && self.identity_kind == other.identity_kind
            && self.upstream_identity() == other.upstream_identity()
            && self.auth_json() == other.auth_json()
            && self.config_toml() == other.config_toml()
    }

    pub(crate) fn into_unauthenticated(mut self) -> Self {
        self.authentication_tag = None;
        self
    }
}

impl fmt::Debug for ProtectedVaultRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtectedVaultRecord")
            .field("adapter_namespace", &self.adapter_namespace)
            .field("identity_kind", &self.identity_kind)
            .field("upstream_identity", &"<redacted>")
            .field("auth_json", &"<redacted>")
            .field(
                "config_toml",
                &self.config_toml.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "authentication_tag",
                &self.authentication_tag.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultRecordState {
    Missing,
    Present,
    Quarantined,
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultTransitionOutcome {
    Moved,
    AlreadyAtDestination,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct VaultStagingResidueId(String);

impl VaultStagingResidueId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for VaultStagingResidueId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VaultStagingResidueId(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultInventoryRecord {
    pub record_ref: VaultRecordRef,
    pub state: VaultRecordState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultStagingResidue {
    pub residue_id: VaultStagingResidueId,
    pub record_ref: VaultRecordRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultBackendInventory {
    pub records: Vec<VaultInventoryRecord>,
    pub staging_residues: Vec<VaultStagingResidue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultRecordRetagOutcome {
    Applied,
    AlreadyApplied,
    RevisionConflict,
}

/// Authenticated tag references that block retirement of one installation
/// key. Any staging residue is a conservative blocker because its intended
/// key cannot be proven without completing recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultRecordTagDependencyAudit {
    pub inspected_records: u32,
    pub live_dependencies: u32,
    pub quarantined_dependencies: u32,
    pub staging_residues: u32,
}

impl VaultRecordTagDependencyAudit {
    pub fn has_retirement_blockers(&self) -> bool {
        self.live_dependencies > 0 || self.quarantined_dependencies > 0 || self.staging_residues > 0
    }
}

/// Read-only namespace inventory required by startup orphan reconciliation.
/// Implementations must return one stable, bounded snapshot or fail closed.
pub trait ProtectedRecordInventoryBackend: ProtectedRecordBackend {
    fn inventory(&mut self, limit: u32) -> Result<VaultBackendInventory, VaultBackendError>;
}

/// Raw Unix owner-mode record storage. Use [`AuthenticatedVault`] for active
/// account mutations; direct access remains available for explicit v1
/// migration and structural recovery tooling.
pub struct FileVault {
    root: PathBuf,
    records_root: PathBuf,
    quarantine_root: PathBuf,
    staging_root: PathBuf,
}

impl fmt::Debug for FileVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileVault")
            .field("root", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl FileVault {
    /// Opens or creates a private vault root. Existing permission drift fails
    /// closed rather than being silently repaired.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, VaultBackendError> {
        let root = root.as_ref().to_path_buf();

        #[cfg(not(unix))]
        {
            let _ = root;
            return Err(VaultBackendError::PlatformPermissionsUnavailable);
        }

        #[cfg(unix)]
        {
            validate_root_path(&root)?;
            prepare_private_directory(&root)?;
            let records_root = root.join(RECORDS_DIRECTORY);
            let quarantine_root = root.join(QUARANTINE_DIRECTORY);
            let staging_root = root.join(STAGING_DIRECTORY);
            prepare_private_directory(&records_root)?;
            prepare_private_directory(&quarantine_root)?;
            prepare_private_directory(&staging_root)?;
            Ok(Self {
                root,
                records_root,
                quarantine_root,
                staging_root,
            })
        }
    }

    pub fn record_state(
        &self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultRecordState, VaultBackendError> {
        self.verify_roots()?;
        let name = record_directory_name(record_ref)?;
        let live = inspect_private_record_directory(&self.records_root.join(name))?;
        let quarantined = inspect_private_record_directory(&self.quarantine_root.join(name))?;
        Ok(match (live, quarantined) {
            (false, false) => VaultRecordState::Missing,
            (true, false) => VaultRecordState::Present,
            (false, true) => VaultRecordState::Quarantined,
            (true, true) => VaultRecordState::Conflict,
        })
    }

    /// Creates a new record without overwriting live or quarantined material.
    /// All files are synced in a private staging directory before one rename.
    pub fn create_record(
        &self,
        record_ref: &VaultRecordRef,
        record: &ProtectedVaultRecord,
    ) -> Result<(), VaultBackendError> {
        validate_record(record)?;
        match self.record_state(record_ref)? {
            VaultRecordState::Missing => {}
            VaultRecordState::Present => return Err(VaultBackendError::RecordAlreadyExists),
            VaultRecordState::Quarantined => {
                return Err(VaultBackendError::QuarantinedRecordExists);
            }
            VaultRecordState::Conflict => return Err(VaultBackendError::RecordStateConflict),
        }

        let name = record_directory_name(record_ref)?;
        let final_path = self.records_root.join(name);
        let staging_path = self.allocate_staging_directory(name)?;
        let mut staging_guard =
            StagingDirectoryGuard::new(&self.staging_root, staging_path.clone());

        let record_format = if record.authentication_tag.is_some() {
            RECORD_FORMAT_V2
        } else {
            RECORD_FORMAT_V1
        };
        write_private_file(&staging_path.join(RECORD_FORMAT_FILE), record_format)?;
        write_private_file(
            &staging_path.join(ADAPTER_NAMESPACE_FILE),
            record.adapter_namespace.as_bytes(),
        )?;
        write_private_file(
            &staging_path.join(IDENTITY_KIND_FILE),
            record.identity_kind.as_bytes(),
        )?;
        write_private_file(
            &staging_path.join(IDENTITY_VALUE_FILE),
            record.upstream_identity.expose_secret(),
        )?;
        write_private_file(
            &staging_path.join(AUTH_FILE),
            record.auth_json.expose_secret(),
        )?;
        if let Some(config_toml) = &record.config_toml {
            write_private_file(&staging_path.join(CONFIG_FILE), config_toml.expose_secret())?;
        }
        if let Some(authentication_tag) = &record.authentication_tag {
            write_private_file(
                &staging_path.join(RECORD_AUTHENTICATION_FILE),
                authentication_tag.as_str().as_bytes(),
            )?;
        }
        sync_directory(&staging_path)?;

        // Independent verification catches partial/permission-drifted staging
        // records before they can become the live target.
        drop(read_record_at(&staging_path)?);
        if inspect_private_record_directory(&final_path)? {
            return Err(VaultBackendError::RecordAlreadyExists);
        }
        fs::rename(&staging_path, &final_path).map_err(|error| {
            if final_path.exists() {
                VaultBackendError::RecordAlreadyExists
            } else {
                VaultBackendError::Io(error.kind())
            }
        })?;
        sync_directory(&self.records_root)?;
        staging_guard.disarm();
        Ok(())
    }

    pub fn read_record(
        &self,
        record_ref: &VaultRecordRef,
    ) -> Result<ProtectedVaultRecord, VaultBackendError> {
        self.verify_roots()?;
        let name = record_directory_name(record_ref)?;
        match self.record_state(record_ref)? {
            VaultRecordState::Present => read_record_at(&self.records_root.join(name)),
            VaultRecordState::Missing => Err(VaultBackendError::RecordMissing),
            VaultRecordState::Quarantined => Err(VaultBackendError::RecordQuarantined),
            VaultRecordState::Conflict => Err(VaultBackendError::RecordStateConflict),
        }
    }

    /// Atomically replaces only the authentication tag of one live or
    /// quarantined v2 record.
    /// The complete expected raw record is checked twice before a same-filesystem
    /// rename, and the complete payload plus replacement tag is checked after.
    pub(crate) fn replace_record_authentication_tag(
        &self,
        record_ref: &VaultRecordRef,
        state: VaultRecordState,
        expected: &ProtectedVaultRecord,
        replacement: &RecordAuthenticationTag,
    ) -> Result<VaultRecordRetagOutcome, VaultBackendError> {
        self.verify_roots()?;
        let name = record_directory_name(record_ref)?;
        let record_root = match (self.record_state(record_ref)?, state) {
            (VaultRecordState::Present, VaultRecordState::Present) => &self.records_root,
            (VaultRecordState::Quarantined, VaultRecordState::Quarantined) => &self.quarantine_root,
            (VaultRecordState::Missing, _) => return Err(VaultBackendError::RecordMissing),
            (VaultRecordState::Conflict, _) => {
                return Err(VaultBackendError::RecordStateConflict);
            }
            _ => return Err(VaultBackendError::RecordChangedDuringRead),
        };
        let record_path = record_root.join(name);
        let expected_tag = expected
            .authentication_tag()
            .ok_or(VaultBackendError::RecordAuthenticationMissing)?;
        let current = read_record_at(&record_path)?;
        let current_tag = current
            .authentication_tag()
            .ok_or(VaultBackendError::RecordAuthenticationMissing)?;
        if current.same_payload(expected) && current_tag == replacement {
            return Ok(VaultRecordRetagOutcome::AlreadyApplied);
        }
        if !current.same_material(expected) {
            return Ok(VaultRecordRetagOutcome::RevisionConflict);
        }
        if expected_tag == replacement {
            return Ok(VaultRecordRetagOutcome::AlreadyApplied);
        }

        let staging_path = self.allocate_staging_directory(name)?;
        let mut staging_guard =
            StagingDirectoryGuard::new(&self.staging_root, staging_path.clone());
        sync_directory(&self.staging_root)?;
        let staged_tag_path = staging_path.join(RECORD_AUTHENTICATION_FILE);
        write_private_file(&staged_tag_path, replacement.as_str().as_bytes())?;
        sync_directory(&staging_path)?;
        let staged = String::from_utf8(read_private_file(
            &staged_tag_path,
            MAX_RECORD_AUTHENTICATION_BYTES,
        )?)
        .map_err(|_| VaultBackendError::RecordCorrupt)?;
        let staged =
            RecordAuthenticationTag::parse(staged).map_err(|_| VaultBackendError::RecordCorrupt)?;
        if staged != *replacement {
            return Err(VaultBackendError::RecordCorrupt);
        }

        let observed = read_record_at(&record_path)?;
        if !observed.same_material(expected) {
            return Ok(VaultRecordRetagOutcome::RevisionConflict);
        }
        let record_tag_path = record_path.join(RECORD_AUTHENTICATION_FILE);
        fs::rename(&staged_tag_path, &record_tag_path)
            .map_err(|error| VaultBackendError::Io(error.kind()))?;
        // Once rename commits, preserve the now-empty residue on any later
        // error so recovery/audit can conservatively block predecessor retire.
        staging_guard.disarm();
        sync_directory(&record_path)?;

        let installed = read_record_at(&record_path)?;
        if !installed.same_payload(expected) || installed.authentication_tag() != Some(replacement)
        {
            return Ok(VaultRecordRetagOutcome::RevisionConflict);
        }
        fs::remove_dir(&staging_path).map_err(|error| VaultBackendError::Io(error.kind()))?;
        sync_directory(&self.staging_root)?;
        Ok(VaultRecordRetagOutcome::Applied)
    }

    /// Moves a verified live record out of service without permanent deletion.
    pub fn quarantine_record(
        &self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultTransitionOutcome, VaultBackendError> {
        self.verify_roots()?;
        let name = record_directory_name(record_ref)?;
        match self.record_state(record_ref)? {
            VaultRecordState::Missing => return Err(VaultBackendError::RecordMissing),
            VaultRecordState::Quarantined => {
                drop(read_record_at(&self.quarantine_root.join(name))?);
                return Ok(VaultTransitionOutcome::AlreadyAtDestination);
            }
            VaultRecordState::Conflict => return Err(VaultBackendError::RecordStateConflict),
            VaultRecordState::Present => {}
        }
        let source = self.records_root.join(name);
        let destination = self.quarantine_root.join(name);
        drop(read_record_at(&source)?);
        fs::rename(&source, &destination).map_err(|error| VaultBackendError::Io(error.kind()))?;
        sync_directory(&self.records_root)?;
        sync_directory(&self.quarantine_root)?;
        Ok(VaultTransitionOutcome::Moved)
    }

    /// Restores a verified quarantined record when no live record exists.
    pub fn restore_record(
        &self,
        record_ref: &VaultRecordRef,
    ) -> Result<VaultTransitionOutcome, VaultBackendError> {
        self.verify_roots()?;
        let name = record_directory_name(record_ref)?;
        match self.record_state(record_ref)? {
            VaultRecordState::Missing => return Err(VaultBackendError::RecordMissing),
            VaultRecordState::Present => {
                drop(read_record_at(&self.records_root.join(name))?);
                return Ok(VaultTransitionOutcome::AlreadyAtDestination);
            }
            VaultRecordState::Conflict => return Err(VaultBackendError::RecordStateConflict),
            VaultRecordState::Quarantined => {}
        }
        let source = self.quarantine_root.join(name);
        let destination = self.records_root.join(name);
        drop(read_record_at(&source)?);
        fs::rename(&source, &destination).map_err(|error| VaultBackendError::Io(error.kind()))?;
        sync_directory(&self.quarantine_root)?;
        sync_directory(&self.records_root)?;
        Ok(VaultTransitionOutcome::Moved)
    }

    pub fn staging_residue_count(&self) -> Result<u32, VaultBackendError> {
        self.verify_roots()?;
        let mut count = 0_u32;
        for entry in
            fs::read_dir(&self.staging_root).map_err(|error| VaultBackendError::Io(error.kind()))?
        {
            let entry = entry.map_err(|error| VaultBackendError::Io(error.kind()))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| VaultBackendError::UnexpectedVaultEntry)?;
            if !name.starts_with(".staging-") {
                return Err(VaultBackendError::UnexpectedVaultEntry);
            }
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| VaultBackendError::Io(error.kind()))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(VaultBackendError::UnsafeFileType);
            }
            verify_directory_permissions(&metadata)?;
            if count == MAX_STAGING_RESIDUES {
                return Err(VaultBackendError::BoundExceeded("staging_residue_count"));
            }
            count = count
                .checked_add(1)
                .ok_or(VaultBackendError::BoundExceeded("staging_residue_count"))?;
        }
        Ok(count)
    }

    /// Returns a deterministic, stable namespace snapshot without reading
    /// protected payload bytes. Unknown names, unsafe entries, concurrent
    /// directory changes, or an exceeded bound fail closed.
    pub fn inventory(&self, limit: u32) -> Result<VaultBackendInventory, VaultBackendError> {
        if limit == 0 || limit > MAX_VAULT_INVENTORY_ITEMS {
            return Err(VaultBackendError::InvalidInventoryLimit);
        }
        self.verify_roots()?;
        let root_before = verify_private_directory(&self.root)?;
        let live_names = stable_private_directory_names(&self.records_root, limit)?;
        let quarantined_names = stable_private_directory_names(&self.quarantine_root, limit)?;
        let staging_names = stable_private_directory_names(&self.staging_root, limit)?;

        let mut record_names = live_names.clone();
        record_names.extend(quarantined_names.iter().cloned());
        let item_count = record_names
            .len()
            .checked_add(staging_names.len())
            .ok_or(VaultBackendError::BoundExceeded("inventory_items"))?;
        if item_count > limit as usize {
            return Err(VaultBackendError::BoundExceeded("inventory_items"));
        }

        let mut records = Vec::with_capacity(record_names.len());
        for name in record_names {
            let record_ref = record_ref_from_directory_name(&name)?;
            let state = match (
                live_names.contains(&name),
                quarantined_names.contains(&name),
            ) {
                (true, false) => VaultRecordState::Present,
                (false, true) => VaultRecordState::Quarantined,
                (true, true) => VaultRecordState::Conflict,
                (false, false) => return Err(VaultBackendError::InventoryChanged),
            };
            records.push(VaultInventoryRecord { record_ref, state });
        }

        let mut staging_residues = Vec::with_capacity(staging_names.len());
        for name in staging_names {
            let (residue_id, record_ref) = parse_staging_residue_name(name)?;
            staging_residues.push(VaultStagingResidue {
                residue_id,
                record_ref,
            });
        }
        self.verify_roots()?;
        let root_after = verify_private_directory(&self.root)?;
        if !same_file_identity(&root_before, &root_after) {
            return Err(VaultBackendError::InventoryChanged);
        }
        Ok(VaultBackendInventory {
            records,
            staging_residues,
        })
    }

    fn verify_roots(&self) -> Result<(), VaultBackendError> {
        for directory in [
            &self.root,
            &self.records_root,
            &self.quarantine_root,
            &self.staging_root,
        ] {
            verify_private_directory(directory)?;
        }
        Ok(())
    }

    fn allocate_staging_directory(&self, record_name: &str) -> Result<PathBuf, VaultBackendError> {
        let epoch_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| VaultBackendError::ClockUnavailable)?
            .as_nanos();
        for _ in 0..MAX_STAGING_ATTEMPTS {
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let name = format!(
                ".staging-{record_name}-{}-{epoch_nanos}-{sequence}",
                std::process::id()
            );
            let path = self.staging_root.join(name);
            match create_private_directory_new(&path) {
                Ok(()) => return Ok(path),
                Err(VaultBackendError::Io(std::io::ErrorKind::AlreadyExists)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(VaultBackendError::BoundExceeded("staging_attempts"))
    }
}

impl ProtectedRecordInventoryBackend for FileVault {
    fn inventory(&mut self, limit: u32) -> Result<VaultBackendInventory, VaultBackendError> {
        FileVault::inventory(self, limit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultBackendError {
    InvalidRoot,
    PlatformPermissionsUnavailable,
    InvalidPayload(&'static str),
    LimitExceeded(&'static str),
    BoundExceeded(&'static str),
    UnsafeFileType,
    InsecurePermissions,
    UnexpectedVaultEntry,
    RecordCorrupt,
    RecordChangedDuringRead,
    RecordAuthenticationMissing,
    RecordAuthenticationFailed,
    InstallationKeyUnavailable,
    RecordAlreadyExists,
    QuarantinedRecordExists,
    RecordMissing,
    RecordQuarantined,
    RecordStateConflict,
    InvalidInventoryLimit,
    InventoryChanged,
    ClockUnavailable,
    Io(std::io::ErrorKind),
}

impl fmt::Display for VaultBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidRoot => "vault root is not an absolute bounded path",
            Self::PlatformPermissionsUnavailable => {
                "platform private-file permission verification is unavailable"
            }
            Self::InvalidPayload(_) => "protected vault payload is invalid",
            Self::LimitExceeded(_) | Self::BoundExceeded(_) => "protected vault bound was exceeded",
            Self::UnsafeFileType => "vault path has an unsafe file type",
            Self::InsecurePermissions => "vault permissions are not restricted to owner mode",
            Self::UnexpectedVaultEntry => "vault record contains an unexpected entry",
            Self::RecordCorrupt => "protected vault record is incomplete or corrupt",
            Self::RecordChangedDuringRead => "protected vault record changed during read",
            Self::RecordAuthenticationMissing => "protected vault record is not authenticated",
            Self::RecordAuthenticationFailed => "protected vault record authentication failed",
            Self::InstallationKeyUnavailable => "required installation key is unavailable",
            Self::RecordAlreadyExists => "live protected vault record already exists",
            Self::QuarantinedRecordExists => "quarantined protected vault record already exists",
            Self::RecordMissing => "protected vault record is missing",
            Self::RecordQuarantined => "protected vault record is quarantined",
            Self::RecordStateConflict => "live and quarantined vault records conflict",
            Self::InvalidInventoryLimit => "protected vault inventory limit is invalid",
            Self::InventoryChanged => "protected vault inventory changed during inspection",
            Self::ClockUnavailable => "system clock is unavailable for staging allocation",
            Self::Io(_) => "protected vault filesystem operation failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for VaultBackendError {}

impl From<std::io::Error> for VaultBackendError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.kind())
    }
}

fn validate_record(record: &ProtectedVaultRecord) -> Result<(), VaultBackendError> {
    validate_identifier(
        &record.adapter_namespace,
        MAX_ADAPTER_NAMESPACE_BYTES as usize,
        "adapter_namespace",
    )?;
    validate_identifier(
        &record.identity_kind,
        MAX_IDENTITY_KIND_BYTES as usize,
        "identity_kind",
    )?;
    validate_identity(record.upstream_identity.expose_secret())?;
    validate_secret_payload(
        record.auth_json.expose_secret(),
        MAX_AUTH_BYTES,
        "auth_json",
    )?;
    if let Some(config_toml) = &record.config_toml {
        validate_secret_payload(config_toml.expose_secret(), MAX_CONFIG_BYTES, "config_toml")?;
    }
    Ok(())
}

fn validate_identifier(
    value: &str,
    maximum_length: usize,
    field: &'static str,
) -> Result<(), VaultBackendError> {
    let valid = !value.is_empty()
        && value.len() <= maximum_length
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b'-' | b':')
        });
    if !valid {
        return Err(VaultBackendError::InvalidPayload(field));
    }
    Ok(())
}

fn validate_identity(value: &[u8]) -> Result<(), VaultBackendError> {
    if value.is_empty() || value.len() as u64 > MAX_IDENTITY_VALUE_BYTES {
        return Err(VaultBackendError::InvalidPayload("upstream_identity"));
    }
    let text = std::str::from_utf8(value)
        .map_err(|_| VaultBackendError::InvalidPayload("upstream_identity"))?;
    if text != text.trim() || text.chars().any(char::is_control) {
        return Err(VaultBackendError::InvalidPayload("upstream_identity"));
    }
    Ok(())
}

fn validate_secret_payload(
    value: &[u8],
    maximum_bytes: u64,
    field: &'static str,
) -> Result<(), VaultBackendError> {
    if value.is_empty() {
        return Err(VaultBackendError::InvalidPayload(field));
    }
    if value.len() as u64 > maximum_bytes {
        return Err(VaultBackendError::LimitExceeded(field));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_root_path(path: &Path) -> Result<(), VaultBackendError> {
    use std::os::unix::ffi::OsStrExt;

    let has_dot_segment = path
        .as_os_str()
        .as_bytes()
        .split(|byte| *byte == b'/')
        .any(|segment| segment == b"." || segment == b"..");
    if !path.is_absolute()
        || path.parent().is_none()
        || has_dot_segment
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir | Component::ParentDir | Component::Prefix(_)
            )
        })
        || path
            .components()
            .filter(|part| matches!(part, Component::Normal(_)))
            .count()
            < 2
    {
        return Err(VaultBackendError::InvalidRoot);
    }
    Ok(())
}

fn record_directory_name(record_ref: &VaultRecordRef) -> Result<&str, VaultBackendError> {
    record_ref
        .as_str()
        .strip_prefix(VAULT_RECORD_REF_PREFIX)
        .ok_or(VaultBackendError::RecordCorrupt)
}

fn record_ref_from_directory_name(name: &str) -> Result<VaultRecordRef, VaultBackendError> {
    VaultRecordRef::parse(format!("{VAULT_RECORD_REF_PREFIX}{name}"))
        .map_err(|_| VaultBackendError::UnexpectedVaultEntry)
}

fn stable_private_directory_names(
    path: &Path,
    limit: u32,
) -> Result<BTreeSet<String>, VaultBackendError> {
    let before = verify_private_directory(path)?;
    let first = private_directory_names(path, limit)?;
    let second = private_directory_names(path, limit)?;
    let after = verify_private_directory(path)?;
    if first != second || !same_file_identity(&before, &after) {
        return Err(VaultBackendError::InventoryChanged);
    }
    Ok(first)
}

fn private_directory_names(path: &Path, limit: u32) -> Result<BTreeSet<String>, VaultBackendError> {
    let before = verify_private_directory(path)?;
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(path).map_err(|error| VaultBackendError::Io(error.kind()))? {
        let entry = entry.map_err(|error| VaultBackendError::Io(error.kind()))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| VaultBackendError::UnexpectedVaultEntry)?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| VaultBackendError::Io(error.kind()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(VaultBackendError::UnsafeFileType);
        }
        verify_directory_permissions(&metadata)?;
        if !names.insert(name) {
            return Err(VaultBackendError::UnexpectedVaultEntry);
        }
        if names.len() > limit as usize {
            return Err(VaultBackendError::BoundExceeded("inventory_items"));
        }
    }
    let after = verify_private_directory(path)?;
    if !same_file_identity(&before, &after) {
        return Err(VaultBackendError::InventoryChanged);
    }
    Ok(names)
}

fn parse_staging_residue_name(
    name: String,
) -> Result<(VaultStagingResidueId, VaultRecordRef), VaultBackendError> {
    const UUID_BYTES: usize = 36;
    let rest = name
        .strip_prefix(".staging-")
        .ok_or(VaultBackendError::UnexpectedVaultEntry)?;
    if !rest.is_ascii()
        || rest.len() <= UUID_BYTES
        || rest.as_bytes().get(UUID_BYTES) != Some(&b'-')
    {
        return Err(VaultBackendError::UnexpectedVaultEntry);
    }
    let record_ref = record_ref_from_directory_name(&rest[..UUID_BYTES])?;
    let mut components = rest[UUID_BYTES + 1..].split('-');
    let process_id = components
        .next()
        .ok_or(VaultBackendError::UnexpectedVaultEntry)?;
    let epoch_nanos = components
        .next()
        .ok_or(VaultBackendError::UnexpectedVaultEntry)?;
    let sequence = components
        .next()
        .ok_or(VaultBackendError::UnexpectedVaultEntry)?;
    if components.next().is_some()
        || !canonical_positive_u32(process_id)
        || !canonical_positive_u128(epoch_nanos)
        || !canonical_u64(sequence)
    {
        return Err(VaultBackendError::UnexpectedVaultEntry);
    }
    Ok((VaultStagingResidueId(name), record_ref))
}

fn canonical_positive_u32(value: &str) -> bool {
    value
        .parse::<u32>()
        .ok()
        .filter(|parsed| *parsed > 0 && *parsed <= i32::MAX as u32)
        .is_some_and(|parsed| parsed.to_string() == value)
}

fn canonical_positive_u128(value: &str) -> bool {
    value
        .parse::<u128>()
        .ok()
        .filter(|parsed| *parsed > 0)
        .is_some_and(|parsed| parsed.to_string() == value)
}

fn canonical_u64(value: &str) -> bool {
    value
        .parse::<u64>()
        .ok()
        .is_some_and(|parsed| parsed.to_string() == value)
}

fn read_record_at(path: &Path) -> Result<ProtectedVaultRecord, VaultBackendError> {
    let before = verify_private_directory(path)?;
    let mut entries = BTreeSet::new();
    for entry in fs::read_dir(path).map_err(|error| VaultBackendError::Io(error.kind()))? {
        let entry = entry.map_err(|error| VaultBackendError::Io(error.kind()))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| VaultBackendError::UnexpectedVaultEntry)?;
        if ![
            RECORD_FORMAT_FILE,
            ADAPTER_NAMESPACE_FILE,
            IDENTITY_KIND_FILE,
            IDENTITY_VALUE_FILE,
            AUTH_FILE,
            CONFIG_FILE,
            RECORD_AUTHENTICATION_FILE,
        ]
        .contains(&name.as_str())
            || !entries.insert(name)
            || entries.len() > MAX_RECORD_ENTRIES
        {
            return Err(VaultBackendError::UnexpectedVaultEntry);
        }
    }
    for required in [
        RECORD_FORMAT_FILE,
        ADAPTER_NAMESPACE_FILE,
        IDENTITY_KIND_FILE,
        IDENTITY_VALUE_FILE,
        AUTH_FILE,
    ] {
        if !entries.contains(required) {
            return Err(VaultBackendError::RecordCorrupt);
        }
    }

    let record_format = read_private_file(
        &path.join(RECORD_FORMAT_FILE),
        RECORD_FORMAT_V1.len().max(RECORD_FORMAT_V2.len()) as u64,
    )?;
    let authenticated_format = if record_format == RECORD_FORMAT_V1 {
        if entries.contains(RECORD_AUTHENTICATION_FILE) {
            return Err(VaultBackendError::RecordCorrupt);
        }
        false
    } else if record_format == RECORD_FORMAT_V2 {
        if !entries.contains(RECORD_AUTHENTICATION_FILE) {
            return Err(VaultBackendError::RecordCorrupt);
        }
        true
    } else {
        return Err(VaultBackendError::RecordCorrupt);
    };
    let adapter_namespace = String::from_utf8(read_private_file(
        &path.join(ADAPTER_NAMESPACE_FILE),
        MAX_ADAPTER_NAMESPACE_BYTES,
    )?)
    .map_err(|_| VaultBackendError::RecordCorrupt)?;
    let identity_kind = String::from_utf8(read_private_file(
        &path.join(IDENTITY_KIND_FILE),
        MAX_IDENTITY_KIND_BYTES,
    )?)
    .map_err(|_| VaultBackendError::RecordCorrupt)?;
    let upstream_identity = SecretBytes::new(read_private_file(
        &path.join(IDENTITY_VALUE_FILE),
        MAX_IDENTITY_VALUE_BYTES,
    )?);
    let auth_json = SecretBytes::new(read_private_file(&path.join(AUTH_FILE), MAX_AUTH_BYTES)?);
    let config_toml = if entries.contains(CONFIG_FILE) {
        Some(SecretBytes::new(read_private_file(
            &path.join(CONFIG_FILE),
            MAX_CONFIG_BYTES,
        )?))
    } else {
        None
    };
    let authentication_tag = if authenticated_format {
        let encoded = String::from_utf8(read_private_file(
            &path.join(RECORD_AUTHENTICATION_FILE),
            MAX_RECORD_AUTHENTICATION_BYTES,
        )?)
        .map_err(|_| VaultBackendError::RecordCorrupt)?;
        Some(
            RecordAuthenticationTag::parse(encoded)
                .map_err(|_| VaultBackendError::RecordCorrupt)?,
        )
    } else {
        None
    };
    let after = verify_private_directory(path)?;
    if !same_file_identity(&before, &after) {
        return Err(VaultBackendError::RecordChangedDuringRead);
    }

    ProtectedVaultRecord::from_secret_parts(
        adapter_namespace,
        identity_kind,
        upstream_identity,
        auth_json,
        config_toml,
        authentication_tag,
    )
    .map_err(|_| VaultBackendError::RecordCorrupt)
}

fn read_private_file(path: &Path, maximum_bytes: u64) -> Result<Vec<u8>, VaultBackendError> {
    let before = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            VaultBackendError::RecordCorrupt
        } else {
            VaultBackendError::Io(error.kind())
        }
    })?;
    verify_private_file_metadata(&before)?;
    if before.len() > maximum_bytes {
        return Err(VaultBackendError::LimitExceeded("record_file"));
    }
    let file = File::open(path).map_err(|error| VaultBackendError::Io(error.kind()))?;
    let opened = file
        .metadata()
        .map_err(|error| VaultBackendError::Io(error.kind()))?;
    verify_private_file_metadata(&opened)?;
    if !same_file_identity(&before, &opened) {
        return Err(VaultBackendError::RecordChangedDuringRead);
    }
    let mut bytes = Vec::new();
    file.take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| VaultBackendError::Io(error.kind()))?;
    if bytes.len() as u64 > maximum_bytes {
        return Err(VaultBackendError::LimitExceeded("record_file"));
    }
    let after =
        fs::symlink_metadata(path).map_err(|_| VaultBackendError::RecordChangedDuringRead)?;
    verify_private_file_metadata(&after)?;
    if !same_file_identity(&opened, &after) {
        return Err(VaultBackendError::RecordChangedDuringRead);
    }
    Ok(bytes)
}

fn inspect_private_record_directory(path: &Path) -> Result<bool, VaultBackendError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(VaultBackendError::UnsafeFileType);
            }
            verify_directory_permissions(&metadata)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(VaultBackendError::Io(error.kind())),
    }
}

fn verify_private_directory(path: &Path) -> Result<Metadata, VaultBackendError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| VaultBackendError::Io(error.kind()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(VaultBackendError::UnsafeFileType);
    }
    verify_directory_permissions(&metadata)?;
    Ok(metadata)
}

#[cfg(unix)]
fn prepare_private_directory(path: &Path) -> Result<(), VaultBackendError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(VaultBackendError::UnsafeFileType);
            }
            verify_directory_permissions(&metadata)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_directory_new(path)
        }
        Err(error) => Err(VaultBackendError::Io(error.kind())),
    }
}

#[cfg(unix)]
fn create_private_directory_new(path: &Path) -> Result<(), VaultBackendError> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = fs::DirBuilder::new();
    builder.mode(PRIVATE_DIRECTORY_MODE);
    builder
        .create(path)
        .map_err(|error| VaultBackendError::Io(error.kind()))?;
    verify_private_directory(path)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_private_directory_new(_path: &Path) -> Result<(), VaultBackendError> {
    Err(VaultBackendError::PlatformPermissionsUnavailable)
}

#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), VaultBackendError> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE_MODE)
        .open(path)
        .map_err(|error| VaultBackendError::Io(error.kind()))?;
    file.write_all(bytes)
        .map_err(|error| VaultBackendError::Io(error.kind()))?;
    file.sync_all()
        .map_err(|error| VaultBackendError::Io(error.kind()))?;
    verify_private_file_metadata(
        &file
            .metadata()
            .map_err(|error| VaultBackendError::Io(error.kind()))?,
    )?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_file(_path: &Path, _bytes: &[u8]) -> Result<(), VaultBackendError> {
    Err(VaultBackendError::PlatformPermissionsUnavailable)
}

#[cfg(unix)]
fn verify_directory_permissions(metadata: &Metadata) -> Result<(), VaultBackendError> {
    use std::os::unix::fs::PermissionsExt;

    if metadata.permissions().mode() & 0o777 != PRIVATE_DIRECTORY_MODE {
        return Err(VaultBackendError::InsecurePermissions);
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_directory_permissions(_metadata: &Metadata) -> Result<(), VaultBackendError> {
    Err(VaultBackendError::PlatformPermissionsUnavailable)
}

#[cfg(unix)]
fn verify_private_file_metadata(metadata: &Metadata) -> Result<(), VaultBackendError> {
    use std::os::unix::fs::PermissionsExt;

    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(VaultBackendError::UnsafeFileType);
    }
    if metadata.permissions().mode() & 0o777 != PRIVATE_FILE_MODE {
        return Err(VaultBackendError::InsecurePermissions);
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_file_metadata(_metadata: &Metadata) -> Result<(), VaultBackendError> {
    Err(VaultBackendError::PlatformPermissionsUnavailable)
}

fn sync_directory(path: &Path) -> Result<(), VaultBackendError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| VaultBackendError::Io(error.kind()))
}

#[cfg(unix)]
fn same_file_identity(left: &Metadata, right: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file_identity(left: &Metadata, right: &Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

struct StagingDirectoryGuard<'a> {
    staging_root: &'a Path,
    path: PathBuf,
    armed: bool,
}

impl<'a> StagingDirectoryGuard<'a> {
    fn new(staging_root: &'a Path, path: PathBuf) -> Self {
        Self {
            staging_root,
            path,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingDirectoryGuard<'_> {
    fn drop(&mut self) {
        if !self.armed
            || self.path.parent() != Some(self.staging_root)
            || !self
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".staging-"))
        {
            return;
        }
        if let Ok(metadata) = fs::symlink_metadata(&self.path)
            && metadata.is_dir()
            && !metadata.file_type().is_symlink()
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink};
    use std::sync::Arc;

    use super::*;

    const RECORD_UUID: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73321";
    const RECORD_UUID_2: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73322";
    const IDENTITY_CANARY: &str = "person+vault@example.invalid";
    const TOKEN_CANARY: &str = "fixture-secret-access-token";
    const CONFIG_CANARY: &str = "fixture-secret-api-key";
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
                "codex-capacity-vault-test-{}-{epoch_nanos}-{sequence}",
                std::process::id()
            ));
            assert!(!path.exists(), "unique test root must not exist");
            Self { path }
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let safe_name = self
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("codex-capacity-vault-test-"));
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

    fn record_ref(uuid: &str) -> VaultRecordRef {
        VaultRecordRef::parse(format!("vault-record:v1:{uuid}"))
            .expect("valid vault record reference")
    }

    fn protected_record(with_config: bool) -> ProtectedVaultRecord {
        ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_account_id",
            IDENTITY_CANARY.as_bytes().to_vec(),
            format!(r#"{{"auth_mode":"chatgpt","access_token":"{TOKEN_CANARY}"}}"#).into_bytes(),
            with_config.then(|| {
                format!("model_provider = \"fixture\"\napi_key = \"{CONFIG_CANARY}\"\n")
                    .into_bytes()
            }),
        )
        .expect("valid synthetic protected record")
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    }

    fn live_record_path(vault: &FileVault, uuid: &str) -> PathBuf {
        vault.records_root.join(uuid)
    }

    #[test]
    fn private_record_round_trips_with_fixed_layout_and_permissions() {
        let root = TestRoot::new();
        let vault = FileVault::open(&root.path).expect("open file vault");
        let reference = record_ref(RECORD_UUID);
        let record = protected_record(true);

        vault
            .create_record(&reference, &record)
            .expect("create protected record");
        assert_eq!(
            vault.record_state(&reference).unwrap(),
            VaultRecordState::Present
        );
        assert_eq!(vault.staging_residue_count().unwrap(), 0);

        for directory in [
            &vault.root,
            &vault.records_root,
            &vault.quarantine_root,
            &vault.staging_root,
            &live_record_path(&vault, RECORD_UUID),
        ] {
            assert_eq!(mode(directory), PRIVATE_DIRECTORY_MODE, "{directory:?}");
        }
        let record_path = live_record_path(&vault, RECORD_UUID);
        let entries: BTreeSet<String> = fs::read_dir(&record_path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(
            entries,
            BTreeSet::from([
                ADAPTER_NAMESPACE_FILE.into(),
                AUTH_FILE.into(),
                CONFIG_FILE.into(),
                IDENTITY_KIND_FILE.into(),
                IDENTITY_VALUE_FILE.into(),
                RECORD_FORMAT_FILE.into(),
            ])
        );
        for entry in &entries {
            assert_eq!(mode(&record_path.join(entry)), PRIVATE_FILE_MODE);
            assert!(!entry.contains(IDENTITY_CANARY));
            assert!(!entry.contains(TOKEN_CANARY));
            assert!(!entry.contains(CONFIG_CANARY));
        }

        let loaded = vault
            .read_record(&reference)
            .expect("read protected record");
        assert_eq!(loaded.adapter_namespace(), "official_codex:v1");
        assert_eq!(loaded.identity_kind(), "upstream_account_id");
        assert_eq!(loaded.upstream_identity(), IDENTITY_CANARY.as_bytes());
        assert!(
            std::str::from_utf8(loaded.auth_json())
                .unwrap()
                .contains(TOKEN_CANARY)
        );
        assert!(
            std::str::from_utf8(loaded.config_toml().expect("config"))
                .unwrap()
                .contains(CONFIG_CANARY)
        );
    }

    #[test]
    fn secret_debug_and_errors_do_not_expose_payload_canaries() {
        let root = TestRoot::new();
        let vault = FileVault::open(&root.path).expect("open file vault");
        let record = protected_record(true);
        let root_path = root.path.to_string_lossy();
        for rendered in [
            format!("{vault:?}"),
            format!("{record:?}"),
            format!("{:?}", record.upstream_identity),
            VaultBackendError::RecordCorrupt.to_string(),
            format!(
                "{:?}",
                VaultBackendError::Io(std::io::ErrorKind::PermissionDenied)
            ),
        ] {
            assert!(!rendered.contains(IDENTITY_CANARY));
            assert!(!rendered.contains(TOKEN_CANARY));
            assert!(!rendered.contains(CONFIG_CANARY));
            assert!(!rendered.contains(root_path.as_ref()));
        }
    }

    #[test]
    fn broad_or_relative_vault_roots_are_rejected_before_creation() {
        for path in [
            Path::new("relative-vault"),
            Path::new("/"),
            Path::new("/tmp"),
            Path::new("/tmp/./vault"),
            Path::new("/tmp/parent/../vault"),
        ] {
            assert_eq!(
                validate_root_path(path),
                Err(VaultBackendError::InvalidRoot)
            );
        }
    }

    #[test]
    fn payload_validation_is_bounded_and_preserves_exact_identity() {
        assert!(matches!(
            ProtectedVaultRecord::new(
                "OfficialCodex",
                "upstream_account_id",
                b"account-1".to_vec(),
                b"{}".to_vec(),
                None
            ),
            Err(VaultBackendError::InvalidPayload("adapter_namespace"))
        ));
        assert!(matches!(
            ProtectedVaultRecord::new(
                "official_codex:v1",
                "upstream_account_id",
                b" account-1 ".to_vec(),
                b"{}".to_vec(),
                None
            ),
            Err(VaultBackendError::InvalidPayload("upstream_identity"))
        ));
        assert!(matches!(
            ProtectedVaultRecord::new(
                "official_codex:v1",
                "upstream_account_id",
                b"account-1".to_vec(),
                vec![b'x'; MAX_AUTH_BYTES as usize + 1],
                None
            ),
            Err(VaultBackendError::LimitExceeded("auth_json"))
        ));
    }

    #[test]
    fn duplicate_create_never_overwrites_the_first_record() {
        let root = TestRoot::new();
        let vault = FileVault::open(&root.path).expect("open file vault");
        let reference = record_ref(RECORD_UUID);
        vault
            .create_record(&reference, &protected_record(true))
            .expect("create first record");
        let replacement = ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_account_id",
            b"replacement-account".to_vec(),
            b"replacement-auth".to_vec(),
            None,
        )
        .unwrap();

        assert_eq!(
            vault.create_record(&reference, &replacement),
            Err(VaultBackendError::RecordAlreadyExists)
        );
        let loaded = vault.read_record(&reference).unwrap();
        assert_eq!(loaded.upstream_identity(), IDENTITY_CANARY.as_bytes());
        assert!(
            std::str::from_utf8(loaded.auth_json())
                .unwrap()
                .contains(TOKEN_CANARY)
        );
    }

    #[test]
    fn concurrent_create_has_one_winner_and_no_staging_residue() {
        let root = TestRoot::new();
        let vault = Arc::new(FileVault::open(&root.path).expect("open file vault"));
        let reference = record_ref(RECORD_UUID);
        let mut workers = Vec::new();
        for _ in 0..2 {
            let vault = Arc::clone(&vault);
            let reference = reference.clone();
            workers.push(std::thread::spawn(move || {
                vault.create_record(&reference, &protected_record(true))
            }));
        }
        let outcomes: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker"))
            .collect();
        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| { **outcome == Err(VaultBackendError::RecordAlreadyExists) })
                .count(),
            1
        );
        assert_eq!(vault.staging_residue_count().unwrap(), 0);
        assert!(vault.read_record(&reference).is_ok());
    }

    #[test]
    fn quarantine_and_restore_are_recoverable_and_idempotent() {
        let root = TestRoot::new();
        let vault = FileVault::open(&root.path).expect("open file vault");
        let reference = record_ref(RECORD_UUID);
        vault
            .create_record(&reference, &protected_record(false))
            .expect("create record");

        assert_eq!(
            vault.quarantine_record(&reference).unwrap(),
            VaultTransitionOutcome::Moved
        );
        assert_eq!(
            vault.record_state(&reference).unwrap(),
            VaultRecordState::Quarantined
        );
        assert!(matches!(
            vault.read_record(&reference),
            Err(VaultBackendError::RecordQuarantined)
        ));
        assert_eq!(
            vault.quarantine_record(&reference).unwrap(),
            VaultTransitionOutcome::AlreadyAtDestination
        );
        assert_eq!(
            vault.restore_record(&reference).unwrap(),
            VaultTransitionOutcome::Moved
        );
        assert_eq!(
            vault.restore_record(&reference).unwrap(),
            VaultTransitionOutcome::AlreadyAtDestination
        );
        assert!(
            vault
                .read_record(&reference)
                .unwrap()
                .config_toml()
                .is_none()
        );
    }

    #[test]
    fn permission_drift_fails_closed_for_roots_and_files() {
        let root = TestRoot::new();
        let vault = FileVault::open(&root.path).expect("open file vault");
        let reference = record_ref(RECORD_UUID);
        vault
            .create_record(&reference, &protected_record(true))
            .expect("create record");
        let auth_path = live_record_path(&vault, RECORD_UUID).join(AUTH_FILE);
        fs::set_permissions(&auth_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            vault.read_record(&reference),
            Err(VaultBackendError::InsecurePermissions)
        ));

        fs::set_permissions(&auth_path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&vault.root, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            vault.record_state(&reference),
            Err(VaultBackendError::InsecurePermissions)
        );
    }

    #[test]
    fn symlinked_secret_file_is_rejected_without_returning_target_bytes() {
        let root = TestRoot::new();
        let vault = FileVault::open(&root.path).expect("open file vault");
        let reference = record_ref(RECORD_UUID);
        vault
            .create_record(&reference, &protected_record(true))
            .expect("create record");

        let outside = root.path.with_extension("outside-secret");
        fs::write(&outside, b"outside-secret-canary").unwrap();
        let auth_path = live_record_path(&vault, RECORD_UUID).join(AUTH_FILE);
        fs::remove_file(&auth_path).unwrap();
        symlink(&outside, &auth_path).unwrap();

        let error = vault
            .read_record(&reference)
            .expect_err("symlink must fail");
        assert_eq!(error, VaultBackendError::UnsafeFileType);
        assert!(!format!("{error:?}").contains("outside-secret-canary"));
        fs::remove_file(&outside).unwrap();
    }

    #[test]
    fn unknown_missing_and_oversized_record_files_fail_closed() {
        let root = TestRoot::new();
        let vault = FileVault::open(&root.path).expect("open file vault");
        let first = record_ref(RECORD_UUID);
        vault
            .create_record(&first, &protected_record(true))
            .expect("create record");
        let first_path = live_record_path(&vault, RECORD_UUID);
        write_private_file(&first_path.join("unexpected-secret-copy"), b"canary").unwrap();
        assert!(matches!(
            vault.read_record(&first),
            Err(VaultBackendError::UnexpectedVaultEntry)
        ));

        let second = record_ref(RECORD_UUID_2);
        vault
            .create_record(&second, &protected_record(true))
            .expect("create second record");
        let second_path = live_record_path(&vault, RECORD_UUID_2);
        fs::remove_file(second_path.join(IDENTITY_KIND_FILE)).unwrap();
        assert!(matches!(
            vault.read_record(&second),
            Err(VaultBackendError::RecordCorrupt)
        ));

        fs::remove_file(second_path.join(AUTH_FILE)).unwrap();
        let auth = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(PRIVATE_FILE_MODE)
            .open(second_path.join(AUTH_FILE))
            .unwrap();
        auth.set_len(MAX_AUTH_BYTES + 1).unwrap();
        assert!(matches!(
            read_private_file(&second_path.join(AUTH_FILE), MAX_AUTH_BYTES),
            Err(VaultBackendError::LimitExceeded("record_file"))
        ));

        fs::write(second_path.join(RECORD_FORMAT_FILE), b"unknown-format\n").unwrap();
        assert!(matches!(
            vault.read_record(&second),
            Err(VaultBackendError::RecordCorrupt)
        ));
    }

    #[test]
    fn staging_residue_is_reported_but_never_auto_deleted() {
        let root = TestRoot::new();
        let vault = FileVault::open(&root.path).expect("open file vault");
        let residue = vault.staging_root.join(".staging-crash-residue");
        let mut builder = fs::DirBuilder::new();
        builder.mode(PRIVATE_DIRECTORY_MODE);
        builder.create(&residue).unwrap();

        assert_eq!(vault.staging_residue_count().unwrap(), 1);
        assert!(residue.exists(), "inspection must not delete crash residue");

        let unexpected = vault.staging_root.join("untrusted-entry");
        let mut builder = fs::DirBuilder::new();
        builder.mode(PRIVATE_DIRECTORY_MODE);
        builder.create(&unexpected).unwrap();
        assert_eq!(
            vault.staging_residue_count(),
            Err(VaultBackendError::UnexpectedVaultEntry)
        );
        fs::remove_dir(&unexpected).unwrap();

        for index in 1..=MAX_STAGING_RESIDUES {
            let bounded_residue = vault.staging_root.join(format!(".staging-bound-{index}"));
            let mut builder = fs::DirBuilder::new();
            builder.mode(PRIVATE_DIRECTORY_MODE);
            builder.create(bounded_residue).unwrap();
        }
        assert_eq!(
            vault.staging_residue_count(),
            Err(VaultBackendError::BoundExceeded("staging_residue_count"))
        );
        assert!(
            residue.exists(),
            "bounded inspection must not delete residue"
        );
    }

    #[test]
    fn inventory_is_bounded_deterministic_and_identifier_redacted() {
        let root = TestRoot::new();
        let vault = FileVault::open(&root.path).expect("open file vault");
        let live = record_ref(RECORD_UUID);
        let quarantined = record_ref(RECORD_UUID_2);
        vault
            .create_record(&live, &protected_record(false))
            .expect("create live record");
        vault
            .create_record(&quarantined, &protected_record(true))
            .expect("create quarantined record");
        vault
            .quarantine_record(&quarantined)
            .expect("quarantine record");

        let residue_name = format!(".staging-{RECORD_UUID}-1234-1788070000000000000-7");
        let residue_path = vault.staging_root.join(&residue_name);
        let mut builder = fs::DirBuilder::new();
        builder.mode(PRIVATE_DIRECTORY_MODE);
        builder.create(&residue_path).expect("create residue");

        let inventory = vault.inventory(3).expect("inventory");
        assert_eq!(
            inventory.records,
            vec![
                VaultInventoryRecord {
                    record_ref: live,
                    state: VaultRecordState::Present,
                },
                VaultInventoryRecord {
                    record_ref: quarantined,
                    state: VaultRecordState::Quarantined,
                },
            ]
        );
        assert_eq!(inventory.staging_residues.len(), 1);
        assert_eq!(
            inventory.staging_residues[0].residue_id.as_str(),
            residue_name
        );
        assert!(residue_path.exists(), "inventory must never remove residue");

        let rendered = format!("{inventory:?}");
        for canary in [RECORD_UUID, RECORD_UUID_2, residue_name.as_str()] {
            assert!(!rendered.contains(canary));
        }
        assert!(rendered.contains("<redacted>"));
        assert_eq!(
            vault.inventory(2),
            Err(VaultBackendError::BoundExceeded("inventory_items"))
        );
        assert_eq!(
            vault.inventory(0),
            Err(VaultBackendError::InvalidInventoryLimit)
        );
        assert_eq!(
            vault.inventory(MAX_VAULT_INVENTORY_ITEMS + 1),
            Err(VaultBackendError::InvalidInventoryLimit)
        );
    }

    #[test]
    fn inventory_rejects_unknown_names_and_unsafe_entry_types() {
        let unknown_root = TestRoot::new();
        let unknown_vault = FileVault::open(&unknown_root.path).expect("open vault");
        let unknown = unknown_vault.records_root.join("not-a-record-id");
        let mut builder = fs::DirBuilder::new();
        builder.mode(PRIVATE_DIRECTORY_MODE);
        builder.create(&unknown).expect("create unknown entry");
        assert_eq!(
            unknown_vault.inventory(16),
            Err(VaultBackendError::UnexpectedVaultEntry)
        );
        assert!(unknown.exists(), "failed inventory must not remove entries");

        let staging_root = TestRoot::new();
        let staging_vault = FileVault::open(&staging_root.path).expect("open vault");
        let noncanonical = staging_vault
            .staging_root
            .join(format!(".staging-{RECORD_UUID}-0123-1-0"));
        let mut builder = fs::DirBuilder::new();
        builder.mode(PRIVATE_DIRECTORY_MODE);
        builder.create(&noncanonical).expect("create residue");
        assert_eq!(
            staging_vault.inventory(16),
            Err(VaultBackendError::UnexpectedVaultEntry)
        );

        let symlink_root = TestRoot::new();
        let symlink_vault = FileVault::open(&symlink_root.path).expect("open vault");
        let outside = symlink_root.path.with_extension("outside");
        let mut builder = fs::DirBuilder::new();
        builder.mode(PRIVATE_DIRECTORY_MODE);
        builder.create(&outside).expect("create outside");
        symlink(&outside, symlink_vault.records_root.join(RECORD_UUID)).expect("symlink");
        assert_eq!(
            symlink_vault.inventory(16),
            Err(VaultBackendError::UnsafeFileType)
        );
        fs::remove_dir(&outside).expect("remove outside");
    }

    #[test]
    fn live_and_quarantined_directory_conflict_fails_closed() {
        let root = TestRoot::new();
        let vault = FileVault::open(&root.path).expect("open file vault");
        let reference = record_ref(RECORD_UUID);
        vault
            .create_record(&reference, &protected_record(true))
            .expect("create record");
        vault.quarantine_record(&reference).unwrap();

        let live = live_record_path(&vault, RECORD_UUID);
        let mut builder = fs::DirBuilder::new();
        builder.mode(PRIVATE_DIRECTORY_MODE);
        builder.create(&live).unwrap();
        assert_eq!(
            vault.record_state(&reference).unwrap(),
            VaultRecordState::Conflict
        );
        assert_eq!(
            vault.restore_record(&reference),
            Err(VaultBackendError::RecordStateConflict)
        );
    }
}
