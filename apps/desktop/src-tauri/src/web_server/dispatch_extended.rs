fn dispatch_extended_command(app: AppHandle, command: &str, args: Value) -> Result<Value, String> {
    match command {
        "set_auto_switch_on_quota_exhaustion" => {
            serialize(crate::local_proxy::set_auto_switch_on_quota_exhaustion(
                app,
                argument(&args, "enabled")?,
            ))
        }
        "set_concurrent_account_routing_enabled" => {
            serialize(crate::local_proxy::set_concurrent_account_routing_enabled(
                app,
                argument(&args, "enabled")?,
            ))
        }
        "set_auto_disable_unreachable_accounts" => {
            serialize(crate::local_proxy::set_auto_disable_unreachable_accounts(
                app,
                argument(&args, "enabled")?,
            ))
        }
        "set_custom_auto_switch_priority_enabled" => {
            serialize(crate::local_proxy::set_custom_auto_switch_priority_enabled(
                app,
                argument(&args, "enabled")?,
            ))
        }
        "set_system_prompt_filter_enabled" => serialize(block_on(
            crate::local_proxy::set_system_prompt_filter_enabled(
                app,
                argument(&args, "enabled")?,
            ),
        )),
        "set_system_prompt_filter_rules" => serialize(block_on(
            crate::local_proxy::set_system_prompt_filter_rules(app, argument(&args, "rules")?),
        )),
        "set_system_prompt_injection_enabled" => serialize(block_on(
            crate::local_proxy::set_system_prompt_injection_enabled(
                app,
                argument(&args, "enabled")?,
            ),
        )),
        "set_system_prompt_injection_prompts" => serialize(block_on(
            crate::local_proxy::set_system_prompt_injection_prompts(
                app,
                argument(&args, "prompts")?,
            ),
        )),
        "set_custom_auto_switch_threshold_enabled" => {
            serialize(crate::local_proxy::set_custom_auto_switch_threshold_enabled(
                app,
                argument(&args, "enabled")?,
            ))
        }
        "set_global_auto_switch_threshold" => serialize(block_on(
            crate::local_proxy::set_global_auto_switch_threshold(
                app,
                argument(&args, "threshold")?,
            ),
        )),
        "set_image_generation_account" => serialize(
            crate::local_proxy::set_image_generation_account(app, argument(&args, "accountId")?),
        ),
        "set_image_model_target" => {
            serialize(block_on(crate::local_proxy::set_image_model_target(
                app,
                argument(&args, "routeKind")?,
                argument(&args, "target")?,
            )))
        }
        "set_local_proxy_openai_auth_account" => serialize(block_on(
            crate::local_proxy::set_local_proxy_openai_auth_account(
                app,
                argument(&args, "accountId")?,
            ),
        )),
        "set_local_proxy_listen_on_all_interfaces" => serialize(
            crate::local_proxy::set_local_proxy_listen_on_all_interfaces(
                app,
                argument(&args, "enabled")?,
                argument(&args, "apiKey")?,
            ),
        ),
        "copy_local_proxy_lan_api_key" => {
            serialize(crate::local_proxy::copy_local_proxy_lan_api_key(app))
        }
        "set_floating_bubble" => serialize(block_on(crate::floating_bubble::set_floating_bubble(
            app,
            argument(&args, "enabled")?,
        ))),
        "set_privacy_mode" => serialize(crate::floating_bubble::set_privacy_mode(
            app,
            argument(&args, "enabled")?,
        )),
        "set_hide_account_notes" => serialize(crate::floating_bubble::set_hide_account_notes(
            app,
            argument(&args, "enabled")?,
        )),
        "set_show_usage_network_errors" => serialize(
            crate::floating_bubble::set_show_usage_network_errors(app, argument(&args, "enabled")?),
        ),
        "set_token_usage_preferences" => {
            serialize(crate::floating_bubble::set_token_usage_preferences(
                app,
                argument(&args, "weeks")?,
                argument(&args, "refreshSeconds")?,
            ))
        }
        "set_auto_disable_status_codes" => serialize(
            crate::commands::set_auto_disable_status_codes(app, argument(&args, "statusCodes")?),
        ),
        "set_bubble_reset_display" => serialize(crate::floating_bubble::set_bubble_reset_display(
            app,
            argument(&args, "display")?,
        )),
        "set_bubble_style" => serialize(crate::floating_bubble::set_bubble_style(
            app,
            argument(&args, "style")?,
        )),
        "set_theme_color" => serialize(crate::floating_bubble::set_theme_color(
            app,
            argument(&args, "color")?,
        )),
        "set_app_language" => serialize(crate::floating_bubble::set_app_language(
            app,
            argument(&args, "language")?,
        )),
        "get_cloud_auth_state" => serialize(block_on(crate::cloud::get_cloud_auth_state(app))),
        "get_saved_cloud_login" => serialize(block_on(crate::cloud::get_saved_cloud_login(app))),
        "set_cloud_base_url" => serialize(block_on(crate::cloud::set_cloud_base_url(
            app,
            argument(&args, "baseUrl")?,
        ))),
        "cloud_login" => serialize(block_on(crate::cloud::cloud_login(
            app,
            argument(&args, "email")?,
            argument(&args, "password")?,
            argument(&args, "rememberPassword")?,
        ))),
        "fetch_cloud_announcement" => {
            serialize(block_on(crate::cloud::fetch_cloud_announcement(app)))
        }
        "fetch_cloud_currency_rates" => {
            serialize(block_on(crate::cloud::fetch_cloud_currency_rates(app)))
        }
        "fetch_cloud_notifications" => {
            serialize(block_on(crate::cloud::fetch_cloud_notifications(app)))
        }
        "fetch_cloud_faqs" => serialize(block_on(crate::cloud::fetch_cloud_faqs(app))),
        "cloud_request_registration_code" => serialize(block_on(
            crate::cloud::cloud_request_registration_code(app, argument(&args, "email")?),
        )),
        "cloud_register" => serialize(block_on(crate::cloud::cloud_register(
            app,
            argument(&args, "email")?,
            argument(&args, "password")?,
            argument(&args, "verificationCode")?,
            argument(&args, "rememberPassword")?,
        ))),
        "cloud_change_password" => serialize(block_on(crate::cloud::cloud_change_password(
            app,
            argument(&args, "currentPassword")?,
            argument(&args, "newPassword")?,
        ))),
        "cloud_logout" => serialize(block_on(crate::cloud::cloud_logout(app))),
        "cloud_sync_accounts" => serialize(block_on(crate::cloud::cloud_sync_accounts(app))),
        "cloud_pull_account" => serialize(block_on(crate::cloud::cloud_pull_account(
            app,
            argument(&args, "id")?,
        ))),
        "cloud_push_accounts" => serialize(block_on(crate::cloud::cloud_push_accounts(app))),
        "cloud_push_account" => serialize(block_on(crate::cloud::cloud_push_account(
            app,
            argument(&args, "id")?,
            argument(&args, "restoreDeleted")?,
        ))),
        "cloud_push_providers" => serialize(block_on(crate::cloud::cloud_push_providers(app))),
        "cloud_push_provider" => serialize(block_on(crate::cloud::cloud_push_provider(
            app,
            argument(&args, "id")?,
        ))),
        "cloud_delete_account" => serialize(block_on(crate::cloud::cloud_delete_account(
            app,
            argument(&args, "id")?,
        ))),
        "cloud_pull_totp" => serialize(block_on(crate::cloud::cloud_pull_totp(app))),
        "cloud_list_deleted_accounts" => {
            serialize(block_on(crate::cloud::cloud_list_deleted_accounts(app)))
        }
        "cloud_restore_deleted_account" => serialize(block_on(
            crate::cloud::cloud_restore_deleted_account(app, argument(&args, "id")?),
        )),
        "cloud_list_deleted_providers" => {
            serialize(block_on(crate::cloud::cloud_list_deleted_providers(app)))
        }
        "cloud_restore_deleted_provider" => serialize(block_on(
            crate::cloud::cloud_restore_deleted_provider(app, argument(&args, "id")?),
        )),
        "cloud_delete_provider" => serialize(block_on(crate::cloud::cloud_delete_provider(
            app,
            argument(&args, "id")?,
        ))),
        "list_market_skills" => serialize(block_on(crate::skills_market::list_market_skills(app))),
        "list_prompt_plugins" => serialize(block_on(crate::prompt_plugins::list_prompt_plugins(app))),
        "publish_prompt_plugin" => serialize(block_on(crate::prompt_plugins::publish_prompt_plugin(
            app,
            argument(&args, "pluginId")?,
            argument(&args, "name")?,
            argument(&args, "version")?,
            argument(&args, "type")?,
            argument(&args, "text")?,
        ))),
        "install_prompt_plugin" => serialize(block_on(crate::prompt_plugins::install_prompt_plugin(
            app,
            argument(&args, "pluginId")?,
        ))),
        "remove_prompt_plugin" => serialize(block_on(crate::prompt_plugins::remove_prompt_plugin(
            app,
            argument(&args, "pluginId")?,
        ))),
        "set_prompt_plugin_enabled" => serialize(block_on(crate::prompt_plugins::set_prompt_plugin_enabled(
            app,
            argument(&args, "pluginId")?,
            argument(&args, "enabled")?,
        ))),
        "install_market_skill" => serialize(block_on(crate::skills_market::install_market_skill(
            app,
            argument(&args, "skill")?,
        ))),
        "remove_market_skill" => serialize(block_on(crate::skills_market::remove_market_skill(
            app,
            argument(&args, "skillId")?,
        ))),
        "set_market_skill_enabled" => {
            serialize(block_on(crate::skills_market::set_market_skill_enabled(
                app,
                argument(&args, "skillId")?,
                argument(&args, "enabled")?,
            )))
        }
        "list_official_plugins" => serialize(block_on(
            crate::official_plugins::list_official_plugins(app),
        )),
        "install_official_plugin" => serialize(block_on(
            crate::official_plugins::install_official_plugin(app, argument(&args, "pluginId")?),
        )),
        "remove_official_plugin" => serialize(block_on(
            crate::official_plugins::remove_official_plugin(app, argument(&args, "pluginId")?),
        )),
        "set_official_plugin_enabled" => serialize(block_on(
            crate::official_plugins::set_official_plugin_enabled(
                app,
                argument(&args, "pluginId")?,
                argument(&args, "enabled")?,
            ),
        )),
        "prepare_safe_account_switch" => serialize(block_on(
            crate::commands::prepare_safe_account_switch(app, argument(&args, "id")?),
        )),
        "confirm_safe_account_switch" => serialize(block_on(
            crate::commands::confirm_safe_account_switch(
                app,
                argument(&args, "confirmToken")?,
            ),
        )),
        "prepare_safe_account_deactivation" => serialize(block_on(
            crate::commands::prepare_safe_account_deactivation(app),
        )),
        "confirm_safe_account_deactivation" => serialize(block_on(
            crate::commands::confirm_safe_account_deactivation(
                app,
                argument(&args, "confirmToken")?,
            ),
        )),
        "prepare_rollback_last_change" => serialize(block_on(
            crate::commands::prepare_rollback_last_change(app),
        )),
        "confirm_rollback_last_change" => serialize(block_on(
            crate::commands::confirm_rollback_last_change(
                app,
                argument(&args, "confirmToken")?,
            ),
        )),
        "set_account_auto_switch_enabled" => {
            serialize(crate::commands::set_account_auto_switch_enabled(
                app,
                argument(&args, "id")?,
                argument(&args, "enabled")?,
            ))
        }
        "set_account_auto_switch_priority" => {
            serialize(crate::commands::set_account_auto_switch_priority(
                app,
                argument(&args, "id")?,
                argument(&args, "priority")?,
            ))
        }
        "set_account_auto_switch_threshold" => serialize(block_on(
            crate::commands::set_account_auto_switch_threshold(
                app,
                argument(&args, "id")?,
                argument(&args, "threshold")?,
            ),
        )),
        "refresh_usage" => serialize(block_on(crate::commands::refresh_usage(
            app,
            argument(&args, "id")?,
        ))),
        "consume_account_quota" => serialize(block_on(crate::commands::consume_account_quota(
            app,
            argument(&args, "id")?,
        ))),
        "delete_account" => serialize(crate::commands::delete_account(app, argument(&args, "id")?)),
        "update_account_note" => serialize(block_on(crate::commands::update_account_note(
            app,
            argument(&args, "input")?,
        ))),
        "fetch_reset_credits" => serialize(block_on(crate::commands::fetch_reset_credits(
            app,
            argument(&args, "id")?,
        ))),
        "consume_reset_credit" => serialize(block_on(crate::commands::consume_reset_credit(
            app,
            argument(&args, "id")?,
        ))),
        "restart_chatgpt" => serialize(block_on(crate::commands::restart_chatgpt(app))),
        "launch_chatgpt" => serialize(block_on(crate::commands::launch_chatgpt(app))),
        "open_managed_folder" => serialize(crate::commands::open_managed_folder(
            app,
            argument(&args, "target")?,
        )),
        _ => Err(format!(
            "Command is not available in the web version: {command}"
        )),
    }
}
