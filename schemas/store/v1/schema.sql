CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY CHECK (version > 0),
    applied_at TEXT NOT NULL CHECK (
        julianday(applied_at) IS NOT NULL
        AND (substr(applied_at, -1) = 'Z' OR substr(applied_at, -6) = '+00:00')
    )
) STRICT;

CREATE TABLE IF NOT EXISTS environments (
    environment_id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(environment_id)) BETWEEN 1 AND 256),
    platform TEXT NOT NULL CHECK (length(trim(platform)) BETWEEN 1 AND 64),
    architecture TEXT NOT NULL CHECK (length(trim(architecture)) BETWEEN 1 AND 64),
    boundary TEXT NOT NULL CHECK (length(trim(boundary)) BETWEEN 1 AND 256),
    created_at TEXT NOT NULL CHECK (
        julianday(created_at) IS NOT NULL
        AND (substr(created_at, -1) = 'Z' OR substr(created_at, -6) = '+00:00')
    ),
    last_seen_at TEXT NOT NULL CHECK (
        julianday(last_seen_at) IS NOT NULL
        AND (substr(last_seen_at, -1) = 'Z' OR substr(last_seen_at, -6) = '+00:00')
    )
) STRICT;

-- Only stable HMAC fingerprints cross this persistence boundary. Ephemeral
-- session bindings deliberately have no representation in SQLite.
CREATE TABLE IF NOT EXISTS account_bindings (
    environment_id TEXT NOT NULL,
    account_fingerprint TEXT NOT NULL CHECK (
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
    created_at TEXT NOT NULL CHECK (
        julianday(created_at) IS NOT NULL
        AND (substr(created_at, -1) = 'Z' OR substr(created_at, -6) = '+00:00')
    ),
    last_seen_at TEXT NOT NULL CHECK (
        julianday(last_seen_at) IS NOT NULL
        AND (substr(last_seen_at, -1) = 'Z' OR substr(last_seen_at, -6) = '+00:00')
    ),
    PRIMARY KEY (environment_id, account_fingerprint),
    FOREIGN KEY (environment_id) REFERENCES environments(environment_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS compatibility_observations (
    observation_id TEXT PRIMARY KEY NOT NULL CHECK (length(observation_id) = 36),
    environment_id TEXT NOT NULL,
    executable_id TEXT NOT NULL CHECK (length(trim(executable_id)) BETWEEN 1 AND 256),
    codex_version TEXT CHECK (codex_version IS NULL OR length(trim(codex_version)) BETWEEN 1 AND 128),
    protocol_schema_fingerprint TEXT CHECK (
        protocol_schema_fingerprint IS NULL
        OR length(trim(protocol_schema_fingerprint)) BETWEEN 1 AND 128
    ),
    compatibility TEXT NOT NULL CHECK (
        compatibility IN (
            'tested',
            'expected_compatible',
            'not_tested',
            'unsupported',
            'known_broken',
            'not_applicable'
        )
    ),
    observed_at TEXT NOT NULL CHECK (
        julianday(observed_at) IS NOT NULL
        AND (substr(observed_at, -1) = 'Z' OR substr(observed_at, -6) = '+00:00')
    ),
    UNIQUE (environment_id, observation_id),
    FOREIGN KEY (environment_id) REFERENCES environments(environment_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS quota_snapshots (
    snapshot_id TEXT PRIMARY KEY NOT NULL CHECK (length(snapshot_id) = 36),
    environment_id TEXT NOT NULL,
    account_fingerprint TEXT NOT NULL,
    compatibility_observation_id TEXT NOT NULL,
    status_schema_version TEXT NOT NULL CHECK (length(trim(status_schema_version)) BETWEEN 1 AND 32),
    captured_at TEXT NOT NULL CHECK (
        julianday(captured_at) IS NOT NULL
        AND (substr(captured_at, -1) = 'Z' OR substr(captured_at, -6) = '+00:00')
    ),
    availability TEXT NOT NULL CHECK (availability IN ('complete', 'partial')),
    freshness TEXT NOT NULL CHECK (freshness = 'live'),
    FOREIGN KEY (environment_id, account_fingerprint)
        REFERENCES account_bindings(environment_id, account_fingerprint)
        ON UPDATE CASCADE ON DELETE CASCADE,
    FOREIGN KEY (environment_id, compatibility_observation_id)
        REFERENCES compatibility_observations(environment_id, observation_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX IF NOT EXISTS quota_snapshots_history_idx
    ON quota_snapshots(environment_id, account_fingerprint, captured_at DESC);

CREATE TABLE IF NOT EXISTS quota_windows (
    snapshot_id TEXT NOT NULL,
    limit_id TEXT NOT NULL CHECK (length(trim(limit_id)) BETWEEN 1 AND 128),
    label TEXT CHECK (label IS NULL OR length(trim(label)) BETWEEN 1 AND 256),
    window_minutes INTEGER CHECK (window_minutes IS NULL OR window_minutes > 0),
    used_basis_points INTEGER NOT NULL CHECK (used_basis_points BETWEEN 0 AND 10000),
    remaining_basis_points INTEGER NOT NULL CHECK (remaining_basis_points BETWEEN 0 AND 10000),
    resets_at TEXT CHECK (
        resets_at IS NULL
        OR (
            julianday(resets_at) IS NOT NULL
            AND (substr(resets_at, -1) = 'Z' OR substr(resets_at, -6) = '+00:00')
        )
    ),
    PRIMARY KEY (snapshot_id, limit_id),
    CHECK (used_basis_points + remaining_basis_points = 10000),
    FOREIGN KEY (snapshot_id) REFERENCES quota_snapshots(snapshot_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX IF NOT EXISTS quota_windows_limit_idx
    ON quota_windows(limit_id, resets_at, snapshot_id);

CREATE TABLE IF NOT EXISTS reset_credit_summaries (
    snapshot_id TEXT PRIMARY KEY NOT NULL,
    summary_status TEXT NOT NULL CHECK (summary_status IN ('available', 'partial', 'unavailable')),
    available_count INTEGER CHECK (available_count IS NULL OR available_count >= 0),
    details_status TEXT NOT NULL CHECK (details_status IN ('complete', 'partial', 'unavailable')),
    CHECK (
        (summary_status = 'unavailable' AND available_count IS NULL AND details_status = 'unavailable')
        OR (summary_status != 'unavailable' AND available_count IS NOT NULL)
    ),
    FOREIGN KEY (snapshot_id) REFERENCES quota_snapshots(snapshot_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS reset_credits (
    snapshot_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    opaque_id TEXT NOT NULL CHECK (length(trim(opaque_id)) BETWEEN 1 AND 256),
    reset_type TEXT NOT NULL CHECK (length(trim(reset_type)) BETWEEN 1 AND 64),
    status TEXT NOT NULL CHECK (length(trim(status)) BETWEEN 1 AND 64),
    granted_at TEXT CHECK (
        granted_at IS NULL
        OR (
            julianday(granted_at) IS NOT NULL
            AND (substr(granted_at, -1) = 'Z' OR substr(granted_at, -6) = '+00:00')
        )
    ),
    expires_at TEXT CHECK (
        expires_at IS NULL
        OR (
            julianday(expires_at) IS NOT NULL
            AND (substr(expires_at, -1) = 'Z' OR substr(expires_at, -6) = '+00:00')
        )
    ),
    title TEXT CHECK (title IS NULL OR length(trim(title)) BETWEEN 1 AND 512),
    description TEXT CHECK (description IS NULL OR length(trim(description)) BETWEEN 1 AND 512),
    PRIMARY KEY (snapshot_id, opaque_id),
    UNIQUE (snapshot_id, ordinal),
    FOREIGN KEY (snapshot_id) REFERENCES reset_credit_summaries(snapshot_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS settings (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    revision INTEGER NOT NULL CHECK (revision > 0),
    auto_refresh_enabled INTEGER NOT NULL CHECK (auto_refresh_enabled IN (0, 1)),
    refresh_interval_seconds INTEGER NOT NULL CHECK (refresh_interval_seconds BETWEEN 60 AND 86400),
    notification_threshold_basis_points INTEGER CHECK (
        notification_threshold_basis_points IS NULL
        OR notification_threshold_basis_points BETWEEN 0 AND 10000
    ),
    reset_credit_notice_hours INTEGER NOT NULL CHECK (reset_credit_notice_hours BETWEEN 1 AND 720),
    quiet_hours_enabled INTEGER NOT NULL CHECK (quiet_hours_enabled IN (0, 1)),
    quiet_hours_start_minute INTEGER CHECK (
        quiet_hours_start_minute IS NULL OR quiet_hours_start_minute BETWEEN 0 AND 1439
    ),
    quiet_hours_end_minute INTEGER CHECK (
        quiet_hours_end_minute IS NULL OR quiet_hours_end_minute BETWEEN 0 AND 1439
    ),
    language TEXT NOT NULL CHECK (language IN ('system', 'en', 'zh-CN')),
    lock_screen_privacy INTEGER NOT NULL CHECK (lock_screen_privacy IN (0, 1)),
    launch_at_login INTEGER NOT NULL CHECK (launch_at_login IN (0, 1)),
    history_retention_days INTEGER NOT NULL CHECK (history_retention_days BETWEEN 7 AND 730),
    updated_at TEXT NOT NULL CHECK (
        julianday(updated_at) IS NOT NULL
        AND (substr(updated_at, -1) = 'Z' OR substr(updated_at, -6) = '+00:00')
    ),
    CHECK (
        (quiet_hours_enabled = 0 AND quiet_hours_start_minute IS NULL AND quiet_hours_end_minute IS NULL)
        OR (quiet_hours_enabled = 1 AND quiet_hours_start_minute IS NOT NULL AND quiet_hours_end_minute IS NOT NULL)
    )
) STRICT;

INSERT OR IGNORE INTO settings (
    singleton_id,
    revision,
    auto_refresh_enabled,
    refresh_interval_seconds,
    notification_threshold_basis_points,
    reset_credit_notice_hours,
    quiet_hours_enabled,
    quiet_hours_start_minute,
    quiet_hours_end_minute,
    language,
    lock_screen_privacy,
    launch_at_login,
    history_retention_days,
    updated_at
) VALUES (
    1,
    1,
    1,
    300,
    2000,
    24,
    0,
    NULL,
    NULL,
    'system',
    1,
    0,
    180,
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
);
