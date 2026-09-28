// A read-only logical history stream. Follow explicit paginated-history links;
// never concatenate unrelated files just because their payload IDs match.
const MAX_HISTORY_SEGMENTS: usize = 32;

// Keep the same per-record memory bound without making one embedded image or
// huge tool output hide the whole session. Hash/count all skipped bytes and
// explicitly report omissions. Mutation readers retain their strict rejection.
fn read_timeline_record(
    reader: &mut dyn BufRead,
    buffer: &mut Vec<u8>,
    digest: &mut Sha256,
    remaining_bytes: u64,
) -> Result<(usize, bool), String> {
    buffer.clear();
    let mut read = 0_usize;
    let mut oversized = false;
    loop {
        let chunk = reader
            .fill_buf()
            .map_err(|_| "会话记录暂不可读".to_owned())?;
        if chunk.is_empty() {
            break;
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(chunk.len(), |index| index + 1);
        read = read.saturating_add(count);
        if read as u64 > remaining_bytes {
            return Err("会话历史超过安全读取上限".to_owned());
        }
        digest.update(&chunk[..count]);
        if !oversized && read <= MAX_JSONL_LINE_BYTES {
            buffer.extend_from_slice(&chunk[..count]);
        } else {
            oversized = true;
            buffer.clear();
        }
        reader.consume(count);
        if newline.is_some() {
            break;
        }
    }
    Ok((read, oversized))
}

struct ThreadReadSource {
    path: PathBuf,
    limit: Option<u64>,
}

fn rollout_segment_id(snapshot: &RolloutSnapshot) -> String {
    let name = snapshot
        .path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let name = name.trim_end_matches(".zst").trim_end_matches(".jsonl");
    let prefix = format!("{}_", snapshot.session_id);
    name.rsplit_once(&prefix)
        .and_then(|(_, tail)| Uuid::parse_str(tail).ok())
        .map(|id| id.to_string())
        .unwrap_or_else(|| snapshot.session_id.clone())
}

fn logical_thread_sources(
    codex_home: &Path,
    session_id: &str,
) -> Result<(RolloutSnapshot, Vec<ThreadReadSource>), String> {
    let matches = gather_snapshots(codex_home)?
        .into_iter()
        .filter(|snapshot| snapshot.session_id == session_id)
        .collect::<Vec<_>>();
    let head = select_thread_read_snapshots(codex_home, matches.clone())
        .into_iter()
        .next()
        .ok_or_else(|| "会话不存在或已移动".to_owned())?
        .snapshot;
    let sources = logical_thread_sources_for_head(&head, &matches)?;
    Ok((head, sources))
}

// Listing/search already discovered all rollouts. Reuse that inventory instead
// of rescanning the entire catalog for every logical session.
fn logical_thread_sources_for_head(
    head: &RolloutSnapshot,
    matches: &[RolloutSnapshot],
) -> Result<Vec<ThreadReadSource>, String> {
    let mut current = head.clone();
    let mut limit = None;
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    loop {
        if sources.len() >= MAX_HISTORY_SEGMENTS || !seen.insert(current.path.clone()) {
            return Err("会话历史分段存在循环或超过读取上限，原文件未修改。".to_owned());
        }
        sources.push(ThreadReadSource {
            path: current.path.clone(),
            limit,
        });
        let meta =
            first_rollout_value(&current.path)?.ok_or_else(|| "会话分段元数据不完整".to_owned())?;
        let Some(base) = meta.pointer("/payload/history_base") else {
            break;
        };
        if meta
            .pointer("/payload/history_mode")
            .and_then(Value::as_str)
            != Some("paginated")
        {
            return Err("暂不支持该会话的历史引用方式，原文件未修改。".to_owned());
        }
        let id = base
            .get("thread_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "历史分段引用缺少标识".to_owned())?;
        let end = base
            .get("end_byte_offset")
            .and_then(Value::as_u64)
            .ok_or_else(|| "历史分段缺少安全读取边界".to_owned())?;
        if end == 0 {
            break;
        }
        let mut candidates = matches.iter().filter(|snapshot| {
            snapshot.session_id == head.session_id && rollout_segment_id(snapshot) == id
        });
        let parent = candidates
            .next()
            .ok_or_else(|| "会话引用的较早历史分段暂不可用，原文件未修改。".to_owned())?;
        if candidates.next().is_some() {
            return Err("历史分段引用对应多个来源，需先确认来源。".to_owned());
        }
        current = parent.clone();
        limit = Some(end);
    }
    sources.reverse();
    Ok(sources)
}

fn source_read_version(sources: &[ThreadReadSource]) -> Result<String, String> {
    let mut digest = Sha256::new();
    for source in sources {
        let metadata =
            fs::symlink_metadata(&source.path).map_err(|_| "历史分段已移动或不可读".to_owned())?;
        if !metadata.is_file() {
            return Err("历史分段不是普通文件".to_owned());
        }
        digest.update(source.path.as_os_str().as_encoded_bytes());
        digest.update(metadata.len().to_le_bytes());
        digest.update(source.limit.unwrap_or(u64::MAX).to_le_bytes());
        if let Ok(modified) = metadata.modified().and_then(|time| {
            time.duration_since(UNIX_EPOCH)
                .map_err(std::io::Error::other)
        }) {
            digest.update(modified.as_nanos().to_le_bytes());
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn parse_logical_thread_detail(
    head: &RolloutSnapshot,
    sources: &[ThreadReadSource],
) -> Result<ParsedThreadDetail, String> {
    let mut reader: Box<dyn BufRead> = Box::new(std::io::Cursor::new(Vec::<u8>::new()));
    let mut bytes = 0_u64;
    for source in sources {
        let (source_reader, size) = logical_source_reader(source)?;
        bytes = bytes.saturating_add(size);
        if bytes > MAX_SESSION_BYTES {
            return Err("会话历史超过安全读取上限".to_owned());
        }
        reader = Box::new(reader.chain(source_reader));
    }
    let mut parsed = parse_thread_detail_reader(head, reader)?;
    parsed.summary.size_bytes = bytes;
    parsed.summary.segment_count = sources.len();
    Ok(parsed)
}

fn logical_source_reader(source: &ThreadReadSource) -> Result<(Box<dyn BufRead + Send>, u64), String> {
    use std::io::{Seek, SeekFrom};
    let metadata = fs::symlink_metadata(&source.path).map_err(|_| "历史分段不可读".to_owned())?;
    if !metadata.is_file() {
        return Err("历史分段不是普通文件".to_owned());
    }
    let length = metadata.len();
    let compressed = source.path.extension().and_then(|value| value.to_str()) == Some("zst");
    if let Some(limit) = source.limit {
        if limit == 0 {
            return Ok((Box::new(std::io::Cursor::new(Vec::<u8>::new())), 0));
        }
        // Paginated byte offsets refer to the original JSONL. Reject an
        // unverifiable compressed prefix instead of guessing its boundary.
        if compressed || limit > length {
            return Err("历史分段读取边界无效，原文件未修改。".to_owned());
        }
        let mut file = File::open(&source.path).map_err(|_| "历史分段不可读".to_owned())?;
        file.seek(SeekFrom::Start(limit - 1))
            .map_err(|_| "历史分段读取边界不可用".to_owned())?;
        let mut last = [0_u8];
        file.read_exact(&mut last)
            .map_err(|_| "历史分段读取边界不可用".to_owned())?;
        if last[0] != b'\n' {
            return Err("历史分段边界不在完整记录末尾，原文件未修改。".to_owned());
        }
    }
    let limit = source.limit.unwrap_or(if compressed {
        MAX_SESSION_BYTES + 1
    } else {
        length
    });
    let size = if compressed { length } else { limit };
    if size > MAX_SESSION_BYTES {
        return Err("会话历史超过安全读取上限".to_owned());
    }
    Ok((Box::new(rollout_reader(&source.path)?.take(limit)), size))
}

struct CachedThreadRead {
    key: (PathBuf, String),
    source_version: String,
    captured: std::time::Instant,
    detail: std::sync::Arc<ParsedThreadDetail>,
}

fn read_logical_thread_detail(
    codex_home: &Path,
    session_id: &str,
    expected_revision: Option<&str>,
) -> Result<std::sync::Arc<ParsedThreadDetail>, String> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<CachedThreadRead>>> =
        std::sync::OnceLock::new();
    read_logical_thread_detail_cached(
        codex_home,
        session_id,
        expected_revision,
        CACHE.get_or_init(|| std::sync::Mutex::new(None)),
    )
}

fn read_logical_thread_detail_cached(
    codex_home: &Path,
    session_id: &str,
    expected_revision: Option<&str>,
    cache: &std::sync::Mutex<Option<CachedThreadRead>>,
) -> Result<std::sync::Arc<ParsedThreadDetail>, String> {
    let (head, sources) = logical_thread_sources(codex_home, session_id)?;
    let version = source_read_version(&sources)?;
    let key = (codex_home.to_path_buf(), session_id.to_owned());
    {
        let cached = cache
            .lock()
            .map_err(|_| "会话读取缓存暂不可用".to_owned())?;
        if let Some(cached) = cached
            .as_ref()
            .filter(|entry| entry.key == key && entry.captured.elapsed().as_secs() < 300)
        {
            // Paging a captured revision is immutable even when Codex appends
            // new messages. Explicit refresh (no expected revision) reads anew.
            if expected_revision == Some(cached.detail.revision.as_str())
                || (expected_revision.is_none() && version == cached.source_version)
            {
                return Ok(std::sync::Arc::clone(&cached.detail));
            }
        }
    }
    let parsed = std::sync::Arc::new(parse_logical_thread_detail(&head, &sources)?);
    *cache
        .lock()
        .map_err(|_| "会话读取缓存暂不可用".to_owned())? = Some(CachedThreadRead {
        key,
        source_version: version,
        captured: std::time::Instant::now(),
        detail: std::sync::Arc::clone(&parsed),
    });
    Ok(parsed)
}
