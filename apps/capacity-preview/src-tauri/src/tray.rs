use capacity_domain::{Availability, Compatibility, Freshness};
use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItem, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Emitter, Manager, Wry};

use crate::{
    DesktopLifecycle, DesktopQuotaWindowView, DesktopState, DesktopStatusEnvelope,
    get_status_inner, refresh_status_inner,
};

const TRAY_ID: &str = "capacity-tray";
const STATUS_ITEM_ID: &str = "capacity-tray-status";
const OPEN_ITEM_ID: &str = "capacity-tray-open";
const REFRESH_ITEM_ID: &str = "capacity-tray-refresh";
const QUIT_ITEM_ID: &str = "capacity-tray-quit";

struct TrayState {
    status_item: MenuItem<Wry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrayPresentation {
    title: String,
    menu_text: String,
    tooltip: String,
}

pub(crate) fn install(app: &mut App) -> tauri::Result<()> {
    let status_item = MenuItemBuilder::with_id(STATUS_ITEM_ID, "Capacity unavailable")
        .enabled(false)
        .build(app)?;
    let open_item = MenuItemBuilder::with_id(OPEN_ITEM_ID, "Open Dashboard").build(app)?;
    let refresh_item = MenuItemBuilder::with_id(REFRESH_ITEM_ID, "Refresh Now").build(app)?;
    let quit_item = MenuItemBuilder::with_id(QUIT_ITEM_ID, "Quit").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&status_item)
        .separator()
        .item(&open_item)
        .item(&refresh_item)
        .separator()
        .item(&quit_item)
        .build()?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_icon_image())
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Codex Capacity Planner · Capacity unavailable")
        .title("—")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app_handle, event| match event.id() {
            id if id == OPEN_ITEM_ID => show_main_window(app_handle),
            id if id == REFRESH_ITEM_ID => refresh_from_tray(app_handle),
            id if id == QUIT_ITEM_ID => app_handle.exit(0),
            _ => {}
        })
        .build(app)?;
    app.manage(TrayState { status_item });
    Ok(())
}

pub(crate) fn update(app_handle: &AppHandle, envelope: &DesktopStatusEnvelope) {
    let presentation = tray_presentation(envelope);
    if let Some(state) = app_handle.try_state::<TrayState>() {
        let _ = state.status_item.set_text(&presentation.menu_text);
    }
    if let Err(error) = crate::with_tray_on_main_thread(app_handle, TRAY_ID, move |tray| {
        let _ = tray.set_title(Some(&presentation.title));
        let _ = tray.set_tooltip(Some(&presentation.tooltip));
    }) {
        eprintln!("failed to dispatch preview tray presentation: {error}");
    }
}

pub(crate) async fn reset_after_data_deletion(app_handle: &AppHandle, state: &DesktopState) {
    update(app_handle, &get_status_inner(state).await);
}

fn show_main_window(app_handle: &AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn refresh_from_tray(app_handle: &AppHandle) {
    if let Some(state) = app_handle.try_state::<TrayState>() {
        let _ = state.status_item.set_text("Refreshing capacity…");
    }
    let app_handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let envelope = {
            let state = app_handle.state::<DesktopState>();
            let _command = state.command_gate.lock().await;
            state.stamp_status(refresh_status_inner(&state).await)
        };
        update(&app_handle, &envelope);
        let _ = app_handle.emit(crate::monitor::MONITOR_STATUS_EVENT, envelope);
    });
}

fn tray_presentation(envelope: &DesktopStatusEnvelope) -> TrayPresentation {
    let Some(status) = envelope.status.as_ref() else {
        let menu_text = match envelope.lifecycle {
            DesktopLifecycle::Idle => "Capacity waiting for first refresh",
            DesktopLifecycle::SelectionRequired => "Choose a Codex installation",
            DesktopLifecycle::Error => "Capacity unavailable",
            DesktopLifecycle::Ready | DesktopLifecycle::Stale => "No capacity snapshot",
        };
        return TrayPresentation {
            title: match envelope.lifecycle {
                DesktopLifecycle::Error => "!",
                _ => "—",
            }
            .to_owned(),
            menu_text: menu_text.to_owned(),
            tooltip: format!("Codex Capacity Planner · {menu_text}"),
        };
    };
    let Some(window) = status
        .quota_windows
        .iter()
        .min_by(|left, right| left.remaining_percent.total_cmp(&right.remaining_percent))
    else {
        return TrayPresentation {
            title: "—".to_owned(),
            menu_text: "No fixed quota window".to_owned(),
            tooltip: "Codex Capacity Planner · No fixed quota window".to_owned(),
        };
    };
    let remaining = window.remaining_percent.round().clamp(0.0, 100.0) as u8;
    let availability = status.data_status.availability;
    let freshness = status.data_status.freshness;
    let compatibility = status.data_status.compatibility;
    let prefix = if freshness == Freshness::Stale {
        "~"
    } else {
        ""
    };
    let suffix = if availability != Availability::Complete
        || freshness == Freshness::NotApplicable
        || !matches!(
            compatibility,
            Compatibility::Tested | Compatibility::ExpectedCompatible
        ) {
        "?"
    } else {
        ""
    };
    let title = format!("{prefix}{remaining}%{suffix}");
    let menu_text = format!(
        "{} · {remaining}% · {} · {} · {}",
        window_duration_label(window),
        availability_label(availability),
        freshness_label(freshness),
        compatibility_label(compatibility),
    );
    TrayPresentation {
        title,
        tooltip: format!("Codex Capacity Planner · {menu_text}"),
        menu_text,
    }
}

fn availability_label(value: Availability) -> &'static str {
    match value {
        Availability::Complete => "complete",
        Availability::Partial => "partial",
        Availability::Unsupported => "unsupported",
        Availability::Failed => "failed",
    }
}

fn freshness_label(value: Freshness) -> &'static str {
    match value {
        Freshness::Live => "live",
        Freshness::Stale => "stale",
        Freshness::NotApplicable => "not applicable",
    }
}

fn compatibility_label(value: Compatibility) -> &'static str {
    match value {
        Compatibility::Tested => "tested",
        Compatibility::ExpectedCompatible => "expected compatible",
        Compatibility::NotTested => "not tested",
        Compatibility::Unsupported => "unsupported",
        Compatibility::KnownBroken => "known broken",
        Compatibility::NotApplicable => "not applicable",
    }
}

fn window_duration_label(window: &DesktopQuotaWindowView) -> String {
    match window.window_minutes {
        Some(300) => "5h".to_owned(),
        Some(10_080) => "1w".to_owned(),
        Some(minutes) if minutes % 10_080 == 0 => format!("{}w", minutes / 10_080),
        Some(minutes) if minutes % 60 == 0 => format!("{}h", minutes / 60),
        Some(minutes) => format!("{minutes}m"),
        None => "Quota".to_owned(),
    }
}

fn tray_icon_image() -> Image<'static> {
    const WIDTH: usize = 22;
    const HEIGHT: usize = 22;
    let mut rgba = vec![0_u8; WIDTH * HEIGHT * 4];
    let color = if cfg!(target_os = "macos") {
        [0, 0, 0, 255]
    } else {
        [112, 224, 180, 255]
    };
    for (x_start, y_start, bar_height) in [(3, 13, 6), (9, 8, 11), (15, 3, 16)] {
        for y in y_start..y_start + bar_height {
            for x in x_start..x_start + 4 {
                let index = (y * WIDTH + x) * 4;
                rgba[index..index + 4].copy_from_slice(&color);
            }
        }
    }
    Image::new_owned(rgba, WIDTH as u32, HEIGHT as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DesktopDataStatusView, DesktopStatusView};
    use capacity_domain::{AccountBindingStatus, ResetCreditDetailsStatus, SummaryStatus};

    fn envelope(
        remaining_percent: f64,
        availability: Availability,
        freshness: Freshness,
    ) -> DesktopStatusEnvelope {
        DesktopStatusEnvelope {
            history_context_id: None,
            selected_executable_source: None,
            schema_version: "1.0",
            sequence: 1,
            lifecycle: if freshness == Freshness::Stale {
                DesktopLifecycle::Stale
            } else {
                DesktopLifecycle::Ready
            },
            status: Some(DesktopStatusView {
                schema_version: "1.0".to_owned(),
                captured_at: "2026-08-30T00:00:00Z".to_owned(),
                quota_observed_at: None,
                quota_freshness: None,
                quota_source: None,
                codex_version: None,
                account: Some(crate::DesktopAccountView {
                    auth_mode: Some("chatgpt".to_owned()),
                    plan_type: Some("fixture-secret-plan".to_owned()),
                    binding_status: AccountBindingStatus::Ephemeral,
                }),
                data_status: DesktopDataStatusView {
                    availability,
                    freshness,
                    compatibility: Compatibility::ExpectedCompatible,
                    reason_codes: Vec::new(),
                },
                quota_windows: vec![DesktopQuotaWindowView {
                    limit_id: "fixture-secret-window".to_owned(),
                    label: Some("fixture-secret-label".to_owned()),
                    window_minutes: Some(10_080),
                    used_percent: 100.0 - remaining_percent,
                    remaining_percent,
                    resets_at: Some("2026-09-05T00:00:00Z".to_owned()),
                }],
                reset_credits: crate::DesktopResetCreditView {
                    summary_status: SummaryStatus::Unavailable,
                    available_count: None,
                    details_status: ResetCreditDetailsStatus::Unavailable,
                },
                usage: crate::DesktopUsageView {
                    availability: Availability::Complete,
                    has_summary: false,
                    reason_codes: Vec::new(),
                },
                diagnostic_codes: Vec::new(),
            }),
            candidates: Vec::new(),
            selected_executable_id: Some("fixture-secret-executable".to_owned()),
            issue: None,
            persistence_enabled: false,
        }
    }

    #[test]
    fn tray_uses_the_lowest_window_and_fixed_duration_label() {
        let mut envelope = envelope(63.4, Availability::Complete, Freshness::Live);
        envelope
            .status
            .as_mut()
            .unwrap()
            .quota_windows
            .push(DesktopQuotaWindowView {
                limit_id: "another-secret-window".to_owned(),
                label: Some("another-secret-label".to_owned()),
                window_minutes: Some(300),
                used_percent: 58.0,
                remaining_percent: 42.0,
                resets_at: Some("2026-08-30T05:00:00Z".to_owned()),
            });

        let presentation = tray_presentation(&envelope);
        assert_eq!(presentation.title, "42%");
        assert_eq!(
            presentation.menu_text,
            "5h · 42% · complete · live · expected compatible"
        );
        assert!(!presentation.tooltip.contains("secret"));
    }

    #[test]
    fn tray_marks_stale_and_partial_without_color_only_semantics() {
        let stale = tray_presentation(&envelope(48.0, Availability::Complete, Freshness::Stale));
        assert_eq!(stale.title, "~48%");
        assert!(stale.menu_text.contains(" · stale · "));

        let partial = tray_presentation(&envelope(48.0, Availability::Partial, Freshness::Live));
        assert_eq!(partial.title, "48%?");
        assert!(partial.menu_text.contains(" · partial · live · "));
    }

    #[test]
    fn tray_keeps_unverified_compatibility_distinct_from_freshness() {
        let mut unverified = envelope(48.0, Availability::Complete, Freshness::Live);
        unverified
            .status
            .as_mut()
            .unwrap()
            .data_status
            .compatibility = Compatibility::NotTested;

        let presentation = tray_presentation(&unverified);
        assert_eq!(presentation.title, "48%?");
        assert!(
            presentation
                .menu_text
                .ends_with("complete · live · not tested")
        );
    }

    #[test]
    fn tray_never_uses_upstream_labels_or_account_metadata() {
        let presentation =
            tray_presentation(&envelope(51.0, Availability::Complete, Freshness::Live));
        for canary in ["fixture-secret", "chatgpt"] {
            assert!(!presentation.menu_text.contains(canary));
            assert!(!presentation.tooltip.contains(canary));
        }
    }

    #[test]
    fn generated_icon_has_transparent_background_and_three_bars() {
        let image = tray_icon_image();
        assert_eq!((image.width(), image.height()), (22, 22));
        assert_eq!(image.rgba()[3], 0);
        let opaque_pixels = image
            .rgba()
            .chunks_exact(4)
            .filter(|pixel| pixel[3] == 255)
            .count();
        assert_eq!(opaque_pixels, (6 + 11 + 16) * 4);
    }
}
