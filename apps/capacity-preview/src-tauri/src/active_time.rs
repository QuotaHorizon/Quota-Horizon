use super::*;
pub use persistence::active_time::{
    DesktopActiveTimeEnvelope, DesktopActiveTimeQuotaEnvelope, DesktopActiveTimeUpdate,
};

pub async fn active_time_quota_for_status(
    app: AppHandle,
    context: String,
    observation_id: String,
    status: DesktopStatusEnvelope,
) -> Result<DesktopActiveTimeQuotaEnvelope, DesktopIssue> {
    let state = app.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    let persistence = Arc::clone(&state.persistence);
    let now = SystemClock.now();
    tokio::task::spawn_blocking(move || {
        persistence.lock().map_or_else(
            |_| {
                DesktopActiveTimeQuotaEnvelope::unavailable(
                    &context,
                    &observation_id,
                    "activity_store_unavailable",
                    &now,
                )
            },
            |p| p.active_time_quota(&context, &observation_id, &status, &now),
        )
    })
    .await
    .map_err(|_| desktop_issue("desktop_state_failed"))
}

pub async fn active_time_for_status(
    app: AppHandle,
    context: String,
    request: Option<DesktopActiveTimeUpdate>,
    status: DesktopStatusEnvelope,
) -> Result<DesktopActiveTimeEnvelope, DesktopIssue> {
    let state = app.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    let persistence = Arc::clone(&state.persistence);
    let now = SystemClock.now();
    tokio::task::spawn_blocking(move || {
        persistence.lock().map_or_else(
            |_| {
                DesktopActiveTimeEnvelope::unavailable(&context, "activity_store_unavailable", &now)
            },
            |mut p| p.active_time(&context, request, &status, &now),
        )
    })
    .await
    .map_err(|_| desktop_issue("desktop_state_failed"))
}
