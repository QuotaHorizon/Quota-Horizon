#![cfg(windows)]

use capacity_domain::UtcTimestamp;
use capacity_mutation::{
    MutationLock, MutationLockError, MutationLockOwner, MutationLockState, MutationOwnerId,
    ProtectedTarget, ProtectedTargetPath, RestorePointOperation, RestorePointStore,
    SystemLegacyViewerProbe,
};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
};

fn temporary() -> tempfile::TempDir {
    assert!(
        !std::env::temp_dir()
            .to_string_lossy()
            .to_lowercase()
            .contains("systemprofile")
    );
    tempfile::tempdir().unwrap()
}

#[test]
fn lock_busy_and_release() {
    let temporary = temporary();
    let root = temporary.path().join("mutation-lock");
    let owner = MutationLockOwner::new(
        MutationOwnerId::parse("mutation-owner:v1:018f47a2-8a71-4f4a-9c35-1f4234a73801").unwrap(),
        std::process::id(),
        UtcTimestamp::parse("2026-10-14T00:00:00Z").unwrap(),
    )
    .unwrap();
    let lock = MutationLock::acquire(&root, owner.clone(), &mut SystemLegacyViewerProbe).unwrap();
    assert!(matches!(
        MutationLock::acquire(&root, owner.clone(), &mut SystemLegacyViewerProbe),
        Err(MutationLockError::Busy)
    ));
    lock.revalidate(&mut SystemLegacyViewerProbe).unwrap();
    lock.release().unwrap();
    assert_eq!(
        MutationLock::inspect(&root).unwrap().state,
        MutationLockState::Unlocked
    );
    drop(MutationLock::acquire(&root, owner, &mut SystemLegacyViewerProbe).unwrap());
}

#[test]
fn restore_capture_commit_rollback() {
    let temporary = temporary();
    let store = RestorePointStore::new(temporary.path().join("恢复 点")).unwrap();
    let path = temporary.path().join("测试 文件.json");
    std::fs::write(&path, b"before").unwrap();
    let targets = [ProtectedTargetPath::new(ProtectedTarget::CurrentAuth, &path).unwrap()];
    let time = UtcTimestamp::parse("2026-10-14T00:00:00Z").unwrap();
    let point = store
        .create_restore_point(
            RestorePointOperation::SwitchAccount,
            &targets,
            false,
            &time,
            &Default::default(),
        )
        .unwrap();
    std::fs::write(&path, b"after").unwrap();
    store.mark_committed(point.id(), &targets, &time).unwrap();
    store
        .rollback_committed(point.id(), &targets, &time)
        .unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"before");
}

fn owner() -> MutationLockOwner {
    MutationLockOwner::new(
        MutationOwnerId::parse("mutation-owner:v1:018f47a2-8a71-4f4a-9c35-1f4234a73801").unwrap(),
        std::process::id(),
        UtcTimestamp::parse("2026-10-14T00:00:00Z").unwrap(),
    )
    .unwrap()
}

fn unsafe_null_dacl(path: &Path) {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        assert_eq!(
            SetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null()
            ),
            0
        );
    }
}

#[test]
fn root_acl_drift_fails_closed() {
    let temp = temporary();
    let root = temp.path().join("lock");
    let lock = MutationLock::acquire(&root, owner(), &mut SystemLegacyViewerProbe).unwrap();
    unsafe_null_dacl(&root);
    assert!(matches!(
        lock.revalidate(&mut SystemLegacyViewerProbe),
        Err(MutationLockError::InsecurePermissions)
    ));
    drop(lock);
    assert!(MutationLock::acquire(&root, owner(), &mut SystemLegacyViewerProbe).is_err());
}

#[test]
fn paths_and_long_unicode_restore() {
    for bad in [
        r"C:relative",
        r"\rooted",
        r"C:\a\..\b",
        r"C:\a\.\b",
        r"C:\a\file:stream",
        r"\\server\share\file",
        r"\\.\C:\file",
        r"C:\a\NUL",
        r"C:\a\trailing.",
    ] {
        assert!(
            ProtectedTargetPath::new(ProtectedTarget::CurrentAuth, bad).is_err(),
            "{bad}"
        );
    }
    let temp = temporary();
    let mut parent = temp.path().to_path_buf();
    for _ in 0..7 {
        parent.push("长路径 空格 abcdefghijklmnopqrstuvwxyz");
    }
    std::fs::create_dir_all(&parent).unwrap();
    let path = parent.join("测试.json");
    assert!(path.as_os_str().encode_wide().count() > 260);
    std::fs::write(&path, b"before").unwrap();
    let store = RestorePointStore::new(parent.join("restore")).unwrap();
    let targets = [ProtectedTargetPath::new(ProtectedTarget::CurrentAuth, &path).unwrap()];
    let time = UtcTimestamp::parse("2026-10-14T00:00:00Z").unwrap();
    let point = store
        .create_restore_point(
            RestorePointOperation::EnterProviderMode,
            &targets,
            false,
            &time,
            &Default::default(),
        )
        .unwrap();
    std::fs::write(&path, b"changed").unwrap();
    let current = store.observe(&targets).unwrap();
    store
        .rollback_prepared(point.id(), &targets, &current, &time)
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"before");
}

#[test]
fn rollback_does_not_overwrite_external_changes_or_follow_hardlinks() {
    let temp = temporary();
    let path = temp.path().join("target");
    let store = RestorePointStore::new(temp.path().join("restore")).unwrap();
    let targets = [ProtectedTargetPath::new(ProtectedTarget::CurrentAuth, &path).unwrap()];
    let time = UtcTimestamp::parse("2026-10-14T00:00:00Z").unwrap();
    let point = store
        .create_restore_point(
            RestorePointOperation::SwitchAccount,
            &targets,
            false,
            &time,
            &Default::default(),
        )
        .unwrap();
    std::fs::write(&path, b"operation").unwrap();
    store.mark_committed(point.id(), &targets, &time).unwrap();
    std::fs::write(&path, b"external").unwrap();
    assert!(
        store
            .rollback_committed(point.id(), &targets, &time)
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"external");
    std::fs::hard_link(&path, temp.path().join("alias")).unwrap();
    assert!(store.observe(&targets).is_err());
}

#[test]
fn child_lock_probe() {
    let Some(root) = std::env::var_os("CAPACITY_MUTATION_WINDOWS_CHILD_ROOT") else {
        return;
    };
    assert!(matches!(
        MutationLock::acquire(root, owner(), &mut SystemLegacyViewerProbe),
        Err(MutationLockError::Busy)
    ));
}

#[test]
fn cross_process_sqlite_lease() {
    let temp = temporary();
    let root = temp.path().join("lock");
    let lock = MutationLock::acquire(&root, owner(), &mut SystemLegacyViewerProbe).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_lock_probe", "--nocapture"])
        .env("CAPACITY_MUTATION_WINDOWS_CHILD_ROOT", &root)
        .status()
        .unwrap();
    assert!(status.success());
    drop(lock);
    drop(MutationLock::acquire(&root, owner(), &mut SystemLegacyViewerProbe).unwrap());
}

#[test]
fn restore_root_drift_fails_before_target_mutation() {
    let temp = temporary();
    let root = temp.path().join("restore");
    let path = temp.path().join("target");
    std::fs::write(&path, b"before").unwrap();
    let store = RestorePointStore::new(&root).unwrap();
    let targets = [ProtectedTargetPath::new(ProtectedTarget::CurrentAuth, &path).unwrap()];
    let time = UtcTimestamp::parse("2026-10-14T00:00:00Z").unwrap();
    let point = store
        .create_restore_point(
            RestorePointOperation::SwitchAccount,
            &targets,
            false,
            &time,
            &Default::default(),
        )
        .unwrap();
    std::fs::write(&path, b"after").unwrap();
    let current = store.observe(&targets).unwrap();
    unsafe_null_dacl(&root);
    assert!(
        store
            .rollback_prepared(point.id(), &targets, &current, &time)
            .is_err()
    );
    assert_eq!(std::fs::read(path).unwrap(), b"after");
}

#[test]
fn rejects_case_and_extended_namespace_target_aliases() {
    let temp = temporary();
    let path = temp.path().join("target.json");
    let store = RestorePointStore::new(temp.path().join("restore")).unwrap();
    let aliases = [
        ProtectedTargetPath::new(ProtectedTarget::CurrentAuth, &path).unwrap(),
        ProtectedTargetPath::new(
            ProtectedTarget::CurrentConfig,
            path.to_string_lossy().to_uppercase(),
        )
        .unwrap(),
    ];
    assert!(matches!(
        store.observe(&aliases),
        Err(capacity_mutation::RestorePointError::DuplicatePath)
    ));
    let extended = format!("\\\\?\\{}", path.display());
    assert!(ProtectedTargetPath::new(ProtectedTarget::CurrentAuth, extended).is_ok());
}

#[test]
fn rejects_reparse_leaf_and_ancestor() {
    let temp = temporary();
    let real = temp.path().join("real");
    let link = temp.path().join("link");
    std::fs::create_dir(&real).unwrap();
    create_junction(&real, &link);
    assert!(MutationLock::acquire(&link, owner(), &mut SystemLegacyViewerProbe).is_err());
    assert!(
        MutationLock::acquire(link.join("lock"), owner(), &mut SystemLegacyViewerProbe).is_err()
    );
    assert!(!real.join("lock").exists());
    let file = real.join("target");
    std::fs::write(&file, b"untouched").unwrap();
    let store = RestorePointStore::new(temp.path().join("restore")).unwrap();
    let targets =
        [ProtectedTargetPath::new(ProtectedTarget::CurrentAuth, link.join("target")).unwrap()];
    assert!(store.observe(&targets).is_err());
    assert_eq!(std::fs::read(file).unwrap(), b"untouched");
}

fn create_junction(real: &Path, link: &Path) {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::*;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn DeviceIoControl(
            handle: *mut std::ffi::c_void,
            code: u32,
            input: *const std::ffi::c_void,
            input_len: u32,
            output: *mut std::ffi::c_void,
            output_len: u32,
            returned: *mut u32,
            overlapped: *mut std::ffi::c_void,
        ) -> i32;
    }
    std::fs::create_dir(link).unwrap();
    let target: Vec<u16> = format!("\\??\\{}", real.display()).encode_utf16().collect();
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&0xA0000003u32.to_le_bytes()); // IO_REPARSE_TAG_MOUNT_POINT
    buffer.extend_from_slice(&((8 + (target.len() + 2) * 2) as u16).to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    for value in [
        0,
        (target.len() * 2) as u16,
        ((target.len() + 1) * 2) as u16,
        0,
    ] {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    for value in target.into_iter().chain([0, 0]) {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    let wide: Vec<u16> = link.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let raw = CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        );
        assert_ne!(
            raw,
            INVALID_HANDLE_VALUE,
            "{}",
            std::io::Error::last_os_error()
        );
        let handle = OwnedHandle::from_raw_handle(raw);
        let mut returned = 0;
        assert_ne!(
            DeviceIoControl(
                handle.as_raw_handle(),
                0x000900A4,
                buffer.as_ptr().cast(),
                buffer.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut()
            ),
            0,
            "{}",
            std::io::Error::last_os_error()
        );
    }
}
