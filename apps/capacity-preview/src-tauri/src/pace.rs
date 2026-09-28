use super::*;
pub use persistence::pace::DesktopPaceEnvelope;
pub use persistence::pace::DesktopPaceTrialsEnvelope;

pub async fn pace_trials_for_status(
    app: AppHandle,
    context: String,
    status: DesktopStatusEnvelope,
    offset: u32,
) -> Result<DesktopPaceTrialsEnvelope, DesktopIssue> {
    let state = app.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    let persistence = Arc::clone(&state.persistence);
    tokio::task::spawn_blocking(move || match persistence.lock() {
        Ok(p) => p.pace_trials(&context, &status, offset),
        Err(_) => {
            DesktopPaceTrialsEnvelope::unavailable(&context, offset, "plan_store_unavailable")
        }
    })
    .await
    .map_err(|_| desktop_issue("desktop_state_failed"))
}

pub async fn pace_for_status(
    app: AppHandle,
    context: String,
    status: DesktopStatusEnvelope,
) -> Result<DesktopPaceEnvelope, DesktopIssue> {
    let state = app.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    let persistence = Arc::clone(&state.persistence);
    let now = SystemClock.now();
    tokio::task::spawn_blocking(move || match persistence.lock() {
        Ok(p) => p.pace(&context, &status, &now),
        Err(_) => DesktopPaceEnvelope::unavailable(&context, "plan_store_unavailable", &now),
    })
    .await
    .map_err(|_| desktop_issue("desktop_state_failed"))
}
