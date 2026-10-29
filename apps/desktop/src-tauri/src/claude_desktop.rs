use std::path::{Path, PathBuf};

#[cfg(not(target_os = "macos"))]
use std::{env, fs};

use serde_json::{json, Value};

use crate::third_party_apps::config_transaction::ConfigTransaction;

const CONFIG_FILE: &str = "claude_desktop_config.json";
const PROFILE_ID: &str = "00000000-0000-4000-8000-000000157210";
const PROFILE_NAME: &str = "QuotaHorizon";
const PROXY_TOKEN: &str = "PROXY_MANAGED";
const PROXY_ROUTE: &str = "/claude-desktop";

pub(crate) fn write_official_proxy_settings(
    transaction: &mut ConfigTransaction,
    context_window: u64,
) -> Result<(), String> {
    sync_desktop_paths(transaction, false, &gateway_profile(context_window))
}

pub(crate) fn write_provider_proxy_settings(
    transaction: &mut ConfigTransaction,
    provider: &crate::models::ProviderProfile,
) -> Result<(), String> {
    sync_desktop_paths(transaction, false, &provider_gateway_profile(provider))
}

pub(crate) fn clear_proxy_settings(transaction: &mut ConfigTransaction) -> Result<(), String> {
    sync_desktop_paths(transaction, true, &Value::Null)
}

fn sync_desktop_paths(
    transaction: &mut ConfigTransaction,
    clear: bool,
    profile: &Value,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        sync_macos_paths(transaction, clear, profile)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut errors = Vec::new();
        let mut cleared_active_profile = false;
        for root in desktop_config_roots() {
            let result = if clear {
                clear_at_root(transaction, &root).map(|active| {
                    cleared_active_profile |= active;
                })
            } else {
                write_at_root(transaction, &root, profile)
            };
            if let Err(error) = result {
                errors.push(format!("{}：{error}", root.display()));
            }
        }
        #[cfg(windows)]
        if let Err(error) = if !clear || cleared_active_profile {
            sync_windows_login_mode(transaction, clear)
        } else {
            Ok(())
        } {
            errors.push(format!("Windows 登录模式：{error}"));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }
}

#[cfg(windows)]
fn sync_windows_login_mode(transaction: &mut ConfigTransaction, clear: bool) -> Result<(), String> {
    let roaming_app_data = env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join("AppData").join("Roaming")))
        .ok_or_else(|| "无法定位 Windows Roaming 配置目录".to_string())?;
    let root = windows_login_root(&roaming_app_data);
    let config_path = root.join(CONFIG_FILE);
    if clear {
        if config_path.exists() {
            update_deployment_mode(transaction, &config_path, "1p")?;
        }
        return Ok(());
    }
    if root.is_dir() {
        update_deployment_mode(transaction, &config_path, "3p")?;
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn windows_login_root(roaming_app_data: &Path) -> PathBuf {
    roaming_app_data.join("Claude")
}

#[cfg(not(target_os = "macos"))]
fn desktop_config_roots() -> Vec<PathBuf> {
    let local_app_data = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join("AppData").join("Local")));
    let Some(local_app_data) = local_app_data else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    for name in ["Claude", "Claude-3p"] {
        let path = local_app_data.join(name);
        if path.is_dir() {
            roots.push(path);
        }
    }
    append_store_package_roots(&mut roots, &local_app_data);
    append_store_package_roots(&mut roots, &local_app_data.join("Packages"));
    roots.sort();
    roots.dedup();
    roots
}

#[cfg(not(target_os = "macos"))]
fn append_store_package_roots(roots: &mut Vec<PathBuf>, parent: &Path) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !name.starts_with("Claude_") || !path.is_dir() {
            continue;
        }
        let package_root = path.join("LocalCache").join("Roaming").join("Claude");
        if package_root.is_dir() {
            roots.push(package_root);
        }
    }
}

#[cfg(target_os = "macos")]
fn sync_macos_paths(
    transaction: &mut ConfigTransaction,
    clear: bool,
    profile: &Value,
) -> Result<(), String> {
    let home = dirs::home_dir().ok_or_else(|| "无法定位用户主目录".to_string())?;
    let (normal_root, threep_root) = macos_config_roots(&home);
    if clear {
        let active = clear_at_root(transaction, &threep_root)?;
        if active && normal_root.join(CONFIG_FILE).exists() {
            update_deployment_mode(transaction, &normal_root.join(CONFIG_FILE), "1p")?;
        }
        return Ok(());
    }
    update_deployment_mode(transaction, &normal_root.join(CONFIG_FILE), "3p")?;
    write_at_root(transaction, &threep_root, profile)
}

#[cfg(any(target_os = "macos", test))]
fn macos_config_roots(home: &Path) -> (PathBuf, PathBuf) {
    let application_support = home.join("Library").join("Application Support");
    (
        application_support.join("Claude"),
        application_support.join("Claude-3p"),
    )
}

fn write_at_root(
    transaction: &mut ConfigTransaction,
    root: &Path,
    profile: &Value,
) -> Result<(), String> {
    let config_path = root.join(CONFIG_FILE);
    let profile_path = root
        .join("configLibrary")
        .join(format!("{PROFILE_ID}.json"));
    let meta_path = root.join("configLibrary").join("_meta.json");
    let mut merged = read_object_or_empty(transaction, &profile_path)?;
    merge_connection_profile(&mut merged, profile)?;
    update_deployment_mode(transaction, &config_path, "3p")?;
    transaction.write_json(&profile_path, &merged)?;
    update_meta(transaction, &meta_path, true)
}

fn clear_at_root(transaction: &mut ConfigTransaction, root: &Path) -> Result<bool, String> {
    let config_path = root.join(CONFIG_FILE);
    let profile_path = root
        .join("configLibrary")
        .join(format!("{PROFILE_ID}.json"));
    let meta_path = root.join("configLibrary").join("_meta.json");
    if !config_path.exists() && !profile_path.exists() && !meta_path.exists() {
        return Ok(false);
    }
    let meta = read_object_or_empty(transaction, &meta_path)?;
    let active = meta.get("appliedId").and_then(Value::as_str) == Some(PROFILE_ID);
    if active {
        update_deployment_mode(transaction, &config_path, "1p")?;
    }
    if profile_path.exists() {
        let mut profile = read_object_or_empty(transaction, &profile_path)?;
        let object = profile.as_object_mut().expect("validated profile object");
        for key in CONNECTION_KEYS {
            if key != "inferenceModels" {
                object.remove(key);
            }
        }
        if object.is_empty() {
            transaction.remove(&profile_path)?;
        } else {
            transaction.write_json(&profile_path, &profile)?;
        }
    }
    update_meta(transaction, &meta_path, false)?;
    Ok(active)
}

fn update_deployment_mode(
    transaction: &mut ConfigTransaction,
    path: &Path,
    mode: &str,
) -> Result<(), String> {
    let mut value = read_object_or_empty(transaction, path)?;
    value["deploymentMode"] = Value::String(mode.to_string());
    transaction.write_json(path, &value)
}

fn update_meta(
    transaction: &mut ConfigTransaction,
    path: &Path,
    enabled: bool,
) -> Result<(), String> {
    let mut value = read_object_or_empty(transaction, path)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "Claude Desktop _meta.json 必须是 JSON 对象".to_string())?;
    let entries = object
        .entry("entries")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| "Claude Desktop _meta.json 的 entries 必须是数组".to_string())?;
    if enabled {
        if !entries
            .iter()
            .any(|entry| entry.get("id").and_then(Value::as_str) == Some(PROFILE_ID))
        {
            entries.push(json!({ "id": PROFILE_ID, "name": PROFILE_NAME }));
        }
        object.insert(
            "appliedId".to_string(),
            Value::String(PROFILE_ID.to_string()),
        );
    } else {
        entries.retain(|entry| entry.get("id").and_then(Value::as_str) != Some(PROFILE_ID));
        if object.get("appliedId").and_then(Value::as_str) == Some(PROFILE_ID) {
            object.remove("appliedId");
        }
    }
    transaction.write_json(path, &value)
}

fn read_object_or_empty(transaction: &mut ConfigTransaction, path: &Path) -> Result<Value, String> {
    let value = transaction.read_json(path)?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(format!("{} 必须是 JSON 对象", path.display()))
    }
}

const CONNECTION_KEYS: [&str; 5] = [
    "inferenceGatewayApiKey",
    "inferenceGatewayAuthScheme",
    "inferenceGatewayBaseUrl",
    "inferenceProvider",
    "inferenceModels",
];

fn merge_connection_profile(existing: &mut Value, incoming: &Value) -> Result<(), String> {
    let object = existing
        .as_object_mut()
        .ok_or("Claude Desktop profile 必须是 JSON 对象")?;
    for key in CONNECTION_KEYS {
        if key != "inferenceModels" {
            object.insert(key.into(), incoming[key].clone());
        }
    }
    let mut models = match object.get("inferenceModels") {
        None => Vec::new(),
        Some(Value::Array(models)) => models.clone(),
        Some(_) => return Err("Claude Desktop inferenceModels 必须是数组".into()),
    };
    for model in incoming["inferenceModels"]
        .as_array()
        .ok_or("缺少托管模型配置")?
    {
        if let Some(old) = models
            .iter_mut()
            .find(|old| old.get("name") == model.get("name"))
        {
            let old = old
                .as_object_mut()
                .ok_or("Claude Desktop 模型配置必须是 JSON 对象")?;
            // Labels and context capability belong to the selected connection;
            // all other model preferences belong to the user.
            old.insert("labelOverride".into(), model["labelOverride"].clone());
            if let Some(supports) = model.get("supports1m") {
                old.insert("supports1m".into(), supports.clone());
            } else {
                old.remove("supports1m");
            }
        } else {
            models.push(model.clone());
        }
    }
    object.insert("inferenceModels".into(), Value::Array(models));
    Ok(())
}

fn gateway_profile(context_window: u64) -> Value {
    let supports_1m = context_window >= 1_000_000;
    let model = |name: &str, label: &str| {
        let mut value = json!({ "name": name, "labelOverride": label });
        if supports_1m {
            value["supports1m"] = Value::Bool(true);
        }
        value
    };
    json!({
        "inferenceGatewayApiKey": PROXY_TOKEN,
        "inferenceGatewayAuthScheme": "bearer",
        "inferenceGatewayBaseUrl": format!("http://127.0.0.1:15722{PROXY_ROUTE}"),
        "inferenceProvider": "gateway",
        "inferenceModels": [
            model("claude-haiku-4-5", "gpt-5.6-luna"),
            model("claude-sonnet-5", "gpt-5.6-sol"),
            model("claude-opus-5", "gpt-5.6-sol")
        ]
    })
}

fn provider_gateway_profile(provider: &crate::models::ProviderProfile) -> Value {
    let context_window = crate::third_party_apps::provider_context_window(provider);
    let supports_1m = context_window >= 1_000_000;
    let model = |name: &str| {
        let mut value = json!({ "name": name, "labelOverride": provider.model });
        if supports_1m {
            value["supports1m"] = Value::Bool(true);
        }
        value
    };
    json!({
        "inferenceGatewayApiKey": PROXY_TOKEN,
        "inferenceGatewayAuthScheme": "bearer",
        "inferenceGatewayBaseUrl": format!("http://127.0.0.1:15722{PROXY_ROUTE}"),
        "inferenceProvider": "gateway",
        "inferenceModels": [
            model("claude-haiku-4-5"),
            model("claude-sonnet-5"),
            model("claude-opus-5")
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn read_value(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn desktop_update_preserves_restrictions_preferences_and_other_profiles() {
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("configLibrary");
        fs::create_dir(&library).unwrap();
        let profile_path = library.join(format!("{PROFILE_ID}.json"));
        fs::write(&profile_path, serde_json::to_vec(&json!({
            "coworkEgressAllowedHosts": ["docs.example.test"],
            "disableDeploymentModeChooser": false,
            "privacy": {"analytics": false},
            "inferenceModels": [
                {"name":"claude-sonnet-5", "labelOverride":"old", "supports1m":true, "thinkingBudget":4096},
                {"name":"user-model", "labelOverride":"keep"}
            ]
        })).unwrap()).unwrap();
        fs::write(
            root.path().join(CONFIG_FILE),
            b"{\"deploymentMode\":\"1p\",\"mcpServers\":{\"keep\":{}}}",
        )
        .unwrap();
        fs::write(
            library.join("_meta.json"),
            serde_json::to_vec(&json!({
                "entries":[{"id":"other","name":"Other"}], "appliedId":"other", "preference":"keep"
            }))
            .unwrap(),
        )
        .unwrap();
        let mut transaction = ConfigTransaction::default();
        write_at_root(&mut transaction, root.path(), &gateway_profile(272_000)).unwrap();
        transaction.commit().unwrap();
        let profile = read_value(&profile_path);
        assert_eq!(
            profile["coworkEgressAllowedHosts"],
            json!(["docs.example.test"])
        );
        assert_eq!(profile["disableDeploymentModeChooser"], false);
        assert_eq!(profile["privacy"], json!({"analytics":false}));
        let sonnet = profile["inferenceModels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == "claude-sonnet-5")
            .unwrap();
        assert_eq!(sonnet["labelOverride"], "gpt-5.6-sol");
        assert_eq!(sonnet["thinkingBudget"], 4096);
        assert!(sonnet.get("supports1m").is_none());
        assert!(profile["inferenceModels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == "user-model"));
        assert_eq!(
            read_value(&root.path().join(CONFIG_FILE))["mcpServers"],
            json!({"keep":{}})
        );
        let meta = read_value(&library.join("_meta.json"));
        assert_eq!(meta["entries"][0]["id"], "other");
        assert_eq!(meta["appliedId"], PROFILE_ID);
        assert_eq!(meta["preference"], "keep");
    }

    #[test]
    fn new_desktop_profile_does_not_relax_network_or_hide_mode_choice() {
        let root = tempfile::tempdir().unwrap();
        let mut transaction = ConfigTransaction::default();
        write_at_root(&mut transaction, root.path(), &gateway_profile(1_000_000)).unwrap();
        transaction.commit().unwrap();
        let profile = read_value(
            &root
                .path()
                .join("configLibrary")
                .join(format!("{PROFILE_ID}.json")),
        );
        assert!(profile.get("coworkEgressAllowedHosts").is_none());
        assert!(profile.get("disableDeploymentModeChooser").is_none());
    }

    #[test]
    fn malformed_later_file_leaves_original_desktop_config_untouched() {
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("configLibrary");
        fs::create_dir(&library).unwrap();
        let config = root.path().join(CONFIG_FILE);
        let original = b"{ \"deploymentMode\": \"1p\", \"keep\": true }\n";
        fs::write(&config, original).unwrap();
        fs::write(library.join("_meta.json"), b"{\"entries\":false}").unwrap();
        let mut transaction = ConfigTransaction::default();
        assert!(write_at_root(&mut transaction, root.path(), &gateway_profile(1_000_000)).is_err());
        assert_eq!(fs::read(config).unwrap(), original);
        assert!(!library.join(format!("{PROFILE_ID}.json")).exists());
    }

    #[test]
    fn clearing_inactive_profile_keeps_other_selection_and_user_fields() {
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("configLibrary");
        fs::create_dir(&library).unwrap();
        let profile = library.join(format!("{PROFILE_ID}.json"));
        fs::write(
            &profile,
            b"{\"inferenceGatewayApiKey\":\"managed\",\"privacy\":{\"analytics\":false}}",
        )
        .unwrap();
        fs::write(
            root.path().join(CONFIG_FILE),
            b"{\"deploymentMode\":\"3p\"}",
        )
        .unwrap();
        fs::write(
            library.join("_meta.json"),
            serde_json::to_vec(
                &json!({"appliedId":"other","entries":[{"id":PROFILE_ID},{"id":"other"}]}),
            )
            .unwrap(),
        )
        .unwrap();
        let mut transaction = ConfigTransaction::default();
        assert!(!clear_at_root(&mut transaction, root.path()).unwrap());
        transaction.commit().unwrap();
        assert_eq!(read_value(&profile), json!({"privacy":{"analytics":false}}));
        assert_eq!(
            read_value(&root.path().join(CONFIG_FILE))["deploymentMode"],
            "3p"
        );
        assert_eq!(
            read_value(&library.join("_meta.json"))["appliedId"],
            "other"
        );
    }

    #[test]
    fn desktop_gateway_profile_uses_the_local_proxy_route() {
        let profile = gateway_profile(1_000_000);
        assert_eq!(
            profile["inferenceGatewayBaseUrl"],
            "http://127.0.0.1:15722/claude-desktop"
        );
        assert_eq!(profile["inferenceGatewayAuthScheme"], "bearer");
        assert_eq!(profile["inferenceGatewayApiKey"], "PROXY_MANAGED");
        assert_eq!(
            profile["inferenceModels"][1]["labelOverride"],
            "gpt-5.6-sol"
        );
        assert!(profile["inferenceModels"]
            .as_array()
            .expect("models")
            .iter()
            .all(|model| model.get("supports1m") == Some(&Value::Bool(true))));
        let compact_profile = gateway_profile(272_000);
        assert!(compact_profile["inferenceModels"]
            .as_array()
            .expect("models")
            .iter()
            .all(|model| model.get("supports1m").is_none()));
    }

    #[test]
    fn macos_roots_match_claude_physical_directories() {
        let home = PathBuf::from("/Users/tester");
        let (normal, threep) = macos_config_roots(&home);
        assert_eq!(
            normal,
            home.join("Library")
                .join("Application Support")
                .join("Claude")
        );
        assert_eq!(
            threep,
            home.join("Library")
                .join("Application Support")
                .join("Claude-3p")
        );
    }

    #[test]
    fn windows_login_root_uses_roaming_app_data() {
        let roaming = PathBuf::from(r"C:\Users\tester\AppData\Roaming");
        assert_eq!(windows_login_root(&roaming), roaming.join("Claude"));
    }

    #[test]
    fn provider_profile_uses_the_selected_provider_model() {
        let provider = crate::third_party_apps::tests::provider();
        let profile = provider_gateway_profile(&provider);
        assert_eq!(profile["inferenceModels"][0]["labelOverride"], "test-model");
    }
}
