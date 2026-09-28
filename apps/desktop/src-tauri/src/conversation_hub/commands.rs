#[tauri::command]
pub(crate) async fn get_codex_thread_revision<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || get_codex_thread_revision_blocking(app))
        .await.map_err(|_| "Unable to check local session changes".to_owned())?
}

#[tauri::command]
pub(crate) async fn search_codex_threads<R: Runtime + 'static>(
    app: tauri::AppHandle<R>, query: String, client_id: String, request_id: u32,
) -> Result<ThreadSearchResult, String> {
    let cancelled = begin_thread_search(&client_id, request_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        start_thread_search_blocking(app, query, client_id, request_id, cancelled)
    }).await.map_err(|_| "Unable to search local sessions".to_owned())?
}

#[tauri::command]
pub(crate) async fn continue_codex_thread_search(client_id: String, request_id: u32, continuation: String) -> Result<ThreadSearchResult, String> {
    tauri::async_runtime::spawn_blocking(move || continue_codex_thread_search_blocking(client_id, request_id, continuation))
        .await.map_err(|_| "Unable to continue local session search".to_owned())?
}

#[tauri::command]
pub(crate) fn cancel_codex_thread_search(client_id: String, request_id: u32) -> Result<(), String> {
    cancel_codex_thread_search_blocking(&client_id, request_id)
}

#[tauri::command]
pub(crate) async fn browse_codex_threads<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    title_query: Option<String>,
    content_query: Option<String>,
) -> Result<Vec<ThreadEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        browse_codex_threads_blocking(app, title_query, content_query)
    })
    .await
    .map_err(|error| format!("Browse conversations task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn measure_codex_thread_tokens<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    session_ids: Vec<String>,
) -> Result<Vec<ThreadTokenTotals>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        measure_codex_thread_tokens_blocking(app, session_ids)
    })
    .await
    .map_err(|error| format!("Measure conversation tokens task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn inspect_codex_thread_detail<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    session_id: String,
    offset: Option<usize>,
    limit: Option<usize>,
    expected_revision: Option<String>,
    from_end: Option<bool>,
) -> Result<ThreadDetailPage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        inspect_codex_thread_detail_blocking(app, session_id, offset, limit, expected_revision, from_end)
    })
    .await
    .map_err(|error| format!("Inspect conversation detail task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn resume_codex_thread<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    session_id: String,
    target_cwd: Option<String>,
) -> Result<ThreadResumeResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        resume_codex_thread_blocking(app, session_id, target_cwd)
    })
    .await
    .map_err(|error| format!("Resume conversation task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn prepare_codex_thread_rebind<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    session_id: String,
    target_cwd: String,
) -> Result<ThreadRebindPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_codex_thread_rebind_blocking(app, session_id, target_cwd)
    })
    .await
    .map_err(|error| format!("Prepare conversation directory change task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn confirm_codex_thread_rebind<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<ThreadRebindReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        confirm_codex_thread_rebind_blocking(app, confirm_token)
    })
    .await
    .map_err(|error| format!("Confirm conversation directory change task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn prepare_codex_thread_discard<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    session_ids: Vec<String>,
) -> Result<ThreadTrashPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_codex_thread_discard_blocking(app, session_ids)
    })
    .await
    .map_err(|error| format!("Conversation trash preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn confirm_codex_thread_discard<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<MutationReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        confirm_codex_thread_discard_blocking(app, confirm_token)
    })
    .await
    .map_err(|error| format!("Conversation trash confirmation task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn browse_codex_thread_bin<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
) -> Result<Vec<BinEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || browse_codex_thread_bin_blocking(app))
        .await
        .map_err(|error| format!("Browse conversation bin task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn prepare_codex_thread_restore<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    session_ids: Vec<String>,
) -> Result<ThreadRestorePreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_codex_thread_restore_blocking(app, session_ids)
    })
    .await
    .map_err(|error| format!("Conversation restore preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn confirm_codex_thread_restore<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<MutationReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        confirm_codex_thread_restore_blocking(app, confirm_token)
    })
    .await
    .map_err(|error| format!("Conversation restore confirmation task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn prepare_codex_thread_purge<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    session_ids: Vec<String>,
    empty_bin: bool,
) -> Result<ThreadPurgePreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_codex_thread_purge_blocking(app, session_ids, empty_bin)
    })
    .await
    .map_err(|error| format!("Conversation purge preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn confirm_codex_thread_purge<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<ThreadPurgeReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        confirm_codex_thread_purge_blocking(app, confirm_token)
    })
    .await
    .map_err(|error| format!("Conversation purge confirmation task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn prepare_codex_thread_archive<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    session_ids: Vec<String>,
) -> Result<ThreadArchivePreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_codex_thread_archive_blocking(app, session_ids)
    })
    .await
    .map_err(|error| format!("Conversation archive preview task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn confirm_codex_thread_archive<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<MutationReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        confirm_codex_thread_archive_blocking(app, confirm_token)
    })
    .await
    .map_err(|error| format!("Conversation archive confirmation task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn inspect_codex_thread_export<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    session_ids: Vec<String>,
) -> Result<BundlePreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        inspect_codex_thread_export_blocking(app, session_ids)
    })
    .await
    .map_err(|error| format!("Inspect conversation export task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn pack_codex_threads<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    session_ids: Vec<String>,
    export_path: String,
) -> Result<BundleResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        pack_codex_threads_blocking(app, session_ids, export_path)
    })
    .await
    .map_err(|error| format!("Pack conversations task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn inspect_codex_thread_import<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    import_path: String,
) -> Result<BundlePreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        inspect_codex_thread_import_blocking(app, import_path)
    })
    .await
    .map_err(|error| format!("Inspect conversation import task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn unpack_codex_threads<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    import_path: String,
    session_ids: Vec<String>,
) -> Result<BundleResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        unpack_codex_threads_blocking(app, import_path, session_ids)
    })
    .await
    .map_err(|error| format!("Unpack conversations task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn migrate_codex_threads<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    session_ids: Vec<String>,
) -> Result<MigrationReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        migrate_codex_threads_blocking(app, session_ids)
    })
    .await
    .map_err(|error| format!("Migrate conversations task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn prepare_codex_thread_visibility_repair<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    mode: String,
    session_ids: Option<Vec<String>>,
) -> Result<ThreadVisibilityRepairPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_codex_thread_visibility_repair_blocking(app, mode, session_ids)
    })
    .await
    .map_err(|error| format!("Prepare conversation visibility repair task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn confirm_codex_thread_visibility_repair<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    confirm_token: String,
) -> Result<ThreadVisibilityRepairReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        confirm_codex_thread_visibility_repair_blocking(app, confirm_token)
    })
    .await
    .map_err(|error| format!("Confirm conversation visibility repair task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn open_codex_thread_file<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    session_id: String,
    folder_only: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        open_codex_thread_file_blocking(app, session_id, folder_only)
    })
    .await
    .map_err(|error| format!("Open conversation file task failed: {error}"))?
}
