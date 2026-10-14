//! Native local-volume filesystem boundary. All handles reject reparse points;
//! ancestor handles deny delete sharing until the operation finishes.
use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::os::windows::{
    ffi::OsStrExt,
    io::{AsRawHandle, FromRawHandle},
};
use std::path::{Component, Path, Prefix};
use std::ptr::{null, null_mut};
use std::sync::Arc;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Threading::*;

fn denied() -> io::Error {
    io::Error::from(io::ErrorKind::PermissionDenied)
}
fn last() -> io::Error {
    io::Error::last_os_error()
}

pub(crate) fn path_key(path: &Path) -> std::path::PathBuf {
    let text = path.to_string_lossy().replace('/', "\\");
    std::path::PathBuf::from(text.strip_prefix("\\\\?\\").unwrap_or(&text).to_uppercase())
}

pub(crate) fn validate_path(path: &Path) -> Result<(), ()> {
    if !path.is_absolute() || path.parent().is_none() {
        return Err(());
    }
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Prefix(prefix))
        if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
    {
        return Err(());
    }
    // Reject Win32 aliases, ADS, DOS devices and normalized dot segments before
    // conversion to the extended namespace (including embedded NUL).
    let text = path.as_os_str().to_str().ok_or(())?;
    for part in text
        .split(['\\', '/'])
        .skip(if text.starts_with("\\\\?\\") { 4 } else { 1 })
    {
        if part.is_empty() {
            continue;
        }
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part == "."
            || part == ".."
            || part.contains('~')
            || part.ends_with(['.', ' '])
            || part.chars().any(|c| c < ' ' || ":*?\"<>|".contains(c))
            || matches!(
                stem.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            )
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(());
        }
    }
    if components.any(|c| matches!(c, Component::CurDir | Component::ParentDir)) {
        return Err(());
    }
    Ok(())
}

fn wide(path: &Path) -> io::Result<Vec<u16>> {
    // Ancestor drive roots are allowed internally, never as public store roots.
    let text = path
        .as_os_str()
        .to_str()
        .ok_or_else(denied)?
        .replace('/', "\\");
    if text.contains('\0') {
        return Err(denied());
    }
    let extended = if text.starts_with("\\\\?\\") {
        text
    } else {
        format!("\\\\?\\{text}")
    };
    Ok(std::ffi::OsStr::new(&extended)
        .encode_wide()
        .chain(Some(0))
        .collect())
}

struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn user_sid() -> io::Result<String> {
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(last());
        }
        let token = std::os::windows::io::OwnedHandle::from_raw_handle(token);
        let mut size = 0;
        GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut size);
        if size == 0 {
            return Err(last());
        }
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        ) == 0
        {
            return Err(last());
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        sid_string(user.User.Sid)
    }
}

unsafe fn sid_string(sid: PSID) -> io::Result<String> {
    unsafe {
        if sid.is_null() || IsValidSid(sid) == 0 {
            return Err(denied());
        }
        let mut text = null_mut();
        if ConvertSidToStringSidW(sid, &mut text) == 0 {
            return Err(last());
        }
        let _allocation = Local(text.cast());
        let mut size = 0;
        while *text.add(size) != 0 {
            size += 1;
        }
        String::from_utf16(std::slice::from_raw_parts(text, size)).map_err(|_| denied())
    }
}

fn descriptor() -> io::Result<Local> {
    let sid = user_sid()?;
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = null_mut();
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err(last());
        }
    }
    Ok(Local(descriptor))
}

fn private_acl(file: &std::fs::File) -> io::Result<()> {
    unsafe {
        let mut owner = null_mut();
        let mut acl: *mut ACL = null_mut();
        let mut descriptor = null_mut();
        let status = GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor,
        );
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let _allocation = Local(descriptor);
        let user = user_sid()?;
        let mut control = 0;
        let mut revision = 0;
        if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0 {
            return Err(last());
        }
        if sid_string(owner)? != user
            || acl.is_null()
            || IsValidAcl(acl) == 0
            || control & SE_DACL_PROTECTED == 0
            || (*acl).AceCount != 2
        {
            return Err(denied());
        }
        let mut user_seen = false;
        let mut system_seen = false;
        for index in 0..u32::from((*acl).AceCount) {
            let mut ace = null_mut();
            if GetAce(acl, index, &mut ace) == 0 {
                return Err(last());
            }
            let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
            if ace.Header.AceType != 0
                || u32::from(ace.Header.AceFlags) & !(OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE)
                    != 0
                || ace.Mask != FILE_ALL_ACCESS
            {
                return Err(denied());
            }
            let sid = sid_string((&ace.SidStart as *const u32).cast_mut().cast())?;
            if sid == user && !user_seen {
                user_seen = true;
            } else if sid == "S-1-5-18" && !system_seen {
                system_seen = true;
            } else {
                return Err(denied());
            }
        }
        if !user_seen || !system_seen {
            return Err(denied());
        }
        Ok(())
    }
}

fn info(file: &std::fs::File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    unsafe {
        let mut info = std::mem::zeroed();
        if GetFileInformationByHandle(file.as_raw_handle(), &mut info) == 0 {
            return Err(last());
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(denied());
        }
        Ok(info)
    }
}

fn open_raw(
    path: &Path,
    access: u32,
    disposition: u32,
    security: *const SECURITY_ATTRIBUTES,
) -> io::Result<std::fs::File> {
    let name = wide(path)?;
    unsafe {
        let handle = CreateFileW(
            name.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            security,
            disposition,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(last());
        }
        let file = std::fs::File::from_raw_handle(handle);
        info(&file)?;
        Ok(file)
    }
}

fn ancestors(path: &Path) -> io::Result<Vec<std::fs::File>> {
    validate_path(path).map_err(|_| denied())?;
    let drive = path.ancestors().last().ok_or_else(denied)?;
    unsafe {
        // DRIVE_FIXED = 3 (Win32 System/WindowsProgramming).
        if GetDriveTypeW(wide(drive)?.as_ptr()) != 3 {
            return Err(denied());
        }
    }
    let mut result = Vec::new();
    let parents: Vec<_> = path.ancestors().skip(1).collect();
    for parent in parents.into_iter().rev() {
        let file = open_raw(parent, FILE_READ_ATTRIBUTES, OPEN_EXISTING, null())?;
        if info(&file)?.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(denied());
        }
        result.push(file);
    }
    Ok(result)
}

#[derive(Clone)]
pub(crate) struct Metadata {
    inner: std::fs::Metadata,
    identity: (u32, u32, u32),
    links: u32,
    file: Arc<std::fs::File>,
    _parents: Arc<Vec<std::fs::File>>,
}
impl std::ops::Deref for Metadata {
    type Target = std::fs::Metadata;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl Metadata {
    pub(crate) fn verify_private(&self) -> io::Result<()> {
        if self.is_file() && self.links != 1 {
            return Err(denied());
        }
        private_acl(&self.file)
    }
    pub(crate) fn same_identity(&self, other: &Self) -> bool {
        self.identity == other.identity
    }
}

pub(crate) struct File {
    inner: Arc<std::fs::File>,
    parents: Arc<Vec<std::fs::File>>,
}
impl File {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let parents = Arc::new(ancestors(path)?);
        Ok(Self {
            inner: Arc::new(open_raw(
                path,
                GENERIC_READ | READ_CONTROL,
                OPEN_EXISTING,
                null(),
            )?),
            parents,
        })
    }
    pub(crate) fn metadata(&self) -> io::Result<Metadata> {
        let info = info(&self.inner)?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 && info.nNumberOfLinks != 1 {
            return Err(denied());
        }
        Ok(Metadata {
            inner: self.inner.metadata()?,
            identity: (
                info.dwVolumeSerialNumber,
                info.nFileIndexHigh,
                info.nFileIndexLow,
            ),
            links: info.nNumberOfLinks,
            file: self.inner.clone(),
            _parents: self.parents.clone(),
        })
    }
}
impl Read for File {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        (&*self.inner).read(bytes)
    }
}

pub(crate) fn metadata(path: &Path) -> io::Result<Metadata> {
    let parents = Arc::new(ancestors(path)?);
    File {
        inner: Arc::new(open_raw(
            path,
            FILE_READ_ATTRIBUTES | READ_CONTROL,
            OPEN_EXISTING,
            null(),
        )?),
        parents,
    }
    .metadata()
}

pub(crate) fn create_directory(path: &Path) -> io::Result<()> {
    let _parents = ancestors(path)?;
    let descriptor = descriptor()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    unsafe {
        if CreateDirectoryW(wide(path)?.as_ptr(), &attributes) == 0 {
            return Err(last());
        }
    }
    metadata(path)?.verify_private()?;
    sync_directory(path.parent().ok_or_else(denied)?)
}

pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let _parents = ancestors(path)?;
    let descriptor = descriptor()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let mut file = open_raw(path, GENERIC_WRITE | READ_CONTROL, CREATE_NEW, &attributes)?;
    private_acl(&file)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_directory(path.parent().ok_or_else(denied)?)
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    let _parents = ancestors(path)?;
    let file = open_raw(path, GENERIC_WRITE, OPEN_EXISTING, null())?;
    if info(&file)?.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(denied());
    }
    file.sync_all()
}

pub(crate) fn rename(source: &Path, destination: &Path, replace: bool) -> io::Result<()> {
    let _source_parents = ancestors(source)?;
    let _destination_parents = ancestors(destination)?;
    // The source and destination are inspected without following reparse points.
    // Drop leaf handles before rename; ancestor handles remain pinned.
    metadata(source)?;
    match metadata(destination) {
        Ok(_) if !replace => return Err(io::ErrorKind::AlreadyExists.into()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let flags = MOVEFILE_WRITE_THROUGH
        | if replace {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };
    unsafe {
        if MoveFileExW(wide(source)?.as_ptr(), wide(destination)?.as_ptr(), flags) == 0 {
            return Err(last());
        }
    }
    sync_directory(destination.parent().ok_or_else(denied)?)?;
    if source.parent() != destination.parent() {
        sync_directory(source.parent().ok_or_else(denied)?)?;
    }
    Ok(())
}

pub(crate) mod fs {
    pub(crate) use std::fs::read_dir;
    pub(crate) fn symlink_metadata(
        path: impl AsRef<std::path::Path>,
    ) -> std::io::Result<super::Metadata> {
        super::metadata(path.as_ref())
    }
    pub(crate) fn rename(
        source: impl AsRef<std::path::Path>,
        destination: impl AsRef<std::path::Path>,
    ) -> std::io::Result<()> {
        super::rename(source.as_ref(), destination.as_ref(), false)
    }
    pub(crate) fn remove_file(path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        let _parents = super::ancestors(path)?;
        super::metadata(path)?;
        std::fs::remove_file(path)
    }
    pub(crate) fn remove_dir_all(path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        let _parents = super::ancestors(path)?;
        let directory = super::metadata(path)?;
        if !directory.is_dir() {
            return Err(super::denied());
        }
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let metadata = super::metadata(&entry.path())?;
            let is_dir = metadata.is_dir();
            drop(metadata);
            if is_dir {
                remove_dir_all(entry.path())?;
            } else {
                remove_file(entry.path())?;
            }
        }
        drop(directory);
        std::fs::remove_dir(path)
    }
}
