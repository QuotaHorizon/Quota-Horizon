-- Context known at capture time only. Never backfill old snapshots from the
-- currently signed-in plan: it could conceal a historical plan change.
CREATE TABLE quota_snapshot_contexts (
    snapshot_id TEXT PRIMARY KEY NOT NULL,
    account_plan_type TEXT CHECK (account_plan_type IS NULL OR length(account_plan_type) BETWEEN 1 AND 64),
    FOREIGN KEY (snapshot_id) REFERENCES quota_snapshots(snapshot_id) ON DELETE CASCADE
) STRICT;
