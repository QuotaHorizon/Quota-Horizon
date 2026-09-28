-- Durable installation-key rotation intent. This table contains only opaque
-- operation/key identifiers, revisions, checkpoints, reason codes, and UTC
-- timestamps; secret key material, fingerprints, record refs, and paths are
-- forbidden.
CREATE TABLE vault_key_rotations (
    rotation_id TEXT PRIMARY KEY NOT NULL CHECK (
        length(rotation_id) = 58
        AND substr(rotation_id, 1, 22) = 'vault-key-rotation:v1:'
        AND substr(rotation_id, 23, 8) NOT GLOB '*[^0-9a-f]*'
        AND substr(rotation_id, 31, 1) = '-'
        AND substr(rotation_id, 32, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(rotation_id, 36, 1) = '-'
        AND substr(rotation_id, 37, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(rotation_id, 41, 1) = '-'
        AND substr(rotation_id, 42, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(rotation_id, 46, 1) = '-'
        AND substr(rotation_id, 47, 12) NOT GLOB '*[^0-9a-f]*'
    ),
    status TEXT NOT NULL CHECK (
        status IN ('in_progress', 'rolling_back', 'succeeded', 'compensated', 'needs_review')
    ),
    checkpoint TEXT NOT NULL CHECK (
        checkpoint IN (
            'prepared',
            'key_ring_started',
            'accounts_migrated',
            'dependencies_cleared',
            'predecessor_retired',
            'rollback_started',
            'accounts_restored',
            'new_key_retired'
        )
    ),
    revision INTEGER NOT NULL CHECK (revision > 0),
    source_key_id TEXT NOT NULL CHECK (
        length(source_key_id) = 36
        AND substr(source_key_id, 1, 8) NOT GLOB '*[^0-9a-f]*'
        AND substr(source_key_id, 9, 1) = '-'
        AND substr(source_key_id, 10, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(source_key_id, 14, 1) = '-'
        AND substr(source_key_id, 15, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(source_key_id, 19, 1) = '-'
        AND substr(source_key_id, 20, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(source_key_id, 24, 1) = '-'
        AND substr(source_key_id, 25, 12) NOT GLOB '*[^0-9a-f]*'
    ),
    target_key_id TEXT NOT NULL CHECK (
        length(target_key_id) = 36
        AND substr(target_key_id, 1, 8) NOT GLOB '*[^0-9a-f]*'
        AND substr(target_key_id, 9, 1) = '-'
        AND substr(target_key_id, 10, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(target_key_id, 14, 1) = '-'
        AND substr(target_key_id, 15, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(target_key_id, 19, 1) = '-'
        AND substr(target_key_id, 20, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(target_key_id, 24, 1) = '-'
        AND substr(target_key_id, 25, 12) NOT GLOB '*[^0-9a-f]*'
    ),
    expected_key_ring_revision INTEGER NOT NULL CHECK (expected_key_ring_revision > 0),
    last_error_code TEXT CHECK (
        last_error_code IS NULL
        OR (
            length(last_error_code) BETWEEN 1 AND 128
            AND last_error_code NOT GLOB '*[^a-z0-9_.:-]*'
        )
    ),
    created_at TEXT NOT NULL CHECK (
        julianday(created_at) IS NOT NULL
        AND (substr(created_at, -1) = 'Z' OR substr(created_at, -6) = '+00:00')
    ),
    updated_at TEXT NOT NULL CHECK (
        julianday(updated_at) IS NOT NULL
        AND (substr(updated_at, -1) = 'Z' OR substr(updated_at, -6) = '+00:00')
    ),
    CHECK (source_key_id != target_key_id),
    CHECK (julianday(updated_at) >= julianday(created_at)),
    CHECK (
        (
            status = 'in_progress'
            AND last_error_code IS NULL
            AND checkpoint IN (
                'prepared',
                'key_ring_started',
                'accounts_migrated',
                'dependencies_cleared',
                'predecessor_retired'
            )
        )
        OR (
            status = 'rolling_back'
            AND last_error_code IS NOT NULL
            AND checkpoint IN ('rollback_started', 'accounts_restored', 'new_key_retired')
        )
        OR (
            status = 'needs_review'
            AND last_error_code IS NOT NULL
        )
        OR (
            status = 'succeeded'
            AND checkpoint = 'predecessor_retired'
            AND last_error_code IS NULL
        )
        OR (
            status = 'compensated'
            AND checkpoint = 'new_key_retired'
            AND last_error_code IS NOT NULL
        )
    )
) STRICT;

CREATE UNIQUE INDEX vault_key_rotations_one_active_idx
    ON vault_key_rotations((1))
    WHERE status IN ('in_progress', 'rolling_back', 'needs_review');

CREATE INDEX vault_key_rotations_recovery_idx
    ON vault_key_rotations(status, created_at, rotation_id);
