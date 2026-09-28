use std::time::Duration;

use capacity_domain::{
    MonitorNotificationDecision, MonitorNotificationKind, NotificationDelivery, NotificationPolicy,
    NotificationPrivacy, QuietHours,
};
use capacity_store::MonitorSettings;
use chrono::{Local, Timelike};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{broadcast, watch};

use crate::{DesktopState, refresh_status_inner};

pub(crate) const MONITOR_STATUS_EVENT: &str = "monitor-status";
const MONITOR_NOTIFICATION_EVENT: &str = "monitor-notification";
const NOTIFICATION_COOLDOWN_MINUTES: u16 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutoRefreshSchedule {
    Disabled,
    Every(Duration),
}

impl AutoRefreshSchedule {
    pub(crate) fn from_settings(settings: Option<&MonitorSettings>) -> Self {
        match settings {
            Some(settings) if settings.auto_refresh_enabled => Self::Every(Duration::from_secs(
                u64::from(settings.refresh_interval_seconds),
            )),
            Some(_) | None => Self::Disabled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MonitorCommand {
    Reconfigure(u64),
    Shutdown,
}

pub(crate) struct MonitorControl {
    sender: watch::Sender<MonitorCommand>,
}

impl MonitorControl {
    pub(crate) fn new() -> Self {
        let (sender, _) = watch::channel(MonitorCommand::Reconfigure(0));
        Self { sender }
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<MonitorCommand> {
        self.sender.subscribe()
    }

    pub(crate) fn reconfigure(&self) {
        let generation = match *self.sender.borrow() {
            MonitorCommand::Reconfigure(generation) => generation.saturating_add(1),
            MonitorCommand::Shutdown => return,
        };
        self.sender
            .send_replace(MonitorCommand::Reconfigure(generation));
    }

    pub(crate) fn shutdown(&self) {
        self.sender.send_replace(MonitorCommand::Shutdown);
    }
}

pub(crate) async fn run_auto_refresh(
    app_handle: AppHandle,
    mut commands: watch::Receiver<MonitorCommand>,
) {
    loop {
        if *commands.borrow_and_update() == MonitorCommand::Shutdown {
            return;
        }
        let schedule = {
            let state = app_handle.state::<DesktopState>();
            state.auto_refresh_schedule().await
        };
        let AutoRefreshSchedule::Every(interval) = schedule else {
            if commands.changed().await.is_err() {
                return;
            }
            continue;
        };

        tokio::select! {
            () = tokio::time::sleep(interval) => {
                let envelope = {
                    let state = app_handle.state::<DesktopState>();
                    let _command = state.command_gate.lock().await;
                    if state.auto_refresh_schedule().await != schedule {
                        None
                    } else {
                        Some(state.stamp_status(refresh_status_inner(&state).await))
                    }
                };
                if let Some(envelope) = envelope {
                    crate::tray::update(&app_handle, &envelope);
                    let _ = app_handle.emit(MONITOR_STATUS_EVENT, envelope);
                }
            }
            changed = commands.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

pub(crate) async fn run_notification_bridge(
    app_handle: AppHandle,
    mut notifications: broadcast::Receiver<MonitorNotificationDecision>,
) {
    loop {
        match notifications.recv().await {
            Ok(decision) => {
                let event = DesktopNotificationEvent::from(decision);
                let _ = app_handle.emit(MONITOR_NOTIFICATION_EVENT, event);
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

pub(crate) fn notification_policy(settings: &MonitorSettings) -> Option<NotificationPolicy> {
    let quiet_hours = if settings.quiet_hours_enabled {
        Some(
            QuietHours::new(
                settings.quiet_hours_start_minute?,
                settings.quiet_hours_end_minute?,
            )
            .ok()?,
        )
    } else {
        None
    };
    NotificationPolicy::new(
        settings.notification_threshold_basis_points,
        settings.reset_credit_notice_hours,
        quiet_hours,
        settings.lock_screen_privacy,
        NOTIFICATION_COOLDOWN_MINUTES,
    )
    .ok()
}

pub(crate) fn current_local_minute() -> u16 {
    let local = Local::now();
    u16::try_from(local.hour() * 60 + local.minute()).unwrap_or(0)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopNotificationEvent {
    schema_version: &'static str,
    template_id: &'static str,
    observed_at: String,
    details: Option<DesktopNotificationDetails>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopNotificationDetails {
    affected_window_count: Option<usize>,
    lowest_remaining_basis_points: Option<u16>,
    reset_credit_count: Option<usize>,
    earliest_expires_at: Option<String>,
}

impl From<MonitorNotificationDecision> for DesktopNotificationEvent {
    fn from(decision: MonitorNotificationDecision) -> Self {
        let observed_at = decision.observed_at.as_str().to_owned();
        if decision.privacy == NotificationPrivacy::Generic {
            return Self {
                schema_version: "1.0",
                template_id: "capacity_state_changed",
                observed_at,
                details: None,
            };
        }
        let (template_id, details) = match decision.kind {
            MonitorNotificationKind::QuotaThresholdCrossed { windows } => {
                let lowest = windows
                    .iter()
                    .map(|window| window.remaining_basis_points)
                    .min();
                (
                    "quota_threshold_crossed",
                    Some(DesktopNotificationDetails {
                        affected_window_count: Some(windows.len()),
                        lowest_remaining_basis_points: lowest,
                        reset_credit_count: None,
                        earliest_expires_at: None,
                    }),
                )
            }
            MonitorNotificationKind::ResetCreditsExpiring {
                credit_count,
                earliest_expires_at,
            } => (
                "reset_credits_expiring",
                Some(DesktopNotificationDetails {
                    affected_window_count: None,
                    lowest_remaining_basis_points: None,
                    reset_credit_count: Some(credit_count),
                    earliest_expires_at: Some(earliest_expires_at.as_str().to_owned()),
                }),
            ),
            MonitorNotificationKind::LiveRestored => ("live_status_restored", None),
        };
        Self {
            schema_version: "1.0",
            template_id,
            observed_at,
            details,
        }
    }
}

pub(crate) fn publish_deliverable(
    sender: &broadcast::Sender<MonitorNotificationDecision>,
    decisions: Vec<MonitorNotificationDecision>,
) {
    for decision in decisions {
        if decision.delivery == NotificationDelivery::Deliver {
            let _ = sender.send(decision);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capacity_domain::{
        MonitorNotificationKind, NotificationPrivacy, QuotaThresholdCrossing, UtcTimestamp,
    };
    use capacity_store::SettingsLanguage;

    fn settings(auto_refresh_enabled: bool) -> MonitorSettings {
        MonitorSettings {
            revision: 1,
            auto_refresh_enabled,
            refresh_interval_seconds: 300,
            notification_threshold_basis_points: Some(2_000),
            reset_credit_notice_hours: 48,
            quiet_hours_enabled: true,
            quiet_hours_start_minute: Some(22 * 60),
            quiet_hours_end_minute: Some(7 * 60),
            language: SettingsLanguage::System,
            lock_screen_privacy: true,
            launch_at_login: false,
            history_retention_days: 180,
            updated_at: UtcTimestamp::parse("2026-08-30T00:00:00Z").unwrap(),
        }
    }

    fn notification_fixture(name: &str) -> serde_json::Value {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../fixtures/desktop/v1")
                .join(name),
        )
        .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn schedule_uses_persisted_auto_refresh_mode() {
        assert_eq!(
            AutoRefreshSchedule::from_settings(Some(&settings(true))),
            AutoRefreshSchedule::Every(Duration::from_secs(300))
        );
        assert_eq!(
            AutoRefreshSchedule::from_settings(Some(&settings(false))),
            AutoRefreshSchedule::Disabled
        );
        assert_eq!(
            AutoRefreshSchedule::from_settings(None),
            AutoRefreshSchedule::Disabled
        );
    }

    #[test]
    fn monitor_control_wakes_on_reconfigure_and_stays_shutdown() {
        let control = MonitorControl::new();
        let receiver = control.subscribe();
        control.reconfigure();
        assert_eq!(*receiver.borrow(), MonitorCommand::Reconfigure(1));
        control.shutdown();
        assert_eq!(*receiver.borrow(), MonitorCommand::Shutdown);
        control.reconfigure();
        assert_eq!(*receiver.borrow(), MonitorCommand::Shutdown);
    }

    #[test]
    fn generic_notification_event_drops_exact_capacity_details() {
        let event = DesktopNotificationEvent::from(MonitorNotificationDecision {
            kind: MonitorNotificationKind::QuotaThresholdCrossed {
                windows: vec![QuotaThresholdCrossing {
                    limit_id: "codex:primary".to_owned(),
                    remaining_basis_points: 1_234,
                }],
            },
            delivery: NotificationDelivery::Deliver,
            privacy: NotificationPrivacy::Generic,
            observed_at: UtcTimestamp::parse("2026-08-30T00:00:00Z").unwrap(),
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("capacity_state_changed"));
        assert!(!json.contains("codex:primary"));
        assert!(!json.contains("1234"));
        assert!(event.details.is_none());
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            notification_fixture("notification-generic-event.json")
        );
    }

    #[test]
    fn detailed_notification_event_is_aggregate_only() {
        let event = DesktopNotificationEvent::from(MonitorNotificationDecision {
            kind: MonitorNotificationKind::QuotaThresholdCrossed {
                windows: vec![QuotaThresholdCrossing {
                    limit_id: "codex:primary".to_owned(),
                    remaining_basis_points: 1_234,
                }],
            },
            delivery: NotificationDelivery::Deliver,
            privacy: NotificationPrivacy::Detailed,
            observed_at: UtcTimestamp::parse("2026-08-30T00:00:00Z").unwrap(),
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("quota_threshold_crossed"));
        assert!(json.contains("1234"));
        assert!(!json.contains("codex:primary"));
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            notification_fixture("notification-threshold-event.json")
        );
    }
}
