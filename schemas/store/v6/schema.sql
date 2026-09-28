-- Immutable explicit intent, isolated by stable account and environment.
-- No schedule reference: the existing mutable daily baseline is not a plan.
CREATE TABLE capacity_demands (
    environment_id TEXT NOT NULL,
    account_fingerprint TEXT NOT NULL,
    demand_id TEXT NOT NULL CHECK (length(demand_id) = 36),
    revision INTEGER NOT NULL CHECK (revision > 0),
    horizon_end TEXT NOT NULL CHECK (julianday(horizon_end) IS NOT NULL AND (substr(horizon_end, -1) = 'Z' OR substr(horizon_end, -6) = '+00:00')),
    demand_kind TEXT NOT NULL CHECK (demand_kind IN ('active_hours', 'maintain_recent_pace')),
    planned_codex_active_hours REAL,
    created_at TEXT NOT NULL CHECK (julianday(created_at) IS NOT NULL AND (substr(created_at, -1) = 'Z' OR substr(created_at, -6) = '+00:00')),
    PRIMARY KEY (environment_id, account_fingerprint, demand_id, revision),
    CHECK ((demand_kind = 'active_hours' AND planned_codex_active_hours IS NOT NULL AND planned_codex_active_hours > 0 AND planned_codex_active_hours <= 8784)
        OR (demand_kind = 'maintain_recent_pace' AND planned_codex_active_hours IS NULL)),
    FOREIGN KEY (environment_id, account_fingerprint) REFERENCES account_bindings(environment_id, account_fingerprint)
        ON UPDATE CASCADE ON DELETE CASCADE
) STRICT;

CREATE TABLE capacity_work_plans (
    environment_id TEXT NOT NULL,
    account_fingerprint TEXT NOT NULL,
    work_plan_id TEXT NOT NULL CHECK (length(work_plan_id) = 36),
    revision INTEGER NOT NULL CHECK (revision > 0),
    demand_id TEXT NOT NULL,
    demand_revision INTEGER NOT NULL CHECK (demand_revision > 0),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL CHECK (julianday(created_at) IS NOT NULL AND (substr(created_at, -1) = 'Z' OR substr(created_at, -6) = '+00:00')),
    PRIMARY KEY (environment_id, account_fingerprint, revision),
    FOREIGN KEY (environment_id, account_fingerprint, demand_id, demand_revision)
        REFERENCES capacity_demands(environment_id, account_fingerprint, demand_id, revision)
        ON UPDATE CASCADE ON DELETE CASCADE
) STRICT;
