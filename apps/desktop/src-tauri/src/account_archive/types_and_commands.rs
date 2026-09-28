use std::{
    fs::{self, File},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};

use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use chrono::Utc;
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{Emitter, Runtime};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

use crate::{
    auth::{account_fields, canonicalize_chatgpt_auth, validate_auth},
    models::{AccountPrivateDetails, ProviderProfile, ProviderSyncPayload, UsageSummary},
    storage::{
        account_private_details_path, auto_switch_priority_path, auto_switch_threshold_path,
        expiration_path, load_account_private_details, load_auto_switch_priority,
        load_auto_switch_threshold, load_expiration, load_note,
        load_or_init_last_modified, load_usage, managed_auth_path, note_path, parse_last_modified,
        read_json, read_state, resolve_paths, save_account_last_modified,
        save_account_private_details, save_auto_switch_priority, save_auto_switch_threshold,
        save_expiration, save_note,
        save_usage, usage_path, write_json_if_changed, write_managed_auth_if_changed, write_state,
    },
};

const ARCHIVE_PAYLOAD_FILE: &str = "accounts.payload";
const ARCHIVE_MAGIC: &[u8] = b"QHARCHIVE2";
const LEGACY_ARCHIVE_MAGIC: &[u8] = b"CSARCHIVE1";
const LEGACY_ARCHIVE_KEY: [u8; 32] = *b"CodexSwitchLocalBackupKeyV1!2026";
const ARCHIVE_SALT_LENGTH: usize = 16;
const NONCE_LENGTH: usize = 12;
const ARCHIVE_KDF_ITERATIONS: u32 = 210_000;
const MIN_ARCHIVE_KDF_ITERATIONS: u32 = 100_000;
const MAX_ARCHIVE_KDF_ITERATIONS: u32 = 1_000_000;
const MIN_ARCHIVE_PASSPHRASE_CHARS: usize = 12;
const MAX_ARCHIVE_PASSPHRASE_BYTES: usize = 4_096;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountArchivePayload {
    format_version: u16,
    exported_at: String,
    active_account_id: Option<String>,
    #[serde(default)]
    active_provider_id: Option<String>,
    accounts: Vec<AccountArchiveEntry>,
    #[serde(default)]
    providers: Vec<ProviderSyncPayload>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountArchiveEntry {
    id: String,
    auth: Value,
    note: String,
    expires_at: String,
    #[serde(default)]
    private_details: AccountPrivateDetails,
    usage: UsageSummary,
    #[serde(default)]
    auto_switch_priority: i32,
    #[serde(default)]
    auto_switch_threshold: f64,
    #[serde(default)]
    last_modified_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountArchiveImportResult {
    imported: usize,
    account_ids: Vec<String>,
    active_account_id: Option<String>,
    providers_imported: usize,
    provider_ids: Vec<String>,
    active_provider_id: Option<String>,
}

#[tauri::command]
pub(crate) fn export_accounts_archive<R: Runtime>(
    app: tauri::AppHandle<R>,
    path: String,
    passphrase: String,
) -> Result<String, String> {
    validate_archive_passphrase(&passphrase)?;
    let payload = collect_accounts(&app)?;
    if payload.accounts.is_empty() && payload.providers.is_empty() {
        return Err("No local accounts or providers to export".to_string());
    }

    let output_path = normalize_archive_path(Path::new(&path));
    let mut passphrase = passphrase.into_bytes();
    let archive = encode_archive(&payload, &passphrase);
    passphrase.fill(0);
    let archive = archive?;
    if let Some(parent) = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
    }
    fs::write(&output_path, archive)
        .map_err(|error| format!("Failed to write {}: {error}", output_path.display()))?;
    Ok(output_path.display().to_string())
}

#[tauri::command]
pub(crate) fn import_accounts_archive<R: Runtime>(
    app: tauri::AppHandle<R>,
    path: String,
    passphrase: String,
) -> Result<AccountArchiveImportResult, String> {
    validate_archive_import_passphrase(&passphrase)?;
    let mut passphrase = passphrase.into_bytes();
    let payload = decode_archive(Path::new(&path), &passphrase);
    passphrase.fill(0);
    let payload = payload?;
    let result = apply_archive(&app, payload)?;
    if !result.account_ids.is_empty() {
        app.emit("accounts-changed", ())
            .map_err(|error| error.to_string())?;
    }
    if !result.provider_ids.is_empty() {
        app.emit("providers-changed", ())
            .map_err(|error| error.to_string())?;
        if let Ok(paths) = resolve_paths(&app) {
            crate::providers::refresh_codex_models_for_current_target(&paths);
        }
    }
    crate::system_tray::refresh_menu(&app);
    Ok(result)
}

fn validate_archive_passphrase(passphrase: &str) -> Result<(), String> {
    if passphrase.chars().count() < MIN_ARCHIVE_PASSPHRASE_CHARS {
        Err(format!(
            "Backup passphrase must contain at least {MIN_ARCHIVE_PASSPHRASE_CHARS} characters"
        ))
    } else if passphrase.len() > MAX_ARCHIVE_PASSPHRASE_BYTES {
        Err("Backup passphrase is too long".to_string())
    } else {
        Ok(())
    }
}

fn validate_archive_import_passphrase(passphrase: &str) -> Result<(), String> {
    if passphrase.len() > MAX_ARCHIVE_PASSPHRASE_BYTES {
        Err("Backup passphrase is too long".to_string())
    } else {
        Ok(())
    }
}
