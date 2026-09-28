use std::ffi::c_void;
use std::fmt;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex, MutexGuard};

use super::{
    IdentityError, InstallationKey, InstallationKeyRingMutationOutcome, InstallationKeyRingRecord,
    InstallationKeyRingRevision, InstallationKeyRingStore, InstallationKeyStore,
    MAX_KEY_RING_RECORD_BYTES, OsSecureRandom, encode_key_ring_record, generate_installation_key,
    parse_key_ring_record,
};

pub const MACOS_KEYCHAIN_SERVICE: &str = "dev.capacity-planner.installation-key.v1";
pub const MACOS_KEYCHAIN_ACCOUNT: &str = "active-installation-key";
const MACOS_KEYCHAIN_LABEL: &str = "Codex Capacity Planner Installation Key";

const ERR_SEC_SUCCESS: i32 = 0;
const ERR_SEC_USER_CANCELED: i32 = -128;
const ERR_SEC_NOT_AVAILABLE: i32 = -25291;
const ERR_SEC_AUTH_FAILED: i32 = -25293;
const ERR_SEC_DUPLICATE_ITEM: i32 = -25299;
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;
const ERR_SEC_MISSING_ENTITLEMENT: i32 = -34018;

/// macOS Data Protection Keychain backend for the versioned installation key
/// ring. The fixed v1 service/account selector remains stable while the value
/// format can migrate from single-key v1 to key-ring v2.
///
/// Creating this value is side-effect free; the first explicit load/create or
/// mutation performs a Keychain operation. Replace/delete use revision,
/// active-key, full-material, and postflight checks, but Security.framework
/// does not provide an atomic value compare-and-swap. Callers must also hold
/// the product-wide mutation lease required by [`InstallationKeyRingStore`].
pub struct MacOsKeychainInstallationKeyStore {
    bridge: Arc<dyn KeychainBridge>,
}

impl MacOsKeychainInstallationKeyStore {
    pub fn new() -> Self {
        Self {
            bridge: Arc::new(SystemKeychainBridge(KeychainLocation::DataProtection)),
        }
    }

    /// OS-protected storage for locally signed desktop builds. Reuse an
    /// existing key in either macOS Keychain; only a missing signing entitlement
    /// may select the login Keychain. Denial, lock, corruption, and cancellation
    /// never create a replacement key or fall back to an app-owned plaintext file.
    pub fn compatible_with_local_builds() -> Self {
        Self {
            bridge: Arc::new(CompatibleKeychainBridge::new(
                Arc::new(SystemKeychainBridge(KeychainLocation::DataProtection)),
                Arc::new(SystemKeychainBridge(KeychainLocation::Login)),
            )),
        }
    }

    /// User-invoked authorization for this product's existing local history key.
    /// Never call from startup, a timer, or a quota refresh. No secret is returned
    /// to the host, and no key is created, replaced, or deleted here.
    pub fn authorize_local_history_access() -> Result<(), IdentityError> {
        let _interaction = KeychainInteractionGuard::enter(true).map_err(map_bridge_error)?;
        match keychain_copy_matching(
            MACOS_KEYCHAIN_SERVICE,
            MACOS_KEYCHAIN_ACCOUNT,
            KeychainLocation::Login,
        )
        .map_err(map_bridge_error)?
        {
            Some(encoded) => parse_key_ring_record(encoded)
                .map(drop)
                .map_err(|_| IdentityError::PlatformCredentialCorrupt),
            None => Ok(()),
        }
    }

    pub fn load(&self) -> Result<Option<InstallationKey>, IdentityError> {
        self.load_key_ring()
            .map(|record| record.map(InstallationKeyRingRecord::into_active_key))
    }

    pub fn load_key_ring(&self) -> Result<Option<InstallationKeyRingRecord>, IdentityError> {
        let Some(encoded) = self
            .bridge
            .load(MACOS_KEYCHAIN_SERVICE, MACOS_KEYCHAIN_ACCOUNT)
            .map_err(map_bridge_error)?
        else {
            return Ok(None);
        };
        parse_key_ring_record(encoded)
            .map(Some)
            .map_err(|_| IdentityError::PlatformCredentialCorrupt)
    }

    pub fn load_or_create(&self) -> Result<InstallationKey, IdentityError> {
        self.load_or_create_key_ring()
            .map(InstallationKeyRingRecord::into_active_key)
    }

    pub fn load_or_create_key_ring(&self) -> Result<InstallationKeyRingRecord, IdentityError> {
        if let Some(existing) = self.load_key_ring()? {
            return Ok(existing);
        }

        let mut random = OsSecureRandom;
        let generated = InstallationKeyRingRecord::initial(generate_installation_key(&mut random)?);
        let mut encoded = encode_key_ring_record(&generated);
        let create_result = self.bridge.create(
            MACOS_KEYCHAIN_SERVICE,
            MACOS_KEYCHAIN_ACCOUNT,
            MACOS_KEYCHAIN_LABEL,
            &encoded,
        );
        encoded.fill(0);

        match create_result.map_err(map_bridge_error)? {
            KeychainCreateOutcome::Added => {
                let installed = self
                    .load_key_ring()?
                    .ok_or(IdentityError::PlatformCredentialCorrupt)?;
                if !generated.same_material(&installed) {
                    return Err(IdentityError::PlatformCredentialCorrupt);
                }
                Ok(installed)
            }
            KeychainCreateOutcome::Duplicate => self
                .load_key_ring()?
                .ok_or(IdentityError::PlatformCredentialCorrupt),
        }
    }

    pub fn replace_key_ring(
        &self,
        expected: &InstallationKeyRingRevision,
        replacement: &InstallationKeyRingRecord,
    ) -> Result<InstallationKeyRingMutationOutcome, IdentityError> {
        if !replacement.is_next_revision_of(expected) {
            return Err(IdentityError::InvalidKeyRing);
        }
        let Some(current) = self.load_key_ring()? else {
            return Ok(InstallationKeyRingMutationOutcome::Missing);
        };
        if !current.matches_revision(expected) {
            return Ok(InstallationKeyRingMutationOutcome::RevisionConflict);
        }
        if !replacement.is_valid_successor_of(&current) {
            return Err(IdentityError::InvalidKeyRing);
        }

        let Some(observed) = self.load_key_ring()? else {
            return Ok(InstallationKeyRingMutationOutcome::Missing);
        };
        if !observed.matches_revision(expected) || !observed.same_material(&current) {
            return Ok(InstallationKeyRingMutationOutcome::RevisionConflict);
        }

        let mut encoded = encode_key_ring_record(replacement);
        let update_result =
            self.bridge
                .update(MACOS_KEYCHAIN_SERVICE, MACOS_KEYCHAIN_ACCOUNT, &encoded);
        encoded.fill(0);
        match update_result.map_err(map_bridge_error)? {
            KeychainMutationOutcome::Applied => {
                let Some(installed) = self.load_key_ring()? else {
                    return Ok(InstallationKeyRingMutationOutcome::Missing);
                };
                if replacement.same_material(&installed) {
                    Ok(InstallationKeyRingMutationOutcome::Applied)
                } else {
                    Ok(InstallationKeyRingMutationOutcome::RevisionConflict)
                }
            }
            KeychainMutationOutcome::Missing => Ok(InstallationKeyRingMutationOutcome::Missing),
        }
    }

    pub fn delete_key_ring(
        &self,
        expected: &InstallationKeyRingRevision,
    ) -> Result<InstallationKeyRingMutationOutcome, IdentityError> {
        let Some(current) = self.load_key_ring()? else {
            return Ok(InstallationKeyRingMutationOutcome::Missing);
        };
        if !current.matches_revision(expected) {
            return Ok(InstallationKeyRingMutationOutcome::RevisionConflict);
        }
        let Some(observed) = self.load_key_ring()? else {
            return Ok(InstallationKeyRingMutationOutcome::Missing);
        };
        if !observed.same_material(&current) {
            return Ok(InstallationKeyRingMutationOutcome::RevisionConflict);
        }

        match self
            .bridge
            .delete(MACOS_KEYCHAIN_SERVICE, MACOS_KEYCHAIN_ACCOUNT)
            .map_err(map_bridge_error)?
        {
            KeychainMutationOutcome::Applied => {
                if self.load_key_ring()?.is_none() {
                    Ok(InstallationKeyRingMutationOutcome::Applied)
                } else {
                    Ok(InstallationKeyRingMutationOutcome::RevisionConflict)
                }
            }
            KeychainMutationOutcome::Missing => Ok(InstallationKeyRingMutationOutcome::Missing),
        }
    }

    #[cfg(test)]
    fn with_bridge(bridge: Arc<dyn KeychainBridge>) -> Self {
        Self { bridge }
    }
}

impl Default for MacOsKeychainInstallationKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for MacOsKeychainInstallationKeyStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MacOsKeychainInstallationKeyStore(<redacted>)")
    }
}

impl InstallationKeyStore for MacOsKeychainInstallationKeyStore {
    fn load(&mut self) -> Result<Option<InstallationKey>, IdentityError> {
        MacOsKeychainInstallationKeyStore::load(self)
    }

    fn load_or_create(&mut self) -> Result<InstallationKey, IdentityError> {
        MacOsKeychainInstallationKeyStore::load_or_create(self)
    }
}

impl InstallationKeyRingStore for MacOsKeychainInstallationKeyStore {
    fn load_key_ring(&mut self) -> Result<Option<InstallationKeyRingRecord>, IdentityError> {
        MacOsKeychainInstallationKeyStore::load_key_ring(self)
    }

    fn load_or_create_key_ring(&mut self) -> Result<InstallationKeyRingRecord, IdentityError> {
        MacOsKeychainInstallationKeyStore::load_or_create_key_ring(self)
    }

    fn replace_key_ring(
        &mut self,
        expected: &InstallationKeyRingRevision,
        replacement: &InstallationKeyRingRecord,
    ) -> Result<InstallationKeyRingMutationOutcome, IdentityError> {
        MacOsKeychainInstallationKeyStore::replace_key_ring(self, expected, replacement)
    }

    fn delete_key_ring(
        &mut self,
        expected: &InstallationKeyRingRevision,
    ) -> Result<InstallationKeyRingMutationOutcome, IdentityError> {
        MacOsKeychainInstallationKeyStore::delete_key_ring(self, expected)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeychainCreateOutcome {
    Added,
    Duplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeychainMutationOutcome {
    Applied,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeychainBridgeError {
    Unavailable,
    MissingEntitlement,
    Denied,
    InteractionRequired,
    Cancelled,
    InvalidResult,
    Failed,
}

trait KeychainBridge: Send + Sync {
    fn load(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>, KeychainBridgeError>;

    fn create(
        &self,
        service: &str,
        account: &str,
        label: &str,
        value: &[u8],
    ) -> Result<KeychainCreateOutcome, KeychainBridgeError>;

    fn update(
        &self,
        service: &str,
        account: &str,
        value: &[u8],
    ) -> Result<KeychainMutationOutcome, KeychainBridgeError>;

    fn delete(
        &self,
        service: &str,
        account: &str,
    ) -> Result<KeychainMutationOutcome, KeychainBridgeError>;
}

fn map_bridge_error(error: KeychainBridgeError) -> IdentityError {
    match error {
        KeychainBridgeError::Unavailable => IdentityError::PlatformCredentialUnavailable,
        KeychainBridgeError::MissingEntitlement => {
            IdentityError::PlatformCredentialMissingEntitlement
        }
        KeychainBridgeError::Denied => IdentityError::PlatformCredentialDenied,
        KeychainBridgeError::InteractionRequired => {
            IdentityError::PlatformCredentialInteractionRequired
        }
        KeychainBridgeError::Cancelled => IdentityError::PlatformCredentialCancelled,
        KeychainBridgeError::InvalidResult => IdentityError::PlatformCredentialCorrupt,
        KeychainBridgeError::Failed => IdentityError::PlatformCredentialFailed,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeychainLocation {
    DataProtection,
    Login,
}

struct CompatibleKeychainBridge {
    protected: Arc<dyn KeychainBridge>,
    login: Arc<dyn KeychainBridge>,
    selected: Mutex<Option<KeychainLocation>>,
}

impl CompatibleKeychainBridge {
    fn new(protected: Arc<dyn KeychainBridge>, login: Arc<dyn KeychainBridge>) -> Self {
        Self {
            protected,
            login,
            selected: Mutex::new(None),
        }
    }

    fn bridge(&self, location: KeychainLocation) -> &dyn KeychainBridge {
        match location {
            KeychainLocation::DataProtection => self.protected.as_ref(),
            KeychainLocation::Login => self.login.as_ref(),
        }
    }

    fn selection(&self) -> Result<KeychainLocation, KeychainBridgeError> {
        self.selected
            .lock()
            .map_err(|_| KeychainBridgeError::Failed)?
            .ok_or(KeychainBridgeError::Failed)
    }
}

impl KeychainBridge for CompatibleKeychainBridge {
    fn load(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>, KeychainBridgeError> {
        let mut selected = self
            .selected
            .lock()
            .map_err(|_| KeychainBridgeError::Failed)?;
        if let Some(location) = *selected {
            return self.bridge(location).load(service, account);
        }
        let missing_entitlement = match self.protected.load(service, account) {
            Ok(Some(value)) => {
                *selected = Some(KeychainLocation::DataProtection);
                return Ok(Some(value));
            }
            Ok(None) => false,
            Err(KeychainBridgeError::MissingEntitlement) => true,
            Err(error) => return Err(error),
        };
        // A later signed build must reuse a local build's existing key rather
        // than silently generating a different account-history namespace.
        let existing = self.login.load(service, account)?;
        *selected = Some(if existing.is_some() || missing_entitlement {
            KeychainLocation::Login
        } else {
            KeychainLocation::DataProtection
        });
        Ok(existing)
    }

    fn create(
        &self,
        service: &str,
        account: &str,
        label: &str,
        value: &[u8],
    ) -> Result<KeychainCreateOutcome, KeychainBridgeError> {
        let location = self.selection()?;
        match self.bridge(location).create(service, account, label, value) {
            Err(KeychainBridgeError::MissingEntitlement)
                if location == KeychainLocation::DataProtection =>
            {
                // An unentitled macOS query can return "not found" and only
                // SecItemAdd reports -34018. Do not mistake that for bad auth.
                eprintln!(
                    "history_keychain: data_protection_missing_entitlement (-34018); selecting macOS login Keychain"
                );
                let existing = self.login.load(service, account)?;
                *self
                    .selected
                    .lock()
                    .map_err(|_| KeychainBridgeError::Failed)? = Some(KeychainLocation::Login);
                if existing.is_some() {
                    Ok(KeychainCreateOutcome::Duplicate)
                } else {
                    self.login.create(service, account, label, value)
                }
            }
            result => result,
        }
    }

    fn update(
        &self,
        service: &str,
        account: &str,
        value: &[u8],
    ) -> Result<KeychainMutationOutcome, KeychainBridgeError> {
        self.bridge(self.selection()?)
            .update(service, account, value)
    }

    fn delete(
        &self,
        service: &str,
        account: &str,
    ) -> Result<KeychainMutationOutcome, KeychainBridgeError> {
        self.bridge(self.selection()?).delete(service, account)
    }
}

struct SystemKeychainBridge(KeychainLocation);

// The legacy login Keychain ignores kSecUseAuthenticationUIFail. Its no-UI
// switch is process-scoped, so serialize our calls, restore the previous value,
// and never let background work wait behind a user authorization dialog.
static KEYCHAIN_INTERACTION: Mutex<()> = Mutex::new(());

struct KeychainInteractionGuard {
    _lease: MutexGuard<'static, ()>,
    previous: u8,
}

impl KeychainInteractionGuard {
    fn enter(allowed: bool) -> Result<Self, KeychainBridgeError> {
        let lease = KEYCHAIN_INTERACTION
            .try_lock()
            .map_err(|_| KeychainBridgeError::InteractionRequired)?;
        let mut previous = 0;
        let status = unsafe { sec_keychain_get_user_interaction_allowed(&mut previous) };
        if status != ERR_SEC_SUCCESS {
            return Err(map_os_status(status));
        }
        let status = unsafe { sec_keychain_set_user_interaction_allowed(u8::from(allowed)) };
        if status != ERR_SEC_SUCCESS {
            return Err(map_os_status(status));
        }
        Ok(Self {
            _lease: lease,
            previous,
        })
    }

    fn for_location(location: KeychainLocation) -> Result<Option<Self>, KeychainBridgeError> {
        if location == KeychainLocation::Login {
            Self::enter(false).map(Some)
        } else {
            Ok(None)
        }
    }
}

impl Drop for KeychainInteractionGuard {
    fn drop(&mut self) {
        unsafe {
            sec_keychain_set_user_interaction_allowed(self.previous);
        }
    }
}

impl KeychainBridge for SystemKeychainBridge {
    fn load(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>, KeychainBridgeError> {
        let _interaction = KeychainInteractionGuard::for_location(self.0)?;
        keychain_copy_matching(service, account, self.0)
    }

    fn create(
        &self,
        service: &str,
        account: &str,
        label: &str,
        value: &[u8],
    ) -> Result<KeychainCreateOutcome, KeychainBridgeError> {
        let _interaction = KeychainInteractionGuard::for_location(self.0)?;
        keychain_add(service, account, label, value, self.0)
    }

    fn update(
        &self,
        service: &str,
        account: &str,
        value: &[u8],
    ) -> Result<KeychainMutationOutcome, KeychainBridgeError> {
        let _interaction = KeychainInteractionGuard::for_location(self.0)?;
        keychain_update(service, account, value, self.0)
    }

    fn delete(
        &self,
        service: &str,
        account: &str,
    ) -> Result<KeychainMutationOutcome, KeychainBridgeError> {
        let _interaction = KeychainInteractionGuard::for_location(self.0)?;
        keychain_delete(service, account, self.0)
    }
}

fn keychain_copy_matching(
    service: &str,
    account: &str,
    location: KeychainLocation,
) -> Result<Option<Vec<u8>>, KeychainBridgeError> {
    let query = build_copy_query_for_location(service, account, location)?;
    let mut result: CfTypeRef = null();
    let status = unsafe { sec_item_copy_matching(query.as_ptr(), &mut result) };
    match status {
        ERR_SEC_SUCCESS => {}
        ERR_SEC_ITEM_NOT_FOUND => return Ok(None),
        status => return Err(map_os_status(status)),
    }
    let result = CfOwned::from_retained(result).ok_or(KeychainBridgeError::InvalidResult)?;
    if unsafe { cf_get_type_id(result.as_ptr()) } != unsafe { cf_data_get_type_id() } {
        return Err(KeychainBridgeError::InvalidResult);
    }
    let length = unsafe { cf_data_get_length(result.as_ptr()) };
    if length <= 0 || length as u64 > MAX_KEY_RING_RECORD_BYTES {
        return Err(KeychainBridgeError::InvalidResult);
    }
    let bytes = unsafe { cf_data_get_byte_ptr(result.as_ptr()) };
    if bytes.is_null() {
        return Err(KeychainBridgeError::InvalidResult);
    }
    let value = unsafe { std::slice::from_raw_parts(bytes, length as usize) }.to_vec();
    Ok(Some(value))
}

#[cfg(test)]
fn build_copy_query(service: &str, account: &str) -> Result<CfOwned, KeychainBridgeError> {
    build_copy_query_for_location(service, account, KeychainLocation::DataProtection)
}

fn build_copy_query_for_location(
    service: &str,
    account: &str,
    location: KeychainLocation,
) -> Result<CfOwned, KeychainBridgeError> {
    let service = CfOwned::string(service)?;
    let account = CfOwned::string(account)?;
    if location == KeychainLocation::Login {
        return unsafe {
            CfOwned::dictionary(
                &[
                    K_SEC_CLASS,
                    K_SEC_ATTR_SERVICE,
                    K_SEC_ATTR_ACCOUNT,
                    K_SEC_RETURN_DATA,
                    K_SEC_MATCH_LIMIT,
                ],
                &[
                    K_SEC_CLASS_GENERIC_PASSWORD,
                    service.as_ptr(),
                    account.as_ptr(),
                    K_CF_BOOLEAN_TRUE,
                    K_SEC_MATCH_LIMIT_ONE,
                ],
            )
        };
    }
    let keys = unsafe {
        [
            K_SEC_CLASS,
            K_SEC_ATTR_SERVICE,
            K_SEC_ATTR_ACCOUNT,
            K_SEC_ATTR_SYNCHRONIZABLE,
            K_SEC_USE_DATA_PROTECTION_KEYCHAIN,
            K_SEC_RETURN_DATA,
            K_SEC_MATCH_LIMIT,
            K_SEC_USE_AUTHENTICATION_UI,
        ]
    };
    let values = unsafe {
        [
            K_SEC_CLASS_GENERIC_PASSWORD,
            service.as_ptr(),
            account.as_ptr(),
            K_CF_BOOLEAN_FALSE,
            K_CF_BOOLEAN_TRUE,
            K_CF_BOOLEAN_TRUE,
            K_SEC_MATCH_LIMIT_ONE,
            K_SEC_USE_AUTHENTICATION_UI_FAIL,
        ]
    };
    CfOwned::dictionary(&keys, &values)
}

#[cfg(test)]
fn build_mutation_query(service: &str, account: &str) -> Result<CfOwned, KeychainBridgeError> {
    build_mutation_query_for_location(service, account, KeychainLocation::DataProtection)
}

fn build_mutation_query_for_location(
    service: &str,
    account: &str,
    location: KeychainLocation,
) -> Result<CfOwned, KeychainBridgeError> {
    let service = CfOwned::string(service)?;
    let account = CfOwned::string(account)?;
    if location == KeychainLocation::Login {
        return unsafe {
            CfOwned::dictionary(
                &[
                    K_SEC_CLASS,
                    K_SEC_ATTR_SERVICE,
                    K_SEC_ATTR_ACCOUNT,
                    K_SEC_USE_AUTHENTICATION_UI,
                ],
                &[
                    K_SEC_CLASS_GENERIC_PASSWORD,
                    service.as_ptr(),
                    account.as_ptr(),
                    K_SEC_USE_AUTHENTICATION_UI_FAIL,
                ],
            )
        };
    }
    let keys = unsafe {
        [
            K_SEC_CLASS,
            K_SEC_ATTR_SERVICE,
            K_SEC_ATTR_ACCOUNT,
            K_SEC_ATTR_SYNCHRONIZABLE,
            K_SEC_USE_DATA_PROTECTION_KEYCHAIN,
            K_SEC_USE_AUTHENTICATION_UI,
        ]
    };
    let values = unsafe {
        [
            K_SEC_CLASS_GENERIC_PASSWORD,
            service.as_ptr(),
            account.as_ptr(),
            K_CF_BOOLEAN_FALSE,
            K_CF_BOOLEAN_TRUE,
            K_SEC_USE_AUTHENTICATION_UI_FAIL,
        ]
    };
    CfOwned::dictionary(&keys, &values)
}

fn keychain_add(
    service: &str,
    account: &str,
    label: &str,
    value: &[u8],
    location: KeychainLocation,
) -> Result<KeychainCreateOutcome, KeychainBridgeError> {
    let attributes = build_add_attributes_for_location(service, account, label, value, location)?;
    match unsafe { sec_item_add(attributes.as_ptr(), null_mut()) } {
        ERR_SEC_SUCCESS => Ok(KeychainCreateOutcome::Added),
        ERR_SEC_DUPLICATE_ITEM => Ok(KeychainCreateOutcome::Duplicate),
        status => Err(map_os_status(status)),
    }
}

#[cfg(test)]
fn build_add_attributes(
    service: &str,
    account: &str,
    label: &str,
    value: &[u8],
) -> Result<CfOwned, KeychainBridgeError> {
    build_add_attributes_for_location(
        service,
        account,
        label,
        value,
        KeychainLocation::DataProtection,
    )
}

fn build_add_attributes_for_location(
    service: &str,
    account: &str,
    label: &str,
    value: &[u8],
    location: KeychainLocation,
) -> Result<CfOwned, KeychainBridgeError> {
    validate_key_record(value)?;
    let service = CfOwned::string(service)?;
    let account = CfOwned::string(account)?;
    let label = CfOwned::string(label)?;
    let value = CfOwned::data(value)?;
    if location == KeychainLocation::Login {
        // Leave the normal application-scoped Keychain ACL intact. No broad
        // trusted-app list, synchronization, or file fallback is introduced.
        return unsafe {
            CfOwned::dictionary(
                &[
                    K_SEC_CLASS,
                    K_SEC_ATTR_SERVICE,
                    K_SEC_ATTR_ACCOUNT,
                    K_SEC_ATTR_LABEL,
                    K_SEC_VALUE_DATA,
                ],
                &[
                    K_SEC_CLASS_GENERIC_PASSWORD,
                    service.as_ptr(),
                    account.as_ptr(),
                    label.as_ptr(),
                    value.as_ptr(),
                ],
            )
        };
    }
    let keys = unsafe {
        [
            K_SEC_CLASS,
            K_SEC_ATTR_SERVICE,
            K_SEC_ATTR_ACCOUNT,
            K_SEC_ATTR_LABEL,
            K_SEC_ATTR_SYNCHRONIZABLE,
            K_SEC_USE_DATA_PROTECTION_KEYCHAIN,
            K_SEC_ATTR_ACCESSIBLE,
            K_SEC_VALUE_DATA,
        ]
    };
    let values = unsafe {
        [
            K_SEC_CLASS_GENERIC_PASSWORD,
            service.as_ptr(),
            account.as_ptr(),
            label.as_ptr(),
            K_CF_BOOLEAN_FALSE,
            K_CF_BOOLEAN_TRUE,
            K_SEC_ATTR_ACCESSIBLE_AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY,
            value.as_ptr(),
        ]
    };
    CfOwned::dictionary(&keys, &values)
}

fn keychain_update(
    service: &str,
    account: &str,
    value: &[u8],
    location: KeychainLocation,
) -> Result<KeychainMutationOutcome, KeychainBridgeError> {
    let query = build_mutation_query_for_location(service, account, location)?;
    let attributes = build_update_attributes(value)?;
    match unsafe { sec_item_update(query.as_ptr(), attributes.as_ptr()) } {
        ERR_SEC_SUCCESS => Ok(KeychainMutationOutcome::Applied),
        ERR_SEC_ITEM_NOT_FOUND => Ok(KeychainMutationOutcome::Missing),
        status => Err(map_os_status(status)),
    }
}

fn build_update_attributes(value: &[u8]) -> Result<CfOwned, KeychainBridgeError> {
    validate_key_record(value)?;
    let value = CfOwned::data(value)?;
    let keys = unsafe { [K_SEC_VALUE_DATA] };
    let values = [value.as_ptr()];
    CfOwned::dictionary(&keys, &values)
}

fn keychain_delete(
    service: &str,
    account: &str,
    location: KeychainLocation,
) -> Result<KeychainMutationOutcome, KeychainBridgeError> {
    let query = build_mutation_query_for_location(service, account, location)?;
    match unsafe { sec_item_delete(query.as_ptr()) } {
        ERR_SEC_SUCCESS => Ok(KeychainMutationOutcome::Applied),
        ERR_SEC_ITEM_NOT_FOUND => Ok(KeychainMutationOutcome::Missing),
        status => Err(map_os_status(status)),
    }
}

fn validate_key_record(value: &[u8]) -> Result<(), KeychainBridgeError> {
    parse_key_ring_record(value.to_vec())
        .map(drop)
        .map_err(|_| KeychainBridgeError::InvalidResult)
}

fn map_os_status(status: i32) -> KeychainBridgeError {
    match status {
        ERR_SEC_NOT_AVAILABLE => KeychainBridgeError::Unavailable,
        ERR_SEC_MISSING_ENTITLEMENT => KeychainBridgeError::MissingEntitlement,
        ERR_SEC_AUTH_FAILED => KeychainBridgeError::Denied,
        ERR_SEC_INTERACTION_NOT_ALLOWED => KeychainBridgeError::InteractionRequired,
        ERR_SEC_USER_CANCELED => KeychainBridgeError::Cancelled,
        _ => KeychainBridgeError::Failed,
    }
}

type CfTypeRef = *const c_void;
type CfIndex = isize;
type CfTypeId = usize;
type CfHashCode = usize;

#[repr(C)]
struct CfDictionaryKeyCallbacks {
    version: CfIndex,
    retain: Option<unsafe extern "C" fn(CfTypeRef, CfTypeRef) -> CfTypeRef>,
    release: Option<unsafe extern "C" fn(CfTypeRef, CfTypeRef)>,
    copy_description: Option<unsafe extern "C" fn(CfTypeRef) -> CfTypeRef>,
    equal: Option<unsafe extern "C" fn(CfTypeRef, CfTypeRef) -> u8>,
    hash: Option<unsafe extern "C" fn(CfTypeRef) -> CfHashCode>,
}

#[repr(C)]
struct CfDictionaryValueCallbacks {
    version: CfIndex,
    retain: Option<unsafe extern "C" fn(CfTypeRef, CfTypeRef) -> CfTypeRef>,
    release: Option<unsafe extern "C" fn(CfTypeRef, CfTypeRef)>,
    copy_description: Option<unsafe extern "C" fn(CfTypeRef) -> CfTypeRef>,
    equal: Option<unsafe extern "C" fn(CfTypeRef, CfTypeRef) -> u8>,
}

struct CfOwned(CfTypeRef);

impl CfOwned {
    fn string(value: &str) -> Result<Self, KeychainBridgeError> {
        let reference = unsafe {
            cf_string_create_with_bytes(
                null(),
                value.as_ptr(),
                value.len() as CfIndex,
                CF_STRING_ENCODING_UTF8,
                0,
            )
        };
        Self::from_retained(reference).ok_or(KeychainBridgeError::Failed)
    }

    fn data(value: &[u8]) -> Result<Self, KeychainBridgeError> {
        let reference = unsafe { cf_data_create(null(), value.as_ptr(), value.len() as CfIndex) };
        Self::from_retained(reference).ok_or(KeychainBridgeError::Failed)
    }

    fn dictionary(keys: &[CfTypeRef], values: &[CfTypeRef]) -> Result<Self, KeychainBridgeError> {
        if keys.len() != values.len() || keys.is_empty() {
            return Err(KeychainBridgeError::Failed);
        }
        let reference = unsafe {
            cf_dictionary_create(
                null(),
                keys.as_ptr(),
                values.as_ptr(),
                keys.len() as CfIndex,
                &K_CF_TYPE_DICTIONARY_KEY_CALLBACKS,
                &K_CF_TYPE_DICTIONARY_VALUE_CALLBACKS,
            )
        };
        Self::from_retained(reference).ok_or(KeychainBridgeError::Failed)
    }

    fn from_retained(reference: CfTypeRef) -> Option<Self> {
        (!reference.is_null()).then_some(Self(reference))
    }

    fn as_ptr(&self) -> CfTypeRef {
        self.0
    }
}

impl Drop for CfOwned {
    fn drop(&mut self) {
        unsafe { cf_release(self.0) };
    }
}

const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    #[link_name = "SecKeychainGetUserInteractionAllowed"]
    fn sec_keychain_get_user_interaction_allowed(state: *mut u8) -> i32;
    #[link_name = "SecKeychainSetUserInteractionAllowed"]
    fn sec_keychain_set_user_interaction_allowed(state: u8) -> i32;
    #[link_name = "SecItemCopyMatching"]
    fn sec_item_copy_matching(query: CfTypeRef, result: *mut CfTypeRef) -> i32;
    #[link_name = "SecItemAdd"]
    fn sec_item_add(attributes: CfTypeRef, result: *mut CfTypeRef) -> i32;
    #[link_name = "SecItemUpdate"]
    fn sec_item_update(query: CfTypeRef, attributes_to_update: CfTypeRef) -> i32;
    #[link_name = "SecItemDelete"]
    fn sec_item_delete(query: CfTypeRef) -> i32;

    #[link_name = "kSecClass"]
    static K_SEC_CLASS: CfTypeRef;
    #[link_name = "kSecClassGenericPassword"]
    static K_SEC_CLASS_GENERIC_PASSWORD: CfTypeRef;
    #[link_name = "kSecAttrService"]
    static K_SEC_ATTR_SERVICE: CfTypeRef;
    #[link_name = "kSecAttrAccount"]
    static K_SEC_ATTR_ACCOUNT: CfTypeRef;
    #[link_name = "kSecAttrLabel"]
    static K_SEC_ATTR_LABEL: CfTypeRef;
    #[link_name = "kSecAttrSynchronizable"]
    static K_SEC_ATTR_SYNCHRONIZABLE: CfTypeRef;
    #[link_name = "kSecAttrAccessible"]
    static K_SEC_ATTR_ACCESSIBLE: CfTypeRef;
    #[link_name = "kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly"]
    static K_SEC_ATTR_ACCESSIBLE_AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY: CfTypeRef;
    #[link_name = "kSecUseDataProtectionKeychain"]
    static K_SEC_USE_DATA_PROTECTION_KEYCHAIN: CfTypeRef;
    #[link_name = "kSecUseAuthenticationUI"]
    static K_SEC_USE_AUTHENTICATION_UI: CfTypeRef;
    #[link_name = "kSecUseAuthenticationUIFail"]
    static K_SEC_USE_AUTHENTICATION_UI_FAIL: CfTypeRef;
    #[link_name = "kSecReturnData"]
    static K_SEC_RETURN_DATA: CfTypeRef;
    #[link_name = "kSecMatchLimit"]
    static K_SEC_MATCH_LIMIT: CfTypeRef;
    #[link_name = "kSecMatchLimitOne"]
    static K_SEC_MATCH_LIMIT_ONE: CfTypeRef;
    #[link_name = "kSecValueData"]
    static K_SEC_VALUE_DATA: CfTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    #[link_name = "CFStringCreateWithBytes"]
    fn cf_string_create_with_bytes(
        allocator: CfTypeRef,
        bytes: *const u8,
        byte_count: CfIndex,
        encoding: u32,
        external_representation: u8,
    ) -> CfTypeRef;
    #[link_name = "CFDataCreate"]
    fn cf_data_create(allocator: CfTypeRef, bytes: *const u8, byte_count: CfIndex) -> CfTypeRef;
    #[link_name = "CFDictionaryCreate"]
    fn cf_dictionary_create(
        allocator: CfTypeRef,
        keys: *const CfTypeRef,
        values: *const CfTypeRef,
        count: CfIndex,
        key_callbacks: *const CfDictionaryKeyCallbacks,
        value_callbacks: *const CfDictionaryValueCallbacks,
    ) -> CfTypeRef;
    #[cfg(test)]
    #[link_name = "CFDictionaryGetCount"]
    fn cf_dictionary_get_count(dictionary: CfTypeRef) -> CfIndex;
    #[cfg(test)]
    #[link_name = "CFDictionaryGetValue"]
    fn cf_dictionary_get_value(dictionary: CfTypeRef, key: CfTypeRef) -> CfTypeRef;
    #[cfg(test)]
    #[link_name = "CFEqual"]
    fn cf_equal(left: CfTypeRef, right: CfTypeRef) -> u8;
    #[link_name = "CFGetTypeID"]
    fn cf_get_type_id(value: CfTypeRef) -> CfTypeId;
    #[link_name = "CFDataGetTypeID"]
    fn cf_data_get_type_id() -> CfTypeId;
    #[link_name = "CFDataGetLength"]
    fn cf_data_get_length(data: CfTypeRef) -> CfIndex;
    #[link_name = "CFDataGetBytePtr"]
    fn cf_data_get_byte_ptr(data: CfTypeRef) -> *const u8;
    #[link_name = "CFRelease"]
    fn cf_release(value: CfTypeRef);

    #[link_name = "kCFBooleanTrue"]
    static K_CF_BOOLEAN_TRUE: CfTypeRef;
    #[link_name = "kCFBooleanFalse"]
    static K_CF_BOOLEAN_FALSE: CfTypeRef;
    #[link_name = "kCFTypeDictionaryKeyCallBacks"]
    static K_CF_TYPE_DICTIONARY_KEY_CALLBACKS: CfDictionaryKeyCallbacks;
    #[link_name = "kCFTypeDictionaryValueCallBacks"]
    static K_CF_TYPE_DICTIONARY_VALUE_CALLBACKS: CfDictionaryValueCallbacks;
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::super::{INSTALLATION_SECRET_BYTES, InstallationKeyBackendRequest};
    use super::*;

    const KEY_ID: &str = "018f47a2-8a71-4f4a-9c35-1f4234a73701";
    const KEY_ID_2: &str = "018f47a2-8a71-4f4a-9c35-1f4234a73702";

    #[test]
    #[ignore = "read-only, noninteractive check of this machine's installation-key namespace"]
    fn native_keychain_read_only_diagnostic() {
        match MacOsKeychainInstallationKeyStore::new().load() {
            Ok(Some(_)) => println!("data_protection_keychain: existing installation key"),
            Ok(None) => println!("data_protection_keychain: no installation key"),
            Err(error) => println!("data_protection_keychain: {error}"),
        }
        match MacOsKeychainInstallationKeyStore::compatible_with_local_builds().load() {
            Ok(Some(_)) => println!("compatible_keychain: existing installation key (no prompt)"),
            Ok(None) => println!("compatible_keychain: no installation key (no prompt)"),
            Err(error) => println!("compatible_keychain: {error} (no prompt)"),
        }
    }

    #[test]
    fn login_keychain_background_calls_disable_ui_and_restore_the_previous_state() {
        let mut original = 0;
        assert_eq!(
            unsafe { sec_keychain_get_user_interaction_allowed(&mut original) },
            ERR_SEC_SUCCESS
        );
        {
            let _guard = KeychainInteractionGuard::enter(false).unwrap();
            let mut allowed = 1;
            assert_eq!(
                unsafe { sec_keychain_get_user_interaction_allowed(&mut allowed) },
                ERR_SEC_SUCCESS
            );
            assert_eq!(allowed, 0);
            assert!(matches!(
                KeychainInteractionGuard::enter(false),
                Err(KeychainBridgeError::InteractionRequired)
            ));
        }
        let mut restored = 0;
        assert_eq!(
            unsafe { sec_keychain_get_user_interaction_allowed(&mut restored) },
            ERR_SEC_SUCCESS
        );
        assert_eq!(restored, original);
    }

    struct FakeState {
        value: Option<Vec<u8>>,
        load_error: Option<KeychainBridgeError>,
        create_error: Option<KeychainBridgeError>,
        update_error: Option<KeychainBridgeError>,
        delete_error: Option<KeychainBridgeError>,
        force_duplicate: bool,
        force_update_missing: bool,
        force_delete_missing: bool,
        duplicate_value: Option<Vec<u8>>,
        load_mutation: Option<(usize, FakeLoadMutation)>,
        post_update_value: Option<Vec<u8>>,
        post_delete_value: Option<Vec<u8>>,
        load_calls: usize,
        create_calls: usize,
        update_calls: usize,
        delete_calls: usize,
        selectors_valid: bool,
    }

    enum FakeLoadMutation {
        Replace(Vec<u8>),
        Remove,
    }

    impl Default for FakeState {
        fn default() -> Self {
            Self {
                value: None,
                load_error: None,
                create_error: None,
                update_error: None,
                delete_error: None,
                force_duplicate: false,
                force_update_missing: false,
                force_delete_missing: false,
                duplicate_value: None,
                load_mutation: None,
                post_update_value: None,
                post_delete_value: None,
                load_calls: 0,
                create_calls: 0,
                update_calls: 0,
                delete_calls: 0,
                selectors_valid: true,
            }
        }
    }

    #[derive(Default)]
    struct FakeBridge {
        state: Mutex<FakeState>,
    }

    impl FakeBridge {
        fn with_value(value: Vec<u8>) -> Self {
            Self {
                state: Mutex::new(FakeState {
                    value: Some(value),
                    ..FakeState::default()
                }),
            }
        }
    }

    impl KeychainBridge for FakeBridge {
        fn load(
            &self,
            service: &str,
            account: &str,
        ) -> Result<Option<Vec<u8>>, KeychainBridgeError> {
            let mut state = self.state.lock().unwrap();
            state.load_calls += 1;
            state.selectors_valid &=
                service == MACOS_KEYCHAIN_SERVICE && account == MACOS_KEYCHAIN_ACCOUNT;
            if let Some(error) = state.load_error {
                return Err(error);
            }
            let mutate_now = state
                .load_mutation
                .as_ref()
                .is_some_and(|(call, _)| *call == state.load_calls);
            if mutate_now {
                let (_, mutation) = state.load_mutation.take().expect("checked fake mutation");
                state.value = match mutation {
                    FakeLoadMutation::Replace(value) => Some(value),
                    FakeLoadMutation::Remove => None,
                };
            }
            Ok(state.value.clone())
        }

        fn create(
            &self,
            service: &str,
            account: &str,
            label: &str,
            value: &[u8],
        ) -> Result<KeychainCreateOutcome, KeychainBridgeError> {
            let mut state = self.state.lock().unwrap();
            state.create_calls += 1;
            state.selectors_valid &= service == MACOS_KEYCHAIN_SERVICE
                && account == MACOS_KEYCHAIN_ACCOUNT
                && label == MACOS_KEYCHAIN_LABEL;
            if let Some(error) = state.create_error {
                return Err(error);
            }
            if state.force_duplicate || state.value.is_some() {
                if state.value.is_none() {
                    state.value = state.duplicate_value.take();
                }
                return Ok(KeychainCreateOutcome::Duplicate);
            }
            state.value = Some(value.to_vec());
            Ok(KeychainCreateOutcome::Added)
        }

        fn update(
            &self,
            service: &str,
            account: &str,
            value: &[u8],
        ) -> Result<KeychainMutationOutcome, KeychainBridgeError> {
            let mut state = self.state.lock().unwrap();
            state.update_calls += 1;
            state.selectors_valid &=
                service == MACOS_KEYCHAIN_SERVICE && account == MACOS_KEYCHAIN_ACCOUNT;
            if let Some(error) = state.update_error {
                return Err(error);
            }
            if state.force_update_missing || state.value.is_none() {
                return Ok(KeychainMutationOutcome::Missing);
            }
            state.value = Some(value.to_vec());
            if let Some(post_update_value) = state.post_update_value.take() {
                state.value = Some(post_update_value);
            }
            Ok(KeychainMutationOutcome::Applied)
        }

        fn delete(
            &self,
            service: &str,
            account: &str,
        ) -> Result<KeychainMutationOutcome, KeychainBridgeError> {
            let mut state = self.state.lock().unwrap();
            state.delete_calls += 1;
            state.selectors_valid &=
                service == MACOS_KEYCHAIN_SERVICE && account == MACOS_KEYCHAIN_ACCOUNT;
            if let Some(error) = state.delete_error {
                return Err(error);
            }
            if state.force_delete_missing || state.value.is_none() {
                return Ok(KeychainMutationOutcome::Missing);
            }
            state.value = state.post_delete_value.take();
            Ok(KeychainMutationOutcome::Applied)
        }
    }

    fn key(id: &str, byte: u8) -> InstallationKey {
        InstallationKey::from_parts(id, [byte; INSTALLATION_SECRET_BYTES]).unwrap()
    }

    fn encoded_key(id: &str, byte: u8) -> Vec<u8> {
        super::super::encode_installation_key(&key(id, byte))
    }

    fn encoded_ring(record: &InstallationKeyRingRecord) -> Vec<u8> {
        encode_key_ring_record(record)
    }

    #[test]
    fn existing_key_loads_without_create_and_uses_fixed_selectors() {
        let bridge = Arc::new(FakeBridge::with_value(encoded_key(KEY_ID, 0x11)));
        let store = MacOsKeychainInstallationKeyStore::with_bridge(bridge.clone());
        let loaded = store.load_or_create().unwrap();
        assert_eq!(loaded.key_id(), KEY_ID);
        let state = bridge.state.lock().unwrap();
        assert_eq!(state.load_calls, 1);
        assert_eq!(state.create_calls, 0);
        assert!(state.selectors_valid);
    }

    #[test]
    fn missing_key_is_created_then_read_back_exactly() {
        let bridge = Arc::new(FakeBridge::default());
        let store = MacOsKeychainInstallationKeyStore::with_bridge(bridge.clone());
        let created = store.load_or_create().unwrap();
        let reloaded = store.load_key_ring().unwrap().unwrap();
        assert_eq!(
            reloaded.storage_version(),
            super::super::InstallationKeyStorageVersion::KeyRingV2
        );
        assert_eq!(reloaded.revision(), 1);
        assert_eq!(created.key_id(), reloaded.active_key_id());
        let state = bridge.state.lock().unwrap();
        assert_eq!(state.create_calls, 1);
        assert_eq!(state.load_calls, 3);
        assert!(state.selectors_valid);
    }

    #[test]
    fn duplicate_create_race_loads_the_winning_key() {
        let bridge = Arc::new(FakeBridge::default());
        {
            let mut state = bridge.state.lock().unwrap();
            state.force_duplicate = true;
            state.duplicate_value = Some(encoded_key(KEY_ID_2, 0x22));
        }
        let store = MacOsKeychainInstallationKeyStore::with_bridge(bridge.clone());
        let loaded = store.load_or_create().unwrap();
        assert_eq!(loaded.key_id(), KEY_ID_2);
        let state = bridge.state.lock().unwrap();
        assert_eq!(state.create_calls, 1);
        assert_eq!(state.load_calls, 2);
    }

    #[test]
    fn legacy_key_migrates_then_rotate_promote_retire_and_delete_use_cas() {
        let bridge = Arc::new(FakeBridge::with_value(encoded_key(KEY_ID, 0x31)));
        let store = MacOsKeychainInstallationKeyStore::with_bridge(bridge.clone());
        let legacy = store.load_key_ring().unwrap().unwrap();
        assert_eq!(
            legacy.storage_version(),
            super::super::InstallationKeyStorageVersion::SingleKeyV1
        );
        let legacy_revision = legacy.revision_token();
        let rotating = legacy.start_rotation(key(KEY_ID_2, 0x32)).unwrap();

        let foreign_successor = InstallationKeyRingRecord::initial(key(KEY_ID, 0x39))
            .start_rotation(key(KEY_ID_2, 0x3a))
            .unwrap();
        assert!(matches!(
            store.replace_key_ring(&legacy_revision, &foreign_successor),
            Err(IdentityError::InvalidKeyRing)
        ));
        assert_eq!(bridge.state.lock().unwrap().update_calls, 0);

        assert_eq!(
            store.replace_key_ring(&legacy_revision, &rotating).unwrap(),
            InstallationKeyRingMutationOutcome::Applied
        );
        assert_eq!(
            store.replace_key_ring(&legacy_revision, &rotating).unwrap(),
            InstallationKeyRingMutationOutcome::RevisionConflict
        );

        let installed = store.load_key_ring().unwrap().unwrap();
        assert_eq!(installed.revision(), 2);
        assert_eq!(installed.active_key_id(), KEY_ID_2);
        assert_eq!(
            installed.verification_key_ids().collect::<Vec<_>>(),
            [KEY_ID]
        );
        let rotating_revision = installed.revision_token();
        let rolled_back = installed.promote_verification_key(KEY_ID).unwrap();
        assert_eq!(
            store
                .replace_key_ring(&rotating_revision, &rolled_back)
                .unwrap(),
            InstallationKeyRingMutationOutcome::Applied
        );

        let rollback = store.load_key_ring().unwrap().unwrap();
        assert_eq!(rollback.revision(), 3);
        assert_eq!(rollback.active_key_id(), KEY_ID);
        assert_eq!(
            rollback.verification_key_ids().collect::<Vec<_>>(),
            [KEY_ID_2]
        );
        let rollback_revision = rollback.revision_token();
        let retired = rollback.retire_verification_key(KEY_ID_2).unwrap();
        assert_eq!(
            store
                .replace_key_ring(&rollback_revision, &retired)
                .unwrap(),
            InstallationKeyRingMutationOutcome::Applied
        );
        assert_eq!(
            store.delete_key_ring(&rotating_revision).unwrap(),
            InstallationKeyRingMutationOutcome::RevisionConflict
        );
        let final_revision = store.load_key_ring().unwrap().unwrap().revision_token();
        assert_eq!(
            store.delete_key_ring(&final_revision).unwrap(),
            InstallationKeyRingMutationOutcome::Applied
        );
        assert!(store.load_key_ring().unwrap().is_none());

        let state = bridge.state.lock().unwrap();
        assert_eq!(state.update_calls, 3);
        assert_eq!(state.delete_calls, 1);
        assert!(state.selectors_valid);
    }

    #[test]
    fn same_token_material_drift_is_a_conflict_before_keychain_update() {
        let initial = InstallationKeyRingRecord::initial(key(KEY_ID, 0x41));
        let bridge = Arc::new(FakeBridge::with_value(encoded_ring(&initial)));
        bridge.state.lock().unwrap().load_mutation = Some((
            2,
            FakeLoadMutation::Replace(encoded_ring(&InstallationKeyRingRecord::initial(key(
                KEY_ID, 0x42,
            )))),
        ));
        let store = MacOsKeychainInstallationKeyStore::with_bridge(bridge.clone());
        let expected = initial.revision_token();
        let replacement = initial.start_rotation(key(KEY_ID_2, 0x43)).unwrap();

        assert_eq!(
            store.replace_key_ring(&expected, &replacement).unwrap(),
            InstallationKeyRingMutationOutcome::RevisionConflict
        );
        let state = bridge.state.lock().unwrap();
        assert_eq!(state.update_calls, 0);
        assert!(state.selectors_valid);
    }

    #[test]
    fn update_and_delete_missing_or_postflight_race_never_report_success() {
        let initial = InstallationKeyRingRecord::initial(key(KEY_ID, 0x51));
        let expected = initial.revision_token();
        let replacement = initial.start_rotation(key(KEY_ID_2, 0x52)).unwrap();
        let missing_bridge = Arc::new(FakeBridge::with_value(encoded_ring(
            &InstallationKeyRingRecord::initial(key(KEY_ID, 0x51)),
        )));
        missing_bridge.state.lock().unwrap().force_update_missing = true;
        let missing_store = MacOsKeychainInstallationKeyStore::with_bridge(missing_bridge.clone());
        assert_eq!(
            missing_store
                .replace_key_ring(&expected, &replacement)
                .unwrap(),
            InstallationKeyRingMutationOutcome::Missing
        );

        let post_update_bridge = Arc::new(FakeBridge::with_value(encoded_ring(
            &InstallationKeyRingRecord::initial(key(KEY_ID, 0x51)),
        )));
        post_update_bridge.state.lock().unwrap().post_update_value = Some(encoded_ring(
            &InstallationKeyRingRecord::initial(key(KEY_ID, 0x51))
                .start_rotation(key(KEY_ID_2, 0x53))
                .unwrap(),
        ));
        let post_update_store = MacOsKeychainInstallationKeyStore::with_bridge(post_update_bridge);
        assert_eq!(
            post_update_store
                .replace_key_ring(&expected, &replacement)
                .unwrap(),
            InstallationKeyRingMutationOutcome::RevisionConflict
        );

        let delete_bridge = Arc::new(FakeBridge::with_value(encoded_ring(
            &InstallationKeyRingRecord::initial(key(KEY_ID, 0x51)),
        )));
        delete_bridge.state.lock().unwrap().force_delete_missing = true;
        let delete_store = MacOsKeychainInstallationKeyStore::with_bridge(delete_bridge);
        assert_eq!(
            delete_store.delete_key_ring(&expected).unwrap(),
            InstallationKeyRingMutationOutcome::Missing
        );

        let reappearing_bridge = Arc::new(FakeBridge::with_value(encoded_ring(
            &InstallationKeyRingRecord::initial(key(KEY_ID, 0x51)),
        )));
        reappearing_bridge.state.lock().unwrap().post_delete_value = Some(encoded_ring(
            &InstallationKeyRingRecord::initial(key(KEY_ID, 0x54)),
        ));
        let reappearing_store = MacOsKeychainInstallationKeyStore::with_bridge(reappearing_bridge);
        assert_eq!(
            reappearing_store.delete_key_ring(&expected).unwrap(),
            InstallationKeyRingMutationOutcome::RevisionConflict
        );
    }

    #[test]
    fn missing_during_second_preflight_never_reaches_mutation() {
        let initial = InstallationKeyRingRecord::initial(key(KEY_ID, 0x61));
        let bridge = Arc::new(FakeBridge::with_value(encoded_ring(&initial)));
        bridge.state.lock().unwrap().load_mutation = Some((2, FakeLoadMutation::Remove));
        let store = MacOsKeychainInstallationKeyStore::with_bridge(bridge.clone());
        let expected = initial.revision_token();
        let replacement = initial.start_rotation(key(KEY_ID_2, 0x62)).unwrap();

        assert_eq!(
            store.replace_key_ring(&expected, &replacement).unwrap(),
            InstallationKeyRingMutationOutcome::Missing
        );
        assert_eq!(bridge.state.lock().unwrap().update_calls, 0);
    }

    #[test]
    fn platform_failures_map_without_status_or_payload_echo() {
        for (bridge_error, expected) in [
            (
                KeychainBridgeError::MissingEntitlement,
                IdentityError::PlatformCredentialMissingEntitlement,
            ),
            (
                KeychainBridgeError::Unavailable,
                IdentityError::PlatformCredentialUnavailable,
            ),
            (
                KeychainBridgeError::Denied,
                IdentityError::PlatformCredentialDenied,
            ),
            (
                KeychainBridgeError::InteractionRequired,
                IdentityError::PlatformCredentialInteractionRequired,
            ),
            (
                KeychainBridgeError::Cancelled,
                IdentityError::PlatformCredentialCancelled,
            ),
            (
                KeychainBridgeError::InvalidResult,
                IdentityError::PlatformCredentialCorrupt,
            ),
            (
                KeychainBridgeError::Failed,
                IdentityError::PlatformCredentialFailed,
            ),
        ] {
            let bridge = Arc::new(FakeBridge::default());
            bridge.state.lock().unwrap().load_error = Some(bridge_error);
            let store = MacOsKeychainInstallationKeyStore::with_bridge(bridge);
            let result = store.load();
            assert!(matches!(result, Err(error) if error == expected));
            let rendered = expected.to_string();
            assert!(!rendered.contains(KEY_ID));
            assert!(!rendered.contains(MACOS_KEYCHAIN_SERVICE));
        }
    }

    #[test]
    fn local_build_uses_login_keychain_only_for_missing_entitlement() {
        let protected = Arc::new(FakeBridge::default());
        protected.state.lock().unwrap().create_error =
            Some(KeychainBridgeError::MissingEntitlement);
        let login = Arc::new(FakeBridge::default());
        let store = MacOsKeychainInstallationKeyStore::with_bridge(Arc::new(
            CompatibleKeychainBridge::new(protected.clone(), login.clone()),
        ));
        let installed = store.load_or_create_key_ring().unwrap();
        assert!(installed.same_material(&store.load_key_ring().unwrap().unwrap()));
        assert_eq!(protected.state.lock().unwrap().create_calls, 1);
        assert_eq!(login.state.lock().unwrap().create_calls, 1);
        // A later build with working entitlements must keep the same HMAC key.
        protected.state.lock().unwrap().create_error = None;
        let reopened = MacOsKeychainInstallationKeyStore::with_bridge(Arc::new(
            CompatibleKeychainBridge::new(protected.clone(), login.clone()),
        ));
        assert!(installed.same_material(&reopened.load_or_create_key_ring().unwrap()));
        assert_eq!(protected.state.lock().unwrap().create_calls, 1);
        assert_eq!(login.state.lock().unwrap().create_calls, 1);
    }

    #[test]
    fn local_compatibility_never_bypasses_denial_lock_or_corruption() {
        for error in [
            KeychainBridgeError::Denied,
            KeychainBridgeError::InteractionRequired,
            KeychainBridgeError::Unavailable,
            KeychainBridgeError::Cancelled,
            KeychainBridgeError::InvalidResult,
        ] {
            let protected = Arc::new(FakeBridge::default());
            protected.state.lock().unwrap().create_error = Some(error);
            let login = Arc::new(FakeBridge::default());
            let store = MacOsKeychainInstallationKeyStore::with_bridge(Arc::new(
                CompatibleKeychainBridge::new(protected, login.clone()),
            ));
            assert_eq!(
                store.load_or_create_key_ring().unwrap_err(),
                map_bridge_error(error)
            );
            assert_eq!(login.state.lock().unwrap().create_calls, 0);
        }
        let protected = Arc::new(FakeBridge::default());
        let login = Arc::new(FakeBridge::with_value(b"corrupt".to_vec()));
        let store = MacOsKeychainInstallationKeyStore::with_bridge(Arc::new(
            CompatibleKeychainBridge::new(protected.clone(), login.clone()),
        ));
        assert_eq!(
            store.load_or_create_key_ring().unwrap_err(),
            IdentityError::PlatformCredentialCorrupt
        );
        assert_eq!(protected.state.lock().unwrap().create_calls, 0);
        assert_eq!(login.state.lock().unwrap().create_calls, 0);
    }

    #[test]
    fn login_queries_keep_native_acl_and_never_request_data_protection() {
        let query = build_copy_query_for_location(
            MACOS_KEYCHAIN_SERVICE,
            MACOS_KEYCHAIN_ACCOUNT,
            KeychainLocation::Login,
        )
        .unwrap();
        let record = encoded_ring(&InstallationKeyRingRecord::initial(key(KEY_ID, 0x71)));
        let attributes = build_add_attributes_for_location(
            MACOS_KEYCHAIN_SERVICE,
            MACOS_KEYCHAIN_ACCOUNT,
            MACOS_KEYCHAIN_LABEL,
            &record,
            KeychainLocation::Login,
        )
        .unwrap();
        unsafe {
            assert!(
                cf_dictionary_get_value(query.as_ptr(), K_SEC_USE_DATA_PROTECTION_KEYCHAIN)
                    .is_null()
            );
            assert!(
                cf_dictionary_get_value(attributes.as_ptr(), K_SEC_USE_DATA_PROTECTION_KEYCHAIN)
                    .is_null()
            );
            assert!(cf_dictionary_get_value(attributes.as_ptr(), K_SEC_ATTR_ACCESSIBLE).is_null());
            assert_eq!(cf_dictionary_get_count(attributes.as_ptr()), 5);
            assert!(cf_dictionary_get_value(query.as_ptr(), K_SEC_USE_AUTHENTICATION_UI).is_null());
        }
        assert_eq!(
            map_os_status(-34018),
            KeychainBridgeError::MissingEntitlement
        );
    }

    #[test]
    fn mutation_failures_are_bounded_and_do_not_echo_key_material() {
        let initial = InstallationKeyRingRecord::initial(key(KEY_ID, 0x71));
        let expected = initial.revision_token();
        let replacement = initial.start_rotation(key(KEY_ID_2, 0x72)).unwrap();
        let update_bridge = Arc::new(FakeBridge::with_value(encoded_ring(
            &InstallationKeyRingRecord::initial(key(KEY_ID, 0x71)),
        )));
        update_bridge.state.lock().unwrap().update_error = Some(KeychainBridgeError::Denied);
        let update_store = MacOsKeychainInstallationKeyStore::with_bridge(update_bridge);
        let update_error = update_store
            .replace_key_ring(&expected, &replacement)
            .unwrap_err();
        assert_eq!(update_error, IdentityError::PlatformCredentialDenied);

        let delete_bridge = Arc::new(FakeBridge::with_value(encoded_ring(
            &InstallationKeyRingRecord::initial(key(KEY_ID, 0x71)),
        )));
        delete_bridge.state.lock().unwrap().delete_error = Some(KeychainBridgeError::Failed);
        let delete_store = MacOsKeychainInstallationKeyStore::with_bridge(delete_bridge);
        let delete_error = delete_store.delete_key_ring(&expected).unwrap_err();
        assert_eq!(delete_error, IdentityError::PlatformCredentialFailed);

        for rendered in [update_error.to_string(), delete_error.to_string()] {
            assert!(!rendered.contains(KEY_ID));
            assert!(!rendered.contains(KEY_ID_2));
            assert!(!rendered.contains(MACOS_KEYCHAIN_SERVICE));
        }
    }

    #[test]
    fn corrupt_keychain_value_never_becomes_an_installation_key() {
        let bridge = Arc::new(FakeBridge::with_value(b"partial-key-record".to_vec()));
        let store = MacOsKeychainInstallationKeyStore::with_bridge(bridge);
        assert!(matches!(
            store.load(),
            Err(IdentityError::PlatformCredentialCorrupt)
        ));
    }

    #[test]
    fn system_backend_construction_and_debug_are_side_effect_free() {
        let store = MacOsKeychainInstallationKeyStore::new();
        let rendered = format!("{store:?}");
        assert_eq!(rendered, "MacOsKeychainInstallationKeyStore(<redacted>)");
        assert!(!rendered.contains(MACOS_KEYCHAIN_SERVICE));
        assert!(!rendered.contains(MACOS_KEYCHAIN_ACCOUNT));
    }

    #[test]
    fn system_query_builders_have_exact_shapes_without_keychain_io() {
        let query = build_copy_query(MACOS_KEYCHAIN_SERVICE, MACOS_KEYCHAIN_ACCOUNT).unwrap();
        let mutation_query =
            build_mutation_query(MACOS_KEYCHAIN_SERVICE, MACOS_KEYCHAIN_ACCOUNT).unwrap();
        let encoded = encoded_ring(&InstallationKeyRingRecord::initial(key(KEY_ID, 0x33)));
        let attributes = build_add_attributes(
            MACOS_KEYCHAIN_SERVICE,
            MACOS_KEYCHAIN_ACCOUNT,
            MACOS_KEYCHAIN_LABEL,
            &encoded,
        )
        .unwrap();
        let update_attributes = build_update_attributes(&encoded).unwrap();
        assert_eq!(unsafe { cf_dictionary_get_count(query.as_ptr()) }, 8);
        assert_eq!(
            unsafe { cf_dictionary_get_count(mutation_query.as_ptr()) },
            6
        );
        assert_eq!(unsafe { cf_dictionary_get_count(attributes.as_ptr()) }, 8);
        assert_eq!(
            unsafe { cf_dictionary_get_count(update_attributes.as_ptr()) },
            1
        );

        let service = CfOwned::string(MACOS_KEYCHAIN_SERVICE).unwrap();
        let account = CfOwned::string(MACOS_KEYCHAIN_ACCOUNT).unwrap();
        let label = CfOwned::string(MACOS_KEYCHAIN_LABEL).unwrap();
        let data = CfOwned::data(&encoded).unwrap();
        assert_dictionary_value(&query, unsafe { K_SEC_CLASS }, unsafe {
            K_SEC_CLASS_GENERIC_PASSWORD
        });
        assert_dictionary_value(&query, unsafe { K_SEC_ATTR_SERVICE }, service.as_ptr());
        assert_dictionary_value(&query, unsafe { K_SEC_ATTR_ACCOUNT }, account.as_ptr());
        assert_dictionary_value(&query, unsafe { K_SEC_ATTR_SYNCHRONIZABLE }, unsafe {
            K_CF_BOOLEAN_FALSE
        });
        assert_dictionary_value(
            &query,
            unsafe { K_SEC_USE_DATA_PROTECTION_KEYCHAIN },
            unsafe { K_CF_BOOLEAN_TRUE },
        );
        assert_dictionary_value(&query, unsafe { K_SEC_RETURN_DATA }, unsafe {
            K_CF_BOOLEAN_TRUE
        });
        assert_dictionary_value(&query, unsafe { K_SEC_MATCH_LIMIT }, unsafe {
            K_SEC_MATCH_LIMIT_ONE
        });
        assert_dictionary_value(&query, unsafe { K_SEC_USE_AUTHENTICATION_UI }, unsafe {
            K_SEC_USE_AUTHENTICATION_UI_FAIL
        });

        assert_dictionary_value(&mutation_query, unsafe { K_SEC_CLASS }, unsafe {
            K_SEC_CLASS_GENERIC_PASSWORD
        });
        assert_dictionary_value(
            &mutation_query,
            unsafe { K_SEC_ATTR_SERVICE },
            service.as_ptr(),
        );
        assert_dictionary_value(
            &mutation_query,
            unsafe { K_SEC_ATTR_ACCOUNT },
            account.as_ptr(),
        );
        assert_dictionary_value(
            &mutation_query,
            unsafe { K_SEC_ATTR_SYNCHRONIZABLE },
            unsafe { K_CF_BOOLEAN_FALSE },
        );
        assert_dictionary_value(
            &mutation_query,
            unsafe { K_SEC_USE_DATA_PROTECTION_KEYCHAIN },
            unsafe { K_CF_BOOLEAN_TRUE },
        );
        assert_dictionary_value(
            &mutation_query,
            unsafe { K_SEC_USE_AUTHENTICATION_UI },
            unsafe { K_SEC_USE_AUTHENTICATION_UI_FAIL },
        );

        assert_dictionary_value(&attributes, unsafe { K_SEC_CLASS }, unsafe {
            K_SEC_CLASS_GENERIC_PASSWORD
        });
        assert_dictionary_value(&attributes, unsafe { K_SEC_ATTR_SERVICE }, service.as_ptr());
        assert_dictionary_value(&attributes, unsafe { K_SEC_ATTR_ACCOUNT }, account.as_ptr());
        assert_dictionary_value(&attributes, unsafe { K_SEC_ATTR_LABEL }, label.as_ptr());
        assert_dictionary_value(&attributes, unsafe { K_SEC_ATTR_SYNCHRONIZABLE }, unsafe {
            K_CF_BOOLEAN_FALSE
        });
        assert_dictionary_value(
            &attributes,
            unsafe { K_SEC_USE_DATA_PROTECTION_KEYCHAIN },
            unsafe { K_CF_BOOLEAN_TRUE },
        );
        assert_dictionary_value(&attributes, unsafe { K_SEC_ATTR_ACCESSIBLE }, unsafe {
            K_SEC_ATTR_ACCESSIBLE_AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY
        });
        assert_dictionary_value(&attributes, unsafe { K_SEC_VALUE_DATA }, data.as_ptr());
        assert_dictionary_value(
            &update_attributes,
            unsafe { K_SEC_VALUE_DATA },
            data.as_ptr(),
        );
        assert!(matches!(
            build_add_attributes(
                MACOS_KEYCHAIN_SERVICE,
                MACOS_KEYCHAIN_ACCOUNT,
                MACOS_KEYCHAIN_LABEL,
                b"wrong-length",
            ),
            Err(KeychainBridgeError::InvalidResult)
        ));
        assert!(matches!(
            build_update_attributes(b"wrong-length"),
            Err(KeychainBridgeError::InvalidResult)
        ));
    }

    fn assert_dictionary_value(dictionary: &CfOwned, key: CfTypeRef, expected: CfTypeRef) {
        let actual = unsafe { cf_dictionary_get_value(dictionary.as_ptr(), key) };
        assert!(!actual.is_null());
        assert_ne!(unsafe { cf_equal(actual, expected) }, 0);
    }

    #[test]
    fn os_status_mapping_is_bounded_and_unknown_status_is_generic() {
        assert_eq!(
            map_os_status(ERR_SEC_NOT_AVAILABLE),
            KeychainBridgeError::Unavailable
        );
        assert_eq!(
            map_os_status(ERR_SEC_AUTH_FAILED),
            KeychainBridgeError::Denied
        );
        assert_eq!(
            map_os_status(ERR_SEC_INTERACTION_NOT_ALLOWED),
            KeychainBridgeError::InteractionRequired
        );
        assert_eq!(
            map_os_status(ERR_SEC_USER_CANCELED),
            KeychainBridgeError::Cancelled
        );
        assert_eq!(map_os_status(-99999), KeychainBridgeError::Failed);
    }

    #[test]
    fn platform_selection_has_no_implicit_fallback_side_effect() {
        let selected = super::super::select_installation_key_store(
            InstallationKeyBackendRequest::PlatformRequired,
        )
        .unwrap();
        assert!(matches!(
            selected,
            super::super::SelectedInstallationKeyStore::MacOsKeychain(_)
        ));
    }
}
