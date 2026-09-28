//! Local-only reading receipts, separate from public evidence and account data.
use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PublicReadReceipt {
    signal_id: String,
    revision: u32,
}

pub(super) fn migrate(conn: &mut Connection) -> Result<(), String> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    tx.execute_batch("CREATE TABLE public_reads (signal_id TEXT PRIMARY KEY, revision INTEGER NOT NULL CHECK(revision>0));
        CREATE TABLE public_read_state (id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL CHECK(version>=0), initialized INTEGER NOT NULL CHECK(initialized IN (0,1)));
        INSERT INTO public_read_state VALUES (1, 0, 0);
        PRAGMA user_version=3;").map_err(db_error)?;
    baseline_if_needed(&tx)?;
    tx.commit().map_err(db_error)
}

// An upgrade or first collection starts from the existing history, not a flood
// of "new" messages. Subsequent app starts must never advance this baseline.
pub(super) fn baseline_if_needed(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("INSERT INTO public_reads SELECT signal_id, MAX(revision) FROM public_signals
        WHERE (SELECT initialized FROM public_read_state WHERE id=1)=0 GROUP BY signal_id;
        UPDATE public_read_state SET initialized=1 WHERE id=1 AND EXISTS(SELECT 1 FROM public_signals);").map_err(db_error)
}

pub(super) fn annotate(
    conn: &Connection,
    history: &mut changes::PublicRevisionHistory,
) -> Result<(), String> {
    let schema: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(db_error)?;
    if schema < 3 {
        return Ok(());
    }
    let (version, initialized): (u32, bool) = conn
        .query_row(
            "SELECT version, initialized FROM public_read_state WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error)?;
    history.read_version = version;
    if !initialized {
        return Ok(());
    }
    let mut query = conn
        .prepare("SELECT COALESCE((SELECT revision FROM public_reads WHERE signal_id=?1),0)")
        .map_err(db_error)?;
    for item in &mut history.items {
        let read_revision: u32 = query
            .query_row([&item.current.signal_id], |row| row.get(0))
            .map_err(db_error)?;
        item.unread = read_revision < item.current.revision;
    }
    Ok(())
}

fn mark_at_path(
    path: &Path,
    receipts: &[PublicReadReceipt],
    at: &UtcTimestamp,
) -> Result<PublicResetTimeline, String> {
    if receipts.is_empty()
        || receipts.len() > 128
        || receipts.iter().any(|item| {
            item.signal_id.is_empty() || item.signal_id.len() > 512 || item.revision == 0
        })
    {
        return Err(db_error("invalid receipt"));
    }
    let mut conn = open_store(path, true)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let mut changed = 0;
    for item in receipts {
        // Bind to exactly the revision the UI supplied; a simultaneous newer
        // collection must remain unread, and an unknown/future ID is rejected.
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM public_signals WHERE signal_id=?1 AND revision=?2)",
                params![item.signal_id, item.revision],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if !exists {
            return Err(db_error("unknown revision"));
        }
        changed += tx.execute("INSERT INTO public_reads(signal_id, revision) VALUES (?1, ?2)
            ON CONFLICT(signal_id) DO UPDATE SET revision=excluded.revision WHERE excluded.revision>public_reads.revision",
            params![item.signal_id, item.revision]).map_err(db_error)?;
    }
    if changed > 0 {
        tx.execute(
            "UPDATE public_read_state SET version=version+1 WHERE id=1",
            [],
        )
        .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)?;
    load_timeline(path, at)
}

#[tauri::command]
pub(crate) async fn mark_public_reset_changes_read<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    receipts: Vec<PublicReadReceipt>,
) -> Result<PublicResetTimeline, String> {
    let path = data_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let timeline = mark_at_path(&path, &receipts, &now())?;
        let _ = app.emit("public-reset-timeline-changed", ());
        Ok(timeline)
    })
    .await
    .map_err(db_error)?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u8) -> UtcTimestamp {
        UtcTimestamp::parse(format!("2026-09-12T{hour:02}:00:00Z")).unwrap()
    }
    fn save(path: &Path, hour: u8, state: &str) -> PublicResetTimeline {
        save_refresh(
            path,
            vec![(
                PublicTimelineSource::QuotaResets,
                Ok(super::super::tests::batch(hour, state)),
            )],
            &at(hour),
        )
        .unwrap()
    }
    fn receipt(timeline: &PublicResetTimeline) -> PublicReadReceipt {
        let current = &timeline.changes.items[0].current;
        PublicReadReceipt {
            signal_id: current.signal_id.clone(),
            revision: current.revision,
        }
    }

    #[test]
    fn first_collection_is_baseline_but_later_changes_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        let first = save(&path, 1, "likely");
        assert!(!first.changes.items[0].unread);
        let changed = save(&path, 2, "confirmed");
        assert!(changed.changes.items[0].unread);
        assert!(load_timeline(&path, &at(3)).unwrap().changes.items[0].unread);
        let read = mark_at_path(&path, &[receipt(&changed)], &at(3)).unwrap();
        assert!(!read.changes.items[0].unread);
        assert_eq!(read.changes.read_version, 1);
        let again = mark_at_path(&path, &[receipt(&changed)], &at(3)).unwrap();
        assert_eq!(again.changes.read_version, 1);
        assert_eq!(again.revision_count, changed.revision_count);
        assert!(!load_timeline(&path, &at(4)).unwrap().changes.items[0].unread);
    }

    #[test]
    fn a_late_receipt_cannot_swallow_a_newer_correction_or_move_read_state_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        save(&path, 1, "likely");
        let shown = save(&path, 2, "confirmed");
        let newer = save(&path, 3, "corrected");
        let read = mark_at_path(&path, &[receipt(&shown)], &at(4)).unwrap();
        assert!(read.changes.items[0].unread);
        mark_at_path(&path, &[receipt(&newer)], &at(4)).unwrap();
        let stale = mark_at_path(&path, &[receipt(&shown)], &at(4)).unwrap();
        assert!(!stale.changes.items[0].unread);
        assert_eq!(stale.changes.read_version, 2);
    }

    #[test]
    fn invalid_batch_is_atomic_and_does_not_rewrite_public_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        save(&path, 1, "likely");
        let changed = save(&path, 2, "confirmed");
        assert!(mark_at_path(
            &path,
            &[
                receipt(&changed),
                PublicReadReceipt {
                    signal_id: "not-stored".into(),
                    revision: 1
                }
            ],
            &at(3)
        )
        .is_err());
        let after = load_timeline(&path, &at(3)).unwrap();
        assert!(after.changes.items[0].unread);
        assert_eq!(after.changes.read_version, 0);
        assert_eq!(after.revision_count, changed.revision_count);
        assert!(mark_at_path(&path, &[], &at(3)).is_err());
        let too_many: Vec<_> = (0..129).map(|_| receipt(&changed)).collect();
        assert!(mark_at_path(&path, &too_many, &at(3)).is_err());
        let mut future = receipt(&changed);
        future.revision += 1;
        assert!(mark_at_path(&path, &[future], &at(3)).is_err());
    }

    #[test]
    fn version_two_migration_preserves_evidence_and_baselines_old_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        save(&path, 1, "likely");
        let before = save(&path, 2, "confirmed");
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "DROP TABLE public_reads; DROP TABLE public_read_state; PRAGMA user_version=2;",
            )
            .unwrap();
        let read_only = load_timeline(&path, &at(3)).unwrap();
        assert!(!read_only.changes.items[0].unread);
        open_store(&path, true).unwrap();
        let backup = path.with_extension("before-reading-state.sqlite3");
        assert_eq!(
            load_timeline(&backup, &at(3)).unwrap().revision_count,
            before.revision_count
        );
        let backup_bytes = fs::read(&backup).unwrap();
        let after = load_timeline(&path, &at(3)).unwrap();
        assert_eq!(after.revision_count, before.revision_count);
        assert!(after.changes.items.iter().all(|item| !item.unread));
        assert!(save(&path, 4, "corrected").changes.items[0].unread);
        assert_eq!(fs::read(&backup).unwrap(), backup_bytes);
    }

    #[test]
    fn failed_migration_rolls_back_schema_and_keeps_existing_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DB_NAME);
        let before = save(&path, 1, "likely");
        Connection::open(&path)
            .unwrap()
            .execute_batch("DROP TABLE public_read_state; PRAGMA user_version=2;")
            .unwrap();
        // A pre-existing conflicting table forces a transaction failure.
        assert!(open_store(&path, true).is_err());
        let conn = open_store(&path, false).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
        assert_eq!(
            load_timeline(&path, &at(2)).unwrap().revision_count,
            before.revision_count
        );
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='public_read_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
