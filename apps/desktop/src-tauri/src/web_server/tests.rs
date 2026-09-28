#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_paths_default_to_the_index_and_strip_queries() {
        assert_eq!(asset_path("/"), Some("index.html".to_string()));
        assert_eq!(
            asset_path("/assets/index.js?v=1"),
            Some("assets/index.js".to_string())
        );
    }

    #[test]
    fn asset_paths_reject_traversal_and_backslashes() {
        assert_eq!(asset_path("/../settings.json"), None);
        assert_eq!(asset_path("/assets\\index.js"), None);
        assert_eq!(asset_path("/assets//index.js"), None);
    }

    #[test]
    fn port_zero_is_invalid_and_the_default_is_disabled() {
        assert!(validate_port(0).is_err());
        assert!(validate_port(1).is_ok());
        assert!(AppSettings::default().web_proxy_port.is_none());
        assert!(!AppSettings::default().web_proxy_listen_on_all_interfaces);
    }

    #[test]
    fn web_server_configuration_changes_when_lan_listening_changes() {
        let loopback = WebServerConfiguration {
            port: 18_080,
            listen_on_all_interfaces: false,
            lan_api_key: None,
        };
        let lan = WebServerConfiguration {
            listen_on_all_interfaces: true,
            ..loopback.clone()
        };

        assert!(loopback != lan);
        assert_eq!(web_server_host(false), "127.0.0.1");
        assert_eq!(web_server_host(true), "0.0.0.0");
    }

    #[test]
    fn hosted_index_includes_the_runtime_marker_once() {
        let source = b"<!doctype html><html><head></head><body></body></html>";
        let injected = inject_hosted_runtime_marker(source);
        let injected_again = inject_hosted_runtime_marker(&injected);
        let html = String::from_utf8(injected_again).unwrap();

        assert_eq!(html.matches(HOSTED_RUNTIME_MARKER).count(), 1);
    }

    #[test]
    fn lan_requests_have_an_explicit_read_only_allowlist() {
        assert!(LAN_COMMAND_ALLOWLIST.contains(&"list_accounts"));
        assert!(LAN_COMMAND_ALLOWLIST.contains(&"get_app_settings"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"cloud_login"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"delete_account"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"save_provider"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"fetch_relay_models"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"launch_chatgpt"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_safe_account_switch"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"confirm_safe_account_switch"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_safe_account_deactivation"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"confirm_safe_account_deactivation"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_safe_provider_switch"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_safe_provider_group_switch"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_safe_aggregate_switch"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_safe_provider_exit"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"confirm_safe_provider_mutation"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_safe_active_provider_edit"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_safe_provider_group_edit"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_safe_provider_model_switch"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_safe_provider_model_control"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"confirm_safe_active_provider_edit"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_safe_active_aggregate_edit"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"confirm_safe_active_aggregate_edit"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_rollback_last_change"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"confirm_rollback_last_change"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"capacity_get_status"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"capacity_get_cached_status"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"capacity_authorize_history_keychain"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"capacity_get_history"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"inspect_codex_thread_detail"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"search_codex_threads"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"cancel_codex_thread_search"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"continue_codex_thread_search"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"get_public_reset_timeline"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"refresh_public_reset_timeline"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_codex_thread_discard"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"confirm_codex_thread_discard"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_codex_thread_restore"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"confirm_codex_thread_restore"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_codex_thread_purge"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"confirm_codex_thread_purge"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"prepare_codex_thread_archive"));
        assert!(!LAN_COMMAND_ALLOWLIST.contains(&"confirm_codex_thread_archive"));
    }

    #[test]
    fn lan_api_keys_are_random_and_constant_time_comparison_rejects_variants() {
        let first = generate_web_lan_api_key();
        let second = generate_web_lan_api_key();
        assert!(first.starts_with(WEB_LAN_API_KEY_PREFIX));
        assert_eq!(first.len(), WEB_LAN_API_KEY_PREFIX.len() + 43);
        assert_ne!(first, second);
        assert!(constant_time_equal(&first, &first));
        assert!(!constant_time_equal(&first, &second));
        assert!(!constant_time_equal(&first, &first[..first.len() - 1]));
    }

    #[test]
    fn non_loopback_origins_must_match_the_listener_host() {
        assert!(same_origin_values(
            Some("http://192.168.1.20:18765"),
            Some("192.168.1.20:18765"),
        ));
        assert!(same_origin_values(None, Some("192.168.1.20:18765")));
        assert!(!same_origin_values(
            Some("http://attacker.invalid:18765"),
            Some("192.168.1.20:18765"),
        ));
        assert!(!same_origin_values(
            Some("https://192.168.1.20:18765"),
            Some("192.168.1.20:18765"),
        ));
    }
}
