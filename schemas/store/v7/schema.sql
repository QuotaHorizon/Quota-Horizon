-- Explicit user activity only. Timer suggestions are not observations.
CREATE TABLE active_time_timers (
    timer_id TEXT PRIMARY KEY NOT NULL CHECK (length(timer_id) = 36),
    environment_id TEXT NOT NULL,
    account_fingerprint TEXT NOT NULL,
    owner_context_id TEXT NOT NULL CHECK (length(owner_context_id) <= 128),
    revision INTEGER NOT NULL CHECK (revision > 0),
    state TEXT NOT NULL CHECK (state IN ('running', 'paused', 'review', 'saved', 'discarded')),
    started_at TEXT NOT NULL CHECK (julianday(started_at) IS NOT NULL AND substr(started_at, -1) = 'Z'),
    ended_at TEXT CHECK (ended_at IS NULL OR (julianday(ended_at) IS NOT NULL AND substr(ended_at, -1) = 'Z')),
    suggested_seconds INTEGER NOT NULL CHECK (suggested_seconds BETWEEN 0 AND 86400),
    updated_at TEXT NOT NULL CHECK (julianday(updated_at) IS NOT NULL AND substr(updated_at, -1) = 'Z'),
    interrupted INTEGER NOT NULL CHECK (interrupted IN (0, 1)),
    FOREIGN KEY (environment_id, account_fingerprint) REFERENCES account_bindings(environment_id, account_fingerprint) ON UPDATE CASCADE ON DELETE CASCADE
) STRICT;
CREATE UNIQUE INDEX active_time_one_open_scope ON active_time_timers(environment_id, account_fingerprint) WHERE state IN ('running', 'paused', 'review');
CREATE UNIQUE INDEX active_time_one_running ON active_time_timers(state) WHERE state = 'running';

CREATE TABLE active_time_observations (
    observation_id TEXT PRIMARY KEY NOT NULL CHECK (length(observation_id) = 36),
    environment_id TEXT NOT NULL,
    account_fingerprint TEXT NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('user_timer', 'user_reported')),
    quality TEXT NOT NULL CHECK (quality = 'user_confirmed'),
    started_at TEXT NOT NULL CHECK (julianday(started_at) IS NOT NULL AND substr(started_at, -1) = 'Z'),
    ended_at TEXT NOT NULL CHECK (julianday(ended_at) > julianday(started_at) AND substr(ended_at, -1) = 'Z'),
    duration_seconds INTEGER NOT NULL CHECK (duration_seconds BETWEEN 1 AND 86400),
    included INTEGER NOT NULL CHECK (included IN (0, 1)),
    revision INTEGER NOT NULL CHECK (revision > 0),
    created_at TEXT NOT NULL CHECK (julianday(created_at) IS NOT NULL AND substr(created_at, -1) = 'Z'),
    FOREIGN KEY (environment_id, account_fingerprint) REFERENCES account_bindings(environment_id, account_fingerprint) ON UPDATE CASCADE ON DELETE CASCADE
) STRICT;
CREATE INDEX active_time_scope_dates ON active_time_observations(environment_id, account_fingerprint, ended_at DESC);
