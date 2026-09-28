#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ProviderApiFormat, ProviderKind};
    use serde_json::json;

    const PASSPHRASE: &[u8] = b"correct horse battery staple";

    #[test]
    fn archive_keeps_payload_encrypted_inside_plain_zip() {
        let payload = AccountArchivePayload {
            format_version: 2,
            exported_at: "2026-07-04T00:00:00Z".to_string(),
            active_account_id: Some("account-1".to_string()),
            active_provider_id: Some("provider-1".to_string()),
            accounts: vec![AccountArchiveEntry {
                id: "account-1".to_string(),
                auth: json!({
                    "tokens": {
                        "access_token": "plain-secret-access-token",
                    }
                }),
                note: "plain-secret-note".to_string(),
                expires_at: "2026-12-31".to_string(),
                private_details: AccountPrivateDetails {
                    password: "plain-secret-password".to_string(),
                    phone_number: "+65 6123 4567".to_string(),
                    totp_secret: "JBSWY3DPEHPK3PXP".to_string(),
                },
                usage: UsageSummary::default(),
                auto_switch_priority: 0,
                auto_switch_threshold: 0.0,
                last_modified_at: Some("2026-07-04T00:00:00Z".to_string()),
            }],
            providers: vec![ProviderSyncPayload {
                id: "provider-1".to_string(),
                kind: ProviderKind::Custom,
                name: "Gateway".to_string(),
                group: String::new(),
                base_url: "https://gateway.example.com/v1".to_string(),
                api_key: "plain-secret-provider-key".to_string(),
                model: "gpt-4.1".to_string(),
                models: vec!["gpt-4.1".to_string()],
                model_reasoning_efforts: Default::default(),
                model_context_windows: Default::default(),
                model_api_formats: Default::default(),
                image_input_models: vec!["gpt-4.1".to_string()],
                context_window: None,
                model_selection_controlled_by_codex: false,
                api_format: ProviderApiFormat::OpenaiResponses,
                balance_platform: None,
                balance_query_url: None,
                balance_query_token: None,
                wallet_query_url: None,
                wallet_query_token: None,
                wallet_username: None,
                wallet_password: None,
                last_modified_at: "2026-07-04T00:00:00Z".to_string(),
                field_modified_at: Default::default(),
            }],
        };

        let archive = encode_archive(&payload, PASSPHRASE).expect("archive should encode");
        let archive_text = String::from_utf8_lossy(&archive);
        assert!(archive_text.contains(ARCHIVE_PAYLOAD_FILE));
        assert!(!archive_text.contains("plain-secret-access-token"));
        assert!(!archive_text.contains("plain-secret-provider-key"));
        assert!(!archive_text.contains("plain-secret-note"));
        assert!(!archive_text.contains("plain-secret-password"));

        let mut zip = ZipArchive::new(Cursor::new(archive)).expect("archive should be a plain zip");
        let mut encrypted = Vec::new();
        zip.by_name(ARCHIVE_PAYLOAD_FILE)
            .expect("zip should contain encrypted payload file")
            .read_to_end(&mut encrypted)
            .expect("payload should read");
        assert!(!String::from_utf8_lossy(&encrypted).contains("plain-secret-note"));
        assert!(!String::from_utf8_lossy(&encrypted).contains("plain-secret-provider-key"));
        assert!(!String::from_utf8_lossy(&encrypted).contains("plain-secret-password"));

        assert!(encrypted.starts_with(ARCHIVE_MAGIC));
        assert!(!encrypted.starts_with(LEGACY_ARCHIVE_MAGIC));
        let compressed =
            decrypt_payload(&encrypted, PASSPHRASE).expect("payload should decrypt");
        let json = gunzip(&compressed).expect("payload should decompress");
        let restored: AccountArchivePayload =
            serde_json::from_slice(&json).expect("payload should decode");
        assert_eq!(restored.accounts[0].note, "plain-secret-note");
        assert_eq!(
            restored.accounts[0].private_details.password,
            "plain-secret-password"
        );
        assert_eq!(restored.providers[0].api_key, "plain-secret-provider-key");
    }

    #[test]
    fn archive_rejects_the_wrong_passphrase_without_leaking_payload() {
        let payload = AccountArchivePayload {
            format_version: 2,
            exported_at: "2026-08-31T00:00:00Z".to_string(),
            active_account_id: None,
            active_provider_id: None,
            accounts: Vec::new(),
            providers: Vec::new(),
        };
        let archive = encode_archive(&payload, b"correct horse battery staple").unwrap();
        let mut zip = ZipArchive::new(Cursor::new(archive)).unwrap();
        let mut encrypted = Vec::new();
        zip.by_name(ARCHIVE_PAYLOAD_FILE)
            .unwrap()
            .read_to_end(&mut encrypted)
            .unwrap();

        assert_eq!(
            decrypt_payload(&encrypted, b"incorrect archive passphrase").unwrap_err(),
            "Backup passphrase is incorrect or the archive is damaged"
        );

        let mut tampered_salt = encrypted.clone();
        tampered_salt[ARCHIVE_MAGIC.len() + std::mem::size_of::<u32>()] ^= 0x01;
        assert_eq!(
            decrypt_payload(&tampered_salt, b"correct horse battery staple").unwrap_err(),
            "Backup passphrase is incorrect or the archive is damaged"
        );

        let mut invalid_work_factor = encrypted;
        let start = ARCHIVE_MAGIC.len();
        invalid_work_factor[start..start + std::mem::size_of::<u32>()]
            .copy_from_slice(&1_u32.to_be_bytes());
        assert_eq!(
            decrypt_payload(
                &invalid_work_factor,
                b"correct horse battery staple"
            )
            .unwrap_err(),
            "The account archive KDF work factor is invalid"
        );
    }

    #[test]
    fn legacy_fixed_key_archives_remain_importable_but_are_never_exported() {
        let compressed = gzip(br#"{"formatVersion":1}"#).unwrap();
        let nonce_bytes = [7_u8; NONCE_LENGTH];
        let cipher = Aes256Gcm::new_from_slice(&LEGACY_ARCHIVE_KEY).unwrap();
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: &compressed,
                    aad: LEGACY_ARCHIVE_MAGIC,
                },
            )
            .unwrap();
        let mut legacy = Vec::new();
        legacy.extend_from_slice(LEGACY_ARCHIVE_MAGIC);
        legacy.extend_from_slice(&nonce_bytes);
        legacy.extend_from_slice(&ciphertext);

        assert_eq!(
            decrypt_payload(&legacy, b"this passphrase is ignored for legacy imports").unwrap(),
            compressed
        );
        assert!(!encrypt_payload(b"payload", PASSPHRASE)
            .unwrap()
            .starts_with(LEGACY_ARCHIVE_MAGIC));
    }

    #[test]
    fn pbkdf2_matches_the_published_hmac_sha256_vector() {
        let derived = pbkdf2_hmac_sha256(b"password", b"salt", 1).unwrap();
        let rendered = derived
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            rendered,
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        let derived = pbkdf2_hmac_sha256(b"password", b"salt", 2).unwrap();
        let rendered = derived
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            rendered,
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
    }

    #[test]
    fn short_archive_passphrases_are_rejected() {
        assert!(validate_archive_passphrase("short").is_err());
        assert!(validate_archive_passphrase("twelve-chars").is_ok());
        assert!(validate_archive_passphrase(&"x".repeat(MAX_ARCHIVE_PASSPHRASE_BYTES + 1)).is_err());
        assert!(validate_archive_import_passphrase("").is_ok());
        assert!(
            validate_archive_import_passphrase(&"x".repeat(MAX_ARCHIVE_PASSPHRASE_BYTES + 1))
                .is_err()
        );
    }
}
