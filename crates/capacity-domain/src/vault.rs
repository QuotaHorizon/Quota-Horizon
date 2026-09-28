use chrono::DateTime;
use thiserror::Error;

use crate::{AccountFingerprint, UtcTimestamp};

pub const VAULT_ACCOUNT_ID_PREFIX: &str = "account:v1:";
pub const VAULT_RECORD_REF_PREFIX: &str = "vault-record:v1:";
pub const VAULT_OPERATION_ID_PREFIX: &str = "vault-operation:v1:";
pub const VAULT_KEY_ROTATION_ID_PREFIX: &str = "vault-key-rotation:v1:";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VaultAccountId(String);

impl VaultAccountId {
    pub fn parse(value: impl Into<String>) -> Result<Self, VaultValidationError> {
        let value = value.into();
        if !is_versioned_uuid(&value, VAULT_ACCOUNT_ID_PREFIX) {
            return Err(VaultValidationError::InvalidAccountId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque reference to a protected record owned by an account-vault backend.
///
/// The referenced credential and stable upstream identity never enter this
/// value or the ordinary SQLite store. Debug output remains redacted so a
/// record locator cannot accidentally become a diagnostic correlation key.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct VaultRecordRef(String);

impl std::fmt::Debug for VaultRecordRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("VaultRecordRef")
            .field(&"<redacted>")
            .finish()
    }
}

impl VaultRecordRef {
    pub fn parse(value: impl Into<String>) -> Result<Self, VaultValidationError> {
        let value = value.into();
        if !is_versioned_uuid(&value, VAULT_RECORD_REF_PREFIX) {
            return Err(VaultValidationError::InvalidRecordRef);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct VaultOperationId(String);

impl std::fmt::Debug for VaultOperationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("VaultOperationId")
            .field(&"<redacted>")
            .finish()
    }
}

impl VaultOperationId {
    pub fn parse(value: impl Into<String>) -> Result<Self, VaultValidationError> {
        let value = value.into();
        if !is_versioned_uuid(&value, VAULT_OPERATION_ID_PREFIX) {
            return Err(VaultValidationError::InvalidOperationId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Non-secret installation-key identifier. Debug remains redacted because the
/// identifier is still a stable local correlation value.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct InstallationKeyId(String);

impl std::fmt::Debug for InstallationKeyId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("InstallationKeyId")
            .field(&"<redacted>")
            .finish()
    }
}

impl InstallationKeyId {
    pub fn parse(value: impl Into<String>) -> Result<Self, VaultValidationError> {
        let value = value.into();
        if !is_lowercase_uuid(&value) {
            return Err(VaultValidationError::InvalidInstallationKeyId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct VaultKeyRotationId(String);

impl std::fmt::Debug for VaultKeyRotationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("VaultKeyRotationId")
            .field(&"<redacted>")
            .finish()
    }
}

impl VaultKeyRotationId {
    pub fn parse(value: impl Into<String>) -> Result<Self, VaultValidationError> {
        let value = value.into();
        if !is_versioned_uuid(&value, VAULT_KEY_ROTATION_ID_PREFIX) {
            return Err(VaultValidationError::InvalidKeyRotationId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultAccountAuthMode {
    ChatGpt,
    ApiKey,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultAccountSource {
    CurrentRuntime,
    ManualChatGpt,
    ManualApi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultAccountLifecycle {
    Ready,
    RecordUnavailable,
    HistoricalOnly,
    NeedsReview,
}

/// Metadata accepted only after a protected backend has allocated the opaque
/// record reference. It intentionally cannot carry credential bytes or the
/// raw upstream identity used to derive `account_fingerprint`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultAccountRegistration {
    pub account_fingerprint: AccountFingerprint,
    pub protected_record_ref: VaultRecordRef,
    pub display_name: String,
    pub auth_mode: VaultAccountAuthMode,
    pub source: VaultAccountSource,
    pub lifecycle: VaultAccountLifecycle,
    pub provider_id: Option<String>,
    pub model: Option<String>,
}

impl VaultAccountRegistration {
    pub fn validate(&self) -> Result<(), VaultValidationError> {
        validate_metadata(&self.display_name, 128, "display_name")?;
        validate_optional_metadata(self.provider_id.as_deref(), 128, "provider_id")?;
        validate_optional_metadata(self.model.as_deref(), 256, "model")?;

        let source_matches_auth = match self.source {
            VaultAccountSource::CurrentRuntime => true,
            VaultAccountSource::ManualChatGpt => self.auth_mode == VaultAccountAuthMode::ChatGpt,
            VaultAccountSource::ManualApi => self.auth_mode == VaultAccountAuthMode::ApiKey,
        };
        if !source_matches_auth {
            return Err(VaultValidationError::SourceAuthMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultAccount {
    pub account_id: VaultAccountId,
    pub account_fingerprint: AccountFingerprint,
    pub protected_record_ref: VaultRecordRef,
    pub display_name: String,
    pub auth_mode: VaultAccountAuthMode,
    pub source: VaultAccountSource,
    pub lifecycle: VaultAccountLifecycle,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub revision: u32,
    pub display_order: u32,
    pub created_at: UtcTimestamp,
    pub updated_at: UtcTimestamp,
    pub last_used_at: Option<UtcTimestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultEnvironmentState {
    pub environment_id: String,
    pub revision: u32,
    pub selected_account_id: Option<VaultAccountId>,
    pub observed_account_id: Option<VaultAccountId>,
    pub updated_at: UtcTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultCatalogSnapshot {
    pub accounts: Vec<VaultAccount>,
    pub environment_state: Option<VaultEnvironmentState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultOperationKind {
    RegisterAccount,
    ForgetAccount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultOperationStatus {
    InProgress,
    Succeeded,
    Compensated,
    NeedsReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultOperationCheckpoint {
    Prepared,
    RecordReady,
    RecordQuarantined,
    MetadataCommitted,
    MetadataRemoved,
    RecordRestored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultOperation {
    pub operation_id: VaultOperationId,
    pub kind: VaultOperationKind,
    pub status: VaultOperationStatus,
    pub checkpoint: VaultOperationCheckpoint,
    pub revision: u32,
    pub account_id: Option<VaultAccountId>,
    pub account_fingerprint: AccountFingerprint,
    pub protected_record_ref: VaultRecordRef,
    pub display_name: String,
    pub auth_mode: VaultAccountAuthMode,
    pub source: VaultAccountSource,
    pub lifecycle: VaultAccountLifecycle,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub expected_account_revision: Option<u32>,
    pub last_error_code: Option<String>,
    pub created_at: UtcTimestamp,
    pub updated_at: UtcTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultOperationTransition {
    RecordReady,
    RecordQuarantined,
    MetadataCommitted { account_id: VaultAccountId },
    MetadataRemoved,
    RecordRestored,
    Succeeded,
    Compensated { reason_code: String },
    NeedsReview { reason_code: String },
    Resume,
}

impl VaultOperation {
    pub fn validate(&self) -> Result<(), VaultValidationError> {
        if self.revision == 0 {
            return Err(VaultValidationError::InvalidOperationState);
        }
        if !timestamp_not_before(&self.updated_at, &self.created_at) {
            return Err(VaultValidationError::InvalidOperationTimestamp);
        }
        VaultAccountRegistration {
            account_fingerprint: self.account_fingerprint.clone(),
            protected_record_ref: self.protected_record_ref.clone(),
            display_name: self.display_name.clone(),
            auth_mode: self.auth_mode,
            source: self.source,
            lifecycle: self.lifecycle,
            provider_id: self.provider_id.clone(),
            model: self.model.clone(),
        }
        .validate()?;
        if self
            .last_error_code
            .as_deref()
            .is_some_and(|reason| !valid_operation_reason(reason))
        {
            return Err(VaultValidationError::InvalidOperationReason);
        }

        let kind_fields_valid = match self.kind {
            VaultOperationKind::RegisterAccount => {
                self.expected_account_revision.is_none()
                    && match self.checkpoint {
                        VaultOperationCheckpoint::Prepared
                        | VaultOperationCheckpoint::RecordReady
                        | VaultOperationCheckpoint::RecordQuarantined => self.account_id.is_none(),
                        VaultOperationCheckpoint::MetadataCommitted => self.account_id.is_some(),
                        VaultOperationCheckpoint::MetadataRemoved
                        | VaultOperationCheckpoint::RecordRestored => false,
                    }
            }
            VaultOperationKind::ForgetAccount => {
                self.account_id.is_some()
                    && self
                        .expected_account_revision
                        .is_some_and(|revision| revision > 0)
                    && matches!(
                        self.checkpoint,
                        VaultOperationCheckpoint::Prepared
                            | VaultOperationCheckpoint::RecordQuarantined
                            | VaultOperationCheckpoint::MetadataRemoved
                            | VaultOperationCheckpoint::RecordRestored
                    )
            }
        };
        if !kind_fields_valid {
            return Err(VaultValidationError::InvalidOperationState);
        }

        let status_valid = match self.status {
            VaultOperationStatus::InProgress => self.last_error_code.is_none(),
            VaultOperationStatus::NeedsReview => self.last_error_code.is_some(),
            VaultOperationStatus::Succeeded => {
                self.last_error_code.is_none()
                    && matches!(
                        (self.kind, self.checkpoint),
                        (
                            VaultOperationKind::RegisterAccount,
                            VaultOperationCheckpoint::MetadataCommitted
                        ) | (
                            VaultOperationKind::ForgetAccount,
                            VaultOperationCheckpoint::MetadataRemoved
                        )
                    )
            }
            VaultOperationStatus::Compensated => {
                self.last_error_code.is_some()
                    && matches!(
                        (self.kind, self.checkpoint),
                        (
                            VaultOperationKind::RegisterAccount,
                            VaultOperationCheckpoint::RecordQuarantined
                        ) | (
                            VaultOperationKind::ForgetAccount,
                            VaultOperationCheckpoint::RecordRestored
                        )
                    )
            }
        };
        if !status_valid {
            return Err(VaultValidationError::InvalidOperationState);
        }
        Ok(())
    }

    pub fn apply_transition(
        &self,
        transition: VaultOperationTransition,
        updated_at: UtcTimestamp,
    ) -> Result<Self, VaultValidationError> {
        self.validate()?;
        if !timestamp_not_before(&updated_at, &self.updated_at) {
            return Err(VaultValidationError::InvalidOperationTimestamp);
        }
        let mut next = self.clone();
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(VaultValidationError::InvalidOperationState)?;
        next.updated_at = updated_at;

        match transition {
            VaultOperationTransition::NeedsReview { reason_code }
                if self.status == VaultOperationStatus::InProgress =>
            {
                validate_operation_reason(&reason_code)?;
                next.status = VaultOperationStatus::NeedsReview;
                next.last_error_code = Some(reason_code);
            }
            VaultOperationTransition::Resume
                if self.status == VaultOperationStatus::NeedsReview =>
            {
                next.status = VaultOperationStatus::InProgress;
                next.last_error_code = None;
            }
            _ if self.status != VaultOperationStatus::InProgress => {
                return Err(VaultValidationError::InvalidOperationTransition);
            }
            VaultOperationTransition::RecordReady
                if self.kind == VaultOperationKind::RegisterAccount
                    && self.checkpoint == VaultOperationCheckpoint::Prepared =>
            {
                next.checkpoint = VaultOperationCheckpoint::RecordReady;
            }
            VaultOperationTransition::RecordQuarantined
                if (self.kind == VaultOperationKind::RegisterAccount
                    && self.checkpoint == VaultOperationCheckpoint::RecordReady)
                    || (self.kind == VaultOperationKind::ForgetAccount
                        && self.checkpoint == VaultOperationCheckpoint::Prepared) =>
            {
                next.checkpoint = VaultOperationCheckpoint::RecordQuarantined;
            }
            VaultOperationTransition::MetadataCommitted { account_id }
                if self.kind == VaultOperationKind::RegisterAccount
                    && self.checkpoint == VaultOperationCheckpoint::RecordReady =>
            {
                next.account_id = Some(account_id);
                next.checkpoint = VaultOperationCheckpoint::MetadataCommitted;
            }
            VaultOperationTransition::MetadataRemoved
                if self.kind == VaultOperationKind::ForgetAccount
                    && self.checkpoint == VaultOperationCheckpoint::RecordQuarantined =>
            {
                next.checkpoint = VaultOperationCheckpoint::MetadataRemoved;
            }
            VaultOperationTransition::RecordRestored
                if self.kind == VaultOperationKind::ForgetAccount
                    && self.checkpoint == VaultOperationCheckpoint::RecordQuarantined =>
            {
                next.checkpoint = VaultOperationCheckpoint::RecordRestored;
            }
            VaultOperationTransition::Succeeded
                if matches!(
                    (self.kind, self.checkpoint),
                    (
                        VaultOperationKind::RegisterAccount,
                        VaultOperationCheckpoint::MetadataCommitted
                    ) | (
                        VaultOperationKind::ForgetAccount,
                        VaultOperationCheckpoint::MetadataRemoved
                    )
                ) =>
            {
                next.status = VaultOperationStatus::Succeeded;
            }
            VaultOperationTransition::Compensated { reason_code }
                if matches!(
                    (self.kind, self.checkpoint),
                    (
                        VaultOperationKind::RegisterAccount,
                        VaultOperationCheckpoint::RecordQuarantined
                    ) | (
                        VaultOperationKind::ForgetAccount,
                        VaultOperationCheckpoint::RecordRestored
                    )
                ) =>
            {
                validate_operation_reason(&reason_code)?;
                next.status = VaultOperationStatus::Compensated;
                next.last_error_code = Some(reason_code);
            }
            _ => return Err(VaultValidationError::InvalidOperationTransition),
        }
        next.validate()?;
        Ok(next)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultKeyRotationStatus {
    InProgress,
    RollingBack,
    Succeeded,
    Compensated,
    NeedsReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultKeyRotationCheckpoint {
    Prepared,
    KeyRingStarted,
    AccountsMigrated,
    DependenciesCleared,
    PredecessorRetired,
    RollbackStarted,
    AccountsRestored,
    NewKeyRetired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultKeyRotation {
    pub rotation_id: VaultKeyRotationId,
    pub status: VaultKeyRotationStatus,
    pub checkpoint: VaultKeyRotationCheckpoint,
    pub revision: u32,
    pub source_key_id: InstallationKeyId,
    pub target_key_id: InstallationKeyId,
    pub expected_key_ring_revision: u64,
    pub last_error_code: Option<String>,
    pub created_at: UtcTimestamp,
    pub updated_at: UtcTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultKeyRotationTransition {
    KeyRingStarted,
    AccountsMigrated,
    DependenciesCleared,
    PredecessorRetired,
    Succeeded,
    BeginRollback { reason_code: String },
    AccountsRestored,
    NewKeyRetired,
    Compensated,
    NeedsReview { reason_code: String },
    Resume,
}

impl VaultKeyRotation {
    pub fn validate(&self) -> Result<(), VaultValidationError> {
        if self.revision == 0
            || self.expected_key_ring_revision == 0
            || self.source_key_id == self.target_key_id
        {
            return Err(VaultValidationError::InvalidKeyRotationState);
        }
        if !timestamp_not_before(&self.updated_at, &self.created_at) {
            return Err(VaultValidationError::InvalidOperationTimestamp);
        }
        if self
            .last_error_code
            .as_deref()
            .is_some_and(|reason| !valid_operation_reason(reason))
        {
            return Err(VaultValidationError::InvalidOperationReason);
        }

        let forward = is_forward_rotation_checkpoint(self.checkpoint);
        let rollback = is_rollback_rotation_checkpoint(self.checkpoint);
        let valid = match self.status {
            VaultKeyRotationStatus::InProgress => forward && self.last_error_code.is_none(),
            VaultKeyRotationStatus::RollingBack => rollback && self.last_error_code.is_some(),
            VaultKeyRotationStatus::NeedsReview => {
                (forward || rollback) && self.last_error_code.is_some()
            }
            VaultKeyRotationStatus::Succeeded => {
                self.checkpoint == VaultKeyRotationCheckpoint::PredecessorRetired
                    && self.last_error_code.is_none()
            }
            VaultKeyRotationStatus::Compensated => {
                self.checkpoint == VaultKeyRotationCheckpoint::NewKeyRetired
                    && self.last_error_code.is_some()
            }
        };
        if !valid {
            return Err(VaultValidationError::InvalidKeyRotationState);
        }
        Ok(())
    }

    pub fn apply_transition(
        &self,
        transition: VaultKeyRotationTransition,
        updated_at: UtcTimestamp,
    ) -> Result<Self, VaultValidationError> {
        self.validate()?;
        if !timestamp_not_before(&updated_at, &self.updated_at) {
            return Err(VaultValidationError::InvalidOperationTimestamp);
        }
        let mut next = self.clone();
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(VaultValidationError::InvalidKeyRotationState)?;
        next.updated_at = updated_at;

        match transition {
            VaultKeyRotationTransition::NeedsReview { reason_code }
                if matches!(
                    self.status,
                    VaultKeyRotationStatus::InProgress | VaultKeyRotationStatus::RollingBack
                ) =>
            {
                validate_operation_reason(&reason_code)?;
                next.status = VaultKeyRotationStatus::NeedsReview;
                next.last_error_code = Some(reason_code);
            }
            VaultKeyRotationTransition::Resume
                if self.status == VaultKeyRotationStatus::NeedsReview =>
            {
                if is_forward_rotation_checkpoint(self.checkpoint) {
                    next.status = VaultKeyRotationStatus::InProgress;
                    next.last_error_code = None;
                } else {
                    next.status = VaultKeyRotationStatus::RollingBack;
                }
            }
            _ if matches!(
                self.status,
                VaultKeyRotationStatus::Succeeded
                    | VaultKeyRotationStatus::Compensated
                    | VaultKeyRotationStatus::NeedsReview
            ) =>
            {
                return Err(VaultValidationError::InvalidKeyRotationTransition);
            }
            VaultKeyRotationTransition::KeyRingStarted
                if self.status == VaultKeyRotationStatus::InProgress
                    && self.checkpoint == VaultKeyRotationCheckpoint::Prepared =>
            {
                next.checkpoint = VaultKeyRotationCheckpoint::KeyRingStarted;
            }
            VaultKeyRotationTransition::AccountsMigrated
                if self.status == VaultKeyRotationStatus::InProgress
                    && self.checkpoint == VaultKeyRotationCheckpoint::KeyRingStarted =>
            {
                next.checkpoint = VaultKeyRotationCheckpoint::AccountsMigrated;
            }
            VaultKeyRotationTransition::DependenciesCleared
                if self.status == VaultKeyRotationStatus::InProgress
                    && self.checkpoint == VaultKeyRotationCheckpoint::AccountsMigrated =>
            {
                next.checkpoint = VaultKeyRotationCheckpoint::DependenciesCleared;
            }
            VaultKeyRotationTransition::PredecessorRetired
                if self.status == VaultKeyRotationStatus::InProgress
                    && self.checkpoint == VaultKeyRotationCheckpoint::DependenciesCleared =>
            {
                next.checkpoint = VaultKeyRotationCheckpoint::PredecessorRetired;
            }
            VaultKeyRotationTransition::Succeeded
                if self.status == VaultKeyRotationStatus::InProgress
                    && self.checkpoint == VaultKeyRotationCheckpoint::PredecessorRetired =>
            {
                next.status = VaultKeyRotationStatus::Succeeded;
            }
            VaultKeyRotationTransition::BeginRollback { reason_code }
                if self.status == VaultKeyRotationStatus::InProgress
                    && self.checkpoint != VaultKeyRotationCheckpoint::PredecessorRetired =>
            {
                validate_operation_reason(&reason_code)?;
                next.status = VaultKeyRotationStatus::RollingBack;
                next.checkpoint = VaultKeyRotationCheckpoint::RollbackStarted;
                next.last_error_code = Some(reason_code);
            }
            VaultKeyRotationTransition::AccountsRestored
                if self.status == VaultKeyRotationStatus::RollingBack
                    && self.checkpoint == VaultKeyRotationCheckpoint::RollbackStarted =>
            {
                next.checkpoint = VaultKeyRotationCheckpoint::AccountsRestored;
            }
            VaultKeyRotationTransition::NewKeyRetired
                if self.status == VaultKeyRotationStatus::RollingBack
                    && self.checkpoint == VaultKeyRotationCheckpoint::AccountsRestored =>
            {
                next.checkpoint = VaultKeyRotationCheckpoint::NewKeyRetired;
            }
            VaultKeyRotationTransition::Compensated
                if self.status == VaultKeyRotationStatus::RollingBack
                    && self.checkpoint == VaultKeyRotationCheckpoint::NewKeyRetired =>
            {
                next.status = VaultKeyRotationStatus::Compensated;
            }
            _ => return Err(VaultValidationError::InvalidKeyRotationTransition),
        }
        next.validate()?;
        Ok(next)
    }
}

fn is_forward_rotation_checkpoint(checkpoint: VaultKeyRotationCheckpoint) -> bool {
    matches!(
        checkpoint,
        VaultKeyRotationCheckpoint::Prepared
            | VaultKeyRotationCheckpoint::KeyRingStarted
            | VaultKeyRotationCheckpoint::AccountsMigrated
            | VaultKeyRotationCheckpoint::DependenciesCleared
            | VaultKeyRotationCheckpoint::PredecessorRetired
    )
}

fn is_rollback_rotation_checkpoint(checkpoint: VaultKeyRotationCheckpoint) -> bool {
    matches!(
        checkpoint,
        VaultKeyRotationCheckpoint::RollbackStarted
            | VaultKeyRotationCheckpoint::AccountsRestored
            | VaultKeyRotationCheckpoint::NewKeyRetired
    )
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum VaultValidationError {
    #[error("vault account ID must be a versioned lowercase UUID")]
    InvalidAccountId,
    #[error("vault record reference must be a versioned lowercase UUID")]
    InvalidRecordRef,
    #[error("vault operation ID must be a versioned lowercase UUID")]
    InvalidOperationId,
    #[error("installation key ID must be a lowercase UUID")]
    InvalidInstallationKeyId,
    #[error("vault key rotation ID must be a versioned lowercase UUID")]
    InvalidKeyRotationId,
    #[error("invalid bounded vault metadata field: {0}")]
    InvalidMetadata(&'static str),
    #[error("vault account source does not match auth mode")]
    SourceAuthMismatch,
    #[error("vault operation reason code is invalid")]
    InvalidOperationReason,
    #[error("vault operation state is invalid")]
    InvalidOperationState,
    #[error("vault operation timestamp moved backwards")]
    InvalidOperationTimestamp,
    #[error("vault operation transition is invalid")]
    InvalidOperationTransition,
    #[error("vault key rotation state is invalid")]
    InvalidKeyRotationState,
    #[error("vault key rotation transition is invalid")]
    InvalidKeyRotationTransition,
}

fn validate_operation_reason(value: &str) -> Result<(), VaultValidationError> {
    if valid_operation_reason(value) {
        Ok(())
    } else {
        Err(VaultValidationError::InvalidOperationReason)
    }
}

fn valid_operation_reason(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'.' | b':' | b'-')
        })
}

fn timestamp_not_before(candidate: &UtcTimestamp, reference: &UtcTimestamp) -> bool {
    let candidate = DateTime::parse_from_rfc3339(candidate.as_str());
    let reference = DateTime::parse_from_rfc3339(reference.as_str());
    matches!((candidate, reference), (Ok(candidate), Ok(reference)) if candidate >= reference)
}

fn validate_optional_metadata(
    value: Option<&str>,
    maximum_length: usize,
    field: &'static str,
) -> Result<(), VaultValidationError> {
    if let Some(value) = value {
        validate_metadata(value, maximum_length, field)?;
    }
    Ok(())
}

fn validate_metadata(
    value: &str,
    maximum_length: usize,
    field: &'static str,
) -> Result<(), VaultValidationError> {
    if value.is_empty()
        || value.len() > maximum_length
        || value != value.trim()
        || value.chars().any(char::is_control)
    {
        return Err(VaultValidationError::InvalidMetadata(field));
    }
    Ok(())
}

fn is_versioned_uuid(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(is_lowercase_uuid)
}

fn is_lowercase_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINGERPRINT: &str = "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73311:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn registration() -> VaultAccountRegistration {
        VaultAccountRegistration {
            account_fingerprint: AccountFingerprint::parse(FINGERPRINT).expect("fingerprint"),
            protected_record_ref: VaultRecordRef::parse(
                "vault-record:v1:018f47a2-8a71-7f4a-9c35-1f4234a73312",
            )
            .expect("record reference"),
            display_name: "Primary ChatGPT".into(),
            auth_mode: VaultAccountAuthMode::ChatGpt,
            source: VaultAccountSource::ManualChatGpt,
            lifecycle: VaultAccountLifecycle::Ready,
            provider_id: None,
            model: None,
        }
    }

    fn registration_operation() -> VaultOperation {
        let registration = registration();
        VaultOperation {
            operation_id: VaultOperationId::parse(
                "vault-operation:v1:018f47a2-8a71-7f4a-9c35-1f4234a73313",
            )
            .expect("operation ID"),
            kind: VaultOperationKind::RegisterAccount,
            status: VaultOperationStatus::InProgress,
            checkpoint: VaultOperationCheckpoint::Prepared,
            revision: 1,
            account_id: None,
            account_fingerprint: registration.account_fingerprint,
            protected_record_ref: registration.protected_record_ref,
            display_name: registration.display_name,
            auth_mode: registration.auth_mode,
            source: registration.source,
            lifecycle: registration.lifecycle,
            provider_id: registration.provider_id,
            model: registration.model,
            expected_account_revision: None,
            last_error_code: None,
            created_at: UtcTimestamp::parse("2026-08-30T07:20:00Z").expect("created at"),
            updated_at: UtcTimestamp::parse("2026-08-30T07:20:00Z").expect("updated at"),
        }
    }

    fn key_rotation() -> VaultKeyRotation {
        VaultKeyRotation {
            rotation_id: VaultKeyRotationId::parse(
                "vault-key-rotation:v1:018f47a2-8a71-7f4a-9c35-1f4234a73321",
            )
            .expect("rotation ID"),
            status: VaultKeyRotationStatus::InProgress,
            checkpoint: VaultKeyRotationCheckpoint::Prepared,
            revision: 1,
            source_key_id: InstallationKeyId::parse("018f47a2-8a71-7f4a-9c35-1f4234a73322")
                .expect("source key ID"),
            target_key_id: InstallationKeyId::parse("018f47a2-8a71-7f4a-9c35-1f4234a73323")
                .expect("target key ID"),
            expected_key_ring_revision: 7,
            last_error_code: None,
            created_at: UtcTimestamp::parse("2026-08-30T13:20:00Z").expect("created at"),
            updated_at: UtcTimestamp::parse("2026-08-30T13:20:00Z").expect("updated at"),
        }
    }

    #[test]
    fn opaque_ids_accept_only_canonical_lowercase_uuid_forms() {
        assert!(VaultAccountId::parse("account:v1:018f47a2-8a71-7f4a-9c35-1f4234a73311").is_ok());
        assert!(
            VaultRecordRef::parse("vault-record:v1:018f47a2-8a71-7f4a-9c35-1f4234a73312").is_ok()
        );
        assert!(VaultAccountId::parse("account:v1:018F47A2-8A71-7F4A-9C35-1F4234A73311").is_err());
        assert!(VaultAccountId::parse("acct-018f47a2").is_err());
    }

    #[test]
    fn record_reference_debug_output_is_redacted() {
        let reference =
            VaultRecordRef::parse("vault-record:v1:018f47a2-8a71-7f4a-9c35-1f4234a73312")
                .expect("record reference");
        let rendered = format!("{reference:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("018f47a2"));
    }

    #[test]
    fn operation_id_is_canonical_and_redacted() {
        let operation_id =
            VaultOperationId::parse("vault-operation:v1:018f47a2-8a71-7f4a-9c35-1f4234a73313")
                .expect("operation ID");
        let rendered = format!("{operation_id:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("018f47a2"));
        assert!(
            VaultOperationId::parse("vault-operation:v1:018F47A2-8A71-7F4A-9C35-1F4234A73313")
                .is_err()
        );
    }

    #[test]
    fn rotation_and_key_ids_are_canonical_and_redacted() {
        let rotation = key_rotation();
        let rendered = format!("{rotation:?}");
        assert!(rendered.contains("<redacted>"));
        for canary in ["018f47a2", "a73321", "a73322", "a73323"] {
            assert!(!rendered.contains(canary));
        }
        assert!(InstallationKeyId::parse("018F47A2-8A71-7F4A-9C35-1F4234A73322").is_err());
        assert!(
            VaultKeyRotationId::parse("vault-key-rotation:v1:018F47A2-8A71-7F4A-9C35-1F4234A73321")
                .is_err()
        );
    }

    #[test]
    fn key_rotation_forward_path_is_strict_and_terminal() {
        let mut rotation = key_rotation();
        rotation.validate().expect("prepared rotation");
        for (transition, timestamp) in [
            (
                VaultKeyRotationTransition::KeyRingStarted,
                "2026-08-30T13:21:00Z",
            ),
            (
                VaultKeyRotationTransition::AccountsMigrated,
                "2026-08-30T13:22:00Z",
            ),
            (
                VaultKeyRotationTransition::DependenciesCleared,
                "2026-08-30T13:23:00Z",
            ),
            (
                VaultKeyRotationTransition::PredecessorRetired,
                "2026-08-30T13:24:00Z",
            ),
            (
                VaultKeyRotationTransition::Succeeded,
                "2026-08-30T13:25:00Z",
            ),
        ] {
            rotation = rotation
                .apply_transition(
                    transition,
                    UtcTimestamp::parse(timestamp).expect("transition time"),
                )
                .expect("forward transition");
        }
        assert_eq!(rotation.status, VaultKeyRotationStatus::Succeeded);
        assert_eq!(rotation.revision, 6);
        assert!(matches!(
            rotation.apply_transition(
                VaultKeyRotationTransition::BeginRollback {
                    reason_code: "late_rollback".into(),
                },
                UtcTimestamp::parse("2026-08-30T13:26:00Z").unwrap()
            ),
            Err(VaultValidationError::InvalidKeyRotationTransition)
        ));
    }

    #[test]
    fn key_rotation_rollback_is_resumable_and_reason_bounded() {
        let rotation = key_rotation()
            .apply_transition(
                VaultKeyRotationTransition::KeyRingStarted,
                UtcTimestamp::parse("2026-08-30T13:21:00Z").unwrap(),
            )
            .unwrap()
            .apply_transition(
                VaultKeyRotationTransition::BeginRollback {
                    reason_code: "account_cascade_conflict".into(),
                },
                UtcTimestamp::parse("2026-08-30T13:22:00Z").unwrap(),
            )
            .expect("begin rollback");
        let paused = rotation
            .apply_transition(
                VaultKeyRotationTransition::NeedsReview {
                    reason_code: "record_backend_unavailable".into(),
                },
                UtcTimestamp::parse("2026-08-30T13:23:00Z").unwrap(),
            )
            .expect("pause rollback");
        let resumed = paused
            .apply_transition(
                VaultKeyRotationTransition::Resume,
                UtcTimestamp::parse("2026-08-30T13:24:00Z").unwrap(),
            )
            .expect("resume rollback");
        assert_eq!(resumed.status, VaultKeyRotationStatus::RollingBack);
        assert!(resumed.last_error_code.is_some());
        let compensated = resumed
            .apply_transition(
                VaultKeyRotationTransition::AccountsRestored,
                UtcTimestamp::parse("2026-08-30T13:25:00Z").unwrap(),
            )
            .unwrap()
            .apply_transition(
                VaultKeyRotationTransition::NewKeyRetired,
                UtcTimestamp::parse("2026-08-30T13:26:00Z").unwrap(),
            )
            .unwrap()
            .apply_transition(
                VaultKeyRotationTransition::Compensated,
                UtcTimestamp::parse("2026-08-30T13:27:00Z").unwrap(),
            )
            .expect("compensated");
        assert_eq!(compensated.status, VaultKeyRotationStatus::Compensated);
        assert!(matches!(
            key_rotation().apply_transition(
                VaultKeyRotationTransition::BeginRollback {
                    reason_code: "Unsafe Reason".into(),
                },
                UtcTimestamp::parse("2026-08-30T13:21:00Z").unwrap()
            ),
            Err(VaultValidationError::InvalidOperationReason)
        ));
    }

    #[test]
    fn registration_operation_follows_the_durable_happy_path() {
        let operation = registration_operation();
        operation.validate().expect("prepared operation");
        let operation = operation
            .apply_transition(
                VaultOperationTransition::RecordReady,
                UtcTimestamp::parse("2026-08-30T07:21:00Z").expect("timestamp"),
            )
            .expect("record ready");
        let account_id = VaultAccountId::parse("account:v1:018f47a2-8a71-7f4a-9c35-1f4234a73314")
            .expect("account ID");
        let operation = operation
            .apply_transition(
                VaultOperationTransition::MetadataCommitted {
                    account_id: account_id.clone(),
                },
                UtcTimestamp::parse("2026-08-30T07:22:00Z").expect("timestamp"),
            )
            .expect("metadata committed");
        let operation = operation
            .apply_transition(
                VaultOperationTransition::Succeeded,
                UtcTimestamp::parse("2026-08-30T07:23:00Z").expect("timestamp"),
            )
            .expect("succeeded");
        assert_eq!(operation.status, VaultOperationStatus::Succeeded);
        assert_eq!(operation.account_id, Some(account_id));
        assert_eq!(operation.revision, 4);
    }

    #[test]
    fn forget_operation_can_restore_and_compensate_but_cannot_cross_flows() {
        let mut operation = registration_operation();
        operation.kind = VaultOperationKind::ForgetAccount;
        operation.account_id = Some(
            VaultAccountId::parse("account:v1:018f47a2-8a71-7f4a-9c35-1f4234a73314")
                .expect("account ID"),
        );
        operation.expected_account_revision = Some(3);
        operation.validate().expect("forget operation");
        assert!(matches!(
            operation.apply_transition(
                VaultOperationTransition::RecordReady,
                UtcTimestamp::parse("2026-08-30T07:21:00Z").expect("timestamp")
            ),
            Err(VaultValidationError::InvalidOperationTransition)
        ));
        let operation = operation
            .apply_transition(
                VaultOperationTransition::RecordQuarantined,
                UtcTimestamp::parse("2026-08-30T07:21:00Z").expect("timestamp"),
            )
            .expect("record quarantined")
            .apply_transition(
                VaultOperationTransition::RecordRestored,
                UtcTimestamp::parse("2026-08-30T07:22:00Z").expect("timestamp"),
            )
            .expect("record restored")
            .apply_transition(
                VaultOperationTransition::Compensated {
                    reason_code: "metadata_revision_conflict".into(),
                },
                UtcTimestamp::parse("2026-08-30T07:23:00Z").expect("timestamp"),
            )
            .expect("compensated");
        assert_eq!(operation.status, VaultOperationStatus::Compensated);
    }

    #[test]
    fn recovery_pause_is_reason_bounded_resumable_and_time_monotonic() {
        let operation = registration_operation();
        assert!(matches!(
            operation.apply_transition(
                VaultOperationTransition::NeedsReview {
                    reason_code: "Unsafe Reason".into(),
                },
                UtcTimestamp::parse("2026-08-30T07:21:00Z").expect("timestamp")
            ),
            Err(VaultValidationError::InvalidOperationReason)
        ));
        assert!(matches!(
            operation.apply_transition(
                VaultOperationTransition::RecordReady,
                UtcTimestamp::parse("2026-08-30T07:19:59Z").expect("timestamp")
            ),
            Err(VaultValidationError::InvalidOperationTimestamp)
        ));
        let paused = operation
            .apply_transition(
                VaultOperationTransition::NeedsReview {
                    reason_code: "record_backend_unavailable".into(),
                },
                UtcTimestamp::parse("2026-08-30T07:21:00Z").expect("timestamp"),
            )
            .expect("pause for review");
        let resumed = paused
            .apply_transition(
                VaultOperationTransition::Resume,
                UtcTimestamp::parse("2026-08-30T07:22:00Z").expect("timestamp"),
            )
            .expect("resume");
        assert_eq!(resumed.status, VaultOperationStatus::InProgress);
        assert_eq!(resumed.checkpoint, VaultOperationCheckpoint::Prepared);
        assert!(resumed.last_error_code.is_none());
    }

    #[test]
    fn registration_rejects_ambiguous_or_mismatched_metadata() {
        assert!(registration().validate().is_ok());

        let mut whitespace = registration();
        whitespace.display_name = " Primary ChatGPT ".into();
        assert_eq!(
            whitespace.validate(),
            Err(VaultValidationError::InvalidMetadata("display_name"))
        );

        let mut mismatched = registration();
        mismatched.source = VaultAccountSource::ManualApi;
        assert_eq!(
            mismatched.validate(),
            Err(VaultValidationError::SourceAuthMismatch)
        );
    }
}
