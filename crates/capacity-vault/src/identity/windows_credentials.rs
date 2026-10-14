use std::fmt;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_NO_SUCH_LOGON_SESSION, ERROR_NOT_FOUND, GetLastError,
    HANDLE, LocalFree, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexExW, GetCurrentProcess, OpenProcessToken, ReleaseMutex, WaitForSingleObject,
};

use super::{
    IdentityError, InstallationKey, InstallationKeyRingMutationOutcome as Outcome,
    InstallationKeyRingRecord, InstallationKeyRingRevision, InstallationKeyRingStore,
    InstallationKeyStore, MAX_KEY_RING_RECORD_BYTES, encode_key_ring_record, parse_key_ring_record,
};

const TARGET: &str = "dev.capacity-planner.installation-key.v1/active-installation-key";
const MUTEX_ACCESS: u32 = 0x0010_0001; // SYNCHRONIZE | MUTEX_MODIFY_STATE

/// Current-user Credential Manager storage. Construction performs no OS access.
/// All operations hold a per-user, cross-session mutex; callers must still hold
/// the product-wide mutation lease for workflows spanning other stores.
pub struct WindowsCredentialInstallationKeyStore {
    backend: Box<dyn CredentialBackend>,
}

impl WindowsCredentialInstallationKeyStore {
    pub fn new() -> Self {
        Self {
            backend: Box::new(NativeBackend {
                target: TARGET.to_owned(),
            }),
        }
    }

    fn read_locked(&self) -> Result<Option<InstallationKeyRingRecord>, IdentityError> {
        self.backend
            .read()?
            .map(|bytes| {
                parse_key_ring_record(bytes).map_err(|_| IdentityError::PlatformCredentialCorrupt)
            })
            .transpose()
    }

    fn write_verified(&self, record: &InstallationKeyRingRecord) -> Result<(), IdentityError> {
        let mut bytes = encode_key_ring_record(record);
        let result = self.backend.write(&bytes);
        bytes.fill(0);
        result?;
        let observed = self
            .read_locked()?
            .ok_or(IdentityError::PlatformCredentialCorrupt)?;
        if !record.same_material(&observed) {
            return Err(IdentityError::PlatformCredentialCorrupt);
        }
        Ok(())
    }
}

impl Default for WindowsCredentialInstallationKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for WindowsCredentialInstallationKeyStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WindowsCredentialInstallationKeyStore(<redacted>)")
    }
}

impl InstallationKeyStore for WindowsCredentialInstallationKeyStore {
    fn load(&mut self) -> Result<Option<InstallationKey>, IdentityError> {
        self.load_key_ring()
            .map(|record| record.map(InstallationKeyRingRecord::into_active_key))
    }

    fn load_or_create(&mut self) -> Result<InstallationKey, IdentityError> {
        self.load_or_create_key_ring()
            .map(InstallationKeyRingRecord::into_active_key)
    }
}

impl InstallationKeyRingStore for WindowsCredentialInstallationKeyStore {
    fn load_key_ring(&mut self) -> Result<Option<InstallationKeyRingRecord>, IdentityError> {
        let _lock = self.backend.lock()?;
        self.read_locked()
    }

    fn load_or_create_key_ring(&mut self) -> Result<InstallationKeyRingRecord, IdentityError> {
        let _lock = self.backend.lock()?;
        if let Some(existing) = self.read_locked()? {
            return Ok(existing);
        }
        let record = InstallationKeyRingRecord::initial(InstallationKey::generate()?);
        self.write_verified(&record)?;
        Ok(record)
    }

    fn replace_key_ring(
        &mut self,
        expected: &InstallationKeyRingRevision,
        replacement: &InstallationKeyRingRecord,
    ) -> Result<Outcome, IdentityError> {
        if !replacement.is_next_revision_of(expected) {
            return Err(IdentityError::InvalidKeyRing);
        }
        let _lock = self.backend.lock()?;
        let Some(current) = self.read_locked()? else {
            return Ok(Outcome::Missing);
        };
        if !current.matches_revision(expected) {
            return Ok(Outcome::RevisionConflict);
        }
        if !replacement.is_valid_successor_of(&current) {
            return Err(IdentityError::InvalidKeyRing);
        }
        self.write_verified(replacement)?;
        Ok(Outcome::Applied)
    }

    fn delete_key_ring(
        &mut self,
        expected: &InstallationKeyRingRevision,
    ) -> Result<Outcome, IdentityError> {
        let _lock = self.backend.lock()?;
        let Some(current) = self.read_locked()? else {
            return Ok(Outcome::Missing);
        };
        if !current.matches_revision(expected) {
            return Ok(Outcome::RevisionConflict);
        }
        if !self.backend.delete()? {
            return Ok(Outcome::Missing);
        }
        if self.read_locked()?.is_some() {
            return Err(IdentityError::PlatformCredentialCorrupt);
        }
        Ok(Outcome::Applied)
    }
}

// The guard must cover read, validation, write/delete, and postflight together.
trait CredentialLock {}
trait CredentialBackend: Send + Sync {
    fn lock(&self) -> Result<Box<dyn CredentialLock + '_>, IdentityError>;
    fn read(&self) -> Result<Option<Vec<u8>>, IdentityError>;
    fn write(&self, bytes: &[u8]) -> Result<(), IdentityError>;
    fn delete(&self) -> Result<bool, IdentityError>;
}

struct NativeBackend {
    target: String,
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn os_error(code: u32) -> IdentityError {
    match code {
        ERROR_ACCESS_DENIED => IdentityError::PlatformCredentialDenied,
        ERROR_NO_SUCH_LOGON_SESSION => IdentityError::PlatformCredentialUnavailable,
        _ => IdentityError::PlatformCredentialFailed,
    }
}

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct LocalAllocation(*mut std::ffi::c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

struct MutexGuard {
    handle: OwnedHandle,
    // Windows mutex ownership is tied to the acquiring thread.
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl CredentialLock for MutexGuard {}
impl Drop for MutexGuard {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.handle.0);
        }
    }
}

fn current_user_sid() -> Result<String, IdentityError> {
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(os_error(GetLastError()));
        }
        let token = OwnedHandle(token);
        let mut size = 0;
        GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut size);
        if size == 0 {
            return Err(IdentityError::PlatformCredentialFailed);
        }
        // TOKEN_USER contains pointers, so the backing buffer must be aligned.
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        ) == 0
        {
            return Err(os_error(GetLastError()));
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut sid = null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
            return Err(os_error(GetLastError()));
        }
        let _allocation = LocalAllocation(sid.cast());
        let mut len = 0;
        while *sid.add(len) != 0 {
            len += 1;
        }
        String::from_utf16(std::slice::from_raw_parts(sid, len))
            .map_err(|_| IdentityError::PlatformCredentialFailed)
    }
}

impl CredentialBackend for NativeBackend {
    fn lock(&self) -> Result<Box<dyn CredentialLock + '_>, IdentityError> {
        let sid = current_user_sid()?;
        let target_hash = super::encode_lower_hex(&super::sha256(self.target.as_bytes()));
        let name = wide(&format!(
            "Global\\capacity-installation-key-{sid}-{target_hash}"
        ));
        // Explicit protected DACL: only this user may wait/release. No default
        // DACL, world access, or session-local fallback on access failure.
        let sddl = wide(&format!("D:P(A;;0x00100001;;;{sid})"));
        unsafe {
            let mut descriptor = null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            ) == 0
            {
                return Err(os_error(GetLastError()));
            }
            let _allocation = LocalAllocation(descriptor);
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            let handle = CreateMutexExW(&attributes, name.as_ptr(), 0, MUTEX_ACCESS);
            if handle.is_null() {
                return Err(os_error(GetLastError()));
            }
            let handle = OwnedHandle(handle);
            let status = WaitForSingleObject(handle.0, 30_000);
            if status == WAIT_OBJECT_0 {
                Ok(Box::new(MutexGuard {
                    handle,
                    _thread: std::marker::PhantomData,
                }))
            } else {
                // An abandoned owner may have died mid-mutation. Do not create
                // or overwrite anything in this operation, even though we own it.
                if status == WAIT_ABANDONED {
                    ReleaseMutex(handle.0);
                }
                Err(IdentityError::PlatformCredentialFailed)
            }
        }
    }

    fn read(&self) -> Result<Option<Vec<u8>>, IdentityError> {
        let target = wide(&self.target);
        unsafe {
            let mut credential = null_mut();
            if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) == 0 {
                let error = GetLastError();
                return if error == ERROR_NOT_FOUND {
                    Ok(None)
                } else {
                    Err(os_error(error))
                };
            }
            if credential.is_null() {
                return Err(IdentityError::PlatformCredentialCorrupt);
            }
            let credential = CredentialAllocation(credential);
            let value = &*credential.0;
            if value.Type != CRED_TYPE_GENERIC
                || value.CredentialBlobSize == 0
                || u64::from(value.CredentialBlobSize) > MAX_KEY_RING_RECORD_BYTES
                || value.CredentialBlob.is_null()
            {
                return Err(IdentityError::PlatformCredentialCorrupt);
            }
            Ok(Some(
                std::slice::from_raw_parts(value.CredentialBlob, value.CredentialBlobSize as usize)
                    .to_vec(),
            ))
        }
    }

    fn write(&self, bytes: &[u8]) -> Result<(), IdentityError> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_KEY_RING_RECORD_BYTES {
            return Err(IdentityError::PlatformCredentialCorrupt);
        }
        let mut target = wide(&self.target);
        let mut username = wide("active-installation-key");
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            CredentialBlobSize: bytes.len() as u32,
            CredentialBlob: bytes.as_ptr().cast_mut(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: username.as_mut_ptr(),
            ..Default::default()
        };
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            Err(os_error(unsafe { GetLastError() }))
        } else {
            Ok(())
        }
    }

    fn delete(&self) -> Result<bool, IdentityError> {
        let target = wide(&self.target);
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } != 0 {
            return Ok(true);
        }
        let error = unsafe { GetLastError() };
        if error == ERROR_NOT_FOUND {
            Ok(false)
        } else {
            Err(os_error(error))
        }
    }
}

struct CredentialAllocation(*mut CREDENTIALW);
impl Drop for CredentialAllocation {
    fn drop(&mut self) {
        unsafe {
            let credential = &*self.0;
            if !credential.CredentialBlob.is_null() {
                for index in 0..credential.CredentialBlobSize as usize {
                    credential.CredentialBlob.add(index).write_volatile(0);
                }
            }
            CredFree(self.0.cast());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex, MutexGuard as StdGuard};

    #[derive(Default)]
    struct State {
        bytes: Option<Vec<u8>>,
        error: Option<IdentityError>,
        writes: usize,
        deletes: usize,
        corrupt_write: bool,
    }

    #[derive(Clone, Default)]
    struct Fake {
        lease: Arc<Mutex<()>>,
        state: Arc<Mutex<State>>,
    }
    impl CredentialLock for StdGuard<'_, ()> {}
    impl CredentialBackend for Fake {
        fn lock(&self) -> Result<Box<dyn CredentialLock + '_>, IdentityError> {
            Ok(Box::new(self.lease.lock().unwrap()))
        }
        fn read(&self) -> Result<Option<Vec<u8>>, IdentityError> {
            let state = self.state.lock().unwrap();
            if let Some(error) = &state.error {
                return Err(error.clone());
            }
            Ok(state.bytes.clone())
        }
        fn write(&self, bytes: &[u8]) -> Result<(), IdentityError> {
            let mut state = self.state.lock().unwrap();
            state.writes += 1;
            state.bytes = Some(if state.corrupt_write {
                b"secret-canary-corrupt".to_vec()
            } else {
                bytes.to_vec()
            });
            Ok(())
        }
        fn delete(&self) -> Result<bool, IdentityError> {
            let mut state = self.state.lock().unwrap();
            state.deletes += 1;
            Ok(state.bytes.take().is_some())
        }
    }
    fn store(fake: &Fake) -> WindowsCredentialInstallationKeyStore {
        WindowsCredentialInstallationKeyStore {
            backend: Box::new(fake.clone()),
        }
    }

    #[test]
    fn fake_create_load_rotate_cas_delete() {
        let fake = Fake::default();
        let mut first = store(&fake);
        let mut second = store(&fake);
        assert!(first.load().unwrap().is_none());
        let initial = first.load_or_create_key_ring().unwrap();
        assert!(initial.same_material(&second.load_or_create_key_ring().unwrap()));
        assert_eq!(fake.state.lock().unwrap().writes, 1);
        let old = initial.revision_token();
        let rotated = initial
            .start_rotation(InstallationKey::generate().unwrap())
            .unwrap();
        assert_eq!(
            first.replace_key_ring(&old, &rotated).unwrap(),
            Outcome::Applied
        );
        assert_eq!(
            second.replace_key_ring(&old, &rotated).unwrap(),
            Outcome::RevisionConflict
        );
        assert_eq!(
            second.delete_key_ring(&old).unwrap(),
            Outcome::RevisionConflict
        );
        assert!(rotated.same_material(&second.load_key_ring().unwrap().unwrap()));
        assert_eq!(
            first.delete_key_ring(&rotated.revision_token()).unwrap(),
            Outcome::Applied
        );
        assert_eq!(first.delete_key_ring(&old).unwrap(), Outcome::Missing);
        assert_eq!(
            first.replace_key_ring(&old, &rotated).unwrap(),
            Outcome::Missing
        );
    }

    #[test]
    fn fake_corrupt_denied_and_unavailable_never_recreate_or_delete() {
        for error in [
            None,
            Some(IdentityError::PlatformCredentialDenied),
            Some(IdentityError::PlatformCredentialUnavailable),
        ] {
            let fake = Fake::default();
            fake.state.lock().unwrap().bytes = Some(b"secret-canary-corrupt".to_vec());
            fake.state.lock().unwrap().error = error.clone();
            let expected = error.unwrap_or(IdentityError::PlatformCredentialCorrupt);
            let mut store = store(&fake);
            let initial = InstallationKeyRingRecord::initial(InstallationKey::generate().unwrap());
            let revision = initial.revision_token();
            let next = initial
                .start_rotation(InstallationKey::generate().unwrap())
                .unwrap();
            assert_eq!(store.load().unwrap_err(), expected);
            assert_eq!(store.load_or_create().unwrap_err(), expected);
            assert_eq!(
                store.replace_key_ring(&revision, &next).unwrap_err(),
                expected
            );
            assert_eq!(store.delete_key_ring(&revision).unwrap_err(), expected);
            assert!(!format!("{store:?} {expected:?} {expected}").contains("secret-canary"));
            let state = fake.state.lock().unwrap();
            assert_eq!((state.writes, state.deletes), (0, 0));
        }
    }

    #[test]
    fn fake_rejects_invalid_successor_and_failed_postflight() {
        let fake = Fake::default();
        let mut store = store(&fake);
        let initial = store.load_or_create_key_ring().unwrap();
        let expected = initial.revision_token();
        let unrelated = InstallationKeyRingRecord::initial(InstallationKey::generate().unwrap())
            .start_rotation(InstallationKey::generate().unwrap())
            .unwrap();
        assert_eq!(
            store.replace_key_ring(&expected, &unrelated).unwrap_err(),
            IdentityError::InvalidKeyRing
        );
        let next = initial
            .start_rotation(InstallationKey::generate().unwrap())
            .unwrap();
        fake.state.lock().unwrap().corrupt_write = true;
        assert_eq!(
            store.replace_key_ring(&expected, &next).unwrap_err(),
            IdentityError::PlatformCredentialCorrupt
        );
        assert!(!format!("{next:?} {expected:?} {store:?}").contains(next.active_key_id()));
    }

    #[test]
    fn fake_concurrent_creators_share_one_key() {
        let fake = Fake::default();
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let fake = fake.clone();
                std::thread::spawn(move || {
                    store(&fake).load_or_create().unwrap().key_id().to_owned()
                })
            })
            .collect();
        let ids: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert!(ids.iter().all(|id| id == &ids[0]));
        assert_eq!(fake.state.lock().unwrap().writes, 1);
    }

    #[test]
    #[ignore = "explicit opt-in: creates and deletes only a fresh random test credential"]
    fn native_unique_credential_roundtrip() {
        let unique = InstallationKey::generate().unwrap();
        let target = format!(
            "dev.capacity-planner.test.installation-key/{}",
            unique.key_id()
        );
        let backend = NativeBackend {
            target: target.clone(),
        };
        // Never overwrite even an unexpected collision in this random namespace.
        {
            let _guard = backend.lock().unwrap();
            assert!(backend.read().unwrap().is_none());
        }
        struct Cleanup(NativeBackend);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Ok(_guard) = self.0.lock() {
                    let _ = self.0.delete();
                }
            }
        }
        let _cleanup = Cleanup(backend);
        let mut store = WindowsCredentialInstallationKeyStore {
            backend: Box::new(NativeBackend {
                target: target.clone(),
            }),
        };
        let mut other = WindowsCredentialInstallationKeyStore {
            backend: Box::new(NativeBackend { target }),
        };
        let initial = store.load_or_create_key_ring().unwrap();
        assert!(initial.same_material(&other.load_or_create_key_ring().unwrap()));
        let expected = initial.revision_token();
        let next = initial
            .start_rotation(InstallationKey::generate().unwrap())
            .unwrap();
        assert_eq!(
            store.replace_key_ring(&expected, &next).unwrap(),
            Outcome::Applied
        );
        assert_eq!(
            other.replace_key_ring(&expected, &next).unwrap(),
            Outcome::RevisionConflict
        );
        assert_eq!(
            other.delete_key_ring(&expected).unwrap(),
            Outcome::RevisionConflict
        );
        assert!(next.same_material(&other.load_key_ring().unwrap().unwrap()));
        assert_eq!(
            store.delete_key_ring(&next.revision_token()).unwrap(),
            Outcome::Applied
        );
        assert!(other.load().unwrap().is_none());
    }
}
