//! Stable public release discovery. Installation remains the native installer flow.
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const API: &str = "https://api.github.com/repos/QuotaHorizon/Quota-Horizon/releases/latest";
const RELEASES: &str = "https://github.com/QuotaHorizon/Quota-Horizon/releases";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseUpdate {
    current_version: String,
    latest_version: Option<String>,
    available: bool,
    notes: String,
    url: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    body: Option<String>,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    state: String,
    size: u64,
}

fn stable_version(value: &str) -> Option<[u64; 3]> {
    let parts = value
        .strip_prefix('v')
        .unwrap_or(value)
        .split('.')
        .collect::<Vec<_>>();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some([
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ])
}

fn interpret(release: Release, current: &str, platform: &str) -> Result<ReleaseUpdate, String> {
    let latest = stable_version(&release.tag_name).ok_or("invalid_release_version")?;
    let installed = stable_version(current).ok_or("invalid_current_version")?;
    let suffix = match platform {
        "macos" => ".dmg",
        "windows" => ".exe",
        _ => "unsupported",
    };
    let ready = release
        .assets
        .iter()
        .any(|a| a.name.ends_with(suffix) && a.state == "uploaded" && a.size > 0);
    Ok(ReleaseUpdate {
        current_version: current.into(),
        latest_version: Some(release.tag_name.trim_start_matches('v').into()),
        available: !release.draft && !release.prerelease && ready && latest > installed,
        notes: release
            .body
            .unwrap_or_default()
            .chars()
            .take(12_000)
            .collect(),
        // Never accept a destination supplied by release text or a third party.
        url: format!("{RELEASES}/tag/{}", release.tag_name),
    })
}

fn fetch() -> Result<ReleaseUpdate, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("QuotaHorizon/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "update_network_unavailable")?;
    let response = client
        .get(API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|_| "update_network_unavailable")?;
    if response.status() == reqwest::StatusCode::FORBIDDEN
        || response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return fetch_web_release(&client);
    }
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(ReleaseUpdate {
            current_version: env!("CARGO_PKG_VERSION").into(),
            latest_version: None,
            available: false,
            notes: String::new(),
            url: RELEASES.into(),
        });
    }
    let response = response
        .error_for_status()
        .map_err(|_| "update_service_unavailable")?;
    let mut body = Vec::new();
    response
        .take(512 * 1024 + 1)
        .read_to_end(&mut body)
        .map_err(|_| "update_read_failed")?;
    if body.len() > 512 * 1024 {
        return Err("update_response_too_large".into());
    }
    let release = serde_json::from_slice(&body).map_err(|_| "invalid_release_response")?;
    interpret(release, env!("CARGO_PKG_VERSION"), std::env::consts::OS)
}

fn interpret_web_release(
    tag: &str,
    html: &str,
    current: &str,
    platform: &str,
) -> Result<ReleaseUpdate, String> {
    let version = stable_version(tag).ok_or("invalid_release_version")?;
    let installed = stable_version(current).ok_or("invalid_current_version")?;
    let prefix = format!("/QuotaHorizon/Quota-Horizon/releases/download/{tag}/");
    let suffix = match platform {
        "macos" => ".dmg",
        "windows" => ".exe",
        _ => "unsupported",
    };
    let ready = html
        .split("href=\"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .any(|path| path.starts_with(&prefix) && path.ends_with(suffix));
    Ok(ReleaseUpdate {
        current_version: current.into(),
        latest_version: Some(tag.trim_start_matches('v').into()),
        available: ready && version > installed,
        notes: String::new(),
        url: format!("{RELEASES}/tag/{tag}"),
    })
}

fn fetch_web_release(client: &reqwest::blocking::Client) -> Result<ReleaseUpdate, String> {
    // The official latest redirect has the same stable-release selection as
    // the API, without consuming its shared anonymous-IP quota.
    let response = client
        .get(format!("{RELEASES}/latest"))
        .send()
        .map_err(|_| "update_network_unavailable")?;
    if !response.status().is_redirection() {
        return Err("update_service_unavailable".into());
    }
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .ok_or("invalid_release_redirect")?;
    let tag = location
        .strip_prefix(&format!("{RELEASES}/tag/"))
        .ok_or("invalid_release_redirect")?;
    stable_version(tag).ok_or("invalid_release_version")?;
    let response = client
        .get(format!("{RELEASES}/expanded_assets/{tag}"))
        .send()
        .map_err(|_| "update_network_unavailable")?
        .error_for_status()
        .map_err(|_| "update_service_unavailable")?;
    let mut body = Vec::new();
    response
        .take(512 * 1024 + 1)
        .read_to_end(&mut body)
        .map_err(|_| "update_read_failed")?;
    if body.len() > 512 * 1024 {
        return Err("update_response_too_large".into());
    }
    let html = std::str::from_utf8(&body).map_err(|_| "invalid_release_response")?;
    interpret_web_release(tag, html, env!("CARGO_PKG_VERSION"), std::env::consts::OS)
}

#[tauri::command]
pub async fn check_release_update() -> Result<ReleaseUpdate, String> {
    static CACHE: OnceLock<Mutex<Option<(Instant, ReleaseUpdate)>>> = OnceLock::new();
    tauri::async_runtime::spawn_blocking(|| {
        let mut cache = CACHE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map_err(|_| "update_check_busy")?;
        if let Some((at, value)) = cache.as_ref() {
            if at.elapsed() < Duration::from_secs(60) {
                return Ok(value.clone());
            }
        }
        let value = fetch()?;
        *cache = Some((Instant::now(), value.clone()));
        Ok(value)
    })
    .await
    .map_err(|_| "update_check_failed")?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn release(tag: &str, asset: &str) -> Release {
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            body: Some("Changes".into()),
            assets: vec![Asset {
                name: asset.into(),
                state: "uploaded".into(),
                size: 100,
            }],
        }
    }
    #[test]
    fn checks_numeric_versions_and_platform_installers() {
        assert!(
            interpret(release("v0.10.0", "App.dmg"), "0.2.0", "macos")
                .unwrap()
                .available
        );
        assert!(
            interpret(release("v0.3.0", "App.exe"), "0.2.0", "windows")
                .unwrap()
                .available
        );
        assert!(
            !interpret(release("v0.3.0", "App.exe"), "0.2.0", "macos")
                .unwrap()
                .available
        );
        assert!(
            !interpret(release("v0.2.0", "App.dmg"), "0.2.0", "macos")
                .unwrap()
                .available
        );
        assert!(
            !interpret(release("v0.1.0", "App.dmg"), "0.2.0", "macos")
                .unwrap()
                .available
        );
        for tag in [
            "v0.3.0-rc.1",
            "v0.3.0/../../evil",
            "1.2",
            "1.2.3.4",
            "+1.2.3",
        ] {
            assert!(interpret(release(tag, "App.dmg"), "0.2.0", "macos").is_err());
        }
        let mut draft = release("v0.3.0", "App.dmg");
        draft.draft = true;
        assert!(!interpret(draft, "0.2.0", "macos").unwrap().available);
    }
    #[test]
    fn web_fallback_requires_an_official_platform_asset() {
        let html =
            "<a href=\"/QuotaHorizon/Quota-Horizon/releases/download/v0.3.0/App.dmg\">Download</a>";
        assert!(
            interpret_web_release("v0.3.0", html, "0.2.0", "macos")
                .unwrap()
                .available
        );
        assert!(
            !interpret_web_release("v0.3.0", html, "0.2.0", "windows")
                .unwrap()
                .available
        );
        assert!(
            !interpret_web_release(
                "v0.3.0",
                &html.replace("/QuotaHorizon/", "https://example.com/QuotaHorizon/"),
                "0.2.0",
                "macos"
            )
            .unwrap()
            .available
        );
    }
    #[test]
    #[ignore = "read-only live GitHub check"]
    fn check_public_release() {
        let result = fetch().unwrap();
        println!("{}", serde_json::to_string(&result).unwrap());
    }
}
