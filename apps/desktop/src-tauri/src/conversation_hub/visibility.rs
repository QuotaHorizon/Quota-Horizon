const VISIBILITY_REPAIR_PLAN_PREFIX: &str = "session-visibility-repair-plan:v1:";
const VISIBILITY_REPAIR_PLAN_TTL_SECONDS: i64 = 120;
const MAX_PENDING_VISIBILITY_REPAIR_PLANS: usize = 32;
const MAX_VISIBILITY_REPAIR_TARGETS: usize = 512;
const MAX_VISIBILITY_REPAIR_DATABASES: usize = 64;
const MAX_VISIBILITY_REPAIR_DATABASE_ROWS: usize = 8192;
const VISIBILITY_REPAIR_TRANSACTION_FORMAT: &str =
    "quota-horizon-session-visibility-repair-v1";
const VISIBILITY_REPAIR_TRANSACTION_PREFIX: &str = "session-visibility-repair-tx-v1-";
const VISIBILITY_REPAIR_TRANSACTION_MANIFEST: &str = "manifest.json";
const MAX_VISIBILITY_REPAIR_MANIFEST_BYTES: u64 = 32 * 1024 * 1024;
const MAX_VISIBILITY_REPAIR_INDEX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_VISIBILITY_FIRST_MESSAGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_VISIBILITY_FIRST_MESSAGE_LINES: usize = 10_000;
const MAX_VISIBILITY_FIRST_MESSAGE_CHARS: usize = 4096;
const VISIBILITY_REPAIR_INDEX_ORIGINAL: &str = "index-original.jsonl";
const VISIBILITY_REPAIR_INDEX_TARGET: &str = "index-target.jsonl";
const CONSERVATIVE_READ_ONLY_SANDBOX: &str = concat!(
    "{\"type\":\"managed\",\"file_system\":{\"type\":\"restricted\",",
    "\"entries\":[{\"path\":{\"type\":\"special\",\"value\":{\"kind\":\"root\"}}",
    ",\"access\":\"read\"}]},\"network\":\"restricted\"}"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VisibilityRepairMode {
    Quick,
    Deep,
}

impl VisibilityRepairMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "quick" => Ok(Self::Quick),
            "deep" => Ok(Self::Deep),
            _ => Err("修复方式无效，请重新打开预览。".to_string()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Deep => "deep",
        }
    }
}

#[derive(Debug, Clone)]
struct PendingVisibilityRepairPlan {
    mode: VisibilityRepairMode,
    session_ids: Option<Vec<String>>,
    expected_revision: String,
    expires_at: DateTime<Utc>,
}

#[derive(Default)]
struct PendingVisibilityRepairPlans {
    plans: HashMap<String, PendingVisibilityRepairPlan>,
}

impl PendingVisibilityRepairPlans {
    fn issue(
        &mut self,
        prepared: &PreparedVisibilityRepair,
        now: DateTime<Utc>,
    ) -> Result<(String, DateTime<Utc>), String> {
        self.plans.retain(|_, plan| plan.expires_at > now);
        if self.plans.len() >= MAX_PENDING_VISIBILITY_REPAIR_PLANS {
            return Err("待确认的会话修复预览过多，请稍后重试。".to_string());
        }
        let token = format!(
            "{VISIBILITY_REPAIR_PLAN_PREFIX}{}",
            Uuid::new_v4().hyphenated()
        );
        let expires_at = now + chrono::Duration::seconds(VISIBILITY_REPAIR_PLAN_TTL_SECONDS);
        self.plans.insert(
            token.clone(),
            PendingVisibilityRepairPlan {
                mode: prepared.mode,
                session_ids: prepared.requested_session_ids.clone(),
                expected_revision: prepared.revision.clone(),
                expires_at,
            },
        );
        Ok((token, expires_at))
    }

    fn consume(
        &mut self,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<PendingVisibilityRepairPlan, String> {
        validate_visibility_repair_plan_token(token)?;
        let plan = self
            .plans
            .remove(token)
            .ok_or_else(|| "会话修复确认已失效，请重新预览。".to_string())?;
        if plan.expires_at <= now {
            return Err("会话修复确认已过期，请重新预览。".to_string());
        }
        Ok(plan)
    }
}

fn validate_visibility_repair_plan_token(token: &str) -> Result<(), String> {
    let value = token
        .strip_prefix(VISIBILITY_REPAIR_PLAN_PREFIX)
        .ok_or_else(|| "会话修复确认令牌格式无效。".to_string())?;
    let parsed = Uuid::parse_str(value).map_err(|_| "会话修复确认令牌格式无效。".to_string())?;
    if parsed.hyphenated().to_string() != value {
        return Err("会话修复确认令牌格式无效。".to_string());
    }
    Ok(())
}

fn pending_visibility_repair_plans(
) -> &'static std::sync::Mutex<PendingVisibilityRepairPlans> {
    static PLANS: std::sync::OnceLock<std::sync::Mutex<PendingVisibilityRepairPlans>> =
        std::sync::OnceLock::new();
    PLANS.get_or_init(|| std::sync::Mutex::new(PendingVisibilityRepairPlans::default()))
}

fn issue_visibility_repair_plan(
    prepared: &PreparedVisibilityRepair,
) -> Result<(String, DateTime<Utc>), String> {
    pending_visibility_repair_plans()
        .lock()
        .map_err(|_| "会话修复预览状态不可用。".to_string())?
        .issue(prepared, Utc::now())
}

fn consume_visibility_repair_plan(token: &str) -> Result<PendingVisibilityRepairPlan, String> {
    pending_visibility_repair_plans()
        .lock()
        .map_err(|_| "会话修复预览状态不可用。".to_string())?
        .consume(token, Utc::now())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct VisibilitySourceFact {
    session_id: String,
    title: String,
    first_user_message: Option<String>,
    cwd: String,
    model_provider: String,
    source: String,
    cli_version: String,
    originator: Option<String>,
    thread_source: Option<String>,
    agent_nickname: Option<String>,
    agent_role: Option<String>,
    agent_path: Option<String>,
    rollout_path: String,
    created_at: i64,
    updated_at: i64,
    tokens_used: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VisibilityRolloutGuard {
    session_id: String,
    source_relative: String,
    content_revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VisibilityDatabaseKind {
    State,
    Catalog,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VisibilityRowChange {
    session_id: String,
    host_id: Option<String>,
    original: Option<SqliteRowSnapshot>,
    target: SqliteRowSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VisibilityDatabasePlan {
    database_relative: String,
    kind: VisibilityDatabaseKind,
    schema_revision: String,
    rows: Vec<VisibilityRowChange>,
}

#[derive(Debug, Clone)]
struct PreparedVisibilityIndex {
    original_exists: bool,
    original_content: String,
    original_revision: String,
    target_exists: bool,
    target_content: String,
    target_revision: String,
    add_count: usize,
    update_count: usize,
}

#[derive(Debug, Clone)]
struct PreparedVisibilityRepair {
    mode: VisibilityRepairMode,
    requested_session_ids: Option<Vec<String>>,
    requested_count: usize,
    sources: Vec<VisibilitySourceFact>,
    rollout_guards: Vec<VisibilityRolloutGuard>,
    databases: Vec<VisibilityDatabasePlan>,
    index: Option<PreparedVisibilityIndex>,
    revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VisibilityRepairTransactionState {
    Prepared,
    Committed,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VisibilityIndexManifest {
    original_exists: bool,
    original_revision: String,
    target_exists: bool,
    target_revision: String,
    add_count: usize,
    update_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VisibilityRepairTransactionManifest {
    format: String,
    id: String,
    created_at: String,
    state: VisibilityRepairTransactionState,
    mode: VisibilityRepairMode,
    repaired_session_count: usize,
    rollout_guards: Vec<VisibilityRolloutGuard>,
    databases: Vec<VisibilityDatabasePlan>,
    index: Option<VisibilityIndexManifest>,
}

#[derive(Debug)]
struct VisibilityRepairTransaction {
    directory: PathBuf,
    manifest: VisibilityRepairTransactionManifest,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VisibilityTableColumn {
    name: String,
    declared_type: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key_position: i64,
}

fn latest_state_db(codex_home: &Path) -> Option<PathBuf> {
    archive_state_databases(codex_home)
        .ok()
        .and_then(|databases| databases.into_iter().next())
}

fn table_has_column(connection: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({})", quote_identifier(table)))
        .map_err(|error| error.to_string())?;
    let values = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| error.to_string())?;
    for value in values {
        if value.map_err(|error| error.to_string())? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn visibility_table_schema(
    connection: &Connection,
    table: &str,
) -> Result<Vec<VisibilityTableColumn>, String> {
    if !matches!(table, "threads" | "local_thread_catalog") {
        return Err("会话修复请求了不支持的数据表。".to_string());
    }
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({})", quote_identifier(table)))
        .map_err(|error| error.to_string())?;
    let columns = statement
        .query_map([], |row| {
            Ok(VisibilityTableColumn {
                name: row.get(1)?,
                declared_type: row.get(2)?,
                not_null: row.get::<_, i64>(3)? != 0,
                default_value: row.get(4)?,
                primary_key_position: row.get(5)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    if columns.is_empty() {
        return Err(format!("Codex 数据库缺少 {table} 表。"));
    }
    if columns.len() > 128
        || columns
            .iter()
            .any(|column| !valid_visibility_identifier(&column.name))
    {
        return Err(format!("Codex 数据库 {table} schema 超过安全范围。"));
    }
    let expected_primary_key = match table {
        "threads" => HashSet::from(["id"]),
        "local_thread_catalog" => HashSet::from(["host_id", "thread_id"]),
        _ => unreachable!(),
    };
    let primary_key = columns
        .iter()
        .filter(|column| column.primary_key_position > 0)
        .map(|column| column.name.as_str())
        .collect::<HashSet<_>>();
    if primary_key != expected_primary_key {
        return Err(format!(
            "Codex 数据库 {table} 缺少受支持的唯一主键，已拒绝自动修复。"
        ));
    }
    Ok(columns)
}

fn visibility_schema_revision(columns: &[VisibilityTableColumn]) -> Result<String, String> {
    let bytes = serde_json::to_vec(columns).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn visibility_content_revision(exists: bool, content: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(if exists {
        b"present:".as_slice()
    } else {
        b"absent:".as_slice()
    });
    digest.update(content);
    format!("{:x}", digest.finalize())
}

fn valid_visibility_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn enum_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => Some(value.clone()),
        Value::Null => None,
        value => serde_json::to_string(value).ok(),
    }
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty())
}

fn bounded_metadata_text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().count() <= 4096)
        .filter(|value| !value.chars().any(char::is_control))
        .map(str::to_string)
}

fn timestamp_from_metadata(meta: &Value) -> Option<i64> {
    meta.get("timestamp")
        .or_else(|| meta.pointer("/payload/timestamp"))
        .and_then(unix_seconds)
}

fn bounded_visibility_message(value: &str) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    if normalized.chars().count() <= MAX_VISIBILITY_FIRST_MESSAGE_CHARS {
        return Some(normalized);
    }
    let mut clipped = normalized
        .chars()
        .take(MAX_VISIBILITY_FIRST_MESSAGE_CHARS.saturating_sub(1))
        .collect::<String>();
    clipped.push('…');
    Some(clipped)
}

fn first_visibility_user_message(path: &Path) -> Result<Option<String>, String> {
    let mut reader = rollout_reader(path)?;
    let mut buffer = String::new();
    let mut parsed_bytes = 0_u64;
    let mut line_count = 0_usize;
    loop {
        let read = bounded_read_line(reader.as_mut(), &mut buffer)?;
        if read == 0 {
            return Ok(None);
        }
        parsed_bytes = parsed_bytes.saturating_add(read as u64);
        line_count += 1;
        if parsed_bytes > MAX_VISIBILITY_FIRST_MESSAGE_BYTES
            || line_count > MAX_VISIBILITY_FIRST_MESSAGE_LINES
        {
            return Ok(None);
        }
        let Ok(entry) = serde_json::from_str::<Value>(buffer.trim()) else {
            continue;
        };
        let payload = entry.get("payload").unwrap_or(&Value::Null);
        let message = match entry.get("type").and_then(Value::as_str) {
            Some("event_msg")
                if payload.get("type").and_then(Value::as_str) == Some("user_message") =>
            {
                payload.get("message").and_then(Value::as_str).map(str::to_string)
            }
            Some("response_item")
                if payload.get("type").and_then(Value::as_str) == Some("message")
                    && payload.get("role").and_then(Value::as_str) == Some("user") =>
            {
                Some(response_message_text(payload))
            }
            _ => None,
        };
        if let Some(message) = message.and_then(|value| bounded_visibility_message(&value)) {
            return Ok(Some(message));
        }
    }
}

fn visibility_source_fact(snapshot: &RolloutSnapshot) -> Result<VisibilitySourceFact, String> {
    let meta = first_rollout_value(&snapshot.path)?
        .ok_or_else(|| "会话 rollout 缺少有效 session_meta。".to_string())?;
    if snapshot_id(&meta).as_deref() != Some(snapshot.session_id.as_str()) {
        return Err("会话 rollout ID 与扫描结果不一致。".to_string());
    }
    let payload = meta
        .get("payload")
        .and_then(Value::as_object)
        .ok_or_else(|| "会话 rollout 缺少 metadata payload。".to_string())?;
    let model_provider = bounded_metadata_text(payload.get("model_provider"))
        .unwrap_or_else(|| "openai".to_string());
    let cwd = snapshot_cwd(&meta).ok_or_else(|| "会话 rollout 缺少 cwd。".to_string())?;
    if cwd.chars().count() > MAX_RESUME_CWD_CHARS || cwd.chars().any(char::is_control) {
        return Err("会话 rollout 的 cwd 无效。".to_string());
    }
    let source = enum_text(payload.get("source")).unwrap_or_else(|| "exec".to_string());
    if source.chars().count() > 4096 || source.chars().any(|value| value == '\0') {
        return Err("会话 rollout 的 source 无效。".to_string());
    }
    let updated_at = snapshot
        .updated_at
        .or_else(|| modified_seconds(&snapshot.path))
        .unwrap_or_else(|| Utc::now().timestamp());
    let created_at = timestamp_from_metadata(&meta).unwrap_or(updated_at);
    let first_user_message = first_visibility_user_message(&snapshot.path)?;
    let title = if snapshot.title == snapshot.session_id {
        first_user_message
            .as_deref()
            .map(excerpt)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| snapshot.title.clone())
    } else {
        snapshot.title.clone()
    };
    Ok(VisibilitySourceFact {
        session_id: snapshot.session_id.clone(),
        title,
        first_user_message,
        cwd,
        model_provider,
        source,
        cli_version: bounded_metadata_text(payload.get("cli_version")).unwrap_or_default(),
        originator: bounded_metadata_text(payload.get("originator")),
        thread_source: enum_text(payload.get("thread_source")),
        agent_nickname: bounded_metadata_text(payload.get("agent_nickname")),
        agent_role: bounded_metadata_text(payload.get("agent_role")),
        agent_path: bounded_metadata_text(payload.get("agent_path")),
        rollout_path: snapshot.path.to_string_lossy().into_owned(),
        created_at,
        updated_at,
        tokens_used: 0,
    })
}

fn normalized_visibility_repair_ids(
    values: Option<Vec<String>>,
) -> Result<Option<Vec<String>>, String> {
    let Some(values) = values else {
        return Ok(None);
    };
    let mut ids = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Err("请至少选择一条待修复会话。".to_string());
    }
    if ids.len() > MAX_VISIBILITY_REPAIR_TARGETS {
        return Err(format!(
            "单次最多可修复 {MAX_VISIBILITY_REPAIR_TARGETS} 条会话。"
        ));
    }
    if ids.iter().any(|id| safe_resume_command(id).is_none()) {
        return Err("待修复会话包含无效 ID。".to_string());
    }
    Ok(Some(ids))
}

fn prepare_visibility_sources(
    codex_home: &Path,
    requested: Option<&[String]>,
) -> Result<(Vec<VisibilitySourceFact>, Vec<VisibilityRolloutGuard>), String> {
    let snapshots = gather_snapshots(codex_home)?;
    let mut active = HashMap::<String, Vec<&RolloutSnapshot>>::new();
    let mut archived = HashSet::new();
    for snapshot in &snapshots {
        match rollout_status(&snapshot.relative_path) {
            Some("active") => {
                active
                    .entry(snapshot.session_id.clone())
                    .or_default()
                    .push(snapshot);
            }
            Some("archived") => {
                archived.insert(snapshot.session_id.clone());
            }
            _ => {}
        }
    }
    let selected = if let Some(requested) = requested {
        let mut selected = Vec::with_capacity(requested.len());
        for id in requested {
            let Some(candidates) = active.get(id) else {
                return Err(if archived.contains(id) {
                    format!("会话 {id} 已归档；可见性修复只处理 active 会话。")
                } else {
                    format!("未找到待修复的 active 会话 {id}。")
                });
            };
            selected.push(select_visibility_snapshot(codex_home, id, candidates)?);
        }
        selected
    } else {
        let mut selected = active
            .iter()
            .map(|(id, candidates)| select_visibility_snapshot(codex_home, id, candidates))
            .collect::<Result<Vec<_>, _>>()?;
        selected.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        selected
    };
    if selected.len() > MAX_VISIBILITY_REPAIR_TARGETS {
        return Err(format!(
            "当前 active 会话超过 {MAX_VISIBILITY_REPAIR_TARGETS} 条，请分批选择后修复。"
        ));
    }

    let mut sources = Vec::with_capacity(selected.len());
    let mut guards = Vec::new();
    for snapshot in selected {
        let fact = visibility_source_fact(snapshot)?;
        for physical in &snapshot.physical_paths {
            ensure_restore_target_parent_is_safe(codex_home, physical)?;
            let metadata = fs::symlink_metadata(physical)
                .map_err(|error| format!("无法验证会话 rollout：{error}"))?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err(format!("会话 rollout 路径类型不安全：{}", physical.display()));
            }
            let meta = first_rollout_value(physical)?
                .ok_or_else(|| "会话 rollout 缺少有效 session_meta。".to_string())?;
            let physical_provider = meta
                .pointer("/payload/model_provider")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("openai");
            let physical_source = enum_text(meta.pointer("/payload/source"))
                .unwrap_or_else(|| "exec".to_string());
            if snapshot_id(&meta).as_deref() != Some(fact.session_id.as_str())
                || snapshot_cwd(&meta).as_deref() != Some(fact.cwd.as_str())
                || physical_provider != fact.model_provider
                || physical_source != fact.source
            {
                return Err(format!(
                    "会话 {} 的多个物理 rollout metadata 不一致，已拒绝修复。",
                    fact.session_id
                ));
            }
            guards.push(VisibilityRolloutGuard {
                session_id: fact.session_id.clone(),
                source_relative: normalized_rebind_relative(codex_home, physical)?,
                content_revision: stable_archive_file_revision(physical)?,
            });
        }
        sources.push(fact);
    }
    sources.sort_by(|left, right| left.session_id.cmp(&right.session_id));
    guards.sort_by(|left, right| left.source_relative.cmp(&right.source_relative));
    let mut unique_guards = HashSet::new();
    if guards
        .iter()
        .any(|guard| !unique_guards.insert(guard.source_relative.clone()))
    {
        return Err("会话修复源包含重复 rollout。".to_string());
    }
    Ok((sources, guards))
}

fn snapshot_matches_recorded_rollout(snapshot: &RolloutSnapshot, recorded: &Path) -> bool {
    snapshot.physical_paths.iter().any(|candidate| {
        if candidate == recorded {
            return true;
        }
        if let (Ok(candidate), Ok(recorded)) = (candidate.canonicalize(), recorded.canonicalize()) {
            if candidate == recorded {
                return true;
            }
        }
        matches!(
            (logical_rollout_path(candidate), logical_rollout_path(recorded)),
            (Some(candidate), Some(recorded)) if candidate == recorded
        )
    })
}

fn select_visibility_snapshot<'a>(
    codex_home: &Path,
    session_id: &str,
    candidates: &[&'a RolloutSnapshot],
) -> Result<&'a RolloutSnapshot, String> {
    if candidates.len() == 1 {
        return Ok(candidates[0]);
    }
    let mut selected: Option<&RolloutSnapshot> = None;
    for database in archive_state_databases(codex_home)? {
        let Some(row) = snapshot_thread_row(Some(&database), session_id)? else {
            continue;
        };
        let Some(recorded) = row_text(&row, "rollout_path") else {
            continue;
        };
        let matches = candidates
            .iter()
            .copied()
            .filter(|snapshot| snapshot_matches_recorded_rollout(snapshot, Path::new(recorded)))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            continue;
        }
        if selected.is_some_and(|current| current.path != matches[0].path) {
            return Err(format!(
                "会话 {session_id} 的 canonical/legacy state DB 选择了不同 rollout，已拒绝修复。"
            ));
        }
        selected = Some(matches[0]);
    }
    selected.ok_or_else(|| {
        format!(
            "会话 {session_id} 存在多个 active rollout，且 state DB 未能唯一标识当前版本；已拒绝修复。"
        )
    })
}

fn row_cell<'a>(row: &'a SqliteRowSnapshot, column: &str) -> Option<&'a SqliteCell> {
    row.columns
        .iter()
        .position(|name| name == column)
        .and_then(|index| row.values.get(index))
}

fn row_text<'a>(row: &'a SqliteRowSnapshot, column: &str) -> Option<&'a str> {
    match row_cell(row, column) {
        Some(SqliteCell::Text(value)) => Some(value),
        _ => None,
    }
}

fn set_row_cell(row: &mut SqliteRowSnapshot, column: &str, value: SqliteCell) {
    if let Some(index) = row.columns.iter().position(|name| name == column) {
        row.values[index] = value;
    }
}

fn snapshot_visibility_row(
    connection: &Connection,
    kind: VisibilityDatabaseKind,
    session_id: &str,
    host_id: Option<&str>,
) -> Result<Option<SqliteRowSnapshot>, String> {
    let (table, predicate) = match kind {
        VisibilityDatabaseKind::State => ("threads", "id = ?1"),
        VisibilityDatabaseKind::Catalog => (
            "local_thread_catalog",
            "thread_id = ?1 AND host_id = ?2",
        ),
    };
    let mut statement = connection
        .prepare(&format!(
            "SELECT * FROM {} WHERE {predicate}",
            quote_identifier(table)
        ))
        .map_err(|error| error.to_string())?;
    let columns = statement
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let map = |row: &rusqlite::Row<'_>| {
        let values = (0..columns.len())
            .map(|index| row.get_ref(index).map(sqlite_cell))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(SqliteRowSnapshot {
            columns: columns.clone(),
            values,
        })
    };
    match kind {
        VisibilityDatabaseKind::State => statement
            .query_row(params![session_id], map)
            .optional()
            .map_err(|error| error.to_string()),
        VisibilityDatabaseKind::Catalog => statement
            .query_row(params![session_id, host_id.unwrap_or_default()], map)
            .optional()
            .map_err(|error| error.to_string()),
    }
}

fn catalog_host_ids(connection: &Connection, session_id: &str) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT host_id FROM local_thread_catalog WHERE thread_id = ?1 ORDER BY host_id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![session_id], |row| row.get(0))
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

fn strip_sql_default_parentheses(mut value: &str) -> &str {
    loop {
        let trimmed = value.trim();
        if trimmed.len() >= 2 && trimmed.starts_with('(') && trimmed.ends_with(')') {
            value = &trimmed[1..trimmed.len() - 1];
        } else {
            return trimmed;
        }
    }
}

fn sqlite_default_cell(value: &str) -> Option<SqliteCell> {
    let value = strip_sql_default_parentheses(value);
    if value.eq_ignore_ascii_case("NULL") {
        return Some(SqliteCell::Null);
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return Some(SqliteCell::Text(
            value[1..value.len() - 1].replace("''", "'"),
        ));
    }
    if let Ok(value) = value.parse::<i64>() {
        return Some(SqliteCell::Integer(value));
    }
    value.parse::<f64>().ok().map(SqliteCell::Real)
}

fn optional_text_cell(value: &Option<String>) -> SqliteCell {
    value
        .as_ref()
        .map(|value| SqliteCell::Text(value.clone()))
        .unwrap_or(SqliteCell::Null)
}

fn known_state_cell(column: &str, source: &VisibilitySourceFact) -> Option<SqliteCell> {
    let preview = source
        .first_user_message
        .clone()
        .or_else(|| (source.title != source.session_id).then(|| source.title.clone()))
        .unwrap_or_default();
    let created_ms = source.created_at.saturating_mul(1000);
    let updated_ms = source.updated_at.saturating_mul(1000);
    match column {
        "id" => Some(SqliteCell::Text(source.session_id.clone())),
        "rollout_path" => Some(SqliteCell::Text(source.rollout_path.clone())),
        "created_at" => Some(SqliteCell::Integer(source.created_at)),
        "updated_at" | "recency_at" => Some(SqliteCell::Integer(source.updated_at)),
        "created_at_ms" => Some(SqliteCell::Integer(created_ms)),
        "updated_at_ms" | "recency_at_ms" => Some(SqliteCell::Integer(updated_ms)),
        "source" => Some(SqliteCell::Text(source.source.clone())),
        "originator" => Some(optional_text_cell(&source.originator)),
        "history_mode" => Some(SqliteCell::Text("legacy".to_string())),
        "thread_source" => Some(optional_text_cell(&source.thread_source)),
        "agent_nickname" => Some(optional_text_cell(&source.agent_nickname)),
        "agent_role" => Some(optional_text_cell(&source.agent_role)),
        "agent_path" => Some(optional_text_cell(&source.agent_path)),
        "model_provider" => Some(SqliteCell::Text(source.model_provider.clone())),
        "model" | "reasoning_effort" => Some(SqliteCell::Null),
        "cwd" => Some(SqliteCell::Text(source.cwd.clone())),
        "cli_version" => Some(SqliteCell::Text(source.cli_version.clone())),
        "title" => Some(SqliteCell::Text(source.title.clone())),
        "name" => Some(SqliteCell::Null),
        "preview" | "first_user_message" => Some(SqliteCell::Text(preview)),
        "sandbox_policy" => Some(SqliteCell::Text(
            CONSERVATIVE_READ_ONLY_SANDBOX.to_string(),
        )),
        "approval_mode" => Some(SqliteCell::Text("on-request".to_string())),
        "tokens_used" => Some(SqliteCell::Integer(source.tokens_used)),
        "has_user_event" => Some(SqliteCell::Integer(i64::from(
            source.first_user_message.is_some(),
        ))),
        "archived" | "is_pinned" => Some(SqliteCell::Integer(0)),
        "archived_at" | "git_sha" | "git_branch" | "git_origin_url"
        | "thread_section_id" | "section_position" | "section_entered_at_ms" | "project_id"
        | "conversation_origin" => Some(SqliteCell::Null),
        "memory_mode" => Some(SqliteCell::Text("enabled".to_string())),
        _ => None,
    }
}

fn build_missing_state_row(
    schema: &[VisibilityTableColumn],
    source: &VisibilitySourceFact,
) -> Result<SqliteRowSnapshot, String> {
    let mut columns = Vec::with_capacity(schema.len());
    let mut values = Vec::with_capacity(schema.len());
    for column in schema {
        let value = known_state_cell(&column.name, source)
            .or_else(|| {
                column
                    .default_value
                    .as_deref()
                    .and_then(sqlite_default_cell)
            })
            .or_else(|| (!column.not_null).then_some(SqliteCell::Null))
            .ok_or_else(|| {
                format!(
                    "Codex state DB 新增了无法安全推导的必填列 {}，已拒绝插入缺失会话。",
                    column.name
                )
            })?;
        columns.push(column.name.clone());
        values.push(value);
    }
    Ok(SqliteRowSnapshot { columns, values })
}

fn validate_existing_rollout_selection(
    row: &SqliteRowSnapshot,
    source: &VisibilitySourceFact,
) -> Result<(), String> {
    let Some(current) = row_text(row, "rollout_path") else {
        return Ok(());
    };
    if current == source.rollout_path {
        return Ok(());
    }
    let current_path = Path::new(current);
    if !current_path.exists() {
        return Ok(());
    }
    let target_path = Path::new(&source.rollout_path);
    if let (Ok(current), Ok(target)) = (current_path.canonicalize(), target_path.canonicalize()) {
        if current == target {
            return Ok(());
        }
    }
    Err(format!(
        "会话 {} 的 state DB 已选择另一条仍存在的 rollout；为避免破坏 revert 语义，已拒绝改写。",
        source.session_id
    ))
}

fn build_state_target(
    schema: &[VisibilityTableColumn],
    original: Option<&SqliteRowSnapshot>,
    source: &VisibilitySourceFact,
) -> Result<SqliteRowSnapshot, String> {
    for required in ["id", "rollout_path", "model_provider", "cwd", "archived"] {
        if !schema.iter().any(|column| column.name == required) {
            return Err(format!("Codex state DB 缺少修复所需列 {required}。"));
        }
    }
    let Some(original) = original else {
        return build_missing_state_row(schema, source);
    };
    validate_existing_rollout_selection(original, source)?;
    let mut target = original.clone();
    set_row_cell(
        &mut target,
        "rollout_path",
        SqliteCell::Text(source.rollout_path.clone()),
    );
    set_row_cell(
        &mut target,
        "model_provider",
        SqliteCell::Text(source.model_provider.clone()),
    );
    set_row_cell(&mut target, "cwd", SqliteCell::Text(source.cwd.clone()));
    set_row_cell(&mut target, "source", SqliteCell::Text(source.source.clone()));
    if !source.cli_version.is_empty() {
        set_row_cell(
            &mut target,
            "cli_version",
            SqliteCell::Text(source.cli_version.clone()),
        );
    }
    set_row_cell(&mut target, "archived", SqliteCell::Integer(0));
    set_row_cell(&mut target, "archived_at", SqliteCell::Null);
    if row_text(original, "title").is_none_or(|value| value.trim().is_empty()) {
        set_row_cell(
            &mut target,
            "title",
            SqliteCell::Text(source.title.clone()),
        );
    }
    if row_text(original, "preview").is_none_or(|value| value.trim().is_empty())
        && (source.first_user_message.is_some() || source.title != source.session_id)
    {
        set_row_cell(
            &mut target,
            "preview",
            SqliteCell::Text(
                source
                    .first_user_message
                    .clone()
                    .unwrap_or_else(|| source.title.clone()),
            ),
        );
    }
    if row_text(original, "first_user_message").is_none_or(|value| value.trim().is_empty()) {
        if let Some(first_user_message) = source.first_user_message.as_ref() {
            set_row_cell(
                &mut target,
                "first_user_message",
                SqliteCell::Text(first_user_message.clone()),
            );
        }
    }
    if source.first_user_message.is_some()
        && row_cell(original, "has_user_event") == Some(&SqliteCell::Integer(0))
    {
        set_row_cell(&mut target, "has_user_event", SqliteCell::Integer(1));
    }
    Ok(target)
}

fn build_catalog_target(
    original: &SqliteRowSnapshot,
    source: &VisibilitySourceFact,
) -> SqliteRowSnapshot {
    let mut target = original.clone();
    set_row_cell(&mut target, "cwd", SqliteCell::Text(source.cwd.clone()));
    set_row_cell(
        &mut target,
        "model_provider",
        SqliteCell::Text(source.model_provider.clone()),
    );
    set_row_cell(&mut target, "missing_candidate", SqliteCell::Integer(0));
    if row_text(original, "display_title").is_none_or(|value| value.trim().is_empty()) {
        set_row_cell(
            &mut target,
            "display_title",
            SqliteCell::Text(source.title.clone()),
        );
    }
    target
}

fn prepare_state_database_plan(
    codex_home: &Path,
    database: &Path,
    sources: &[VisibilitySourceFact],
) -> Result<Option<VisibilityDatabasePlan>, String> {
    validate_archive_state_database_path(database)?;
    let connection = Connection::open(database)
        .map_err(|error| format!("无法打开 Codex state DB {}：{error}", database.display()))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let schema = visibility_table_schema(&connection, "threads")?;
    let schema_revision = visibility_schema_revision(&schema)?;
    let mut rows = Vec::new();
    for source in sources {
        let original = snapshot_visibility_row(
            &connection,
            VisibilityDatabaseKind::State,
            &source.session_id,
            None,
        )?;
        let target = build_state_target(&schema, original.as_ref(), source)?;
        if original.as_ref() != Some(&target) {
            rows.push(VisibilityRowChange {
                session_id: source.session_id.clone(),
                host_id: None,
                original,
                target,
            });
        }
    }
    if rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(VisibilityDatabasePlan {
        database_relative: normalized_rebind_relative(codex_home, database)?,
        kind: VisibilityDatabaseKind::State,
        schema_revision,
        rows,
    }))
}

fn prepare_catalog_database_plan(
    codex_home: &Path,
    database: &Path,
    sources: &[VisibilitySourceFact],
) -> Result<Option<VisibilityDatabasePlan>, String> {
    let metadata = fs::symlink_metadata(database)
        .map_err(|error| format!("无法验证 Codex catalog {}：{error}", database.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(format!("Codex catalog 路径类型不安全：{}", database.display()));
    }
    let connection = Connection::open(database)
        .map_err(|error| format!("无法打开 Codex catalog {}：{error}", database.display()))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let schema = visibility_table_schema(&connection, "local_thread_catalog")?;
    for required in ["host_id", "thread_id"] {
        if !schema.iter().any(|column| column.name == required) {
            return Err(format!("Codex thread catalog 缺少修复所需列 {required}。"));
        }
    }
    let schema_revision = visibility_schema_revision(&schema)?;
    let mut rows = Vec::new();
    for source in sources {
        for host_id in catalog_host_ids(&connection, &source.session_id)? {
            let original = snapshot_visibility_row(
                &connection,
                VisibilityDatabaseKind::Catalog,
                &source.session_id,
                Some(&host_id),
            )?
            .ok_or_else(|| "Codex thread catalog 在扫描期间发生变化。".to_string())?;
            let target = build_catalog_target(&original, source);
            if target != original {
                rows.push(VisibilityRowChange {
                    session_id: source.session_id.clone(),
                    host_id: Some(host_id),
                    original: Some(original),
                    target,
                });
            }
        }
    }
    if rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(VisibilityDatabasePlan {
        database_relative: normalized_rebind_relative(codex_home, database)?,
        kind: VisibilityDatabaseKind::Catalog,
        schema_revision,
        rows,
    }))
}

fn read_visibility_index(codex_home: &Path) -> Result<(bool, String), String> {
    let path = codex_home.join(INDEX_NAME);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((false, String::new()))
        }
        Err(error) => return Err(format!("无法验证会话索引：{error}")),
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > MAX_VISIBILITY_REPAIR_INDEX_BYTES
    {
        return Err("会话索引路径类型或大小不安全。".to_string());
    }
    let content = fs::read_to_string(&path).map_err(|error| format!("无法读取会话索引：{error}"))?;
    Ok((true, content))
}

fn prepare_visibility_index(
    codex_home: &Path,
    sources: &[VisibilitySourceFact],
) -> Result<PreparedVisibilityIndex, String> {
    let (original_exists, original_content) = read_visibility_index(codex_home)?;
    let mut lines = original_content
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut positions = HashMap::<String, usize>::new();
    for (index, line) in lines.iter().enumerate() {
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            continue;
        };
        if positions.insert(id.to_string(), index).is_some() {
            return Err(format!("会话索引包含重复 ID {id}，已拒绝自动重建。"));
        }
    }
    let mut add_count = 0usize;
    let mut update_count = 0usize;
    let mut additions = Vec::new();
    for source in sources {
        if let Some(index) = positions.get(&source.session_id).copied() {
            let mut value: Value = serde_json::from_str(lines[index].trim())
                .map_err(|_| "会话索引在准备期间发生变化。".to_string())?;
            let Some(object) = value.as_object_mut() else {
                return Err("会话索引 entry 不是 JSON object。".to_string());
            };
            let mut changed = false;
            if object
                .get("thread_name")
                .and_then(Value::as_str)
                .is_none_or(|value| value.trim().is_empty())
            {
                object.insert(
                    "thread_name".to_string(),
                    Value::String(source.title.clone()),
                );
                changed = true;
            }
            if !object.contains_key("updated_at") {
                if let Some(timestamp) = DateTime::<Utc>::from_timestamp(source.updated_at, 0) {
                    object.insert(
                        "updated_at".to_string(),
                        Value::String(timestamp.to_rfc3339()),
                    );
                    changed = true;
                }
            }
            if changed {
                let ending = if lines[index].ends_with("\r\n") {
                    "\r\n"
                } else if lines[index].ends_with('\n') {
                    "\n"
                } else {
                    ""
                };
                lines[index] = format!(
                    "{}{ending}",
                    serde_json::to_string(&value).map_err(|error| error.to_string())?
                );
                update_count += 1;
            }
        } else {
            let updated_at = DateTime::<Utc>::from_timestamp(source.updated_at, 0)
                .map(|value| value.to_rfc3339());
            additions.push(
                serde_json::to_string(&json!({
                    "id": source.session_id,
                    "thread_name": source.title,
                    "updated_at": updated_at,
                }))
                .map_err(|error| error.to_string())?,
            );
            add_count += 1;
        }
    }
    let changed = add_count + update_count > 0;
    let target_content = if !changed {
        original_content.clone()
    } else {
        let newline = if original_content.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let mut target = lines.concat();
        for addition in additions {
            if !target.is_empty() && !target.ends_with('\n') {
                target.push_str(newline);
            }
            target.push_str(&addition);
            target.push_str(newline);
        }
        target
    };
    if target_content.len() as u64 > MAX_VISIBILITY_REPAIR_INDEX_BYTES {
        return Err("修复后的会话索引超过安全大小限制。".to_string());
    }
    let target_exists = original_exists || !target_content.is_empty();
    Ok(PreparedVisibilityIndex {
        original_exists,
        original_revision: visibility_content_revision(
            original_exists,
            original_content.as_bytes(),
        ),
        original_content,
        target_exists,
        target_revision: visibility_content_revision(target_exists, target_content.as_bytes()),
        target_content,
        add_count,
        update_count,
    })
}

fn visibility_repair_revision(prepared: &PreparedVisibilityRepair) -> Result<String, String> {
    let index = prepared.index.as_ref().map(|index| {
        json!({
            "originalExists": index.original_exists,
            "originalRevision": index.original_revision,
            "targetExists": index.target_exists,
            "targetRevision": index.target_revision,
            "addCount": index.add_count,
            "updateCount": index.update_count,
        })
    });
    let value = json!({
        "mode": prepared.mode,
        "requestedSessionIds": prepared.requested_session_ids,
        "requestedCount": prepared.requested_count,
        "sources": prepared.sources,
        "rolloutGuards": prepared.rollout_guards,
        "databases": prepared.databases,
        "index": index,
    });
    let bytes = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn prepare_visibility_repair(
    codex_home: &Path,
    mode: VisibilityRepairMode,
    session_ids: Option<Vec<String>>,
) -> Result<PreparedVisibilityRepair, String> {
    let requested_session_ids = normalized_visibility_repair_ids(session_ids)?;
    let (sources, rollout_guards) = prepare_visibility_sources(
        codex_home,
        requested_session_ids.as_deref(),
    )?;
    let requested_count = requested_session_ids
        .as_ref()
        .map(Vec::len)
        .unwrap_or(sources.len());
    let mut databases = Vec::new();
    for database in archive_state_databases(codex_home)? {
        if let Some(plan) = prepare_state_database_plan(codex_home, &database, &sources)? {
            databases.push(plan);
        }
    }
    for database in catalog_database_paths(codex_home)? {
        if let Some(plan) = prepare_catalog_database_plan(codex_home, &database, &sources)? {
            databases.push(plan);
        }
    }
    databases.sort_by(|left, right| {
        left.database_relative
            .cmp(&right.database_relative)
            .then_with(|| (left.kind as u8).cmp(&(right.kind as u8)))
    });
    let index = (mode == VisibilityRepairMode::Deep)
        .then(|| prepare_visibility_index(codex_home, &sources))
        .transpose()?;
    let mut prepared = PreparedVisibilityRepair {
        mode,
        requested_session_ids,
        requested_count,
        sources,
        rollout_guards,
        databases,
        index,
        revision: String::new(),
    };
    prepared.revision = visibility_repair_revision(&prepared)?;
    Ok(prepared)
}

fn valid_visibility_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn validate_visibility_row_snapshot(
    snapshot: &SqliteRowSnapshot,
    schema: &[VisibilityTableColumn],
    kind: VisibilityDatabaseKind,
    session_id: &str,
    host_id: Option<&str>,
) -> Result<(), String> {
    let schema_columns = schema
        .iter()
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>();
    let snapshot_columns = snapshot
        .columns
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if snapshot.columns.len() != snapshot.values.len()
        || snapshot.columns.len() > 128
        || snapshot_columns != schema_columns
        || snapshot.columns.iter().any(|name| !valid_visibility_identifier(name))
        || row_text(snapshot, match kind {
            VisibilityDatabaseKind::State => "id",
            VisibilityDatabaseKind::Catalog => "thread_id",
        }) != Some(session_id)
    {
        return Err("会话修复事务包含无效的数据库行。".to_string());
    }
    match kind {
        VisibilityDatabaseKind::State if host_id.is_some() => {
            Err("state DB 修复行不应包含 host ID。".to_string())
        }
        VisibilityDatabaseKind::Catalog
            if host_id.is_none() || row_text(snapshot, "host_id") != host_id =>
        {
            Err("catalog 修复行缺少匹配的 host ID。".to_string())
        }
        _ => Ok(()),
    }
}

fn validate_visibility_database_plan(
    codex_home: &Path,
    plan: &VisibilityDatabasePlan,
) -> Result<(), String> {
    let relative_valid = match plan.kind {
        VisibilityDatabaseKind::State => {
            valid_rebind_state_database_relative(&plan.database_relative)
        }
        VisibilityDatabaseKind::Catalog => {
            valid_rebind_catalog_database_relative(&plan.database_relative)
        }
    };
    if !relative_valid
        || !valid_visibility_sha256(&plan.schema_revision)
        || plan.rows.is_empty()
        || plan.rows.len() > MAX_VISIBILITY_REPAIR_DATABASE_ROWS
    {
        return Err("会话修复事务包含无效的数据库目标。".to_string());
    }
    let path = codex_home.join(&plan.database_relative);
    match plan.kind {
        VisibilityDatabaseKind::State => validate_archive_state_database_path(&path)?,
        VisibilityDatabaseKind::Catalog => {
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("无法验证 Codex catalog：{error}"))?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err("Codex catalog 路径类型不安全。".to_string());
            }
        }
    }
    let connection = Connection::open(&path).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let table = match plan.kind {
        VisibilityDatabaseKind::State => "threads",
        VisibilityDatabaseKind::Catalog => "local_thread_catalog",
    };
    let schema = visibility_table_schema(&connection, table)?;
    if visibility_schema_revision(&schema)? != plan.schema_revision {
        return Err("Codex 数据库 schema 在预览后发生变化。".to_string());
    }
    let mut keys = HashSet::new();
    for row in &plan.rows {
        if safe_resume_command(&row.session_id).is_none()
            || !keys.insert((row.session_id.clone(), row.host_id.clone()))
            || (plan.kind == VisibilityDatabaseKind::Catalog && row.original.is_none())
        {
            return Err("会话修复事务包含重复或无效的数据库行。".to_string());
        }
        if let Some(original) = row.original.as_ref() {
            validate_visibility_row_snapshot(
                original,
                &schema,
                plan.kind,
                &row.session_id,
                row.host_id.as_deref(),
            )?;
        }
        validate_visibility_row_snapshot(
            &row.target,
            &schema,
            plan.kind,
            &row.session_id,
            row.host_id.as_deref(),
        )?;
    }
    Ok(())
}

fn visibility_row_key_values(
    kind: VisibilityDatabaseKind,
    session_id: &str,
    host_id: Option<&str>,
) -> Vec<SqlValue> {
    match kind {
        VisibilityDatabaseKind::State => vec![SqlValue::Text(session_id.to_string())],
        VisibilityDatabaseKind::Catalog => vec![
            SqlValue::Text(session_id.to_string()),
            SqlValue::Text(host_id.unwrap_or_default().to_string()),
        ],
    }
}

fn visibility_row_predicate(kind: VisibilityDatabaseKind, first_parameter: usize) -> String {
    match kind {
        VisibilityDatabaseKind::State => format!("id = ?{first_parameter}"),
        VisibilityDatabaseKind::Catalog => format!(
            "thread_id = ?{first_parameter} AND host_id = ?{}",
            first_parameter + 1
        ),
    }
}

fn insert_visibility_row(
    connection: &Connection,
    kind: VisibilityDatabaseKind,
    target: &SqliteRowSnapshot,
) -> Result<(), String> {
    let table = match kind {
        VisibilityDatabaseKind::State => "threads",
        VisibilityDatabaseKind::Catalog => "local_thread_catalog",
    };
    let placeholders = (1..=target.columns.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({placeholders})",
        quote_identifier(table),
        target
            .columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let values = target.values.iter().map(sql_value).collect::<Vec<_>>();
    let changed = connection
        .execute(&sql, params_from_iter(values))
        .map_err(|error| format!("无法插入缺失的 Codex 会话状态：{error}"))?;
    if changed != 1 {
        return Err("插入缺失的 Codex 会话状态未影响唯一一行。".to_string());
    }
    Ok(())
}

fn update_visibility_row(
    connection: &Connection,
    kind: VisibilityDatabaseKind,
    session_id: &str,
    host_id: Option<&str>,
    original: &SqliteRowSnapshot,
    target: &SqliteRowSnapshot,
) -> Result<(), String> {
    let key_columns: &[&str] = match kind {
        VisibilityDatabaseKind::State => &["id"],
        VisibilityDatabaseKind::Catalog => &["thread_id", "host_id"],
    };
    let changes = original
        .columns
        .iter()
        .zip(&original.values)
        .zip(&target.values)
        .filter(|((column, original), target)| {
            !key_columns.contains(&column.as_str()) && original != target
        })
        .map(|((column, _), target)| (column, target))
        .collect::<Vec<_>>();
    if changes.is_empty() {
        return Ok(());
    }
    let table = match kind {
        VisibilityDatabaseKind::State => "threads",
        VisibilityDatabaseKind::Catalog => "local_thread_catalog",
    };
    let assignments = changes
        .iter()
        .enumerate()
        .map(|(index, (column, _))| format!("{} = ?{}", quote_identifier(column), index + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let predicate = visibility_row_predicate(kind, changes.len() + 1);
    let sql = format!(
        "UPDATE {} SET {assignments} WHERE {predicate}",
        quote_identifier(table)
    );
    let mut values = changes
        .into_iter()
        .map(|(_, value)| sql_value(value))
        .collect::<Vec<_>>();
    values.extend(visibility_row_key_values(kind, session_id, host_id));
    let changed = connection
        .execute(&sql, params_from_iter(values))
        .map_err(|error| format!("无法更新 Codex 会话状态：{error}"))?;
    if changed != 1 {
        return Err("更新 Codex 会话状态未影响唯一一行。".to_string());
    }
    Ok(())
}

fn delete_visibility_row(
    connection: &Connection,
    kind: VisibilityDatabaseKind,
    session_id: &str,
    host_id: Option<&str>,
) -> Result<(), String> {
    let table = match kind {
        VisibilityDatabaseKind::State => "threads",
        VisibilityDatabaseKind::Catalog => "local_thread_catalog",
    };
    let sql = format!(
        "DELETE FROM {} WHERE {}",
        quote_identifier(table),
        visibility_row_predicate(kind, 1)
    );
    let changed = connection
        .execute(
            &sql,
            params_from_iter(visibility_row_key_values(kind, session_id, host_id)),
        )
        .map_err(|error| format!("无法回滚新插入的 Codex 会话状态：{error}"))?;
    if changed != 1 {
        return Err("回滚新插入的 Codex 会话状态未影响唯一一行。".to_string());
    }
    Ok(())
}

fn transition_visibility_row(
    connection: &Connection,
    kind: VisibilityDatabaseKind,
    change: &VisibilityRowChange,
    expected: Option<&SqliteRowSnapshot>,
    target: Option<&SqliteRowSnapshot>,
) -> Result<(), String> {
    let current = snapshot_visibility_row(
        connection,
        kind,
        &change.session_id,
        change.host_id.as_deref(),
    )?;
    if current.as_ref() == target {
        return Ok(());
    }
    if current.as_ref() != expected {
        return Err("Codex 会话状态出现无关并发变化，已停止事务。".to_string());
    }
    match (expected, target) {
        (None, Some(target)) => insert_visibility_row(connection, kind, target)?,
        (Some(original), Some(target)) => update_visibility_row(
            connection,
            kind,
            &change.session_id,
            change.host_id.as_deref(),
            original,
            target,
        )?,
        (Some(_), None) => delete_visibility_row(
            connection,
            kind,
            &change.session_id,
            change.host_id.as_deref(),
        )?,
        (None, None) => {}
    }
    if snapshot_visibility_row(
        connection,
        kind,
        &change.session_id,
        change.host_id.as_deref(),
    )?
    .as_ref()
        != target
    {
        return Err("Codex 会话状态写入后语义校验失败。".to_string());
    }
    Ok(())
}

fn apply_visibility_database_plan(
    codex_home: &Path,
    plan: &VisibilityDatabasePlan,
    forward: bool,
) -> Result<(), String> {
    validate_visibility_database_plan(codex_home, plan)?;
    let path = codex_home.join(&plan.database_relative);
    let mut connection = Connection::open(&path).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("无法锁定 Codex 会话数据库：{error}"))?;
    let table = match plan.kind {
        VisibilityDatabaseKind::State => "threads",
        VisibilityDatabaseKind::Catalog => "local_thread_catalog",
    };
    if visibility_schema_revision(&visibility_table_schema(&transaction, table)?)?
        != plan.schema_revision
    {
        return Err("Codex 数据库 schema 在获取写锁前发生变化。".to_string());
    }
    if forward {
        for change in &plan.rows {
            transition_visibility_row(
                &transaction,
                plan.kind,
                change,
                change.original.as_ref(),
                Some(&change.target),
            )?;
        }
    } else {
        for change in plan.rows.iter().rev() {
            transition_visibility_row(
                &transaction,
                plan.kind,
                change,
                Some(&change.target),
                change.original.as_ref(),
            )?;
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("无法提交 Codex 会话数据库事务：{error}"))
}

fn verify_visibility_database_plan(
    codex_home: &Path,
    plan: &VisibilityDatabasePlan,
    forward: bool,
) -> Result<(), String> {
    validate_visibility_database_plan(codex_home, plan)?;
    let connection = Connection::open(codex_home.join(&plan.database_relative))
        .map_err(|error| error.to_string())?;
    for change in &plan.rows {
        let expected = if forward {
            Some(&change.target)
        } else {
            change.original.as_ref()
        };
        if snapshot_visibility_row(
            &connection,
            plan.kind,
            &change.session_id,
            change.host_id.as_deref(),
        )?
        .as_ref()
            != expected
        {
            return Err("Codex 会话数据库 postflight 校验失败。".to_string());
        }
    }
    Ok(())
}

fn visibility_repair_transaction_root<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法定位应用数据目录：{error}"))?
        .join("codex-thread-visibility-transactions"))
}

fn ensure_visibility_repair_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() => {
            return Err(format!("会话修复事务路径类型不安全：{}", path.display()))
        }
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("无法验证会话修复事务路径：{error}")),
    }
    fs::create_dir_all(path).map_err(|error| format!("无法创建会话修复事务路径：{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("无法保护会话修复事务路径：{error}"))?;
    }
    Ok(())
}

fn sync_visibility_repair_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("无法同步会话修复事务：{error}"))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn write_visibility_repair_manifest(
    directory: &Path,
    manifest: &VisibilityRepairTransactionManifest,
) -> Result<(), String> {
    let text = serde_json::to_string_pretty(manifest).map_err(|error| error.to_string())?;
    if text.len() as u64 > MAX_VISIBILITY_REPAIR_MANIFEST_BYTES {
        return Err("会话修复事务清单超过安全大小限制。".to_string());
    }
    write_text_atomic(
        &directory.join(VISIBILITY_REPAIR_TRANSACTION_MANIFEST),
        &format!("{text}\n"),
    )?;
    sync_visibility_repair_directory(directory)
}

fn write_visibility_index_payload(path: &Path, content: &str) -> Result<(), String> {
    if content.len() as u64 > MAX_VISIBILITY_REPAIR_INDEX_BYTES {
        return Err("会话修复索引 payload 超过安全大小限制。".to_string());
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("无法创建会话修复索引 payload：{error}"))?;
    file.write_all(content.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("无法同步会话修复索引 payload：{error}"))
}

fn create_visibility_repair_transaction(
    transaction_root: &Path,
    codex_home: &Path,
    prepared: &PreparedVisibilityRepair,
) -> Result<VisibilityRepairTransaction, String> {
    ensure_visibility_repair_directory(transaction_root)?;
    let id = format!(
        "{VISIBILITY_REPAIR_TRANSACTION_PREFIX}{}",
        Uuid::new_v4().hyphenated()
    );
    let staging = transaction_root.join(format!(".capture-{}", Uuid::new_v4().hyphenated()));
    ensure_visibility_repair_directory(&staging)?;
    let result = (|| -> Result<VisibilityRepairTransaction, String> {
        let index = if let Some(index) = prepared.index.as_ref() {
            write_visibility_index_payload(
                &staging.join(VISIBILITY_REPAIR_INDEX_ORIGINAL),
                &index.original_content,
            )?;
            write_visibility_index_payload(
                &staging.join(VISIBILITY_REPAIR_INDEX_TARGET),
                &index.target_content,
            )?;
            Some(VisibilityIndexManifest {
                original_exists: index.original_exists,
                original_revision: index.original_revision.clone(),
                target_exists: index.target_exists,
                target_revision: index.target_revision.clone(),
                add_count: index.add_count,
                update_count: index.update_count,
            })
        } else {
            None
        };
        let manifest = VisibilityRepairTransactionManifest {
            format: VISIBILITY_REPAIR_TRANSACTION_FORMAT.to_string(),
            id: id.clone(),
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            state: VisibilityRepairTransactionState::Prepared,
            mode: prepared.mode,
            repaired_session_count: prepared.sources.len(),
            rollout_guards: prepared.rollout_guards.clone(),
            databases: prepared.databases.clone(),
            index,
        };
        write_visibility_repair_manifest(&staging, &manifest)?;
        let destination = transaction_root.join(&id);
        fs::rename(&staging, &destination)
            .map_err(|error| format!("无法提交会话修复恢复点：{error}"))?;
        sync_visibility_repair_directory(transaction_root)?;
        read_visibility_repair_transaction(codex_home, &destination)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn read_visibility_index_payload(path: &Path) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "会话修复索引 payload 缺失。".to_string())?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > MAX_VISIBILITY_REPAIR_INDEX_BYTES
    {
        return Err("会话修复索引 payload 类型或大小不安全。".to_string());
    }
    fs::read_to_string(path).map_err(|_| "会话修复索引 payload 不是有效文本。".to_string())
}

fn read_visibility_repair_transaction(
    codex_home: &Path,
    directory: &Path,
) -> Result<VisibilityRepairTransaction, String> {
    ensure_visibility_repair_directory(directory)?;
    let manifest_path = directory.join(VISIBILITY_REPAIR_TRANSACTION_MANIFEST);
    let metadata = fs::symlink_metadata(&manifest_path)
        .map_err(|error| format!("无法验证会话修复事务清单：{error}"))?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > MAX_VISIBILITY_REPAIR_MANIFEST_BYTES
    {
        return Err("会话修复事务清单类型或大小不安全。".to_string());
    }
    let manifest: VisibilityRepairTransactionManifest = serde_json::from_slice(
        &fs::read(&manifest_path).map_err(|error| error.to_string())?,
    )
    .map_err(|_| "会话修复事务清单无效。".to_string())?;
    let id_value = manifest
        .id
        .strip_prefix(VISIBILITY_REPAIR_TRANSACTION_PREFIX)
        .ok_or_else(|| "会话修复事务 ID 无效。".to_string())?;
    if manifest.format != VISIBILITY_REPAIR_TRANSACTION_FORMAT
        || directory.file_name().and_then(|value| value.to_str()) != Some(manifest.id.as_str())
        || Uuid::parse_str(id_value)
            .ok()
            .is_none_or(|uuid| uuid.hyphenated().to_string() != id_value)
        || manifest.repaired_session_count > MAX_VISIBILITY_REPAIR_TARGETS
    {
        return Err("会话修复事务身份无效。".to_string());
    }
    if manifest.state != VisibilityRepairTransactionState::Prepared {
        return Ok(VisibilityRepairTransaction {
            directory: directory.to_path_buf(),
            manifest,
        });
    }

    let mut guard_paths = HashSet::new();
    let mut guard_sessions = HashSet::new();
    for guard in &manifest.rollout_guards {
        let relative = safe_relative_path(&guard.source_relative)
            .ok_or_else(|| "会话修复事务包含不安全的 rollout 路径。".to_string())?;
        if safe_resume_command(&guard.session_id).is_none()
            || rollout_status(&relative) != Some("active")
            || !valid_archive_content_revision(&guard.content_revision)
            || !guard_paths.insert(guard.source_relative.clone())
        {
            return Err("会话修复事务包含无效或重复的 rollout guard。".to_string());
        }
        guard_sessions.insert(guard.session_id.clone());
    }
    if guard_sessions.len() != manifest.repaired_session_count
        || (manifest.repaired_session_count > 0 && manifest.rollout_guards.is_empty())
    {
        return Err("会话修复事务的 rollout guard 数量不一致。".to_string());
    }

    if manifest.databases.len() > MAX_VISIBILITY_REPAIR_DATABASES {
        return Err("会话修复事务包含过多数据库。".to_string());
    }
    let mut database_paths = HashSet::new();
    for database in &manifest.databases {
        if !database_paths.insert((database.database_relative.clone(), database.kind)) {
            return Err("会话修复事务包含重复数据库。".to_string());
        }
        validate_visibility_database_plan(codex_home, database)?;
        if database
            .rows
            .iter()
            .any(|row| !guard_sessions.contains(&row.session_id))
        {
            return Err("会话修复事务包含未受 rollout guard 保护的数据库行。".to_string());
        }
    }
    if (manifest.mode == VisibilityRepairMode::Quick && manifest.index.is_some())
        || (manifest.mode == VisibilityRepairMode::Deep && manifest.index.is_none())
    {
        return Err("会话修复事务的修复方式与索引计划不一致。".to_string());
    }
    if let Some(index) = manifest.index.as_ref() {
        if !valid_visibility_sha256(&index.original_revision)
            || !valid_visibility_sha256(&index.target_revision)
            || index.add_count.saturating_add(index.update_count)
                > manifest.repaired_session_count
        {
            return Err("会话修复事务包含无效的索引 revision。".to_string());
        }
        let original = read_visibility_index_payload(
            &directory.join(VISIBILITY_REPAIR_INDEX_ORIGINAL),
        )?;
        let target =
            read_visibility_index_payload(&directory.join(VISIBILITY_REPAIR_INDEX_TARGET))?;
        if visibility_content_revision(index.original_exists, original.as_bytes())
            != index.original_revision
            || visibility_content_revision(index.target_exists, target.as_bytes())
                != index.target_revision
        {
            return Err("会话修复索引 payload 校验失败。".to_string());
        }
    }
    Ok(VisibilityRepairTransaction {
        directory: directory.to_path_buf(),
        manifest,
    })
}

fn verify_visibility_rollout_guards(
    codex_home: &Path,
    guards: &[VisibilityRolloutGuard],
) -> Result<(), String> {
    for guard in guards {
        let relative = safe_relative_path(&guard.source_relative)
            .ok_or_else(|| "会话修复 rollout guard 路径无效。".to_string())?;
        let path = codex_home.join(relative);
        ensure_restore_target_parent_is_safe(codex_home, &path)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| "会话修复所依据的 rollout 已不存在。".to_string())?;
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_file()
            || stable_archive_file_revision(&path)? != guard.content_revision
        {
            return Err("会话修复所依据的 rollout 在预览后发生变化。".to_string());
        }
    }
    Ok(())
}

fn verify_visibility_index(
    codex_home: &Path,
    expected_exists: bool,
    expected_revision: &str,
) -> Result<(), String> {
    let (exists, content) = read_visibility_index(codex_home)?;
    if exists != expected_exists
        || visibility_content_revision(exists, content.as_bytes()) != expected_revision
    {
        return Err("会话索引在修复期间发生变化。".to_string());
    }
    Ok(())
}

fn apply_visibility_index(
    codex_home: &Path,
    expected_exists: bool,
    expected_revision: &str,
    target_exists: bool,
    target_revision: &str,
    target_content: &str,
) -> Result<(), String> {
    let (current_exists, current_content) = read_visibility_index(codex_home)?;
    let current_revision = visibility_content_revision(current_exists, current_content.as_bytes());
    if current_exists == target_exists && current_revision == target_revision {
        return Ok(());
    }
    if current_exists != expected_exists || current_revision != expected_revision {
        return Err("会话索引出现无关并发变化，已停止事务。".to_string());
    }
    let path = codex_home.join(INDEX_NAME);
    if target_exists {
        write_text_atomic(&path, target_content)?;
    } else if current_exists {
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("无法验证待回滚会话索引：{error}"))?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err("待回滚会话索引路径类型不安全。".to_string());
        }
        fs::remove_file(&path).map_err(|error| format!("无法移除新建会话索引：{error}"))?;
        sync_visibility_repair_directory(codex_home)?;
    }
    verify_visibility_index(codex_home, target_exists, target_revision)
}

fn visibility_index_payload(
    transaction: &VisibilityRepairTransaction,
    original: bool,
) -> Result<String, String> {
    read_visibility_index_payload(&transaction.directory.join(if original {
        VISIBILITY_REPAIR_INDEX_ORIGINAL
    } else {
        VISIBILITY_REPAIR_INDEX_TARGET
    }))
}

fn verify_visibility_repair_precondition(
    codex_home: &Path,
    transaction: &VisibilityRepairTransaction,
) -> Result<(), String> {
    verify_visibility_rollout_guards(codex_home, &transaction.manifest.rollout_guards)?;
    for database in &transaction.manifest.databases {
        verify_visibility_database_plan(codex_home, database, false)?;
    }
    if let Some(index) = transaction.manifest.index.as_ref() {
        verify_visibility_index(codex_home, index.original_exists, &index.original_revision)?;
    }
    Ok(())
}

fn verify_visibility_repair_postcondition(
    codex_home: &Path,
    transaction: &VisibilityRepairTransaction,
) -> Result<(), String> {
    verify_visibility_rollout_guards(codex_home, &transaction.manifest.rollout_guards)?;
    for database in &transaction.manifest.databases {
        verify_visibility_database_plan(codex_home, database, true)?;
    }
    if let Some(index) = transaction.manifest.index.as_ref() {
        verify_visibility_index(codex_home, index.target_exists, &index.target_revision)?;
    }
    Ok(())
}

fn rollback_visibility_repair_transaction(
    codex_home: &Path,
    transaction: &mut VisibilityRepairTransaction,
) -> Result<(), String> {
    let mut failures = Vec::new();
    if let Some(index) = transaction.manifest.index.as_ref() {
        match visibility_index_payload(transaction, true).and_then(|original| {
            apply_visibility_index(
                codex_home,
                index.target_exists,
                &index.target_revision,
                index.original_exists,
                &index.original_revision,
                &original,
            )
        }) {
            Ok(()) => {}
            Err(error) => failures.push(error),
        }
    }
    for database in transaction.manifest.databases.iter().rev() {
        if let Err(error) = apply_visibility_database_plan(codex_home, database, false) {
            failures.push(error);
        }
    }
    for database in &transaction.manifest.databases {
        if let Err(error) = verify_visibility_database_plan(codex_home, database, false) {
            failures.push(error);
        }
    }
    if let Some(index) = transaction.manifest.index.as_ref() {
        if let Err(error) =
            verify_visibility_index(codex_home, index.original_exists, &index.original_revision)
        {
            failures.push(error);
        }
    }
    if !failures.is_empty() {
        return Err(format!(
            "会话修复仍有 {} 个 surface 无法确认恢复。",
            failures.len()
        ));
    }
    transaction.manifest.state = VisibilityRepairTransactionState::RolledBack;
    write_visibility_repair_manifest(&transaction.directory, &transaction.manifest)?;
    let _ = fs::remove_dir_all(&transaction.directory);
    Ok(())
}

fn visibility_repair_counts(
    databases: &[VisibilityDatabasePlan],
) -> (usize, usize, usize) {
    let mut state_update_count = 0usize;
    let mut state_insert_count = 0usize;
    let mut catalog_update_count = 0usize;
    for database in databases {
        match database.kind {
            VisibilityDatabaseKind::State => {
                for row in &database.rows {
                    if row.original.is_some() {
                        state_update_count += 1;
                    } else {
                        state_insert_count += 1;
                    }
                }
            }
            VisibilityDatabaseKind::Catalog => catalog_update_count += database.rows.len(),
        }
    }
    (
        state_update_count,
        state_insert_count,
        catalog_update_count,
    )
}

fn execute_visibility_repair_transaction<Revalidate>(
    codex_home: &Path,
    transaction_root: &Path,
    prepared: PreparedVisibilityRepair,
    mut revalidate: Revalidate,
) -> Result<ThreadVisibilityRepairReport, String>
where
    Revalidate: FnMut() -> Result<(), String>,
{
    revalidate()?;
    let mut transaction =
        create_visibility_repair_transaction(transaction_root, codex_home, &prepared)?;
    let result = (|| -> Result<ThreadVisibilityRepairReport, String> {
        verify_visibility_repair_precondition(codex_home, &transaction)?;
        revalidate()?;
        for database in &transaction.manifest.databases {
            apply_visibility_database_plan(codex_home, database, true)?;
            revalidate()?;
        }
        if let Some(index) = transaction.manifest.index.as_ref() {
            let target = visibility_index_payload(&transaction, false)?;
            apply_visibility_index(
                codex_home,
                index.original_exists,
                &index.original_revision,
                index.target_exists,
                &index.target_revision,
                &target,
            )?;
            revalidate()?;
        }
        verify_visibility_repair_postcondition(codex_home, &transaction)?;
        revalidate()?;
        transaction.manifest.state = VisibilityRepairTransactionState::Committed;
        write_visibility_repair_manifest(&transaction.directory, &transaction.manifest)?;
        let (state_update_count, state_insert_count, catalog_update_count) =
            visibility_repair_counts(&transaction.manifest.databases);
        let (index_add_count, index_update_count) = transaction
            .manifest
            .index
            .as_ref()
            .map(|index| (index.add_count, index.update_count))
            .unwrap_or_default();
        let total = state_update_count
            + state_insert_count
            + catalog_update_count
            + index_add_count
            + index_update_count;
        Ok(ThreadVisibilityRepairReport {
            mode: transaction.manifest.mode.as_str().to_string(),
            repaired_session_count: transaction.manifest.repaired_session_count,
            state_update_count,
            state_insert_count,
            catalog_update_count,
            index_add_count,
            index_update_count,
            message: if total == 0 {
                "官方会话状态已一致，无需修改。".to_string()
            } else {
                format!("会话可见性已安全修复，共校正 {total} 处官方状态。")
            },
        })
    })();
    match result {
        Ok(report) => {
            let _ = fs::remove_dir_all(&transaction.directory);
            Ok(report)
        }
        Err(error) => match rollback_visibility_repair_transaction(codex_home, &mut transaction) {
            Ok(()) => Err(format!(
                "{error} 已自动恢复 state DB、catalog 与会话索引到预览前状态；请重新预览后再试。"
            )),
            Err(_) => Err(concat!(
                "会话可见性修复未能完成，且无法确认自动恢复结果。",
                "请暂时不要继续相关会话，并保留 QuotaHorizon 应用数据以便恢复。"
            )
            .to_string()),
        },
    }
}

fn recover_visibility_repair_transactions_with_revalidate<Revalidate>(
    codex_home: &Path,
    transaction_root: &Path,
    mut revalidate: Revalidate,
) -> Result<(), String>
where
    Revalidate: FnMut() -> Result<(), String>,
{
    if !transaction_root.exists() {
        return Ok(());
    }
    ensure_visibility_repair_directory(transaction_root)?;
    let mut entries = fs::read_dir(transaction_root)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if !file_type.is_dir() {
            return Err("会话修复事务目录包含意外文件。".to_string());
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(".capture-") {
            fs::remove_dir_all(entry.path())
                .map_err(|error| format!("无法清理未发布的会话修复恢复点：{error}"))?;
            continue;
        }
        let mut transaction = read_visibility_repair_transaction(codex_home, &entry.path())?;
        revalidate()?;
        match transaction.manifest.state {
            VisibilityRepairTransactionState::Prepared => {
                rollback_visibility_repair_transaction(codex_home, &mut transaction)?;
            }
            VisibilityRepairTransactionState::Committed
            | VisibilityRepairTransactionState::RolledBack => {
                fs::remove_dir_all(&transaction.directory)
                    .map_err(|error| format!("无法清理已结束的会话修复事务：{error}"))?;
            }
        }
        revalidate()?;
    }
    Ok(())
}

fn recover_incomplete_visibility_repair_transactions<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<(), String> {
    let transaction_root = visibility_repair_transaction_root(app)?;
    if !transaction_root.exists()
        || fs::read_dir(&transaction_root)
            .map_err(|error| format!("无法读取会话修复事务：{error}"))?
            .next()
            .is_none()
    {
        return Ok(());
    }
    let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "会话修复恢复安全互斥不可用。".to_string())?;
    let codex_home = resolve_paths(app)?.codex_home;
    recover_visibility_repair_transactions_with_revalidate(
        &codex_home,
        &transaction_root,
        || {
            lease
                .revalidate(&mut legacy_probe)
                .map_err(|_| "会话修复恢复期间安全互斥失效。".to_string())
        },
    )
}

pub(crate) fn prepare_codex_thread_visibility_repair_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    mode: String,
    session_ids: Option<Vec<String>>,
) -> Result<ThreadVisibilityRepairPreview, String> {
    recover_incomplete_visibility_repair_transactions(&app)?;
    let mode = VisibilityRepairMode::parse(mode.trim())?;
    let paths = resolve_paths(&app)?;
    let prepared = prepare_visibility_repair(&paths.codex_home, mode, session_ids)?;
    let (confirm_token, expires_at) = issue_visibility_repair_plan(&prepared)?;
    let (state_update_count, state_insert_count, catalog_update_count) =
        visibility_repair_counts(&prepared.databases);
    let (index_add_count, index_update_count) = prepared
        .index
        .as_ref()
        .map(|index| (index.add_count, index.update_count))
        .unwrap_or_default();
    let mut protected_targets = vec![ThreadVisibilityRepairProtectedTarget::RolloutSource];
    if prepared.databases.iter().any(|database| {
        database.kind == VisibilityDatabaseKind::State
            && database.database_relative.starts_with("sqlite/")
    }) {
        protected_targets.push(ThreadVisibilityRepairProtectedTarget::CanonicalState);
    }
    if prepared.databases.iter().any(|database| {
        database.kind == VisibilityDatabaseKind::State
            && !database.database_relative.starts_with("sqlite/")
    }) {
        protected_targets.push(ThreadVisibilityRepairProtectedTarget::LegacyState);
    }
    if catalog_update_count > 0 {
        protected_targets.push(ThreadVisibilityRepairProtectedTarget::ThreadCatalog);
    }
    if prepared.index.is_some() {
        protected_targets.push(ThreadVisibilityRepairProtectedTarget::SessionIndex);
    }
    Ok(ThreadVisibilityRepairPreview {
        confirm_token,
        expires_at: expires_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        mode: prepared.mode.as_str().to_string(),
        scope: if prepared.requested_session_ids.is_some() {
            "selected"
        } else {
            "all"
        }
        .to_string(),
        requested_count: prepared.requested_count,
        scanned_count: prepared.sources.len(),
        rollout_guard_count: prepared.rollout_guards.len(),
        state_update_count,
        state_insert_count,
        catalog_update_count,
        index_add_count,
        index_update_count,
        protected_targets,
        rollout_source_unchanged: true,
        automatic_rollback: true,
        crash_recovery: true,
    })
}

pub(crate) fn confirm_codex_thread_visibility_repair_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<ThreadVisibilityRepairReport, String> {
    let plan = consume_visibility_repair_plan(&confirm_token)?;
    let (lease, mut legacy_probe) = crate::commands::acquire_shared_mutation_lease()?;
    lease
        .revalidate(&mut legacy_probe)
        .map_err(|_| "会话修复安全互斥在执行前失效，未修改任何内容。".to_string())?;
    let paths = resolve_paths(&app)?;
    let transaction_root = visibility_repair_transaction_root(&app)?;
    recover_visibility_repair_transactions_with_revalidate(
        &paths.codex_home,
        &transaction_root,
        || {
            lease
                .revalidate(&mut legacy_probe)
                .map_err(|_| "会话修复恢复期间安全互斥失效。".to_string())
        },
    )?;
    let prepared = prepare_visibility_repair(
        &paths.codex_home,
        plan.mode,
        plan.session_ids,
    )?;
    if prepared.revision != plan.expected_revision {
        return Err(concat!(
            "会话 rollout、state DB、catalog 或索引在预览后发生变化，",
            "未修改任何内容。请重新预览。"
        )
        .to_string());
    }
    execute_visibility_repair_transaction(
        &paths.codex_home,
        &transaction_root,
        prepared,
        || {
            lease
                .revalidate(&mut legacy_probe)
                .map_err(|_| "会话修复安全互斥失效。".to_string())
        },
    )
}

pub(crate) fn open_codex_thread_file_blocking<R: Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    folder_only: bool,
) -> Result<(), String> {
    let codex_home = resolve_paths(&app)?.codex_home;
    let snapshot = gather_snapshots(&codex_home)?
        .into_iter()
        .find(|item| item.session_id == session_id)
        .ok_or_else(|| "未找到所选会话文件".to_string())?;
    let path = if folder_only {
        snapshot.path.parent().unwrap_or(&codex_home).to_path_buf()
    } else {
        snapshot.path
    };
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|error| format!("无法打开 {}：{error}", path.display()))
}
