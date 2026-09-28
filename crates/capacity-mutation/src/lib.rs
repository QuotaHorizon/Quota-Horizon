//! Cross-process admission boundary for explicit local mutations.
//!
//! The lock database is intentionally separate from product data. A live
//! `BEGIN IMMEDIATE` transaction is the OS-released ownership primitive; a
//! bounded owner marker exists only to classify active versus stale work after
//! a crash. This crate does not expose arbitrary paths to a WebView and does
//! not execute recovery outside a live, revalidated lock capability.

use std::fmt;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::path::Component;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use capacity_domain::{UtcTimestamp, VaultAccount, VaultAccountRegistration};
use capacity_store::CapacityStore;
pub use capacity_vault::VaultRecoveryAction;
use capacity_vault::{
    AccountVaultError, JournaledAccountRegistrationReceipt, JournaledForgetAccountReceipt,
    ProtectedRecordBackend, ProtectedVaultRecord, journaled_forget_account,
    journaled_register_account,
};
use rusqlite::{Connection, ErrorCode, OpenFlags, params};

mod reconciliation;
mod recovery;
mod restore_point;
mod rotation;

pub use reconciliation::{
    VaultReconciliationArtifact, VaultReconciliationDisposition, VaultReconciliationError,
    VaultReconciliationItem, VaultReconciliationReport,
};
pub use recovery::{
    VaultRecoveryDisposition, VaultRecoveryExecutionError, VaultRecoveryExecutionItem,
    VaultRecoveryExecutionReport,
};
pub use restore_point::{
    ProtectedTarget, ProtectedTargetPath, RestorePoint, RestorePointError, RestorePointId,
    RestorePointOperation, RestorePointReceipt, RestorePointState, RestorePointStore,
    TargetObservation, TargetSnapshot, TargetState,
};
pub use rotation::{
    LockedVaultKeyRotationError, VaultKeyRotationDisposition, VaultKeyRotationExecutionItem,
    VaultKeyRotationExecutionReceipt, VaultKeyRotationRecoveryReport,
    VaultKeyRotationRollbackRequest,
};

const LOCK_DATABASE_FILE: &str = "mutation-lock-v1.sqlite3";
const OWNER_MARKER_FILE: &str = "mutation-owner-v1";
const MACOS_SHARED_LOCK_DIRECTORY: &str = "CodexMutationLock-v1";
const OWNER_MARKER_FORMAT: &str = "capacity-mutation-owner-v1";
const LOCK_PROTOCOL_VERSION: &str = "capacity-mutation-lock-v1";
const LEGACY_VIEWER_COMMAND_PATTERN: &str = "(^|/)CodexQuotaViewer( |$)";
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const MAX_OWNER_MARKER_BYTES: u64 = 512;
const MAX_OWNER_TEMP_ATTEMPTS: u64 = 16;
const OWNER_ID_PREFIX: &str = "mutation-owner:v1:";
const LOCK_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS lock_protocol (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    protocol_version TEXT NOT NULL CHECK(protocol_version = 'capacity-mutation-lock-v1')
) STRICT";
const EXPECTED_LOCK_SCHEMA: &str = "CREATE TABLE lock_protocol (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    protocol_version TEXT NOT NULL CHECK(protocol_version = 'capacity-mutation-lock-v1')
) STRICT";

static OWNER_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Resolves the frozen macOS coordination root shared with the legacy
/// CodexQuotaViewer process. The home directory comes from the trusted desktop
/// adapter; it is never accepted from the WebView.
pub fn macos_shared_mutation_lock_root(
    home_directory: impl AsRef<Path>,
) -> Result<PathBuf, MutationLockError> {
    let root = home_directory
        .as_ref()
        .join("Library")
        .join("Application Support")
        .join(MACOS_SHARED_LOCK_DIRECTORY);
    #[cfg(unix)]
    validate_root_path(&root)?;
    #[cfg(not(unix))]
    if !root.is_absolute() {
        return Err(MutationLockError::InvalidRoot);
    }
    Ok(root)
}

#[derive(Clone, PartialEq, Eq)]
pub struct MutationOwnerId(String);

impl MutationOwnerId {
    pub fn parse(value: impl Into<String>) -> Result<Self, MutationLockError> {
        let value = value.into();
        let Some(uuid) = value.strip_prefix(OWNER_ID_PREFIX) else {
            return Err(MutationLockError::InvalidOwnerId);
        };
        if !is_lowercase_uuid(uuid) {
            return Err(MutationLockError::InvalidOwnerId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MutationOwnerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MutationOwnerId(<redacted>)")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct MutationLockOwner {
    owner_id: MutationOwnerId,
    process_id: u32,
    started_at: UtcTimestamp,
}

impl MutationLockOwner {
    pub fn new(
        owner_id: MutationOwnerId,
        process_id: u32,
        started_at: UtcTimestamp,
    ) -> Result<Self, MutationLockError> {
        if process_id == 0 || process_id > i32::MAX as u32 {
            return Err(MutationLockError::InvalidProcessId);
        }
        Ok(Self {
            owner_id,
            process_id,
            started_at,
        })
    }

    pub fn owner_id(&self) -> &MutationOwnerId {
        &self.owner_id
    }

    pub fn process_id(&self) -> u32 {
        self.process_id
    }

    pub fn started_at(&self) -> &UtcTimestamp {
        &self.started_at
    }
}

impl fmt::Debug for MutationLockOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MutationLockOwner")
            .field("owner_id", &self.owner_id)
            .field("process_id", &self.process_id)
            .field("started_at", &self.started_at)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyViewerState {
    NotRunning,
    Running,
    Unavailable,
}

pub trait LegacyViewerProbe {
    fn legacy_viewer_state(&mut self) -> LegacyViewerState;
}

/// Fixed-command macOS process probe for the legacy first-party Viewer. No
/// caller-controlled command, argument, environment value, or output crosses
/// this adapter. Other platforms report `NotRunning` because the legacy
/// AppKit executable cannot run there.
#[derive(Debug, Default)]
pub struct SystemLegacyViewerProbe;

impl LegacyViewerProbe for SystemLegacyViewerProbe {
    fn legacy_viewer_state(&mut self) -> LegacyViewerState {
        #[cfg(target_os = "macos")]
        {
            let status = Command::new("/usr/bin/pgrep")
                .args(["-f", LEGACY_VIEWER_COMMAND_PATTERN])
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            match status {
                Ok(status) if status.success() => LegacyViewerState::Running,
                Ok(status) if status.code() == Some(1) => LegacyViewerState::NotRunning,
                Ok(_) | Err(_) => LegacyViewerState::Unavailable,
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = LEGACY_VIEWER_COMMAND_PATTERN;
            LegacyViewerState::NotRunning
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationLockState {
    Unlocked,
    Active,
    Stale,
    Unverifiable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationLockInspection {
    pub state: MutationLockState,
    pub owner: Option<MutationLockOwner>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationLockError {
    InvalidRoot,
    PlatformPermissionsUnavailable,
    UnsafeFileType,
    InsecurePermissions,
    InvalidOwnerId,
    InvalidProcessId,
    LockDatabaseUnavailable,
    LockDatabaseCorrupt,
    OwnerMarkerCorrupt,
    OwnerMarkerUnavailable,
    LeaseLost,
    Busy,
    LegacyViewerRunning,
    LegacyViewerStateUnavailable,
    Io(std::io::ErrorKind),
}

impl fmt::Display for MutationLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidRoot => "mutation lock root is invalid",
            Self::PlatformPermissionsUnavailable => {
                "private mutation-lock permissions are unavailable"
            }
            Self::UnsafeFileType => "mutation lock path has an unsafe file type",
            Self::InsecurePermissions => "mutation lock permissions are not private",
            Self::InvalidOwnerId => "mutation lock owner ID is invalid",
            Self::InvalidProcessId => "mutation lock process ID is invalid",
            Self::LockDatabaseUnavailable => "mutation lock database is unavailable",
            Self::LockDatabaseCorrupt => "mutation lock database failed verification",
            Self::OwnerMarkerCorrupt => "mutation lock owner marker is corrupt",
            Self::OwnerMarkerUnavailable => "mutation lock owner marker is unavailable",
            Self::LeaseLost => "mutation lock ownership was lost",
            Self::Busy => "another mutation operation holds the global lock",
            Self::LegacyViewerRunning => "the legacy QuotaViewer is running",
            Self::LegacyViewerStateUnavailable => "the legacy QuotaViewer state cannot be verified",
            Self::Io(_) => "mutation lock filesystem operation failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for MutationLockError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockedVaultMutationError {
    Lock(MutationLockError),
    Vault(AccountVaultError),
}

impl fmt::Display for LockedVaultMutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(formatter),
            Self::Vault(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for LockedVaultMutationError {}

pub struct MutationLock {
    connection: Option<Connection>,
    root: PathBuf,
    root_identity: Metadata,
    database_identity: Metadata,
    owner: MutationLockOwner,
}

impl fmt::Debug for MutationLock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MutationLock")
            .field("root", &"<redacted>")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl MutationLock {
    /// Acquires the per-user mutation lock after checking the legacy Viewer,
    /// then checks the Viewer again after ownership is established. A stale,
    /// valid marker is replaced only after SQLite proves no live owner exists.
    pub fn acquire<P: LegacyViewerProbe>(
        root: impl AsRef<Path>,
        owner: MutationLockOwner,
        legacy_probe: &mut P,
    ) -> Result<Self, MutationLockError> {
        require_legacy_viewer_stopped(legacy_probe)?;
        let root = root.as_ref().to_path_buf();
        prepare_lock_root(&root)?;
        let root_identity = verify_private_directory(&root)?;
        let connection = open_or_initialize_lock_database(&root)?;
        let database_identity = verify_private_file(&root.join(LOCK_DATABASE_FILE))?;
        begin_immediate(&connection)?;

        if let Err(error) = require_legacy_viewer_stopped(legacy_probe) {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(error);
        }
        if let Err(error) = write_owner_marker(&root, &owner) {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(error);
        }
        let current_root = verify_private_directory(&root)?;
        let current_database = verify_private_file(&root.join(LOCK_DATABASE_FILE))?;
        if !same_file_identity(&root_identity, &current_root)
            || !same_file_identity(&database_identity, &current_database)
        {
            let _ = connection.execute_batch("ROLLBACK");
            return Err(MutationLockError::LeaseLost);
        }
        Ok(Self {
            connection: Some(connection),
            root,
            root_identity,
            database_identity,
            owner,
        })
    }

    /// Read-only classification of the OS lock and its bounded owner marker.
    /// `Stale` means the marker remains but `BEGIN IMMEDIATE` is available.
    pub fn inspect(root: impl AsRef<Path>) -> Result<MutationLockInspection, MutationLockError> {
        let root = root.as_ref();
        match fs::symlink_metadata(root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(MutationLockInspection {
                    state: MutationLockState::Unlocked,
                    owner: None,
                });
            }
            Err(error) => return Err(MutationLockError::Io(error.kind())),
            Ok(metadata) => verify_private_directory_metadata(&metadata)?,
        }
        let database_path = root.join(LOCK_DATABASE_FILE);
        match fs::symlink_metadata(&database_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return match read_owner_marker(root) {
                    Ok(None) => Ok(MutationLockInspection {
                        state: MutationLockState::Unlocked,
                        owner: None,
                    }),
                    Ok(Some(owner)) => Ok(MutationLockInspection {
                        state: MutationLockState::Unverifiable,
                        owner: Some(owner),
                    }),
                    Err(_) => Ok(MutationLockInspection {
                        state: MutationLockState::Unverifiable,
                        owner: None,
                    }),
                };
            }
            Err(error) => return Err(MutationLockError::Io(error.kind())),
            Ok(metadata) => verify_private_file_metadata(&metadata)?,
        }

        let connection = open_existing_lock_database(root)?;
        match begin_immediate(&connection) {
            Ok(()) => {
                let marker = read_owner_marker(root);
                let _ = connection.execute_batch("ROLLBACK");
                match marker {
                    Ok(None) => Ok(MutationLockInspection {
                        state: MutationLockState::Unlocked,
                        owner: None,
                    }),
                    Ok(Some(owner)) => Ok(MutationLockInspection {
                        state: MutationLockState::Stale,
                        owner: Some(owner),
                    }),
                    Err(_) => Ok(MutationLockInspection {
                        state: MutationLockState::Unverifiable,
                        owner: None,
                    }),
                }
            }
            Err(MutationLockError::Busy) => match read_owner_marker(root) {
                Ok(owner) => Ok(MutationLockInspection {
                    state: MutationLockState::Active,
                    owner,
                }),
                Err(_) => Ok(MutationLockInspection {
                    state: MutationLockState::Active,
                    owner: None,
                }),
            },
            Err(error) => Err(error),
        }
    }

    pub fn owner(&self) -> &MutationLockOwner {
        &self.owner
    }

    /// Revalidates the live cross-process capability and the legacy Viewer
    /// exclusion immediately before or after one bounded mutation phase.
    /// Callers must keep this lease alive for the whole transaction.
    pub fn revalidate(
        &self,
        legacy_probe: &mut impl LegacyViewerProbe,
    ) -> Result<(), MutationLockError> {
        self.verify_admission(legacy_probe)
    }

    /// Runs one durable registration while this cross-process capability is
    /// alive. The method shape prevents accidental lock release before the
    /// journal/backend/store sequence completes.
    pub fn journaled_register_account<B: ProtectedRecordBackend>(
        &self,
        legacy_probe: &mut impl LegacyViewerProbe,
        metadata: &mut CapacityStore,
        protected_backend: &mut B,
        registration: &VaultAccountRegistration,
        protected_record: &ProtectedVaultRecord,
        changed_at: &UtcTimestamp,
    ) -> Result<JournaledAccountRegistrationReceipt, LockedVaultMutationError> {
        self.verify_admission(legacy_probe)
            .map_err(LockedVaultMutationError::Lock)?;
        let outcome = journaled_register_account(
            metadata,
            protected_backend,
            registration,
            protected_record,
            changed_at,
        );
        self.verify_admission(legacy_probe)
            .map_err(LockedVaultMutationError::Lock)?;
        outcome.map_err(LockedVaultMutationError::Vault)
    }

    /// Runs one durable forget operation under the same lock capability.
    pub fn journaled_forget_account<B: ProtectedRecordBackend>(
        &self,
        legacy_probe: &mut impl LegacyViewerProbe,
        metadata: &mut CapacityStore,
        protected_backend: &mut B,
        expected_account: &VaultAccount,
        changed_at: &UtcTimestamp,
    ) -> Result<JournaledForgetAccountReceipt, LockedVaultMutationError> {
        self.verify_admission(legacy_probe)
            .map_err(LockedVaultMutationError::Lock)?;
        let outcome =
            journaled_forget_account(metadata, protected_backend, expected_account, changed_at);
        self.verify_admission(legacy_probe)
            .map_err(LockedVaultMutationError::Lock)?;
        outcome.map_err(LockedVaultMutationError::Vault)
    }

    /// Executes only recovery actions whose current postconditions can be
    /// proven without caller-supplied secret material. Registration retry and
    /// manual-review items remain explicit deferred dispositions.
    pub fn recover_vault_operations<B: ProtectedRecordBackend, P: LegacyViewerProbe>(
        &self,
        legacy_probe: &mut P,
        metadata: &mut CapacityStore,
        protected_backend: &mut B,
        limit: u32,
        changed_at: &UtcTimestamp,
    ) -> Result<VaultRecoveryExecutionReport, VaultRecoveryExecutionError> {
        recovery::execute_vault_recovery_queue(
            self,
            legacy_probe,
            metadata,
            protected_backend,
            limit,
            changed_at,
        )
    }

    /// Inventories protected-record artifacts and quarantines only a live
    /// record that remains unclaimed after lock-bound revalidation. Recovery
    /// records, staging residues, conflicts, and active operation artifacts
    /// are retained for explicit follow-up.
    pub fn reconcile_vault_artifacts<
        B: capacity_vault::ProtectedRecordInventoryBackend,
        P: LegacyViewerProbe,
    >(
        &self,
        legacy_probe: &mut P,
        metadata: &CapacityStore,
        protected_backend: &mut B,
        limit: u32,
    ) -> Result<VaultReconciliationReport, VaultReconciliationError> {
        reconciliation::reconcile_vault_artifacts(
            self,
            legacy_probe,
            metadata,
            protected_backend,
            limit,
        )
    }

    pub(crate) fn verify_admission(
        &self,
        legacy_probe: &mut impl LegacyViewerProbe,
    ) -> Result<(), MutationLockError> {
        self.verify_held()?;
        require_legacy_viewer_stopped(legacy_probe)
    }

    fn verify_held(&self) -> Result<(), MutationLockError> {
        let connection = self
            .connection
            .as_ref()
            .ok_or(MutationLockError::LeaseLost)?;
        let current_root = verify_private_directory(&self.root)?;
        let current_database = verify_private_file(&self.root.join(LOCK_DATABASE_FILE))?;
        if !same_file_identity(&self.root_identity, &current_root)
            || !same_file_identity(&self.database_identity, &current_database)
            || read_owner_marker(&self.root)?.as_ref() != Some(&self.owner)
        {
            return Err(MutationLockError::LeaseLost);
        }
        verify_lock_database(connection)
    }
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        if self.connection.is_none() {
            return;
        }
        if self.verify_held().is_ok()
            && read_owner_marker(&self.root).ok().flatten().as_ref() == Some(&self.owner)
        {
            let marker_path = self.root.join(OWNER_MARKER_FILE);
            if fs::remove_file(&marker_path).is_ok() {
                let _ = sync_directory(&self.root);
            }
        }
        if let Some(connection) = self.connection.take() {
            let _ = connection.execute_batch("ROLLBACK");
        }
    }
}

fn require_legacy_viewer_stopped<P: LegacyViewerProbe>(
    legacy_probe: &mut P,
) -> Result<(), MutationLockError> {
    match legacy_probe.legacy_viewer_state() {
        LegacyViewerState::NotRunning => Ok(()),
        LegacyViewerState::Running => Err(MutationLockError::LegacyViewerRunning),
        LegacyViewerState::Unavailable => Err(MutationLockError::LegacyViewerStateUnavailable),
    }
}

fn begin_immediate(connection: &Connection) -> Result<(), MutationLockError> {
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(|error| {
            if matches!(
                error.sqlite_error_code(),
                Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
            ) {
                MutationLockError::Busy
            } else {
                MutationLockError::LockDatabaseUnavailable
            }
        })
}

fn open_or_initialize_lock_database(root: &Path) -> Result<Connection, MutationLockError> {
    let database_path = root.join(LOCK_DATABASE_FILE);
    prepare_private_file(&database_path)?;
    let connection = open_verified_connection(&database_path)?;
    let initialization = connection.execute_batch(&format!(
        "BEGIN IMMEDIATE;
         {LOCK_SCHEMA};
         INSERT OR IGNORE INTO lock_protocol(singleton, protocol_version)
         VALUES (1, '{LOCK_PROTOCOL_VERSION}');
         PRAGMA user_version = 1;
         COMMIT;"
    ));
    if let Err(error) = initialization {
        let _ = connection.execute_batch("ROLLBACK");
        return Err(
            if matches!(
                error.sqlite_error_code(),
                Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
            ) {
                MutationLockError::Busy
            } else {
                MutationLockError::LockDatabaseUnavailable
            },
        );
    }
    verify_lock_database(&connection)?;
    Ok(connection)
}

fn open_existing_lock_database(root: &Path) -> Result<Connection, MutationLockError> {
    let database_path = root.join(LOCK_DATABASE_FILE);
    let connection = open_verified_connection(&database_path)?;
    verify_lock_database(&connection)?;
    Ok(connection)
}

fn open_verified_connection(path: &Path) -> Result<Connection, MutationLockError> {
    let before = verify_private_file(path)?;
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| MutationLockError::LockDatabaseUnavailable)?;
    connection
        .busy_timeout(std::time::Duration::ZERO)
        .map_err(|_| MutationLockError::LockDatabaseUnavailable)?;
    let after = verify_private_file(path)?;
    if !same_file_identity(&before, &after) {
        return Err(MutationLockError::LockDatabaseUnavailable);
    }
    Ok(connection)
}

fn verify_lock_database(connection: &Connection) -> Result<(), MutationLockError> {
    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| MutationLockError::LockDatabaseCorrupt)?;
    let table_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| MutationLockError::LockDatabaseCorrupt)?;
    let protocol: String = connection
        .query_row(
            "SELECT protocol_version FROM lock_protocol WHERE singleton = ?1",
            params![1_i64],
            |row| row.get(0),
        )
        .map_err(|_| MutationLockError::LockDatabaseCorrupt)?;
    let schema: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'lock_protocol'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| MutationLockError::LockDatabaseCorrupt)?;
    let integrity: String = connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(|_| MutationLockError::LockDatabaseCorrupt)?;
    if user_version != 1
        || table_count != 1
        || protocol != LOCK_PROTOCOL_VERSION
        || schema != EXPECTED_LOCK_SCHEMA
        || integrity != "ok"
    {
        return Err(MutationLockError::LockDatabaseCorrupt);
    }
    Ok(())
}

fn write_owner_marker(root: &Path, owner: &MutationLockOwner) -> Result<(), MutationLockError> {
    match read_owner_marker(root) {
        Ok(_) => {}
        Err(MutationLockError::Io(std::io::ErrorKind::NotFound)) => {}
        Err(error) => return Err(error),
    }
    let bytes = render_owner_marker(owner);
    if bytes.len() as u64 > MAX_OWNER_MARKER_BYTES {
        return Err(MutationLockError::OwnerMarkerCorrupt);
    }
    let temporary_path = allocate_owner_temporary_path(root, owner.process_id)?;
    let mut temporary_guard = TemporaryOwnerMarker::new(root, temporary_path.clone());
    write_private_file_new(&temporary_path, bytes.as_bytes())?;
    fs::rename(&temporary_path, root.join(OWNER_MARKER_FILE))
        .map_err(|error| MutationLockError::Io(error.kind()))?;
    sync_directory(root)?;
    temporary_guard.disarm();
    if read_owner_marker(root)?.as_ref() != Some(owner) {
        return Err(MutationLockError::OwnerMarkerCorrupt);
    }
    Ok(())
}

fn render_owner_marker(owner: &MutationLockOwner) -> String {
    format!(
        "{OWNER_MARKER_FORMAT}\nowner_id={}\nprocess_id={}\nstarted_at={}\n",
        owner.owner_id.as_str(),
        owner.process_id,
        owner.started_at.as_str()
    )
}

fn read_owner_marker(root: &Path) -> Result<Option<MutationLockOwner>, MutationLockError> {
    let path = root.join(OWNER_MARKER_FILE);
    let before = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(MutationLockError::Io(error.kind())),
    };
    verify_private_file_metadata(&before)?;
    if before.len() > MAX_OWNER_MARKER_BYTES {
        return Err(MutationLockError::OwnerMarkerCorrupt);
    }
    let file = File::open(&path).map_err(|error| MutationLockError::Io(error.kind()))?;
    let opened = file
        .metadata()
        .map_err(|error| MutationLockError::Io(error.kind()))?;
    verify_private_file_metadata(&opened)?;
    if !same_file_identity(&before, &opened) {
        return Err(MutationLockError::OwnerMarkerUnavailable);
    }
    let mut bytes = Vec::new();
    file.take(MAX_OWNER_MARKER_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| MutationLockError::Io(error.kind()))?;
    if bytes.len() as u64 > MAX_OWNER_MARKER_BYTES {
        return Err(MutationLockError::OwnerMarkerCorrupt);
    }
    let after =
        fs::symlink_metadata(&path).map_err(|_| MutationLockError::OwnerMarkerUnavailable)?;
    verify_private_file_metadata(&after)?;
    if !same_file_identity(&opened, &after) {
        return Err(MutationLockError::OwnerMarkerUnavailable);
    }
    parse_owner_marker(&bytes).map(Some)
}

fn parse_owner_marker(bytes: &[u8]) -> Result<MutationLockOwner, MutationLockError> {
    let text = std::str::from_utf8(bytes).map_err(|_| MutationLockError::OwnerMarkerCorrupt)?;
    if !text.ends_with('\n') || text.chars().any(|character| character == '\r') {
        return Err(MutationLockError::OwnerMarkerCorrupt);
    }
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() != 4 || lines[0] != OWNER_MARKER_FORMAT {
        return Err(MutationLockError::OwnerMarkerCorrupt);
    }
    let owner_id = lines[1]
        .strip_prefix("owner_id=")
        .ok_or(MutationLockError::OwnerMarkerCorrupt)
        .and_then(|value| {
            MutationOwnerId::parse(value.to_owned())
                .map_err(|_| MutationLockError::OwnerMarkerCorrupt)
        })?;
    let process_text = lines[2]
        .strip_prefix("process_id=")
        .ok_or(MutationLockError::OwnerMarkerCorrupt)?;
    let process_id = process_text
        .parse::<u32>()
        .map_err(|_| MutationLockError::OwnerMarkerCorrupt)?;
    if process_id.to_string() != process_text {
        return Err(MutationLockError::OwnerMarkerCorrupt);
    }
    let started_at = lines[3]
        .strip_prefix("started_at=")
        .ok_or(MutationLockError::OwnerMarkerCorrupt)
        .and_then(|value| {
            UtcTimestamp::parse(value.to_owned()).map_err(|_| MutationLockError::OwnerMarkerCorrupt)
        })?;
    MutationLockOwner::new(owner_id, process_id, started_at)
        .map_err(|_| MutationLockError::OwnerMarkerCorrupt)
}

fn allocate_owner_temporary_path(
    root: &Path,
    process_id: u32,
) -> Result<PathBuf, MutationLockError> {
    for _ in 0..MAX_OWNER_TEMP_ATTEMPTS {
        let sequence = OWNER_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(".mutation-owner-{process_id}-{sequence}.tmp"));
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(path),
            Ok(_) => continue,
            Err(error) => return Err(MutationLockError::Io(error.kind())),
        }
    }
    Err(MutationLockError::OwnerMarkerUnavailable)
}

fn prepare_lock_root(path: &Path) -> Result<(), MutationLockError> {
    #[cfg(not(unix))]
    {
        let _ = path;
        return Err(MutationLockError::PlatformPermissionsUnavailable);
    }

    #[cfg(unix)]
    {
        validate_root_path(path)?;
        match fs::symlink_metadata(path) {
            Ok(metadata) => verify_private_directory_metadata(&metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                use std::os::unix::fs::DirBuilderExt;

                let mut builder = fs::DirBuilder::new();
                builder.mode(PRIVATE_DIRECTORY_MODE);
                if let Err(error) = builder.create(path)
                    && error.kind() != std::io::ErrorKind::AlreadyExists
                {
                    return Err(MutationLockError::Io(error.kind()));
                }
                verify_private_directory_metadata(
                    &fs::symlink_metadata(path)
                        .map_err(|error| MutationLockError::Io(error.kind()))?,
                )
            }
            Err(error) => Err(MutationLockError::Io(error.kind())),
        }
    }
}

#[cfg(unix)]
fn validate_root_path(path: &Path) -> Result<(), MutationLockError> {
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
            .filter(|component| matches!(component, Component::Normal(_)))
            .count()
            < 2
    {
        return Err(MutationLockError::InvalidRoot);
    }
    Ok(())
}

fn prepare_private_file(path: &Path) -> Result<(), MutationLockError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => verify_private_file_metadata(&metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match write_private_file_new(path, &[]) {
                Ok(()) => Ok(()),
                Err(MutationLockError::Io(std::io::ErrorKind::AlreadyExists)) => {
                    verify_private_file(path).map(|_| ())
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(MutationLockError::Io(error.kind())),
    }
}

#[cfg(unix)]
fn write_private_file_new(path: &Path, bytes: &[u8]) -> Result<(), MutationLockError> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE_MODE)
        .open(path)
        .map_err(|error| MutationLockError::Io(error.kind()))?;
    file.write_all(bytes)
        .map_err(|error| MutationLockError::Io(error.kind()))?;
    file.sync_all()
        .map_err(|error| MutationLockError::Io(error.kind()))?;
    verify_private_file_metadata(
        &file
            .metadata()
            .map_err(|error| MutationLockError::Io(error.kind()))?,
    )
}

#[cfg(not(unix))]
fn write_private_file_new(_path: &Path, _bytes: &[u8]) -> Result<(), MutationLockError> {
    Err(MutationLockError::PlatformPermissionsUnavailable)
}

fn verify_private_directory(path: &Path) -> Result<Metadata, MutationLockError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| MutationLockError::Io(error.kind()))?;
    verify_private_directory_metadata(&metadata)?;
    Ok(metadata)
}

fn verify_private_directory_metadata(metadata: &Metadata) -> Result<(), MutationLockError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(MutationLockError::UnsafeFileType);
    }
    verify_directory_permissions(metadata)
}

fn verify_private_file(path: &Path) -> Result<Metadata, MutationLockError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            MutationLockError::OwnerMarkerUnavailable
        } else {
            MutationLockError::Io(error.kind())
        }
    })?;
    verify_private_file_metadata(&metadata)?;
    Ok(metadata)
}

fn verify_private_file_metadata(metadata: &Metadata) -> Result<(), MutationLockError> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(MutationLockError::UnsafeFileType);
    }
    verify_file_permissions(metadata)
}

#[cfg(unix)]
fn verify_directory_permissions(metadata: &Metadata) -> Result<(), MutationLockError> {
    use std::os::unix::fs::PermissionsExt;

    if metadata.permissions().mode() & 0o777 != PRIVATE_DIRECTORY_MODE {
        return Err(MutationLockError::InsecurePermissions);
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_directory_permissions(_metadata: &Metadata) -> Result<(), MutationLockError> {
    Err(MutationLockError::PlatformPermissionsUnavailable)
}

#[cfg(unix)]
fn verify_file_permissions(metadata: &Metadata) -> Result<(), MutationLockError> {
    use std::os::unix::fs::PermissionsExt;

    if metadata.permissions().mode() & 0o777 != PRIVATE_FILE_MODE {
        return Err(MutationLockError::InsecurePermissions);
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_file_permissions(_metadata: &Metadata) -> Result<(), MutationLockError> {
    Err(MutationLockError::PlatformPermissionsUnavailable)
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

fn sync_directory(path: &Path) -> Result<(), MutationLockError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| MutationLockError::Io(error.kind()))
}

fn is_lowercase_uuid(value: &str) -> bool {
    value.len() == 36
        && value.as_bytes().iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
            }
        })
}

struct TemporaryOwnerMarker<'a> {
    root: &'a Path,
    path: PathBuf,
    armed: bool,
}

impl<'a> TemporaryOwnerMarker<'a> {
    fn new(root: &'a Path, path: PathBuf) -> Self {
        Self {
            root,
            path,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryOwnerMarker<'_> {
    fn drop(&mut self) {
        if !self.armed
            || self.path.parent() != Some(self.root)
            || !self
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".mutation-owner-") && name.ends_with(".tmp"))
        {
            return;
        }
        if let Ok(metadata) = fs::symlink_metadata(&self.path)
            && metadata.is_file()
            && !metadata.file_type().is_symlink()
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::VecDeque;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use capacity_domain::{
        AccountFingerprint, VaultAccountAuthMode, VaultAccountLifecycle, VaultAccountSource,
        VaultOperation, VaultOperationStatus, VaultOperationTransition, VaultRecordRef,
    };
    use capacity_store::{
        VaultAccountRegistrationOutcome, VaultAccountRemovalOutcome, VaultOperationAdvanceOutcome,
        VaultOperationBeginOutcome,
    };
    use capacity_vault::{FileVault, VaultRecordState, VaultTransitionOutcome};

    use super::*;

    const OWNER_UUID: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73501";
    const OWNER_UUID_2: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73502";
    const FINGERPRINT: &str = "hmac-sha256:v1:018f47a2-8a71-7f4a-9c35-1f4234a73401:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const RECORD_UUID: &str = "018f47a2-8a71-7f4a-9c35-1f4234a73402";
    const CHILD_MODE: &str = "CAPACITY_MUTATION_LOCK_CHILD";
    const CHILD_ROOT: &str = "CAPACITY_MUTATION_LOCK_CHILD_ROOT";
    const CHECKPOINT_CHILD_MODE: &str = "CAPACITY_MUTATION_CHECKPOINT_CHILD";
    const CHECKPOINT_CHILD_BASE: &str = "CAPACITY_MUTATION_CHECKPOINT_BASE";
    const CHECKPOINT_CHILD_SCENARIO: &str = "CAPACITY_MUTATION_CHECKPOINT_SCENARIO";
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
                "capacity-mutation-test-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            builder.mode(PRIVATE_DIRECTORY_MODE);
            builder.create(&base).expect("create test base");
            let lock = base.join("lock-root");
            Self { base, lock }
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let safe = self
                .base
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("capacity-mutation-test-"));
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

    struct ChildGuard(Option<Child>);

    impl ChildGuard {
        fn child_mut(&mut self) -> &mut Child {
            self.0.as_mut().expect("child")
        }

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
            timestamp("2026-08-30T09:00:00Z"),
        )
        .expect("owner")
    }

    #[test]
    fn macos_shared_root_is_frozen_and_rejects_untrusted_relative_home() {
        assert_eq!(
            macos_shared_mutation_lock_root("/Users/example").unwrap(),
            PathBuf::from("/Users/example/Library/Application Support/CodexMutationLock-v1")
        );
        assert_eq!(
            macos_shared_mutation_lock_root("relative-home").unwrap_err(),
            MutationLockError::InvalidRoot
        );
        assert_eq!(
            macos_shared_mutation_lock_root("/Users/example/../other").unwrap_err(),
            MutationLockError::InvalidRoot
        );
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
            provider_id: None,
            model: None,
        }
    }

    fn protected_record() -> ProtectedVaultRecord {
        ProtectedVaultRecord::new(
            "official_codex:v1",
            "upstream_account_id",
            b"mutation-person@example.invalid".to_vec(),
            br#"{"auth_mode":"chatgpt","access_token":"mutation-secret-token"}"#.to_vec(),
            None,
        )
        .expect("protected record")
    }

    fn advance_child_operation(
        metadata: &mut CapacityStore,
        operation: &VaultOperation,
        transition: VaultOperationTransition,
    ) -> VaultOperation {
        match metadata
            .advance_vault_operation(
                &operation.operation_id,
                operation.revision,
                transition,
                &timestamp("2026-08-30T09:02:00Z"),
            )
            .expect("advance child operation")
        {
            VaultOperationAdvanceOutcome::Updated(operation) => operation,
            VaultOperationAdvanceOutcome::RevisionConflict(_) => {
                panic!("child operation revision changed")
            }
            VaultOperationAdvanceOutcome::Missing => panic!("child operation disappeared"),
        }
    }

    fn prepare_registration_checkpoint(
        scenario: &str,
        metadata: &mut CapacityStore,
        vault: &mut FileVault,
    ) {
        let registration = registration();
        let mut operation = match metadata
            .begin_vault_registration_operation(&registration, &timestamp("2026-08-30T09:01:00Z"))
            .expect("begin child registration")
        {
            VaultOperationBeginOutcome::Created(operation) => operation,
            VaultOperationBeginOutcome::Existing(_) => panic!("unexpected child registration"),
        };
        if scenario == "register_prepared" {
            return;
        }
        vault
            .create_record(&registration.protected_record_ref, &protected_record())
            .expect("create child record");
        operation =
            advance_child_operation(metadata, &operation, VaultOperationTransition::RecordReady);
        if scenario == "register_record_ready" {
            return;
        }
        if scenario == "register_record_quarantined" {
            assert_eq!(
                vault
                    .quarantine_record(&registration.protected_record_ref)
                    .expect("quarantine child record"),
                VaultTransitionOutcome::Moved
            );
            let _ = advance_child_operation(
                metadata,
                &operation,
                VaultOperationTransition::RecordQuarantined,
            );
            return;
        }
        assert_eq!(scenario, "register_metadata_committed");
        let account = match metadata
            .register_vault_account(&registration, &timestamp("2026-08-30T09:02:00Z"))
            .expect("register child metadata")
        {
            VaultAccountRegistrationOutcome::Created(account) => account,
            VaultAccountRegistrationOutcome::Existing(_) => panic!("unexpected existing account"),
        };
        let _ = advance_child_operation(
            metadata,
            &operation,
            VaultOperationTransition::MetadataCommitted {
                account_id: account.account_id,
            },
        );
    }

    fn prepare_forget_checkpoint(
        scenario: &str,
        metadata: &mut CapacityStore,
        vault: &mut FileVault,
    ) {
        let registration = registration();
        vault
            .create_record(&registration.protected_record_ref, &protected_record())
            .expect("create child record");
        let account = match metadata
            .register_vault_account(&registration, &timestamp("2026-08-30T09:01:00Z"))
            .expect("register child account")
        {
            VaultAccountRegistrationOutcome::Created(account) => account,
            VaultAccountRegistrationOutcome::Existing(_) => panic!("unexpected existing account"),
        };
        let mut operation = match metadata
            .begin_vault_forget_operation(&account, &timestamp("2026-08-30T09:02:00Z"))
            .expect("begin child forget")
        {
            VaultOperationBeginOutcome::Created(operation) => operation,
            VaultOperationBeginOutcome::Existing(_) => panic!("unexpected child forget"),
        };
        if scenario == "forget_prepared" {
            return;
        }
        assert_eq!(
            vault
                .quarantine_record(&registration.protected_record_ref)
                .expect("quarantine child record"),
            VaultTransitionOutcome::Moved
        );
        operation = advance_child_operation(
            metadata,
            &operation,
            VaultOperationTransition::RecordQuarantined,
        );
        if scenario == "forget_record_quarantined" {
            return;
        }
        if scenario == "forget_record_restored" {
            assert_eq!(
                vault
                    .restore_record(&registration.protected_record_ref)
                    .expect("restore child record"),
                VaultTransitionOutcome::Moved
            );
            let _ = advance_child_operation(
                metadata,
                &operation,
                VaultOperationTransition::RecordRestored,
            );
            return;
        }
        assert_eq!(scenario, "forget_metadata_removed");
        assert!(matches!(
            metadata
                .remove_vault_account_metadata(
                    &account.account_id,
                    account.revision,
                    &timestamp("2026-08-30T09:03:00Z"),
                )
                .expect("remove child metadata"),
            VaultAccountRemovalOutcome::Removed(_)
        ));
        let _ = advance_child_operation(
            metadata,
            &operation,
            VaultOperationTransition::MetadataRemoved,
        );
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
    fn owner_ids_and_debug_output_are_strict_and_redacted() {
        assert!(MutationOwnerId::parse(format!("{OWNER_ID_PREFIX}{OWNER_UUID}")).is_ok());
        for invalid in [
            "mutation-owner:v1:not-a-uuid",
            "mutation-owner:v1:018F47a2-8a71-7f4a-9c35-1f4234a73501",
            "vault-operation:v1:018f47a2-8a71-7f4a-9c35-1f4234a73501",
        ] {
            assert_eq!(
                MutationOwnerId::parse(invalid),
                Err(MutationLockError::InvalidOwnerId)
            );
        }
        let rendered = format!("{:?}", owner(OWNER_UUID));
        assert!(!rendered.contains(OWNER_UUID));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn lock_is_cross_connection_exclusive_and_drop_clears_owner() {
        let root = TestRoot::new();
        let first_owner = owner(OWNER_UUID);
        let mut first_probe = FakeLegacyProbe::default();
        let lease = MutationLock::acquire(&root.lock, first_owner.clone(), &mut first_probe)
            .expect("acquire first lock");
        assert_eq!(first_probe.calls, 2);
        assert_eq!(lease.owner(), &first_owner);
        let inspection = MutationLock::inspect(&root.lock).expect("inspect active lock");
        assert_eq!(inspection.state, MutationLockState::Active);
        assert_eq!(inspection.owner, Some(first_owner));
        assert_eq!(
            fs::symlink_metadata(&root.lock)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for path in [
            root.lock.join(LOCK_DATABASE_FILE),
            root.lock.join(OWNER_MARKER_FILE),
        ] {
            assert_eq!(
                fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        let mut second_probe = FakeLegacyProbe::default();
        assert_eq!(
            MutationLock::acquire(&root.lock, owner(OWNER_UUID_2), &mut second_probe)
                .expect_err("second owner must be blocked"),
            MutationLockError::Busy
        );
        drop(lease);
        assert_eq!(
            MutationLock::inspect(&root.lock).unwrap().state,
            MutationLockState::Unlocked
        );
        let second = MutationLock::acquire(
            &root.lock,
            owner(OWNER_UUID_2),
            &mut FakeLegacyProbe::default(),
        )
        .expect("acquire after drop");
        drop(second);
    }

    #[test]
    fn legacy_viewer_checks_fail_closed_before_and_after_lock_acquisition() {
        let root = TestRoot::new();
        let mut running = FakeLegacyProbe::with_states([LegacyViewerState::Running]);
        assert_eq!(
            MutationLock::acquire(&root.lock, owner(OWNER_UUID), &mut running)
                .expect_err("running legacy viewer must block"),
            MutationLockError::LegacyViewerRunning
        );
        assert!(
            !root.lock.exists(),
            "preflight must have no filesystem side effect"
        );

        let mut unknown = FakeLegacyProbe::with_states([LegacyViewerState::Unavailable]);
        assert_eq!(
            MutationLock::acquire(&root.lock, owner(OWNER_UUID), &mut unknown)
                .expect_err("unknown legacy viewer state must block"),
            MutationLockError::LegacyViewerStateUnavailable
        );

        let mut late_start = FakeLegacyProbe::with_states([
            LegacyViewerState::NotRunning,
            LegacyViewerState::Running,
        ]);
        assert_eq!(
            MutationLock::acquire(&root.lock, owner(OWNER_UUID), &mut late_start)
                .expect_err("late legacy viewer start must block"),
            MutationLockError::LegacyViewerRunning
        );
        assert_eq!(late_start.calls, 2);
        assert_eq!(
            MutationLock::inspect(&root.lock).unwrap().state,
            MutationLockState::Unlocked
        );
        assert!(!root.lock.join(OWNER_MARKER_FILE).exists());
    }

    #[test]
    fn corrupt_or_insecure_owner_marker_is_never_silently_replaced() {
        let root = TestRoot::new();
        let lease = MutationLock::acquire(
            &root.lock,
            owner(OWNER_UUID),
            &mut FakeLegacyProbe::default(),
        )
        .expect("initialize lock");
        drop(lease);
        let marker = root.lock.join(OWNER_MARKER_FILE);
        write_private_file_new(&marker, b"corrupt\n").expect("write corrupt marker");
        assert_eq!(
            MutationLock::inspect(&root.lock).unwrap().state,
            MutationLockState::Unverifiable
        );
        assert_eq!(
            MutationLock::acquire(
                &root.lock,
                owner(OWNER_UUID_2),
                &mut FakeLegacyProbe::default(),
            )
            .expect_err("corrupt marker must block"),
            MutationLockError::OwnerMarkerCorrupt
        );
        assert!(marker.exists());

        fs::remove_file(&marker).unwrap();
        write_private_file_new(&marker, render_owner_marker(&owner(OWNER_UUID)).as_bytes())
            .expect("write marker");
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            MutationLock::acquire(
                &root.lock,
                owner(OWNER_UUID_2),
                &mut FakeLegacyProbe::default(),
            )
            .expect_err("insecure marker must block"),
            MutationLockError::InsecurePermissions
        );
    }

    #[test]
    fn unsafe_roots_and_marker_symlinks_fail_closed_without_path_disclosure() {
        let root = TestRoot::new();
        assert_eq!(
            MutationLock::acquire(
                "relative-lock",
                owner(OWNER_UUID),
                &mut FakeLegacyProbe::default(),
            )
            .expect_err("relative root must fail"),
            MutationLockError::InvalidRoot
        );
        let lease = MutationLock::acquire(
            &root.lock,
            owner(OWNER_UUID),
            &mut FakeLegacyProbe::default(),
        )
        .unwrap();
        drop(lease);
        let outside = root.base.join("outside");
        write_private_file_new(&outside, b"outside").unwrap();
        symlink(&outside, root.lock.join(OWNER_MARKER_FILE)).unwrap();
        let error = MutationLock::acquire(
            &root.lock,
            owner(OWNER_UUID_2),
            &mut FakeLegacyProbe::default(),
        )
        .expect_err("symlink must fail");
        assert_eq!(error, MutationLockError::UnsafeFileType);
        assert!(!format!("{error:?} {error}").contains(root.base.to_string_lossy().as_ref()));
    }

    #[test]
    fn database_symlink_and_schema_drift_fail_closed() {
        let symlink_root = TestRoot::new();
        let lease = MutationLock::acquire(
            &symlink_root.lock,
            owner(OWNER_UUID),
            &mut FakeLegacyProbe::default(),
        )
        .expect("initialize lock");
        drop(lease);
        let database = symlink_root.lock.join(LOCK_DATABASE_FILE);
        fs::remove_file(&database).unwrap();
        symlink(symlink_root.base.join("missing"), &database).unwrap();
        assert_eq!(
            MutationLock::inspect(&symlink_root.lock).expect_err("database symlink"),
            MutationLockError::UnsafeFileType
        );

        let drift_root = TestRoot::new();
        let lease = MutationLock::acquire(
            &drift_root.lock,
            owner(OWNER_UUID),
            &mut FakeLegacyProbe::default(),
        )
        .expect("initialize lock");
        drop(lease);
        let connection = Connection::open(drift_root.lock.join(LOCK_DATABASE_FILE)).unwrap();
        connection
            .execute_batch(
                "DROP TABLE lock_protocol;
                 CREATE TABLE lock_protocol(singleton INTEGER PRIMARY KEY, protocol_version TEXT);
                 INSERT INTO lock_protocol VALUES (1, 'capacity-mutation-lock-v1');",
            )
            .unwrap();
        drop(connection);
        assert_eq!(
            MutationLock::acquire(
                &drift_root.lock,
                owner(OWNER_UUID_2),
                &mut FakeLegacyProbe::default(),
            )
            .expect_err("schema drift"),
            MutationLockError::LockDatabaseCorrupt
        );
    }

    #[test]
    fn held_database_identity_replacement_revokes_mutation_capability() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.base.join("capacity.sqlite3")).unwrap();
        let mut vault = FileVault::open(root.base.join("vault")).unwrap();
        let lock = MutationLock::acquire(
            &root.lock,
            owner(OWNER_UUID),
            &mut FakeLegacyProbe::default(),
        )
        .expect("mutation lock");
        let database = root.lock.join(LOCK_DATABASE_FILE);
        fs::rename(&database, root.lock.join("displaced-lock.sqlite3")).unwrap();
        write_private_file_new(&database, &[]).unwrap();
        let registration = registration();
        assert_eq!(
            lock.journaled_register_account(
                &mut FakeLegacyProbe::default(),
                &mut metadata,
                &mut vault,
                &registration,
                &protected_record(),
                &timestamp("2026-08-30T09:01:00Z"),
            ),
            Err(LockedVaultMutationError::Lock(MutationLockError::LeaseLost))
        );
        assert!(
            metadata
                .vault_account_by_fingerprint(&registration.account_fingerprint)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            vault.record_state(&registration.protected_record_ref),
            Ok(VaultRecordState::Missing)
        );
        drop(lock);
    }

    #[test]
    fn post_mutation_legacy_probe_detects_a_late_viewer_start() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.base.join("capacity.sqlite3")).unwrap();
        let mut vault = FileVault::open(root.base.join("vault")).unwrap();
        let mut probe = FakeLegacyProbe::with_states([
            LegacyViewerState::NotRunning,
            LegacyViewerState::NotRunning,
            LegacyViewerState::NotRunning,
            LegacyViewerState::Running,
        ]);
        let lock = MutationLock::acquire(&root.lock, owner(OWNER_UUID), &mut probe).unwrap();
        let registration = registration();
        assert_eq!(
            lock.journaled_register_account(
                &mut probe,
                &mut metadata,
                &mut vault,
                &registration,
                &protected_record(),
                &timestamp("2026-08-30T09:01:00Z"),
            ),
            Err(LockedVaultMutationError::Lock(
                MutationLockError::LegacyViewerRunning
            ))
        );
        assert_eq!(probe.calls, 4);
        assert!(
            metadata
                .vault_account_by_fingerprint(&registration.account_fingerprint)
                .unwrap()
                .is_some(),
            "the durable mutation completed but coexistence changed before return"
        );
        drop(lock);
    }

    #[test]
    fn locked_account_facade_serializes_registration_and_forget() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.base.join("capacity.sqlite3")).unwrap();
        let mut vault = FileVault::open(root.base.join("vault")).unwrap();
        let registration = registration();
        let mut probe = FakeLegacyProbe::default();
        let registration_lock = MutationLock::acquire(&root.lock, owner(OWNER_UUID), &mut probe)
            .expect("registration lock");
        let account = registration_lock
            .journaled_register_account(
                &mut probe,
                &mut metadata,
                &mut vault,
                &registration,
                &protected_record(),
                &timestamp("2026-08-30T09:01:00Z"),
            )
            .expect("locked registration")
            .receipt
            .account;
        assert_eq!(probe.calls, 4);
        drop(registration_lock);
        assert_eq!(
            MutationLock::inspect(&root.lock).unwrap().state,
            MutationLockState::Unlocked
        );

        let forget_lock = MutationLock::acquire(&root.lock, owner(OWNER_UUID_2), &mut probe)
            .expect("forget lock");
        let receipt = forget_lock
            .journaled_forget_account(
                &mut probe,
                &mut metadata,
                &mut vault,
                &account,
                &timestamp("2026-08-30T09:02:00Z"),
            )
            .expect("locked forget");
        assert!(receipt.operation_id.is_some());
        assert_eq!(probe.calls, 8);
        assert_eq!(
            vault.record_state(&registration.protected_record_ref),
            Ok(VaultRecordState::Quarantined)
        );
        drop(forget_lock);
        assert_eq!(
            MutationLock::inspect(&root.lock).unwrap().state,
            MutationLockState::Unlocked
        );
    }

    #[test]
    fn vault_error_releases_lock_for_the_next_operation() {
        let root = TestRoot::new();
        let mut metadata = CapacityStore::open(root.base.join("capacity.sqlite3")).unwrap();
        let mut vault = FileVault::open(root.base.join("vault")).unwrap();
        let mut invalid = registration();
        invalid.display_name.clear();
        let mut probe = FakeLegacyProbe::default();
        let lock = MutationLock::acquire(&root.lock, owner(OWNER_UUID), &mut probe)
            .expect("mutation lock");
        let error = lock
            .journaled_register_account(
                &mut probe,
                &mut metadata,
                &mut vault,
                &invalid,
                &protected_record(),
                &timestamp("2026-08-30T09:01:00Z"),
            )
            .expect_err("invalid registration");
        assert_eq!(
            error,
            LockedVaultMutationError::Vault(AccountVaultError::InvalidRegistration)
        );
        drop(lock);
        assert_eq!(
            MutationLock::inspect(&root.lock).unwrap().state,
            MutationLockState::Unlocked
        );
        let lease = MutationLock::acquire(
            &root.lock,
            owner(OWNER_UUID_2),
            &mut FakeLegacyProbe::default(),
        )
        .expect("next owner");
        drop(lease);
    }

    #[test]
    fn subprocess_lock_holder() {
        if std::env::var_os(CHILD_MODE).is_none() {
            return;
        }
        let root = PathBuf::from(std::env::var_os(CHILD_ROOT).expect("child root"));
        let child_owner = MutationLockOwner::new(
            MutationOwnerId::parse(format!("{OWNER_ID_PREFIX}{OWNER_UUID}")).unwrap(),
            std::process::id(),
            timestamp("2026-08-30T09:00:00Z"),
        )
        .unwrap();
        let _lease = MutationLock::acquire(&root, child_owner, &mut FakeLegacyProbe::default())
            .expect("child lock");
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }

    #[test]
    fn subprocess_checkpoint_holder() {
        if std::env::var_os(CHECKPOINT_CHILD_MODE).is_none() {
            return;
        }
        let base =
            PathBuf::from(std::env::var_os(CHECKPOINT_CHILD_BASE).expect("checkpoint child base"));
        let scenario = std::env::var(CHECKPOINT_CHILD_SCENARIO).expect("checkpoint scenario");
        let mut metadata = CapacityStore::open(base.join("capacity.sqlite3")).expect("child store");
        let mut vault = FileVault::open(base.join("vault")).expect("child vault");
        let _lease = MutationLock::acquire(
            base.join("lock-root"),
            owner(OWNER_UUID),
            &mut FakeLegacyProbe::default(),
        )
        .expect("child mutation lock");
        if scenario.starts_with("register_") {
            prepare_registration_checkpoint(&scenario, &mut metadata, &mut vault);
        } else if scenario.starts_with("forget_") {
            prepare_forget_checkpoint(&scenario, &mut metadata, &mut vault);
        } else {
            panic!("unknown checkpoint scenario")
        }
        write_private_file_new(&base.join("checkpoint-ready-v1"), b"ready\n")
            .expect("publish checkpoint readiness");
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }

    #[test]
    fn real_process_kill_matrix_recovers_every_durable_checkpoint() {
        if std::env::var_os(CHECKPOINT_CHILD_MODE).is_some() {
            return;
        }
        let scenarios = [
            (
                "register_prepared",
                VaultOperationStatus::InProgress,
                VaultRecoveryDisposition::RegistrationRequestRequired,
                VaultRecordState::Missing,
                false,
            ),
            (
                "register_record_ready",
                VaultOperationStatus::InProgress,
                VaultRecoveryDisposition::RegistrationRequestRequired,
                VaultRecordState::Present,
                false,
            ),
            (
                "register_metadata_committed",
                VaultOperationStatus::Succeeded,
                VaultRecoveryDisposition::Succeeded,
                VaultRecordState::Present,
                true,
            ),
            (
                "register_record_quarantined",
                VaultOperationStatus::Compensated,
                VaultRecoveryDisposition::Compensated,
                VaultRecordState::Quarantined,
                false,
            ),
            (
                "forget_prepared",
                VaultOperationStatus::Succeeded,
                VaultRecoveryDisposition::Succeeded,
                VaultRecordState::Quarantined,
                false,
            ),
            (
                "forget_record_quarantined",
                VaultOperationStatus::Succeeded,
                VaultRecoveryDisposition::Succeeded,
                VaultRecordState::Quarantined,
                false,
            ),
            (
                "forget_metadata_removed",
                VaultOperationStatus::Succeeded,
                VaultRecoveryDisposition::Succeeded,
                VaultRecordState::Quarantined,
                false,
            ),
            (
                "forget_record_restored",
                VaultOperationStatus::Compensated,
                VaultRecoveryDisposition::Compensated,
                VaultRecordState::Present,
                true,
            ),
        ];
        let test_executable = std::env::current_exe().expect("test executable");
        for (scenario, expected_status, expected_disposition, expected_record, account_exists) in
            scenarios
        {
            let root = TestRoot::new();
            let child = Command::new(&test_executable)
                .args([
                    "--exact",
                    "tests::subprocess_checkpoint_holder",
                    "--nocapture",
                ])
                .env(CHECKPOINT_CHILD_MODE, "1")
                .env(CHECKPOINT_CHILD_BASE, &root.base)
                .env(CHECKPOINT_CHILD_SCENARIO, scenario)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn checkpoint child");
            let mut child = ChildGuard(Some(child));
            wait_until(Duration::from_secs(5), || {
                root.base.join("checkpoint-ready-v1").exists()
            });
            assert_eq!(
                MutationLock::inspect(&root.lock).unwrap().state,
                MutationLockState::Active,
                "scenario={scenario}"
            );
            child.kill_and_wait();
            wait_until(Duration::from_secs(5), || {
                MutationLock::inspect(&root.lock)
                    .is_ok_and(|inspection| inspection.state == MutationLockState::Stale)
            });

            let mut metadata =
                CapacityStore::open(root.base.join("capacity.sqlite3")).expect("recovery store");
            let mut vault = FileVault::open(root.base.join("vault")).expect("recovery vault");
            let pending = metadata
                .recoverable_vault_operations(10)
                .expect("pending checkpoint");
            assert_eq!(pending.len(), 1, "scenario={scenario}");
            let operation_id = pending[0].operation_id.clone();
            let replacement = MutationLock::acquire(
                &root.lock,
                owner(OWNER_UUID_2),
                &mut FakeLegacyProbe::default(),
            )
            .expect("replacement owner");
            let report = replacement
                .recover_vault_operations(
                    &mut FakeLegacyProbe::default(),
                    &mut metadata,
                    &mut vault,
                    10,
                    &timestamp("2026-08-30T09:10:00Z"),
                )
                .expect("recover killed checkpoint");
            assert_eq!(report.scanned, 1, "scenario={scenario}");
            assert_eq!(report.items.len(), 1, "scenario={scenario}");
            assert_eq!(
                report.items[0].disposition, expected_disposition,
                "scenario={scenario}"
            );
            let recovered = metadata
                .vault_operation(&operation_id)
                .expect("read recovered operation")
                .expect("recovered operation exists");
            assert_eq!(recovered.status, expected_status, "scenario={scenario}");
            assert_eq!(
                vault.record_state(&registration().protected_record_ref),
                Ok(expected_record),
                "scenario={scenario}"
            );
            assert_eq!(
                metadata
                    .vault_account_by_fingerprint(&registration().account_fingerprint)
                    .expect("account lookup")
                    .is_some(),
                account_exists,
                "scenario={scenario}"
            );
            replacement.verify_held().expect("replacement still held");
            drop(replacement);
            assert_eq!(
                MutationLock::inspect(&root.lock).unwrap().state,
                MutationLockState::Unlocked,
                "scenario={scenario}"
            );
        }
    }

    #[test]
    fn os_releases_lock_after_process_kill_and_next_owner_reconciles_stale_marker() {
        if std::env::var_os(CHILD_MODE).is_some() {
            return;
        }
        let root = TestRoot::new();
        let test_executable = std::env::current_exe().expect("test executable");
        let child = Command::new(test_executable)
            .args(["--exact", "tests::subprocess_lock_holder", "--nocapture"])
            .env(CHILD_MODE, "1")
            .env(CHILD_ROOT, &root.lock)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn lock holder");
        let mut child = ChildGuard(Some(child));
        wait_until(Duration::from_secs(5), || {
            root.lock.join(OWNER_MARKER_FILE).exists()
        });
        assert!(child.child_mut().try_wait().unwrap().is_none());
        let active = MutationLock::inspect(&root.lock).expect("active inspection");
        assert_eq!(active.state, MutationLockState::Active);
        assert!(active.owner.is_some());

        child.kill_and_wait();
        wait_until(Duration::from_secs(5), || {
            MutationLock::inspect(&root.lock)
                .is_ok_and(|inspection| inspection.state == MutationLockState::Stale)
        });
        let stale = MutationLock::inspect(&root.lock).unwrap();
        assert_eq!(stale.state, MutationLockState::Stale);

        let replacement_owner = owner(OWNER_UUID_2);
        let replacement = MutationLock::acquire(
            &root.lock,
            replacement_owner.clone(),
            &mut FakeLegacyProbe::default(),
        )
        .expect("replace stale owner after OS lock release");
        let inspection = MutationLock::inspect(&root.lock).unwrap();
        assert_eq!(inspection.state, MutationLockState::Active);
        assert_eq!(inspection.owner, Some(replacement_owner));
        drop(replacement);
        assert_eq!(
            MutationLock::inspect(&root.lock).unwrap().state,
            MutationLockState::Unlocked
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires the patched installed CodexQuotaViewer and explicit acceptance environment"]
    fn installed_legacy_viewer_shared_protocol_matches_expected_state() {
        let home = PathBuf::from(
            std::env::var_os("CAPACITY_MUTATION_ACCEPTANCE_HOME")
                .expect("explicit acceptance home"),
        );
        let expected_lock = match std::env::var("CAPACITY_MUTATION_EXPECTED_LOCK_STATE")
            .expect("expected lock state")
            .as_str()
        {
            "active" => MutationLockState::Active,
            "stale" => MutationLockState::Stale,
            "unlocked" => MutationLockState::Unlocked,
            _ => panic!("unsupported expected lock state"),
        };
        let expected_viewer = match std::env::var("CAPACITY_MUTATION_EXPECTED_VIEWER_STATE")
            .expect("expected Viewer state")
            .as_str()
        {
            "running" => LegacyViewerState::Running,
            "not_running" => LegacyViewerState::NotRunning,
            _ => panic!("unsupported expected Viewer state"),
        };
        let root = macos_shared_mutation_lock_root(home).expect("shared root");
        let inspection = MutationLock::inspect(root).expect("cross-language inspection");
        assert_eq!(inspection.state, expected_lock);
        if matches!(
            expected_lock,
            MutationLockState::Active | MutationLockState::Stale
        ) {
            assert!(inspection.owner.is_some());
        }
        let mut probe = SystemLegacyViewerProbe;
        assert_eq!(probe.legacy_viewer_state(), expected_viewer);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "launches the patched installed Viewer while Rust owns the real shared lease"]
    fn rust_owner_blocks_real_legacy_viewer_startup() {
        let home = PathBuf::from(
            std::env::var_os("CAPACITY_MUTATION_ACCEPTANCE_HOME")
                .expect("explicit acceptance home"),
        );
        let viewer = PathBuf::from(
            std::env::var_os("CAPACITY_MUTATION_VIEWER_EXECUTABLE")
                .expect("explicit installed Viewer executable"),
        );
        assert!(viewer.is_absolute());
        let mut system_probe = SystemLegacyViewerProbe;
        assert_eq!(
            system_probe.legacy_viewer_state(),
            LegacyViewerState::NotRunning,
            "stop the installed Viewer before this acceptance"
        );
        let root = macos_shared_mutation_lock_root(home).expect("shared root");
        let expected_owner = owner(OWNER_UUID_2);
        let lease = MutationLock::acquire(
            &root,
            expected_owner.clone(),
            &mut FakeLegacyProbe::default(),
        )
        .expect("Rust shared owner");
        let child = Command::new(viewer)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("launch installed Viewer");
        let mut child = ChildGuard(Some(child));
        wait_until(Duration::from_secs(5), || {
            let mut probe = SystemLegacyViewerProbe;
            probe.legacy_viewer_state() == LegacyViewerState::Running
        });
        thread::sleep(Duration::from_millis(500));
        assert!(child.child_mut().try_wait().unwrap().is_none());
        lease.verify_held().expect("Rust lease remains held");
        let inspection = MutationLock::inspect(&root).expect("active Rust owner");
        assert_eq!(inspection.state, MutationLockState::Active);
        assert_eq!(inspection.owner, Some(expected_owner));

        child.kill_and_wait();
        drop(lease);
        assert_eq!(
            MutationLock::inspect(&root).unwrap().state,
            MutationLockState::Unlocked
        );
    }
}
