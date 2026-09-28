fn encode_archive(payload: &AccountArchivePayload, passphrase: &[u8]) -> Result<Vec<u8>, String> {
    let json = serde_json::to_vec(payload).map_err(|error| error.to_string())?;
    let compressed = gzip(&json)?;
    let encrypted = encrypt_payload(&compressed, passphrase)?;

    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file(ARCHIVE_PAYLOAD_FILE, options)
        .map_err(|error| format!("Failed to create archive payload: {error}"))?;
    zip.write_all(&encrypted)
        .map_err(|error| format!("Failed to write archive payload: {error}"))?;
    let cursor = zip
        .finish()
        .map_err(|error| format!("Failed to finalize archive: {error}"))?;
    Ok(cursor.into_inner())
}

fn decode_archive(path: &Path, passphrase: &[u8]) -> Result<AccountArchivePayload, String> {
    let file =
        File::open(path).map_err(|error| format!("Failed to open {}: {error}", path.display()))?;
    let mut zip = ZipArchive::new(file)
        .map_err(|error| format!("The selected file is not a valid .cs archive: {error}"))?;
    let mut encrypted = Vec::new();
    zip.by_name(ARCHIVE_PAYLOAD_FILE)
        .map_err(|_| "The selected archive is missing its encrypted account payload".to_string())?
        .read_to_end(&mut encrypted)
        .map_err(|error| format!("Failed to read archive payload: {error}"))?;
    let compressed = decrypt_payload(&encrypted, passphrase)?;
    let json = gunzip(&compressed)?;
    let payload: AccountArchivePayload = serde_json::from_slice(&json)
        .map_err(|error| format!("Account archive payload is invalid: {error}"))?;
    Ok(payload)
}

fn gzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(bytes)
        .map_err(|error| format!("Failed to compress account archive: {error}"))?;
    encoder
        .finish()
        .map_err(|error| format!("Failed to finish account archive compression: {error}"))
}

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = GzDecoder::new(bytes);
    let mut decoded = Vec::new();
    decoder
        .read_to_end(&mut decoded)
        .map_err(|error| format!("Failed to decompress account archive: {error}"))?;
    Ok(decoded)
}

fn encrypt_payload(bytes: &[u8], passphrase: &[u8]) -> Result<Vec<u8>, String> {
    let salt: [u8; ARCHIVE_SALT_LENGTH] = rand::random();
    let mut key = pbkdf2_hmac_sha256(passphrase, &salt, ARCHIVE_KDF_ITERATIONS)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|error| format!("Failed to initialize account archive encryption: {error}"))?;
    let nonce_bytes: [u8; NONCE_LENGTH] = rand::random();
    let nonce = Nonce::from_slice(&nonce_bytes);
    let mut header = Vec::with_capacity(
        ARCHIVE_MAGIC.len() + std::mem::size_of::<u32>() + ARCHIVE_SALT_LENGTH + NONCE_LENGTH,
    );
    header.extend_from_slice(ARCHIVE_MAGIC);
    header.extend_from_slice(&ARCHIVE_KDF_ITERATIONS.to_be_bytes());
    header.extend_from_slice(&salt);
    header.extend_from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: bytes,
                aad: &header,
            },
        )
        .map_err(|_| "Failed to encrypt account archive payload".to_string());
    key.fill(0);
    let ciphertext = ciphertext?;
    let mut output = Vec::with_capacity(header.len() + ciphertext.len());
    output.extend_from_slice(&header);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

fn decrypt_payload(bytes: &[u8], passphrase: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.starts_with(LEGACY_ARCHIVE_MAGIC) {
        return decrypt_legacy_payload(bytes);
    }
    let header_length = ARCHIVE_MAGIC.len()
        + std::mem::size_of::<u32>()
        + ARCHIVE_SALT_LENGTH
        + NONCE_LENGTH;
    if bytes.len() <= header_length {
        return Err("The encrypted account archive payload is incomplete".to_string());
    }
    if &bytes[..ARCHIVE_MAGIC.len()] != ARCHIVE_MAGIC {
        return Err("The selected file is not a QuotaHorizon account archive".to_string());
    }
    let iteration_start = ARCHIVE_MAGIC.len();
    let iteration_end = iteration_start + std::mem::size_of::<u32>();
    let iterations = u32::from_be_bytes(
        bytes[iteration_start..iteration_end]
            .try_into()
            .map_err(|_| "The account archive KDF header is invalid".to_string())?,
    );
    if !(MIN_ARCHIVE_KDF_ITERATIONS..=MAX_ARCHIVE_KDF_ITERATIONS).contains(&iterations) {
        return Err("The account archive KDF work factor is invalid".to_string());
    }
    let salt_start = iteration_end;
    let salt_end = salt_start + ARCHIVE_SALT_LENGTH;
    let nonce_start = salt_end;
    let nonce_end = nonce_start + NONCE_LENGTH;
    let nonce = Nonce::from_slice(&bytes[nonce_start..nonce_end]);
    let ciphertext = &bytes[nonce_end..];
    let mut key = pbkdf2_hmac_sha256(passphrase, &bytes[salt_start..salt_end], iterations)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|error| format!("Failed to initialize account archive encryption: {error}"))?;
    let decrypted = cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad: &bytes[..nonce_end],
            },
        )
        .map_err(|_| "Backup passphrase is incorrect or the archive is damaged".to_string());
    key.fill(0);
    decrypted
}

fn decrypt_legacy_payload(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() <= LEGACY_ARCHIVE_MAGIC.len() + NONCE_LENGTH {
        return Err("The legacy account archive payload is incomplete".to_string());
    }
    let nonce_start = LEGACY_ARCHIVE_MAGIC.len();
    let nonce_end = nonce_start + NONCE_LENGTH;
    let cipher = Aes256Gcm::new_from_slice(&LEGACY_ARCHIVE_KEY)
        .map_err(|error| format!("Failed to initialize legacy archive decryption: {error}"))?;
    cipher
        .decrypt(
            Nonce::from_slice(&bytes[nonce_start..nonce_end]),
            Payload {
                msg: &bytes[nonce_end..],
                aad: LEGACY_ARCHIVE_MAGIC,
            },
        )
        .map_err(|_| "The legacy account archive is damaged".to_string())
}

fn pbkdf2_hmac_sha256(
    passphrase: &[u8],
    salt: &[u8],
    iterations: u32,
) -> Result<[u8; 32], String> {
    if passphrase.is_empty() || salt.is_empty() || iterations == 0 {
        return Err("Account archive KDF input is invalid".to_string());
    }
    let mut first_input = Vec::with_capacity(salt.len() + 4);
    first_input.extend_from_slice(salt);
    first_input.extend_from_slice(&1_u32.to_be_bytes());
    let mut current = hmac_sha256(passphrase, &first_input);
    first_input.fill(0);
    let mut output = current;
    for _ in 1..iterations {
        current = hmac_sha256(passphrase, &current);
        for (target, value) in output.iter_mut().zip(current.iter()) {
            *target ^= *value;
        }
    }
    current.fill(0);
    Ok(output)
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_BYTES: usize = 64;
    let mut key_block = [0_u8; BLOCK_BYTES];
    if key.len() > BLOCK_BYTES {
        key_block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; BLOCK_BYTES];
    let mut outer_pad = [0x5c_u8; BLOCK_BYTES];
    for index in 0..BLOCK_BYTES {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    let digest: [u8; 32] = outer.finalize().into();
    key_block.fill(0);
    inner_pad.fill(0);
    outer_pad.fill(0);
    digest
}
