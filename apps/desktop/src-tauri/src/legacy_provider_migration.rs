//! Maps Viewer API-account material into the imported product's Provider model.
//! Credentials stay in the native host; neither parsing nor preview activates
//! a Provider, contacts its endpoint, or writes Codex configuration.

use capacity_migration::{LegacyAccountAuthMode, LegacyAccountImportRecord};
use serde_json::Value;
use sha2::{Digest, Sha256};
use toml_edit::DocumentMut;
use url::Url;

use crate::models::{ProviderApiFormat, ProviderKind, ProviderProfile, ReasoningEffort};

const MAX_CONNECTION_VALUE: usize = 4096;
const MAX_API_KEY_BYTES: usize = 64 * 1024;

fn required_text(
    value: Option<&str>,
    maximum: usize,
    reason: &'static str,
) -> Result<String, &'static str> {
    let value = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(reason)?;
    if value.len() > maximum || value.chars().any(char::is_control) {
        return Err(reason);
    }
    Ok(value.to_owned())
}

fn imported_provider_id(source_id: &str) -> String {
    // The identifier derives from the legacy record ID, never from its API key.
    let digest = Sha256::digest(source_id.as_bytes());
    format!("viewer-{:x}", digest)[..31].to_owned()
}

pub(crate) fn map_legacy_api_account(
    record: &LegacyAccountImportRecord,
) -> Result<Option<ProviderProfile>, &'static str> {
    match record.auth_mode {
        LegacyAccountAuthMode::Chatgpt => return Ok(None),
        LegacyAccountAuthMode::Unknown => return Err("legacy_provider_auth_unsupported"),
        LegacyAccountAuthMode::ApiKey => {}
    }
    let auth: Value =
        serde_json::from_slice(&record.auth_json).map_err(|_| "legacy_provider_auth_invalid")?;
    if auth.get("tokens").is_some_and(|value| !value.is_null())
        || auth.get("agent_identity").is_some()
    {
        return Err("legacy_provider_mixed_auth");
    }
    let api_key = required_text(
        auth.get("OPENAI_API_KEY").and_then(Value::as_str),
        MAX_API_KEY_BYTES,
        "legacy_provider_key_missing",
    )?;
    let config = record
        .config_toml
        .as_ref()
        .ok_or("legacy_provider_config_missing")?;
    let config = std::str::from_utf8(config)
        .map_err(|_| "legacy_provider_config_invalid")?
        .parse::<DocumentMut>()
        .map_err(|_| "legacy_provider_config_invalid")?;
    let model = required_text(
        config.get("model").and_then(|item| item.as_str()),
        256,
        "legacy_provider_model_missing",
    )?;
    let selected = match config.get("model_provider") {
        Some(item) => item
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("legacy_provider_config_invalid")?,
        None => "openai",
    };
    if config
        .get("model_providers")
        .is_some_and(|item| !item.is_table_like())
    {
        return Err("legacy_provider_config_invalid");
    }
    let provider = config
        .get("model_providers")
        .and_then(|item| item.get(selected));
    if selected != "openai" && provider.is_none() {
        return Err("legacy_provider_endpoint_missing");
    }
    if provider.is_some_and(|provider| !provider.is_table_like()) {
        return Err("legacy_provider_config_invalid");
    }
    // Extra authentication/routing cannot be represented by the current saved
    // Provider contract. Require review rather than silently changing requests.
    for field in [
        "env_key",
        "http_headers",
        "env_http_headers",
        "query_params",
        "experimental_bearer_token",
    ] {
        if provider.and_then(|item| item.get(field)).is_some() {
            return Err("legacy_provider_custom_auth_review");
        }
    }
    if let Some(fields) = provider.and_then(|item| item.as_table_like()) {
        for (name, _) in fields.iter() {
            if !matches!(
                name,
                "name" | "base_url" | "wire_api" | "requires_openai_auth" | "supports_websockets"
            ) {
                return Err("legacy_provider_transport_review");
            }
        }
    }
    if provider
        .and_then(|item| item.get("supports_websockets"))
        .is_some_and(|item| item.as_bool() != Some(false))
    {
        return Err("legacy_provider_transport_review");
    }
    if provider
        .and_then(|item| item.get("requires_openai_auth"))
        .is_some_and(|item| item.as_bool().is_none())
    {
        return Err("legacy_provider_config_invalid");
    }
    if provider
        .and_then(|item| item.get("requires_openai_auth"))
        .is_some_and(|item| item.as_bool() == Some(false))
    {
        return Err("legacy_provider_custom_auth_review");
    }
    let base_url = match provider.and_then(|item| item.get("base_url")) {
        Some(item) => Some(item.as_str().ok_or("legacy_provider_endpoint_invalid")?),
        None if selected == "openai" => Some("https://api.openai.com/v1"),
        None => None,
    };
    let endpoint = required_text(
        base_url,
        MAX_CONNECTION_VALUE,
        "legacy_provider_endpoint_missing",
    )?;
    let url = Url::parse(&endpoint).map_err(|_| "legacy_provider_endpoint_invalid")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("legacy_provider_endpoint_invalid");
    }
    let wire_api = match provider.and_then(|item| item.get("wire_api")) {
        Some(item) => item.as_str().ok_or("legacy_provider_transport_review")?,
        None => "responses",
    };
    let api_format = match wire_api {
        "responses" => ProviderApiFormat::OpenaiResponses,
        "chat" => ProviderApiFormat::OpenaiChat,
        _ => return Err("legacy_provider_transport_review"),
    };
    let mut model_reasoning_efforts = crate::models::ModelReasoningEfforts::new();
    if let Some(effort) = config.get("model_reasoning_effort") {
        let effort: ReasoningEffort = serde_json::from_value(Value::String(
            effort
                .as_str()
                .ok_or("legacy_provider_reasoning_review")?
                .to_owned(),
        ))
        .map_err(|_| "legacy_provider_reasoning_review")?;
        model_reasoning_efforts.insert(model.clone(), vec![effort]);
    }
    let context_window = config
        .get("model_context_window")
        .map(|value| {
            value
                .as_integer()
                .filter(|value| *value > 0 && *value <= u32::MAX as i64)
                .map(|value| value as u64)
                .ok_or("legacy_provider_context_invalid")
        })
        .transpose()?;
    let mut model_context_windows = crate::models::ModelContextWindows::new();
    if let Some(context) = context_window {
        model_context_windows.insert(model.clone(), context);
    }
    let profile = ProviderProfile {
        id: imported_provider_id(&record.source_id),
        kind: ProviderKind::Custom,
        name: required_text(
            Some(&record.display_name),
            256,
            "legacy_provider_name_invalid",
        )?,
        group: String::new(),
        base_url: endpoint,
        api_key,
        model: model.clone(),
        models: vec![model],
        model_reasoning_efforts,
        model_context_windows,
        model_api_formats: Default::default(),
        image_input_models: Vec::new(),
        image_input_models_configured: false,
        context_window,
        model_selection_controlled_by_codex: false,
        api_format,
        balance_platform: None,
        balance_query_url: None,
        balance_query_token: None,
        wallet_query_url: None,
        wallet_query_token: None,
        wallet_username: None,
        wallet_password: None,
    };
    crate::providers::normalize_imported_provider(profile)
        .map(Some)
        .map_err(|_| "legacy_provider_profile_invalid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> LegacyAccountImportRecord {
        LegacyAccountImportRecord {
            source_id: "fixture-source-1".to_owned(),
            display_name: "Fixture API".to_owned(),
            auth_mode: LegacyAccountAuthMode::ApiKey,
            created_at: "2026-09-07T00:00:00Z".to_owned(),
            last_used_at: None,
            auth_json: br#"{"OPENAI_API_KEY":"fixture-api-key","auth_mode":"apikey"}"#.to_vec(),
            config_toml: Some(
                br#"model_provider = "custom"
model = "fixture-model"
model_reasoning_effort = "high"
model_context_window = 128000
[model_providers.custom]
name = "custom"
wire_api = "responses"
requires_openai_auth = true
base_url = "https://api.example.test/v1"
"#
                .to_vec(),
            ),
        }
    }

    #[test]
    fn maps_viewer_synthesized_config_into_existing_provider_shape_without_mutation() {
        let source = record();
        let before_auth = source.auth_json.clone();
        let before_config = source.config_toml.clone();
        let profile = map_legacy_api_account(&source).unwrap().unwrap();
        assert_eq!(profile.kind, ProviderKind::Custom);
        assert_eq!(profile.base_url, "https://api.example.test/v1");
        assert_eq!(profile.model, "fixture-model");
        assert_eq!(profile.context_window, Some(128000));
        assert_eq!(
            profile.model_reasoning_efforts["fixture-model"],
            vec![ReasoningEffort::High]
        );
        assert_eq!(profile.api_format, ProviderApiFormat::OpenaiResponses);
        assert_eq!(profile.api_key, "fixture-api-key");
        assert_eq!(profile.id, imported_provider_id(&source.source_id));
        assert_eq!(source.auth_json, before_auth);
        assert_eq!(source.config_toml, before_config);
    }

    #[test]
    fn api_keys_do_not_become_chatgpt_providers_or_change_identifiers() {
        let mut source = record();
        source.config_toml = Some(b"model = 'fixture-model'\n".to_vec());
        let first = map_legacy_api_account(&source).unwrap().unwrap();
        assert_eq!(first.kind, ProviderKind::Custom);
        assert_eq!(first.base_url, "https://api.openai.com/v1");
        source.auth_json = br#"{"OPENAI_API_KEY":"different-fixture-key"}"#.to_vec();
        assert_eq!(
            map_legacy_api_account(&source).unwrap().unwrap().id,
            first.id
        );
        source.auth_mode = LegacyAccountAuthMode::Chatgpt;
        assert!(map_legacy_api_account(&source).unwrap().is_none());
    }

    #[test]
    fn refuses_to_discard_custom_headers_or_external_auth_and_transport_rules() {
        for extra in [
            "env_key = 'SECRET'",
            "http_headers = { X = 'fixture' }",
            "env_http_headers = { X = 'KEY' }",
            "query_params = { a = 'b' }",
            "supports_websockets = true",
            "experimental_bearer_token = 'fixture'",
        ] {
            let mut source = record();
            source
                .config_toml
                .as_mut()
                .unwrap()
                .extend_from_slice(format!("{extra}\n").as_bytes());
            assert!(map_legacy_api_account(&source).is_err(), "accepted {extra}");
        }
    }

    #[test]
    fn rejects_unsafe_endpoint_and_does_not_invent_a_missing_model_or_key() {
        for endpoint in [
            "file:///tmp",
            "https://name:secret@example.test/v1",
            "https://example.test/v1?key=fixture",
            "https://example.test/v1#token",
        ] {
            let mut source = record();
            source.config_toml = Some(
                format!("model='fixture'\n[model_providers.openai]\nbase_url='{endpoint}'\n")
                    .into_bytes(),
            );
            assert_eq!(
                map_legacy_api_account(&source).unwrap_err(),
                "legacy_provider_endpoint_invalid"
            );
        }
        let mut source = record();
        source.config_toml = Some(b"model_provider = 'custom'\n".to_vec());
        assert_eq!(
            map_legacy_api_account(&source).unwrap_err(),
            "legacy_provider_model_missing"
        );
        source.auth_json = br#"{"OPENAI_API_KEY":""}"#.to_vec();
        assert_eq!(
            map_legacy_api_account(&source).unwrap_err(),
            "legacy_provider_key_missing"
        );
    }

    #[test]
    fn malformed_fields_never_silently_select_defaults_or_drop_transport_options() {
        for config in [
            "model='fixture'\nmodel_provider=7\n",
            "model='fixture'\nmodel_providers='invalid'\n",
            "model='fixture'\n[model_providers.openai]\nbase_url=7\n",
            "model='fixture'\n[model_providers.openai]\nwire_api=7\n",
            "model='fixture'\n[model_providers.openai]\nsupports_websockets='false'\n",
            "model='fixture'\n[model_providers.openai]\nrequires_openai_auth=false\n",
            "model='fixture'\n[model_providers.openai]\nrequest_max_retries=8\n",
        ] {
            let mut source = record();
            source.config_toml = Some(config.as_bytes().to_vec());
            assert!(
                map_legacy_api_account(&source).is_err(),
                "accepted invalid settings"
            );
        }
    }
}
