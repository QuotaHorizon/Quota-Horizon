use std::{io::Read, time::Duration};

use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::Value;
use tauri::Runtime;
use url::Url;

use crate::{
    models::ProviderProfile,
    provider_connectivity::{ProviderConnectivityIssue, ProviderConnectivityIssueCode},
    providers::read_provider,
    storage::resolve_paths,
};

const MAX_MODEL_RESPONSE_BYTES: u64 = 1024 * 1024;
const MODEL_QUERY_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum RelayModelDiscoveryStatus {
    Ready,
    ManualFallback,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RelayModelDiscoveryResult {
    status: RelayModelDiscoveryStatus,
    models: Vec<String>,
    issue: Option<ProviderConnectivityIssue>,
}

impl RelayModelDiscoveryResult {
    fn ready(models: Vec<String>) -> Self {
        Self {
            status: RelayModelDiscoveryStatus::Ready,
            models,
            issue: None,
        }
    }

    fn manual_fallback(issue: ProviderConnectivityIssue) -> Self {
        Self {
            status: RelayModelDiscoveryStatus::ManualFallback,
            models: Vec::new(),
            issue: Some(issue),
        }
    }
}

#[tauri::command]
pub(crate) async fn fetch_relay_models<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    base_url: String,
    api_key: Option<String>,
    provider_id: Option<String>,
) -> Result<RelayModelDiscoveryResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let saved_provider = match provider_id.as_deref() {
            Some(id) => match resolve_paths(&app).and_then(|paths| read_provider(&paths, id)) {
                Ok(provider) => Some(provider),
                Err(_) => {
                    return RelayModelDiscoveryResult::manual_fallback(
                        ProviderConnectivityIssue::new(
                            ProviderConnectivityIssueCode::ProviderUnavailable,
                        ),
                    )
                }
            },
            None => None,
        };
        match fetch_relay_models_with_saved_provider_blocking(
            &base_url,
            api_key.as_deref(),
            saved_provider.as_ref(),
        ) {
            Ok(models) => RelayModelDiscoveryResult::ready(models),
            Err(issue) => RelayModelDiscoveryResult::manual_fallback(issue),
        }
    })
    .await
    .map_err(|error| format!("Relay model query task failed: {error}"))
}

pub(crate) fn fetch_relay_models_blocking(
    base_url: &str,
    api_key: &str,
) -> Result<Vec<String>, String> {
    let query_url = relay_models_url(base_url).map_err(|issue| issue.to_string())?;
    let token =
        required_api_key(Some(api_key), None, &query_url).map_err(|issue| issue.to_string())?;
    fetch_relay_models_with_token(&query_url, &token).map_err(|issue| issue.to_string())
}

fn fetch_relay_models_with_saved_provider_blocking(
    base_url: &str,
    api_key: Option<&str>,
    saved_provider: Option<&ProviderProfile>,
) -> Result<Vec<String>, ProviderConnectivityIssue> {
    let query_url = relay_models_url(base_url)?;
    let token = required_api_key(api_key, saved_provider, &query_url)?;
    fetch_relay_models_with_token(&query_url, &token)
}

fn required_api_key(
    supplied_key: Option<&str>,
    saved_provider: Option<&ProviderProfile>,
    query_url: &Url,
) -> Result<String, ProviderConnectivityIssue> {
    let supplied_key = supplied_key.unwrap_or_default().trim();
    if !supplied_key.is_empty() {
        return Ok(supplied_key.to_string());
    }
    let Some(provider) = saved_provider else {
        return Err(ProviderConnectivityIssue::new(
            ProviderConnectivityIssueCode::CredentialRequired,
        ));
    };
    let saved_query_url = relay_models_url(&provider.base_url)?;
    if saved_query_url != *query_url {
        return Err(ProviderConnectivityIssue::new(
            ProviderConnectivityIssueCode::CredentialEndpointMismatch,
        ));
    }
    let token = provider.api_key.trim();
    if token.is_empty() {
        return Err(ProviderConnectivityIssue::new(
            ProviderConnectivityIssueCode::CredentialUnavailable,
        ));
    }
    Ok(token.to_string())
}

fn fetch_relay_models_with_token(
    query_url: &Url,
    token: &str,
) -> Result<Vec<String>, ProviderConnectivityIssue> {
    let client = crate::system_proxy::apply(Client::builder())
        .timeout(MODEL_QUERY_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("QuotaHorizon")
        .build()
        .map_err(|_| {
            ProviderConnectivityIssue::new(ProviderConnectivityIssueCode::RequestFailed)
        })?;
    let response = client
        .get(query_url.clone())
        .bearer_auth(token)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .map_err(|error| ProviderConnectivityIssue::from_request(&error))?;
    let payload = read_model_response(response)?;
    parse_models(&payload)
}

fn relay_models_url(base_url: &str) -> Result<Url, ProviderConnectivityIssue> {
    let normalized = base_url.trim().trim_end_matches('/');
    crate::providers::ensure_not_local_proxy_base_url(normalized).map_err(|_| {
        ProviderConnectivityIssue::new(ProviderConnectivityIssueCode::LocalProxyUrl)
    })?;
    let mut url = Url::parse(normalized).map_err(|_| {
        ProviderConnectivityIssue::new(ProviderConnectivityIssueCode::InvalidBaseUrl)
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ProviderConnectivityIssue::new(
            ProviderConnectivityIssueCode::InvalidBaseUrl,
        ));
    }
    let path = url.path().trim_end_matches('/');
    url.set_path(&format!("{path}/models"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn read_model_response(
    response: reqwest::blocking::Response,
) -> Result<Value, ProviderConnectivityIssue> {
    let status = response.status();
    if !status.is_success() {
        return Err(ProviderConnectivityIssue::from_http_status(status.as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MODEL_RESPONSE_BYTES)
    {
        return Err(ProviderConnectivityIssue::new(
            ProviderConnectivityIssueCode::ResponseTooLarge,
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_MODEL_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            ProviderConnectivityIssue::new(ProviderConnectivityIssueCode::RequestFailed)
        })?;
    if bytes.len() as u64 > MAX_MODEL_RESPONSE_BYTES {
        return Err(ProviderConnectivityIssue::new(
            ProviderConnectivityIssueCode::ResponseTooLarge,
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ProviderConnectivityIssue::new(ProviderConnectivityIssueCode::InvalidResponse))
}

fn parse_models(payload: &Value) -> Result<Vec<String>, ProviderConnectivityIssue> {
    let data = payload
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ProviderConnectivityIssue::new(ProviderConnectivityIssueCode::InvalidResponse)
        })?;
    let mut models = Vec::new();
    for item in data {
        let Some(model) = item
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
        else {
            continue;
        };
        if !models.iter().any(|existing| existing == model) {
            models.push(model.to_string());
        }
    }
    if models.is_empty() {
        Err(ProviderConnectivityIssue::new(
            ProviderConnectivityIssueCode::EmptyModelCatalog,
        ))
    } else {
        Ok(models)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ProviderApiFormat, ProviderKind};
    use serde_json::json;
    use tiny_http::{Header, Response, Server};

    fn saved_provider(base_url: &str, api_key: &str) -> ProviderProfile {
        ProviderProfile {
            id: "provider-a".to_string(),
            kind: ProviderKind::Custom,
            name: "Relay".to_string(),
            group: String::new(),
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            model: "gpt-test".to_string(),
            models: vec!["gpt-test".to_string()],
            model_reasoning_efforts: Default::default(),
            model_context_windows: Default::default(),
            model_api_formats: Default::default(),
            image_input_models: Vec::new(),
            image_input_models_configured: false,
            context_window: None,
            model_selection_controlled_by_codex: true,
            api_format: ProviderApiFormat::OpenaiResponses,
            balance_platform: None,
            balance_query_url: None,
            balance_query_token: None,
            wallet_query_url: None,
            wallet_query_token: None,
            wallet_username: None,
            wallet_password: None,
        }
    }

    #[test]
    fn builds_models_url_from_openai_compatible_base_url() {
        let url = relay_models_url("https://relay.example.com/api/v1/?ignored=yes").unwrap();
        assert_eq!(url.as_str(), "https://relay.example.com/api/v1/models");
    }

    #[test]
    fn parses_and_deduplicates_openai_model_lists() {
        let models = parse_models(&json!({
            "object": "list",
            "data": [
                { "id": "gpt-5.6-sol", "object": "model" },
                { "id": "claude-sonnet-4-5", "type": "model" },
                { "id": "gpt-5.6-sol", "object": "model" }
            ]
        }))
        .unwrap();

        assert_eq!(models, vec!["gpt-5.6-sol", "claude-sonnet-4-5"]);
    }

    #[test]
    fn fetches_models_with_bearer_authentication() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", server.server_addr());
        let worker = std::thread::spawn(move || {
            let request = server.recv().unwrap();
            assert_eq!(request.url(), "/v1/models");
            assert!(request.headers().iter().any(|header| {
                header.field.equiv("Authorization")
                    && header.value.as_str() == "Bearer sk-relay-test"
            }));
            request
                .respond(
                    Response::from_string(r#"{"object":"list","data":[{"id":"gpt-test"}]}"#)
                        .with_header(
                            Header::from_bytes("Content-Type", "application/json").unwrap(),
                        ),
                )
                .unwrap();
        });

        let models = fetch_relay_models_blocking(&base_url, "sk-relay-test").unwrap();

        assert_eq!(models, vec!["gpt-test"]);
        worker.join().unwrap();
    }

    #[test]
    fn refuses_redirects_before_forwarding_a_relay_credential() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", server.server_addr());
        let worker = std::thread::spawn(move || {
            let request = server.recv().unwrap();
            request
                .respond(Response::empty(302).with_header(
                    Header::from_bytes("Location", "https://other.example.com/v1/models").unwrap(),
                ))
                .unwrap();
        });

        assert_eq!(
            fetch_relay_models_blocking(&base_url, "sk-relay-test").unwrap_err(),
            "Provider endpoint redirected the credential-bearing request (HTTP 302)"
        );
        worker.join().unwrap();
    }

    #[test]
    fn reuses_a_saved_key_only_for_the_same_models_endpoint() {
        let provider = saved_provider("https://relay.example.com/api/v1", "sk-saved");
        let same = relay_models_url("https://relay.example.com/api/v1/").unwrap();
        assert_eq!(
            required_api_key(None, Some(&provider), &same).unwrap(),
            "sk-saved"
        );

        let different = relay_models_url("https://other.example.com/v1").unwrap();
        assert_eq!(
            required_api_key(None, Some(&provider), &different)
                .unwrap_err()
                .to_string(),
            "The saved Provider credential cannot be used with a different endpoint"
        );
    }

    #[test]
    fn a_new_key_takes_precedence_over_the_saved_key() {
        let provider = saved_provider("https://relay.example.com/v1", "sk-saved");
        let different = relay_models_url("https://other.example.com/v1").unwrap();

        assert_eq!(
            required_api_key(Some(" sk-new "), Some(&provider), &different).unwrap(),
            "sk-new"
        );
    }

    #[test]
    fn manual_fallback_wire_contract_has_no_raw_server_detail() {
        let result = RelayModelDiscoveryResult::manual_fallback(
            ProviderConnectivityIssue::from_http_status(401),
        );
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({
                "status": "manual_fallback",
                "models": [],
                "issue": {
                    "code": "unauthorized",
                    "retryable": false,
                    "httpStatus": 401
                }
            })
        );
    }
}
