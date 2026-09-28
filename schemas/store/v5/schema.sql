-- Reusable local work schedule. It deliberately contains no account identity
-- or quota values: a later immutable WorkPlan revision can reference this
-- schedule together with an explicit account/environment demand contract.
CREATE TABLE work_schedule_settings (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    revision INTEGER NOT NULL CHECK (revision > 0),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    updated_at TEXT NOT NULL CHECK (
        julianday(updated_at) IS NOT NULL
        AND (substr(updated_at, -1) = 'Z' OR substr(updated_at, -6) = '+00:00')
    )
) STRICT;

CREATE TABLE work_schedule_periods (
    singleton_id INTEGER NOT NULL CHECK (singleton_id = 1),
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 15),
    start_minute_of_day INTEGER NOT NULL CHECK (start_minute_of_day BETWEEN 0 AND 1439),
    end_minute_of_day INTEGER NOT NULL CHECK (end_minute_of_day BETWEEN 0 AND 1439),
    PRIMARY KEY (singleton_id, ordinal),
    CHECK (start_minute_of_day != end_minute_of_day),
    FOREIGN KEY (singleton_id) REFERENCES work_schedule_settings(singleton_id) ON DELETE CASCADE
) STRICT;

INSERT INTO work_schedule_settings (
    singleton_id,
    revision,
    enabled,
    updated_at
) VALUES (
    1,
    1,
    0,
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
);
