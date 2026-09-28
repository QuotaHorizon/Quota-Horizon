// Read-only projection of physical rollouts into logical sessions. Keep the raw
// discovery set intact: mutation admission must still detect ambiguous sources.
const THREAD_TAIL_BYTES: u64 = 128 * 1024;

// A cheap, read-only change signal for the visible session page. No transcript
// contents, account identifiers, or filesystem paths cross this IPC boundary.
fn thread_catalog_revision(codex_home: &Path) -> Result<String, String> {
    let mut rollouts = Vec::new();
    for folder in ROLLOUT_FOLDERS {
        collect_rollout_paths(&codex_home.join(folder), &mut rollouts)?;
    }
    rollouts.sort();
    rollouts.dedup();
    let mut paths = Vec::new();
    for path in rollouts {
        paths.push(compressed_rollout_path(&path));
        paths.push(path);
    }
    paths.push(codex_home.join(INDEX_NAME));
    for path in archive_state_databases(codex_home)? {
        paths.push(PathBuf::from(format!("{}-wal", path.to_string_lossy())));
        paths.push(path);
    }
    paths.sort();
    let mut hash = Sha256::new();
    hash.update(codex_home.as_os_str().as_encoded_bytes());
    for path in paths {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("Unable to inspect local session changes".to_owned()),
        };
        let name = path.as_os_str().as_encoded_bytes();
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name);
        hash.update(metadata.len().to_le_bytes());
        let modified = metadata.modified().ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok()).unwrap_or_default();
        hash.update(modified.as_nanos().to_le_bytes());
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn get_codex_thread_revision_blocking<R: Runtime>(app: tauri::AppHandle<R>) -> Result<String, String> {
    thread_catalog_revision(&resolve_paths(&app)?.codex_home)
}

#[derive(Debug, Default)]
struct ThreadReadHint {
    path: PathBuf,
    title: Option<String>,
    updated_at: Option<i64>,
}

#[derive(Debug)]
struct ThreadReadSelection {
    snapshot: RolloutSnapshot,
    rollout_count: usize,
}

fn read_hint_timestamp(value: rusqlite::types::ValueRef<'_>) -> Option<i64> {
    match value {
        rusqlite::types::ValueRef::Integer(value) => unix_seconds(&json!(value)),
        rusqlite::types::ValueRef::Text(value) => {
            unix_seconds(&json!(std::str::from_utf8(value).ok()?))
        }
        _ => None,
    }
    .filter(|value| *value > 0)
}

fn read_thread_hints_from_database(
    path: &Path,
    ids: &[String],
) -> rusqlite::Result<Vec<(String, ThreadReadHint)>> {
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(std::time::Duration::from_millis(250))?;
    let columns = connection
        .prepare("PRAGMA table_info(threads)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    if !columns.contains("id") || !columns.contains("rollout_path") {
        return Ok(Vec::new());
    }
    let optional = |column: &str| {
        if columns.contains(column) {
            quote_identifier(column)
        } else {
            "NULL".into()
        }
    };
    let titles = ["display_title", "title", "thread_name"]
        .into_iter()
        .map(|column| format!("NULLIF(TRIM({}), '')", optional(column)))
        .collect::<Vec<_>>()
        .join(", ");
    let mut result = Vec::new();
    for chunk in ids.chunks(500) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT id, rollout_path, COALESCE({titles}), {}, {} FROM threads WHERE id IN ({placeholders})",
            optional("updated_at"), optional("updated_at_ms"),
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(chunk), |row| {
            let updated = read_hint_timestamp(row.get_ref(3)?);
            let milliseconds = row
                .get::<_, Option<i64>>(4)
                .ok()
                .flatten()
                .filter(|value| *value > 0)
                .map(|value| value / 1000);
            Ok((
                row.get(0)?,
                ThreadReadHint {
                    path: PathBuf::from(row.get::<_, String>(1)?),
                    title: row.get(2)?,
                    updated_at: updated.max(milliseconds),
                },
            ))
        })?;
        result.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    Ok(result)
}

fn thread_read_hints(
    codex_home: &Path,
    snapshots: &[RolloutSnapshot],
) -> HashMap<String, ThreadReadHint> {
    let ids = snapshots
        .iter()
        .map(|snapshot| snapshot.session_id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut hints = HashMap::new();
    // Reuse the existing canonical-then-legacy path contract and the Viewer's
    // first-database-wins identity policy. Missing/locked DBs do not hide files.
    for database in archive_state_databases(codex_home).unwrap_or_default() {
        for (id, hint) in read_thread_hints_from_database(&database, &ids).unwrap_or_default() {
            hints.entry(id).or_insert(hint);
        }
    }
    hints
}

fn hint_matches_snapshot(
    codex_home: &Path,
    hint: &ThreadReadHint,
    snapshot: &RolloutSnapshot,
) -> bool {
    let path = if hint.path.is_absolute() {
        hint.path.clone()
    } else {
        codex_home.join(&hint.path)
    };
    // Never open a DB-provided arbitrary path. It may only identify a rollout
    // already discovered under the fixed roots with this logical session ID.
    snapshot.physical_paths.contains(&path)
}

fn rollout_tail_timestamp(path: &Path) -> Option<i64> {
    use std::io::{Seek, SeekFrom};
    if path.extension().and_then(|value| value.to_str()) == Some("zst") {
        return None; // Do not decompress an entire archived session for a list.
    }
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(THREAD_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(length - start).read_to_end(&mut bytes).ok()?;
    let mut lines = bytes.split(|byte| *byte == b'\n');
    if start != 0 {
        lines.next();
    } // The first slice may start mid-UTF-8/JSON.
    lines
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter_map(|value| value.get("timestamp").and_then(unix_seconds))
        .filter(|value| *value > 0)
        .max()
}

fn select_thread_read_snapshots(
    codex_home: &Path,
    snapshots: Vec<RolloutSnapshot>,
) -> Vec<ThreadReadSelection> {
    let hints = thread_read_hints(codex_home, &snapshots);
    let mut groups: HashMap<String, Vec<RolloutSnapshot>> = HashMap::new();
    for snapshot in snapshots {
        groups
            .entry(snapshot.session_id.clone())
            .or_default()
            .push(snapshot);
    }
    groups
        .into_iter()
        .map(|(id, mut copies)| {
            let hint = hints.get(&id);
            let rollout_count = copies.len();
            copies.sort_by(|left, right| {
                let rank = |snapshot: &RolloutSnapshot| {
                    (
                        hint.is_some_and(|hint| hint_matches_snapshot(codex_home, hint, snapshot)),
                        rollout_status(&snapshot.relative_path) == Some("active"),
                        snapshot.started_at,
                        snapshot.updated_at,
                    )
                };
                rank(left)
                    .cmp(&rank(right))
                    .then_with(|| right.relative_path.cmp(&left.relative_path))
            });
            let mut snapshot = copies
                .pop()
                .expect("a discovered session has at least one rollout");
            let hint = hint.filter(|hint| hint_matches_snapshot(codex_home, hint, &snapshot));
            if let Some(title) = hint.and_then(|hint| hint.title.as_ref()) {
                snapshot.title = title.clone();
            }
            let activity = [
                index_timestamp(Some(&snapshot.index_value)),
                hint.and_then(|hint| hint.updated_at),
                rollout_tail_timestamp(&snapshot.path),
                snapshot.started_at,
            ]
            .into_iter()
            .flatten()
            .max();
            snapshot.updated_at = activity.or_else(|| modified_seconds(&snapshot.path));
            ThreadReadSelection {
                snapshot,
                rollout_count,
            }
        })
        .collect()
}

fn thread_metadata_matches(snapshot: &RolloutSnapshot, query: &str) -> bool {
    [&snapshot.title, &snapshot.cwd, &snapshot.session_id]
        .into_iter()
        .any(|text| text.to_lowercase().contains(query))
}
