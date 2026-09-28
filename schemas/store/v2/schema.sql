-- Account-vault metadata only. Protected auth/config material and the raw
-- upstream identity used for HMAC derivation live behind protected_record_ref
-- in a separate vault backend and never enter this database.
CREATE TABLE vault_accounts (
    account_id TEXT PRIMARY KEY NOT NULL CHECK (
        length(account_id) = 47
        AND substr(account_id, 1, 11) = 'account:v1:'
        AND substr(account_id, 12, 36) NOT GLOB '*[^0-9a-f-]*'
        AND substr(account_id, 20, 1) = '-'
        AND substr(account_id, 25, 1) = '-'
        AND substr(account_id, 30, 1) = '-'
        AND substr(account_id, 35, 1) = '-'
    ),
    account_fingerprint TEXT NOT NULL UNIQUE CHECK (
        length(account_fingerprint) = 116
        AND substr(account_fingerprint, 1, 15) = 'hmac-sha256:v1:'
        AND substr(account_fingerprint, 52, 1) = ':'
        AND substr(account_fingerprint, 16, 36) NOT GLOB '*[^0-9a-f-]*'
        AND substr(account_fingerprint, 24, 1) = '-'
        AND substr(account_fingerprint, 29, 1) = '-'
        AND substr(account_fingerprint, 34, 1) = '-'
        AND substr(account_fingerprint, 39, 1) = '-'
        AND substr(account_fingerprint, 53) NOT GLOB '*[^0-9a-f]*'
    ),
    protected_record_ref TEXT NOT NULL UNIQUE CHECK (
        length(protected_record_ref) = 52
        AND substr(protected_record_ref, 1, 16) = 'vault-record:v1:'
        AND substr(protected_record_ref, 17, 36) NOT GLOB '*[^0-9a-f-]*'
        AND substr(protected_record_ref, 25, 1) = '-'
        AND substr(protected_record_ref, 30, 1) = '-'
        AND substr(protected_record_ref, 35, 1) = '-'
        AND substr(protected_record_ref, 40, 1) = '-'
    ),
    display_name TEXT NOT NULL CHECK (length(trim(display_name)) BETWEEN 1 AND 128),
    auth_mode TEXT NOT NULL CHECK (auth_mode IN ('chatgpt', 'api_key', 'unknown')),
    source TEXT NOT NULL CHECK (source IN ('current_runtime', 'manual_chatgpt', 'manual_api')),
    lifecycle TEXT NOT NULL CHECK (
        lifecycle IN ('ready', 'record_unavailable', 'historical_only', 'needs_review')
    ),
    provider_id TEXT CHECK (provider_id IS NULL OR length(trim(provider_id)) BETWEEN 1 AND 128),
    model TEXT CHECK (model IS NULL OR length(trim(model)) BETWEEN 1 AND 256),
    revision INTEGER NOT NULL CHECK (revision > 0),
    display_order INTEGER NOT NULL UNIQUE CHECK (display_order >= 0),
    created_at TEXT NOT NULL CHECK (
        julianday(created_at) IS NOT NULL
        AND (substr(created_at, -1) = 'Z' OR substr(created_at, -6) = '+00:00')
    ),
    updated_at TEXT NOT NULL CHECK (
        julianday(updated_at) IS NOT NULL
        AND (substr(updated_at, -1) = 'Z' OR substr(updated_at, -6) = '+00:00')
    ),
    last_used_at TEXT CHECK (
        last_used_at IS NULL
        OR (
            julianday(last_used_at) IS NOT NULL
            AND (substr(last_used_at, -1) = 'Z' OR substr(last_used_at, -6) = '+00:00')
        )
    ),
    CHECK (julianday(updated_at) >= julianday(created_at)),
    CHECK (last_used_at IS NULL OR julianday(last_used_at) >= julianday(created_at)),
    CHECK (
        source = 'current_runtime'
        OR (source = 'manual_chatgpt' AND auth_mode = 'chatgpt')
        OR (source = 'manual_api' AND auth_mode = 'api_key')
    )
) STRICT;

CREATE INDEX vault_accounts_order_idx
    ON vault_accounts(display_order, account_id);

-- Selection is user intent; observation is the account independently seen in
-- the current runtime. Keeping both prevents a requested switch from being
-- presented as successful before postflight verification.
CREATE TABLE vault_environment_states (
    environment_id TEXT PRIMARY KEY NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    selected_account_id TEXT,
    observed_account_id TEXT,
    updated_at TEXT NOT NULL CHECK (
        julianday(updated_at) IS NOT NULL
        AND (substr(updated_at, -1) = 'Z' OR substr(updated_at, -6) = '+00:00')
    ),
    FOREIGN KEY (environment_id) REFERENCES environments(environment_id) ON DELETE CASCADE,
    FOREIGN KEY (selected_account_id) REFERENCES vault_accounts(account_id) ON DELETE SET NULL,
    FOREIGN KEY (observed_account_id) REFERENCES vault_accounts(account_id) ON DELETE SET NULL
) STRICT;
