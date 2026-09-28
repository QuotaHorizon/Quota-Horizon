//! Upgrade the installed v5 layout with synthetic data by default. The one
//! explicitly ignored local rehearsal opens its specified source read-only and
//! migrates a private disposable SQLite snapshot, never the source database.

use super::*;
use crate::tests::{FINGERPRINT, observation, persist_legacy_snapshot, snapshot, v4_connection};
use rusqlite::types::Value;

type Rows = BTreeMap<&'static str, Vec<Vec<Value>>>;

fn v5_store() -> CapacityStore {
    let mut connection = v4_connection();
    let transaction = connection.transaction().unwrap();
    transaction.execute_batch(MIGRATION_V5).unwrap();
    record_schema_migration(&transaction, 5).unwrap();
    transaction.pragma_update(None, "user_version", 5).unwrap();
    transaction.commit().unwrap();
    let mut store = CapacityStore {
        connection,
        file_backed: false,
    };
    for date in ["2026-09-01T08:00:00Z", "2026-09-01T08:05:00Z"] {
        persist_legacy_snapshot(&mut store, &snapshot(date), &observation(date)).unwrap();
    }
    store
        .connection
        .execute_batch(
            "UPDATE settings SET revision = 4, refresh_interval_seconds = 900,
                 language = 'zh-CN', history_retention_days = 365;
             UPDATE work_schedule_settings SET revision = 3, enabled = 1;
             INSERT INTO work_schedule_periods VALUES (1, 0, 1380, 420);",
        )
        .unwrap();
    store
        .register_vault_account(
            &VaultAccountRegistration {
                account_fingerprint: AccountFingerprint::parse(FINGERPRINT).unwrap(),
                protected_record_ref: VaultRecordRef::parse(
                    "vault-record:v1:018f47a2-8a71-7f4a-9c35-1f4234a73312",
                )
                .unwrap(),
                display_name: "Synthetic upgrade account".into(),
                auth_mode: VaultAccountAuthMode::ChatGpt,
                source: VaultAccountSource::ManualChatGpt,
                lifecycle: VaultAccountLifecycle::Ready,
                provider_id: None,
                model: None,
            },
            &UtcTimestamp::parse("2026-09-01T08:00:00Z").unwrap(),
        )
        .unwrap();
    store
        .verify_schema_version(5, &SCHEMA_COLUMNS[..SCHEMA_V5_TABLE_COUNT])
        .unwrap();
    store
}

fn legacy_rows(connection: &Connection) -> Rows {
    SCHEMA_COLUMNS[..SCHEMA_V5_TABLE_COUNT]
        .iter()
        .map(|(table, columns)| {
            let predicate = if *table == "schema_migrations" {
                " WHERE version <= 5"
            } else {
                ""
            };
            let order = (1..=columns.len())
                .map(|index| index.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM {table}{predicate} ORDER BY {order}");
            let rows = connection
                .prepare(&sql)
                .unwrap()
                .query_map([], |row| {
                    (0..columns.len()).map(|index| row.get(index)).collect()
                })
                .unwrap()
                .collect::<Result<Vec<Vec<Value>>, _>>()
                .unwrap();
            (*table, rows)
        })
        .collect()
}

fn assert_v5_unchanged(store: &CapacityStore, before: &Rows) {
    assert_eq!(store.schema_version().unwrap(), 5);
    store
        .verify_schema_version(5, &SCHEMA_COLUMNS[..SCHEMA_V5_TABLE_COUNT])
        .unwrap();
    assert_eq!(&legacy_rows(&store.connection), before);
}

#[test]
fn v5_upgrade_preserves_every_legacy_value_and_does_not_invent_new_evidence() {
    let mut store = v5_store();
    let before = legacy_rows(&store.connection);
    store.migrate().unwrap();
    store.verify_schema().unwrap();
    assert_eq!(legacy_rows(&store.connection), before);
    for (table, _) in &SCHEMA_COLUMNS[SCHEMA_V5_TABLE_COUNT..] {
        assert_eq!(table_count(&store.connection, table).unwrap(), 0);
    }
    store.migrate().unwrap();
    assert_eq!(legacy_rows(&store.connection), before);
}

#[test]
fn each_later_migration_failure_rolls_the_entire_upgrade_back_to_v5() {
    for version in 6..=STORE_SCHEMA_VERSION {
        let mut store = v5_store();
        let before = legacy_rows(&store.connection);
        store
            .connection
            .execute_batch(&format!(
                "CREATE TEMP TRIGGER reject_upgrade BEFORE INSERT ON schema_migrations
                 WHEN NEW.version = {version} BEGIN
                     SELECT RAISE(ABORT, 'synthetic migration failure'); END;"
            ))
            .unwrap();
        assert!(store.migrate().is_err(), "migration {version}");
        assert_v5_unchanged(&store, &before);
    }
}

#[test]
fn successful_sql_with_invalid_final_ledger_must_still_roll_back() {
    let mut store = v5_store();
    let before = legacy_rows(&store.connection);
    // Unlike RAISE(ABORT), this lets every SQL statement return success. The
    // postcondition must be checked before COMMIT, not only when opening later.
    store
        .connection
        .execute_batch(
            "CREATE TEMP TRIGGER omit_last_ledger BEFORE INSERT ON schema_migrations
             WHEN NEW.version = 9 BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
    assert!(matches!(store.migrate(), Err(StoreError::SchemaInvariant)));
    assert_v5_unchanged(&store, &before);
}

#[test]
fn successful_sql_with_missing_settings_must_restore_original_settings() {
    let mut store = v5_store();
    let before = legacy_rows(&store.connection);
    store
        .connection
        .execute_batch(
            "CREATE TEMP TRIGGER remove_settings AFTER INSERT ON schema_migrations
             WHEN NEW.version = 9 BEGIN DELETE FROM settings; END;",
        )
        .unwrap();
    assert!(matches!(store.migrate(), Err(StoreError::SchemaInvariant)));
    assert_v5_unchanged(&store, &before);
}

#[test]
fn unrelated_unversioned_database_is_not_left_partially_initialized() {
    let connection = Connection::open_in_memory().unwrap();
    configure_connection(&connection, false).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE unrelated(value TEXT) STRICT; INSERT INTO unrelated VALUES ('keep');",
        )
        .unwrap();
    let mut store = CapacityStore {
        connection,
        file_backed: false,
    };
    assert!(matches!(store.migrate(), Err(StoreError::SchemaInvariant)));
    assert_eq!(store.schema_version().unwrap(), 0);
    let count: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        store
            .connection
            .query_row("SELECT value FROM unrelated", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
}

fn write_private_fixture(store: &CapacityStore, path: &Path) {
    prepare_database_file(path).unwrap();
    store
        .connection
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
}

#[test]
fn file_backed_v5_upgrade_survives_reopening_without_changing_old_rows() {
    let fixture = v5_store();
    let before = legacy_rows(&fixture.connection);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("synthetic.sqlite3");
    write_private_fixture(&fixture, &path);
    for _ in 0..2 {
        let store = CapacityStore::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), STORE_SCHEMA_VERSION);
        assert_eq!(legacy_rows(&store.connection), before);
    }
}

#[test]
fn failed_file_backed_validation_leaves_a_reopenable_v5_database() {
    let fixture = v5_store();
    let before = legacy_rows(&fixture.connection);
    fixture
        .connection
        .execute_batch(
            "CREATE TRIGGER omit_last_ledger BEFORE INSERT ON schema_migrations
         WHEN NEW.version = 9 BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("synthetic.sqlite3");
    write_private_fixture(&fixture, &path);
    assert!(matches!(
        CapacityStore::open(&path),
        Err(StoreError::SchemaInvariant)
    ));
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let restored = CapacityStore {
        connection,
        file_backed: true,
    };
    assert_v5_unchanged(&restored, &before);
}

fn rehearse_read_only_v5_source(source: &Path) -> Rows {
    verify_database_file(source).expect("source must already be a private regular file");
    let connection = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open source read-only");
    CapacityStore::verify_connection_schema_version(
        &connection,
        5,
        &SCHEMA_COLUMNS[..SCHEMA_V5_TABLE_COUNT],
    )
    .expect("source must have the installed v5 layout");
    let directory = tempfile::tempdir().expect("private disposable directory");
    let copy = directory.path().join("upgrade-rehearsal.sqlite3");
    prepare_database_file(&copy).expect("create private destination");
    // SQLite produces a consistent logical snapshot, including committed WAL
    // pages. A file copy of the live main database alone would not be sufficient.
    connection
        .execute("VACUUM INTO ?1", [copy.to_str().expect("temporary path")])
        .expect("create consistent private snapshot from read-only source");
    assert!(
        std::fs::metadata(&copy).unwrap().len() <= 128 * 1024 * 1024,
        "rehearsal snapshot exceeds the bounded local verification size"
    );
    let before_connection =
        Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let before = legacy_rows(&before_connection);
    drop(before_connection);
    for _ in 0..2 {
        let store = CapacityStore::open(&copy).expect("upgrade and reopen disposable copy");
        // Never print private rows, even in a failed opt-in rehearsal.
        assert!(
            legacy_rows(&store.connection) == before,
            "upgrade changed pre-existing values in the disposable copy"
        );
        assert_eq!(store.schema_version().unwrap(), STORE_SCHEMA_VERSION);
    }
    let source_version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(source_version, 5, "source schema must remain unchanged");
    drop(connection);
    directory
        .close()
        .expect("remove only owned disposable rehearsal files");
    before
}

#[test]
fn read_only_rehearsal_includes_live_wal_without_upgrading_source() {
    let fixture = v5_store();
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("live-synthetic.sqlite3");
    write_private_fixture(&fixture, &source);
    let writer = Connection::open(&source).unwrap();
    writer
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    writer
        .execute("UPDATE settings SET revision = 17", [])
        .unwrap();
    let captured = rehearse_read_only_v5_source(&source);
    assert_eq!(captured, legacy_rows(&writer));
    assert_eq!(
        writer
            .query_row("SELECT revision FROM settings", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        17
    );
}

#[test]
#[ignore = "opt-in private source read; only a disposable copy is migrated"]
fn local_history_upgrade_rehearsal() {
    let source = std::env::var_os("HORIZON_HISTORY_REHEARSAL_SOURCE")
        .expect("set the explicit local history source for this opt-in rehearsal");
    rehearse_read_only_v5_source(Path::new(&source));
}
