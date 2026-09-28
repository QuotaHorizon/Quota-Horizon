use chrono::{DateTime, Local};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Emitter, Manager, Runtime,
};

mod capacity;
mod popover;

pub(crate) use capacity::update_from_serializable as update_capacity_status;

use crate::{
    commands,
    models::{AccountSummary, ProviderSummary, UsageSummary, UsageWindow},
    providers,
    storage::read_app_settings,
};

const TRAY_ID: &str = "main-tray";
const DASHBOARD_ID: &str = "tray:dashboard";
const SETTINGS_ID: &str = "tray:settings";
const OPEN_SETTINGS_EVENT: &str = "open-settings";
const REFRESH_ACCOUNTS_ID: &str = "tray:refresh-accounts";
const ACCOUNT_SWITCH_REQUESTED_EVENT: &str = "tray-account-switch-requested";
const ACCOUNT_REFRESH_ALL_REQUESTED_EVENT: &str = "tray-account-refresh-all-requested";
const RESTART_CHATGPT_ID: &str = "tray:restart-chatgpt";
const RESTART_APP_ID: &str = "tray:restart-app";
const QUIT_ID: &str = "tray:quit";
const ACCOUNT_PREFIX: &str = "tray:account:";
const PROVIDER_PREFIX: &str = "tray:provider:";
const PROVIDER_MODEL_PREFIX: &str = "tray:provider-model:";
const PROVIDER_SUBMENU_PREFIX: &str = "tray:provider-submenu:";
const MENU_EMAIL_CHARS: usize = 15;
const MENU_PROVIDER_CHARS: usize = 28;

pub(crate) fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let menu = build_menu(app.handle())?;
    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("QuotaHorizon")
        .title("Quota —")
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                if let TrayIconEvent::Click { position, .. } = event {
                    popover::toggle(tray.app_handle(), position);
                }
            }
        });

    #[cfg(not(target_os = "macos"))]
    let builder = match app.default_window_icon().cloned() {
        Some(icon) => builder.icon(icon),
        None => builder,
    };

    builder.build(app)?;
    capacity::setup(app);
    Ok(())
}

pub(crate) fn refresh_menu<R: Runtime>(app: &AppHandle<R>) {
    match build_menu(app) {
        Ok(menu) => {
            if let Err(error) =
                capacity_desktop_service::with_tray_on_main_thread(app, TRAY_ID, move |tray| {
                    if let Err(error) = tray.set_menu(Some(menu)) {
                        eprintln!("failed to refresh tray menu: {error}");
                    }
                })
            {
                eprintln!("failed to dispatch tray menu: {error}");
            }
            capacity::update_from_active_account(app);
        }
        Err(error) => eprintln!("failed to build tray menu: {error}"),
    }
}

pub(crate) fn show_dashboard<R: Runtime>(app: &AppHandle<R>) {
    popover::hide(app);
    if app.get_webview_window("main").is_some() {
        #[cfg(target_os = "macos")]
        crate::main_window::reset_to_default_size(app);
        crate::desktop_lifecycle::show_main(app);
    }
}

fn show_settings<R: Runtime>(app: &AppHandle<R>) {
    show_dashboard(app);
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Err(error) = window.emit(OPEN_SETTINGS_EVENT, ()) {
        eprintln!("failed to open settings from menu: {error}");
    }
}

pub(crate) fn hide_capacity_popover<R: Runtime>(app: &AppHandle<R>) {
    popover::hide(app);
}

pub(crate) fn is_capacity_popover(label: &str) -> bool {
    popover::is_label(label)
}

pub(crate) fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let id = event.id().as_ref();
    if id == SETTINGS_ID {
        show_settings(app);
        return;
    }
    if id == DASHBOARD_ID {
        show_dashboard(app);
        return;
    }
    if id == REFRESH_ACCOUNTS_ID {
        show_dashboard(app);
        if let Some(window) = app.get_webview_window("main") {
            if let Err(error) = window.emit(ACCOUNT_REFRESH_ALL_REQUESTED_EVENT, ()) {
                eprintln!("failed to request account refresh from tray: {error}");
            }
        }
        return;
    }
    if id == RESTART_CHATGPT_ID {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = commands::restart_chatgpt_blocking(app) {
                eprintln!("failed to restart ChatGPT from menu: {error}");
            }
        });
        return;
    }
    if id == RESTART_APP_ID {
        crate::desktop_lifecycle::restart_application(app);
        return;
    }
    if id == QUIT_ID {
        crate::desktop_lifecycle::quit_from_menu(app);
        return;
    }
    if let Some(account_id) = id.strip_prefix(ACCOUNT_PREFIX) {
        show_dashboard(app);
        if let Some(window) = app.get_webview_window("main") {
            if let Err(error) = window.emit(ACCOUNT_SWITCH_REQUESTED_EVENT, account_id.to_string())
            {
                eprintln!("failed to request safe account switch from tray: {error}");
            }
        }
        return;
    }
    if let Some(selection) = parse_provider_model_menu_id(id) {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = providers::switch_provider_model_and_activate_blocking(
                app,
                selection.provider_id,
                selection.model,
            ) {
                eprintln!("failed to switch provider model from menu: {error}");
            }
        });
        return;
    }
    if let Some(provider_id) = id.strip_prefix(PROVIDER_PREFIX) {
        let app = app.clone();
        let provider_id = provider_id.to_string();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = providers::switch_provider_blocking(app, provider_id) {
                eprintln!("failed to switch provider from tray: {error}");
            }
        });
    }
}

pub(crate) fn build_menu<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Menu<R>, Box<dyn std::error::Error>> {
    let menu = Menu::new(app)?;
    let settings = read_app_settings(app).unwrap_or_default();
    let chinese = settings.language.as_deref() == Some("zh");
    let privacy_mode = settings.privacy_mode;

    let accounts_header = MenuItem::with_id(
        app,
        "tray:accounts-header",
        if chinese { "账号" } else { "Accounts" },
        false,
        None::<&str>,
    )?;
    menu.append(&accounts_header)?;

    let has_accounts = match commands::list_accounts_blocking(app.clone()) {
        Ok(accounts) if accounts.is_empty() => {
            let empty = MenuItem::with_id(app, "tray:empty", "暂无节点", false, None::<&str>)?;
            menu.append(&empty)?;
            false
        }
        Ok(accounts) => {
            for account in accounts {
                let item = CheckMenuItem::with_id(
                    app,
                    format!("{ACCOUNT_PREFIX}{}", account.id),
                    account_label(&account, chinese, privacy_mode),
                    true,
                    account.active,
                    None::<&str>,
                )?;
                menu.append(&item)?;
            }
            true
        }
        Err(error) => {
            let item = MenuItem::with_id(
                app,
                "tray:accounts-error",
                format!("节点读取失败: {error}"),
                false,
                None::<&str>,
            )?;
            menu.append(&item)?;
            false
        }
    };
    menu.append(&MenuItem::with_id(
        app,
        REFRESH_ACCOUNTS_ID,
        if chinese {
            "刷新全部账号额度"
        } else {
            "Refresh all account usage"
        },
        has_accounts,
        None::<&str>,
    )?)?;

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    append_provider_items(app, &menu, chinese)?;

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        SETTINGS_ID,
        if chinese { "设置" } else { "Settings" },
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        DASHBOARD_ID,
        if chinese { "仪表板" } else { "Dashboard" },
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        RESTART_CHATGPT_ID,
        if chinese {
            "重启 ChatGPT"
        } else {
            "Restart ChatGPT"
        },
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        RESTART_APP_ID,
        if chinese {
            "重启 QuotaHorizon"
        } else {
            "Restart QuotaHorizon"
        },
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        QUIT_ID,
        if chinese { "退出程序" } else { "Quit" },
        true,
        None::<&str>,
    )?)?;
    Ok(menu)
}

fn append_provider_items<R: Runtime>(
    app: &AppHandle<R>,
    menu: &Menu<R>,
    chinese: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let header = MenuItem::with_id(
        app,
        "tray:providers-header",
        if chinese {
            "三方 Provider"
        } else {
            "Providers"
        },
        false,
        None::<&str>,
    )?;
    menu.append(&header)?;

    match providers::list_providers(app.clone()) {
        Ok(providers) => {
            if providers.is_empty() {
                let empty = MenuItem::with_id(
                    app,
                    "tray:providers-empty",
                    "No providers",
                    false,
                    None::<&str>,
                )?;
                menu.append(&empty)?;
                return Ok(());
            }

            for provider in providers {
                append_provider_item(app, menu, &provider)?;
            }
        }
        Err(error) => {
            let item = MenuItem::with_id(
                app,
                "tray:providers-error",
                format!("Providers error: {error}"),
                false,
                None::<&str>,
            )?;
            menu.append(&item)?;
        }
    }
    Ok(())
}

fn append_provider_item<R: Runtime>(
    app: &AppHandle<R>,
    menu: &Menu<R>,
    provider: &ProviderSummary,
) -> Result<(), Box<dyn std::error::Error>> {
    if provider.model_selection_controlled_by_codex || provider.models.is_empty() {
        let item = CheckMenuItem::with_id(
            app,
            format!("{PROVIDER_PREFIX}{}", provider.id),
            provider_label(provider),
            provider.supports_direct_switch,
            provider.active,
            None::<&str>,
        )?;
        menu.append(&item)?;
        return Ok(());
    }

    let submenu = Submenu::with_id(
        app,
        format!("{PROVIDER_SUBMENU_PREFIX}{}", provider.id),
        provider_submenu_label(provider),
        provider.supports_direct_switch,
    )?;
    for model in &provider.models {
        let item = CheckMenuItem::with_id(
            app,
            provider_model_menu_id(&provider.id, model),
            escape_menu_text(&truncate_menu_provider(model)),
            provider.supports_direct_switch,
            provider.active && provider.model == *model,
            None::<&str>,
        )?;
        submenu.append(&item)?;
    }
    menu.append(&submenu)?;
    Ok(())
}

fn provider_submenu_label(provider: &ProviderSummary) -> String {
    let label = provider_label(provider);
    if provider.active {
        format!("✓ {label}")
    } else {
        label
    }
}

struct ProviderModelSelection {
    provider_id: String,
    model: String,
}

fn provider_model_menu_id(provider_id: &str, model: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

    format!(
        "{PROVIDER_MODEL_PREFIX}{}:{}",
        URL_SAFE_NO_PAD.encode(provider_id),
        URL_SAFE_NO_PAD.encode(model)
    )
}

fn parse_provider_model_menu_id(id: &str) -> Option<ProviderModelSelection> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

    let encoded = id.strip_prefix(PROVIDER_MODEL_PREFIX)?;
    let (provider_id, model) = encoded.split_once(':')?;
    Some(ProviderModelSelection {
        provider_id: String::from_utf8(URL_SAFE_NO_PAD.decode(provider_id).ok()?).ok()?,
        model: String::from_utf8(URL_SAFE_NO_PAD.decode(model).ok()?).ok()?,
    })
}

fn plan_has_no_short_quota_window(plan: &str) -> bool {
    matches!(
        plan.chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
            .as_str(),
        "pro" | "prolite" | "chatgptpro" | "chatgptprolite"
    )
}

pub(crate) fn account_usage_windows<'a>(
    usage: &'a UsageSummary,
    plan: &str,
) -> (Option<&'a UsageWindow>, Option<&'a UsageWindow>) {
    const SHORT_WINDOW_MAX_MINUTES: i64 = 24 * 60;
    let mut short = None;
    let mut weekly = None;
    for window in [usage.primary.as_ref(), usage.secondary.as_ref()]
        .into_iter()
        .flatten()
    {
        match window.window_minutes.filter(|minutes| *minutes > 0) {
            Some(minutes)
                if minutes <= SHORT_WINDOW_MAX_MINUTES
                    && short.is_none_or(|current: &UsageWindow| {
                        minutes < current.window_minutes.unwrap_or(i64::MAX)
                    }) =>
            {
                short = Some(window);
            }
            Some(minutes)
                if minutes > SHORT_WINDOW_MAX_MINUTES
                    && weekly.is_none_or(|current: &UsageWindow| {
                        minutes > current.window_minutes.unwrap_or_default()
                    }) =>
            {
                weekly = Some(window);
            }
            Some(_) | None => {}
        }
    }

    let no_short_window = plan_has_no_short_quota_window(plan);
    if weekly.is_none() {
        if no_short_window {
            weekly = usage.primary.as_ref().or(usage.secondary.as_ref());
        } else if usage
            .secondary
            .as_ref()
            .is_some_and(|window| window.window_minutes.is_none())
        {
            weekly = usage.secondary.as_ref();
        }
    }
    if no_short_window {
        short = None;
    } else if short.is_none() {
        short = usage
            .primary
            .as_ref()
            .filter(|candidate| weekly.is_none_or(|weekly| !std::ptr::eq(*candidate, weekly)));
    }
    (short, weekly)
}

fn account_label(account: &AccountSummary, chinese: bool, privacy_mode: bool) -> String {
    let (short, weekly) = account_usage_windows(&account.usage, &account.plan);
    format!(
        "{} | 5h {} | 1week {} | {}",
        escape_menu_text(&truncate_menu_email(&menu_account_email(
            &account.email,
            privacy_mode,
        ))),
        remaining_label(short),
        remaining_label(weekly),
        usage_freshness_label(&account.usage, chinese),
    )
}

fn menu_account_email(email: &str, privacy_mode: bool) -> String {
    if !privacy_mode {
        return email.to_string();
    }
    let characters = email.chars().collect::<Vec<_>>();
    if characters.len() <= 10 {
        return "*****".to_string();
    }
    format!(
        "{}*****{}",
        characters[..5].iter().collect::<String>(),
        characters[characters.len() - 5..]
            .iter()
            .collect::<String>()
    )
}

fn usage_freshness_label(usage: &UsageSummary, chinese: bool) -> String {
    if usage
        .error
        .as_deref()
        .is_some_and(|error| !error.trim().is_empty())
    {
        return if chinese {
            "刷新错误"
        } else {
            "refresh error"
        }
        .to_string();
    }
    let Some(fetched_at) = usage.fetched_at.as_deref() else {
        return if chinese {
            "未刷新"
        } else {
            "not refreshed"
        }
        .to_string();
    };
    let Ok(fetched_at) = DateTime::parse_from_rfc3339(fetched_at) else {
        return if chinese {
            "时间未知"
        } else {
            "time unknown"
        }
        .to_string();
    };
    let clock = fetched_at.with_timezone(&Local).format("%H:%M");
    if chinese {
        format!("更新 {clock}")
    } else {
        format!("updated {clock}")
    }
}

pub(crate) fn provider_label(provider: &ProviderSummary) -> String {
    let name = escape_menu_text(&truncate_menu_provider(&provider.name));
    if provider.model_selection_controlled_by_codex || provider.model.trim().is_empty() {
        name
    } else {
        format!(
            "{name} | {}",
            escape_menu_text(&truncate_menu_provider(&provider.model))
        )
    }
}

fn remaining_label(window: Option<&UsageWindow>) -> String {
    window
        .map(|window| quota_percent_label(window.remaining_percent))
        .unwrap_or_else(|| "-".to_string())
}

fn quota_percent_label(value: f64) -> String {
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return "-".to_owned();
    }
    let tenths = (value * 10.0).round() / 10.0;
    if tenths == 0.0 {
        "0%".to_owned()
    } else if tenths.fract() == 0.0 {
        format!("{tenths:.0}%")
    } else {
        format!("{tenths:.1}%")
    }
}

fn truncate_menu_provider(text: &str) -> String {
    let mut chars = text.chars();
    let truncated = chars.by_ref().take(MENU_PROVIDER_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn escape_menu_text(text: &str) -> String {
    text.replace('&', "&&")
}

fn truncate_menu_email(text: &str) -> String {
    let mut chars = text.chars();
    let truncated = chars.by_ref().take(MENU_EMAIL_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::{
        account_usage_windows, menu_account_email, parse_provider_model_menu_id,
        plan_has_no_short_quota_window, provider_model_menu_id, quota_percent_label,
        usage_freshness_label,
    };
    use crate::models::{UsageSummary, UsageWindow};

    #[test]
    fn observed_quota_keeps_fractions_without_zero_padding() {
        for (value, expected) in [
            (98.0, "98%"),
            (0.0, "0%"),
            (-0.0, "0%"),
            (100.0, "100%"),
            (28.4, "28.4%"),
            (28.36, "28.4%"),
            (28.35, "28.4%"),
            (100.0 - 71.6, "28.4%"),
            (28.01, "28%"),
            (-1.0, "-"),
            (101.0, "-"),
            (f64::NAN, "-"),
            (f64::INFINITY, "-"),
        ] {
            assert_eq!(quota_percent_label(value), expected);
        }
    }

    #[test]
    fn provider_model_menu_id_round_trips_special_characters() {
        let id = provider_model_menu_id("relay:one", "openai/gpt:latest 中文");
        let selection = parse_provider_model_menu_id(&id).expect("menu id should be valid");

        assert_eq!(selection.provider_id, "relay:one");
        assert_eq!(selection.model, "openai/gpt:latest 中文");
    }

    #[test]
    fn provider_model_menu_id_rejects_invalid_values() {
        assert!(parse_provider_model_menu_id("tray:provider-model:not-base64:!").is_none());
        assert!(parse_provider_model_menu_id("tray:provider:relay").is_none());
    }

    #[test]
    fn account_usage_freshness_is_safe_and_compact() {
        assert_eq!(
            usage_freshness_label(&UsageSummary::default(), false),
            "not refreshed"
        );
        assert_eq!(
            usage_freshness_label(
                &UsageSummary {
                    error: Some("sensitive upstream details".to_string()),
                    ..UsageSummary::default()
                },
                true,
            ),
            "刷新错误"
        );
        assert!(usage_freshness_label(
            &UsageSummary {
                fetched_at: Some("2026-09-04T08:30:00Z".to_string()),
                ..UsageSummary::default()
            },
            false,
        )
        .starts_with("updated "));
    }

    #[test]
    fn tray_account_email_follows_the_saved_privacy_mode() {
        assert_eq!(
            menu_account_email("person@example.com", true),
            "perso*****e.com"
        );
        assert_eq!(
            menu_account_email("person@example.com", false),
            "person@example.com"
        );
        assert_eq!(menu_account_email("short@mail", true), "*****");
    }

    #[test]
    fn pro_usage_treats_a_primary_weekly_window_as_weekly_only() {
        let weekly = UsageWindow {
            used_percent: 51.0,
            remaining_percent: 49.0,
            resets_at: None,
            window_minutes: Some(10_080),
        };
        let usage = UsageSummary {
            primary: Some(weekly.clone()),
            ..UsageSummary::default()
        };

        let (short, selected_weekly) = account_usage_windows(&usage, "prolite");
        assert!(short.is_none());
        assert_eq!(
            selected_weekly.map(|window| (window.remaining_percent, window.window_minutes)),
            Some((49.0, Some(10_080)))
        );
        assert!(plan_has_no_short_quota_window("ChatGPT Pro"));
    }
}
