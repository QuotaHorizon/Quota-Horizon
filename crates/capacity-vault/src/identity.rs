use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use capacity_domain::{AccountFingerprint, VaultRecordRef};

use crate::{
    ProtectedVaultRecord, SecretBytes, VaultBackendError, prepare_private_directory,
    read_private_file, sync_directory, validate_root_path, verify_private_directory,
    verify_private_file_metadata, write_private_file,
};

mod key_ring;
#[cfg(target_os = "macos")]
mod macos_keychain;

pub use key_ring::{
    InstallationKeyRingRecord, InstallationKeyRingRevision, InstallationKeyStorageVersion,
};
#[cfg(target_os = "macos")]
pub use macos_keychain::MacOsKeychainInstallationKeyStore;

use key_ring::{MAX_KEY_RING_RECORD_BYTES, encode_key_ring_record, parse_key_ring_record};

pub const INSTALLATION_SECRET_BYTES: usize = 32;
pub const ACCOUNT_BINDING_DOMAIN: &str = "codex-capacity-planner/account-binding/v1";
pub const RECORD_BINDING_DOMAIN: &str = "codex-capacity-planner/protected-record/v1";
pub const RECORD_AUTHENTICATION_PREFIX: &str = "record-hmac-sha256:v1:";

const INSTALLATION_KEY_FORMAT: &[u8] = b"capacity-installation-key-v1\n";
const ACTIVE_KEY_FILE: &str = "active-installation-key-v1";
const KEY_STAGING_PREFIX: &str = ".installation-key-staging-";
const KEY_ID_BYTES: usize = 36;
const KEY_FILE_BYTES: u64 =
    (INSTALLATION_KEY_FORMAT.len() + KEY_ID_BYTES + 1 + INSTALLATION_SECRET_BYTES) as u64;
const MAX_KEY_NAMESPACE_ENTRIES: usize = 64;
const MAX_STAGING_ATTEMPTS: u64 = 16;

static KEY_STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One installation-scoped HMAC key. Secret bytes are neither clonable nor
/// serializable and are redacted from Debug output.
pub struct InstallationKey {
    key_id: String,
    secret: SecretBytes,
}

impl InstallationKey {
    pub fn from_parts(
        key_id: impl Into<String>,
        secret: [u8; INSTALLATION_SECRET_BYTES],
    ) -> Result<Self, IdentityError> {
        let key_id = key_id.into();
        if !is_lowercase_uuid(&key_id) {
            return Err(IdentityError::InvalidKeyId);
        }
        if secret.iter().all(|byte| *byte == 0) {
            return Err(IdentityError::InvalidSecret);
        }
        Ok(Self {
            key_id,
            secret: SecretBytes::new(secret.to_vec()),
        })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    fn secret(&self) -> &[u8] {
        self.secret.expose_secret()
    }

    fn same_material(&self, other: &Self) -> bool {
        self.key_id == other.key_id && constant_time_eq(self.secret(), other.secret())
    }

    pub fn generate() -> Result<Self, IdentityError> {
        generate_installation_key(&mut OsSecureRandom)
    }

    /// Generates fresh secret material for an already-journaled identifier.
    /// This is used only when recovery proves that the prepared rotation never
    /// reached key-ring storage. The identifier is non-secret, while the new
    /// secret remains non-cloneable and is sourced from the operating system.
    pub fn generate_for_id(key_id: &str) -> Result<Self, IdentityError> {
        if !is_lowercase_uuid(key_id) {
            return Err(IdentityError::InvalidKeyId);
        }
        let mut secret = [0_u8; INSTALLATION_SECRET_BYTES];
        OsSecureRandom.fill(&mut secret)?;
        if secret.iter().all(|byte| *byte == 0) {
            secret.fill(0);
            return Err(IdentityError::RandomSourceRejected);
        }
        let result = Self::from_parts(key_id, secret);
        secret.fill(0);
        result
    }
}

impl fmt::Debug for InstallationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InstallationKey(<redacted>)")
    }
}

/// Active key plus bounded verification-only predecessors. New fingerprints
/// and record tags always use the active key; predecessors exist solely to
/// make an explicit, two-key rotation window possible.
pub struct InstallationKeyRing {
    active: InstallationKey,
    verification_keys: Vec<InstallationKey>,
}

impl InstallationKeyRing {
    pub const MAX_VERIFICATION_KEYS: usize = 8;

    pub fn new(active: InstallationKey) -> Self {
        Self {
            active,
            verification_keys: Vec::new(),
        }
    }

    pub fn with_verification_keys(
        active: InstallationKey,
        verification_keys: Vec<InstallationKey>,
    ) -> Result<Self, IdentityError> {
        if verification_keys.len() > Self::MAX_VERIFICATION_KEYS
            || verification_keys
                .iter()
                .any(|key| key.key_id() == active.key_id())
        {
            return Err(IdentityError::InvalidKeyRing);
        }
        for (index, key) in verification_keys.iter().enumerate() {
            if verification_keys[index + 1..]
                .iter()
                .any(|candidate| candidate.key_id() == key.key_id())
            {
                return Err(IdentityError::InvalidKeyRing);
            }
        }
        Ok(Self {
            active,
            verification_keys,
        })
    }

    pub fn active_key_id(&self) -> &str {
        self.active.key_id()
    }

    pub fn verification_key_ids(&self) -> impl ExactSizeIterator<Item = &str> {
        self.verification_keys.iter().map(InstallationKey::key_id)
    }

    pub fn contains_key_id(&self, key_id: &str) -> bool {
        self.key_by_id(key_id).is_some()
    }

    pub fn account_fingerprint(
        &self,
        record: &ProtectedVaultRecord,
    ) -> Result<AccountFingerprint, IdentityError> {
        derive_account_fingerprint(&self.active, record)
    }

    /// Derives the same stable account binding with one exact member of this
    /// ring. Rotation rollback needs the predecessor without ever exporting
    /// secret material from the ring.
    pub fn account_fingerprint_with_key_id(
        &self,
        record: &ProtectedVaultRecord,
        key_id: &str,
    ) -> Result<AccountFingerprint, IdentityError> {
        let key = self.key_by_id(key_id).ok_or(IdentityError::UnknownKeyId)?;
        derive_account_fingerprint(key, record)
    }

    pub fn authenticate_record(
        &self,
        record_ref: &VaultRecordRef,
        record: &ProtectedVaultRecord,
    ) -> RecordAuthenticationTag {
        derive_record_authentication(&self.active, record_ref, record)
    }

    pub fn verify_record(
        &self,
        record_ref: &VaultRecordRef,
        record: &ProtectedVaultRecord,
        tag: &RecordAuthenticationTag,
    ) -> Result<(), IdentityError> {
        let key = self
            .key_by_id(tag.key_id())
            .ok_or(IdentityError::UnknownKeyId)?;
        let expected = derive_record_authentication(key, record_ref, record);
        if constant_time_eq(expected.digest(), tag.digest()) {
            Ok(())
        } else {
            Err(IdentityError::RecordAuthenticationFailed)
        }
    }

    fn key_by_id(&self, key_id: &str) -> Option<&InstallationKey> {
        if self.active.key_id() == key_id {
            return Some(&self.active);
        }
        self.verification_keys
            .iter()
            .find(|key| key.key_id() == key_id)
    }

    fn start_rotation(self, new_active: InstallationKey) -> Result<Self, IdentityError> {
        if self.verification_keys.len() >= Self::MAX_VERIFICATION_KEYS
            || self.key_by_id(new_active.key_id()).is_some()
        {
            return Err(IdentityError::InvalidKeyRing);
        }
        let mut verification_keys = Vec::with_capacity(self.verification_keys.len() + 1);
        verification_keys.push(self.active);
        verification_keys.extend(self.verification_keys);
        Self::with_verification_keys(new_active, verification_keys)
    }

    fn retire_verification_key(self, key_id: &str) -> Result<Self, IdentityError> {
        if self.active.key_id() == key_id
            || !self
                .verification_keys
                .iter()
                .any(|key| key.key_id() == key_id)
        {
            return Err(IdentityError::InvalidKeyRing);
        }
        let verification_keys = self
            .verification_keys
            .into_iter()
            .filter(|key| key.key_id() != key_id)
            .collect();
        Self::with_verification_keys(self.active, verification_keys)
    }

    fn promote_verification_key(self, key_id: &str) -> Result<Self, IdentityError> {
        if self.active.key_id() == key_id {
            return Err(IdentityError::InvalidKeyRing);
        }
        let mut promoted = None;
        let mut verification_keys = Vec::with_capacity(self.verification_keys.len());
        verification_keys.push(self.active);
        for key in self.verification_keys {
            if key.key_id() == key_id {
                promoted = Some(key);
            } else {
                verification_keys.push(key);
            }
        }
        let promoted = promoted.ok_or(IdentityError::InvalidKeyRing)?;
        Self::with_verification_keys(promoted, verification_keys)
    }

    fn same_material(&self, other: &Self) -> bool {
        self.active.same_material(&other.active)
            && self.verification_keys.len() == other.verification_keys.len()
            && self
                .verification_keys
                .iter()
                .zip(&other.verification_keys)
                .all(|(left, right)| left.same_material(right))
    }

    fn is_valid_successor_of(&self, current: &Self) -> bool {
        self.is_started_rotation_of(current)
            || self.is_single_retirement_of(current)
            || self.is_promotion_of(current)
    }

    fn is_started_rotation_of(&self, current: &Self) -> bool {
        self.verification_keys.len() == current.verification_keys.len() + 1
            && self
                .verification_keys
                .first()
                .is_some_and(|key| key.same_material(&current.active))
            && same_key_sequence(&self.verification_keys[1..], &current.verification_keys)
    }

    fn is_single_retirement_of(&self, current: &Self) -> bool {
        self.active.same_material(&current.active)
            && current.verification_keys.len() == self.verification_keys.len() + 1
            && is_ordered_single_removal(&current.verification_keys, &self.verification_keys)
    }

    fn is_promotion_of(&self, current: &Self) -> bool {
        if self.verification_keys.len() != current.verification_keys.len()
            || !self
                .verification_keys
                .first()
                .is_some_and(|key| key.same_material(&current.active))
        {
            return false;
        }
        current
            .verification_keys
            .iter()
            .position(|key| self.active.same_material(key))
            .is_some_and(|promoted_index| {
                self.verification_keys[1..]
                    .iter()
                    .zip(
                        current
                            .verification_keys
                            .iter()
                            .enumerate()
                            .filter_map(|(index, key)| (index != promoted_index).then_some(key)),
                    )
                    .all(|(left, right)| left.same_material(right))
            })
    }
}

fn same_key_sequence(left: &[InstallationKey], right: &[InstallationKey]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.same_material(right))
}

fn is_ordered_single_removal(before: &[InstallationKey], after: &[InstallationKey]) -> bool {
    if before.len() != after.len() + 1 {
        return false;
    }
    (0..before.len()).any(|removed_index| {
        before
            .iter()
            .enumerate()
            .filter_map(|(index, key)| (index != removed_index).then_some(key))
            .zip(after)
            .all(|(left, right)| left.same_material(right))
    })
}

impl fmt::Debug for InstallationKeyRing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstallationKeyRing")
            .field("active", &"<redacted>")
            .field("verification_key_count", &self.verification_keys.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RecordAuthenticationTag {
    encoded: String,
    key_id: String,
    digest: [u8; 32],
}

impl RecordAuthenticationTag {
    pub fn parse(value: impl Into<String>) -> Result<Self, IdentityError> {
        let encoded = value.into();
        let payload = encoded
            .strip_prefix(RECORD_AUTHENTICATION_PREFIX)
            .ok_or(IdentityError::InvalidRecordAuthenticationTag)?;
        let (key_id, digest_hex) = payload
            .split_once(':')
            .ok_or(IdentityError::InvalidRecordAuthenticationTag)?;
        if !is_lowercase_uuid(key_id) || digest_hex.len() != 64 {
            return Err(IdentityError::InvalidRecordAuthenticationTag);
        }
        let digest =
            decode_lower_hex_32(digest_hex).ok_or(IdentityError::InvalidRecordAuthenticationTag)?;
        let key_id = key_id.to_owned();
        Ok(Self {
            encoded,
            key_id,
            digest,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.encoded
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}

impl fmt::Debug for RecordAuthenticationTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RecordAuthenticationTag(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    InvalidKeyId,
    InvalidSecret,
    InvalidKeyRing,
    InvalidRecordAuthenticationTag,
    UnknownKeyId,
    RecordAuthenticationFailed,
    KeyRecordCorrupt,
    KeyNamespaceInvalid,
    PlatformRandomUnavailable,
    PlatformCredentialUnavailable,
    PlatformCredentialMissingEntitlement,
    PlatformCredentialDenied,
    PlatformCredentialInteractionRequired,
    PlatformCredentialCancelled,
    PlatformCredentialCorrupt,
    PlatformCredentialFailed,
    RandomSourceRejected,
    ClockUnavailable,
    Storage(VaultBackendError),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidKeyId => "installation key identifier is invalid",
            Self::InvalidSecret => "installation key material is invalid",
            Self::InvalidKeyRing => "installation key ring is invalid",
            Self::InvalidRecordAuthenticationTag => {
                "protected-record authentication tag is invalid"
            }
            Self::UnknownKeyId => "required installation key is unavailable",
            Self::RecordAuthenticationFailed => "protected-record authentication failed",
            Self::KeyRecordCorrupt => "installation key record is corrupt",
            Self::KeyNamespaceInvalid => "installation key namespace is invalid",
            Self::PlatformRandomUnavailable => "operating-system random source is unavailable",
            Self::PlatformCredentialUnavailable => "platform credential storage is unavailable",
            Self::PlatformCredentialMissingEntitlement => {
                "platform credential storage requires a signing entitlement (-34018)"
            }
            Self::PlatformCredentialDenied => "platform credential access was denied",
            Self::PlatformCredentialInteractionRequired => {
                "platform credential access requires user interaction"
            }
            Self::PlatformCredentialCancelled => "platform credential access was cancelled",
            Self::PlatformCredentialCorrupt => "platform credential record is corrupt",
            Self::PlatformCredentialFailed => "platform credential storage operation failed",
            Self::RandomSourceRejected => {
                "operating-system random source returned invalid material"
            }
            Self::ClockUnavailable => "system clock is unavailable for key staging",
            Self::Storage(_) => "installation key storage operation failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for IdentityError {}

impl From<VaultBackendError> for IdentityError {
    fn from(error: VaultBackendError) -> Self {
        Self::Storage(error)
    }
}

/// Narrow seam for macOS Keychain, Windows Credential Manager, Linux secret
/// service, and the explicit owner-mode fallback. Implementations must keep
/// both success and failure surfaces free of secret material.
pub trait InstallationKeyStore {
    fn load(&mut self) -> Result<Option<InstallationKey>, IdentityError>;

    fn load_or_create(&mut self) -> Result<InstallationKey, IdentityError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallationKeyRingMutationOutcome {
    Applied,
    RevisionConflict,
    Missing,
}

/// Versioned key-ring persistence seam used by explicit rotation workflows.
/// Replace and delete require both the expected revision and active key ID;
/// callers must additionally hold the product-wide mutation lease.
pub trait InstallationKeyRingStore {
    fn load_key_ring(&mut self) -> Result<Option<InstallationKeyRingRecord>, IdentityError>;

    fn load_or_create_key_ring(&mut self) -> Result<InstallationKeyRingRecord, IdentityError>;

    fn replace_key_ring(
        &mut self,
        expected: &InstallationKeyRingRevision,
        replacement: &InstallationKeyRingRecord,
    ) -> Result<InstallationKeyRingMutationOutcome, IdentityError>;

    fn delete_key_ring(
        &mut self,
        expected: &InstallationKeyRingRevision,
    ) -> Result<InstallationKeyRingMutationOutcome, IdentityError>;
}

/// There is intentionally no "platform then fallback" request. A caller must
/// either require the platform credential store or explicitly supply a private
/// fallback root after informed user choice.
pub enum InstallationKeyBackendRequest {
    PlatformRequired,
    ExplicitFileFallback { root: PathBuf },
}

impl fmt::Debug for InstallationKeyBackendRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlatformRequired => formatter.write_str("PlatformRequired"),
            Self::ExplicitFileFallback { .. } => {
                formatter.write_str("ExplicitFileFallback { root: <redacted> }")
            }
        }
    }
}

pub enum SelectedInstallationKeyStore {
    #[cfg(target_os = "macos")]
    MacOsKeychain(MacOsKeychainInstallationKeyStore),
    ExplicitFileFallback(FileInstallationKeyStore),
}

impl fmt::Debug for SelectedInstallationKeyStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOsKeychain(_) => formatter.write_str("MacOsKeychain(<redacted>)"),
            Self::ExplicitFileFallback(_) => {
                formatter.write_str("ExplicitFileFallback(<redacted>)")
            }
        }
    }
}

impl InstallationKeyStore for SelectedInstallationKeyStore {
    fn load(&mut self) -> Result<Option<InstallationKey>, IdentityError> {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOsKeychain(store) => store.load(),
            Self::ExplicitFileFallback(store) => store.load(),
        }
    }

    fn load_or_create(&mut self) -> Result<InstallationKey, IdentityError> {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOsKeychain(store) => store.load_or_create(),
            Self::ExplicitFileFallback(store) => store.load_or_create(),
        }
    }
}

impl InstallationKeyRingStore for SelectedInstallationKeyStore {
    fn load_key_ring(&mut self) -> Result<Option<InstallationKeyRingRecord>, IdentityError> {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOsKeychain(store) => store.load_key_ring(),
            Self::ExplicitFileFallback(store) => store.load_key_ring(),
        }
    }

    fn load_or_create_key_ring(&mut self) -> Result<InstallationKeyRingRecord, IdentityError> {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOsKeychain(store) => store.load_or_create_key_ring(),
            Self::ExplicitFileFallback(store) => store.load_or_create_key_ring(),
        }
    }

    fn replace_key_ring(
        &mut self,
        expected: &InstallationKeyRingRevision,
        replacement: &InstallationKeyRingRecord,
    ) -> Result<InstallationKeyRingMutationOutcome, IdentityError> {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOsKeychain(store) => store.replace_key_ring(expected, replacement),
            Self::ExplicitFileFallback(store) => store.replace_key_ring(expected, replacement),
        }
    }

    fn delete_key_ring(
        &mut self,
        expected: &InstallationKeyRingRevision,
    ) -> Result<InstallationKeyRingMutationOutcome, IdentityError> {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOsKeychain(store) => store.delete_key_ring(expected),
            Self::ExplicitFileFallback(store) => store.delete_key_ring(expected),
        }
    }
}

pub fn select_installation_key_store(
    request: InstallationKeyBackendRequest,
) -> Result<SelectedInstallationKeyStore, IdentityError> {
    match request {
        InstallationKeyBackendRequest::PlatformRequired => {
            #[cfg(target_os = "macos")]
            {
                Ok(SelectedInstallationKeyStore::MacOsKeychain(
                    MacOsKeychainInstallationKeyStore::new(),
                ))
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(IdentityError::PlatformCredentialUnavailable)
            }
        }
        InstallationKeyBackendRequest::ExplicitFileFallback { root } => {
            Ok(SelectedInstallationKeyStore::ExplicitFileFallback(
                FileInstallationKeyStore::open_explicit_fallback(root)?,
            ))
        }
    }
}

/// Explicitly selected Unix owner-mode fallback for the installation key.
/// Calling this adapter is the opt-in; desktop code must not silently choose it
/// when a platform credential store fails.
pub struct FileInstallationKeyStore {
    root: PathBuf,
    active_key_path: PathBuf,
}

impl FileInstallationKeyStore {
    pub fn open_explicit_fallback(root: impl AsRef<Path>) -> Result<Self, IdentityError> {
        let root = root.as_ref().to_path_buf();

        #[cfg(not(unix))]
        {
            let _ = root;
            return Err(IdentityError::Storage(
                VaultBackendError::PlatformPermissionsUnavailable,
            ));
        }

        #[cfg(unix)]
        {
            validate_root_path(&root)?;
            prepare_private_directory(&root)?;
            let store = Self {
                active_key_path: root.join(ACTIVE_KEY_FILE),
                root,
            };
            store.verify_namespace()?;
            Ok(store)
        }
    }

    pub fn load(&self) -> Result<Option<InstallationKey>, IdentityError> {
        self.load_key_ring()
            .map(|record| record.map(InstallationKeyRingRecord::into_active_key))
    }

    pub fn load_key_ring(&self) -> Result<Option<InstallationKeyRingRecord>, IdentityError> {
        self.verify_namespace()?;
        match fs::symlink_metadata(&self.active_key_path) {
            Ok(_) => load_key_ring_at(&self.active_key_path).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(IdentityError::Storage(VaultBackendError::Io(error.kind()))),
        }
    }

    pub fn load_or_create(&self) -> Result<InstallationKey, IdentityError> {
        self.load_or_create_key_ring()
            .map(InstallationKeyRingRecord::into_active_key)
    }

    pub fn load_or_create_key_ring(&self) -> Result<InstallationKeyRingRecord, IdentityError> {
        let mut random = OsSecureRandom;
        self.load_or_create_key_ring_with(&mut random)
    }

    pub fn replace_key_ring(
        &self,
        expected: &InstallationKeyRingRevision,
        replacement: &InstallationKeyRingRecord,
    ) -> Result<InstallationKeyRingMutationOutcome, IdentityError> {
        if !replacement.is_next_revision_of(expected) {
            return Err(IdentityError::InvalidKeyRing);
        }
        self.verify_namespace()?;
        let Some(current) = self.load_key_ring()? else {
            return Ok(InstallationKeyRingMutationOutcome::Missing);
        };
        if !current.matches_revision(expected) {
            return Ok(InstallationKeyRingMutationOutcome::RevisionConflict);
        }
        if !replacement.is_valid_successor_of(&current) {
            return Err(IdentityError::InvalidKeyRing);
        }

        let staging_path = self.allocate_staging_path()?;
        let mut guard = KeyStagingGuard::new(&self.root, staging_path.clone());
        let mut encoded = encode_key_ring_record(replacement);
        let write_result = write_private_file(&staging_path, &encoded);
        encoded.fill(0);
        write_result?;
        let staged = load_key_ring_at(&staging_path)?;
        if !replacement.same_material(&staged) {
            return Err(IdentityError::KeyRecordCorrupt);
        }

        let Some(observed) = self.load_key_ring()? else {
            return Ok(InstallationKeyRingMutationOutcome::Missing);
        };
        if !observed.matches_revision(expected) || !observed.same_material(&current) {
            return Ok(InstallationKeyRingMutationOutcome::RevisionConflict);
        }
        fs::rename(&staging_path, &self.active_key_path)
            .map_err(|error| IdentityError::Storage(VaultBackendError::Io(error.kind())))?;
        guard.disarm();
        sync_directory(&self.root)?;
        let installed = load_key_ring_at(&self.active_key_path)?;
        if !replacement.same_material(&installed) {
            return Err(IdentityError::KeyRecordCorrupt);
        }
        self.verify_namespace()?;
        Ok(InstallationKeyRingMutationOutcome::Applied)
    }

    pub fn delete_key_ring(
        &self,
        expected: &InstallationKeyRingRevision,
    ) -> Result<InstallationKeyRingMutationOutcome, IdentityError> {
        self.verify_namespace()?;
        let Some(current) = self.load_key_ring()? else {
            return Ok(InstallationKeyRingMutationOutcome::Missing);
        };
        if !current.matches_revision(expected) {
            return Ok(InstallationKeyRingMutationOutcome::RevisionConflict);
        }
        fs::remove_file(&self.active_key_path)
            .map_err(|error| IdentityError::Storage(VaultBackendError::Io(error.kind())))?;
        sync_directory(&self.root)?;
        self.verify_namespace()?;
        if self.load_key_ring()?.is_some() {
            return Err(IdentityError::KeyRecordCorrupt);
        }
        Ok(InstallationKeyRingMutationOutcome::Applied)
    }

    pub fn staging_residue_count(&self) -> Result<u32, IdentityError> {
        self.verify_namespace()
    }

    fn load_or_create_key_ring_with<R: SecureRandom>(
        &self,
        random: &mut R,
    ) -> Result<InstallationKeyRingRecord, IdentityError> {
        if let Some(existing) = self.load_key_ring()? {
            return Ok(existing);
        }

        let generated = InstallationKeyRingRecord::initial(generate_installation_key(random)?);
        let staging_path = self.allocate_staging_path()?;
        let mut guard = KeyStagingGuard::new(&self.root, staging_path.clone());
        let mut encoded = encode_key_ring_record(&generated);
        let write_result = write_private_file(&staging_path, &encoded);
        encoded.fill(0);
        write_result?;

        let staged = load_key_ring_at(&staging_path)?;
        if !generated.same_material(&staged) {
            return Err(IdentityError::KeyRecordCorrupt);
        }
        drop(staged);

        match fs::hard_link(&staging_path, &self.active_key_path) {
            Ok(()) => {
                sync_directory(&self.root)?;
                let installed = load_key_ring_at(&self.active_key_path)?;
                if !generated.same_material(&installed) {
                    return Err(IdentityError::KeyRecordCorrupt);
                }
                guard.remove_now()?;
                sync_directory(&self.root)?;
                Ok(installed)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let installed = load_key_ring_at(&self.active_key_path)?;
                guard.remove_now()?;
                sync_directory(&self.root)?;
                Ok(installed)
            }
            Err(error) => Err(IdentityError::Storage(VaultBackendError::Io(error.kind()))),
        }
    }

    fn allocate_staging_path(&self) -> Result<PathBuf, IdentityError> {
        let epoch_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| IdentityError::ClockUnavailable)?
            .as_nanos();
        for _ in 0..MAX_STAGING_ATTEMPTS {
            let sequence = KEY_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = self.root.join(format!(
                "{KEY_STAGING_PREFIX}{}-{epoch_nanos}-{sequence}",
                std::process::id()
            ));
            if !path.exists() {
                return Ok(path);
            }
        }
        Err(IdentityError::KeyNamespaceInvalid)
    }

    fn verify_namespace(&self) -> Result<u32, IdentityError> {
        verify_private_directory(&self.root)?;
        let mut staging_count = 0_u32;
        let mut entry_count = 0_usize;
        for entry in fs::read_dir(&self.root)
            .map_err(|error| IdentityError::Storage(VaultBackendError::Io(error.kind())))?
        {
            let entry = entry
                .map_err(|error| IdentityError::Storage(VaultBackendError::Io(error.kind())))?;
            entry_count = entry_count
                .checked_add(1)
                .ok_or(IdentityError::KeyNamespaceInvalid)?;
            if entry_count > MAX_KEY_NAMESPACE_ENTRIES {
                return Err(IdentityError::KeyNamespaceInvalid);
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| IdentityError::KeyNamespaceInvalid)?;
            if name != ACTIVE_KEY_FILE && !valid_staging_name(&name) {
                return Err(IdentityError::KeyNamespaceInvalid);
            }
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| IdentityError::Storage(VaultBackendError::Io(error.kind())))?;
            verify_private_file_metadata(&metadata)?;
            if name.starts_with(KEY_STAGING_PREFIX) {
                staging_count = staging_count
                    .checked_add(1)
                    .ok_or(IdentityError::KeyNamespaceInvalid)?;
            }
        }
        Ok(staging_count)
    }
}

impl fmt::Debug for FileInstallationKeyStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileInstallationKeyStore")
            .field("root", &"<redacted>")
            .finish()
    }
}

impl InstallationKeyStore for FileInstallationKeyStore {
    fn load(&mut self) -> Result<Option<InstallationKey>, IdentityError> {
        FileInstallationKeyStore::load(self)
    }

    fn load_or_create(&mut self) -> Result<InstallationKey, IdentityError> {
        FileInstallationKeyStore::load_or_create(self)
    }
}

impl InstallationKeyRingStore for FileInstallationKeyStore {
    fn load_key_ring(&mut self) -> Result<Option<InstallationKeyRingRecord>, IdentityError> {
        FileInstallationKeyStore::load_key_ring(self)
    }

    fn load_or_create_key_ring(&mut self) -> Result<InstallationKeyRingRecord, IdentityError> {
        FileInstallationKeyStore::load_or_create_key_ring(self)
    }

    fn replace_key_ring(
        &mut self,
        expected: &InstallationKeyRingRevision,
        replacement: &InstallationKeyRingRecord,
    ) -> Result<InstallationKeyRingMutationOutcome, IdentityError> {
        FileInstallationKeyStore::replace_key_ring(self, expected, replacement)
    }

    fn delete_key_ring(
        &mut self,
        expected: &InstallationKeyRingRevision,
    ) -> Result<InstallationKeyRingMutationOutcome, IdentityError> {
        FileInstallationKeyStore::delete_key_ring(self, expected)
    }
}

trait SecureRandom {
    fn fill(&mut self, output: &mut [u8]) -> Result<(), IdentityError>;
}

struct OsSecureRandom;

impl SecureRandom for OsSecureRandom {
    fn fill(&mut self, output: &mut [u8]) -> Result<(), IdentityError> {
        #[cfg(unix)]
        {
            File::open("/dev/urandom")
                .and_then(|mut source| source.read_exact(output))
                .map_err(|_| IdentityError::PlatformRandomUnavailable)
        }

        #[cfg(not(unix))]
        {
            let _ = output;
            Err(IdentityError::PlatformRandomUnavailable)
        }
    }
}

pub fn derive_account_fingerprint(
    key: &InstallationKey,
    record: &ProtectedVaultRecord,
) -> Result<AccountFingerprint, IdentityError> {
    let mut message = Vec::with_capacity(
        ACCOUNT_BINDING_DOMAIN.len()
            + record.adapter_namespace().len()
            + record.identity_kind().len()
            + record.upstream_identity().len()
            + 3,
    );
    message.extend_from_slice(ACCOUNT_BINDING_DOMAIN.as_bytes());
    message.push(0);
    message.extend_from_slice(record.adapter_namespace().as_bytes());
    message.push(0);
    message.extend_from_slice(record.identity_kind().as_bytes());
    message.push(0);
    message.extend_from_slice(record.upstream_identity());
    let digest = hmac_sha256(key.secret(), &message);
    message.fill(0);
    AccountFingerprint::parse(format!(
        "hmac-sha256:v1:{}:{}",
        key.key_id(),
        encode_lower_hex(&digest)
    ))
    .map_err(|_| IdentityError::InvalidKeyId)
}

fn derive_record_authentication(
    key: &InstallationKey,
    record_ref: &VaultRecordRef,
    record: &ProtectedVaultRecord,
) -> RecordAuthenticationTag {
    let mut message = Vec::with_capacity(
        RECORD_BINDING_DOMAIN.len()
            + record_ref.as_str().len()
            + record.adapter_namespace().len()
            + record.identity_kind().len()
            + record.upstream_identity().len()
            + record.auth_json().len()
            + record.config_toml().map_or(0, <[u8]>::len)
            + 64,
    );
    append_framed(&mut message, RECORD_BINDING_DOMAIN.as_bytes());
    append_framed(&mut message, record_ref.as_str().as_bytes());
    append_framed(&mut message, record.adapter_namespace().as_bytes());
    append_framed(&mut message, record.identity_kind().as_bytes());
    append_framed(&mut message, record.upstream_identity());
    append_framed(&mut message, record.auth_json());
    match record.config_toml() {
        Some(config) => {
            message.push(1);
            append_framed(&mut message, config);
        }
        None => message.push(0),
    }
    let digest = hmac_sha256(key.secret(), &message);
    message.fill(0);
    let encoded = format!(
        "{RECORD_AUTHENTICATION_PREFIX}{}:{}",
        key.key_id(),
        encode_lower_hex(&digest)
    );
    RecordAuthenticationTag {
        encoded,
        key_id: key.key_id().to_owned(),
        digest,
    }
}

fn append_framed(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value);
}

fn generate_installation_key<R: SecureRandom>(
    random: &mut R,
) -> Result<InstallationKey, IdentityError> {
    let mut key_id_bytes = [0_u8; 16];
    let mut secret = [0_u8; INSTALLATION_SECRET_BYTES];
    random.fill(&mut key_id_bytes)?;
    random.fill(&mut secret)?;
    if secret.iter().all(|byte| *byte == 0) {
        return Err(IdentityError::RandomSourceRejected);
    }
    key_id_bytes[6] = (key_id_bytes[6] & 0x0f) | 0x40;
    key_id_bytes[8] = (key_id_bytes[8] & 0x3f) | 0x80;
    let key_id = format_uuid(key_id_bytes);
    let result = InstallationKey::from_parts(key_id, secret);
    secret.fill(0);
    result
}

#[cfg(test)]
fn encode_installation_key(key: &InstallationKey) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(KEY_FILE_BYTES as usize);
    bytes.extend_from_slice(INSTALLATION_KEY_FORMAT);
    bytes.extend_from_slice(key.key_id().as_bytes());
    bytes.push(b'\n');
    bytes.extend_from_slice(key.secret());
    bytes
}

fn load_key_ring_at(path: &Path) -> Result<InstallationKeyRingRecord, IdentityError> {
    let bytes = read_private_file(path, MAX_KEY_RING_RECORD_BYTES)?;
    parse_key_ring_record(bytes)
}

fn parse_installation_key(mut bytes: Vec<u8>) -> Result<InstallationKey, IdentityError> {
    let expected_length = KEY_FILE_BYTES as usize;
    let valid_prefix = bytes.len() == expected_length
        && bytes.starts_with(INSTALLATION_KEY_FORMAT)
        && bytes.get(INSTALLATION_KEY_FORMAT.len() + KEY_ID_BYTES) == Some(&b'\n');
    if !valid_prefix {
        bytes.fill(0);
        return Err(IdentityError::KeyRecordCorrupt);
    }
    let key_id_start = INSTALLATION_KEY_FORMAT.len();
    let key_id_end = key_id_start + KEY_ID_BYTES;
    let key_id = match std::str::from_utf8(&bytes[key_id_start..key_id_end]) {
        Ok(value) if is_lowercase_uuid(value) => value.to_owned(),
        _ => {
            bytes.fill(0);
            return Err(IdentityError::KeyRecordCorrupt);
        }
    };
    let mut secret = [0_u8; INSTALLATION_SECRET_BYTES];
    secret.copy_from_slice(&bytes[key_id_end + 1..]);
    bytes.fill(0);
    let result = InstallationKey::from_parts(key_id, secret);
    secret.fill(0);
    result.map_err(|_| IdentityError::KeyRecordCorrupt)
}

fn valid_staging_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(KEY_STAGING_PREFIX) else {
        return false;
    };
    let mut parts = rest.split('-');
    let Some(process_id) = parts.next() else {
        return false;
    };
    let Some(epoch_nanos) = parts.next() else {
        return false;
    };
    let Some(sequence) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && canonical_number(process_id, false)
        && canonical_number(epoch_nanos, false)
        && canonical_number(sequence, true)
}

fn canonical_number(value: &str, allow_zero: bool) -> bool {
    value
        .parse::<u128>()
        .ok()
        .filter(|number| allow_zero || *number > 0)
        .is_some_and(|number| number.to_string() == value)
}

struct KeyStagingGuard<'a> {
    root: &'a Path,
    path: PathBuf,
    armed: bool,
}

impl<'a> KeyStagingGuard<'a> {
    fn new(root: &'a Path, path: PathBuf) -> Self {
        Self {
            root,
            path,
            armed: true,
        }
    }

    fn remove_now(&mut self) -> Result<(), IdentityError> {
        if !self.armed {
            return Ok(());
        }
        fs::remove_file(&self.path)
            .map_err(|error| IdentityError::Storage(VaultBackendError::Io(error.kind())))?;
        self.armed = false;
        Ok(())
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for KeyStagingGuard<'_> {
    fn drop(&mut self) {
        if self.armed
            && self.path.parent() == Some(self.root)
            && self
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(valid_staging_name)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn is_lowercase_uuid(value: &str) -> bool {
    if value.len() != KEY_ID_BYTES {
        return false;
    }
    value.bytes().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => byte == b'-',
        _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
    })
}

fn format_uuid(bytes: [u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn encode_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn decode_lower_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_lower_hex_nibble(pair[0])?;
        let low = decode_lower_hex_nibble(pair[1])?;
        output[index] = (high << 4) | low;
    }
    Some(output)
}

fn decode_lower_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_BYTES: usize = 64;
    let mut normalized_key = [0_u8; BLOCK_BYTES];
    if key.len() > BLOCK_BYTES {
        normalized_key[..32].copy_from_slice(&sha256(key));
    } else {
        normalized_key[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = normalized_key;
    let mut outer_pad = normalized_key;
    for byte in &mut inner_pad {
        *byte ^= 0x36;
    }
    for byte in &mut outer_pad {
        *byte ^= 0x5c;
    }
    normalized_key.fill(0);

    let mut inner = Sha256::new();
    inner.update(&inner_pad);
    inner.update(message);
    let mut inner_digest = inner.finalize();
    inner_pad.fill(0);

    let mut outer = Sha256::new();
    outer.update(&outer_pad);
    outer.update(&inner_digest);
    let digest = outer.finalize();
    outer_pad.fill(0);
    inner_digest.fill(0);
    digest
}

fn sha256(message: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(message);
    hasher.finalize()
}

struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffer_len: usize,
    message_len: u64,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: [0; 64],
            buffer_len: 0,
            message_len: 0,
        }
    }

    fn update(&mut self, mut input: &[u8]) {
        self.message_len = self.message_len.wrapping_add(input.len() as u64);
        if self.buffer_len > 0 {
            let wanted = 64 - self.buffer_len;
            let copied = wanted.min(input.len());
            self.buffer[self.buffer_len..self.buffer_len + copied]
                .copy_from_slice(&input[..copied]);
            self.buffer_len += copied;
            input = &input[copied..];
            if self.buffer_len == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buffer.fill(0);
                self.buffer_len = 0;
            }
        }
        while input.len() >= 64 {
            let block: &[u8; 64] = input[..64].try_into().expect("fixed SHA-256 block");
            self.compress(block);
            input = &input[64..];
        }
        self.buffer[..input.len()].copy_from_slice(input);
        self.buffer_len = input.len();
    }

    fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.message_len.wrapping_mul(8);
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;
        if self.buffer_len > 56 {
            self.buffer[self.buffer_len..].fill(0);
            let block = self.buffer;
            self.compress(&block);
            self.buffer.fill(0);
            self.buffer_len = 0;
        }
        self.buffer[self.buffer_len..56].fill(0);
        self.buffer[56..].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buffer;
        self.compress(&block);
        self.buffer.fill(0);

        let mut digest = [0_u8; 32];
        for (chunk, word) in digest.chunks_exact_mut(4).zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        self.state.fill(0);
        digest
    }

    fn compress(&mut self, block: &[u8; 64]) {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut schedule = [0_u32; 64];
        for (index, chunk) in block.chunks_exact(4).enumerate() {
            schedule[index] = u32::from_be_bytes(chunk.try_into().expect("four-byte SHA word"));
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for index in 0..64 {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(sum1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(schedule[index]);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
        schedule.fill(0);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::Arc;

    use super::*;

    const KEY_ID: &str = "018f47a2-8a71-4f4a-9c35-1f4234a73501";
    const KEY_ID_2: &str = "018f47a2-8a71-4f4a-9c35-1f4234a73502";
    const IDENTITY_CANARY: &str = "identity-hmac@example.invalid";
    const TOKEN_CANARY: &str = "identity-hmac-secret-token";

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let sequence = KEY_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock")
                .as_nanos();
            Self(std::env::temp_dir().join(format!(
                "capacity-installation-key-test-{}-{nanos}-{sequence}",
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
                    .is_some_and(|name| name.starts_with("capacity-installation-key-test-"))
            {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }

    struct FixedRandom {
        next: u8,
    }

    impl SecureRandom for FixedRandom {
        fn fill(&mut self, output: &mut [u8]) -> Result<(), IdentityError> {
            for byte in output {
                *byte = self.next;
                self.next = self.next.wrapping_add(1);
            }
            Ok(())
        }
    }

    struct ZeroRandom;

    impl SecureRandom for ZeroRandom {
        fn fill(&mut self, output: &mut [u8]) -> Result<(), IdentityError> {
            output.fill(0);
            Ok(())
        }
    }

    fn key(id: &str, byte: u8) -> InstallationKey {
        InstallationKey::from_parts(id, [byte; INSTALLATION_SECRET_BYTES]).expect("key")
    }

    fn record() -> ProtectedVaultRecord {
        ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_account_id",
            IDENTITY_CANARY.as_bytes().to_vec(),
            format!(r#"{{"access_token":"{TOKEN_CANARY}"}}"#).into_bytes(),
            Some(b"model_provider = \"official\"\n".to_vec()),
        )
        .expect("record")
    }

    #[test]
    fn sha256_and_hmac_match_published_vectors() {
        assert_eq!(
            encode_lower_hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            encode_lower_hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            encode_lower_hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            encode_lower_hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
        for (length, expected) in [
            (
                55,
                "463eb28e72f82e0a96c0a4cc53690c571281131f672aa229e0d45ae59b598b59",
            ),
            (
                56,
                "da2ae4d6b36748f2a318f23e7ab1dfdf45acdc9d049bd80e59de82a60895f562",
            ),
            (
                63,
                "29af2686fd53374a36b0846694cc342177e428d1647515f078784d69cdb9e488",
            ),
            (
                64,
                "fdeab9acf3710362bd2658cdc9a29e8f9c757fcf9811603a8c447cd1d9151108",
            ),
            (
                65,
                "4bfd2c8b6f1eec7a2afeb48b934ee4b2694182027e6d0fc075074f2fabb31781",
            ),
            (
                127,
                "92ca0fa6651ee2f97b884b7246a562fa71250fedefe5ebf270d31c546bfea976",
            ),
            (
                128,
                "471fb943aa23c511f6f72f8d1652d9c880cfa392ad80503120547703e56a2be5",
            ),
        ] {
            let message: Vec<u8> = (0..length).map(|byte| byte as u8).collect();
            assert_eq!(encode_lower_hex(&sha256(&message)), expected, "{length}");
        }
    }

    #[test]
    fn account_fingerprint_is_strict_stable_and_domain_separated() {
        let key = key(KEY_ID, 0x11);
        let first = derive_account_fingerprint(&key, &record()).expect("fingerprint");
        let second = derive_account_fingerprint(&key, &record()).expect("fingerprint");
        assert_eq!(first, second);
        assert!(
            first
                .as_str()
                .starts_with(&format!("hmac-sha256:v1:{KEY_ID}:"))
        );

        let different_identity = ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_account_id",
            b"Identity-Hmac@example.invalid".to_vec(),
            b"{}".to_vec(),
            None,
        )
        .unwrap();
        assert_ne!(
            first,
            derive_account_fingerprint(&key, &different_identity).unwrap()
        );
        let different_kind = ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_email_exact",
            IDENTITY_CANARY.as_bytes().to_vec(),
            b"{}".to_vec(),
            None,
        )
        .unwrap();
        assert_ne!(
            first,
            derive_account_fingerprint(&key, &different_kind).unwrap()
        );
    }

    #[test]
    fn record_authentication_binds_reference_and_every_payload_field() {
        let ring = InstallationKeyRing::new(key(KEY_ID, 0x22));
        let reference = VaultRecordRef::parse(
            "vault-record:v1:018f47a2-8a71-4f4a-9c35-1f4234a73511".to_owned(),
        )
        .unwrap();
        let tag = ring.authenticate_record(&reference, &record());
        ring.verify_record(&reference, &record(), &tag)
            .expect("valid tag");

        let other_reference = VaultRecordRef::parse(
            "vault-record:v1:018f47a2-8a71-4f4a-9c35-1f4234a73512".to_owned(),
        )
        .unwrap();
        assert_eq!(
            ring.verify_record(&other_reference, &record(), &tag),
            Err(IdentityError::RecordAuthenticationFailed)
        );
        let changed = ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_account_id",
            IDENTITY_CANARY.as_bytes().to_vec(),
            b"changed-auth".to_vec(),
            Some(b"model_provider = \"official\"\n".to_vec()),
        )
        .unwrap();
        assert_eq!(
            ring.verify_record(&reference, &changed, &tag),
            Err(IdentityError::RecordAuthenticationFailed)
        );
    }

    #[test]
    fn rotation_window_verifies_old_tags_but_uses_the_new_key() {
        let reference = VaultRecordRef::parse(
            "vault-record:v1:018f47a2-8a71-4f4a-9c35-1f4234a73513".to_owned(),
        )
        .unwrap();
        let old_ring = InstallationKeyRing::new(key(KEY_ID, 0x33));
        let old_tag = old_ring.authenticate_record(&reference, &record());
        let old_fingerprint = old_ring.account_fingerprint(&record()).unwrap();

        let rotating = InstallationKeyRing::with_verification_keys(
            key(KEY_ID_2, 0x44),
            vec![key(KEY_ID, 0x33)],
        )
        .unwrap();
        rotating
            .verify_record(&reference, &record(), &old_tag)
            .expect("old key retained during rotation");
        let new_fingerprint = rotating.account_fingerprint(&record()).unwrap();
        assert_ne!(old_fingerprint, new_fingerprint);
        assert!(new_fingerprint.as_str().contains(KEY_ID_2));

        let after_retirement = InstallationKeyRing::new(key(KEY_ID_2, 0x44));
        assert_eq!(
            after_retirement.verify_record(&reference, &record(), &old_tag),
            Err(IdentityError::UnknownKeyId)
        );
    }

    #[test]
    fn invalid_key_material_and_ambiguous_rotation_sets_are_rejected() {
        assert!(matches!(
            InstallationKey::from_parts(KEY_ID, [0; INSTALLATION_SECRET_BYTES]),
            Err(IdentityError::InvalidSecret)
        ));
        assert!(matches!(
            InstallationKeyRing::with_verification_keys(key(KEY_ID, 0x11), vec![key(KEY_ID, 0x22)]),
            Err(IdentityError::InvalidKeyRing)
        ));
        let predecessors: Vec<_> = (0..=InstallationKeyRing::MAX_VERIFICATION_KEYS)
            .map(|index| {
                let id = format!("018f47a2-8a71-4f4a-9c35-{index:012x}");
                key(&id, index as u8 + 1)
            })
            .collect();
        assert!(matches!(
            InstallationKeyRing::with_verification_keys(key(KEY_ID, 0x33), predecessors),
            Err(IdentityError::InvalidKeyRing)
        ));
    }

    #[test]
    fn explicit_file_fallback_is_atomic_private_and_stable() {
        let root = TestRoot::new();
        let store = FileInstallationKeyStore::open_explicit_fallback(&root.0).unwrap();
        let mut random = FixedRandom { next: 1 };
        let first = store
            .load_or_create_key_ring_with(&mut random)
            .unwrap()
            .into_active_key();
        let loaded = store.load().unwrap().expect("persisted key");
        assert!(first.same_material(&loaded));
        assert_eq!(store.staging_residue_count().unwrap(), 0);
        assert_eq!(
            fs::symlink_metadata(&root.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::symlink_metadata(root.0.join(ACTIVE_KEY_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            crate::PRIVATE_FILE_MODE
        );
        assert_eq!(
            fs::metadata(root.0.join(ACTIVE_KEY_FILE)).unwrap().len(),
            key_ring::encoded_key_ring_bytes(1)
        );
        assert_eq!(first.key_id().as_bytes()[14], b'4');
        assert!(matches!(
            first.key_id().as_bytes()[19],
            b'8' | b'9' | b'a' | b'b'
        ));
    }

    #[test]
    fn legacy_single_key_is_read_then_guardedly_migrated_to_v2() {
        let root = TestRoot::new();
        let store = FileInstallationKeyStore::open_explicit_fallback(&root.0).unwrap();
        let mut legacy_bytes = encode_installation_key(&key(KEY_ID, 0x31));
        write_private_file(&root.0.join(ACTIVE_KEY_FILE), &legacy_bytes).unwrap();
        legacy_bytes.fill(0);

        let legacy = store.load_key_ring().unwrap().unwrap();
        assert_eq!(
            legacy.storage_version(),
            InstallationKeyStorageVersion::SingleKeyV1
        );
        assert_eq!(legacy.revision(), 1);
        assert_eq!(legacy.active_key_id(), KEY_ID);
        let expected = legacy.revision_token();
        let replacement = legacy.start_rotation(key(KEY_ID_2, 0x32)).unwrap();

        assert_eq!(
            store.replace_key_ring(&expected, &replacement).unwrap(),
            InstallationKeyRingMutationOutcome::Applied
        );
        let migrated = store.load_key_ring().unwrap().unwrap();
        assert_eq!(
            migrated.storage_version(),
            InstallationKeyStorageVersion::KeyRingV2
        );
        assert_eq!(migrated.revision(), 2);
        assert_eq!(migrated.active_key_id(), KEY_ID_2);
        assert_eq!(
            migrated.verification_key_ids().collect::<Vec<_>>(),
            [KEY_ID]
        );
    }

    #[test]
    fn versioned_file_ring_enforces_revision_for_replace_retire_and_delete() {
        let root = TestRoot::new();
        let store = FileInstallationKeyStore::open_explicit_fallback(&root.0).unwrap();
        let initial = store
            .load_or_create_key_ring_with(&mut FixedRandom { next: 7 })
            .unwrap();
        let initial_id = initial.active_key_id().to_owned();
        let initial_revision = initial.revision_token();
        let rotating = initial.start_rotation(key(KEY_ID_2, 0x42)).unwrap();

        let other_root = TestRoot::new();
        let other_store = FileInstallationKeyStore::open_explicit_fallback(&other_root.0).unwrap();
        let wrong_active_revision = other_store
            .load_or_create_key_ring_with(&mut FixedRandom { next: 91 })
            .unwrap()
            .revision_token();
        assert_eq!(
            wrong_active_revision.revision(),
            initial_revision.revision()
        );
        assert_ne!(
            wrong_active_revision.active_key_id(),
            initial_revision.active_key_id()
        );
        assert_eq!(
            store
                .replace_key_ring(&wrong_active_revision, &rotating)
                .unwrap(),
            InstallationKeyRingMutationOutcome::RevisionConflict
        );
        assert_eq!(
            store.load_key_ring().unwrap().unwrap().active_key_id(),
            initial_id
        );

        let foreign_successor = InstallationKeyRingRecord::initial(key(KEY_ID, 0x71))
            .start_rotation(key(KEY_ID_2, 0x72))
            .unwrap();
        assert!(matches!(
            store.replace_key_ring(&initial_revision, &foreign_successor),
            Err(IdentityError::InvalidKeyRing)
        ));
        assert_eq!(
            store.load_key_ring().unwrap().unwrap().active_key_id(),
            initial_id
        );

        assert_eq!(
            store
                .replace_key_ring(&initial_revision, &rotating)
                .unwrap(),
            InstallationKeyRingMutationOutcome::Applied
        );
        assert_eq!(
            store
                .replace_key_ring(&initial_revision, &rotating)
                .unwrap(),
            InstallationKeyRingMutationOutcome::RevisionConflict
        );
        assert_eq!(store.staging_residue_count().unwrap(), 0);

        let installed = store.load_key_ring().unwrap().unwrap();
        assert_eq!(installed.revision(), 2);
        assert_eq!(installed.active_key_id(), KEY_ID_2);
        assert_eq!(
            installed.verification_key_ids().collect::<Vec<_>>(),
            [initial_id.as_str()]
        );
        let rotating_revision = installed.revision_token();
        let retired = installed.retire_verification_key(&initial_id).unwrap();
        assert_eq!(
            store
                .replace_key_ring(&rotating_revision, &retired)
                .unwrap(),
            InstallationKeyRingMutationOutcome::Applied
        );

        assert_eq!(
            store.delete_key_ring(&rotating_revision).unwrap(),
            InstallationKeyRingMutationOutcome::RevisionConflict
        );
        let final_record = store.load_key_ring().unwrap().unwrap();
        assert_eq!(final_record.revision(), 3);
        assert_eq!(final_record.verification_key_ids().len(), 0);
        let final_revision = final_record.revision_token();
        assert_eq!(
            store.delete_key_ring(&final_revision).unwrap(),
            InstallationKeyRingMutationOutcome::Applied
        );
        assert!(store.load_key_ring().unwrap().is_none());
        assert_eq!(store.staging_residue_count().unwrap(), 0);
    }

    #[test]
    fn backend_selection_requires_an_explicit_fallback_request_and_redacts_its_root() {
        let root = TestRoot::new();
        let request = InstallationKeyBackendRequest::ExplicitFileFallback {
            root: root.0.clone(),
        };
        let rendered = format!("{request:?}");
        assert_eq!(rendered, "ExplicitFileFallback { root: <redacted> }");
        assert!(!rendered.contains(root.0.to_string_lossy().as_ref()));

        let mut selected = select_installation_key_store(request).unwrap();
        assert!(matches!(
            &selected,
            SelectedInstallationKeyStore::ExplicitFileFallback(_)
        ));
        let ring = InstallationKeyRingStore::load_or_create_key_ring(&mut selected).unwrap();
        let active_key_id = ring.active_key_id().to_owned();
        assert_eq!(active_key_id.len(), 36);
        let key = InstallationKeyStore::load(&mut selected)
            .unwrap()
            .expect("selected ring active key");
        assert_eq!(key.key_id(), active_key_id);
    }

    #[test]
    fn rejected_random_material_creates_no_key_or_staging_residue() {
        let root = TestRoot::new();
        let store = FileInstallationKeyStore::open_explicit_fallback(&root.0).unwrap();
        assert!(matches!(
            store.load_or_create_key_ring_with(&mut ZeroRandom),
            Err(IdentityError::RandomSourceRejected)
        ));
        assert!(store.load().unwrap().is_none());
        assert_eq!(store.staging_residue_count().unwrap(), 0);
    }

    #[test]
    fn concurrent_file_fallback_creation_converges_on_one_key() {
        let root = TestRoot::new();
        let store = Arc::new(FileInstallationKeyStore::open_explicit_fallback(&root.0).unwrap());
        let mut workers = Vec::new();
        for _ in 0..4 {
            let store = Arc::clone(&store);
            workers.push(std::thread::spawn(move || store.load_or_create().unwrap()));
        }
        let keys: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        for key in &keys[1..] {
            assert!(keys[0].same_material(key));
        }
        assert_eq!(store.staging_residue_count().unwrap(), 0);
    }

    #[test]
    fn file_fallback_fails_closed_on_corruption_permission_drift_and_symlink() {
        let root = TestRoot::new();
        let store = FileInstallationKeyStore::open_explicit_fallback(&root.0).unwrap();
        fs::write(root.0.join(ACTIVE_KEY_FILE), b"partial").unwrap();
        fs::set_permissions(
            root.0.join(ACTIVE_KEY_FILE),
            fs::Permissions::from_mode(crate::PRIVATE_FILE_MODE),
        )
        .unwrap();
        assert!(matches!(store.load(), Err(IdentityError::KeyRecordCorrupt)));

        fs::set_permissions(
            root.0.join(ACTIVE_KEY_FILE),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(matches!(
            store.load(),
            Err(IdentityError::Storage(
                VaultBackendError::InsecurePermissions
            ))
        ));

        fs::remove_file(root.0.join(ACTIVE_KEY_FILE)).unwrap();
        let outside = root.0.with_extension("outside");
        fs::write(&outside, vec![0_u8; KEY_FILE_BYTES as usize]).unwrap();
        symlink(&outside, root.0.join(ACTIVE_KEY_FILE)).unwrap();
        assert!(matches!(
            store.load(),
            Err(IdentityError::Storage(VaultBackendError::UnsafeFileType))
        ));
        fs::remove_file(&outside).unwrap();
    }

    #[test]
    fn identity_debug_and_errors_never_render_key_or_payload_canaries() {
        let root = TestRoot::new();
        let store = FileInstallationKeyStore::open_explicit_fallback(&root.0).unwrap();
        let installation_key = key(KEY_ID, 0x55);
        let ring = InstallationKeyRing::new(key(KEY_ID_2, 0x66));
        let key_ring_record = InstallationKeyRingRecord::initial(key(KEY_ID, 0x67));
        let key_ring_revision = key_ring_record.revision_token();
        let reference = VaultRecordRef::parse(
            "vault-record:v1:018f47a2-8a71-4f4a-9c35-1f4234a73514".to_owned(),
        )
        .unwrap();
        let tag = ring.authenticate_record(&reference, &record());
        let path_canary = root.0.to_string_lossy();
        for rendered in [
            format!("{store:?}"),
            format!("{installation_key:?}"),
            format!("{ring:?}"),
            format!("{key_ring_record:?}"),
            format!("{key_ring_revision:?}"),
            format!("{tag:?}"),
            IdentityError::RecordAuthenticationFailed.to_string(),
            format!("{:?}", IdentityError::KeyRecordCorrupt),
        ] {
            assert!(!rendered.contains(KEY_ID));
            assert!(!rendered.contains(KEY_ID_2));
            assert!(!rendered.contains(IDENTITY_CANARY));
            assert!(!rendered.contains(TOKEN_CANARY));
            assert!(!rendered.contains(path_canary.as_ref()));
        }
    }

    #[test]
    fn tag_parser_rejects_noncanonical_or_wrong_length_values() {
        for value in [
            "record-hmac-sha256:v1:not-a-uuid:00",
            "record-hmac-sha256:v1:018f47a2-8a71-4f4a-9c35-1f4234a73501:AA00000000000000000000000000000000000000000000000000000000000000",
            "hmac-sha256:v1:018f47a2-8a71-4f4a-9c35-1f4234a73501:0000000000000000000000000000000000000000000000000000000000000000",
        ] {
            assert_eq!(
                RecordAuthenticationTag::parse(value),
                Err(IdentityError::InvalidRecordAuthenticationTag)
            );
        }
    }
}
