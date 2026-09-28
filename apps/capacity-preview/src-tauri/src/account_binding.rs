use std::fmt;

use capacity_domain::AccountFingerprint;
use capacity_vault::{
    IdentityError, InstallationKeyStore, ProtectedVaultRecord, SelectedInstallationKeyStore,
    derive_account_fingerprint,
};
#[cfg(any(test, not(target_os = "macos")))]
use capacity_vault::{InstallationKeyBackendRequest, select_installation_key_store};

const ACCOUNT_DIGEST_BYTES: usize = 24;
const ADAPTER_NAMESPACE: &str = "openai-codex";
const IDENTITY_KIND: &str = "manager-account-digest-v1";

/// A non-secret, one-way account digest produced by the formal desktop host.
///
/// The raw upstream account identifier and auth material never cross this
/// boundary. The digest is HMAC-bound again with the installation key before
/// it is accepted by persistence as a stable account fingerprint.
#[derive(Clone, PartialEq, Eq)]
pub struct DesktopAccountIdentity(String);

impl DesktopAccountIdentity {
    pub fn from_account_digest(value: impl Into<String>) -> Result<Self, AccountBindingError> {
        let value = value.into();
        if value.len() != ACCOUNT_DIGEST_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(AccountBindingError::InvalidIdentity);
        }
        Ok(Self(value))
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for DesktopAccountIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DesktopAccountIdentity(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountBindingError {
    InvalidIdentity,
    PlatformCredentialUnavailable,
    PlatformCredentialDenied,
    PlatformCredentialInteractionRequired,
    PlatformCredentialMissingEntitlement,
    PlatformCredentialCorrupt,
    FingerprintDerivationFailed,
}

impl AccountBindingError {
    pub(crate) fn reason_code(self) -> &'static str {
        match self {
            Self::InvalidIdentity => "history_identity_invalid",
            Self::PlatformCredentialUnavailable => "history_keychain_unavailable",
            Self::PlatformCredentialDenied => "history_keychain_denied",
            Self::PlatformCredentialInteractionRequired => "history_keychain_interaction_required",
            Self::PlatformCredentialMissingEntitlement => "history_keychain_missing_entitlement",
            Self::PlatformCredentialCorrupt => "history_keychain_corrupt",
            Self::FingerprintDerivationFailed => "history_fingerprint_failed",
        }
    }

    pub(crate) fn from_key_error(error: IdentityError) -> Self {
        match error {
            IdentityError::PlatformCredentialDenied
            | IdentityError::PlatformCredentialCancelled => Self::PlatformCredentialDenied,
            IdentityError::PlatformCredentialInteractionRequired => {
                Self::PlatformCredentialInteractionRequired
            }
            IdentityError::PlatformCredentialMissingEntitlement => {
                Self::PlatformCredentialMissingEntitlement
            }
            IdentityError::PlatformCredentialCorrupt | IdentityError::KeyRecordCorrupt => {
                Self::PlatformCredentialCorrupt
            }
            _ => Self::PlatformCredentialUnavailable,
        }
    }
}

pub(crate) struct DesktopAccountBindingStore {
    backend: Option<SelectedInstallationKeyStore>,
}

impl DesktopAccountBindingStore {
    #[cfg(test)]
    pub(crate) fn unavailable() -> Self {
        Self { backend: None }
    }

    pub(crate) fn platform_required() -> Self {
        #[cfg(target_os = "macos")]
        let backend = Some(SelectedInstallationKeyStore::MacOsKeychain(
            capacity_vault::MacOsKeychainInstallationKeyStore::compatible_with_local_builds(),
        ));
        #[cfg(not(target_os = "macos"))]
        let backend =
            select_installation_key_store(InstallationKeyBackendRequest::PlatformRequired).ok();
        Self { backend }
    }

    pub(crate) fn fingerprint(
        &mut self,
        identity: &DesktopAccountIdentity,
    ) -> Result<AccountFingerprint, AccountBindingError> {
        let backend = self
            .backend
            .as_mut()
            .ok_or(AccountBindingError::PlatformCredentialUnavailable)?;
        let key = backend
            .load_or_create()
            .map_err(AccountBindingError::from_key_error)?;
        // Fingerprint derivation uses only the exact identity tuple. The
        // placeholder payload is never persisted and deliberately contains no
        // authentication material.
        let record = ProtectedVaultRecord::new(
            ADAPTER_NAMESPACE,
            IDENTITY_KIND,
            identity.as_bytes().to_vec(),
            b"{}".to_vec(),
            None,
        )
        .map_err(|_| AccountBindingError::FingerprintDerivationFailed)?;
        derive_account_fingerprint(&key, &record)
            .map_err(|_| AccountBindingError::FingerprintDerivationFailed)
    }

    #[cfg(test)]
    pub(crate) fn explicit_file_fallback(
        root: &std::path::Path,
    ) -> Result<Self, AccountBindingError> {
        let backend =
            select_installation_key_store(InstallationKeyBackendRequest::ExplicitFileFallback {
                root: root.to_path_buf(),
            })
            .map_err(|_| AccountBindingError::PlatformCredentialUnavailable)?;
        Ok(Self {
            backend: Some(backend),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT_A: &str = "0123456789abcdef01234567";
    const ACCOUNT_B: &str = "89abcdef0123456789abcdef";

    #[test]
    fn accepts_only_the_formal_hosts_bounded_lowercase_digest() {
        assert!(DesktopAccountIdentity::from_account_digest(ACCOUNT_A).is_ok());
        for invalid in [
            "person@example.com",
            "0123456789ABCDEF01234567",
            "0123456789abcdef0123456",
            "0123456789abcdef012345678",
            "0123456789abcdef01234\n67",
        ] {
            assert_eq!(
                DesktopAccountIdentity::from_account_digest(invalid).unwrap_err(),
                AccountBindingError::InvalidIdentity
            );
        }
    }

    #[test]
    fn debug_output_never_contains_the_account_digest() {
        let identity = DesktopAccountIdentity::from_account_digest(ACCOUNT_A).unwrap();
        let rendered = format!("{identity:?}");
        assert!(!rendered.contains(ACCOUNT_A));
        assert_eq!(rendered, "DesktopAccountIdentity(<redacted>)");
    }

    #[test]
    fn explicit_test_store_is_stable_and_account_separated() {
        let directory = tempfile::tempdir().expect("temporary key store");
        let key_store = directory.path().join("key-store");
        let mut first =
            DesktopAccountBindingStore::explicit_file_fallback(&key_store).expect("open key store");
        let account_a = DesktopAccountIdentity::from_account_digest(ACCOUNT_A).unwrap();
        let account_b = DesktopAccountIdentity::from_account_digest(ACCOUNT_B).unwrap();
        let fingerprint_a = first.fingerprint(&account_a).expect("first fingerprint");
        let fingerprint_b = first.fingerprint(&account_b).expect("second fingerprint");
        assert_ne!(fingerprint_a, fingerprint_b);

        let mut reopened = DesktopAccountBindingStore::explicit_file_fallback(&key_store)
            .expect("reopen key store");
        assert_eq!(
            reopened
                .fingerprint(&account_a)
                .expect("stable fingerprint"),
            fingerprint_a
        );
    }
}
