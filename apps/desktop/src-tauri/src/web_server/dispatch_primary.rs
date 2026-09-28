fn dispatch_command(app: AppHandle, command: &str, args: Value) -> Result<Value, String> {
    match command {
        "capacity_get_cached_status" => serialize(block_on(async move {
            crate::capacity_bridge::capacity_get_cached_status(app).await
        })),
        "capacity_get_implementation_status" => serialize(Ok(
            capacity_desktop_service::implementation_status(&app),
        )),
        "capacity_get_status" => serialize(block_on(async move {
            crate::capacity_bridge::capacity_get_status(app)
                .await
                .map_err(|error| error.to_string())
        })),
        "capacity_refresh_status" => serialize(block_on(async move {
            crate::capacity_bridge::capacity_refresh_status(app)
                .await
                .map_err(|error| error.to_string())
        })),
        "capacity_select_codex_executable" => serialize(block_on(async move {
            crate::capacity_bridge::capacity_select_codex_executable(
                argument(&args, "executableId")?,
                app,
            )
            .await
            .map_err(|error| error.to_string())
        })),
        "capacity_get_history" => serialize(block_on(async move {
            crate::capacity_bridge::capacity_get_history(
                argument(&args, "limitId")?,
                argument(&args, "maximumPoints")?,
                app,
            )
            .await
            .map_err(|error| error.to_string())
        })),
        "capacity_get_diagnostics_preview" => serialize(block_on(async move {
            capacity_desktop_service::diagnostics(app)
                .await
                .map_err(|error| error.to_string())
        })),
        "capacity_get_vault_mutation_status" => serialize(block_on(async move {
            capacity_desktop_service::vault_mutation_status(app)
                .await
                .map_err(|error| error.to_string())
        })),
        "get_app_info" => serialize(crate::commands::get_app_info(app)),
        "list_accounts" => serialize(block_on(crate::commands::list_accounts(app))),
        "get_app_settings" => serialize(crate::floating_bubble::get_app_settings(app)),
        "set_claude_code_write_target" => serialize(block_on(
            crate::claude_code::set_claude_code_write_target(
                app,
                argument(&args, "target")?,
            ),
        )),
        "set_third_party_app_write_settings" => serialize(block_on(
            crate::third_party_apps::set_third_party_app_write_settings(
                app,
                argument(&args, "settings")?,
            ),
        )),
        "launch_claude_code" => serialize(block_on(crate::claude_code::launch_claude_code(app))),
        "restart_claude_code" => serialize(block_on(crate::claude_code::restart_claude_code(app))),
        "launch_open_code" => serialize(block_on(crate::open_code::launch_open_code(app))),
        "restart_open_code" => serialize(block_on(crate::open_code::restart_open_code(app))),
        "set_gpt_5_6_sol_context_window" => serialize(block_on(
            crate::local_proxy::set_gpt_5_6_sol_context_window(
                app,
                argument(&args, "contextWindow")?,
            ),
        )),
        "get_official_model_context_settings" => serialize(block_on(
            crate::local_proxy::get_official_model_context_settings(app),
        )),
        "set_official_model_context_window" => serialize(block_on(
            crate::local_proxy::set_official_model_context_window(
                app,
                argument(&args, "model")?,
                argument(&args, "contextWindow")?,
            ),
        )),
        "set_upstream_429_retry_timeout" => serialize(block_on(
            crate::local_proxy::set_upstream_429_retry_timeout(
                app,
                argument(&args, "timeoutSeconds")?,
            ),
        )),
        "set_close_to_tray" => serialize(block_on(crate::main_window::set_close_to_tray(
            app.clone(),
            argument(&args, "enabled")?,
        ))),
        "set_launch_at_startup" => serialize(block_on(crate::autostart::set_launch_at_startup(
            app,
            argument(&args, "enabled")?,
        ))),
        "set_web_proxy_port" => {
            serialize(block_on(set_web_proxy_port(app, argument(&args, "port")?)))
        }
        "set_web_proxy_listen_on_all_interfaces" => serialize(block_on(
            set_web_proxy_listen_on_all_interfaces(app, argument(&args, "enabled")?),
        )),
        "copy_web_proxy_lan_api_key" => serialize(block_on(copy_web_proxy_lan_api_key(app))),
        "set_network_proxy" => serialize(block_on(crate::network_proxy::set_network_proxy(
            app,
            argument(&args, "settings")?,
        ))),
        "list_aggregate_apis" => serialize(block_on(crate::aggregate_api::list_aggregate_apis(app))),
        "list_providers" => serialize(crate::providers::list_providers(app)),
        "save_provider" => serialize(crate::providers::save_provider(
            app,
            argument(&args, "provider")?,
        )),
        "fetch_antigravity_models" => serialize(block_on(
            crate::antigravity_provider::fetch_antigravity_models(
                app,
                argument(&args, "baseUrl")?,
                argument(&args, "apiKey")?,
                argument(&args, "providerId")?,
            ),
        )),
        "fetch_claude_code_models" => serialize(block_on(
            crate::claude_code_provider::fetch_claude_code_models(
                app,
                argument(&args, "baseUrl")?,
                argument(&args, "apiKey")?,
                argument(&args, "providerId")?,
            ),
        )),
        "fetch_grok_models" => serialize(block_on(crate::grok_provider::fetch_grok_models(
            app,
            argument(&args, "baseUrl")?,
            argument(&args, "apiKey")?,
            argument(&args, "providerId")?,
        ))),
        "fetch_preset_models" => serialize(block_on(crate::preset_provider::fetch_preset_models(
            app,
            argument(&args, "request")?,
        ))),
        "detect_relay_platform" => {
            serialize(block_on(crate::provider_platform::detect_relay_platform(
                argument(&args, "baseUrl")?,
                argument(&args, "apiKey")?,
            )))
        }
        "fetch_relay_models" => serialize(block_on(crate::provider_models::fetch_relay_models(
            app,
            argument(&args, "baseUrl")?,
            argument(&args, "apiKey")?,
            argument(&args, "providerId")?,
        ))),
        "fetch_deepseek_models" => serialize(block_on(crate::providers::fetch_deepseek_models(
            app,
            argument(&args, "baseUrl")?,
            argument(&args, "apiKey")?,
            argument(&args, "providerId")?,
        ))),
        "query_provider_balance" => serialize(block_on(crate::providers::query_provider_balance(
            app,
            argument(&args, "id")?,
        ))),
        "query_provider_usage" => serialize(block_on(crate::providers::query_provider_usage(
            app,
            argument(&args, "id")?,
        ))),
        "prepare_safe_provider_switch" => serialize(block_on(
            crate::commands::prepare_safe_provider_switch(
            app,
            argument(&args, "id")?,
        ))),
        "prepare_safe_provider_group_switch" => serialize(block_on(
            crate::commands::prepare_safe_provider_group_switch(
            app,
            argument(&args, "group")?,
        ))),
        "prepare_safe_aggregate_switch" => serialize(block_on(
            crate::commands::prepare_safe_aggregate_switch(
                app,
                argument(&args, "id")?,
            ),
        )),
        "prepare_safe_provider_exit" => serialize(block_on(
            crate::commands::prepare_safe_provider_exit(app),
        )),
        "confirm_safe_provider_mutation" => serialize(block_on(
            crate::commands::confirm_safe_provider_mutation(
                app,
                argument(&args, "confirmToken")?,
            ),
        )),
        "prepare_safe_active_provider_edit" => serialize(block_on(
            crate::commands::prepare_safe_active_provider_edit(
                app,
                argument(&args, "provider")?,
            ),
        )),
        "prepare_safe_provider_group_edit" => serialize(block_on(
            crate::commands::prepare_safe_provider_group_edit(
                app,
                argument(&args, "id")?,
                argument(&args, "group")?,
            ),
        )),
        "prepare_safe_provider_model_switch" => serialize(block_on(
            crate::commands::prepare_safe_provider_model_switch(
                app,
                argument(&args, "id")?,
                argument(&args, "model")?,
            ),
        )),
        "prepare_safe_provider_model_control" => serialize(block_on(
            crate::commands::prepare_safe_provider_model_control(
                app,
                argument(&args, "id")?,
                argument(&args, "controlledByCodex")?,
            ),
        )),
        "confirm_safe_active_provider_edit" => serialize(block_on(
            crate::commands::confirm_safe_active_provider_edit(
                app,
                argument(&args, "confirmToken")?,
            ),
        )),
        "prepare_safe_active_aggregate_edit" => serialize(block_on(
            crate::commands::prepare_safe_active_aggregate_edit(
                app,
                argument(&args, "aggregate")?,
            ),
        )),
        "confirm_safe_active_aggregate_edit" => serialize(block_on(
            crate::commands::confirm_safe_active_aggregate_edit(
                app,
                argument(&args, "confirmToken")?,
            ),
        )),
        "switch_provider_model" => serialize(crate::providers::switch_provider_model(
            app,
            argument(&args, "id")?,
            argument(&args, "model")?,
        )),
        "set_provider_model_control" => serialize(crate::providers::set_provider_model_control(
            app,
            argument(&args, "id")?,
            argument(&args, "controlledByCodex")?,
        )),
        "set_provider_group" => serialize(block_on(crate::providers::set_provider_group(
            app,
            argument(&args, "id")?,
            argument(&args, "group")?,
        ))),
        "set_provider_groups" => serialize(block_on(crate::providers::set_provider_groups(
            app,
            argument(&args, "groups")?,
        ))),
        "set_provider_auto_switch_enabled" => {
            serialize(crate::providers::set_provider_auto_switch_enabled(
                app,
                argument(&args, "id")?,
                argument(&args, "enabled")?,
            ))
        }
        "delete_provider" => serialize(crate::providers::delete_provider(
            app,
            argument(&args, "id")?,
        )),
        "get_local_proxy_status" => {
            serialize(block_on(crate::local_proxy::get_local_proxy_status(app)))
        }
        "list_proxy_sessions" => serialize(block_on(crate::local_proxy::list_proxy_sessions(app))),
        "list_proxy_session_requests" => serialize(block_on(
            crate::local_proxy::list_proxy_session_requests(argument(&args, "sessionId")?),
        )),
        "get_recent_proxy_session_latency" => serialize(block_on(
            crate::local_proxy::get_recent_proxy_session_latency(),
        )),
        "list_token_usage_entries" => {
            serialize(block_on(crate::local_proxy::list_token_usage_entries(app)))
        }
        "list_token_usage_entries_since" => serialize(block_on(
            crate::local_proxy::list_token_usage_entries_since(
                app,
                argument(&args, "startTs")?,
            ),
        )),
        "list_daily_token_usage" => serialize(block_on(
            crate::local_proxy::list_daily_token_usage(app, argument(&args, "startTs")?),
        )),
        "list_account_token_usage" => serialize(block_on(
            crate::local_proxy::list_account_token_usage(app, argument(&args, "startTs")?),
        )),
        "list_provider_token_usage" => serialize(block_on(
            crate::local_proxy::list_provider_token_usage(app, argument(&args, "startTs")?),
        )),
        "start_local_proxy" => serialize(block_on(crate::local_proxy::start_local_proxy(app))),
        "stop_local_proxy" => serialize(block_on(crate::local_proxy::stop_local_proxy(app))),
        "stop_local_proxy_without_migrating" => serialize(block_on(
            crate::local_proxy::stop_local_proxy_without_migrating(app),
        )),
        "restore_non_proxy_conversations" => serialize(block_on(
            crate::commands::restore_non_proxy_conversations(app),
        )),
        "browse_codex_threads" => {
            serialize(crate::conversation_hub::browse_codex_threads_blocking(
                app,
                argument(&args, "titleQuery")?,
                argument(&args, "contentQuery")?,
            ))
        }
        "search_codex_threads" => serialize(crate::conversation_hub::search_codex_threads_blocking(
            app, argument(&args, "query")?, argument(&args, "clientId")?, argument(&args, "requestId")?,
        )),
        "cancel_codex_thread_search" => serialize(crate::conversation_hub::cancel_codex_thread_search_blocking(
            &argument::<String>(&args, "clientId")?, argument(&args, "requestId")?,
        )),
        "continue_codex_thread_search" => serialize(crate::conversation_hub::continue_codex_thread_search_blocking(
            argument(&args, "clientId")?, argument(&args, "requestId")?, argument(&args, "continuation")?,
        )),
        "get_codex_thread_revision" => serialize(
            crate::conversation_hub::get_codex_thread_revision_blocking(app),
        ),
        "measure_codex_thread_tokens" => serialize(
            crate::conversation_hub::measure_codex_thread_tokens_blocking(
                app,
                argument(&args, "sessionIds")?,
            ),
        ),
        "inspect_codex_thread_detail" => serialize(
            crate::conversation_hub::inspect_codex_thread_detail_blocking(
                app,
                argument(&args, "sessionId")?,
                argument(&args, "offset")?,
                argument(&args, "limit")?,
                argument(&args, "expectedRevision")?,
                argument(&args, "fromEnd")?,
            ),
        ),
        "prepare_codex_thread_discard" => {
            serialize(crate::conversation_hub::prepare_codex_thread_discard_blocking(
                app,
                argument(&args, "sessionIds")?,
            ))
        }
        "confirm_codex_thread_discard" => serialize(
            crate::conversation_hub::confirm_codex_thread_discard_blocking(
                app,
                argument(&args, "confirmToken")?,
            ),
        ),
        "browse_codex_thread_bin" => serialize(
            crate::conversation_hub::browse_codex_thread_bin_blocking(app),
        ),
        "prepare_codex_thread_restore" => serialize(
            crate::conversation_hub::prepare_codex_thread_restore_blocking(
                app,
                argument(&args, "sessionIds")?,
            ),
        ),
        "confirm_codex_thread_restore" => serialize(
            crate::conversation_hub::confirm_codex_thread_restore_blocking(
                app,
                argument(&args, "confirmToken")?,
            ),
        ),
        "prepare_codex_thread_purge" => serialize(
            crate::conversation_hub::prepare_codex_thread_purge_blocking(
                app,
                argument(&args, "sessionIds")?,
                argument(&args, "emptyBin")?,
            ),
        ),
        "confirm_codex_thread_purge" => serialize(
            crate::conversation_hub::confirm_codex_thread_purge_blocking(
                app,
                argument(&args, "confirmToken")?,
            ),
        ),
        "prepare_codex_thread_archive" => serialize(
            crate::conversation_hub::prepare_codex_thread_archive_blocking(
                app,
                argument(&args, "sessionIds")?,
            ),
        ),
        "confirm_codex_thread_archive" => serialize(
            crate::conversation_hub::confirm_codex_thread_archive_blocking(
                app,
                argument(&args, "confirmToken")?,
            ),
        ),
        "inspect_codex_thread_export" => serialize(
            crate::conversation_hub::inspect_codex_thread_export_blocking(
                app,
                argument(&args, "sessionIds")?,
            ),
        ),
        "pack_codex_threads" => serialize(crate::conversation_hub::pack_codex_threads_blocking(
            app,
            argument(&args, "sessionIds")?,
            argument(&args, "exportPath")?,
        )),
        "inspect_codex_thread_import" => serialize(
            crate::conversation_hub::inspect_codex_thread_import_blocking(
                app,
                argument(&args, "importPath")?,
            ),
        ),
        "unpack_codex_threads" => {
            serialize(crate::conversation_hub::unpack_codex_threads_blocking(
                app,
                argument(&args, "importPath")?,
                argument(&args, "sessionIds")?,
            ))
        }
        "migrate_codex_threads" => serialize(
            crate::conversation_hub::migrate_codex_threads_blocking(
                app,
                argument(&args, "sessionIds")?,
            ),
        ),
        "open_codex_thread_file" => {
            serialize(crate::conversation_hub::open_codex_thread_file_blocking(
                app,
                argument(&args, "sessionId")?,
                argument(&args, "folderOnly")?,
            ))
        }
        _ => dispatch_extended_command(app, command, args),
    }
}
