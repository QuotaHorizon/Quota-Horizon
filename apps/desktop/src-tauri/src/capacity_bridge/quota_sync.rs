use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Mutex,
};
use std::time::Duration;

use capacity_desktop_service::{
    DesktopQuotaObservation, DesktopQuotaObservationWindow, DesktopStatusEnvelope,
};
use chrono::{DateTime, SecondsFormat, Utc};
use tauri::{AppHandle, Emitter, Listener, Manager};

use crate::models::{AccountSummary, UsageSummary, UsageWindow};
use crate::storage::{load_usage, resolve_paths, save_usage_if_newer, usage_path, Paths};

pub(crate) const STATUS_EVENT: &str = "capacity-status-changed";
const BACKGROUND_INTERVAL: Duration = Duration::from_secs(30);

pub(super) struct QuotaSyncState {
    pub gate: tauri::async_runtime::Mutex<()>,
    pub refresh_revision: AtomicU64,
    sequence: AtomicU64,
    retained: Mutex<Option<(String, DesktopStatusEnvelope)>>,
    stop: Mutex<Option<mpsc::Sender<()>>>,
}

pub(crate) fn setup(app: &AppHandle) -> Result<(), String> {
    let (stop, receiver) = mpsc::channel();
    app.manage(QuotaSyncState {
        gate: tauri::async_runtime::Mutex::new(()),
        refresh_revision: AtomicU64::new(0),
        sequence: AtomicU64::new(0),
        retained: Mutex::new(None),
        stop: Mutex::new(Some(stop)),
    });
    // Both readers publish through the same host reconciliation boundary. The
    // outgoing event is distinct so a status read cannot trigger a refresh loop.
    for event in ["monitor-status", "accounts-changed"] {
        let app = app.clone();
        let listener_app = app.clone();
        app.listen_any(event, move |_| {
            let app = listener_app.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(envelope) = super::capacity_get_status(app.clone()).await {
                    let _ = app.emit(STATUS_EVENT, envelope);
                }
            });
        });
    }
    let opened_app = app.clone();
    app.listen_any("capacity-popover-opened", move |_| {
        let app = opened_app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = super::capacity_refresh_status(app).await;
        });
    });
    let app = app.clone();
    std::thread::Builder::new()
        .name("horizon-quota-refresh".to_owned())
        .spawn(move || loop {
            let result =
                tauri::async_runtime::block_on(super::capacity_refresh_status(app.clone()));
            let delay = if result
                .as_ref()
                .ok()
                .and_then(|envelope| envelope.live_quota_observation())
                .is_some()
            {
                BACKGROUND_INTERVAL
            } else {
                Duration::from_secs(60)
            };
            match receiver.recv_timeout(delay) {
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                _ => break,
            }
        })
        .map_err(|_| "Unable to start quota background refresh".to_owned())?;
    Ok(())
}

pub(crate) fn shutdown<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(state) = app.try_state::<QuotaSyncState>() {
        if let Ok(mut stop) = state.stop.lock() {
            if let Some(stop) = stop.take() {
                let _ = stop.send(());
            }
        }
    }
}

fn active_account(app: &AppHandle, expected_digest: &str) -> Option<AccountSummary> {
    let paths = resolve_paths(app).ok()?;
    if super::capacity_account_digest_for_paths(&paths)
        .ok()
        .flatten()
        .as_deref()
        != Some(expected_digest)
    {
        return None;
    }
    crate::commands::list_accounts_blocking(app.clone())
        .ok()?
        .into_iter()
        .find(|account| account.id == expected_digest)
}

fn observation_from_usage(usage: &UsageSummary, plan: &str) -> Option<DesktopQuotaObservation> {
    let plan = usage
        .plan
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(plan);
    let (short, weekly) = crate::system_tray::account_usage_windows(usage, plan);
    let windows = [short, weekly]
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, window)| DesktopQuotaObservationWindow {
            limit_id: if index == 0 {
                "codex:primary"
            } else {
                "codex:secondary"
            }
            .to_owned(),
            window_minutes: window
                .window_minutes
                .and_then(|value| u64::try_from(value).ok()),
            remaining_percent: window.remaining_percent,
            resets_at: window
                .resets_at
                .and_then(|value| DateTime::<Utc>::from_timestamp(value, 0))
                .map(|value| value.to_rfc3339_opts(SecondsFormat::Millis, true)),
        })
        .collect();
    let observation = DesktopQuotaObservation {
        captured_at: usage.fetched_at.clone()?,
        plan_type: Some(plan.to_owned()),
        windows,
    };
    observation.is_valid().then_some(observation)
}

pub(super) fn startup_snapshot_for_paths(paths: &Paths) -> Option<DesktopStatusEnvelope> {
    let (digest, plan) = super::capacity_account_metadata_for_paths(paths).ok()??;
    let usage = load_usage(&usage_path(paths, &digest));
    // Recheck after the disk read: an external login or switch invalidates it.
    if super::capacity_account_digest_for_paths(paths)
        .ok()
        .flatten()
        .as_deref()
        != Some(digest.as_str())
    {
        return None;
    }
    DesktopStatusEnvelope::from_cached_quota(&observation_from_usage(&usage, &plan)?)
}

fn retained_for_identity(
    retained: &Option<(String, DesktopStatusEnvelope)>,
    identity: Option<&str>,
) -> Option<DesktopStatusEnvelope> {
    let (owner, envelope) = retained.as_ref()?;
    (identity == Some(owner.as_str())).then(|| envelope.clone().into_cached_snapshot())
}

pub(super) fn cached_snapshot(app: &AppHandle) -> Option<DesktopStatusEnvelope> {
    let paths = resolve_paths(app).ok()?;
    let identity = super::capacity_account_digest_for_paths(&paths).ok()?;
    let retained = app
        .state::<QuotaSyncState>()
        .retained
        .lock()
        .ok()
        .and_then(|value| retained_for_identity(&value, identity.as_deref()));
    if super::capacity_account_digest_for_paths(&paths).ok()? != identity {
        return None;
    }
    retained.or_else(|| startup_snapshot_for_paths(&paths))
}

fn usage_from_observation(
    mut cached: UsageSummary,
    observation: &DesktopQuotaObservation,
) -> UsageSummary {
    let convert = |window: &DesktopQuotaObservationWindow| UsageWindow {
        used_percent: 100.0 - window.remaining_percent,
        remaining_percent: window.remaining_percent,
        resets_at: window
            .resets_at
            .as_ref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.timestamp()),
        window_minutes: window
            .window_minutes
            .and_then(|value| i64::try_from(value).ok()),
    };
    let mut windows = observation.windows.iter().collect::<Vec<_>>();
    windows.sort_by_key(|window| window.window_minutes.unwrap_or(u64::MAX));
    cached.primary = windows.first().map(|window| convert(window));
    cached.secondary = windows.get(1).map(|window| convert(window));
    cached.fetched_at = Some(observation.captured_at.clone());
    if observation.plan_type.is_some() {
        cached.plan = observation.plan_type.clone();
    }
    cached.error = None;
    cached
}

/// Called under the host gate after the source read, with the identity captured
/// before it. A switch during the request must not update another account.
pub(super) fn reconcile(
    app: &AppHandle,
    mut envelope: DesktopStatusEnvelope,
    expected_digest: Option<&str>,
) -> DesktopStatusEnvelope {
    if let Some(account) = expected_digest.and_then(|digest| active_account(app, digest)) {
        if let Ok(paths) = resolve_paths(app) {
            let file = usage_path(&paths, &account.id);
            let mut usage = load_usage(&file);
            if let Some(observation) = envelope.live_quota_observation() {
                let incoming = usage_from_observation(usage.clone(), &observation);
                match save_usage_if_newer(&file, &incoming, &observation.captured_at) {
                    Ok((saved, changed)) => {
                        usage = saved;
                        if changed {
                            let _ = app.emit("accounts-changed", ());
                        }
                    }
                    Err(_) => eprintln!("Unable to synchronize the active quota cache"),
                }
            }
            envelope = envelope
                .reconcile_managed_quota(observation_from_usage(&usage, &account.plan).as_ref());
        }
    }
    let state = app.state::<QuotaSyncState>();
    envelope.set_host_sequence(state.sequence.fetch_add(1, Ordering::SeqCst) + 1);
    if let Ok(mut retained) = state.retained.lock() {
        let current = resolve_paths(app).ok().and_then(|paths| {
            super::capacity_account_digest_for_paths(&paths)
                .ok()
                .flatten()
        });
        *retained = expected_digest
            .filter(|expected| current.as_deref() == Some(*expected))
            .map(|owner| (owner.to_owned(), envelope.clone()));
    }
    crate::system_tray::update_capacity_status(app, &envelope);
    envelope
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_snapshot_is_available_only_for_the_same_known_account() {
        let observation = DesktopQuotaObservation {
            captured_at: "2026-09-26T00:00:00Z".into(),
            plan_type: Some("pro".into()),
            windows: vec![DesktopQuotaObservationWindow {
                limit_id: "codex:primary".into(),
                window_minutes: Some(10080),
                remaining_percent: 28.0,
                resets_at: None,
            }],
        };
        let mut envelope = DesktopStatusEnvelope::from_cached_quota(&observation).unwrap();
        envelope.set_host_sequence(12);
        let retained = Some(("account-a".into(), envelope));
        assert!(retained_for_identity(&retained, None).is_none());
        assert!(retained_for_identity(&retained, Some("account-b")).is_none());
        assert!(retained_for_identity(&None, Some("account-a")).is_none());
        let cached = retained_for_identity(&retained, Some("account-a")).unwrap();
        let json = serde_json::to_value(cached).unwrap();
        assert_eq!(json["sequence"], 0);
        assert_eq!(json["status"]["quotaWindows"][0]["remainingPercent"], 28.0);
        assert_eq!(json["status"]["dataStatus"]["freshness"], "stale");
    }

    #[test]
    fn weekly_only_account_round_trips_without_inventing_a_short_window() {
        let original = UsageSummary {
            primary: Some(UsageWindow {
                remaining_percent: 28.0,
                used_percent: 72.0,
                window_minutes: Some(10080),
                resets_at: Some(1789257600),
            }),
            fetched_at: Some("2026-09-05T00:01:00Z".to_owned()),
            ..Default::default()
        };
        let observation = observation_from_usage(&original, "pro").unwrap();
        let usage = usage_from_observation(UsageSummary::default(), &observation);
        assert_eq!(usage.primary.unwrap().remaining_percent, 28.0);
        assert!(usage.secondary.is_none());
        assert_eq!(observation.windows.len(), 1);
    }

    #[test]
    fn startup_uses_the_saved_quota_plan_instead_of_stale_login_metadata() {
        let mut usage = UsageSummary {
            primary: Some(UsageWindow {
                remaining_percent: 72.0,
                used_percent: 28.0,
                window_minutes: Some(10080),
                resets_at: None,
            }),
            plan: Some("prolite".into()),
            fetched_at: Some("2026-10-02T16:50:00Z".into()),
            ..Default::default()
        };
        let observation = observation_from_usage(&usage, "free").unwrap();
        assert_eq!(observation.plan_type.as_deref(), Some("prolite"));
        let snapshot = DesktopStatusEnvelope::from_cached_quota(&observation).unwrap();
        assert_eq!(
            serde_json::to_value(snapshot).unwrap()["status"]["account"]["planType"],
            "prolite"
        );
        usage.plan = Some("free".into());
        assert_eq!(
            observation_from_usage(&usage, "pro")
                .unwrap()
                .plan_type
                .as_deref(),
            Some("free")
        );
        usage.plan = None;
        assert_eq!(
            observation_from_usage(&usage, "plus")
                .unwrap()
                .plan_type
                .as_deref(),
            Some("plus")
        );
    }
}
