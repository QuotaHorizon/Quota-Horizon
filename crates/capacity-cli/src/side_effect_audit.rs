use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;

const FNV_128_OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
const FNV_128_PRIME: u128 = 0x0000000001000000000000000000013b;
pub const MAX_FINGERPRINT_FILE_BYTES: u64 = 1024 * 1024;
pub const FINGERPRINT_BUDGET_BYTES_PER_CATEGORY: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditCategory {
    Auth,
    Config,
    Thread,
    State,
    RuntimeCache,
}

impl AuditCategory {
    pub const ALL: [Self; 5] = [
        Self::Auth,
        Self::Config,
        Self::Thread,
        Self::State,
        Self::RuntimeCache,
    ];

    pub const fn is_protected(self) -> bool {
        !matches!(self, Self::RuntimeCache)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct TrackedPath {
    category: AuditCategory,
    relative_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EntryFingerprint {
    kind: EntryKind,
    content: Option<u128>,
    length: u64,
    modified_nanos: Option<u128>,
}

#[derive(Debug, Clone, Default)]
pub struct ProtectedSnapshot {
    entries: BTreeMap<TrackedPath, EntryFingerprint>,
    fingerprinted_bytes: BTreeMap<AuditCategory, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CategoryDelta {
    pub category: AuditCategory,
    pub tracked_before: usize,
    pub tracked_after: usize,
    pub control_changes: usize,
    pub active_changes: usize,
    pub active_only_changes: usize,
    pub overlapping_changes: usize,
    pub active_created: usize,
    pub active_removed: usize,
    pub active_content_changed: usize,
    pub active_metadata_changed: usize,
    pub content_fingerprinted_before: usize,
    pub content_fingerprinted_after: usize,
    pub metadata_only_before: usize,
    pub metadata_only_after: usize,
    pub metadata_only_bytes_before: u64,
    pub metadata_only_bytes_after: u64,
}

#[derive(Debug, Default)]
struct ChangeSet {
    paths: BTreeSet<PathBuf>,
    created: usize,
    removed: usize,
    content_changed: usize,
    metadata_changed: usize,
}

pub fn capture_protected_snapshot(codex_home: &Path) -> io::Result<ProtectedSnapshot> {
    if !codex_home.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Codex home directory is not available",
        ));
    }

    let mut snapshot = ProtectedSnapshot::default();
    capture_exact(
        codex_home,
        Path::new("auth.json"),
        AuditCategory::Auth,
        &mut snapshot,
    )?;
    capture_exact(
        codex_home,
        Path::new("config.toml"),
        AuditCategory::Config,
        &mut snapshot,
    )?;

    for relative in ["history.jsonl", "session_index.jsonl"] {
        capture_exact(
            codex_home,
            Path::new(relative),
            AuditCategory::Thread,
            &mut snapshot,
        )?;
    }
    for relative in ["sessions", "archived_sessions"] {
        capture_tree(
            codex_home,
            Path::new(relative),
            AuditCategory::Thread,
            &mut snapshot,
        )?;
    }

    capture_matching_files(codex_home, Path::new(""), &mut snapshot, |name| {
        if name.starts_with("thread_history_") && sqlite_family(name) {
            Some(AuditCategory::Thread)
        } else if name.starts_with("state_") && sqlite_family(name) {
            Some(AuditCategory::State)
        } else if runtime_cache_name(name) {
            Some(AuditCategory::RuntimeCache)
        } else {
            None
        }
    })?;
    capture_matching_files(codex_home, Path::new("sqlite"), &mut snapshot, |name| {
        (name.starts_with("state_") && sqlite_family(name)).then_some(AuditCategory::State)
    })?;
    capture_tree(
        codex_home,
        Path::new(".tmp"),
        AuditCategory::RuntimeCache,
        &mut snapshot,
    )?;

    Ok(snapshot)
}

pub fn compare_snapshots(
    control_before: &ProtectedSnapshot,
    active_before: &ProtectedSnapshot,
    active_after: &ProtectedSnapshot,
) -> Vec<CategoryDelta> {
    AuditCategory::ALL
        .into_iter()
        .map(|category| {
            let control = changes_between(control_before, active_before, category);
            let active = changes_between(active_before, active_after, category);
            let active_only_changes = active.paths.difference(&control.paths).count();
            let overlapping_changes = active.paths.intersection(&control.paths).count();
            CategoryDelta {
                category,
                tracked_before: count_category(active_before, category),
                tracked_after: count_category(active_after, category),
                control_changes: control.paths.len(),
                active_changes: active.paths.len(),
                active_only_changes,
                overlapping_changes,
                active_created: active.created,
                active_removed: active.removed,
                active_content_changed: active.content_changed,
                active_metadata_changed: active.metadata_changed,
                content_fingerprinted_before: count_fingerprint_coverage(
                    active_before,
                    category,
                    true,
                ),
                content_fingerprinted_after: count_fingerprint_coverage(
                    active_after,
                    category,
                    true,
                ),
                metadata_only_before: count_fingerprint_coverage(active_before, category, false),
                metadata_only_after: count_fingerprint_coverage(active_after, category, false),
                metadata_only_bytes_before: metadata_only_bytes(active_before, category),
                metadata_only_bytes_after: metadata_only_bytes(active_after, category),
            }
        })
        .collect()
}

pub fn protected_active_change_count(deltas: &[CategoryDelta]) -> usize {
    deltas
        .iter()
        .filter(|delta| delta.category.is_protected())
        .map(|delta| delta.active_changes)
        .sum()
}

pub fn runtime_cache_active_change_count(deltas: &[CategoryDelta]) -> usize {
    deltas
        .iter()
        .filter(|delta| delta.category == AuditCategory::RuntimeCache)
        .map(|delta| delta.active_changes)
        .sum()
}

fn capture_exact(
    root: &Path,
    relative: &Path,
    category: AuditCategory,
    snapshot: &mut ProtectedSnapshot,
) -> io::Result<()> {
    capture_entry(root, relative, category, snapshot)
}

fn capture_tree(
    root: &Path,
    relative: &Path,
    category: AuditCategory,
    snapshot: &mut ProtectedSnapshot,
) -> io::Result<()> {
    let absolute = root.join(relative);
    let metadata = match fs::symlink_metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || metadata.is_file() {
        return capture_entry(root, relative, category, snapshot);
    }
    if !metadata.is_dir() {
        return Ok(());
    }

    let mut entries = fs::read_dir(absolute)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let child_relative = relative.join(entry.file_name());
        let child_metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if child_metadata.is_dir() && !child_metadata.file_type().is_symlink() {
            capture_tree(root, &child_relative, category, snapshot)?;
        } else {
            capture_entry(root, &child_relative, category, snapshot)?;
        }
    }
    Ok(())
}

fn capture_matching_files(
    root: &Path,
    relative_directory: &Path,
    snapshot: &mut ProtectedSnapshot,
    category_for_name: impl Fn(&str) -> Option<AuditCategory>,
) -> io::Result<()> {
    let absolute = root.join(relative_directory);
    let mut entries = match fs::read_dir(absolute) {
        Ok(entries) => entries.collect::<Result<Vec<_>, _>>()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(category) = category_for_name(name) else {
            continue;
        };
        capture_entry(root, &relative_directory.join(name), category, snapshot)?;
    }
    Ok(())
}

fn capture_entry(
    root: &Path,
    relative: &Path,
    category: AuditCategory,
    snapshot: &mut ProtectedSnapshot,
) -> io::Result<()> {
    let absolute = root.join(relative);
    let metadata = match fs::symlink_metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let file_type = metadata.file_type();
    let (kind, content) = if file_type.is_symlink() {
        let target = match fs::read_link(&absolute) {
            Ok(target) => target,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        (
            EntryKind::Symlink,
            Some(fingerprint_bytes(target.to_string_lossy().as_bytes())),
        )
    } else if metadata.is_file() {
        let used = snapshot
            .fingerprinted_bytes
            .get(&category)
            .copied()
            .unwrap_or(0);
        let should_fingerprint = metadata.len() <= MAX_FINGERPRINT_FILE_BYTES
            && used.saturating_add(metadata.len()) <= FINGERPRINT_BUDGET_BYTES_PER_CATEGORY;
        let content = if should_fingerprint {
            let content = match fingerprint_file(&absolute) {
                Ok(content) => content,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            };
            snapshot
                .fingerprinted_bytes
                .insert(category, used.saturating_add(metadata.len()));
            Some(content)
        } else {
            None
        };
        (EntryKind::File, content)
    } else {
        return Ok(());
    };
    snapshot.entries.insert(
        TrackedPath {
            category,
            relative_path: relative.to_path_buf(),
        },
        EntryFingerprint {
            kind,
            content,
            length: metadata.len(),
            modified_nanos: modified_nanos(&metadata),
        },
    );
    Ok(())
}

fn fingerprint_file(path: &Path) -> io::Result<u128> {
    let mut file = File::open(path)?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut hash = FNV_128_OFFSET;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            return Ok(hash);
        }
        hash = update_fingerprint(hash, &buffer[..count]);
    }
}

fn fingerprint_bytes(bytes: &[u8]) -> u128 {
    update_fingerprint(FNV_128_OFFSET, bytes)
}

fn update_fingerprint(mut hash: u128, bytes: &[u8]) -> u128 {
    for byte in bytes {
        hash ^= u128::from(*byte);
        hash = hash.wrapping_mul(FNV_128_PRIME);
    }
    hash
}

fn modified_nanos(metadata: &Metadata) -> Option<u128> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}

fn changes_between(
    before: &ProtectedSnapshot,
    after: &ProtectedSnapshot,
    category: AuditCategory,
) -> ChangeSet {
    let keys = before
        .entries
        .keys()
        .chain(after.entries.keys())
        .filter(|key| key.category == category)
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut changes = ChangeSet::default();
    for key in keys {
        match (before.entries.get(&key), after.entries.get(&key)) {
            (None, Some(_)) => {
                changes.created += 1;
                changes.paths.insert(key.relative_path);
            }
            (Some(_), None) => {
                changes.removed += 1;
                changes.paths.insert(key.relative_path);
            }
            (Some(before), Some(after))
                if before.kind != after.kind || before.length != after.length =>
            {
                changes.content_changed += 1;
                changes.paths.insert(key.relative_path);
            }
            (Some(before), Some(after))
                if before
                    .content
                    .zip(after.content)
                    .is_some_and(|(before, after)| before != after) =>
            {
                changes.content_changed += 1;
                changes.paths.insert(key.relative_path);
            }
            (Some(before), Some(after)) if before.modified_nanos != after.modified_nanos => {
                changes.metadata_changed += 1;
                changes.paths.insert(key.relative_path);
            }
            _ => {}
        }
    }
    changes
}

fn count_category(snapshot: &ProtectedSnapshot, category: AuditCategory) -> usize {
    snapshot
        .entries
        .keys()
        .filter(|entry| entry.category == category)
        .count()
}

fn count_fingerprint_coverage(
    snapshot: &ProtectedSnapshot,
    category: AuditCategory,
    fingerprinted: bool,
) -> usize {
    snapshot
        .entries
        .iter()
        .filter(|(entry, value)| {
            entry.category == category && value.content.is_some() == fingerprinted
        })
        .count()
}

fn metadata_only_bytes(snapshot: &ProtectedSnapshot, category: AuditCategory) -> u64 {
    snapshot
        .entries
        .iter()
        .filter(|(entry, value)| entry.category == category && value.content.is_none())
        .fold(0_u64, |total, (_, value)| {
            total.saturating_add(value.length)
        })
}

fn sqlite_family(name: &str) -> bool {
    name.contains(".sqlite")
}

fn runtime_cache_name(name: &str) -> bool {
    name.starts_with(".codex-global-state.json")
        || matches!(
            name,
            "models_cache.json" | "version.json" | ".app-server-state-reconciled-v1"
        )
}
