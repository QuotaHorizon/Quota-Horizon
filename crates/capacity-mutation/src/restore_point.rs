use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::Read;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use capacity_domain::UtcTimestamp;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const RESTORE_POINT_FORMAT: &str = "quota-horizon-restore-point-v1";
const RESTORE_POINT_ID_PREFIX: &str = "rp-v1-";
const MANIFEST_FILE: &str = "manifest.json";
const PAYLOAD_DIRECTORY: &str = "files";
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const DEFAULT_MAX_RESTORE_POINTS: usize = 5;
const DEFAULT_MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_MAX_TOTAL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedTarget {
    CurrentAuth,
    CurrentConfig,
    ManagerState,
    ProviderConfigBackup,
    ProviderModelCatalog,
    AggregateApiStore,
    ActiveProviderProfile,
    ActiveProviderFieldMetadata,
    ImportedProviderProfile,
    ImportedProviderFieldMetadata,
    PreviousManagedAuth,
    PreviousAccountFieldMetadata,
    PreviousAccountRevision,
    TargetManagedAuth,
    TargetAccountFieldMetadata,
    TargetAccountRevision,
    CanonicalThreadState,
    LegacyThreadState,
    SessionIndex,
}

impl ProtectedTarget {
    fn payload_name(self) -> &'static str {
        match self {
            Self::CurrentAuth => "current-auth.bin",
            Self::CurrentConfig => "current-config.bin",
            Self::ManagerState => "manager-state.bin",
            Self::ProviderConfigBackup => "provider-config-backup.bin",
            Self::ProviderModelCatalog => "provider-model-catalog.bin",
            Self::AggregateApiStore => "aggregate-api-store.bin",
            Self::ActiveProviderProfile => "active-provider-profile.bin",
            Self::ActiveProviderFieldMetadata => "active-provider-field-metadata.bin",
            Self::ImportedProviderProfile => "imported-provider-profile.bin",
            Self::ImportedProviderFieldMetadata => "imported-provider-field-metadata.bin",
            Self::PreviousManagedAuth => "previous-managed-auth.bin",
            Self::PreviousAccountFieldMetadata => "previous-account-field-metadata.bin",
            Self::PreviousAccountRevision => "previous-account-revision.bin",
            Self::TargetManagedAuth => "target-managed-auth.bin",
            Self::TargetAccountFieldMetadata => "target-account-field-metadata.bin",
            Self::TargetAccountRevision => "target-account-revision.bin",
            Self::CanonicalThreadState => "canonical-thread-state.bin",
            Self::LegacyThreadState => "legacy-thread-state.bin",
            Self::SessionIndex => "session-index.bin",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProtectedTargetPath {
    target: ProtectedTarget,
    path: PathBuf,
}

impl ProtectedTargetPath {
    pub fn new(
        target: ProtectedTarget,
        path: impl Into<PathBuf>,
    ) -> Result<Self, RestorePointError> {
        let path = path.into();
        validate_absolute_path(&path).map_err(|_| RestorePointError::InvalidTargetPath(target))?;
        Ok(Self { target, path })
    }

    pub fn target(&self) -> ProtectedTarget {
        self.target
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Debug for ProtectedTargetPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtectedTargetPath")
            .field("target", &self.target)
            .field("path", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RestorePointId(String);

impl RestorePointId {
    pub fn parse(value: impl Into<String>) -> Result<Self, RestorePointError> {
        let value = value.into();
        let Some(uuid) = value.strip_prefix(RESTORE_POINT_ID_PREFIX) else {
            return Err(RestorePointError::InvalidRestorePointId);
        };
        let parsed = Uuid::parse_str(uuid).map_err(|_| RestorePointError::InvalidRestorePointId)?;
        if parsed.hyphenated().to_string() != uuid {
            return Err(RestorePointError::InvalidRestorePointId);
        }
        Ok(Self(value))
    }

    fn generate() -> Self {
        Self(format!("{RESTORE_POINT_ID_PREFIX}{}", Uuid::new_v4()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RestorePointId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RestorePointId(<redacted>)")
    }
}

impl<'de> Deserialize<'de> for RestorePointId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestorePointOperation {
    SwitchAccount,
    DeactivateAccount,
    EnterProviderMode,
    ExitProviderMode,
    EditProviderProfile,
    ImportLegacyProvider,
    EditAggregateApi,
    RepairThreads,
    RollbackLastChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "presence", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetState {
    Absent,
    Present {
        sha256: String,
        size: u64,
        unix_mode: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetObservation {
    target: ProtectedTarget,
    state: TargetState,
}

impl TargetObservation {
    pub fn target(&self) -> ProtectedTarget {
        self.target
    }

    pub fn state(&self) -> &TargetState {
        &self.state
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TargetSnapshot(Vec<TargetObservation>);

impl TargetSnapshot {
    pub fn observations(&self) -> &[TargetObservation] {
        &self.0
    }

    pub fn state(&self, target: ProtectedTarget) -> Option<&TargetState> {
        self.0
            .binary_search_by_key(&target, |observation| observation.target)
            .ok()
            .map(|index| &self.0[index].state)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RestorePointState {
    Prepared,
    Committed {
        committed_at: UtcTimestamp,
        postconditions: TargetSnapshot,
    },
    RolledBack {
        rolled_back_at: UtcTimestamp,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestorePointFile {
    target: ProtectedTarget,
    precondition: TargetState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestorePoint {
    format: String,
    id: RestorePointId,
    created_at: UtcTimestamp,
    operation: RestorePointOperation,
    codex_was_running: bool,
    state: RestorePointState,
    files: Vec<RestorePointFile>,
}

impl RestorePoint {
    pub fn id(&self) -> &RestorePointId {
        &self.id
    }

    pub fn created_at(&self) -> &UtcTimestamp {
        &self.created_at
    }

    pub fn operation(&self) -> RestorePointOperation {
        self.operation
    }

    pub fn codex_was_running(&self) -> bool {
        self.codex_was_running
    }

    pub fn state(&self) -> &RestorePointState {
        &self.state
    }

    pub fn protected_targets(&self) -> impl ExactSizeIterator<Item = ProtectedTarget> + '_ {
        self.files.iter().map(|file| file.target)
    }

    fn preconditions(&self) -> TargetSnapshot {
        TargetSnapshot(
            self.files
                .iter()
                .map(|file| TargetObservation {
                    target: file.target,
                    state: file.precondition.clone(),
                })
                .collect(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePointReceipt {
    pub restore_point_id: RestorePointId,
    pub restored: TargetSnapshot,
    pub already_restored: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestorePointError {
    InvalidRoot,
    InvalidTargetPath(ProtectedTarget),
    DuplicateTarget(ProtectedTarget),
    DuplicatePath,
    TargetInsideRestoreRoot(ProtectedTarget),
    InvalidRestorePointId,
    RestorePointMissing,
    InvalidManifest,
    TargetSetMismatch,
    UnsafeFileType(ProtectedTarget),
    UnsafeRestoreArtifact,
    InsecureRestorePermissions,
    FileTooLarge(ProtectedTarget),
    RestorePointTooLarge,
    CorruptedPayload(ProtectedTarget),
    PreconditionChanged(ProtectedTarget),
    RestorePointNotPrepared,
    RestorePointNotCommitted,
    RollbackCompensationFailed,
    PlatformPermissionsUnavailable,
    Io(std::io::ErrorKind),
}

impl fmt::Display for RestorePointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidRoot => "restore-point root is invalid",
            Self::InvalidTargetPath(_) => "protected target path is invalid",
            Self::DuplicateTarget(_) => "protected target is duplicated",
            Self::DuplicatePath => "multiple protected targets resolve to one path",
            Self::TargetInsideRestoreRoot(_) => "protected target overlaps the restore-point store",
            Self::InvalidRestorePointId => "restore-point ID is invalid",
            Self::RestorePointMissing => "restore point does not exist",
            Self::InvalidManifest => "restore-point manifest is invalid",
            Self::TargetSetMismatch => "protected target set does not match the restore point",
            Self::UnsafeFileType(_) => "protected target has an unsafe file type",
            Self::UnsafeRestoreArtifact => "restore point contains an unsafe artifact",
            Self::InsecureRestorePermissions => "restore-point permissions are not private",
            Self::FileTooLarge(_) => "protected target exceeds the file-size limit",
            Self::RestorePointTooLarge => "restore point exceeds the total-size limit",
            Self::CorruptedPayload(_) => "restore-point payload failed integrity verification",
            Self::PreconditionChanged(_) => "protected target changed after the operation preview",
            Self::RestorePointNotPrepared => "restore point is not in prepared state",
            Self::RestorePointNotCommitted => "restore point is not committed",
            Self::RollbackCompensationFailed => "rollback failed and compensation was incomplete",
            Self::PlatformPermissionsUnavailable => {
                "private restore-point permissions are unavailable"
            }
            Self::Io(_) => "restore-point filesystem operation failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for RestorePointError {}

#[derive(Clone)]
pub struct RestorePointStore {
    root: PathBuf,
    max_restore_points: usize,
    max_file_bytes: u64,
    max_total_bytes: u64,
}

impl fmt::Debug for RestorePointStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RestorePointStore")
            .field("root", &"<redacted>")
            .field("max_restore_points", &self.max_restore_points)
            .finish_non_exhaustive()
    }
}

impl RestorePointStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, RestorePointError> {
        let root = root.into();
        validate_absolute_path(&root).map_err(|_| RestorePointError::InvalidRoot)?;
        Ok(Self {
            root,
            max_restore_points: DEFAULT_MAX_RESTORE_POINTS,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        })
    }

    #[cfg(test)]
    fn with_limits(
        root: impl Into<PathBuf>,
        max_restore_points: usize,
        max_file_bytes: u64,
        max_total_bytes: u64,
    ) -> Result<Self, RestorePointError> {
        let mut store = Self::new(root)?;
        store.max_restore_points = max_restore_points;
        store.max_file_bytes = max_file_bytes;
        store.max_total_bytes = max_total_bytes;
        Ok(store)
    }

    pub fn observe(
        &self,
        targets: &[ProtectedTargetPath],
    ) -> Result<TargetSnapshot, RestorePointError> {
        let targets = self.normalize_targets(targets)?;
        let captures = self.capture_targets(&targets)?;
        Ok(snapshot_from_captures(&captures))
    }

    pub fn create_restore_point(
        &self,
        operation: RestorePointOperation,
        targets: &[ProtectedTargetPath],
        codex_was_running: bool,
        created_at: &UtcTimestamp,
        protected_ids: &BTreeSet<RestorePointId>,
    ) -> Result<RestorePoint, RestorePointError> {
        let targets = self.normalize_targets(targets)?;
        self.prepare_root()?;
        let captures = self.capture_targets(&targets)?;
        let id = RestorePointId::generate();
        let point = RestorePoint {
            format: RESTORE_POINT_FORMAT.to_string(),
            id: id.clone(),
            created_at: created_at.clone(),
            operation,
            codex_was_running,
            state: RestorePointState::Prepared,
            files: captures
                .iter()
                .map(|capture| RestorePointFile {
                    target: capture.target,
                    precondition: capture.state.clone(),
                })
                .collect(),
        };
        let staging = self
            .root
            .join(format!(".capture-{}", Uuid::new_v4().hyphenated()));
        create_private_directory(&staging)?;
        let mut staging_guard = OwnedDirectoryGuard::new(staging.clone(), ".capture-");
        let payload_directory = staging.join(PAYLOAD_DIRECTORY);
        create_private_directory(&payload_directory)?;
        for capture in &captures {
            if let Some(bytes) = &capture.bytes {
                write_private_file_new(
                    &payload_directory.join(capture.target.payload_name()),
                    bytes,
                )?;
            }
        }
        self.write_manifest_new(&staging, &point)?;
        sync_directory(&payload_directory)?;
        sync_directory(&staging)?;
        self.verify_directory(&staging, &point.id)?;

        let destination = self.point_directory(&point.id);
        fs::rename(&staging, &destination).map_err(io_error)?;
        sync_directory(&self.root)?;
        staging_guard.disarm();
        let verified = self.verify(&point.id)?;
        if verified != point {
            return Err(RestorePointError::InvalidManifest);
        }
        self.prune(protected_ids)?;
        Ok(verified)
    }

    pub fn verify(&self, id: &RestorePointId) -> Result<RestorePoint, RestorePointError> {
        self.verify_directory(&self.point_directory(id), id)
    }

    pub fn mark_committed(
        &self,
        id: &RestorePointId,
        targets: &[ProtectedTargetPath],
        committed_at: &UtcTimestamp,
    ) -> Result<RestorePoint, RestorePointError> {
        let mut point = self.verify(id)?;
        let targets = self.normalize_targets(targets)?;
        self.require_exact_target_set(&point, &targets)?;
        let postconditions = self.observe_normalized(&targets)?;
        match &point.state {
            RestorePointState::Prepared => {
                point.state = RestorePointState::Committed {
                    committed_at: committed_at.clone(),
                    postconditions,
                };
                self.replace_manifest(&point)?;
                self.verify(id)
            }
            RestorePointState::Committed {
                postconditions: existing,
                ..
            } if existing == &postconditions => Ok(point),
            _ => Err(RestorePointError::RestorePointNotPrepared),
        }
    }

    pub fn rollback_prepared(
        &self,
        id: &RestorePointId,
        targets: &[ProtectedTargetPath],
        expected_current: &TargetSnapshot,
        rolled_back_at: &UtcTimestamp,
    ) -> Result<RestorePointReceipt, RestorePointError> {
        let point = self.verify(id)?;
        match point.state {
            RestorePointState::Prepared => {
                self.rollback(&point, targets, expected_current, rolled_back_at)
            }
            RestorePointState::RolledBack { .. } => self.already_restored_receipt(&point, targets),
            RestorePointState::Committed { .. } => Err(RestorePointError::RestorePointNotPrepared),
        }
    }

    pub fn rollback_committed(
        &self,
        id: &RestorePointId,
        targets: &[ProtectedTargetPath],
        rolled_back_at: &UtcTimestamp,
    ) -> Result<RestorePointReceipt, RestorePointError> {
        let point = self.verify(id)?;
        match &point.state {
            RestorePointState::Committed { postconditions, .. } => {
                let targets = self.normalize_targets(targets)?;
                self.require_exact_target_set(&point, &targets)?;
                let current = self.observe_normalized(&targets)?;
                let preconditions = point.preconditions();
                if current == preconditions {
                    self.mark_rolled_back(point, rolled_back_at)?;
                    return Ok(RestorePointReceipt {
                        restore_point_id: id.clone(),
                        restored: current,
                        already_restored: true,
                    });
                }
                if &current != postconditions {
                    return Err(first_snapshot_difference(postconditions, &current)
                        .map(RestorePointError::PreconditionChanged)
                        .unwrap_or(RestorePointError::TargetSetMismatch));
                }
                let postconditions = postconditions.clone();
                self.rollback_normalized(
                    point,
                    &targets,
                    &postconditions,
                    rolled_back_at,
                    false,
                    None,
                )
            }
            RestorePointState::RolledBack { .. } => self.already_restored_receipt(&point, targets),
            RestorePointState::Prepared => Err(RestorePointError::RestorePointNotCommitted),
        }
    }

    /// Verifies, without mutating either targets or restore metadata, that a
    /// committed point can still be rolled back. The already-restored
    /// precondition is accepted so an interrupted idempotent retry can finish
    /// by marking the point rolled back.
    pub fn verify_rollback_ready(
        &self,
        id: &RestorePointId,
        targets: &[ProtectedTargetPath],
    ) -> Result<RestorePoint, RestorePointError> {
        let point = self.verify(id)?;
        let targets = self.normalize_targets(targets)?;
        self.require_exact_target_set(&point, &targets)?;
        let current = self.observe_normalized(&targets)?;
        match &point.state {
            RestorePointState::Committed { postconditions, .. }
                if current == *postconditions || current == point.preconditions() =>
            {
                Ok(point)
            }
            RestorePointState::Committed { postconditions, .. } => {
                Err(first_snapshot_difference(postconditions, &current)
                    .map(RestorePointError::PreconditionChanged)
                    .unwrap_or(RestorePointError::TargetSetMismatch))
            }
            RestorePointState::Prepared => Err(RestorePointError::RestorePointNotCommitted),
            RestorePointState::RolledBack { .. } => {
                Err(RestorePointError::RestorePointNotCommitted)
            }
        }
    }

    pub fn latest_committed(&self) -> Result<Option<RestorePoint>, RestorePointError> {
        if !self.root.exists() {
            return Ok(None);
        }
        self.verify_root()?;
        let mut points = Vec::new();
        for (id, _) in self.strict_point_directories()? {
            let point = self.verify(&id)?;
            if matches!(point.state, RestorePointState::Committed { .. }) {
                points.push(point);
            }
        }
        points.sort_by(|left, right| {
            right
                .created_at
                .as_str()
                .cmp(left.created_at.as_str())
                .then_with(|| right.id.as_str().cmp(left.id.as_str()))
        });
        Ok(points.into_iter().next())
    }

    pub fn prune(
        &self,
        protected_ids: &BTreeSet<RestorePointId>,
    ) -> Result<Vec<RestorePointId>, RestorePointError> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        self.verify_root()?;
        let directories = self.strict_point_directories()?;
        if directories.len() <= self.max_restore_points {
            return Ok(Vec::new());
        }

        let mut entries = directories
            .into_iter()
            .map(|(id, path)| {
                let point = self.verify(&id).ok();
                let automatically_protected = point
                    .as_ref()
                    .is_some_and(|point| matches!(point.state, RestorePointState::Prepared));
                let order = point
                    .as_ref()
                    .map(|point| point.created_at.as_str().to_string())
                    .unwrap_or_default();
                let protected = protected_ids.contains(&id) || automatically_protected;
                (id, path, order, protected)
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .2
                .cmp(&left.2)
                .then_with(|| right.0.as_str().cmp(left.0.as_str()))
        });

        let protected_count = entries.iter().filter(|entry| entry.3).count();
        let mut unprotected_slots = self
            .max_restore_points
            .saturating_sub(protected_count.min(self.max_restore_points));
        let mut removed = Vec::new();
        for (id, path, _, protected) in entries {
            if protected {
                continue;
            }
            if unprotected_slots > 0 {
                unprotected_slots -= 1;
                continue;
            }
            remove_owned_restore_directory(&self.root, &path, &id)?;
            removed.push(id);
        }
        if !removed.is_empty() {
            sync_directory(&self.root)?;
        }
        Ok(removed)
    }

    fn rollback(
        &self,
        point: &RestorePoint,
        targets: &[ProtectedTargetPath],
        expected_current: &TargetSnapshot,
        rolled_back_at: &UtcTimestamp,
    ) -> Result<RestorePointReceipt, RestorePointError> {
        let targets = self.normalize_targets(targets)?;
        self.require_exact_target_set(point, &targets)?;
        validate_snapshot(expected_current, &targets)?;
        let current = self.observe_normalized(&targets)?;
        let preconditions = point.preconditions();
        if current == preconditions {
            self.mark_rolled_back(point.clone(), rolled_back_at)?;
            return Ok(RestorePointReceipt {
                restore_point_id: point.id.clone(),
                restored: current,
                already_restored: true,
            });
        }
        if &current != expected_current {
            return Err(first_snapshot_difference(expected_current, &current)
                .map(RestorePointError::PreconditionChanged)
                .unwrap_or(RestorePointError::TargetSetMismatch));
        }
        self.rollback_normalized(
            point.clone(),
            &targets,
            expected_current,
            rolled_back_at,
            false,
            None,
        )
    }

    fn rollback_normalized(
        &self,
        point: RestorePoint,
        targets: &[ProtectedTargetPath],
        expected_current: &TargetSnapshot,
        rolled_back_at: &UtcTimestamp,
        already_restored: bool,
        fail_before_target: Option<usize>,
    ) -> Result<RestorePointReceipt, RestorePointError> {
        let verified_payloads = self.load_verified_payloads(&point)?;
        let current_captures = self.capture_targets(targets)?;
        let current_snapshot = snapshot_from_captures(&current_captures);
        if &current_snapshot != expected_current {
            return Err(
                first_snapshot_difference(expected_current, &current_snapshot)
                    .map(RestorePointError::PreconditionChanged)
                    .unwrap_or(RestorePointError::TargetSetMismatch),
            );
        }

        let mut applied = Vec::new();
        for (index, target) in targets.iter().enumerate() {
            if fail_before_target == Some(index) {
                if compensate_applied(&applied, self.max_file_bytes).is_err() {
                    return Err(RestorePointError::RollbackCompensationFailed);
                }
                return Err(RestorePointError::Io(std::io::ErrorKind::Other));
            }
            let current = current_captures
                .iter()
                .find(|capture| capture.target == target.target)
                .ok_or(RestorePointError::TargetSetMismatch)?;
            let desired = point
                .files
                .iter()
                .find(|file| file.target == target.target)
                .ok_or(RestorePointError::TargetSetMismatch)?;
            let desired_bytes = verified_payloads.get(&target.target).map(Vec::as_slice);
            if let Err(error) = apply_target_state(
                target,
                &current.state,
                &desired.precondition,
                desired_bytes,
                self.max_file_bytes,
            ) {
                if compensate_applied(&applied, self.max_file_bytes).is_err() {
                    return Err(RestorePointError::RollbackCompensationFailed);
                }
                return Err(error);
            }
            applied.push(AppliedTarget {
                target: target.clone(),
                before_state: current.state.clone(),
                before_bytes: current.bytes.clone(),
                applied_state: desired.precondition.clone(),
            });
        }

        let restored = self.observe_normalized(targets)?;
        let preconditions = point.preconditions();
        if restored != preconditions {
            if compensate_applied(&applied, self.max_file_bytes).is_err() {
                return Err(RestorePointError::RollbackCompensationFailed);
            }
            return Err(first_snapshot_difference(&preconditions, &restored)
                .map(RestorePointError::PreconditionChanged)
                .unwrap_or(RestorePointError::TargetSetMismatch));
        }
        self.mark_rolled_back(point.clone(), rolled_back_at)?;
        Ok(RestorePointReceipt {
            restore_point_id: point.id,
            restored,
            already_restored,
        })
    }

    fn already_restored_receipt(
        &self,
        point: &RestorePoint,
        targets: &[ProtectedTargetPath],
    ) -> Result<RestorePointReceipt, RestorePointError> {
        let targets = self.normalize_targets(targets)?;
        self.require_exact_target_set(point, &targets)?;
        let current = self.observe_normalized(&targets)?;
        let preconditions = point.preconditions();
        if current != preconditions {
            return Err(first_snapshot_difference(&preconditions, &current)
                .map(RestorePointError::PreconditionChanged)
                .unwrap_or(RestorePointError::TargetSetMismatch));
        }
        Ok(RestorePointReceipt {
            restore_point_id: point.id.clone(),
            restored: current,
            already_restored: true,
        })
    }

    fn mark_rolled_back(
        &self,
        mut point: RestorePoint,
        rolled_back_at: &UtcTimestamp,
    ) -> Result<(), RestorePointError> {
        point.state = RestorePointState::RolledBack {
            rolled_back_at: rolled_back_at.clone(),
        };
        self.replace_manifest(&point)?;
        let verified = self.verify(&point.id)?;
        if verified != point {
            return Err(RestorePointError::InvalidManifest);
        }
        Ok(())
    }

    fn normalize_targets(
        &self,
        targets: &[ProtectedTargetPath],
    ) -> Result<Vec<ProtectedTargetPath>, RestorePointError> {
        if targets.is_empty() {
            return Err(RestorePointError::TargetSetMismatch);
        }
        let mut normalized = targets.to_vec();
        normalized.sort_by_key(|target| target.target);
        let mut seen_paths = BTreeSet::new();
        let mut previous_target = None;
        for target in &normalized {
            validate_absolute_path(&target.path)
                .map_err(|_| RestorePointError::InvalidTargetPath(target.target))?;
            if previous_target == Some(target.target) {
                return Err(RestorePointError::DuplicateTarget(target.target));
            }
            previous_target = Some(target.target);
            if !seen_paths.insert(target.path.clone()) {
                return Err(RestorePointError::DuplicatePath);
            }
            if target.path.starts_with(&self.root) || self.root.starts_with(&target.path) {
                return Err(RestorePointError::TargetInsideRestoreRoot(target.target));
            }
        }
        Ok(normalized)
    }

    fn capture_targets(
        &self,
        targets: &[ProtectedTargetPath],
    ) -> Result<Vec<CapturedTarget>, RestorePointError> {
        let mut total = 0_u64;
        targets
            .iter()
            .map(|target| {
                let capture = capture_target(target, self.max_file_bytes)?;
                if let TargetState::Present { size, .. } = &capture.state {
                    total = total
                        .checked_add(*size)
                        .ok_or(RestorePointError::RestorePointTooLarge)?;
                    if total > self.max_total_bytes {
                        return Err(RestorePointError::RestorePointTooLarge);
                    }
                }
                Ok(capture)
            })
            .collect()
    }

    fn observe_normalized(
        &self,
        targets: &[ProtectedTargetPath],
    ) -> Result<TargetSnapshot, RestorePointError> {
        Ok(snapshot_from_captures(&self.capture_targets(targets)?))
    }

    fn require_exact_target_set(
        &self,
        point: &RestorePoint,
        targets: &[ProtectedTargetPath],
    ) -> Result<(), RestorePointError> {
        let expected = point
            .files
            .iter()
            .map(|file| file.target)
            .collect::<Vec<_>>();
        let actual = targets
            .iter()
            .map(|target| target.target)
            .collect::<Vec<_>>();
        if expected != actual {
            return Err(RestorePointError::TargetSetMismatch);
        }
        Ok(())
    }

    fn prepare_root(&self) -> Result<(), RestorePointError> {
        match fs::symlink_metadata(&self.root) {
            Ok(_) => self.verify_root(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let parent = self.root.parent().ok_or(RestorePointError::InvalidRoot)?;
                verify_directory(parent, false)?;
                create_private_directory(&self.root)?;
                sync_directory(parent)
            }
            Err(error) => Err(io_error(error)),
        }
    }

    fn verify_root(&self) -> Result<(), RestorePointError> {
        verify_directory(&self.root, true)
    }

    fn point_directory(&self, id: &RestorePointId) -> PathBuf {
        self.root.join(id.as_str())
    }

    fn write_manifest_new(
        &self,
        directory: &Path,
        point: &RestorePoint,
    ) -> Result<(), RestorePointError> {
        let bytes = serialize_manifest(point)?;
        write_private_file_new(&directory.join(MANIFEST_FILE), &bytes)
    }

    fn replace_manifest(&self, point: &RestorePoint) -> Result<(), RestorePointError> {
        let directory = self.point_directory(&point.id);
        verify_directory(&directory, true)?;
        let bytes = serialize_manifest(point)?;
        let temporary = directory.join(format!(".manifest-{}.tmp", Uuid::new_v4().hyphenated()));
        let mut guard = OwnedFileGuard::new(temporary.clone(), ".manifest-");
        write_private_file_new(&temporary, &bytes)?;
        replace_file(&temporary, &directory.join(MANIFEST_FILE))?;
        sync_directory(&directory)?;
        guard.disarm();
        Ok(())
    }

    fn verify_directory(
        &self,
        directory: &Path,
        expected_id: &RestorePointId,
    ) -> Result<RestorePoint, RestorePointError> {
        verify_directory(directory, true)?;
        let manifest_path = directory.join(MANIFEST_FILE);
        let bytes = read_restore_artifact(&manifest_path, MAX_MANIFEST_BYTES)?;
        let point: RestorePoint =
            serde_json::from_slice(&bytes).map_err(|_| RestorePointError::InvalidManifest)?;
        validate_manifest(
            &point,
            expected_id,
            self.max_file_bytes,
            self.max_total_bytes,
        )?;
        let payload_directory = directory.join(PAYLOAD_DIRECTORY);
        verify_directory(&payload_directory, true)?;
        let expected_payloads = point
            .files
            .iter()
            .filter(|file| matches!(file.precondition, TargetState::Present { .. }))
            .map(|file| file.target.payload_name().to_string())
            .collect::<BTreeSet<_>>();
        let actual_payloads = fs::read_dir(&payload_directory)
            .map_err(io_error)?
            .map(|entry| {
                let entry = entry.map_err(io_error)?;
                let file_type = entry.file_type().map_err(io_error)?;
                if !file_type.is_file() || file_type.is_symlink() {
                    return Err(RestorePointError::UnsafeRestoreArtifact);
                }
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| RestorePointError::UnsafeRestoreArtifact)
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        if expected_payloads != actual_payloads {
            return Err(RestorePointError::InvalidManifest);
        }
        self.load_verified_payloads_from(directory, &point)?;
        Ok(point)
    }

    fn load_verified_payloads(
        &self,
        point: &RestorePoint,
    ) -> Result<BTreeMap<ProtectedTarget, Vec<u8>>, RestorePointError> {
        self.load_verified_payloads_from(&self.point_directory(&point.id), point)
    }

    fn load_verified_payloads_from(
        &self,
        directory: &Path,
        point: &RestorePoint,
    ) -> Result<BTreeMap<ProtectedTarget, Vec<u8>>, RestorePointError> {
        let payload_directory = directory.join(PAYLOAD_DIRECTORY);
        point
            .files
            .iter()
            .filter_map(|file| match &file.precondition {
                TargetState::Absent => None,
                TargetState::Present { sha256, size, .. } => {
                    Some((file.target, sha256.as_str(), *size))
                }
            })
            .map(|(target, expected_sha256, expected_size)| {
                let bytes = read_restore_artifact(
                    &payload_directory.join(target.payload_name()),
                    self.max_file_bytes,
                )
                .map_err(|error| match error {
                    RestorePointError::Io(std::io::ErrorKind::NotFound) => {
                        RestorePointError::CorruptedPayload(target)
                    }
                    other => other,
                })?;
                if bytes.len() as u64 != expected_size || sha256(&bytes) != expected_sha256 {
                    return Err(RestorePointError::CorruptedPayload(target));
                }
                Ok((target, bytes))
            })
            .collect()
    }

    fn strict_point_directories(
        &self,
    ) -> Result<Vec<(RestorePointId, PathBuf)>, RestorePointError> {
        let mut directories = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let name = match entry.file_name().into_string() {
                Ok(name) => name,
                Err(_) => continue,
            };
            let Ok(id) = RestorePointId::parse(name) else {
                continue;
            };
            let file_type = entry.file_type().map_err(io_error)?;
            if !file_type.is_dir() || file_type.is_symlink() {
                return Err(RestorePointError::UnsafeRestoreArtifact);
            }
            directories.push((id, entry.path()));
        }
        Ok(directories)
    }
}

#[derive(Clone)]
struct CapturedTarget {
    target: ProtectedTarget,
    state: TargetState,
    bytes: Option<Vec<u8>>,
}

struct AppliedTarget {
    target: ProtectedTargetPath,
    before_state: TargetState,
    before_bytes: Option<Vec<u8>>,
    applied_state: TargetState,
}

fn validate_absolute_path(path: &Path) -> Result<(), ()> {
    if !path.is_absolute() || path.parent().is_none() {
        return Err(());
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::CurDir | Component::ParentDir | Component::Prefix(_)
        )
    }) {
        return Err(());
    }
    Ok(())
}

fn validate_manifest(
    point: &RestorePoint,
    expected_id: &RestorePointId,
    max_file_bytes: u64,
    max_total_bytes: u64,
) -> Result<(), RestorePointError> {
    if point.format != RESTORE_POINT_FORMAT || &point.id != expected_id || point.files.is_empty() {
        return Err(RestorePointError::InvalidManifest);
    }
    let mut total = 0_u64;
    let mut previous = None;
    for file in &point.files {
        if previous.is_some_and(|previous| previous >= file.target) {
            return Err(RestorePointError::InvalidManifest);
        }
        previous = Some(file.target);
        validate_target_state(&file.precondition, max_file_bytes)?;
        if let TargetState::Present { size, .. } = file.precondition {
            total = total
                .checked_add(size)
                .ok_or(RestorePointError::InvalidManifest)?;
        }
    }
    if total > max_total_bytes {
        return Err(RestorePointError::InvalidManifest);
    }
    match &point.state {
        RestorePointState::Prepared | RestorePointState::RolledBack { .. } => {}
        RestorePointState::Committed { postconditions, .. } => {
            if postconditions.0.len() != point.files.len() {
                return Err(RestorePointError::InvalidManifest);
            }
            for (file, observation) in point.files.iter().zip(&postconditions.0) {
                if file.target != observation.target {
                    return Err(RestorePointError::InvalidManifest);
                }
                validate_target_state(&observation.state, max_file_bytes)?;
            }
        }
    }
    Ok(())
}

fn validate_target_state(
    state: &TargetState,
    max_file_bytes: u64,
) -> Result<(), RestorePointError> {
    let TargetState::Present {
        sha256,
        size,
        unix_mode,
    } = state
    else {
        return Ok(());
    };
    if sha256.len() != 64
        || !sha256
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        || *size > max_file_bytes
        || *unix_mode > 0o777
    {
        return Err(RestorePointError::InvalidManifest);
    }
    Ok(())
}

fn validate_snapshot(
    snapshot: &TargetSnapshot,
    targets: &[ProtectedTargetPath],
) -> Result<(), RestorePointError> {
    if snapshot.0.len() != targets.len() {
        return Err(RestorePointError::TargetSetMismatch);
    }
    for (observation, target) in snapshot.0.iter().zip(targets) {
        if observation.target != target.target {
            return Err(RestorePointError::TargetSetMismatch);
        }
    }
    Ok(())
}

fn first_snapshot_difference(
    expected: &TargetSnapshot,
    actual: &TargetSnapshot,
) -> Option<ProtectedTarget> {
    expected
        .0
        .iter()
        .zip(&actual.0)
        .find_map(|(expected, actual)| (expected != actual).then_some(expected.target))
}

fn snapshot_from_captures(captures: &[CapturedTarget]) -> TargetSnapshot {
    TargetSnapshot(
        captures
            .iter()
            .map(|capture| TargetObservation {
                target: capture.target,
                state: capture.state.clone(),
            })
            .collect(),
    )
}

fn capture_target(
    target: &ProtectedTargetPath,
    max_file_bytes: u64,
) -> Result<CapturedTarget, RestorePointError> {
    let before = match fs::symlink_metadata(&target.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CapturedTarget {
                target: target.target,
                state: TargetState::Absent,
                bytes: None,
            });
        }
        Err(error) => return Err(io_error(error)),
    };
    if before.file_type().is_symlink() || !before.is_file() {
        return Err(RestorePointError::UnsafeFileType(target.target));
    }
    if before.len() > max_file_bytes {
        return Err(RestorePointError::FileTooLarge(target.target));
    }
    let file = File::open(&target.path).map_err(io_error)?;
    let opened = file.metadata().map_err(io_error)?;
    if opened.file_type().is_symlink()
        || !opened.is_file()
        || !same_file_observation(&before, &opened)
    {
        return Err(RestorePointError::PreconditionChanged(target.target));
    }
    let mut bytes = Vec::new();
    file.take(max_file_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > max_file_bytes {
        return Err(RestorePointError::FileTooLarge(target.target));
    }
    let after = fs::symlink_metadata(&target.path).map_err(io_error)?;
    if !same_file_observation(&opened, &after) || after.len() != bytes.len() as u64 {
        return Err(RestorePointError::PreconditionChanged(target.target));
    }
    let state = TargetState::Present {
        sha256: sha256(&bytes),
        size: bytes.len() as u64,
        unix_mode: file_mode(&opened)?,
    };
    Ok(CapturedTarget {
        target: target.target,
        state,
        bytes: Some(bytes),
    })
}

fn apply_target_state(
    target: &ProtectedTargetPath,
    expected_current: &TargetState,
    desired: &TargetState,
    desired_bytes: Option<&[u8]>,
    max_file_bytes: u64,
) -> Result<(), RestorePointError> {
    let current = capture_target(target, max_file_bytes)?;
    if &current.state != expected_current {
        return Err(RestorePointError::PreconditionChanged(target.target));
    }
    match desired {
        TargetState::Absent => {
            if matches!(current.state, TargetState::Present { .. }) {
                fs::remove_file(&target.path).map_err(io_error)?;
                sync_parent(&target.path)?;
            }
        }
        TargetState::Present {
            sha256: expected_sha256,
            size,
            unix_mode,
        } => {
            let bytes = desired_bytes.ok_or(RestorePointError::CorruptedPayload(target.target))?;
            if bytes.len() as u64 != *size || sha256(bytes) != *expected_sha256 {
                return Err(RestorePointError::CorruptedPayload(target.target));
            }
            atomic_write_target(&target.path, bytes, *unix_mode)?;
        }
    }
    let applied = capture_target(target, max_file_bytes)?;
    if &applied.state != desired {
        return Err(RestorePointError::PreconditionChanged(target.target));
    }
    Ok(())
}

fn compensate_applied(
    applied: &[AppliedTarget],
    max_file_bytes: u64,
) -> Result<(), RestorePointError> {
    for operation in applied.iter().rev() {
        apply_target_state(
            &operation.target,
            &operation.applied_state,
            &operation.before_state,
            operation.before_bytes.as_deref(),
            max_file_bytes,
        )?;
    }
    Ok(())
}

fn serialize_manifest(point: &RestorePoint) -> Result<Vec<u8>, RestorePointError> {
    let bytes = serde_json::to_vec_pretty(point).map_err(|_| RestorePointError::InvalidManifest)?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(RestorePointError::InvalidManifest);
    }
    Ok(bytes)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_restore_artifact(path: &Path, maximum: u64) -> Result<Vec<u8>, RestorePointError> {
    let before = fs::symlink_metadata(path).map_err(io_error)?;
    verify_private_file_metadata(&before)?;
    if before.len() > maximum {
        return Err(RestorePointError::UnsafeRestoreArtifact);
    }
    let file = File::open(path).map_err(io_error)?;
    let opened = file.metadata().map_err(io_error)?;
    verify_private_file_metadata(&opened)?;
    if !same_file_observation(&before, &opened) {
        return Err(RestorePointError::UnsafeRestoreArtifact);
    }
    let mut bytes = Vec::new();
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > maximum {
        return Err(RestorePointError::UnsafeRestoreArtifact);
    }
    let after = fs::symlink_metadata(path).map_err(io_error)?;
    if !same_file_observation(&opened, &after) || after.len() != bytes.len() as u64 {
        return Err(RestorePointError::UnsafeRestoreArtifact);
    }
    Ok(bytes)
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<(), RestorePointError> {
    let mut builder = fs::DirBuilder::new();
    builder.mode(PRIVATE_DIRECTORY_MODE);
    builder.create(path).map_err(io_error)?;
    verify_directory(path, true)
}

#[cfg(not(unix))]
fn create_private_directory(_path: &Path) -> Result<(), RestorePointError> {
    Err(RestorePointError::PlatformPermissionsUnavailable)
}

#[cfg(unix)]
fn write_private_file_new(path: &Path, bytes: &[u8]) -> Result<(), RestorePointError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE_MODE)
        .open(path)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    verify_private_file_metadata(&file.metadata().map_err(io_error)?)
}

#[cfg(not(unix))]
fn write_private_file_new(_path: &Path, _bytes: &[u8]) -> Result<(), RestorePointError> {
    Err(RestorePointError::PlatformPermissionsUnavailable)
}

fn verify_directory(path: &Path, require_private: bool) -> Result<(), RestorePointError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RestorePointError::UnsafeRestoreArtifact);
    }
    if require_private {
        verify_private_directory_metadata(&metadata)?;
    }
    Ok(())
}

#[cfg(unix)]
fn verify_private_directory_metadata(metadata: &Metadata) -> Result<(), RestorePointError> {
    if metadata.permissions().mode() & 0o777 != PRIVATE_DIRECTORY_MODE {
        return Err(RestorePointError::InsecureRestorePermissions);
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_directory_metadata(_metadata: &Metadata) -> Result<(), RestorePointError> {
    Err(RestorePointError::PlatformPermissionsUnavailable)
}

fn verify_private_file_metadata(metadata: &Metadata) -> Result<(), RestorePointError> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(RestorePointError::UnsafeRestoreArtifact);
    }
    verify_private_file_mode(metadata)
}

#[cfg(unix)]
fn verify_private_file_mode(metadata: &Metadata) -> Result<(), RestorePointError> {
    if metadata.permissions().mode() & 0o777 != PRIVATE_FILE_MODE {
        return Err(RestorePointError::InsecureRestorePermissions);
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_file_mode(_metadata: &Metadata) -> Result<(), RestorePointError> {
    Err(RestorePointError::PlatformPermissionsUnavailable)
}

#[cfg(unix)]
fn file_mode(metadata: &Metadata) -> Result<u32, RestorePointError> {
    Ok(metadata.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn file_mode(_metadata: &Metadata) -> Result<u32, RestorePointError> {
    Err(RestorePointError::PlatformPermissionsUnavailable)
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

#[cfg(unix)]
fn same_file_observation(left: &Metadata, right: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    same_file_identity(left, right)
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(not(unix))]
fn same_file_observation(left: &Metadata, right: &Metadata) -> bool {
    same_file_identity(left, right)
        && left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
}

#[cfg(unix)]
fn atomic_write_target(path: &Path, bytes: &[u8], mode: u32) -> Result<(), RestorePointError> {
    let parent = path.parent().ok_or(RestorePointError::InvalidRoot)?;
    verify_directory(parent, false)?;
    let temporary = parent.join(format!(
        ".quota-horizon-restore-{}.tmp",
        Uuid::new_v4().hyphenated()
    ));
    let mut guard = OwnedFileGuard::new(temporary.clone(), ".quota-horizon-restore-");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE_MODE)
        .open(&temporary)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    replace_file(&temporary, path)?;
    sync_directory(parent)?;
    guard.disarm();
    Ok(())
}

#[cfg(not(unix))]
fn atomic_write_target(_path: &Path, _bytes: &[u8], _mode: u32) -> Result<(), RestorePointError> {
    Err(RestorePointError::PlatformPermissionsUnavailable)
}

fn replace_file(source: &Path, destination: &Path) -> Result<(), RestorePointError> {
    #[cfg(unix)]
    {
        fs::rename(source, destination).map_err(io_error)
    }
    #[cfg(not(unix))]
    {
        let _ = (source, destination);
        Err(RestorePointError::PlatformPermissionsUnavailable)
    }
}

fn sync_parent(path: &Path) -> Result<(), RestorePointError> {
    sync_directory(path.parent().ok_or(RestorePointError::InvalidRoot)?)
}

fn sync_directory(path: &Path) -> Result<(), RestorePointError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)
}

fn remove_owned_restore_directory(
    root: &Path,
    directory: &Path,
    id: &RestorePointId,
) -> Result<(), RestorePointError> {
    if directory.parent() != Some(root)
        || directory.file_name().and_then(|name| name.to_str()) != Some(id.as_str())
    {
        return Err(RestorePointError::UnsafeRestoreArtifact);
    }
    verify_directory(directory, true)?;
    fs::remove_dir_all(directory).map_err(io_error)
}

fn io_error(error: std::io::Error) -> RestorePointError {
    RestorePointError::Io(error.kind())
}

struct OwnedDirectoryGuard {
    path: PathBuf,
    prefix: &'static str,
    armed: bool,
}

impl OwnedDirectoryGuard {
    fn new(path: PathBuf, prefix: &'static str) -> Self {
        Self {
            path,
            prefix,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for OwnedDirectoryGuard {
    fn drop(&mut self) {
        if self.armed
            && self
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(self.prefix))
            && fs::symlink_metadata(&self.path)
                .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

struct OwnedFileGuard {
    path: PathBuf,
    prefix: &'static str,
    armed: bool,
}

impl OwnedFileGuard {
    fn new(path: PathBuf, prefix: &'static str) -> Self {
        Self {
            path,
            prefix,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for OwnedFileGuard {
    fn drop(&mut self) {
        if self.armed
            && self
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(self.prefix))
            && fs::symlink_metadata(&self.path)
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use capacity_domain::UtcTimestamp;
    use tempfile::TempDir;

    use super::*;

    struct Harness {
        _temporary: TempDir,
        store: RestorePointStore,
        auth: PathBuf,
        config: PathBuf,
    }

    impl Harness {
        fn new() -> Self {
            let temporary = tempfile::tempdir().expect("temporary directory");
            let base = temporary.path();
            let targets = base.join("targets");
            fs::create_dir(&targets).expect("target directory");
            let store_root = base.join("restore-points");
            let store = RestorePointStore::new(store_root).expect("restore store");
            Self {
                _temporary: temporary,
                store,
                auth: targets.join("auth.json"),
                config: targets.join("config.toml"),
            }
        }

        fn targets(&self) -> Vec<ProtectedTargetPath> {
            vec![
                ProtectedTargetPath::new(ProtectedTarget::CurrentAuth, &self.auth).unwrap(),
                ProtectedTargetPath::new(ProtectedTarget::CurrentConfig, &self.config).unwrap(),
            ]
        }
    }

    fn timestamp(value: &str) -> UtcTimestamp {
        UtcTimestamp::parse(value).expect("timestamp")
    }

    fn create_point(harness: &Harness) -> RestorePoint {
        harness
            .store
            .create_restore_point(
                RestorePointOperation::SwitchAccount,
                &harness.targets(),
                true,
                &timestamp("2026-08-31T01:00:00Z"),
                &BTreeSet::new(),
            )
            .expect("create restore point")
    }

    #[test]
    fn captures_present_and_absent_targets_without_persisting_paths() {
        let harness = Harness::new();
        fs::write(&harness.auth, b"before-auth").unwrap();
        fs::set_permissions(&harness.auth, fs::Permissions::from_mode(0o640)).unwrap();

        let point = create_point(&harness);

        assert!(matches!(point.state(), RestorePointState::Prepared));
        assert_eq!(
            point.protected_targets().collect::<Vec<_>>(),
            vec![ProtectedTarget::CurrentAuth, ProtectedTarget::CurrentConfig]
        );
        let manifest = fs::read_to_string(
            harness
                .store
                .point_directory(point.id())
                .join(MANIFEST_FILE),
        )
        .unwrap();
        assert!(!manifest.contains(harness.auth.to_str().unwrap()));
        assert!(!manifest.contains(harness.config.to_str().unwrap()));
        assert_eq!(
            fs::metadata(harness.store.root.clone())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(
                harness
                    .store
                    .point_directory(point.id())
                    .join(PAYLOAD_DIRECTORY)
                    .join(ProtectedTarget::CurrentAuth.payload_name())
            )
            .unwrap()
            .permissions()
            .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn committed_rollback_restores_bytes_absence_and_mode_idempotently() {
        let harness = Harness::new();
        fs::write(&harness.auth, b"before-auth").unwrap();
        fs::set_permissions(&harness.auth, fs::Permissions::from_mode(0o640)).unwrap();
        let point = create_point(&harness);

        fs::write(&harness.auth, b"after-auth").unwrap();
        fs::set_permissions(&harness.auth, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&harness.config, b"after-config").unwrap();
        let committed = harness
            .store
            .mark_committed(
                point.id(),
                &harness.targets(),
                &timestamp("2026-08-31T01:01:00Z"),
            )
            .unwrap();
        assert!(matches!(
            committed.state(),
            RestorePointState::Committed { .. }
        ));
        assert_eq!(
            harness
                .store
                .verify_rollback_ready(point.id(), &harness.targets())
                .unwrap(),
            committed
        );

        let first = harness
            .store
            .rollback_committed(
                point.id(),
                &harness.targets(),
                &timestamp("2026-08-31T01:02:00Z"),
            )
            .unwrap();
        assert!(!first.already_restored);
        assert_eq!(fs::read(&harness.auth).unwrap(), b"before-auth");
        assert_eq!(
            fs::metadata(&harness.auth).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(!harness.config.exists());

        let second = harness
            .store
            .rollback_committed(
                point.id(),
                &harness.targets(),
                &timestamp("2026-08-31T01:03:00Z"),
            )
            .unwrap();
        assert!(second.already_restored);
    }

    #[test]
    fn refuses_committed_rollback_after_an_external_change() {
        let harness = Harness::new();
        fs::write(&harness.auth, b"before").unwrap();
        let point = create_point(&harness);
        fs::write(&harness.auth, b"after").unwrap();
        harness
            .store
            .mark_committed(
                point.id(),
                &harness.targets(),
                &timestamp("2026-08-31T01:01:00Z"),
            )
            .unwrap();
        fs::write(&harness.auth, b"external-change").unwrap();

        assert_eq!(
            harness
                .store
                .verify_rollback_ready(point.id(), &harness.targets())
                .unwrap_err(),
            RestorePointError::PreconditionChanged(ProtectedTarget::CurrentAuth)
        );
        assert_eq!(
            harness
                .store
                .rollback_committed(
                    point.id(),
                    &harness.targets(),
                    &timestamp("2026-08-31T01:02:00Z"),
                )
                .unwrap_err(),
            RestorePointError::PreconditionChanged(ProtectedTarget::CurrentAuth)
        );
        assert_eq!(fs::read(&harness.auth).unwrap(), b"external-change");
    }

    #[test]
    fn corrupted_payload_is_rejected_before_any_target_changes() {
        let harness = Harness::new();
        fs::write(&harness.auth, b"before-auth").unwrap();
        fs::write(&harness.config, b"before-config").unwrap();
        let point = create_point(&harness);
        fs::write(&harness.auth, b"after-auth").unwrap();
        fs::write(&harness.config, b"after-config").unwrap();
        harness
            .store
            .mark_committed(
                point.id(),
                &harness.targets(),
                &timestamp("2026-08-31T01:01:00Z"),
            )
            .unwrap();
        let payload = harness
            .store
            .point_directory(point.id())
            .join(PAYLOAD_DIRECTORY)
            .join(ProtectedTarget::CurrentConfig.payload_name());
        fs::write(&payload, b"corrupted").unwrap();
        fs::set_permissions(&payload, fs::Permissions::from_mode(0o600)).unwrap();

        assert_eq!(
            harness
                .store
                .rollback_committed(
                    point.id(),
                    &harness.targets(),
                    &timestamp("2026-08-31T01:02:00Z"),
                )
                .unwrap_err(),
            RestorePointError::CorruptedPayload(ProtectedTarget::CurrentConfig)
        );
        assert_eq!(fs::read(&harness.auth).unwrap(), b"after-auth");
        assert_eq!(fs::read(&harness.config).unwrap(), b"after-config");
    }

    #[test]
    fn partial_multi_file_restore_compensates_already_applied_targets() {
        let harness = Harness::new();
        fs::write(&harness.auth, b"before-auth").unwrap();
        fs::write(&harness.config, b"before-config").unwrap();
        let point = create_point(&harness);
        fs::write(&harness.auth, b"after-auth").unwrap();
        fs::write(&harness.config, b"after-config").unwrap();
        let committed = harness
            .store
            .mark_committed(
                point.id(),
                &harness.targets(),
                &timestamp("2026-08-31T01:01:00Z"),
            )
            .unwrap();
        let targets = harness.store.normalize_targets(&harness.targets()).unwrap();
        let postconditions = match committed.state() {
            RestorePointState::Committed { postconditions, .. } => postconditions.clone(),
            _ => panic!("restore point must be committed"),
        };

        assert_eq!(
            harness
                .store
                .rollback_normalized(
                    committed,
                    &targets,
                    &postconditions,
                    &timestamp("2026-08-31T01:02:00Z"),
                    false,
                    Some(1),
                )
                .unwrap_err(),
            RestorePointError::Io(std::io::ErrorKind::Other)
        );
        assert_eq!(fs::read(&harness.auth).unwrap(), b"after-auth");
        assert_eq!(fs::read(&harness.config).unwrap(), b"after-config");
        assert!(matches!(
            harness.store.verify(point.id()).unwrap().state(),
            RestorePointState::Committed { .. }
        ));
    }

    #[test]
    fn prepared_rollback_requires_exact_failure_snapshot() {
        let harness = Harness::new();
        fs::write(&harness.auth, b"before").unwrap();
        let point = create_point(&harness);
        fs::write(&harness.auth, b"partial-write").unwrap();
        let expected = harness.store.observe(&harness.targets()).unwrap();
        fs::write(&harness.auth, b"raced-write").unwrap();

        assert_eq!(
            harness
                .store
                .rollback_prepared(
                    point.id(),
                    &harness.targets(),
                    &expected,
                    &timestamp("2026-08-31T01:02:00Z"),
                )
                .unwrap_err(),
            RestorePointError::PreconditionChanged(ProtectedTarget::CurrentAuth)
        );
        assert_eq!(fs::read(&harness.auth).unwrap(), b"raced-write");
    }

    #[test]
    fn rejects_symlink_targets_and_duplicate_coverage() {
        let harness = Harness::new();
        fs::write(&harness.auth, b"secret").unwrap();
        symlink(&harness.auth, &harness.config).unwrap();
        assert_eq!(
            harness.store.observe(&harness.targets()).unwrap_err(),
            RestorePointError::UnsafeFileType(ProtectedTarget::CurrentConfig)
        );

        let duplicates = vec![
            ProtectedTargetPath::new(ProtectedTarget::CurrentAuth, &harness.auth).unwrap(),
            ProtectedTargetPath::new(ProtectedTarget::CurrentConfig, &harness.auth).unwrap(),
        ];
        assert_eq!(
            harness.store.observe(&duplicates).unwrap_err(),
            RestorePointError::DuplicatePath
        );
    }

    #[test]
    fn enforces_file_and_total_size_limits_without_creating_store() {
        let harness = Harness::new();
        fs::write(&harness.auth, b"12345").unwrap();
        let limited = RestorePointStore::with_limits(harness.store.root.clone(), 5, 4, 8).unwrap();
        assert_eq!(
            limited
                .create_restore_point(
                    RestorePointOperation::SwitchAccount,
                    &harness.targets(),
                    false,
                    &timestamp("2026-08-31T01:00:00Z"),
                    &BTreeSet::new(),
                )
                .unwrap_err(),
            RestorePointError::FileTooLarge(ProtectedTarget::CurrentAuth)
        );
        assert!(limited.root.read_dir().unwrap().next().is_none());
    }

    #[test]
    fn pruning_keeps_prepared_and_explicitly_protected_points() {
        let harness = Harness::new();
        fs::write(&harness.auth, b"seed").unwrap();
        let store = RestorePointStore::with_limits(
            harness.store.root.clone(),
            2,
            DEFAULT_MAX_FILE_BYTES,
            DEFAULT_MAX_TOTAL_BYTES,
        )
        .unwrap();
        let targets = harness.targets();
        let first = store
            .create_restore_point(
                RestorePointOperation::SwitchAccount,
                &targets,
                false,
                &timestamp("2026-08-31T01:00:00Z"),
                &BTreeSet::new(),
            )
            .unwrap();
        fs::write(&harness.auth, b"one").unwrap();
        store
            .mark_committed(first.id(), &targets, &timestamp("2026-08-31T01:00:30Z"))
            .unwrap();
        let second = store
            .create_restore_point(
                RestorePointOperation::SwitchAccount,
                &targets,
                false,
                &timestamp("2026-08-31T01:01:00Z"),
                &BTreeSet::new(),
            )
            .unwrap();
        fs::write(&harness.auth, b"two").unwrap();
        store
            .mark_committed(second.id(), &targets, &timestamp("2026-08-31T01:01:30Z"))
            .unwrap();
        let protected = BTreeSet::from([first.id().clone()]);
        let third = store
            .create_restore_point(
                RestorePointOperation::SwitchAccount,
                &targets,
                false,
                &timestamp("2026-08-31T01:02:00Z"),
                &protected,
            )
            .unwrap();

        assert!(store.point_directory(first.id()).exists());
        assert!(!store.point_directory(second.id()).exists());
        assert!(store.point_directory(third.id()).exists());
    }

    #[test]
    fn latest_committed_fails_closed_instead_of_skipping_a_corrupt_point() {
        let harness = Harness::new();
        fs::write(&harness.auth, b"before").unwrap();
        let point = create_point(&harness);
        fs::write(&harness.auth, b"after").unwrap();
        harness
            .store
            .mark_committed(
                point.id(),
                &harness.targets(),
                &timestamp("2026-08-31T01:01:00Z"),
            )
            .unwrap();
        let manifest = harness
            .store
            .point_directory(point.id())
            .join(MANIFEST_FILE);
        fs::write(&manifest, b"not-json").unwrap();
        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();

        assert_eq!(
            harness.store.latest_committed().unwrap_err(),
            RestorePointError::InvalidManifest
        );
    }

    #[test]
    fn restore_point_ids_and_debug_output_are_strict_and_redacted() {
        let id = RestorePointId::parse("rp-v1-018f47a2-8a71-7f4a-9c35-1f4234a73501").unwrap();
        assert!(format!("{id:?}").contains("<redacted>"));
        for invalid in [
            "018f47a2-8a71-7f4a-9c35-1f4234a73501",
            "rp-v1-018F47A2-8A71-7F4A-9C35-1F4234A73501",
            "rp-v1-not-a-uuid",
        ] {
            assert_eq!(
                RestorePointId::parse(invalid).unwrap_err(),
                RestorePointError::InvalidRestorePointId
            );
        }
    }
}
