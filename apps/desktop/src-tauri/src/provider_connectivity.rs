use std::{fmt, time::Duration};

use reqwest::blocking::{Client, ClientBuilder};
use serde::Serialize;
use tauri::Runtime;
use url::Url;

use crate::{
    claude_code_provider::is_claude_code_identity,
    grok_provider::is_grok_identity,
    models::ProviderProfile,
    providers::{ensure_not_local_proxy_base_url, read_provider},
    storage::resolve_paths,
};

const CONNECTIVITY_TIMEOUT: Duration = Duration::from_secs(10);
const ANTHROPIC_VERSION: &str = "2023-06-01";
const LM_STUDIO_PROVIDER_NAME: &str = "LM Studio";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderConnectivityIssueCode {
    InvalidBaseUrl,
    LocalProxyUrl,
    CredentialRequired,
    CredentialEndpointMismatch,
    CredentialUnavailable,
    Timeout,
    Network,
    Unauthorized,
    Forbidden,
    NotFound,
    RateLimited,
    UpstreamServer,
    RedirectRejected,
    ResponseTooLarge,
    InvalidResponse,
    EmptyModelCatalog,
    RequestFailed,
    ProviderUnavailable,
    HttpError,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderConnectivityIssue {
    code: ProviderConnectivityIssueCode,
    retryable: bool,
    http_status: Option<u16>,
}

impl ProviderConnectivityIssue {
    pub(crate) fn new(code: ProviderConnectivityIssueCode) -> Self {
        let retryable = matches!(
            code,
            ProviderConnectivityIssueCode::Timeout
                | ProviderConnectivityIssueCode::Network
                | ProviderConnectivityIssueCode::RateLimited
                | ProviderConnectivityIssueCode::UpstreamServer
                | ProviderConnectivityIssueCode::RequestFailed
        );
        Self {
            code,
            retryable,
            http_status: None,
        }
    }

    pub(crate) fn from_http_status(status: u16) -> Self {
        let code = match status {
            300..=399 => ProviderConnectivityIssueCode::RedirectRejected,
            401 => ProviderConnectivityIssueCode::Unauthorized,
            403 => ProviderConnectivityIssueCode::Forbidden,
            404 => ProviderConnectivityIssueCode::NotFound,
            429 => ProviderConnectivityIssueCode::RateLimited,
            500..=599 => ProviderConnectivityIssueCode::UpstreamServer,
            _ => ProviderConnectivityIssueCode::HttpError,
        };
        Self {
            http_status: Some(status),
            ..Self::new(code)
        }
    }

    pub(crate) fn from_request(error: &reqwest::Error) -> Self {
        let code = if error.is_timeout() {
            ProviderConnectivityIssueCode::Timeout
        } else if error.is_connect() {
            ProviderConnectivityIssueCode::Network
        } else {
            ProviderConnectivityIssueCode::RequestFailed
        };
        Self::new(code)
    }
}

impl fmt::Display for ProviderConnectivityIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.code {
            ProviderConnectivityIssueCode::InvalidBaseUrl => "Provider Base URL is invalid",
            ProviderConnectivityIssueCode::LocalProxyUrl => {
                "Provider Base URL points to the QuotaHorizon local proxy"
            }
            ProviderConnectivityIssueCode::CredentialRequired => "Provider credential is required",
            ProviderConnectivityIssueCode::CredentialEndpointMismatch => {
                "The saved Provider credential cannot be used with a different endpoint"
            }
            ProviderConnectivityIssueCode::CredentialUnavailable => {
                "Saved Provider credential is unavailable"
            }
            ProviderConnectivityIssueCode::Timeout => "Provider connection timed out",
            ProviderConnectivityIssueCode::Network => "Provider network connection failed",
            ProviderConnectivityIssueCode::Unauthorized => "Provider rejected authentication",
            ProviderConnectivityIssueCode::Forbidden => "Provider denied access",
            ProviderConnectivityIssueCode::NotFound => "Provider endpoint was not found",
            ProviderConnectivityIssueCode::RateLimited => "Provider rate limit was reached",
            ProviderConnectivityIssueCode::UpstreamServer => "Provider server failed",
            ProviderConnectivityIssueCode::RedirectRejected => {
                "Provider endpoint redirected the credential-bearing request"
            }
            ProviderConnectivityIssueCode::ResponseTooLarge => "Provider response is too large",
            ProviderConnectivityIssueCode::InvalidResponse => "Provider response is invalid",
            ProviderConnectivityIssueCode::EmptyModelCatalog => {
                "Provider returned no available models"
            }
            ProviderConnectivityIssueCode::RequestFailed => "Provider request failed",
            ProviderConnectivityIssueCode::ProviderUnavailable => {
                "Provider configuration is unavailable"
            }
            ProviderConnectivityIssueCode::HttpError => "Provider returned an HTTP error",
        };
        match self.http_status {
            Some(status) => write!(formatter, "{label} (HTTP {status})"),
            None => formatter.write_str(label),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderConnectivityReport {
    reachable: bool,
    issue: Option<ProviderConnectivityIssue>,
}

impl ProviderConnectivityReport {
    fn reachable() -> Self {
        Self {
            reachable: true,
            issue: None,
        }
    }

    fn failed(issue: ProviderConnectivityIssue) -> Self {
        Self {
            reachable: false,
            issue: Some(issue),
        }
    }
}

#[derive(Clone, Copy)]
enum ConnectivityEndpoint {
    Standard,
    ClaudeCode,
    Grok,
    LmStudio,
}

#[tauri::command]
pub(crate) async fn test_provider_connectivity<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    id: String,
) -> Result<ProviderConnectivityReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        match test_provider_connectivity_blocking(&app, &id) {
            Ok(()) => ProviderConnectivityReport::reachable(),
            Err(issue) => ProviderConnectivityReport::failed(issue),
        }
    })
    .await
    .map_err(|error| format!("Provider connectivity task failed: {error}"))
}

fn test_provider_connectivity_blocking<R: Runtime>(
    app: &tauri::AppHandle<R>,
    id: &str,
) -> Result<(), ProviderConnectivityIssue> {
    let paths = resolve_paths(app).map_err(|_| {
        ProviderConnectivityIssue::new(ProviderConnectivityIssueCode::ProviderUnavailable)
    })?;
    let provider = read_provider(&paths, id).map_err(|_| {
        ProviderConnectivityIssue::new(ProviderConnectivityIssueCode::ProviderUnavailable)
    })?;
    let endpoint = connectivity_endpoint(&provider);
    let url = connectivity_url(&provider.base_url, endpoint)?;
    let client = connectivity_client(&url)?;
    let response = send_connectivity_request(&client, &url, &provider, endpoint)?;
    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(ProviderConnectivityIssue::from_http_status(status.as_u16()))
    }
}

fn connectivity_endpoint(provider: &ProviderProfile) -> ConnectivityEndpoint {
    let identity = (
        provider.kind,
        provider.name.as_str(),
        provider.base_url.as_str(),
        provider.api_format,
    );
    if is_claude_code_identity(identity.0, identity.1, identity.2, identity.3) {
        return ConnectivityEndpoint::ClaudeCode;
    }
    if is_grok_identity(identity.0, identity.1, identity.2, identity.3) {
        return ConnectivityEndpoint::Grok;
    }
    if provider.name.trim() == LM_STUDIO_PROVIDER_NAME {
        return ConnectivityEndpoint::LmStudio;
    }
    ConnectivityEndpoint::Standard
}

fn connectivity_url(
    base_url: &str,
    endpoint: ConnectivityEndpoint,
) -> Result<Url, ProviderConnectivityIssue> {
    let normalized = base_url.trim().trim_end_matches('/');
    ensure_not_local_proxy_base_url(normalized).map_err(|_| {
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
    let path = match endpoint {
        ConnectivityEndpoint::ClaudeCode => "/v1/models".to_string(),
        ConnectivityEndpoint::Grok => "/v1/language-models".to_string(),
        ConnectivityEndpoint::LmStudio => "/api/v1/models".to_string(),
        ConnectivityEndpoint::Standard => format!("{}/models", url.path().trim_end_matches('/')),
    };
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn connectivity_client(url: &Url) -> Result<Client, ProviderConnectivityIssue> {
    let builder = ClientBuilder::new()
        .timeout(CONNECTIVITY_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("QuotaHorizon");
    let builder = if url.host_str().is_some_and(is_loopback_host) {
        builder.no_proxy()
    } else {
        crate::system_proxy::apply(builder)
    };
    builder
        .build()
        .map_err(|_| ProviderConnectivityIssue::new(ProviderConnectivityIssueCode::RequestFailed))
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || matches!(host, "127.0.0.1" | "::1" | "[::1]")
}

fn send_connectivity_request(
    client: &Client,
    url: &Url,
    provider: &ProviderProfile,
    endpoint: ConnectivityEndpoint,
) -> Result<reqwest::blocking::Response, ProviderConnectivityIssue> {
    let mut request = client
        .get(url.clone())
        .header(reqwest::header::ACCEPT, "application/json");
    let api_key = provider.api_key.trim();
    if !api_key.is_empty() {
        request = request.bearer_auth(api_key);
    }
    if matches!(endpoint, ConnectivityEndpoint::ClaudeCode) {
        request = request
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("x-api-key", api_key);
    }
    request
        .send()
        .map_err(|error| ProviderConnectivityIssue::from_request(&error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ProviderApiFormat, ProviderKind};
    use tiny_http::{Header, Response, Server};

    fn provider(base_url: String) -> ProviderProfile {
        ProviderProfile {
            id: "provider-a".to_string(),
            kind: ProviderKind::Custom,
            name: "Relay".to_string(),
            group: String::new(),
            base_url,
            api_key: "sk-connectivity-test".to_string(),
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
    fn appends_standard_models_endpoint() {
        let url = connectivity_url(
            "https://relay.example.com/api/v1/",
            ConnectivityEndpoint::Standard,
        )
        .unwrap();
        assert_eq!(url.as_str(), "https://relay.example.com/api/v1/models");
    }

    #[test]
    fn uses_provider_specific_models_endpoints() {
        let claude = connectivity_url(
            "https://api.anthropic.com/v1",
            ConnectivityEndpoint::ClaudeCode,
        )
        .unwrap();
        let grok = connectivity_url("https://api.x.ai/v1", ConnectivityEndpoint::Grok).unwrap();
        assert_eq!(claude.as_str(), "https://api.anthropic.com/v1/models");
        assert_eq!(grok.as_str(), "https://api.x.ai/v1/language-models");
    }

    #[test]
    fn classifies_http_failures_without_server_text() {
        assert_eq!(
            ProviderConnectivityIssue::from_http_status(401).code,
            ProviderConnectivityIssueCode::Unauthorized
        );
        assert_eq!(
            ProviderConnectivityIssue::from_http_status(429).code,
            ProviderConnectivityIssueCode::RateLimited
        );
        assert!(ProviderConnectivityIssue::from_http_status(503).retryable);
        assert!(!ProviderConnectivityIssue::from_http_status(404).retryable);
    }

    #[test]
    fn issue_wire_contract_is_stable_and_redacted() {
        let issue = ProviderConnectivityIssue::from_http_status(401);
        assert_eq!(
            serde_json::to_value(issue).unwrap(),
            serde_json::json!({
                "code": "unauthorized",
                "retryable": false,
                "httpStatus": 401
            })
        );
    }

    #[test]
    fn credential_bearing_connectivity_checks_do_not_follow_redirects() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", server.server_addr());
        let provider = provider(base_url);
        let url = connectivity_url(&provider.base_url, ConnectivityEndpoint::Standard).unwrap();
        let client = connectivity_client(&url).unwrap();
        let worker = std::thread::spawn(move || {
            let request = server.recv().unwrap();
            assert!(request.headers().iter().any(|header| {
                header.field.equiv("Authorization")
                    && header.value.as_str() == "Bearer sk-connectivity-test"
            }));
            request
                .respond(
                    Response::empty(302)
                        .with_header(Header::from_bytes("Location", "/redirected").unwrap()),
                )
                .unwrap();
        });

        let response =
            send_connectivity_request(&client, &url, &provider, ConnectivityEndpoint::Standard)
                .unwrap();

        assert_eq!(response.status().as_u16(), 302);
        worker.join().unwrap();
    }
}
