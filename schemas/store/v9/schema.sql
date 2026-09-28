-- Frozen prospective inputs have no snapshot FK: retention must not silently
-- alter a recorded estimate. These records have their own matching retention.
CREATE TABLE pace_trials (
    trial_id TEXT PRIMARY KEY NOT NULL CHECK (length(trial_id) = 36),
    environment_id TEXT NOT NULL,
    account_fingerprint TEXT NOT NULL,
    limit_id TEXT NOT NULL CHECK (length(limit_id) BETWEEN 1 AND 128),
    algorithm_version TEXT NOT NULL CHECK (length(algorithm_version) BETWEEN 1 AND 128),
    issued_at TEXT NOT NULL CHECK (julianday(issued_at) IS NOT NULL AND substr(issued_at,-1)='Z'),
    forecast_horizon TEXT NOT NULL CHECK (julianday(forecast_horizon)>julianday(issued_at) AND substr(forecast_horizon,-1)='Z'),
    inputs_json TEXT NOT NULL CHECK (length(inputs_json) BETWEEN 1 AND 4194304),
    FOREIGN KEY (environment_id,account_fingerprint) REFERENCES account_bindings(environment_id,account_fingerprint) ON UPDATE CASCADE ON DELETE CASCADE
) STRICT;
CREATE INDEX pace_trial_scope_time ON pace_trials(environment_id,account_fingerprint,issued_at DESC);
CREATE TRIGGER pace_trial_immutable BEFORE UPDATE OF trial_id,limit_id,algorithm_version,issued_at,forecast_horizon,inputs_json ON pace_trials BEGIN SELECT RAISE(ABORT,'immutable pace trial'); END;

CREATE TABLE pace_trial_outcomes (
    trial_id TEXT PRIMARY KEY NOT NULL,
    assessed_at TEXT NOT NULL CHECK (julianday(assessed_at) IS NOT NULL AND substr(assessed_at,-1)='Z'),
    outcome_json TEXT NOT NULL CHECK (length(outcome_json) BETWEEN 1 AND 4194304),
    FOREIGN KEY (trial_id) REFERENCES pace_trials(trial_id) ON DELETE CASCADE
) STRICT;
CREATE TRIGGER pace_outcome_immutable BEFORE UPDATE ON pace_trial_outcomes BEGIN SELECT RAISE(ABORT,'immutable pace outcome'); END;
