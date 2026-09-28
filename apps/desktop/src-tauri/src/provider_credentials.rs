use std::{
    collections::BTreeSet,
    fmt, fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{models::ProviderProfile, storage::write_json_atomic};

const PROVIDER_CREDENTIAL_SERVICE: &str = "dev.quota-horizon.provider-credential.v1";
const PROVIDER_CREDENTIAL_REF_PREFIX: &str = "provider-credential:v1:";
const PROVIDER_CREDENTIAL_REF_FIELD: &str = "credentialRef";
const PROVIDER_CREDENTIAL_INDEX_FILE: &str = "provider-credential-index-v1";
const PROVIDER_CREDENTIAL_INDEX_FORMAT: &str = "quota-horizon-provider-credential-index-v1";
const ACTIVE_PROVIDER_RESTORE_PAYLOAD: &str = "active-provider-profile.bin";
const MAX_INDEXED_PROVIDER_CREDENTIALS: usize = 4096;

static PROVIDER_CREDENTIAL_LOCK: Mutex<()> = Mutex::new(());

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderCredentialIndex {
    format: String,
    refs: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderCredentialPruneReport {
    pub(crate) indexed: usize,
    pub(crate) rooted: usize,
    pub(crate) deleted: usize,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderCredentialBundle {
    provider_id: String,
    base_url: String,
    api_key: String,
    balance_query_token: Option<String>,
    wallet_query_token: Option<String>,
    wallet_password: Option<String>,
}

impl ProviderCredentialBundle {
    fn from_profile(profile: &ProviderProfile) -> Self {
        Self {
            provider_id: profile.id.clone(),
            base_url: profile.base_url.clone(),
            api_key: profile.api_key.clone(),
            balance_query_token: non_empty(profile.balance_query_token.as_deref()),
            wallet_query_token: non_empty(profile.wallet_query_token.as_deref()),
            wallet_password: non_empty(profile.wallet_password.as_deref()),
        }
    }

    fn is_empty(&self) -> bool {
        self.api_key.trim().is_empty()
            && self.balance_query_token.is_none()
            && self.wallet_query_token.is_none()
            && self.wallet_password.is_none()
    }

    fn hydrate(&self, profile: &mut ProviderProfile) -> Result<(), String> {
        if self.provider_id != profile.id || self.base_url != profile.base_url {
            return Err(
                "Provider credential owner or endpoint does not match its profile".to_string(),
            );
        }
        profile.api_key.clone_from(&self.api_key);
        profile
            .balance_query_token
            .clone_from(&self.balance_query_token);
        profile
            .wallet_query_token
            .clone_from(&self.wallet_query_token);
        profile.wallet_password.clone_from(&self.wallet_password);
        Ok(())
    }
}

impl fmt::Debug for ProviderCredentialBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderCredentialBundle")
            .field("provider_id", &self.provider_id)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field(
                "balance_query_token",
                &self.balance_query_token.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "wallet_query_token",
                &self.wallet_query_token.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "wallet_password",
                &self.wallet_password.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|value| !value.is_empty()).map(str::to_string)
}

trait ProviderCredentialBackend {
    fn read(&self, credential_ref: &str) -> Result<Option<Vec<u8>>, String>;
    fn write(&self, credential_ref: &str, value: &[u8]) -> Result<(), String>;
    fn delete(&self, credential_ref: &str) -> Result<(), String>;
}

pub(crate) struct SystemProviderCredentialBackend;

impl SystemProviderCredentialBackend {
    fn entry(credential_ref: &str) -> Result<keyring::Entry, String> {
        validate_credential_ref(credential_ref)?;
        keyring::Entry::new(PROVIDER_CREDENTIAL_SERVICE, credential_ref).map_err(|error| {
            format!("Could not access the system Provider credential store: {error}")
        })
    }
}

impl ProviderCredentialBackend for SystemProviderCredentialBackend {
    fn read(&self, credential_ref: &str) -> Result<Option<Vec<u8>>, String> {
        match Self::entry(credential_ref)?.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(format!(
                "Could not read the saved Provider credential from the system store: {error}"
            )),
        }
    }

    fn write(&self, credential_ref: &str, value: &[u8]) -> Result<(), String> {
        Self::entry(credential_ref)?
            .set_secret(value)
            .map_err(|error| format!("Could not save the Provider credential: {error}"))
    }

    fn delete(&self, credential_ref: &str) -> Result<(), String> {
        match Self::entry(credential_ref)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(format!("Could not remove the Provider credential: {error}")),
        }
    }
}

pub(crate) fn read_provider_profile(path: &Path) -> Result<ProviderProfile, String> {
    read_provider_profile_with_backend(path, &SystemProviderCredentialBackend)
}

fn read_provider_profile_with_backend(
    path: &Path,
    backend: &dyn ProviderCredentialBackend,
) -> Result<ProviderProfile, String> {
    let value = crate::storage::read_json(path)?;
    profile_from_value(path, value, backend)
}

fn profile_from_value(
    path: &Path,
    value: Value,
    backend: &dyn ProviderCredentialBackend,
) -> Result<ProviderProfile, String> {
    let credential_ref = credential_ref_from_value(&value)?;
    let mut profile: ProviderProfile = serde_json::from_value(value.clone())
        .map_err(|error| format!("Provider profile {} is invalid: {error}", path.display()))?;
    let Some(credential_ref) = credential_ref else {
        return Ok(profile);
    };
    if profile_contains_plaintext_secrets(&profile) {
        return Err(format!(
            "Provider profile {} contains both plaintext and vaulted credentials",
            path.display()
        ));
    }
    let encoded = backend.read(&credential_ref)?.ok_or_else(|| {
        format!(
            "Provider profile {} credential is unavailable",
            path.display()
        )
    })?;
    let bundle: ProviderCredentialBundle = serde_json::from_slice(&encoded)
        .map_err(|_| format!("Provider profile {} credential is invalid", path.display()))?;
    bundle.hydrate(&mut profile)?;
    Ok(profile)
}

#[cfg(not(test))]
pub(crate) fn write_provider_profile(path: &Path, profile: &ProviderProfile) -> Result<(), String> {
    let _guard = PROVIDER_CREDENTIAL_LOCK
        .lock()
        .map_err(|_| "Provider credential lock is unavailable".to_string())?;
    write_provider_profile_with_backend(path, profile, &SystemProviderCredentialBackend)
}

// Existing provider unit tests intentionally create legacy plaintext fixtures. The production
// writer above is covered through backend-injected tests without touching the developer Keychain.
#[cfg(test)]
pub(crate) fn write_provider_profile(path: &Path, profile: &ProviderProfile) -> Result<(), String> {
    let value = serde_json::to_value(profile).map_err(|error| error.to_string())?;
    write_json_atomic(path, &value)
}

fn write_provider_profile_with_backend(
    path: &Path,
    profile: &ProviderProfile,
    backend: &dyn ProviderCredentialBackend,
) -> Result<(), String> {
    let previous = match fs::read(path) {
        Ok(bytes) => Some(serde_json::from_slice::<Value>(&bytes).map_err(|error| {
            format!(
                "Existing Provider profile {} is invalid: {error}",
                path.display()
            )
        })?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "Failed to read existing Provider profile {}: {error}",
                path.display()
            ));
        }
    };
    let previous_ref = previous
        .as_ref()
        .map(credential_ref_from_value)
        .transpose()?
        .flatten();
    let bundle = ProviderCredentialBundle::from_profile(profile);
    let reusable_ref = previous_ref
        .as_deref()
        .map(|credential_ref| read_bundle(backend, credential_ref))
        .transpose()?
        .filter(|existing| existing == &bundle)
        .and(previous_ref);
    let (credential_ref, created_ref) = if bundle.is_empty() {
        (None, None)
    } else if let Some(credential_ref) = reusable_ref {
        (Some(credential_ref), None)
    } else {
        let credential_ref = new_credential_ref();
        let encoded = serde_json::to_vec(&bundle)
            .map_err(|error| format!("Could not encode the Provider credential: {error}"))?;
        backend.write(&credential_ref, &encoded)?;
        if read_bundle(backend, &credential_ref)? != bundle {
            let _ = backend.delete(&credential_ref);
            return Err("Provider credential verification failed".to_string());
        }
        if let Err(error) = register_credential_ref(path, &credential_ref) {
            let _ = backend.delete(&credential_ref);
            return Err(error);
        }
        (Some(credential_ref.clone()), Some(credential_ref))
    };

    let value = redacted_profile_value(profile, credential_ref.as_deref())?;
    if let Err(error) = write_json_atomic(path, &value) {
        cleanup_created_ref(path, backend, created_ref.as_deref());
        return Err(error);
    }
    let postflight = read_provider_profile_with_backend(path, backend)
        .and_then(|installed| profile_values_match(profile, &installed));
    if let Err(error) = postflight {
        let restore_result = restore_previous_profile(path, previous.as_ref());
        cleanup_created_ref(path, backend, created_ref.as_deref());
        return match restore_result {
            Ok(()) => Err(format!("Provider credential postflight failed: {error}")),
            Err(restore_error) => Err(format!(
                "Provider credential postflight failed: {error}; profile restore failed: {restore_error}"
            )),
        };
    }
    Ok(())
}

pub(crate) fn migrate_legacy_provider_profile(path: &Path) -> Result<bool, String> {
    let _guard = PROVIDER_CREDENTIAL_LOCK
        .lock()
        .map_err(|_| "Provider credential lock is unavailable".to_string())?;
    migrate_legacy_provider_profile_with_backend(path, &SystemProviderCredentialBackend)
}

pub(crate) fn migrate_legacy_provider_store(root: &Path) -> Result<usize, String> {
    fs::create_dir_all(root)
        .map_err(|error| format!("Failed to create Provider store: {error}"))?;
    let mut migrated = 0_usize;
    for entry in
        fs::read_dir(root).map_err(|error| format!("Failed to read Provider store: {error}"))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if !path.is_file()
            || path.extension().and_then(|value| value.to_str()) != Some("json")
            || entry
                .file_name()
                .to_string_lossy()
                .ends_with(".field-modified-at.json")
        {
            continue;
        }
        migrated += usize::from(migrate_legacy_provider_profile(&path)?);
    }
    Ok(migrated)
}

fn migrate_legacy_provider_profile_with_backend(
    path: &Path,
    backend: &dyn ProviderCredentialBackend,
) -> Result<bool, String> {
    let value = crate::storage::read_json(path)?;
    if credential_ref_from_value(&value)?.is_some() {
        let _ = profile_from_value(path, value, backend)?;
        return Ok(false);
    }
    let profile: ProviderProfile = serde_json::from_value(value)
        .map_err(|error| format!("Provider profile {} is invalid: {error}", path.display()))?;
    if !profile_contains_plaintext_secrets(&profile) {
        return Ok(false);
    }
    write_provider_profile_with_backend(path, &profile, backend)?;
    Ok(true)
}

fn redacted_profile_value(
    profile: &ProviderProfile,
    credential_ref: Option<&str>,
) -> Result<Value, String> {
    let mut value = serde_json::to_value(profile).map_err(|error| error.to_string())?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "Provider profile must be a JSON object".to_string())?;
    object.insert("apiKey".to_string(), Value::String(String::new()));
    object.insert("balanceQueryToken".to_string(), Value::Null);
    object.insert("walletQueryToken".to_string(), Value::Null);
    object.insert("walletPassword".to_string(), Value::Null);
    match credential_ref {
        Some(credential_ref) => {
            validate_credential_ref(credential_ref)?;
            object.insert(
                PROVIDER_CREDENTIAL_REF_FIELD.to_string(),
                Value::String(credential_ref.to_string()),
            );
        }
        None => {
            object.remove(PROVIDER_CREDENTIAL_REF_FIELD);
        }
    }
    Ok(value)
}

fn credential_ref_from_value(value: &Value) -> Result<Option<String>, String> {
    let Some(value) = value.get(PROVIDER_CREDENTIAL_REF_FIELD) else {
        return Ok(None);
    };
    let credential_ref = value
        .as_str()
        .ok_or_else(|| "Provider credentialRef must be a string".to_string())?;
    validate_credential_ref(credential_ref)?;
    Ok(Some(credential_ref.to_string()))
}

fn validate_credential_ref(value: &str) -> Result<(), String> {
    let suffix = value
        .strip_prefix(PROVIDER_CREDENTIAL_REF_PREFIX)
        .ok_or_else(|| "Provider credentialRef has an unsupported format".to_string())?;
    let uuid = Uuid::parse_str(suffix)
        .map_err(|_| "Provider credentialRef has an unsupported format".to_string())?;
    if uuid.to_string() != suffix {
        return Err("Provider credentialRef has an unsupported format".to_string());
    }
    Ok(())
}

fn new_credential_ref() -> String {
    format!("{PROVIDER_CREDENTIAL_REF_PREFIX}{}", Uuid::new_v4())
}

fn read_bundle(
    backend: &dyn ProviderCredentialBackend,
    credential_ref: &str,
) -> Result<ProviderCredentialBundle, String> {
    let encoded = backend
        .read(credential_ref)?
        .ok_or_else(|| "Saved Provider credential is unavailable".to_string())?;
    serde_json::from_slice(&encoded).map_err(|_| "Saved Provider credential is invalid".to_string())
}

fn profile_contains_plaintext_secrets(profile: &ProviderProfile) -> bool {
    !profile.api_key.is_empty()
        || non_empty(profile.balance_query_token.as_deref()).is_some()
        || non_empty(profile.wallet_query_token.as_deref()).is_some()
        || non_empty(profile.wallet_password.as_deref()).is_some()
}

fn profile_values_match(
    expected: &ProviderProfile,
    installed: &ProviderProfile,
) -> Result<(), String> {
    let expected = serde_json::to_value(expected).map_err(|error| error.to_string())?;
    let installed = serde_json::to_value(installed).map_err(|error| error.to_string())?;
    if expected == installed {
        Ok(())
    } else {
        Err("installed Provider profile does not match the requested profile".to_string())
    }
}

fn restore_previous_profile(path: &Path, previous: Option<&Value>) -> Result<(), String> {
    match previous {
        Some(previous) => write_json_atomic(path, previous),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "Failed to remove incomplete Provider profile: {error}"
            )),
        },
    }
}

fn credential_index_path(profile_path: &Path) -> Result<PathBuf, String> {
    profile_path
        .parent()
        .map(|parent| parent.join(PROVIDER_CREDENTIAL_INDEX_FILE))
        .ok_or_else(|| "Provider profile has no credential index directory".to_string())
}

fn load_credential_index(path: &Path) -> Result<ProviderCredentialIndex, String> {
    let value = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
            .map_err(|error| format!("Provider credential index is invalid: {error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProviderCredentialIndex {
                format: PROVIDER_CREDENTIAL_INDEX_FORMAT.to_string(),
                refs: BTreeSet::new(),
            });
        }
        Err(error) => return Err(format!("Failed to read Provider credential index: {error}")),
    };
    let index: ProviderCredentialIndex = serde_json::from_value(value)
        .map_err(|error| format!("Provider credential index is invalid: {error}"))?;
    if index.format != PROVIDER_CREDENTIAL_INDEX_FORMAT
        || index.refs.len() > MAX_INDEXED_PROVIDER_CREDENTIALS
        || index
            .refs
            .iter()
            .any(|credential_ref| validate_credential_ref(credential_ref).is_err())
    {
        return Err("Provider credential index is invalid".to_string());
    }
    Ok(index)
}

fn write_credential_index(path: &Path, index: &ProviderCredentialIndex) -> Result<(), String> {
    if index.refs.len() > MAX_INDEXED_PROVIDER_CREDENTIALS {
        return Err("Provider credential index limit was exceeded".to_string());
    }
    let value = serde_json::to_value(index)
        .map_err(|error| format!("Could not encode Provider credential index: {error}"))?;
    write_json_atomic(path, &value)
}

fn register_credential_ref(profile_path: &Path, credential_ref: &str) -> Result<(), String> {
    validate_credential_ref(credential_ref)?;
    let index_path = credential_index_path(profile_path)?;
    let mut index = load_credential_index(&index_path)?;
    if index.refs.insert(credential_ref.to_string()) {
        write_credential_index(&index_path, &index)?;
    }
    Ok(())
}

fn unregister_credential_ref(profile_path: &Path, credential_ref: &str) -> Result<(), String> {
    let index_path = credential_index_path(profile_path)?;
    let mut index = load_credential_index(&index_path)?;
    if index.refs.remove(credential_ref) {
        write_credential_index(&index_path, &index)?;
    }
    Ok(())
}

pub(crate) fn prune_orphaned_provider_credentials(
    provider_root: &Path,
) -> Result<ProviderCredentialPruneReport, String> {
    let _guard = PROVIDER_CREDENTIAL_LOCK
        .lock()
        .map_err(|_| "Provider credential lock is unavailable".to_string())?;
    prune_orphaned_provider_credentials_with_backend(
        provider_root,
        &SystemProviderCredentialBackend,
    )
}

fn prune_orphaned_provider_credentials_with_backend(
    provider_root: &Path,
    backend: &dyn ProviderCredentialBackend,
) -> Result<ProviderCredentialPruneReport, String> {
    let index_path = provider_root.join(PROVIDER_CREDENTIAL_INDEX_FILE);
    let mut index = load_credential_index(&index_path)?;
    let roots = collect_credential_roots(provider_root)?;
    let mut index_changed = false;
    for credential_ref in &roots {
        index_changed |= index.refs.insert(credential_ref.clone());
    }
    let indexed = index.refs.len();
    let orphaned = index.refs.difference(&roots).cloned().collect::<Vec<_>>();
    let mut deleted = 0_usize;
    for credential_ref in orphaned {
        backend.delete(&credential_ref)?;
        index.refs.remove(&credential_ref);
        deleted += 1;
    }
    if index_changed || deleted > 0 {
        write_credential_index(&index_path, &index)?;
    }
    Ok(ProviderCredentialPruneReport {
        indexed,
        rooted: roots.len(),
        deleted,
    })
}

fn collect_credential_roots(provider_root: &Path) -> Result<BTreeSet<String>, String> {
    let mut roots = BTreeSet::new();
    if provider_root.exists() {
        for entry in fs::read_dir(provider_root)
            .map_err(|error| format!("Failed to inventory Provider profiles: {error}"))?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if !path.is_file()
                || (path.extension().and_then(|value| value.to_str()) != Some("json")
                    && !is_provider_import_stage(&path))
                || entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".field-modified-at.json")
            {
                continue;
            }
            let value = crate::storage::read_json(&path)?;
            if let Some(credential_ref) = credential_ref_from_value(&value)? {
                roots.insert(credential_ref);
            }
        }
    }
    let Some(app_data) = provider_root.parent() else {
        return Err("Provider store has no application data directory".to_string());
    };
    let restore_root = app_data.join("restore-points-v1");
    if !restore_root.exists() {
        return Ok(roots);
    }
    for entry in fs::read_dir(&restore_root)
        .map_err(|error| format!("Failed to inventory Provider restore points: {error}"))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry.path().is_dir() || entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let payload = entry
            .path()
            .join("files")
            .join(ACTIVE_PROVIDER_RESTORE_PAYLOAD);
        let value = match fs::read(&payload) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                format!("Provider restore-point credential reference is invalid: {error}")
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "Failed to inventory Provider restore-point credential reference: {error}"
                ));
            }
        };
        if let Some(credential_ref) = credential_ref_from_value(&value)? {
            roots.insert(credential_ref);
        }
    }
    Ok(roots)
}

// A migration stages only a redacted profile next to the destination. Keep its
// protected credential alive until the durable import either commits or rolls
// back. The normal Provider inventory ignores this non-JSON extension.
pub(crate) fn is_provider_import_stage(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".viewer-import-"))
        .and_then(|name| name.strip_suffix(".staged"))
        .is_some_and(|id| Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id))
}

fn cleanup_created_ref(
    profile_path: &Path,
    backend: &dyn ProviderCredentialBackend,
    credential_ref: Option<&str>,
) {
    if let Some(credential_ref) = credential_ref {
        if backend.delete(credential_ref).is_ok() {
            let _ = unregister_credential_ref(profile_path, credential_ref);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use super::*;
    use crate::models::{ProviderApiFormat, ProviderKind};

    #[derive(Default)]
    pub(crate) struct MemoryBackend {
        values: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl MemoryBackend {
        pub(crate) fn write_profile(
            &self,
            path: &Path,
            profile: &ProviderProfile,
        ) -> Result<(), String> {
            write_provider_profile_with_backend(path, profile, self)
        }

        pub(crate) fn read_profile(&self, path: &Path) -> Result<ProviderProfile, String> {
            read_provider_profile_with_backend(path, self)
        }
    }

    #[test]
    fn staged_import_credentials_are_roots_but_not_visible_provider_profiles() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("providers");
        fs::create_dir_all(&root).unwrap();
        let staged = root.join(format!(".viewer-import-{}.staged", Uuid::new_v4()));
        let backend = MemoryBackend::default();
        backend
            .write_profile(&staged, &profile("fixture-stage-secret"))
            .unwrap();
        assert!(is_provider_import_stage(&staged));
        assert_eq!(
            prune_orphaned_provider_credentials_with_backend(&root, &backend)
                .unwrap()
                .deleted,
            0
        );
        assert_eq!(
            backend.read_profile(&staged).unwrap().api_key,
            "fixture-stage-secret"
        );
        assert_ne!(staged.extension().unwrap(), "json");
        fs::remove_file(&staged).unwrap();
        assert_eq!(
            prune_orphaned_provider_credentials_with_backend(&root, &backend)
                .unwrap()
                .deleted,
            1
        );
        assert!(!is_provider_import_stage(Path::new(
            ".viewer-import-not-an-id.staged"
        )));
    }

    impl ProviderCredentialBackend for MemoryBackend {
        fn read(&self, credential_ref: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.values.lock().unwrap().get(credential_ref).cloned())
        }

        fn write(&self, credential_ref: &str, value: &[u8]) -> Result<(), String> {
            self.values
                .lock()
                .unwrap()
                .insert(credential_ref.to_string(), value.to_vec());
            Ok(())
        }

        fn delete(&self, credential_ref: &str) -> Result<(), String> {
            self.values.lock().unwrap().remove(credential_ref);
            Ok(())
        }
    }

    struct RejectWriteBackend;

    impl ProviderCredentialBackend for RejectWriteBackend {
        fn read(&self, _credential_ref: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }

        fn write(&self, _credential_ref: &str, _value: &[u8]) -> Result<(), String> {
            Err("synthetic credential store failure".to_string())
        }

        fn delete(&self, _credential_ref: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn profile(api_key: &str) -> ProviderProfile {
        ProviderProfile {
            id: "provider-a".to_string(),
            kind: ProviderKind::Custom,
            name: "Relay".to_string(),
            group: String::new(),
            base_url: "https://relay.example.com/v1".to_string(),
            api_key: api_key.to_string(),
            model: "gpt-test".to_string(),
            models: vec!["gpt-test".to_string()],
            model_reasoning_efforts: Default::default(),
            model_context_windows: Default::default(),
            model_api_formats: Default::default(),
            image_input_models: Vec::new(),
            image_input_models_configured: false,
            context_window: None,
            model_selection_controlled_by_codex: true,
            api_format: ProviderApiFormat::OpenaiResponses,
            balance_platform: None,
            balance_query_url: None,
            balance_query_token: Some("balance-secret".to_string()),
            wallet_query_url: None,
            wallet_query_token: Some("wallet-secret".to_string()),
            wallet_username: Some("wallet-user".to_string()),
            wallet_password: Some("wallet-password".to_string()),
        }
    }

    #[test]
    fn writes_only_a_reference_and_hydrates_every_secret() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider-a.json");
        let backend = MemoryBackend::default();
        let expected = profile("sk-secret-canary");

        write_provider_profile_with_backend(&path, &expected, &backend).unwrap();

        let bytes = fs::read(&path).unwrap();
        let rendered = String::from_utf8(bytes).unwrap();
        for secret in [
            "sk-secret-canary",
            "balance-secret",
            "wallet-secret",
            "wallet-password",
        ] {
            assert!(!rendered.contains(secret));
        }
        assert!(rendered.contains(PROVIDER_CREDENTIAL_REF_PREFIX));
        assert_eq!(
            serde_json::to_value(read_provider_profile_with_backend(&path, &backend).unwrap())
                .unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }

    #[test]
    fn immutable_refs_make_profile_restore_restore_the_old_secret() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider-a.json");
        let backend = MemoryBackend::default();
        let original = profile("sk-original");
        write_provider_profile_with_backend(&path, &original, &backend).unwrap();
        let original_value = crate::storage::read_json(&path).unwrap();
        let original_ref = credential_ref_from_value(&original_value).unwrap().unwrap();

        let updated = profile("sk-updated");
        write_provider_profile_with_backend(&path, &updated, &backend).unwrap();
        let updated_value = crate::storage::read_json(&path).unwrap();
        let updated_ref = credential_ref_from_value(&updated_value).unwrap().unwrap();
        assert_ne!(original_ref, updated_ref);

        write_json_atomic(&path, &original_value).unwrap();
        assert_eq!(
            read_provider_profile_with_backend(&path, &backend)
                .unwrap()
                .api_key,
            "sk-original"
        );
    }

    #[test]
    fn metadata_only_edits_reuse_the_existing_immutable_reference() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider-a.json");
        let backend = MemoryBackend::default();
        let original = profile("sk-stable");
        write_provider_profile_with_backend(&path, &original, &backend).unwrap();
        let original_ref = credential_ref_from_value(&crate::storage::read_json(&path).unwrap())
            .unwrap()
            .unwrap();

        let mut renamed = original;
        renamed.name = "Renamed Relay".to_string();
        write_provider_profile_with_backend(&path, &renamed, &backend).unwrap();
        let renamed_ref = credential_ref_from_value(&crate::storage::read_json(&path).unwrap())
            .unwrap()
            .unwrap();

        assert_eq!(original_ref, renamed_ref);
        assert_eq!(backend.values.lock().unwrap().len(), 1);
    }

    #[test]
    fn migrates_plaintext_only_after_vault_postflight() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider-a.json");
        let backend = MemoryBackend::default();
        let original = profile("sk-legacy-canary");
        write_json_atomic(&path, &serde_json::to_value(&original).unwrap()).unwrap();

        assert!(migrate_legacy_provider_profile_with_backend(&path, &backend).unwrap());
        assert!(!migrate_legacy_provider_profile_with_backend(&path, &backend).unwrap());
        assert!(!fs::read_to_string(&path)
            .unwrap()
            .contains("sk-legacy-canary"));
        assert_eq!(
            read_provider_profile_with_backend(&path, &backend)
                .unwrap()
                .api_key,
            "sk-legacy-canary"
        );
    }

    #[test]
    fn migration_failure_preserves_the_complete_legacy_profile() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider-a.json");
        let original = profile("sk-legacy-preserved");
        let original_value = serde_json::to_value(&original).unwrap();
        write_json_atomic(&path, &original_value).unwrap();

        assert_eq!(
            migrate_legacy_provider_profile_with_backend(&path, &RejectWriteBackend).unwrap_err(),
            "synthetic credential store failure"
        );
        assert_eq!(crate::storage::read_json(&path).unwrap(), original_value);
    }

    #[test]
    fn rejects_cross_profile_credential_reference_substitution() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider-a.json");
        let backend = MemoryBackend::default();
        write_provider_profile_with_backend(&path, &profile("sk-secret"), &backend).unwrap();
        let mut value = crate::storage::read_json(&path).unwrap();
        value["id"] = Value::String("provider-b".to_string());
        write_json_atomic(&path, &value).unwrap();

        assert!(read_provider_profile_with_backend(&path, &backend)
            .unwrap_err()
            .contains("owner or endpoint"));
    }

    #[test]
    fn debug_output_redacts_all_secret_values() {
        let bundle = ProviderCredentialBundle::from_profile(&profile("sk-debug-canary"));
        let rendered = format!("{bundle:?}");
        assert!(!rendered.contains("sk-debug-canary"));
        assert!(!rendered.contains("balance-secret"));
        assert!(!rendered.contains("wallet-secret"));
        assert!(!rendered.contains("wallet-password"));
    }

    #[test]
    fn prune_keeps_restore_point_refs_then_deletes_only_true_orphans() {
        let directory = tempfile::tempdir().unwrap();
        let provider_root = directory.path().join("providers");
        fs::create_dir_all(&provider_root).unwrap();
        let path = provider_root.join("provider-a.json");
        let backend = MemoryBackend::default();
        let original = profile("sk-original");
        write_provider_profile_with_backend(&path, &original, &backend).unwrap();
        let original_value = crate::storage::read_json(&path).unwrap();
        let original_ref = credential_ref_from_value(&original_value).unwrap().unwrap();

        write_provider_profile_with_backend(&path, &profile("sk-current"), &backend).unwrap();
        let restore_payload = directory
            .path()
            .join("restore-points-v1/rp-v1-test/files")
            .join(ACTIVE_PROVIDER_RESTORE_PAYLOAD);
        fs::create_dir_all(restore_payload.parent().unwrap()).unwrap();
        write_json_atomic(&restore_payload, &original_value).unwrap();
        fs::remove_file(provider_root.join(PROVIDER_CREDENTIAL_INDEX_FILE)).unwrap();

        let retained =
            prune_orphaned_provider_credentials_with_backend(&provider_root, &backend).unwrap();
        assert_eq!(retained.indexed, 2);
        assert_eq!(retained.rooted, 2);
        assert_eq!(retained.deleted, 0);

        fs::remove_dir_all(directory.path().join("restore-points-v1")).unwrap();
        let pruned =
            prune_orphaned_provider_credentials_with_backend(&provider_root, &backend).unwrap();
        assert_eq!(pruned.indexed, 2);
        assert_eq!(pruned.rooted, 1);
        assert_eq!(pruned.deleted, 1);
        assert!(!backend.values.lock().unwrap().contains_key(&original_ref));
    }

    #[test]
    fn corrupt_restore_payload_stops_pruning_before_any_delete() {
        let directory = tempfile::tempdir().unwrap();
        let provider_root = directory.path().join("providers");
        fs::create_dir_all(&provider_root).unwrap();
        let path = provider_root.join("provider-a.json");
        let backend = MemoryBackend::default();
        write_provider_profile_with_backend(&path, &profile("sk-original"), &backend).unwrap();
        write_provider_profile_with_backend(&path, &profile("sk-current"), &backend).unwrap();
        let restore_payload = directory
            .path()
            .join("restore-points-v1/rp-v1-test/files")
            .join(ACTIVE_PROVIDER_RESTORE_PAYLOAD);
        fs::create_dir_all(restore_payload.parent().unwrap()).unwrap();
        fs::write(&restore_payload, b"not-json").unwrap();

        assert!(
            prune_orphaned_provider_credentials_with_backend(&provider_root, &backend)
                .unwrap_err()
                .contains("restore-point credential reference is invalid")
        );
        assert_eq!(backend.values.lock().unwrap().len(), 2);
    }
}
