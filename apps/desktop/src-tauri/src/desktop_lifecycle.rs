//! Main-window presentation is independent of menu-bar/background monitoring.
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager, Runtime};

pub(crate) struct DesktopLifecycle {
    menu_bar_resident: bool,
    quit_from_menu: AtomicBool,
    restarting: AtomicBool,
}

impl DesktopLifecycle {
    pub(crate) fn new(headless: bool) -> Self {
        Self {
            menu_bar_resident: cfg!(target_os = "macos") && !headless,
            quit_from_menu: AtomicBool::new(false),
            restarting: AtomicBool::new(false),
        }
    }

    pub(crate) fn permits_exit(&self, code: Option<i32>) -> bool {
        !self.menu_bar_resident
            || self.quit_from_menu.load(Ordering::SeqCst)
            || code == Some(tauri::RESTART_EXIT_CODE)
    }
}

pub(crate) fn prepare_exit<R: Runtime>(app: &AppHandle<R>) {
    crate::web_server::shutdown();
    if let Err(error) = crate::main_window::save_cached(app) {
        eprintln!("failed to save main window state before exit: {error}");
    }
    crate::cpa_pool::shutdown_background_refresh();
    crate::public_reset_feed::shutdown(app);
    crate::capacity_bridge::shutdown(app);
    capacity_desktop_service::shutdown(app);
}

pub(crate) fn restart_application<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<DesktopLifecycle>();
    if state.restarting.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::system_tray::hide_capacity_popover(app);
    let worker = app.clone();
    if let Err(error) = crate::app_restart::restart(app.clone(), move || prepare_exit(&worker)) {
        state.restarting.store(false, Ordering::SeqCst);
        eprintln!("Unable to restart QuotaHorizon: {error}");
    }
}

pub(crate) fn quit_from_menu<R: Runtime>(app: &AppHandle<R>) {
    app.state::<DesktopLifecycle>()
        .quit_from_menu
        .store(true, Ordering::SeqCst);
    app.exit(0);
}

pub(crate) fn show_main<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    #[cfg(target_os = "macos")]
    if let Err(error) = app.set_activation_policy(tauri::ActivationPolicy::Regular) {
        eprintln!("failed to show the application in the Dock: {error}");
    }
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

pub(crate) fn hide_main<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(error) = crate::main_window::save_cached(app) {
            eprintln!("failed to save main window state: {error}");
        }
        window.hide().map_err(|error| error.to_string())?;
    }
    crate::system_tray::hide_capacity_popover(app);
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory)
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) fn hide_main_window<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    hide_main(&app)
}

#[cfg(target_os = "macos")]
const RETURN_TO_MENU_BAR: &str = "app:return-to-menu-bar";
#[cfg(target_os = "macos")]
const RESTART_APPLICATION: &str = "app:restart-horizon";

#[cfg(target_os = "macos")]
pub(crate) fn setup(app: &mut tauri::App, chinese: bool) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};

    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    native_termination::install(app.handle()).map_err(std::io::Error::other)?;
    // Keep Tauri's standard Edit/Window menus, but replace its native Quit
    // action. Cmd+Q returns to monitoring; only the status-item menu offers exit.
    let menu = Menu::default(app.handle())?;
    if let Some(original) = menu.items()?.first() {
        menu.remove(original)?;
    }
    let application_menu = Submenu::with_items(
        app,
        "QuotaHorizon",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("QuotaHorizon"), None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                RESTART_APPLICATION,
                if chinese {
                    "重启 QuotaHorizon"
                } else {
                    "Restart QuotaHorizon"
                },
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                RETURN_TO_MENU_BAR,
                if chinese {
                    "返回菜单栏（继续监控）"
                } else {
                    "Return to Menu Bar (Keep Monitoring)"
                },
                true,
                Some("Cmd+Q"),
            )?,
        ],
    )?;
    menu.prepend(&application_menu)?;
    app.set_menu(menu)?;
    Ok(())
}

#[cfg(target_os = "macos")]
mod native_termination {
    use std::sync::OnceLock;
    static APPLICATION: OnceLock<tauri::AppHandle> = OnceLock::new();

    unsafe extern "C" {
        fn horizon_install_termination_guard(callback: extern "C" fn()) -> i32;
        #[cfg(test)]
        fn horizon_allows_system_quit(reason: u32) -> i32;
    }

    extern "C" fn return_to_menu_bar() {
        if let Some(app) = APPLICATION.get() {
            if let Err(error) = super::hide_main(app) {
                eprintln!("failed to return to menu bar after native quit request: {error}");
            }
        }
    }

    pub(super) fn install(app: &tauri::AppHandle) -> Result<(), String> {
        APPLICATION
            .set(app.clone())
            .map_err(|_| "macOS lifecycle is already initialized")?;
        // SAFETY: setup runs on AppKit's main thread. The C callback has static
        // lifetime, uses the matching ABI, and the bridge retains no Rust data.
        if unsafe { horizon_install_termination_guard(return_to_menu_bar) } == 1 {
            Ok(())
        } else {
            Err("Unable to install the menu-bar quit policy on the macOS delegate".into())
        }
    }

    #[test]
    fn native_quit_guard_preserves_system_session_termination_only() {
        for reason in [*b"shut", *b"rest", *b"rlgo", *b"logo"] {
            // SAFETY: pure native helper taking only a fixed-width integer.
            assert_eq!(
                unsafe { horizon_allows_system_quit(u32::from_be_bytes(reason)) },
                1
            );
        }
        for reason in [0, u32::from_be_bytes(*b"quit"), u32::MAX] {
            assert_eq!(unsafe { horizon_allows_system_quit(reason) }, 0);
        }
    }
}

pub(crate) fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: tauri::menu::MenuEvent) {
    #[cfg(target_os = "macos")]
    if event.id().as_ref() == RESTART_APPLICATION {
        restart_application(app);
        return;
    }
    #[cfg(target_os = "macos")]
    if event.id().as_ref() == RETURN_TO_MENU_BAR {
        if let Err(error) = hide_main(app) {
            eprintln!("failed to return to menu-bar monitoring: {error}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, event);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resident() -> DesktopLifecycle {
        DesktopLifecycle {
            menu_bar_resident: true,
            quit_from_menu: AtomicBool::new(false),
            restarting: AtomicBool::new(false),
        }
    }

    #[test]
    fn closing_windows_or_process_ipc_cannot_quit_a_resident_app() {
        let state = resident();
        assert!(!state.permits_exit(None));
        assert!(!state.permits_exit(Some(0)));
        assert!(!state.permits_exit(Some(1)));
    }

    #[test]
    fn explicit_status_menu_quit_is_allowed() {
        let state = resident();
        state.quit_from_menu.store(true, Ordering::SeqCst);
        assert!(state.permits_exit(Some(0)));
    }

    #[test]
    fn restart_and_headless_exit_keep_their_existing_behavior() {
        assert!(resident().permits_exit(Some(tauri::RESTART_EXIT_CODE)));
        let headless = DesktopLifecycle::new(true);
        assert!(headless.permits_exit(None));
        assert!(headless.permits_exit(Some(0)));
    }
}
