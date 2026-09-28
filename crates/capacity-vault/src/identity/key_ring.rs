use std::fmt;

use super::{
    INSTALLATION_KEY_FORMAT, INSTALLATION_SECRET_BYTES, IdentityError, InstallationKey,
    InstallationKeyRing, KEY_ID_BYTES, parse_installation_key,
};

const KEY_RING_FORMAT_V2: &[u8] = b"capacity-installation-key-ring-v2\n";
const KEY_RING_REVISION_BYTES: usize = 8;
const KEY_RING_COUNT_BYTES: usize = 1;
const ENCODED_KEY_BYTES: usize = KEY_ID_BYTES + INSTALLATION_SECRET_BYTES;
const MAX_KEY_COUNT: usize = InstallationKeyRing::MAX_VERIFICATION_KEYS + 1;

pub(super) const MAX_KEY_RING_RECORD_BYTES: u64 = encoded_key_ring_bytes(MAX_KEY_COUNT);

pub(super) const fn encoded_key_ring_bytes(key_count: usize) -> u64 {
    (KEY_RING_FORMAT_V2.len()
        + KEY_RING_REVISION_BYTES
        + KEY_RING_COUNT_BYTES
        + key_count * ENCODED_KEY_BYTES) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallationKeyStorageVersion {
    SingleKeyV1,
    KeyRingV2,
}

/// Non-secret optimistic concurrency token. Debug deliberately withholds the
/// active key identifier so logs keep the same redaction boundary as keys.
#[derive(Clone, PartialEq, Eq)]
pub struct InstallationKeyRingRevision {
    revision: u64,
    active_key_id: String,
}

impl InstallationKeyRingRevision {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn active_key_id(&self) -> &str {
        &self.active_key_id
    }
}

impl fmt::Debug for InstallationKeyRingRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstallationKeyRingRevision")
            .field("revision", &self.revision)
            .field("active_key_id", &"<redacted>")
            .finish()
    }
}

/// Versioned, non-clone key-ring payload. A legacy single-key record is
/// represented as revision 1 until the first guarded v2 write.
pub struct InstallationKeyRingRecord {
    revision: u64,
    storage_version: InstallationKeyStorageVersion,
    ring: InstallationKeyRing,
}

impl InstallationKeyRingRecord {
    pub fn initial(active: InstallationKey) -> Self {
        Self {
            revision: 1,
            storage_version: InstallationKeyStorageVersion::KeyRingV2,
            ring: InstallationKeyRing::new(active),
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn storage_version(&self) -> InstallationKeyStorageVersion {
        self.storage_version
    }

    pub fn active_key_id(&self) -> &str {
        self.ring.active_key_id()
    }

    pub fn verification_key_ids(&self) -> impl ExactSizeIterator<Item = &str> {
        self.ring.verification_key_ids()
    }

    pub fn revision_token(&self) -> InstallationKeyRingRevision {
        InstallationKeyRingRevision {
            revision: self.revision,
            active_key_id: self.active_key_id().to_owned(),
        }
    }

    pub fn start_rotation(self, new_active: InstallationKey) -> Result<Self, IdentityError> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(IdentityError::InvalidKeyRing)?;
        let ring = self.ring.start_rotation(new_active)?;
        Ok(Self {
            revision,
            storage_version: InstallationKeyStorageVersion::KeyRingV2,
            ring,
        })
    }

    pub fn retire_verification_key(self, key_id: &str) -> Result<Self, IdentityError> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(IdentityError::InvalidKeyRing)?;
        let ring = self.ring.retire_verification_key(key_id)?;
        Ok(Self {
            revision,
            storage_version: InstallationKeyStorageVersion::KeyRingV2,
            ring,
        })
    }

    pub fn promote_verification_key(self, key_id: &str) -> Result<Self, IdentityError> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(IdentityError::InvalidKeyRing)?;
        let ring = self.ring.promote_verification_key(key_id)?;
        Ok(Self {
            revision,
            storage_version: InstallationKeyStorageVersion::KeyRingV2,
            ring,
        })
    }

    pub fn into_key_ring(self) -> InstallationKeyRing {
        self.ring
    }

    pub(super) fn legacy(active: InstallationKey) -> Self {
        Self {
            revision: 1,
            storage_version: InstallationKeyStorageVersion::SingleKeyV1,
            ring: InstallationKeyRing::new(active),
        }
    }

    pub(super) fn into_active_key(self) -> InstallationKey {
        self.ring.active
    }

    pub(super) fn matches_revision(&self, expected: &InstallationKeyRingRevision) -> bool {
        self.revision == expected.revision && self.active_key_id() == expected.active_key_id
    }

    pub(super) fn is_next_revision_of(&self, expected: &InstallationKeyRingRevision) -> bool {
        expected
            .revision
            .checked_add(1)
            .is_some_and(|revision| revision == self.revision)
            && self.storage_version == InstallationKeyStorageVersion::KeyRingV2
    }

    pub(super) fn is_valid_successor_of(&self, current: &Self) -> bool {
        current
            .revision
            .checked_add(1)
            .is_some_and(|revision| revision == self.revision)
            && self.storage_version == InstallationKeyStorageVersion::KeyRingV2
            && self.ring.is_valid_successor_of(&current.ring)
    }

    pub(super) fn same_material(&self, other: &Self) -> bool {
        self.revision == other.revision
            && self.storage_version == other.storage_version
            && self.ring.same_material(&other.ring)
    }
}

impl fmt::Debug for InstallationKeyRingRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstallationKeyRingRecord")
            .field("revision", &self.revision)
            .field("storage_version", &self.storage_version)
            .field("active", &"<redacted>")
            .field("verification_key_count", &self.ring.verification_keys.len())
            .finish()
    }
}

pub(super) fn encode_key_ring_record(record: &InstallationKeyRingRecord) -> Vec<u8> {
    let key_count = record.ring.verification_keys.len() + 1;
    let mut bytes = Vec::with_capacity(
        usize::try_from(encoded_key_ring_bytes(key_count)).expect("key-ring bytes are bounded"),
    );
    bytes.extend_from_slice(KEY_RING_FORMAT_V2);
    bytes.extend_from_slice(&record.revision.to_be_bytes());
    bytes.push(u8::try_from(key_count).expect("key-ring count is bounded"));
    append_key(&mut bytes, &record.ring.active);
    for key in &record.ring.verification_keys {
        append_key(&mut bytes, key);
    }
    bytes
}

pub(super) fn parse_key_ring_record(
    mut bytes: Vec<u8>,
) -> Result<InstallationKeyRingRecord, IdentityError> {
    if bytes.starts_with(INSTALLATION_KEY_FORMAT) {
        return parse_installation_key(bytes).map(InstallationKeyRingRecord::legacy);
    }
    let result = parse_v2_key_ring(&bytes);
    bytes.fill(0);
    result
}

fn parse_v2_key_ring(bytes: &[u8]) -> Result<InstallationKeyRingRecord, IdentityError> {
    let header_bytes = KEY_RING_FORMAT_V2.len() + KEY_RING_REVISION_BYTES + KEY_RING_COUNT_BYTES;
    if bytes.len() < header_bytes || !bytes.starts_with(KEY_RING_FORMAT_V2) {
        return Err(IdentityError::KeyRecordCorrupt);
    }
    let revision_start = KEY_RING_FORMAT_V2.len();
    let revision_end = revision_start + KEY_RING_REVISION_BYTES;
    let revision = u64::from_be_bytes(
        bytes[revision_start..revision_end]
            .try_into()
            .map_err(|_| IdentityError::KeyRecordCorrupt)?,
    );
    let key_count = usize::from(bytes[revision_end]);
    if revision == 0 || key_count == 0 || key_count > MAX_KEY_COUNT {
        return Err(IdentityError::KeyRecordCorrupt);
    }
    let expected_bytes = header_bytes
        .checked_add(
            key_count
                .checked_mul(ENCODED_KEY_BYTES)
                .ok_or(IdentityError::KeyRecordCorrupt)?,
        )
        .ok_or(IdentityError::KeyRecordCorrupt)?;
    if bytes.len() != expected_bytes {
        return Err(IdentityError::KeyRecordCorrupt);
    }

    let mut keys = Vec::with_capacity(key_count);
    for chunk in bytes[header_bytes..].chunks_exact(ENCODED_KEY_BYTES) {
        let key_id = std::str::from_utf8(&chunk[..KEY_ID_BYTES])
            .map_err(|_| IdentityError::KeyRecordCorrupt)?;
        let mut secret = [0_u8; INSTALLATION_SECRET_BYTES];
        secret.copy_from_slice(&chunk[KEY_ID_BYTES..]);
        let key = InstallationKey::from_parts(key_id, secret)
            .map_err(|_| IdentityError::KeyRecordCorrupt);
        secret.fill(0);
        keys.push(key?);
    }
    let mut keys = keys.into_iter();
    let active = keys.next().ok_or(IdentityError::KeyRecordCorrupt)?;
    let verification_keys = keys.collect();
    let ring = InstallationKeyRing::with_verification_keys(active, verification_keys)
        .map_err(|_| IdentityError::KeyRecordCorrupt)?;
    Ok(InstallationKeyRingRecord {
        revision,
        storage_version: InstallationKeyStorageVersion::KeyRingV2,
        ring,
    })
}

fn append_key(bytes: &mut Vec<u8>, key: &InstallationKey) {
    bytes.extend_from_slice(key.key_id().as_bytes());
    bytes.extend_from_slice(key.secret());
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_ID: &str = "018f47a2-8a71-4f4a-9c35-1f4234a73801";
    const KEY_ID_2: &str = "018f47a2-8a71-4f4a-9c35-1f4234a73802";

    fn key(id: &str, byte: u8) -> InstallationKey {
        InstallationKey::from_parts(id, [byte; INSTALLATION_SECRET_BYTES]).unwrap()
    }

    #[test]
    fn v2_codec_round_trips_revision_order_and_secret_material() {
        let record = InstallationKeyRingRecord::initial(key(KEY_ID, 0x11))
            .start_rotation(key(KEY_ID_2, 0x22))
            .unwrap();
        let mut encoded = encode_key_ring_record(&record);
        let decoded = parse_key_ring_record(encoded.clone()).unwrap();

        assert!(record.same_material(&decoded));
        assert_eq!(decoded.revision(), 2);
        assert_eq!(decoded.active_key_id(), KEY_ID_2);
        assert_eq!(decoded.verification_key_ids().collect::<Vec<_>>(), [KEY_ID]);
        encoded.fill(0);
    }

    #[test]
    fn v2_codec_rejects_zero_revision_duplicate_keys_and_length_drift() {
        let record = InstallationKeyRingRecord::initial(key(KEY_ID, 0x33));
        let encoded = encode_key_ring_record(&record);
        let revision_start = KEY_RING_FORMAT_V2.len();
        let count_offset = revision_start + KEY_RING_REVISION_BYTES;

        let mut zero_revision = encoded.clone();
        zero_revision[revision_start..count_offset].fill(0);
        assert!(matches!(
            parse_key_ring_record(zero_revision),
            Err(IdentityError::KeyRecordCorrupt)
        ));

        let mut duplicate = encoded.clone();
        duplicate[count_offset] = 2;
        duplicate.extend_from_slice(&encoded[count_offset + 1..]);
        assert!(matches!(
            parse_key_ring_record(duplicate),
            Err(IdentityError::KeyRecordCorrupt)
        ));

        let mut trailing = encoded;
        trailing.push(0);
        assert!(matches!(
            parse_key_ring_record(trailing),
            Err(IdentityError::KeyRecordCorrupt)
        ));
    }

    #[test]
    fn rotation_can_promote_the_predecessor_for_a_guarded_rollback() {
        let rotating = InstallationKeyRingRecord::initial(key(KEY_ID, 0x44))
            .start_rotation(key(KEY_ID_2, 0x55))
            .unwrap();
        let rolled_back = rotating.promote_verification_key(KEY_ID).unwrap();

        assert_eq!(rolled_back.revision(), 3);
        assert_eq!(rolled_back.active_key_id(), KEY_ID);
        assert_eq!(
            rolled_back.verification_key_ids().collect::<Vec<_>>(),
            [KEY_ID_2]
        );
    }
}
