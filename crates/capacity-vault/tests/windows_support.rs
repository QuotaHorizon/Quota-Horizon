#![cfg(windows)]

use capacity_vault::{
    FileInstallationKeyStore, FileVault, IdentityError, InstallationKey,
    InstallationKeyBackendRequest, SelectedInstallationKeyStore, VaultBackendError,
    select_installation_key_store,
};

#[test]
fn windows_selects_native_backend_without_accessing_credentials() {
    assert!(matches!(
        select_installation_key_store(InstallationKeyBackendRequest::PlatformRequired),
        Ok(SelectedInstallationKeyStore::WindowsCredential(_))
    ));
}

#[test]
fn windows_can_generate_installation_keys() {
    let first = InstallationKey::generate().unwrap();
    let second = InstallationKey::generate().unwrap();
    assert_ne!(first.key_id(), second.key_id());
}

#[test]
fn unix_file_backends_do_not_create_unprotected_windows_stores() {
    let root = std::env::temp_dir().join(format!(
        "quota-horizon-unsupported-vault-{}",
        std::process::id()
    ));
    assert!(!root.exists());
    assert!(matches!(
        FileInstallationKeyStore::open_explicit_fallback(&root),
        Err(IdentityError::Storage(
            VaultBackendError::PlatformPermissionsUnavailable
        ))
    ));
    assert!(matches!(
        FileVault::open(&root),
        Err(VaultBackendError::PlatformPermissionsUnavailable)
    ));
    assert!(!root.exists());
}
