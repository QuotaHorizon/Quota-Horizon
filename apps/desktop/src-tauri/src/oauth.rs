use std::{
    collections::HashMap,
    io::Read,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use reqwest::blocking::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager, Runtime, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;
use tiny_http::{Header, Request, Response as HttpResponse, Server, StatusCode};

use crate::{
    auth::decode_jwt,
    browser::open_default_private,
    codex_api::{CLIENT_ID, ISSUER, ORIGINATOR},
    models::{LoginPhase, LoginStart, LoginStatus},
    storage::{resolve_paths, save_login_account},
};

mod attempt;
use attempt::LoginAttempt;

#[derive(Default)]
pub(crate) struct AppState {
    login: Mutex<Option<Arc<LoginAttempt>>>,
}

fn random_urlsafe<const N: usize>() -> String {
    let mut bytes = [0_u8; N];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn bind_login_server() -> Result<(Server, u16), String> {
    for port in [1455_u16, 1457_u16] {
        if let Ok(server) = Server::http(("127.0.0.1", port)) {
            return Ok((server, port));
        }
    }
    Err("登录回调端口 1455 和 1457 均被占用，请关闭其他 Codex 登录窗口后重试".to_string())
}

fn authorize_url(port: u16, state: &str, challenge: &str) -> Result<String, String> {
    let redirect_uri = format!("http://localhost:{port}/auth/callback");
    let mut url =
        url::Url::parse(&format!("{ISSUER}/oauth/authorize")).map_err(|error| error.to_string())?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair(
            "scope",
            "openid profile email offline_access api.connectors.read api.connectors.invoke",
        )
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("state", state)
        .append_pair("originator", ORIGINATOR);
    Ok(url.to_string())
}

fn exchange_code(client: &Client, port: u16, code: &str, verifier: &str) -> Result<Value, String> {
    let redirect_uri = format!("http://localhost:{port}/auth/callback");
    let response = client
        .post(format!("{ISSUER}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("originator", ORIGINATOR)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &redirect_uri),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier),
        ])
        .send()
        .map_err(|_| "network".to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err("denied".to_string());
        }
        return Err("network".to_string());
    }
    const MAX_TOKEN_RESPONSE: u64 = 1024 * 1024;
    let mut bytes = Vec::new();
    response
        .take(MAX_TOKEN_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "network".to_string())?;
    if bytes.len() as u64 > MAX_TOKEN_RESPONSE {
        return Err("invalidResponse".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalidResponse".to_string())
}

fn html_response(request: Request, status: u16, title: &str, message: &str) {
    let safe_title = title.replace('<', "&lt;").replace('>', "&gt;");
    let safe_message = message.replace('<', "&lt;").replace('>', "&gt;");
    let body = format!(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>{safe_title}</title><style>body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#f2f6f1;color:#183024;font-family:system-ui,sans-serif}}main{{width:min(420px,calc(100% - 40px));padding:38px;text-align:center;border:1px solid #d8e4da;border-radius:18px;background:white;box-shadow:0 24px 70px #1738241a}}i{{display:grid;place-items:center;width:54px;height:54px;margin:auto;border-radius:16px;background:#e2f1e5;color:#28734d;font-style:normal;font-size:26px}}h1{{font-size:22px;margin:18px 0 8px}}p{{color:#6c7c72;line-height:1.6;font-size:14px}}</style></head><body><main><i>✓</i><h1>{safe_title}</h1><p>{safe_message}</p></main></body></html>"#
    );
    let mut response = HttpResponse::from_string(body).with_status_code(StatusCode(status));
    if let Ok(header) = Header::from_bytes("Content-Type", "text/html; charset=utf-8") {
        response = response.with_header(header);
    }
    let _ = request.respond(response);
}

pub(crate) fn emit_login<R: Runtime>(
    app: &tauri::AppHandle<R>,
    ok: bool,
    message: impl Into<String>,
    account_id: Option<String>,
) {
    let _ = app.emit(
        "login-status",
        LoginStatus {
            ok,
            message: message.into(),
            account_id,
            login_id: None,
            phase: None,
            reason: None,
        },
    );
}

fn close_login_window<R: Runtime>(app: &tauri::AppHandle<R>, attempt: &LoginAttempt) {
    if let Some(window) = app.get_webview_window(&attempt.window_label()) {
        let _ = window.close();
    }
}

fn publish<R: Runtime>(app: &tauri::AppHandle<R>, status: LoginStatus) {
    let _ = app.emit("login-status", status);
}

fn transition<R: Runtime>(
    app: &tauri::AppHandle<R>,
    attempt: &LoginAttempt,
    phase: LoginPhase,
    reason: Option<&str>,
) {
    if let Some(status) = attempt.transition(phase, reason) {
        publish(app, status);
    }
}

fn refresh_saved_account_usage<R: Runtime>(app: &tauri::AppHandle<R>, account_id: &str) {
    if crate::commands::refresh_usage_blocking(app.clone(), account_id.to_string()).is_err() {
        eprintln!("initial account usage refresh failed; saved account retained");
    }
}

fn open_login_in_default_browser<R: Runtime>(
    app: &tauri::AppHandle<R>,
    url: &str,
) -> Result<(), String> {
    app.opener()
        .open_url(url.to_string(), None::<&str>)
        .map_err(|error| format!("无法打开默认浏览器：{error}"))
}

fn open_login_browser<R: Runtime>(
    app: &tauri::AppHandle<R>,
    url: &str,
    private_mode: bool,
) -> Result<(), String> {
    if private_mode {
        return open_default_private(app, url);
    }
    open_login_in_default_browser(app, url)
}

fn open_embedded_login_window<R: Runtime + 'static>(
    app: &tauri::AppHandle<R>,
    url: &str,
    attempt: Arc<LoginAttempt>,
) -> Result<(), String> {
    let fallback_url = url.to_string();
    let window_app = app.clone();
    app.run_on_main_thread(move || {
        if !attempt.active() {
            return;
        }
        let result = (|| {
            let redirect_target = serde_json::to_string(&fallback_url)
                .unwrap_or_else(|_| "\"about:blank\"".to_string());
            let redirect_script =
                format!("window.setTimeout(() => window.location.replace({redirect_target}), 50);");
            let window = WebviewWindowBuilder::new(
                &window_app,
                attempt.window_label(),
                WebviewUrl::App("login.html".into()),
            )
            .title("登录 ChatGPT - QuotaHorizon")
            .inner_size(520.0, 720.0)
            .min_inner_size(420.0, 620.0)
            .center()
            .incognito(true)
            .build()?;
            let close_app = window_app.clone();
            let close_attempt = attempt.clone();
            window.on_window_event(move |event| {
                if matches!(
                    event,
                    tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                ) {
                    transition(&close_app, &close_attempt, LoginPhase::Cancelled, None);
                }
            });
            window.show()?;
            window.set_focus()?;
            window.eval(redirect_script)?;
            Ok::<(), tauri::Error>(())
        })();

        if result.is_err() && attempt.active() {
            match open_login_in_default_browser(&window_app, &fallback_url) {
                Ok(()) => transition(&window_app, &attempt, LoginPhase::BrowserFallback, None),
                Err(_) => transition(
                    &window_app,
                    &attempt,
                    LoginPhase::Failed,
                    Some("openBrowser"),
                ),
            }
        }
    })
    .map_err(|error| format!("无法调度应用内登录窗口：{error}"))
}

fn run_login_loop<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    server: Server,
    port: u16,
    expected_state: String,
    verifier: String,
    attempt: Arc<LoginAttempt>,
) {
    while attempt.active() && !attempt.expired() {
        let request = match server.recv_timeout(Duration::from_millis(250)) {
            Ok(Some(request)) => request,
            Ok(None) => continue,
            Err(_) => {
                transition(
                    &app,
                    &attempt,
                    LoginPhase::Failed,
                    Some("callbackUnavailable"),
                );
                break;
            }
        };
        let parsed = match url::Url::parse(&format!("http://localhost{}", request.url())) {
            Ok(value) => value,
            Err(_) => {
                html_response(
                    request,
                    400,
                    "登录请求无效",
                    "回调地址无法解析，请重新尝试。",
                );
                continue;
            }
        };
        if request.method() != &tiny_http::Method::Get || parsed.path() != "/auth/callback" {
            html_response(request, 404, "页面不存在", "请回到 QuotaHorizon 继续操作。");
            continue;
        }
        let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
        if params.get("state") != Some(&expected_state) {
            html_response(
                request,
                400,
                "安全校验失败",
                "登录 state 不匹配，请关闭窗口后重试。",
            );
            continue;
        }
        if params.contains_key("error") {
            html_response(
                request,
                403,
                "登录未完成",
                "授权未完成，当前登录未改变。请回到 Horizon 重试。",
            );
            transition(&app, &attempt, LoginPhase::Failed, Some("denied"));
            break;
        }
        let Some(code) = params.get("code").filter(|value| !value.is_empty()) else {
            html_response(request, 400, "登录未完成", "授权响应中缺少 code。");
            transition(&app, &attempt, LoginPhase::Failed, Some("invalidResponse"));
            break;
        };
        let client = match crate::system_proxy::apply(Client::builder())
            .timeout(Duration::from_secs(25))
            .redirect(reqwest::redirect::Policy::none())
            .build()
        {
            Ok(client) => client,
            Err(_) => {
                html_response(request, 500, "登录失败", "无法创建安全网络连接。");
                transition(&app, &attempt, LoginPhase::Failed, Some("network"));
                break;
            }
        };
        transition(&app, &attempt, LoginPhase::Exchanging, None);
        if !attempt.active() {
            html_response(request, 409, "登录已取消", "当前登录未改变。");
            break;
        }
        let tokens = match exchange_code(&client, port, code, &verifier) {
            Ok(tokens) => tokens,
            Err(reason) => {
                transition(&app, &attempt, LoginPhase::Failed, Some(&reason));
                html_response(
                    request,
                    502,
                    "登录未完成",
                    "未能完成登录，当前账号未改变。请回到 Horizon 查看状态并重试。",
                );
                break;
            }
        };
        let auth = match auth_from_tokens(tokens) {
            Ok(auth) => auth,
            Err(_) => {
                transition(&app, &attempt, LoginPhase::Failed, Some("invalidResponse"));
                html_response(
                    request,
                    502,
                    "授权响应不完整",
                    "账号没有保存，当前登录未改变。请回到 Horizon 重试。",
                );
                break;
            }
        };
        match attempt.save(|| save_login_account(&resolve_paths(&app)?, auth)) {
            Some(status) if status.ok => {
                html_response(
                    request,
                    200,
                    "账号已保存",
                    "当前 Codex 登录未改变。请回到 Horizon，预览并确认后切换。",
                );
                let account_id = status
                    .account_id
                    .clone()
                    .expect("successful save has account id");
                publish(&app, status);
                let _ = app.emit("accounts-changed", ());
                close_login_window(&app, &attempt);
                // The callback port is free before the optional first quota refresh.
                drop(server);
                refresh_saved_account_usage(&app, &account_id);
                crate::system_tray::refresh_menu(&app);
                return;
            }
            status => {
                if let Some(status) = status {
                    publish(&app, status);
                }
                html_response(
                    request,
                    409,
                    "登录未保存",
                    "登录已取消、超时或保存失败。请回到 Horizon 查看状态。",
                );
                break;
            }
        }
    }
    if attempt.expired() {
        transition(&app, &attempt, LoginPhase::TimedOut, None);
    }
    close_login_window(&app, &attempt);
}

fn auth_from_tokens(tokens: Value) -> Result<Value, String> {
    let id_token = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "登录响应缺少 id_token".to_string())?;
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "登录响应缺少 access_token".to_string())?;
    let refresh_token = tokens
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "登录响应缺少 refresh_token".to_string())?;
    let claims = decode_jwt(id_token)?;
    let account_id = claims
        .get("https://api.openai.com/auth")
        .and_then(|value| value.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Login response has no account identity".to_string())?;
    let identity = claims
        .get("https://api.openai.com/auth")
        .and_then(|value| {
            value
                .get("chatgpt_user_id")
                .or_else(|| value.get("user_id"))
        })
        .or_else(|| claims.get("sub"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    if identity.is_none() {
        return Err("Login response has no user identity".into());
    }
    Ok(json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "last_refresh": chrono::Utc::now().to_rfc3339(),
        "tokens": {
            "id_token": id_token,
            "access_token": access_token,
            "refresh_token": refresh_token,
            "account_id": account_id,
        },
    }))
}

#[tauri::command]
pub(crate) fn start_login<R: Runtime + 'static>(
    app: tauri::AppHandle<R>,
    window: WebviewWindow<R>,
    state: State<'_, AppState>,
    embedded: bool,
    private_mode: bool,
    login_id: String,
) -> Result<LoginStart, String> {
    require_main_window(&window)?;
    if uuid::Uuid::parse_str(&login_id).is_err() {
        return Err("invalidRequest".into());
    }
    let attempt = {
        let mut current = state.login.lock().map_err(|_| "unavailable".to_string())?;
        if current.as_ref().is_some_and(|attempt| attempt.active()) {
            return Err("alreadyRunning".into());
        }
        let attempt = Arc::new(LoginAttempt::new(login_id.clone()));
        *current = Some(attempt.clone());
        attempt
    };
    let (server, port) = match bind_login_server() {
        Ok(server) => server,
        Err(_) => {
            transition(&app, &attempt, LoginPhase::Failed, Some("portBusy"));
            return Err("portBusy".into());
        }
    };
    let verifier = random_urlsafe::<64>();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let oauth_state = random_urlsafe::<32>();
    let url = match authorize_url(port, &oauth_state, &challenge) {
        Ok(url) => url,
        Err(_) => {
            transition(&app, &attempt, LoginPhase::Failed, Some("invalidRequest"));
            return Err("invalidRequest".into());
        }
    };

    let thread_app = app.clone();
    let thread_attempt = attempt.clone();
    thread::spawn(move || {
        run_login_loop(
            thread_app,
            server,
            port,
            oauth_state,
            verifier,
            thread_attempt,
        )
    });

    if embedded {
        let window_app = app.clone();
        let window_url = url.clone();
        let window_attempt = attempt.clone();
        thread::spawn(move || {
            if open_embedded_login_window(&window_app, &window_url, window_attempt.clone()).is_err()
            {
                transition(
                    &window_app,
                    &window_attempt,
                    LoginPhase::Failed,
                    Some("openBrowser"),
                );
            }
        });
    } else if attempt.active() && open_login_browser(&app, &url, private_mode).is_err() {
        transition(&app, &attempt, LoginPhase::Failed, Some("openBrowser"));
        return Err("openBrowser".into());
    }
    Ok(LoginStart { login_id, embedded })
}

fn require_main_window<R: Runtime>(window: &WebviewWindow<R>) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("mainWindowRequired".into())
    }
}

#[tauri::command]
pub(crate) fn get_login_status<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, AppState>,
) -> Result<Option<LoginStatus>, String> {
    require_main_window(&window)?;
    Ok(state
        .login
        .lock()
        .map_err(|_| "unavailable".to_string())?
        .as_ref()
        .map(|attempt| attempt.snapshot()))
}

#[tauri::command]
pub(crate) fn cancel_login<R: Runtime>(
    app: tauri::AppHandle<R>,
    window: WebviewWindow<R>,
    state: State<'_, AppState>,
    login_id: String,
) -> Result<Option<LoginStatus>, String> {
    require_main_window(&window)?;
    let attempt = state
        .login
        .lock()
        .map_err(|_| "unavailable".to_string())?
        .clone();
    let Some(attempt) = attempt.filter(|attempt| attempt.id == login_id) else {
        return Ok(None);
    };
    transition(&app, &attempt, LoginPhase::Cancelled, None);
    close_login_window(&app, &attempt);
    Ok(Some(attempt.snapshot()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_response_requires_all_nonempty_tokens_and_a_readable_identity() {
        let claims = json!({"sub": "synthetic", "https://api.openai.com/auth": {"chatgpt_account_id": "synthetic"}});
        let token = format!(
            "e30.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        );
        let complete = json!({"id_token": token, "access_token": "synthetic-access", "refresh_token": "synthetic-refresh"});
        let auth = auth_from_tokens(complete.clone()).unwrap();
        assert!(
            chrono::DateTime::parse_from_rfc3339(auth["last_refresh"].as_str().unwrap()).is_ok()
        );
        assert_eq!(auth["tokens"]["account_id"], "synthetic");
        for field in ["id_token", "access_token", "refresh_token"] {
            let mut incomplete = complete.clone();
            incomplete[field] = json!("");
            assert!(auth_from_tokens(incomplete).is_err());
        }
        let mut missing_identity = complete.clone();
        missing_identity["id_token"] = json!("e30.e30.signature");
        assert!(auth_from_tokens(missing_identity).is_err());
        assert!(auth_from_tokens(
            json!({"id_token": "broken", "access_token": "synthetic", "refresh_token": "synthetic"})
        )
        .is_err());
    }
}
