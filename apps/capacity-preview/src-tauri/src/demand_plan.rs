use super::*;
pub use persistence::demand::{DesktopDemandPlanEnvelope, DesktopDemandPlanUpdate};

pub async fn demand_plan_for_status(
    app: AppHandle,
    context: String,
    request: Option<DesktopDemandPlanUpdate>,
    status: DesktopStatusEnvelope,
) -> Result<DesktopDemandPlanEnvelope, DesktopIssue> {
    let state = app.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    let persistence = Arc::clone(&state.persistence);
    let now = SystemClock.now();
    tokio::task::spawn_blocking(move || {
        persistence.lock().map_or_else(
            |_| DesktopDemandPlanEnvelope::unavailable(&context, "plan_store_unavailable", &now),
            |mut persistence| persistence.demand_plan(&context, request, &status, &now),
        )
    })
    .await
    .map_err(|_| desktop_issue("desktop_state_failed"))
}
