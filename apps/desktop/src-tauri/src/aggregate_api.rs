use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Runtime};
use uuid::Uuid;

use crate::{
    aggregate_scheduler, local_proxy,
    models::{ModelContextWindows, ModelReasoningEfforts, ProviderKind, ProviderProfile},
    providers,
    storage::{read_json, read_state, resolve_paths, write_json_atomic, Paths},
};

const AGGREGATE_API_FILE_NAME: &str = "aggregate-apis.json";
pub(crate) const ACTIVE_ID_PREFIX: &str = "aggregate:";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AggregateApiConfig {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) model: String,
    pub(crate) member_provider_ids: Vec<String>,
    pub(crate) enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AggregateApiInput {
    id: Option<String>,
    name: String,
    model: String,
    member_provider_ids: Vec<String>,
    enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AggregateApiSummary {
    id: String,
    name: String,
    model: String,
    member_provider_ids: Vec<String>,
    enabled: bool,
    active: bool,
    member_conversation_counts: HashMap<String, usize>,
}

pub(crate) struct PreparedAggregateApiSave {
    pub(crate) existing: Option<AggregateApiConfig>,
    pub(crate) config: AggregateApiConfig,
}

#[tauri::command]
pub(crate) async fn list_aggregate_apis<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
) -> Result<Vec<AggregateApiSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || list_summaries(&app))
        .await
        .map_err(|error| format!("Aggregate API list task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn save_aggregate_api<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    aggregate: AggregateApiInput,
) -> Result<AggregateApiSummary, String> {
    tauri::async_runtime::spawn_blocking(move || save_blocking(&app, aggregate))
        .await
        .map_err(|error| format!("Aggregate API save task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn delete_aggregate_api<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    id: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || delete_blocking(&app, &id))
        .await
        .map_err(|error| format!("Aggregate API delete task failed: {error}"))?
}

pub(crate) fn is_active_id(id: &str) -> bool {
    id.starts_with(ACTIVE_ID_PREFIX)
}

pub(crate) fn config_id_from_active_id(id: &str) -> Option<&str> {
    id.strip_prefix(ACTIVE_ID_PREFIX)
}

pub(crate) fn active_id(id: &str) -> String {
    format!("{ACTIVE_ID_PREFIX}{id}")
}

pub(crate) fn read_active_config(
    paths: &Paths,
    active_provider_id: &str,
) -> Result<AggregateApiConfig, String> {
    let id = config_id_from_active_id(active_provider_id)
        .ok_or_else(|| "Aggregate API id is invalid".to_string())?;
    read_config(paths, id)
}

pub(crate) fn read_config(paths: &Paths, id: &str) -> Result<AggregateApiConfig, String> {
    validate_id(id)?;
    read_configs(paths)?
        .into_iter()
        .find(|aggregate| aggregate.id == id)
        .ok_or_else(|| "Aggregate API does not exist".to_string())
}

pub(crate) fn member_profiles(
    paths: &Paths,
    aggregate: &AggregateApiConfig,
) -> Result<Vec<ProviderProfile>, String> {
    let profiles = aggregate
        .member_provider_ids
        .iter()
        .map(|id| providers::read_provider(paths, id))
        .collect::<Result<Vec<_>, _>>()?;
    validate_members(&aggregate.model, &profiles)?;
    Ok(profiles)
}

pub(crate) fn logical_profile(
    aggregate: &AggregateApiConfig,
    profiles: &[ProviderProfile],
) -> Result<ProviderProfile, String> {
    let mut profile = profiles
        .first()
        .cloned()
        .ok_or_else(|| "Aggregate API does not contain any available APIs".to_string())?;
    profile.id = active_id(&aggregate.id);
    profile.kind = ProviderKind::Custom;
    profile.name.clone_from(&aggregate.name);
    profile.group.clear();
    profile.model.clone_from(&aggregate.model);
    profile.models = vec![aggregate.model.clone()];
    profile.model_reasoning_efforts = common_reasoning_efforts(&aggregate.model, profiles);
    profile.model_context_windows = common_context_window(&aggregate.model, profiles);
    profile.image_input_models = common_image_input_models(aggregate, profiles);
    profile.image_input_models_configured = true;
    profile.model_selection_controlled_by_codex = true;
    profile.balance_platform = None;
    Ok(profile)
}

pub(crate) fn force_aggregate_model(provider: &ProviderProfile, model: &str) -> ProviderProfile {
    let mut selected = provider.clone();
    selected.model = model.to_string();
    selected.models = vec![model.to_string()];
    selected.model_selection_controlled_by_codex = false;
    selected
}

pub(crate) fn remove_provider_membership(paths: &Paths, provider_id: &str) -> Result<(), String> {
    let mut configs = read_configs(paths)?;
    let mut changed = false;
    for aggregate in &mut configs {
        let previous_len = aggregate.member_provider_ids.len();
        aggregate
            .member_provider_ids
            .retain(|member_id| member_id != provider_id);
        if aggregate.member_provider_ids.len() != previous_len {
            changed = true;
            aggregate.enabled &= aggregate.member_provider_ids.len() >= 2;
            aggregate_scheduler::reset(&aggregate.id);
        }
    }
    if changed {
        write_configs(paths, &configs)?;
    }
    Ok(())
}

pub(crate) fn active_aggregate_contains_provider(
    paths: &Paths,
    provider_id: &str,
) -> Result<bool, String> {
    let state = read_state(paths);
    let Some(active_id) = state
        .active_provider_id
        .as_deref()
        .filter(|id| is_active_id(id))
    else {
        return Ok(false);
    };
    Ok(read_active_config(paths, active_id)?
        .member_provider_ids
        .iter()
        .any(|id| id == provider_id))
}

fn list_summaries<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Vec<AggregateApiSummary>, String> {
    let paths = resolve_paths(app)?;
    let active_provider_id = read_state(&paths).active_provider_id;
    let active_session_ids = local_proxy::active_proxy_session_ids()?;
    let mut summaries = read_configs(&paths)?
        .into_iter()
        .map(|aggregate| {
            summary(
                aggregate,
                active_provider_id.as_deref(),
                &active_session_ids,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    summaries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(summaries)
}

fn save_blocking<R: Runtime>(
    app: &tauri::AppHandle<R>,
    input: AggregateApiInput,
) -> Result<AggregateApiSummary, String> {
    let _mutation_guard = crate::commands::account_switch_lock()
        .lock()
        .map_err(|_| "Account and Provider switch lock is poisoned".to_string())?;
    let paths = resolve_paths(app)?;
    let prepared = prepare_aggregate_save(&paths, input)?;
    let expected_active_id = active_id(&prepared.config.id);
    if read_state(&paths).active_provider_id.as_deref() == Some(&expected_active_id) {
        return Err("Stop the active Aggregate API before editing it".to_string());
    }
    upsert_config(&paths, &prepared.config)?;
    aggregate_scheduler::reset(&prepared.config.id);

    emit_changed(app)?;
    summary(
        prepared.config,
        read_state(&paths).active_provider_id.as_deref(),
        &local_proxy::active_proxy_session_ids()?,
    )
}

pub(crate) fn prepare_aggregate_save(
    paths: &Paths,
    input: AggregateApiInput,
) -> Result<PreparedAggregateApiSave, String> {
    let configs = read_configs(paths)?;
    let id = input
        .id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    validate_id(&id)?;
    let existing = configs.iter().find(|aggregate| aggregate.id == id).cloned();
    let config = normalize_input(id, input, paths)?;
    Ok(PreparedAggregateApiSave { existing, config })
}

fn upsert_config(paths: &Paths, config: &AggregateApiConfig) -> Result<(), String> {
    let mut configs = read_configs(paths)?;
    if let Some(existing) = configs
        .iter_mut()
        .find(|aggregate| aggregate.id == config.id)
    {
        *existing = config.clone();
    } else {
        configs.push(config.clone());
    }
    write_configs(paths, &configs)
}

pub(crate) fn write_active_config_for_transaction(
    paths: &Paths,
    original: &AggregateApiConfig,
    updated: &AggregateApiConfig,
) -> Result<(), String> {
    if original.id != updated.id {
        return Err("Safe Aggregate edit target changed".to_string());
    }
    let mut configs = read_configs(paths)?;
    let current = configs
        .iter_mut()
        .find(|aggregate| aggregate.id == original.id)
        .ok_or_else(|| "Aggregate API does not exist".to_string())?;
    if current != original {
        return Err("Aggregate API changed before the safe edit was applied".to_string());
    }
    *current = updated.clone();
    write_configs(paths, &configs)
}

fn delete_blocking<R: Runtime>(app: &tauri::AppHandle<R>, id: &str) -> Result<(), String> {
    let _mutation_guard = crate::commands::account_switch_lock()
        .lock()
        .map_err(|_| "Account and Provider switch lock is poisoned".to_string())?;
    validate_id(id)?;
    let paths = resolve_paths(app)?;
    let mut configs = read_configs(&paths)?;
    let previous_len = configs.len();
    configs.retain(|aggregate| aggregate.id != id);
    if configs.len() == previous_len {
        return Err("Aggregate API does not exist".to_string());
    }
    let expected_active_id = active_id(id);
    let was_active = read_state(&paths).active_provider_id.as_deref() == Some(&expected_active_id);
    if was_active {
        return Err("Stop the active Aggregate API before deleting it".to_string());
    }
    write_configs(&paths, &configs)?;
    aggregate_scheduler::reset(id);
    emit_changed(app)
}

fn normalize_input(
    id: String,
    input: AggregateApiInput,
    paths: &Paths,
) -> Result<AggregateApiConfig, String> {
    let name = require_value("Aggregate API name", &input.name)?;
    let model = require_value("Model", &input.model)?;
    let member_provider_ids = unique_members(input.member_provider_ids)?;
    let profiles = member_provider_ids
        .iter()
        .map(|provider_id| providers::read_provider(paths, provider_id))
        .collect::<Result<Vec<_>, _>>()?;
    validate_members(&model, &profiles)?;
    Ok(AggregateApiConfig {
        id,
        name,
        model,
        member_provider_ids,
        enabled: input.enabled,
    })
}

fn unique_members(member_provider_ids: Vec<String>) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let members = member_provider_ids
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty() && seen.insert(id.clone()))
        .collect::<Vec<_>>();
    if members.len() < 2 {
        return Err("Select at least two APIs for the aggregate".to_string());
    }
    Ok(members)
}

fn validate_members(model: &str, profiles: &[ProviderProfile]) -> Result<(), String> {
    if profiles.len() < 2 {
        return Err("Aggregate API does not contain enough available APIs".to_string());
    }
    if profiles
        .iter()
        .any(|provider| !provider_supports_model(provider, model))
    {
        return Err("Every API in an aggregate must support the selected model".to_string());
    }
    Ok(())
}

pub(crate) fn validate_member_profiles(
    model: &str,
    profiles: &[ProviderProfile],
) -> Result<(), String> {
    validate_members(model, profiles)
}

fn provider_supports_model(provider: &ProviderProfile, model: &str) -> bool {
    provider.model == model || provider.models.iter().any(|candidate| candidate == model)
}

fn common_reasoning_efforts(model: &str, profiles: &[ProviderProfile]) -> ModelReasoningEfforts {
    let Some(first) = profiles
        .first()
        .and_then(|provider| provider.model_reasoning_efforts.get(model))
        .cloned()
    else {
        return ModelReasoningEfforts::new();
    };
    let common = first
        .into_iter()
        .filter(|effort| {
            profiles.iter().all(|provider| {
                provider
                    .model_reasoning_efforts
                    .get(model)
                    .is_some_and(|efforts| efforts.contains(effort))
            })
        })
        .collect::<Vec<_>>();
    if common.is_empty() {
        ModelReasoningEfforts::new()
    } else {
        [(model.to_string(), common)].into()
    }
}

fn common_context_window(model: &str, profiles: &[ProviderProfile]) -> ModelContextWindows {
    profiles
        .iter()
        .filter_map(|provider| provider.model_context_windows.get(model).copied())
        .min()
        .map(|window| [(model.to_string(), window)].into())
        .unwrap_or_default()
}

fn common_image_input_models(
    aggregate: &AggregateApiConfig,
    profiles: &[ProviderProfile],
) -> Vec<String> {
    let supported_by_all = profiles.iter().all(|provider| {
        provider
            .image_input_models
            .iter()
            .any(|model| model == &aggregate.model)
    });
    if supported_by_all {
        vec![aggregate.model.clone()]
    } else {
        Vec::new()
    }
}

fn summary(
    config: AggregateApiConfig,
    active_provider_id: Option<&str>,
    active_session_ids: &HashSet<String>,
) -> Result<AggregateApiSummary, String> {
    let expected_active_id = active_id(&config.id);
    let member_conversation_counts = aggregate_scheduler::conversation_counts(
        &config.id,
        &config.member_provider_ids,
        active_session_ids,
    )?;
    Ok(AggregateApiSummary {
        active: active_provider_id == Some(&expected_active_id),
        id: config.id,
        name: config.name,
        model: config.model,
        member_provider_ids: config.member_provider_ids,
        enabled: config.enabled,
        member_conversation_counts,
    })
}

fn read_configs(paths: &Paths) -> Result<Vec<AggregateApiConfig>, String> {
    let path = store_path(paths)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    serde_json::from_value(read_json(&path)?)
        .map_err(|error| format!("Aggregate API configuration is invalid: {error}"))
}

fn write_configs(paths: &Paths, configs: &[AggregateApiConfig]) -> Result<(), String> {
    let value = serde_json::to_value(configs).map_err(|error| error.to_string())?;
    write_json_atomic(&store_path(paths)?, &value)
}

pub(crate) fn store_path(paths: &Paths) -> Result<PathBuf, String> {
    paths
        .providers
        .parent()
        .map(|parent| parent.join(AGGREGATE_API_FILE_NAME))
        .ok_or_else(|| "Provider store does not have a parent directory".to_string())
}

fn require_value(label: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(format!("{label} is required"))
    } else {
        Ok(value.to_string())
    }
}

fn validate_id(id: &str) -> Result<(), String> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    if valid {
        Ok(())
    } else {
        Err("Aggregate API id is invalid".to_string())
    }
}

fn emit_changed<R: Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    app.emit("aggregate-apis-changed", ())
        .map_err(|error| error.to_string())?;
    app.emit("providers-changed", ())
        .map_err(|error| error.to_string())
}

pub(crate) fn finalize_active_config_edit<R: Runtime>(
    app: &tauri::AppHandle<R>,
    config: AggregateApiConfig,
) -> (AggregateApiSummary, Vec<String>) {
    let mut warnings = Vec::new();
    aggregate_scheduler::reset(&config.id);
    let active_provider_id = resolve_paths(app)
        .map(|paths| read_state(&paths).active_provider_id)
        .unwrap_or_else(|error| {
            warnings.push(format!("Aggregate state summary failed: {error}"));
            None
        });
    let active_session_ids = local_proxy::active_proxy_session_ids().unwrap_or_else(|error| {
        warnings.push(format!("Aggregate session summary failed: {error}"));
        HashSet::new()
    });
    let result = summary(
        config.clone(),
        active_provider_id.as_deref(),
        &active_session_ids,
    )
    .unwrap_or_else(|error| {
        warnings.push(format!("Aggregate conversation summary failed: {error}"));
        AggregateApiSummary {
            active: active_provider_id
                .as_deref()
                .and_then(config_id_from_active_id)
                == Some(config.id.as_str()),
            id: config.id.clone(),
            name: config.name.clone(),
            model: config.model.clone(),
            member_provider_ids: config.member_provider_ids.clone(),
            enabled: config.enabled,
            member_conversation_counts: HashMap::new(),
        }
    });
    if let Err(error) = emit_changed(app) {
        warnings.push(format!("Aggregate events failed: {error}"));
    }
    crate::system_tray::refresh_menu(app);
    (result, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn paths(root: &std::path::Path) -> Paths {
        let codex_home = root.join("codex-home");
        let app_data = root.join("app-data");
        Paths {
            current_auth: codex_home.join("auth.json"),
            current_config: codex_home.join("config.toml"),
            codex_home,
            accounts: app_data.join("accounts"),
            providers: app_data.join("providers"),
            config_backup: app_data.join("config-before-provider.toml"),
            state_file: app_data.join("state.json"),
        }
    }

    #[test]
    fn active_aggregate_members_cannot_be_mutated_through_provider_commands() {
        let root = tempfile::tempdir().expect("tempdir");
        let paths = paths(root.path());
        fs::create_dir_all(paths.providers.parent().expect("app data")).expect("app data");
        write_configs(
            &paths,
            &[AggregateApiConfig {
                id: "aggregate-a".to_string(),
                name: "Aggregate A".to_string(),
                model: "gpt-4.1".to_string(),
                member_provider_ids: vec!["p1".to_string(), "p2".to_string()],
                enabled: true,
            }],
        )
        .expect("aggregate config");
        crate::storage::write_state(
            &paths,
            &crate::models::ManagerStateFile {
                active_provider_id: Some("aggregate:aggregate-a".to_string()),
                ..crate::models::ManagerStateFile::default()
            },
        )
        .expect("state");

        assert!(active_aggregate_contains_provider(&paths, "p1").unwrap());
        assert!(!active_aggregate_contains_provider(&paths, "p3").unwrap());
    }

    #[test]
    fn active_config_transaction_requires_the_exact_original_config() {
        let root = tempfile::tempdir().expect("tempdir");
        let paths = paths(root.path());
        fs::create_dir_all(paths.providers.parent().expect("app data")).expect("app data");
        let original = AggregateApiConfig {
            id: "aggregate-a".to_string(),
            name: "Aggregate A".to_string(),
            model: "gpt-4.1".to_string(),
            member_provider_ids: vec!["p1".to_string(), "p2".to_string()],
            enabled: true,
        };
        write_configs(&paths, std::slice::from_ref(&original)).expect("aggregate config");
        let mut updated = original.clone();
        updated.name = "Aggregate Updated".to_string();

        write_active_config_for_transaction(&paths, &original, &updated).expect("safe update");
        assert_eq!(read_config(&paths, &original.id).unwrap(), updated);
        assert!(write_active_config_for_transaction(&paths, &original, &updated).is_err());
    }
}
