use super::*;
pub use capacity_store::PlanningArchiveQuery;
pub use persistence::planning_archive::DesktopPlanningArchiveEnvelope;

pub async fn planning_archive_for_status(
    app: AppHandle,
    context: String,
    query: PlanningArchiveQuery,
    status: DesktopStatusEnvelope,
) -> Result<DesktopPlanningArchiveEnvelope, DesktopIssue> {
    let state = app.state::<DesktopState>();
    let _command = state.command_gate.lock().await;
    let persistence = Arc::clone(&state.persistence);
    let now = SystemClock.now();
    tokio::task::spawn_blocking(move || match persistence.lock() {
        Ok(p) => p.planning_archive(&context, query, &status, &now),
        Err(_) => DesktopPlanningArchiveEnvelope::unavailable(
            &context,
            query,
            "plan_store_unavailable",
            &now,
        ),
    })
    .await
    .map_err(|_| desktop_issue("desktop_state_failed"))
}
