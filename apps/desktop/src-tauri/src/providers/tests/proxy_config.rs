    #[test]
    fn providers_can_only_switch_while_proxy_is_running() {
        assert!(!provider_switch_supported(false));
        assert!(provider_switch_supported(true));
    }

    #[test]
    fn safe_provider_revision_binds_credential_and_routing_fields_without_exposing_them() {
        let first = provider();
        let mut changed = first.clone();
        changed.api_key = "replacement-secret".to_string();
        let first_revision = provider_target_revision(
            &PreparedProviderMutationTarget::Provider(Box::new(first)),
        )
        .unwrap();
        let changed_revision = provider_target_revision(
            &PreparedProviderMutationTarget::Provider(Box::new(changed)),
        )
        .unwrap();

        assert_eq!(first_revision.len(), 64);
        assert_ne!(first_revision, changed_revision);
        assert!(!changed_revision.contains("replacement-secret"));
    }

    #[test]
    fn safe_active_provider_edit_revision_binds_old_and_new_credentials() {
        let current = provider();
        let mut updated = current.clone();
        updated.api_key = "updated-active-secret".to_string();
        let revision = active_provider_edit_revision(&current, &updated).unwrap();
        let mut drifted = current.clone();
        drifted.api_key = "externally-drifted-secret".to_string();
        let drifted_revision = active_provider_edit_revision(&drifted, &updated).unwrap();

        assert_eq!(revision.len(), 64);
        assert_ne!(revision, drifted_revision);
        assert!(!revision.contains("updated-active-secret"));
        assert!(!drifted_revision.contains("externally-drifted-secret"));
    }

    #[test]
    fn active_provider_profile_and_field_metadata_update_together() {
        let paths = test_paths();
        let original = provider();
        let mut updated = original.clone();
        updated.name = "Updated Gateway".to_string();
        write_provider(&paths, &original).unwrap();
        write_state(
            &paths,
            &crate::models::ManagerStateFile {
                active_provider_id: Some(original.id.clone()),
                ..crate::models::ManagerStateFile::default()
            },
        )
        .unwrap();
        let context = ActiveProviderEditContext {
            paths: paths.clone(),
            original_state: read_state(&paths),
            original_profile: original.clone(),
            updated_profile: updated.clone(),
            prepared_route: PreparedActiveProviderEditRoute::Single,
            revision: active_provider_edit_revision(&original, &updated).unwrap(),
            proxy_running: true,
            write_codex: false,
        };

        write_active_provider_profile_for_transaction(&paths, &updated, &original).unwrap();
        verify_active_provider_edit_postcondition(&context).unwrap();
        assert_eq!(read_provider(&paths, &updated.id).unwrap().name, "Updated Gateway");
        assert!(provider_field_modified_at_path(&paths, &updated.id).exists());
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn active_provider_group_member_edit_rebuilds_the_confirmed_member_set() {
        let paths = test_paths();
        let mut original = provider();
        original.group = "production".to_string();
        let mut peer = original.clone();
        peer.id = "peer".to_string();
        peer.name = "Peer Gateway".to_string();
        peer.model = "gpt-4.2".to_string();
        peer.models = vec!["gpt-4.2".to_string()];
        let mut updated = original.clone();
        updated.name = "Updated Gateway".to_string();
        write_provider(&paths, &original).unwrap();
        write_provider(&paths, &peer).unwrap();
        write_state(
            &paths,
            &crate::models::ManagerStateFile {
                active_provider_group: Some("production".to_string()),
                ..crate::models::ManagerStateFile::default()
            },
        )
        .unwrap();
        let current_group = provider_group_profiles(&paths, "production").unwrap();
        let expected_group = vec![updated.clone(), peer.clone()];
        let context = ActiveProviderEditContext {
            paths: paths.clone(),
            original_state: read_state(&paths),
            original_profile: original.clone(),
            updated_profile: updated.clone(),
            prepared_route: PreparedActiveProviderEditRoute::Group {
                group: "production".to_string(),
                providers: expected_group.clone(),
            },
            revision: active_provider_group_edit_revision(
                "production",
                &current_group,
                &original,
                &updated,
                &expected_group,
            )
            .unwrap(),
            proxy_running: true,
            write_codex: false,
        };

        write_active_provider_profile_for_transaction(&paths, &updated, &original).unwrap();
        verify_active_provider_edit_postcondition(&context).unwrap();
        assert_eq!(
            provider_profiles_revision(
                &provider_group_profiles(&paths, "production").unwrap()
            )
            .unwrap(),
            provider_profiles_revision(&expected_group).unwrap()
        );
        assert!(provider_field_modified_at_path(&paths, &updated.id).exists());
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn safe_group_edit_revision_binds_every_current_member() {
        let mut current = provider();
        current.group = "production".to_string();
        let mut peer = current.clone();
        peer.id = "peer".to_string();
        peer.name = "Peer Gateway".to_string();
        let current_group = vec![current.clone(), peer.clone()];
        let mut updated = current.clone();
        updated.name = "Updated Gateway".to_string();
        let updated_group = vec![updated.clone(), peer.clone()];
        let revision = active_provider_group_edit_revision(
            "production",
            &current_group,
            &current,
            &updated,
            &updated_group,
        )
        .unwrap();
        peer.api_key = "peer-secret-drift".to_string();
        let drifted_group = vec![current.clone(), peer];
        let drifted_revision = active_provider_group_edit_revision(
            "production",
            &drifted_group,
            &current,
            &updated,
            &updated_group,
        )
        .unwrap();

        assert_ne!(revision, drifted_revision);
        assert!(!drifted_revision.contains("peer-secret-drift"));
    }

    #[test]
    fn active_aggregate_member_edit_rebuilds_the_confirmed_route() {
        let paths = test_paths();
        let original = provider();
        let mut peer = original.clone();
        peer.id = "peer".to_string();
        peer.name = "Peer Gateway".to_string();
        let mut updated = original.clone();
        updated.name = "Updated Gateway".to_string();
        write_provider(&paths, &original).unwrap();
        write_provider(&paths, &peer).unwrap();
        let config = crate::aggregate_api::AggregateApiConfig {
            id: "aggregate-a".to_string(),
            name: "Aggregate A".to_string(),
            model: "gpt-4.1".to_string(),
            member_provider_ids: vec![original.id.clone(), peer.id.clone()],
            enabled: true,
        };
        write_json_atomic(
            &paths
                .providers
                .parent()
                .unwrap()
                .join("aggregate-apis.json"),
            &serde_json::to_value(vec![config.clone()]).unwrap(),
        )
        .unwrap();
        write_state(
            &paths,
            &crate::models::ManagerStateFile {
                active_provider_id: Some(crate::aggregate_api::active_id(&config.id)),
                ..crate::models::ManagerStateFile::default()
            },
        )
        .unwrap();
        let current_members = crate::aggregate_api::member_profiles(&paths, &config).unwrap();
        let expected_members = vec![updated.clone(), peer.clone()];
        let context = ActiveProviderEditContext {
            paths: paths.clone(),
            original_state: read_state(&paths),
            original_profile: original.clone(),
            updated_profile: updated.clone(),
            prepared_route: PreparedActiveProviderEditRoute::Aggregate {
                config: Box::new(config.clone()),
                providers: expected_members.clone(),
            },
            revision: active_aggregate_member_edit_revision(
                &config,
                &current_members,
                &original,
                &updated,
                &expected_members,
            )
            .unwrap(),
            proxy_running: true,
            write_codex: false,
        };

        write_active_provider_profile_for_transaction(&paths, &updated, &original).unwrap();
        verify_active_provider_edit_postcondition(&context).unwrap();
        assert_eq!(
            provider_profiles_revision(
                &crate::aggregate_api::member_profiles(&paths, &config).unwrap()
            )
            .unwrap(),
            provider_profiles_revision(&expected_members).unwrap()
        );
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn safe_aggregate_member_revision_binds_peer_credentials() {
        let current = provider();
        let mut peer = current.clone();
        peer.id = "peer".to_string();
        peer.name = "Peer Gateway".to_string();
        let config = crate::aggregate_api::AggregateApiConfig {
            id: "aggregate-a".to_string(),
            name: "Aggregate A".to_string(),
            model: "gpt-4.1".to_string(),
            member_provider_ids: vec![current.id.clone(), peer.id.clone()],
            enabled: true,
        };
        let current_members = vec![current.clone(), peer.clone()];
        let mut updated = current.clone();
        updated.name = "Updated Gateway".to_string();
        let updated_members = vec![updated.clone(), peer.clone()];
        let revision = active_aggregate_member_edit_revision(
            &config,
            &current_members,
            &current,
            &updated,
            &updated_members,
        )
        .unwrap();
        peer.api_key = "peer-aggregate-drift".to_string();
        let drifted_members = vec![current.clone(), peer];
        let drifted_revision = active_aggregate_member_edit_revision(
            &config,
            &drifted_members,
            &current,
            &updated,
            &updated_members,
        )
        .unwrap();

        assert_ne!(revision, drifted_revision);
        assert!(!drifted_revision.contains("peer-aggregate-drift"));
    }

    #[test]
    fn model_edit_preparation_preserves_the_existing_profile() {
        let paths = test_paths();
        let mut original = provider();
        original.models.push("gpt-4.2".to_string());
        write_provider(&paths, &original).unwrap();

        let switched = prepare_provider_model_switch(&paths, &original.id, "gpt-4.2").unwrap();
        assert_eq!(switched.profile.model, "gpt-4.2");
        assert_eq!(switched.profile.api_key, original.api_key);
        let controlled =
            prepare_provider_model_control(&paths, &original.id, true).unwrap();
        assert!(controlled.profile.model_selection_controlled_by_codex);
        assert_eq!(controlled.profile.api_key, original.api_key);
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn active_aggregate_disable_updates_store_and_exits_to_direct_official_route() {
        let paths = test_paths();
        fs::create_dir_all(&paths.codex_home).unwrap();
        let first = provider();
        let mut second = first.clone();
        second.id = "peer".to_string();
        second.name = "Peer Gateway".to_string();
        write_provider(&paths, &first).unwrap();
        write_provider(&paths, &second).unwrap();
        let original = crate::aggregate_api::AggregateApiConfig {
            id: "aggregate-a".to_string(),
            name: "Aggregate A".to_string(),
            model: "gpt-4.1".to_string(),
            member_provider_ids: vec![first.id.clone(), second.id.clone()],
            enabled: true,
        };
        let mut updated = original.clone();
        updated.enabled = false;
        write_json_atomic(
            &paths
                .providers
                .parent()
                .unwrap()
                .join("aggregate-apis.json"),
            &serde_json::to_value(vec![original.clone()]).unwrap(),
        )
        .unwrap();
        let original_state = crate::models::ManagerStateFile {
            active_provider_id: Some(crate::aggregate_api::active_id(&original.id)),
            ..crate::models::ManagerStateFile::default()
        };
        write_state(&paths, &original_state).unwrap();
        fs::write(
            &paths.current_config,
            format!(
                "model_provider = \"codex-switch-local\"\n\n[model_providers.codex-switch-local]\nbase_url = \"{LOCAL_PROXY_BASE_URL}\"\n"
            ),
        )
        .unwrap();
        fs::write(&paths.config_backup, "model = \"gpt-5.6-sol\"\n").unwrap();
        fs::write(provider_model_catalog_path(&paths), b"aggregate catalog").unwrap();
        let original_members = crate::aggregate_api::member_profiles(&paths, &original).unwrap();
        let updated_members = crate::aggregate_api::member_profiles(&paths, &updated).unwrap();
        let context = ActiveAggregateEditContext {
            paths: paths.clone(),
            original_state,
            original_config: original.clone(),
            updated_config: updated.clone(),
            updated_members: updated_members.clone(),
            revision: active_aggregate_config_edit_revision(
                &original,
                &updated,
                &original_members,
                &updated_members,
            )
            .unwrap(),
            proxy_running: false,
            write_codex: true,
        };

        apply_active_aggregate_edit_files_without_binding(&context).unwrap();
        verify_active_aggregate_edit_postcondition(&context).unwrap();
        assert!(!crate::aggregate_api::read_config(&paths, &original.id)
            .unwrap()
            .enabled);
        assert!(read_state(&paths).active_provider_id.is_none());
        assert!(!codex_config::contains_local_proxy(
            &fs::read_to_string(&paths.current_config).unwrap()
        ));
        assert!(!paths.config_backup.exists());
        assert!(!provider_model_catalog_path(&paths).exists());
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn safe_aggregate_config_revision_binds_all_members() {
        let first = provider();
        let mut second = first.clone();
        second.id = "peer".to_string();
        second.name = "Peer Gateway".to_string();
        let original = crate::aggregate_api::AggregateApiConfig {
            id: "aggregate-a".to_string(),
            name: "Aggregate A".to_string(),
            model: "gpt-4.1".to_string(),
            member_provider_ids: vec![first.id.clone(), second.id.clone()],
            enabled: true,
        };
        let mut updated = original.clone();
        updated.name = "Aggregate Updated".to_string();
        let members = vec![first.clone(), second.clone()];
        let revision =
            active_aggregate_config_edit_revision(&original, &updated, &members, &members).unwrap();
        second.api_key = "aggregate-config-peer-drift".to_string();
        let drifted_members = vec![first, second];
        let drifted_revision = active_aggregate_config_edit_revision(
            &original,
            &updated,
            &drifted_members,
            &members,
        )
        .unwrap();

        assert_ne!(revision, drifted_revision);
        assert!(!drifted_revision.contains("aggregate-config-peer-drift"));
    }

    #[test]
    fn safe_aggregate_revision_binds_every_member_credential_without_exposing_it() {
        let first = provider();
        let mut second = first.clone();
        second.id = "p2".to_string();
        second.name = "Gateway 2".to_string();
        let config = crate::aggregate_api::AggregateApiConfig {
            id: "aggregate-a".to_string(),
            name: "Aggregate A".to_string(),
            model: "gpt-4.1".to_string(),
            member_provider_ids: vec![first.id.clone(), second.id.clone()],
            enabled: true,
        };
        let original_revision = provider_target_revision(
            &PreparedProviderMutationTarget::Aggregate {
                config: Box::new(config.clone()),
                providers: vec![first.clone(), second.clone()],
            },
        )
        .unwrap();
        second.api_key = "aggregate-replacement-secret".to_string();
        let changed_revision = provider_target_revision(
            &PreparedProviderMutationTarget::Aggregate {
                config: Box::new(config),
                providers: vec![first, second],
            },
        )
        .unwrap();

        assert_eq!(original_revision.len(), 64);
        assert_ne!(original_revision, changed_revision);
        assert!(!changed_revision.contains("aggregate-replacement-secret"));
    }

    #[test]
    fn safe_aggregate_files_enter_with_semantic_postflight() {
        let paths = test_paths();
        let original_state = crate::models::ManagerStateFile::default();
        write_state(&paths, &original_state).unwrap();
        write_text_atomic(&paths.current_config, "model = \"gpt-5.6-sol\"\n").unwrap();
        let first = provider();
        let mut second = first.clone();
        second.id = "p2".to_string();
        second.name = "Gateway 2".to_string();
        let config = crate::aggregate_api::AggregateApiConfig {
            id: "aggregate-a".to_string(),
            name: "Aggregate A".to_string(),
            model: "gpt-4.1".to_string(),
            member_provider_ids: vec![first.id.clone(), second.id.clone()],
            enabled: true,
        };
        let enter = ProviderMutationContext {
            target: ProviderRuntimeSelection::Aggregate(config.id.clone()),
            current: ProviderRuntimeSelection::Official,
            target_label: config.name.clone(),
            target_revision: "aggregate-revision".to_string(),
            proxy_running: true,
            write_codex: true,
            paths: paths.clone(),
            original_state,
            prepared: PreparedProviderMutationTarget::Aggregate {
                config: Box::new(config),
                providers: vec![first, second],
            },
        };

        apply_provider_mutation_files_without_binding(&enter).unwrap();
        verify_provider_mutation_postcondition(&enter).unwrap();
        assert_eq!(
            read_state(&paths).active_provider_id.as_deref(),
            Some("aggregate:aggregate-a")
        );
        assert!(paths.config_backup.exists());
        assert!(provider_model_catalog_path(&paths).exists());
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn aggregate_runtime_identity_is_distinct_from_a_provider_profile() {
        let state = crate::models::ManagerStateFile {
            active_provider_id: Some("aggregate:aggregate-a".to_string()),
            ..crate::models::ManagerStateFile::default()
        };

        assert!(matches!(
            provider_runtime_selection(&state),
            ProviderRuntimeSelection::Aggregate(id) if id == "aggregate-a"
        ));
    }

    #[test]
    fn safe_provider_files_enter_and_exit_with_semantic_postflight() {
        let paths = test_paths();
        let original_state = crate::models::ManagerStateFile::default();
        write_state(&paths, &original_state).unwrap();
        write_text_atomic(&paths.current_config, "model = \"gpt-5.6-sol\"\n").unwrap();
        let profile = provider();
        let enter = ProviderMutationContext {
            target: ProviderRuntimeSelection::Provider(profile.id.clone()),
            current: ProviderRuntimeSelection::Official,
            target_label: profile.name.clone(),
            target_revision: "provider-revision".to_string(),
            proxy_running: true,
            write_codex: true,
            paths: paths.clone(),
            original_state,
            prepared: PreparedProviderMutationTarget::Provider(Box::new(profile)),
        };

        apply_provider_mutation_files_without_binding(&enter).unwrap();
        verify_provider_mutation_postcondition(&enter).unwrap();
        assert!(paths.config_backup.exists());
        assert!(provider_model_catalog_path(&paths).exists());

        let provider_state = read_state(&paths);
        let exit = ProviderMutationContext {
            target: ProviderRuntimeSelection::Official,
            current: ProviderRuntimeSelection::Provider("p".to_string()),
            target_label: "Official Codex".to_string(),
            target_revision: "official-revision".to_string(),
            proxy_running: false,
            write_codex: true,
            paths: paths.clone(),
            original_state: provider_state,
            prepared: PreparedProviderMutationTarget::Official,
        };
        apply_provider_mutation_files_without_binding(&exit).unwrap();
        verify_provider_mutation_postcondition(&exit).unwrap();
        assert!(!paths.config_backup.exists());
        assert!(!provider_model_catalog_path(&paths).exists());
        assert!(!codex_config::contains_local_proxy(
            &fs::read_to_string(&paths.current_config).unwrap()
        ));
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn new_provider_ids_are_random_version_four_uuids() {
        let paths = test_paths();
        let first = unique_provider_id(&paths);
        let second = unique_provider_id(&paths);

        assert_eq!(Uuid::parse_str(&first).unwrap().get_version_num(), 4);
        assert_eq!(Uuid::parse_str(&second).unwrap().get_version_num(), 4);
        assert_ne!(first, second);
    }

    #[test]
    fn startup_restores_legacy_direct_provider_config() {
        let paths = test_paths();
        let official_config = "model = \"gpt-5.5\"\n";
        let legacy_provider_config = format!(
            "# Codex Switch provider start\n\
             model_provider = \"custom\"\n\
             model = \"gpt-4.1\"\n\
             # Codex Switch provider end\n\n\
             {official_config}\n\
             # Codex Switch custom provider start\n\
             [model_providers.custom]\n\
             base_url = \"https://gateway.example.com/v1\"\n\
             # Codex Switch custom provider end\n"
        );
        write_text_atomic(&paths.config_backup, official_config).unwrap();
        write_text_atomic(&paths.current_config, &legacy_provider_config).unwrap();
        write_state(
            &paths,
            &crate::models::ManagerStateFile {
                active_account_id: Some("official-account".to_string()),
                active_provider_id: Some("p".to_string()),
                ..crate::models::ManagerStateFile::default()
            },
        )
        .unwrap();

        cleanup_non_proxy_provider_state(&paths).unwrap();

        let restored = fs::read_to_string(&paths.current_config).unwrap();
        let document = restored.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(document["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(document["model_provider"].as_str(), Some("openai"));
        assert!(!restored.contains("https://gateway.example.com/v1"));
        assert!(!paths.config_backup.exists());
        let state = read_state(&paths);
        assert_eq!(state.active_account_id.as_deref(), Some("official-account"));
        assert!(state.active_provider_id.is_none());
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn empty_config_backup_is_replaced_by_the_current_config() {
        let paths = test_paths();
        write_text_atomic(&paths.config_backup, "\n").unwrap();
        let current = "model = \"gpt-5.5\"\n\n[features]\njs_repl = true\n";
        write_text_atomic(&paths.current_config, current).unwrap();

        backup_codex_config_if_needed(&paths, true).unwrap();

        assert_eq!(fs::read_to_string(&paths.config_backup).unwrap(), current);
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn empty_config_backup_does_not_get_created_without_a_current_config() {
        let paths = test_paths();

        backup_codex_config_if_needed(&paths, true).unwrap();

        assert!(!paths.config_backup.exists());
    }

    #[test]
    fn official_proxy_without_login_selection_removes_current_auth() {
        let paths = test_paths();
        let auth = test_auth();
        let (_, _, _, id) = crate::auth::account_fields(&auth).unwrap();
        write_json_atomic(&managed_auth_path(&paths, &id), &auth).unwrap();
        write_json_atomic(&paths.current_auth, &auth).unwrap();
        write_state(
            &paths,
            &crate::models::ManagerStateFile {
                active_account_id: Some(id),
                ..crate::models::ManagerStateFile::default()
            },
        )
        .unwrap();

        apply_local_proxy_config_for_paths(&paths).unwrap();

        assert!(!paths.current_auth.exists());
        assert!(fs::read_to_string(&paths.current_config)
            .unwrap()
            .contains("requires_openai_auth = false"));
        assert_eq!(
            codex_config::root_model(&fs::read_to_string(&paths.current_config).unwrap())
                .as_deref(),
            Some(DEFAULT_OFFICIAL_MODEL)
        );
        let root = paths.codex_home.parent().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn official_proxy_can_start_without_any_account() {
        let paths = test_paths();

        ensure_local_proxy_compatible_for_state(&paths).unwrap();
        apply_local_proxy_config_for_paths(&paths).unwrap();

        assert!(!paths.current_auth.exists());
        assert!(fs::read_to_string(&paths.current_config)
            .unwrap()
            .contains("requires_openai_auth = false"));
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn provider_activation_removes_stale_auth_without_openai_login_selection() {
        let paths = test_paths();
        let auth = test_auth();
        let (_, _, _, id) = crate::auth::account_fields(&auth).unwrap();
        write_json_atomic(&managed_auth_path(&paths, &id), &auth).unwrap();
        write_json_atomic(&paths.current_auth, &auth).unwrap();
        write_provider(&paths, &provider()).unwrap();
        write_state(
            &paths,
            &crate::models::ManagerStateFile {
                active_provider_id: Some("p".to_string()),
                local_proxy_openai_auth_account_id: None,
                ..crate::models::ManagerStateFile::default()
            },
        )
        .unwrap();

        // This is the path used by a hot Provider switch. It must enforce the
        // same auth-file invariant as starting the proxy from scratch.
        write_provider_local_proxy_config(&paths, &provider()).unwrap();

        assert!(!paths.current_auth.exists());
        assert_eq!(read_json(&managed_auth_path(&paths, &id)).unwrap(), auth);
        assert!(fs::read_to_string(&paths.current_config)
            .unwrap()
            .contains("requires_openai_auth = false"));
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn official_proxy_config_restores_selected_login_auth_after_provider_switch() {
        let paths = test_paths();
        let auth = test_auth();
        let (_, _, _, id) = crate::auth::account_fields(&auth).unwrap();
        write_json_atomic(&managed_auth_path(&paths, &id), &auth).unwrap();
        write_state(
            &paths,
            &crate::models::ManagerStateFile {
                local_proxy_openai_auth_account_id: Some(id),
                ..crate::models::ManagerStateFile::default()
            },
        )
        .unwrap();

        write_official_local_proxy_config(&paths).unwrap();

        assert!(paths.current_auth.exists());
        assert!(fs::read_to_string(&paths.current_config)
            .unwrap()
            .contains("requires_openai_auth = true"));
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn restoring_official_config_preserves_current_non_provider_settings() {
        let paths = test_paths();
        let options = LocalProxyConfigOptions {
            name: "Proxy",
            model: Some("deepseek-chat"),
            include_model_catalog: true,
            requires_openai_auth: false,
            token_command: "csw",
        };
        let current = merge_local_proxy_config(STALE_OFFICIAL_CONFIG, &options)
            .unwrap()
            .replace("js_repl = true", "js_repl = false")
            .replace(
                "BROWSER_USE_AVAILABLE_BACKENDS = \"iab\"",
                "BROWSER_USE_AVAILABLE_BACKENDS = \"chrome,iab\"",
            )
            .replace("old-hash", "trusted-client-hash")
            .replace(
                "NODE_REPL_TRUSTED_CODE_PATHS = 'C:\\old'",
                "NODE_REPL_TRUSTED_CODE_PATHS = 'C:\\Users\\Test\\.codex;C:\\Users\\Test\\AppData\\Local\\OpenAI\\Codex'",
            )
            .replace("sandbox = \"workspace-write\"", "sandbox = \"elevated\"");
        write_text_atomic(&paths.config_backup, STALE_OFFICIAL_CONFIG).unwrap();
        write_text_atomic(&paths.current_config, &current).unwrap();

        restore_official_config(&paths).unwrap();

        let restored = fs::read_to_string(&paths.current_config).unwrap();
        assert!(restored.contains("model = \"gpt-5.5\""));
        assert!(restored.contains("js_repl = false"));
        assert!(restored.contains("BROWSER_USE_AVAILABLE_BACKENDS = \"chrome,iab\""));
        assert!(restored.contains("NODE_REPL_TRUSTED_CODE_PATHS = 'C:\\Users\\Test\\.codex"));
        assert!(restored.contains("sandbox = \"elevated\""));
        assert!(restored.contains("https://custom.example.com/v1"));
        assert!(!restored.contains(LOCAL_PROXY_BASE_URL));
        restored.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(!paths.config_backup.exists());
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn official_proxy_keeps_agent_identity_out_of_current_auth() {
        let paths = test_paths();
        let auth = test_agent_identity_auth();
        let (_, _, _, id) = crate::auth::account_fields(&auth).unwrap();
        write_json_atomic(&managed_auth_path(&paths, &id), &auth).unwrap();
        write_state(
            &paths,
            &crate::models::ManagerStateFile {
                active_account_id: Some(id),
                ..crate::models::ManagerStateFile::default()
            },
        )
        .unwrap();

        ensure_local_proxy_compatible_for_state(&paths).unwrap();
        apply_local_proxy_config_for_paths(&paths).unwrap();
        assert!(!paths.current_auth.exists());
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn proxy_start_allows_an_agent_identity_when_a_provider_is_selected() {
        let paths = test_paths();
        let auth = test_agent_identity_auth();
        let (_, _, _, id) = crate::auth::account_fields(&auth).unwrap();
        write_json_atomic(&managed_auth_path(&paths, &id), &auth).unwrap();
        write_provider(&paths, &provider()).unwrap();
        write_state(
            &paths,
            &crate::models::ManagerStateFile {
                active_account_id: Some(id),
                active_provider_id: Some("p".to_string()),
                ..crate::models::ManagerStateFile::default()
            },
        )
        .unwrap();

        ensure_local_proxy_compatible_for_state(&paths).unwrap();
        apply_local_proxy_config_for_paths(&paths).unwrap();
        assert!(!paths.current_auth.exists());
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn selected_proxy_openai_login_writes_auth_and_enables_config_flag() {
        let paths = test_paths();
        let auth = test_auth();
        let (_, _, _, id) = crate::auth::account_fields(&auth).unwrap();
        write_json_atomic(&managed_auth_path(&paths, &id), &auth).unwrap();
        write_provider(&paths, &provider()).unwrap();
        write_state(
            &paths,
            &crate::models::ManagerStateFile {
                active_provider_id: Some("p".to_string()),
                local_proxy_openai_auth_account_id: Some(id),
                ..crate::models::ManagerStateFile::default()
            },
        )
        .unwrap();

        ensure_local_proxy_compatible_for_state(&paths).unwrap();
        apply_local_proxy_config_for_paths(&paths).unwrap();

        assert_eq!(read_json(&paths.current_auth).unwrap(), auth);
        assert!(fs::read_to_string(&paths.current_config)
            .unwrap()
            .contains("requires_openai_auth = true"));

        let mut state = read_state(&paths);
        state.local_proxy_openai_auth_account_id = None;
        write_state(&paths, &state).unwrap();
        apply_local_proxy_config_for_paths(&paths).unwrap();

        assert!(!paths.current_auth.exists());
        assert!(fs::read_to_string(&paths.current_config)
            .unwrap()
            .contains("requires_openai_auth = false"));
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn agent_identity_cannot_be_used_as_proxy_openai_login() {
        let paths = test_paths();
        let auth = test_agent_identity_auth();
        let (_, _, _, id) = crate::auth::account_fields(&auth).unwrap();
        write_json_atomic(&managed_auth_path(&paths, &id), &auth).unwrap();

        let error = validate_local_proxy_openai_auth_account(&paths, Some(&id)).unwrap_err();

        assert!(error.contains("OAuth token"));
        fs::remove_dir_all(paths.codex_home.parent().unwrap()).unwrap();
    }

    #[test]
    fn local_proxy_config_points_codex_to_local_responses() {
        let options = LocalProxyConfigOptions {
            name: "Proxy",
            model: Some("deepseek-chat"),
            include_model_catalog: true,
            requires_openai_auth: false,
            token_command: r"C:\Program Files\Codex Switch\csw.exe",
        };
        let merged = merge_local_proxy_config("model = \"old\"", &options).unwrap();
        assert!(merged.contains("model_provider = \"codex-switch-local\""));
        assert!(merged.contains("model = \"deepseek-chat\""));
        assert!(merged.contains("model_catalog_json = \"codex-switch-model-catalog.json\""));
        assert!(merged.contains("base_url = \"http://127.0.0.1:15722/v1\""));
        assert!(merged.contains("requires_openai_auth = false"));
        let document = merged.parse::<toml_edit::DocumentMut>().unwrap();
        let provider = &document["model_providers"][codex_config::LOCAL_PROXY_PROVIDER_ID];
        assert_eq!(
            provider["http_headers"][LOCAL_PROXY_ACTOR_AUTHORIZATION_HEADER].as_str(),
            Some(LOCAL_PROXY_TOKEN)
        );
        assert!(merged.contains("--print-local-proxy-token"));
        assert_eq!(
            provider["auth"]["command"].as_str(),
            Some(r"C:\Program Files\Codex Switch\csw.exe")
        );
        assert!(!merged.contains("model = \"old\""));
    }

    #[test]
    fn switching_provider_replaces_the_previous_proxy_model() {
        let first = LocalProxyConfigOptions {
            name: "First",
            model: Some("first-model"),
            include_model_catalog: true,
            requires_openai_auth: false,
            token_command: "csw",
        };
        let second = LocalProxyConfigOptions {
            name: "Second",
            model: Some("second-model"),
            include_model_catalog: true,
            requires_openai_auth: false,
            token_command: "csw",
        };

        let first_config = merge_local_proxy_config("", &first).unwrap();
        let second_config = merge_local_proxy_config(&first_config, &second).unwrap();

        assert_eq!(
            codex_config::root_model(&second_config).as_deref(),
            Some("second-model")
        );
        assert!(!second_config.contains("first-model"));
    }

    #[test]
    fn switch_control_uses_fixed_model_name_for_codex() {
        let provider = provider();

        assert_eq!(
            codex_model_for_provider(&provider),
            CODEX_SWITCH_CONTROL_MODEL
        );
    }

    #[test]
    fn codex_control_uses_first_available_provider_model() {
        let mut provider = provider();
        provider.model_selection_controlled_by_codex = true;
        provider.model = "stale-model".to_string();
        provider.models = vec!["available-model".to_string(), "second-model".to_string()];

        assert_eq!(codex_model_for_provider(&provider), "available-model");
    }

    #[test]
    fn synced_openai_provider_allows_empty_api_key() {
        let mut provider = provider();
        provider.kind = ProviderKind::OpenAi;
        provider.api_key = "  ".to_string();

        let profile = normalize_synced_provider(provider).unwrap();

        assert!(profile.api_key.is_empty());
    }

    #[test]
    fn synced_antigravity_preset_allows_empty_api_key() {
        let mut provider = provider();
        provider.name = "Google Antigravity".to_string();
        provider.base_url = "http://localhost:51122/v1".to_string();
        provider.api_key.clear();

        let profile = normalize_synced_provider(provider).unwrap();

        assert!(profile.api_key.is_empty());
    }

    #[test]
    fn local_preset_save_policy_allows_empty_keys_only_for_exact_identities() {
        use crate::preset_provider::allows_missing_api_key_fields;

        assert!(allows_missing_api_key_fields(
            ProviderKind::Custom,
            "Ollama",
            "http://localhost:11434/v1",
            ProviderApiFormat::OpenaiResponses,
        ));
        assert!(allows_missing_api_key_fields(
            ProviderKind::Custom,
            "LM Studio",
            "http://127.0.0.1:1234/v1",
            ProviderApiFormat::OpenaiResponses,
        ));
        assert!(!allows_missing_api_key_fields(
            ProviderKind::Custom,
            "Ollama",
            "http://192.168.1.2:11434/v1",
            ProviderApiFormat::OpenaiResponses,
        ));
        assert!(!allows_missing_api_key_fields(
            ProviderKind::Custom,
            "OpenRouter",
            "https://openrouter.ai/api/v1",
            ProviderApiFormat::OpenaiResponses,
        ));
    }
