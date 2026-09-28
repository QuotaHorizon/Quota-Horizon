    #[test]
    fn synchronizes_agent_identity_auth_to_local_codex_auth_json() {
        let paths = test_paths();
        let auth = agent_identity_auth();
        let (_, _, _, id) = crate::auth::account_fields(&auth).unwrap();
        write_json_atomic(&managed_auth_path(&paths, &id), &auth).unwrap();

        write_managed_auth_to_current(&paths, &id).unwrap();

        assert_eq!(read_json(&paths.current_auth).unwrap(), auth);
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn allows_agent_identity_switches_only_while_local_proxy_is_running() {
        let auth = agent_identity_auth();
        ensure_account_switch_allowed(&auth, true).unwrap();
        let error = ensure_account_switch_allowed(&auth, false).unwrap_err();
        assert!(error.contains("本地代理模式"));
    }

    #[test]
    fn background_auth_sync_defers_writes_while_client_is_running() {
        let paths = test_paths();
        let old_auth = json!({ "credential": "old" });
        let new_auth = json!({ "credential": "new" });
        write_json_atomic(&paths.current_auth, &old_auth).unwrap();

        assert!(!sync_current_auth_with_client_state(&paths, &new_auth, true).unwrap());
        assert_eq!(read_json(&paths.current_auth).unwrap(), old_auth);

        assert!(sync_current_auth_with_client_state(&paths, &new_auth, false).unwrap());
        assert_eq!(read_json(&paths.current_auth).unwrap(), new_auth);

        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn selected_managed_auth_replaces_stale_current_auth() {
        let paths = test_paths();
        let token = access_token();
        let selected = json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": token,
                "access_token": token,
                "refresh_token": "refresh-token"
            },
            "last_refresh": "2026-07-21T00:00:00Z"
        });
        let (_, _, _, selected_id) = crate::auth::account_fields(&selected).unwrap();
        write_json_atomic(&managed_auth_path(&paths, &selected_id), &selected).unwrap();
        write_json_atomic(&paths.current_auth, &json!({ "credential": "stale" })).unwrap();

        write_managed_auth_to_current(&paths, &selected_id).unwrap();

        assert_eq!(
            read_json(&paths.current_auth).unwrap(),
            read_json(&managed_auth_path(&paths, &selected_id)).unwrap()
        );
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn quota_credential_persistence_never_reapplies_even_the_selected_account() {
        let paths = test_paths();
        let token = access_token();
        let original = json!({"tokens": {"id_token": token, "access_token": "old-access", "refresh_token": "old-refresh"}});
        let id = crate::auth::account_fields(&original).unwrap().3;
        write_json_atomic(&managed_auth_path(&paths, &id), &original).unwrap();
        let selected = ManagerStateFile { active_account_id: Some(id.clone()), ..Default::default() };
        crate::storage::write_state(&paths, &selected).unwrap();
        write_json_atomic(&paths.current_auth, &original).unwrap();
        let before = fs::read(&paths.current_auth).unwrap();
        let mut request = RequestAuth::new(original);
        request.value["tokens"]["access_token"] = json!("renewed-access");
        assert!(request.persist(&paths, &id).unwrap());
        assert_eq!(fs::read(&paths.current_auth).unwrap(), before);
        assert_eq!(read_json(&managed_auth_path(&paths, &id)).unwrap()["tokens"]["access_token"], "renewed-access");
        assert!(!paths.current_config.exists());
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn runtime_renewal_is_adopted_before_retrying_a_rotated_managed_token() {
        let paths = test_paths();
        let token = access_token();
        let mut old = json!({"last_refresh": "2026-09-07T00:00:00Z", "tokens": {"id_token": token, "access_token": "old", "refresh_token": "old-refresh"}});
        crate::auth::canonicalize_chatgpt_auth(&mut old).unwrap();
        let id = crate::auth::account_fields(&old).unwrap().3;
        write_json_atomic(&managed_auth_path(&paths, &id), &old).unwrap();
        let mut new = old.clone();
        new["last_refresh"] = json!("2026-09-07T01:00:00Z");
        new["tokens"]["access_token"] = json!("runtime-new");
        new["tokens"]["refresh_token"] = json!("runtime-new-refresh");
        write_json_atomic(&paths.current_auth, &new).unwrap();
        let source = fs::read(&paths.current_auth).unwrap();
        let mut request = RequestAuth::new(old);
        request.refresh_with(&paths, &id, |_| panic!("already renewed; must not rotate the old token again")).unwrap();
        assert_eq!(request.value, new);
        assert_eq!(fs::read(&paths.current_auth).unwrap(), source);
        assert!(!paths.state_file.exists());
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn simultaneous_quota_requests_renew_a_rotating_token_only_once() {
        use std::sync::{atomic::{AtomicUsize, Ordering}, mpsc};
        let paths = test_paths();
        let token = access_token();
        let mut original = json!({"tokens": {"id_token": token, "access_token": "old-access", "refresh_token": "old-refresh"}});
        crate::auth::canonicalize_chatgpt_auth(&mut original).unwrap();
        let id = crate::auth::account_fields(&original).unwrap().3;
        write_json_atomic(&managed_auth_path(&paths, &id), &original).unwrap();
        let renewals = AtomicUsize::new(0);
        let (entered, entered_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let first_paths = &paths;
            let first_id = &id;
            let counter = &renewals;
            let first_original = original.clone();
            let first = scope.spawn(move || {
                let mut request = RequestAuth::new(first_original);
                request.refresh_with(first_paths, first_id, |auth| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    entered.send(()).unwrap();
                    release_rx.recv().unwrap();
                    auth["tokens"]["access_token"] = json!("new-access");
                    auth["tokens"]["refresh_token"] = json!("rotated-refresh");
                    Ok(())
                }).unwrap();
                request.value
            });
            entered_rx.recv().unwrap();
            let second = scope.spawn(|| {
                let mut request = RequestAuth::new(original.clone());
                request.refresh_with(&paths, &id, |_| {
                    renewals.fetch_add(1, Ordering::SeqCst);
                    Err("A rotating token was replayed".to_string())
                }).unwrap();
                request.value
            });
            release.send(()).unwrap();
            assert_eq!(first.join().unwrap(), second.join().unwrap());
        });
        assert_eq!(renewals.load(Ordering::SeqCst), 1);
        assert_eq!(read_json(&managed_auth_path(&paths, &id)).unwrap()["tokens"]["refresh_token"], "rotated-refresh");
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn in_flight_refresh_reloads_a_newer_login_instead_of_overwriting_it() {
        let paths = test_paths();
        let token = access_token();
        let stale = json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": token,
                "access_token": "stale-access",
                "refresh_token": "stale-refresh"
            },
            "last_refresh": "2026-08-30T01:00:00Z"
        });
        let (_, _, _, id) = crate::auth::account_fields(&stale).unwrap();
        let mut fresh = stale.clone();
        fresh["tokens"]["access_token"] = Value::String("fresh-login-access".to_string());
        fresh["tokens"]["refresh_token"] = Value::String("fresh-login-refresh".to_string());
        fresh["last_refresh"] = Value::String("2026-08-30T02:00:00Z".to_string());
        write_json_atomic(&managed_auth_path(&paths, &id), &fresh).unwrap();

        let mut request = RequestAuth::new(stale);
        request.value["tokens"]["access_token"] =
            Value::String("refreshed-stale-access".to_string());
        assert!(!request.persist(&paths, &id).unwrap());
        assert_eq!(request.value, fresh);
        assert_eq!(read_json(&managed_auth_path(&paths, &id)).unwrap(), fresh);

        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn updates_disabled_account_ids_without_duplicates() {
        let mut state = ManagerStateFile::default();

        assert!(update_disabled_account_ids(&mut state, "account-b", false));
        assert!(update_disabled_account_ids(&mut state, "account-a", false));
        assert!(!update_disabled_account_ids(&mut state, "account-a", false));
        assert_eq!(state.disabled_account_ids, ["account-a", "account-b"]);

        assert!(update_disabled_account_ids(&mut state, "account-a", true));
        assert!(!update_disabled_account_ids(&mut state, "account-a", true));
        assert_eq!(state.disabled_account_ids, ["account-b"]);
    }

    #[test]
    fn usage_refresh_failures_only_disable_enabled_access_rejections() {
        assert!(should_disable_account_auto_switch(
            "Codex usage endpoint returned HTTP 401 Unauthorized",
            true,
            &[401, 402, 403],
        ));
        assert!(should_disable_account_auto_switch(
            "Codex usage endpoint returned HTTP 403 Forbidden",
            true,
            &[401, 402, 403],
        ));
        assert!(should_disable_account_auto_switch(
            "Codex usage endpoint returned HTTP 402 Payment Required",
            true,
            &[401, 402, 403],
        ));

        assert!(!should_disable_account_auto_switch(
            "Codex usage endpoint returned HTTP 401 Unauthorized",
            false,
            &[401, 402, 403],
        ));
        assert!(!should_disable_account_auto_switch(
            "Codex usage endpoint returned HTTP 403 Forbidden",
            false,
            &[401, 402, 403],
        ));
        assert!(!should_disable_account_auto_switch(
            "Codex usage endpoint returned HTTP 402 Payment Required",
            false,
            &[401, 402, 403],
        ));
        assert!(!should_disable_account_auto_switch(
            "failed to read Codex usage: error sending request",
            true,
            &[401, 402, 403],
        ));
        assert!(!should_disable_account_auto_switch(
            "failed to read Codex usage: operation timed out",
            true,
            &[401, 402, 403],
        ));
        assert!(!should_disable_account_auto_switch(
            "Codex usage endpoint returned HTTP 503 Service Unavailable",
            true,
            &[401, 402, 403],
        ));
        assert!(should_disable_account_auto_switch(
            "Codex usage endpoint returned HTTP 429 Too Many Requests",
            true,
            &[429],
        ));
        assert!(!should_disable_account_auto_switch(
            "Codex usage endpoint returned HTTP 401 Unauthorized",
            true,
            &[429],
        ));
    }

    #[test]
    fn usage_network_errors_exclude_explicit_http_statuses() {
        assert!(is_usage_network_error(
            "failed to read Codex usage: error sending request for url"
        ));
        assert!(is_usage_network_error(
            "failed to read Codex usage: operation timed out"
        ));
        assert!(is_usage_network_error("DNS lookup failed"));
        assert!(!is_usage_network_error(
            "Codex usage endpoint returned HTTP 503 Service Unavailable"
        ));
        assert!(!is_usage_network_error("failed to parse Codex usage"));
    }

    #[test]
    fn failed_usage_refresh_preserves_the_last_successful_observation_time() {
        let cached = UsageSummary {
            fetched_at: Some("2026-09-04T07:00:00Z".to_string()),
            plan: Some("pro".to_string()),
            ..UsageSummary::default()
        };

        let visible = usage_with_refresh_failure(cached.clone(), "network timeout", true);
        assert_eq!(visible.fetched_at.as_deref(), Some("2026-09-04T07:00:00Z"));
        assert_eq!(visible.plan.as_deref(), Some("pro"));
        assert_eq!(visible.error.as_deref(), Some("network timeout"));

        let quiet = usage_with_refresh_failure(cached, "network timeout", false);
        assert_eq!(quiet.fetched_at.as_deref(), Some("2026-09-04T07:00:00Z"));
        assert_eq!(quiet.error, None);
    }
