-- Durable coordination journal for multi-store account-vault mutations. This
-- table contains bounded metadata and opaque references only; protected bytes,
-- raw upstream identities, paths, and serialized payloads are forbidden.
CREATE TABLE vault_operations (
    operation_id TEXT PRIMARY KEY NOT NULL CHECK (
        length(operation_id) = 55
        AND substr(operation_id, 1, 19) = 'vault-operation:v1:'
        AND substr(operation_id, 20, 8) NOT GLOB '*[^0-9a-f]*'
        AND substr(operation_id, 28, 1) = '-'
        AND substr(operation_id, 29, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(operation_id, 33, 1) = '-'
        AND substr(operation_id, 34, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(operation_id, 38, 1) = '-'
        AND substr(operation_id, 39, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(operation_id, 43, 1) = '-'
        AND substr(operation_id, 44, 12) NOT GLOB '*[^0-9a-f]*'
    ),
    kind TEXT NOT NULL CHECK (kind IN ('register_account', 'forget_account')),
    status TEXT NOT NULL CHECK (
        status IN ('in_progress', 'succeeded', 'compensated', 'needs_review')
    ),
    checkpoint TEXT NOT NULL CHECK (
        checkpoint IN (
            'prepared',
            'record_ready',
            'record_quarantined',
            'metadata_committed',
            'metadata_removed',
            'record_restored'
        )
    ),
    revision INTEGER NOT NULL CHECK (revision > 0),
    account_id TEXT CHECK (
        account_id IS NULL
        OR (
            length(account_id) = 47
            AND substr(account_id, 1, 11) = 'account:v1:'
            AND substr(account_id, 12, 8) NOT GLOB '*[^0-9a-f]*'
            AND substr(account_id, 20, 1) = '-'
            AND substr(account_id, 21, 4) NOT GLOB '*[^0-9a-f]*'
            AND substr(account_id, 25, 1) = '-'
            AND substr(account_id, 26, 4) NOT GLOB '*[^0-9a-f]*'
            AND substr(account_id, 30, 1) = '-'
            AND substr(account_id, 31, 4) NOT GLOB '*[^0-9a-f]*'
            AND substr(account_id, 35, 1) = '-'
            AND substr(account_id, 36, 12) NOT GLOB '*[^0-9a-f]*'
        )
    ),
    account_fingerprint TEXT NOT NULL CHECK (
        length(account_fingerprint) = 116
        AND substr(account_fingerprint, 1, 15) = 'hmac-sha256:v1:'
        AND substr(account_fingerprint, 52, 1) = ':'
        AND substr(account_fingerprint, 16, 8) NOT GLOB '*[^0-9a-f]*'
        AND substr(account_fingerprint, 24, 1) = '-'
        AND substr(account_fingerprint, 25, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(account_fingerprint, 29, 1) = '-'
        AND substr(account_fingerprint, 30, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(account_fingerprint, 34, 1) = '-'
        AND substr(account_fingerprint, 35, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(account_fingerprint, 39, 1) = '-'
        AND substr(account_fingerprint, 40, 12) NOT GLOB '*[^0-9a-f]*'
        AND substr(account_fingerprint, 53) NOT GLOB '*[^0-9a-f]*'
    ),
    protected_record_ref TEXT NOT NULL CHECK (
        length(protected_record_ref) = 52
        AND substr(protected_record_ref, 1, 16) = 'vault-record:v1:'
        AND substr(protected_record_ref, 17, 8) NOT GLOB '*[^0-9a-f]*'
        AND substr(protected_record_ref, 25, 1) = '-'
        AND substr(protected_record_ref, 26, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(protected_record_ref, 30, 1) = '-'
        AND substr(protected_record_ref, 31, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(protected_record_ref, 35, 1) = '-'
        AND substr(protected_record_ref, 36, 4) NOT GLOB '*[^0-9a-f]*'
        AND substr(protected_record_ref, 40, 1) = '-'
        AND substr(protected_record_ref, 41, 12) NOT GLOB '*[^0-9a-f]*'
    ),
    display_name TEXT NOT NULL CHECK (length(trim(display_name)) BETWEEN 1 AND 128),
    auth_mode TEXT NOT NULL CHECK (auth_mode IN ('chatgpt', 'api_key', 'unknown')),
    source TEXT NOT NULL CHECK (source IN ('current_runtime', 'manual_chatgpt', 'manual_api')),
    lifecycle TEXT NOT NULL CHECK (
        lifecycle IN ('ready', 'record_unavailable', 'historical_only', 'needs_review')
    ),
    provider_id TEXT CHECK (provider_id IS NULL OR length(trim(provider_id)) BETWEEN 1 AND 128),
    model TEXT CHECK (model IS NULL OR length(trim(model)) BETWEEN 1 AND 256),
    expected_account_revision INTEGER CHECK (
        expected_account_revision IS NULL OR expected_account_revision > 0
    ),
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
    CHECK (julianday(updated_at) >= julianday(created_at)),
    CHECK (
        source = 'current_runtime'
        OR (source = 'manual_chatgpt' AND auth_mode = 'chatgpt')
        OR (source = 'manual_api' AND auth_mode = 'api_key')
    ),
    CHECK (
        (
            kind = 'register_account'
            AND expected_account_revision IS NULL
            AND checkpoint IN ('prepared', 'record_ready', 'record_quarantined', 'metadata_committed')
            AND (
                (checkpoint = 'metadata_committed' AND account_id IS NOT NULL)
                OR (checkpoint != 'metadata_committed' AND account_id IS NULL)
            )
        )
        OR (
            kind = 'forget_account'
            AND expected_account_revision IS NOT NULL
            AND account_id IS NOT NULL
            AND checkpoint IN ('prepared', 'record_quarantined', 'metadata_removed', 'record_restored')
        )
    ),
    CHECK (
        (status = 'in_progress' AND last_error_code IS NULL)
        OR (status = 'needs_review' AND last_error_code IS NOT NULL)
        OR (
            status = 'succeeded'
            AND last_error_code IS NULL
            AND (
                (kind = 'register_account' AND checkpoint = 'metadata_committed')
                OR (kind = 'forget_account' AND checkpoint = 'metadata_removed')
            )
        )
        OR (
            status = 'compensated'
            AND last_error_code IS NOT NULL
            AND (
                (kind = 'register_account' AND checkpoint = 'record_quarantined')
                OR (kind = 'forget_account' AND checkpoint = 'record_restored')
            )
        )
    )
) STRICT;

CREATE UNIQUE INDEX vault_operations_active_fingerprint_idx
    ON vault_operations(account_fingerprint)
    WHERE status IN ('in_progress', 'needs_review');

CREATE UNIQUE INDEX vault_operations_active_record_ref_idx
    ON vault_operations(protected_record_ref)
    WHERE status IN ('in_progress', 'needs_review');

CREATE UNIQUE INDEX vault_operations_active_account_idx
    ON vault_operations(account_id)
    WHERE account_id IS NOT NULL AND status IN ('in_progress', 'needs_review');

CREATE INDEX vault_operations_recovery_idx
    ON vault_operations(status, created_at, operation_id);
