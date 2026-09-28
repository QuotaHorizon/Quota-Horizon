// Modified by the QuotaHorizon project from QuotaHorizon v1.3.5.
mod account_archive;
mod account_refresh_lock;
mod agent_identity;
mod aggregate_api;
mod aggregate_scheduler;
mod antigravity_provider;
mod app_restart;
mod auth;
mod autostart;
mod browser;
mod capacity_bridge;
mod ccs_import;
mod claude_code;
mod claude_code_provider;
mod claude_desktop;
mod cloud;
mod codex_api;
mod codex_config;
mod codex_home;
mod codex_runtime;
mod commands;
mod conversation_hub;
mod cpa_pool;
mod desktop_lifecycle;
mod floating_bubble;
mod grok_provider;
mod launch_options;
mod legacy_account_migration;
mod legacy_migration;
mod legacy_migration_apply;
mod legacy_provider_import;
mod legacy_provider_migration;
mod local_proxy;
mod main_window;
mod models;
mod network_proxy;
mod oauth;
mod official_plugins;
mod open_code;
mod preset_provider;
mod prompt_plugins;
mod provider_api_cache;
mod provider_connectivity;
mod provider_credentials;
mod provider_models;
mod provider_platform;
mod providers;
mod public_reset_archive;
mod public_reset_feed;
mod public_reset_sources;
mod remote_control;
mod skills_market;
mod storage;
mod system_proxy;
mod system_tray;
mod third_party_apps;
mod totp_qr;
mod totp_window;
mod web_server;
mod web_session_login;

use oauth::AppState;
use tauri::Manager;
use tauri_plugin_deep_link::DeepLinkExt;
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if std::env::args_os().any(|argument| argument == "--print-local-proxy-token") {
        println!("{}", providers::LOCAL_PROXY_TOKEN);
        return;
    }
    let launch_options = match launch_options::LaunchOptions::from_environment() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}\nUsage: quota-horizon-desktop --headless --port=<1-65535>");
            std::process::exit(2);
        }
    };
    let mut context = tauri::generate_context!();
    if launch_options.headless {
        context.config_mut().app.windows.clear();
    }
    #[cfg(target_os = "macos")]
    for window in &mut context.config_mut().app.windows {
        if window.label == "main" {
            window.visible = false;
            window.focus = false;
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            match launch_options::LaunchOptions::parse(args) {
                Ok(options) if options.headless => {
                    if let Some(port) = options.port {
                        if let Err(error) = web_server::restart_at_port(app, port) {
                            eprintln!("failed to apply headless launch request: {error}");
                        }
                    }
                }
                Ok(_) => system_tray::show_dashboard(app),
                Err(error) => eprintln!("invalid launch request: {error}"),
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .manage(AppState::default())
        .manage(ccs_import::ImportState::default())
        .manage(main_window::MainWindowStateCache::default())
        .manage(main_window::CloseBehaviorState::default())
        .manage(desktop_lifecycle::DesktopLifecycle::new(launch_options.headless))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .setup(move |app| {
            if let Err(error) = legacy_provider_import::recover_interrupted_import(app.handle()) {
                eprintln!("Viewer connection migration recovery requires attention: {error}");
            }
            if let Err(error) = legacy_account_migration::recover_interrupted_import(app.handle()) {
                eprintln!("Viewer account migration recovery requires attention: {error}");
            }
            if let Err(error) = legacy_migration_apply::recover_interrupted_import(app.handle()) {
                eprintln!("Viewer migration recovery requires attention: {error}");
            }
            storage::migrate_app_settings_for_version(app.handle())?;
            let settings = storage::read_app_settings(app.handle())?;
            #[cfg(target_os = "macos")]
            if !launch_options.headless {
                desktop_lifecycle::setup(app, settings.language.as_deref() == Some("zh"))?;
            }
            third_party_apps::capture_running_app_paths(app.handle());
            codex_home::initialize(settings.codex_home.as_deref());
            match storage::resolve_paths(app.handle()).and_then(|paths| {
                let migrated =
                    provider_credentials::migrate_legacy_provider_store(&paths.providers)?;
                let pruned =
                    provider_credentials::prune_orphaned_provider_credentials(&paths.providers)?;
                Ok((migrated, pruned))
            }) {
                Ok((migrated, pruned)) if migrated > 0 || pruned.deleted > 0 => {
                    eprintln!(
                        "Provider credential maintenance migrated {migrated} profile(s), retained {}/{} referenced record(s), and pruned {} orphan(s)",
                        pruned.rooted,
                        pruned.indexed,
                        pruned.deleted,
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    eprintln!("Provider credential migration was deferred: {error}");
                }
            }
            main_window::configure_close_behavior(app.handle(), settings.close_to_tray);
            if let Err(error) = system_proxy::configure(&settings.network_proxy) {
                eprintln!("failed to restore the network proxy setting: {error}");
            }
            if !launch_options.headless {
                if let Err(error) = autostart::restore_preference(app.handle()) {
                    eprintln!("failed to restore the startup setting: {error}");
                }
            }
            if !launch_options.headless {
                main_window::restore_or_set_default(app)?;
            }
            commands::initialize_local_state(app.handle());
            capacity_desktop_service::initialize_with_explicit_executable(
                app,
                false,
                capacity_bridge::preferred_codex_executable(),
            )?;
            capacity_bridge::setup(app.handle())?;
            public_reset_feed::setup(app.handle())?;
            cpa_pool::setup_background_refresh(app.handle())?;
            #[cfg(any(target_os = "linux", all(debug_assertions, windows)))]
            if let Err(error) = app.deep_link().register_all() {
                eprintln!("failed to register desktop import links: {error}");
            }
            match app.deep_link().get_current() {
                Ok(Some(urls)) => {
                    for url in urls {
                        ccs_import::handle_url(app.handle(), &url);
                    }
                }
                Ok(None) => {}
                Err(error) => eprintln!("failed to read the startup import link: {error}"),
            }
            let import_app = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    ccs_import::handle_url(&import_app, &url);
                }
            });
            if !launch_options.headless {
                if let Err(error) = codex_runtime::setup(app.handle()) {
                    eprintln!("failed to initialize the Codex runtime channel: {error}");
                }
            }
            match local_proxy::restore_local_proxy_if_enabled(app.handle()) {
                Ok(true) => {}
                Ok(false) => providers::cleanup_stale_local_proxy_config(app.handle())?,
                Err(error) => {
                    eprintln!("failed to restore local proxy: {error}");
                    providers::cleanup_stale_local_proxy_config(app.handle())?;
                }
            }
            if !launch_options.headless {
                system_tray::setup(app)?;
                floating_bubble::setup(app.handle())?;
            }
            if let Err(error) = web_server::setup(app.handle(), launch_options.port) {
                if launch_options.headless {
                    return Err(std::io::Error::other(error).into());
                }
                eprintln!("failed to restore web version server: {error}");
            }
            remote_control::start(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if matches!(
                    event,
                    tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_)
                ) {
                    main_window::remember(window);
                }
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    main_window::remember_and_save(window);
                    if main_window::close_to_tray(window) {
                        api.prevent_close();
                        if let Err(error) = desktop_lifecycle::hide_main(window.app_handle()) {
                            eprintln!("failed to hide main window: {error}");
                        }
                    } else {
                        window.app_handle().exit(0);
                    }
                }
            }
            if window.label() == local_proxy::TOKEN_USAGE_WINDOW_LABEL {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.destroy();
                }
            }
            if window.label() == totp_window::TOTP_WINDOW_LABEL {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    if let Err(error) = window.destroy() {
                        eprintln!("failed to close 2FA window: {error}");
                    }
                }
            }
            if window.label() == web_session_login::WINDOW_LABEL {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    if let Err(error) = window.destroy() {
                        eprintln!("failed to close ChatGPT web login window: {error}");
                    }
                }
            }
            if window.label() == floating_bubble::BUBBLE_LABEL
                && matches!(event, tauri::WindowEvent::Moved(_))
            {
                floating_bubble::remember_position(window);
            }
            if system_tray::is_capacity_popover(window.label())
                && matches!(event, tauri::WindowEvent::Focused(false))
            {
                system_tray::hide_capacity_popover(window.app_handle());
            }
        })
        .on_menu_event(desktop_lifecycle::handle_menu_event)
        .invoke_handler(tauri::generate_handler![
            capacity_bridge::account_overview::capacity_get_account_overviews,
            desktop_lifecycle::hide_main_window,
            capacity_bridge::capacity_get_implementation_status,
            capacity_bridge::capacity_get_cached_status,
            capacity_bridge::capacity_authorize_history_keychain,
            capacity_bridge::capacity_get_status,
            public_reset_archive::get_public_reset_archive,
            public_reset_feed::get_public_reset_timeline,
            public_reset_feed::refresh_public_reset_timeline,
            public_reset_feed::read_state::mark_public_reset_changes_read,
            capacity_bridge::capacity_refresh_status,
            capacity_bridge::capacity_select_codex_executable,
            capacity_bridge::capacity_get_history,
            capacity_bridge::capacity_get_work_plan,
            capacity_bridge::capacity_get_demand_plan,
            capacity_bridge::capacity_update_demand_plan,
            capacity_bridge::capacity_get_active_time,
            capacity_bridge::capacity_get_active_time_quota,
            capacity_bridge::capacity_get_planning_archive,
            capacity_bridge::capacity_get_pace,
            capacity_bridge::capacity_get_pace_trials,
            capacity_bridge::capacity_update_active_time,
            capacity_bridge::capacity_update_work_schedule,
            capacity_bridge::capacity_get_diagnostics_preview,
            capacity_bridge::capacity_get_vault_mutation_status,
            commands::get_app_info,
            legacy_migration::inspect_legacy_quotaviewer_migration,
            legacy_migration_apply::prepare_legacy_quotaviewer_import,
            legacy_migration_apply::confirm_legacy_quotaviewer_import,
            legacy_migration_apply::get_latest_legacy_migration_operation,
            legacy_migration_apply::rollback_legacy_quotaviewer_import,
            legacy_account_migration::prepare_legacy_quotaviewer_account_import,
            legacy_account_migration::confirm_legacy_quotaviewer_account_import,
            legacy_account_migration::get_latest_legacy_account_migration_operation,
            legacy_account_migration::rollback_legacy_quotaviewer_account_import,
            legacy_provider_import::prepare_legacy_quotaviewer_provider_import,
            legacy_provider_import::confirm_legacy_quotaviewer_provider_import,
            legacy_provider_import::get_latest_legacy_provider_import,
            legacy_provider_import::rollback_legacy_quotaviewer_provider_import,
            ccs_import::take_ccswitch_import_request,
            ccs_import::cancel_ccswitch_provider_import,
            ccs_import::confirm_ccswitch_provider_import,
            commands::open_managed_folder,
            codex_home::set_codex_home,
            commands::list_accounts,
            commands::copy_account_auth_json,
            commands::import_auth_file,
            commands::import_account_json_file,
            commands::import_account_json_text,
            commands::import_compatible_json_file,
            commands::import_sub2api_json_file,
            account_archive::export_accounts_archive,
            account_archive::import_accounts_archive,
            commands::prepare_safe_account_switch,
            commands::confirm_safe_account_switch,
            commands::prepare_safe_account_deactivation,
            commands::confirm_safe_account_deactivation,
            commands::prepare_safe_provider_switch,
            commands::prepare_safe_provider_group_switch,
            commands::prepare_safe_aggregate_switch,
            commands::prepare_safe_provider_exit,
            commands::confirm_safe_provider_mutation,
            commands::prepare_safe_active_provider_edit,
            commands::prepare_safe_provider_group_edit,
            commands::prepare_safe_provider_model_switch,
            commands::prepare_safe_provider_model_control,
            commands::confirm_safe_active_provider_edit,
            commands::prepare_safe_active_aggregate_edit,
            commands::confirm_safe_active_aggregate_edit,
            commands::prepare_rollback_last_change,
            commands::confirm_rollback_last_change,
            commands::set_account_auto_switch_enabled,
            commands::set_account_auto_switch_priority,
            commands::set_account_auto_switch_threshold,
            commands::set_auto_disable_status_codes,
            commands::update_account_note,
            commands::delete_account,
            commands::refresh_usage,
            commands::consume_account_quota,
            commands::fetch_reset_credits,
            commands::consume_reset_credit,
            commands::restart_chatgpt,
            commands::launch_chatgpt,
            claude_code::set_claude_code_write_target,
            third_party_apps::set_third_party_app_write_settings,
            claude_code::launch_claude_code,
            claude_code::restart_claude_code,
            open_code::launch_open_code,
            open_code::restart_open_code,
            commands::restore_non_proxy_conversations,
            conversation_hub::browse_codex_threads,
            conversation_hub::search_codex_threads,
            conversation_hub::continue_codex_thread_search,
            conversation_hub::cancel_codex_thread_search,
            conversation_hub::get_codex_thread_revision,
            conversation_hub::measure_codex_thread_tokens,
            conversation_hub::inspect_codex_thread_detail,
            conversation_hub::resume_codex_thread,
            conversation_hub::prepare_codex_thread_rebind,
            conversation_hub::confirm_codex_thread_rebind,
            conversation_hub::prepare_codex_thread_discard,
            conversation_hub::confirm_codex_thread_discard,
            conversation_hub::browse_codex_thread_bin,
            conversation_hub::prepare_codex_thread_restore,
            conversation_hub::confirm_codex_thread_restore,
            conversation_hub::prepare_codex_thread_purge,
            conversation_hub::confirm_codex_thread_purge,
            conversation_hub::prepare_codex_thread_archive,
            conversation_hub::confirm_codex_thread_archive,
            conversation_hub::inspect_codex_thread_export,
            conversation_hub::pack_codex_threads,
            conversation_hub::inspect_codex_thread_import,
            conversation_hub::unpack_codex_threads,
            conversation_hub::migrate_codex_threads,
            conversation_hub::prepare_codex_thread_visibility_repair,
            conversation_hub::confirm_codex_thread_visibility_repair,
            conversation_hub::open_codex_thread_file,
            cpa_pool::get_cpa_pool_status,
            cpa_pool::refresh_cpa_pool_status,
            cpa_pool::set_cpa_bridge_settings,
            providers::list_providers,
            providers::save_provider,
            provider_connectivity::test_provider_connectivity,
            antigravity_provider::fetch_antigravity_models,
            claude_code_provider::fetch_claude_code_models,
            grok_provider::fetch_grok_models,
            preset_provider::fetch_preset_models,
            provider_models::fetch_relay_models,
            provider_platform::detect_relay_platform,
            providers::fetch_deepseek_models,
            providers::query_provider_balance,
            providers::query_provider_usage,
            providers::switch_provider_model,
            providers::set_provider_model_control,
            providers::set_provider_group,
            providers::set_provider_groups,
            providers::set_provider_auto_switch_enabled,
            providers::delete_provider,
            aggregate_api::list_aggregate_apis,
            aggregate_api::save_aggregate_api,
            aggregate_api::delete_aggregate_api,
            local_proxy::get_local_proxy_status,
            local_proxy::set_gpt_5_6_sol_context_window,
            local_proxy::get_official_model_context_settings,
            local_proxy::set_official_model_context_window,
            local_proxy::set_upstream_429_retry_timeout,
            local_proxy::list_proxy_sessions,
            local_proxy::list_proxy_session_requests,
            local_proxy::get_proxy_session_unlimited_conversation,
            local_proxy::set_proxy_session_unlimited_conversation,
            local_proxy::get_recent_proxy_session_latency,
            local_proxy::export_diagnostic_logs,
            local_proxy::list_token_usage_entries,
            local_proxy::list_token_usage_entries_since,
            local_proxy::list_daily_token_usage,
            local_proxy::list_account_token_usage,
            local_proxy::list_provider_token_usage,
            local_proxy::show_token_usage_window,
            totp_window::show_totp_window,
            local_proxy::start_local_proxy,
            local_proxy::stop_local_proxy,
            local_proxy::stop_local_proxy_without_migrating,
            local_proxy::set_auto_switch_on_quota_exhaustion,
            local_proxy::set_concurrent_account_routing_enabled,
            local_proxy::set_custom_auto_switch_priority_enabled,
            local_proxy::set_custom_auto_switch_threshold_enabled,
            local_proxy::set_global_auto_switch_threshold,
            local_proxy::set_auto_disable_unreachable_accounts,
            local_proxy::set_system_prompt_filter_enabled,
            local_proxy::set_system_prompt_filter_rules,
            local_proxy::set_system_prompt_injection_enabled,
            local_proxy::set_system_prompt_injection_prompts,
            local_proxy::set_image_generation_account,
            local_proxy::set_image_model_target,
            local_proxy::set_local_proxy_openai_auth_account,
            local_proxy::set_local_proxy_listen_on_all_interfaces,
            local_proxy::copy_local_proxy_lan_api_key,
            floating_bubble::get_app_settings,
            autostart::set_launch_at_startup,
            main_window::set_close_to_tray,
            floating_bubble::set_floating_bubble,
            floating_bubble::set_privacy_mode,
            floating_bubble::set_hide_account_notes,
            floating_bubble::set_show_usage_network_errors,
            floating_bubble::set_token_usage_preferences,
            floating_bubble::set_bubble_reset_display,
            floating_bubble::set_bubble_style,
            floating_bubble::set_theme_color,
            floating_bubble::set_app_language,
            network_proxy::set_network_proxy,
            web_server::set_web_proxy_port,
            web_server::set_web_proxy_listen_on_all_interfaces,
            web_server::copy_web_proxy_lan_api_key,
            floating_bubble::resize_floating_bubble,
            floating_bubble::resize_floating_bubble_for_provider_card,
            floating_bubble::drag_floating_bubble,
            floating_bubble::show_floating_bubble_menu,
            floating_bubble::show_dashboard_from_bubble,
            oauth::start_login,
            oauth::get_login_status,
            oauth::cancel_login,
            web_session_login::start_web_session_login,
            cloud::get_cloud_auth_state,
            cloud::get_saved_cloud_login,
            cloud::fetch_cloud_announcement,
            cloud::fetch_cloud_currency_rates,
            cloud::fetch_cloud_faqs,
            cloud::fetch_cloud_notifications,
            cloud::report_announcement_click,
            cloud::submit_feedback,
            cloud::report_first_installation,
            cloud::report_device_activity,
            cloud::report_base_url_change,
            cloud::set_cloud_base_url,
            cloud::cloud_login,
            cloud::cloud_request_registration_code,
            cloud::cloud_register,
            cloud::cloud_change_password,
            cloud::cloud_logout,
            cloud::cloud_push_accounts,
            cloud::cloud_push_account,
            cloud::cloud_pull_account,
            cloud::cloud_push_providers,
            cloud::cloud_push_provider,
            cloud::cloud_delete_account,
            cloud::cloud_list_deleted_accounts,
            cloud::cloud_restore_deleted_account,
            cloud::cloud_list_deleted_providers,
            cloud::cloud_restore_deleted_provider,
            cloud::cloud_delete_provider,
            cloud::cloud_sync_accounts,
            cloud::cloud_sync_totp,
            cloud::cloud_pull_totp,
            totp_qr::decode_totp_qr_image,
            skills_market::list_market_skills,
            skills_market::upload_market_skill,
            skills_market::install_market_skill,
            skills_market::remove_market_skill,
            skills_market::set_market_skill_enabled,
            prompt_plugins::list_prompt_plugins,
            prompt_plugins::publish_prompt_plugin,
            prompt_plugins::install_prompt_plugin,
            prompt_plugins::remove_prompt_plugin,
            prompt_plugins::set_prompt_plugin_enabled,
            official_plugins::list_official_plugins,
            official_plugins::install_official_plugin,
            official_plugins::remove_official_plugin,
            official_plugins::set_official_plugin_enabled,
        ])
        .build(context)
        .unwrap_or_else(|error| {
            eprintln!("failed to start QuotaHorizon: {error}");
            std::process::exit(1);
        })
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if matches!(event, tauri::RunEvent::Reopen { .. }) {
                system_tray::show_dashboard(app);
            }
            if let tauri::RunEvent::ExitRequested { api, code, .. } = &event {
                if !app.state::<desktop_lifecycle::DesktopLifecycle>().permits_exit(*code) {
                    api.prevent_exit();
                    return;
                }
            }
            if matches!(event, tauri::RunEvent::Exit) {
                desktop_lifecycle::prepare_exit(app);
            }
        });
}
