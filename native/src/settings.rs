//! Codex and ai-switch settings with the original React settings contracts.

use crate::{router::ApiError, workspace};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static CONFIG_LOCK: Mutex<()> = Mutex::new(());

fn error(status: u16, detail: impl Into<String>) -> ApiError { ApiError { status, detail: detail.into() } }
pub type SettingsResult = Result<Value, ApiError>;

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_else(|| ".".into()))
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), ApiError> {
    let parent = path.parent().ok_or_else(|| error(500, "配置路径无效"))?;
    fs::create_dir_all(parent).map_err(|cause| error(500, cause.to_string()))?;
    let temp = parent.join(format!(".{}.{}.tmp", path.file_name().unwrap_or_default().to_string_lossy(), uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let result = (|| {
        use std::io::Write;
        let mut file = options.open(&temp).map_err(|cause| error(500, cause.to_string()))?;
        file.write_all(contents).map_err(|cause| error(500, cause.to_string()))?;
        file.sync_all().map_err(|cause| error(500, cause.to_string()))?;
        fs::rename(&temp, path).map_err(|cause| error(500, cause.to_string()))
    })();
    if result.is_err() { let _ = fs::remove_file(temp); }
    result
}

pub fn env_file() -> HashMap<String, String> {
    let text = fs::read_to_string(workspace::project_root().join(".env")).unwrap_or_default();
    let mut values = HashMap::new();
    for line in text.lines() {
        let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
        if line.starts_with('#') { continue }
        if let Some((key, value)) = line.split_once('=') {
            values.insert(key.trim().to_string(), value.trim().trim_matches(['\'', '"']).to_string());
        }
    }
    values
}

fn runtime_override() -> Option<String> {
    std::env::var("EASEL_AGENT_RUNTIME").ok().or_else(|| env_file().get("EASEL_AGENT_RUNTIME").cloned())
}

fn harness_file() -> PathBuf { workspace::project_root().join("outputs/_harness/config.json") }

pub fn harness_config() -> Value {
    let mut config = fs::read_to_string(harness_file()).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).unwrap_or_else(|| json!({}));
    if !config.is_object() { config = json!({}); }
    let runtime = runtime_override().unwrap_or_else(|| config["runtime"].as_str().unwrap_or("codex").to_string());
    config["runtime"] = json!(runtime);
    if !config["models"].is_object() { config["models"] = json!({}); }
    if !config["sandbox"].is_string() { config["sandbox"] = json!("workspace-write"); }
    if !config["codex_auto_approve"].is_boolean() { config["codex_auto_approve"] = json!(false); }
    config
}

pub fn harness_get() -> SettingsResult {
    let mut config = harness_config();
    config["locked"] = json!(runtime_override().is_some());
    config["agents"] = json!([]); // Host providers are merged by the frontend.
    config["codex_auto_approve"] = json!(false);
    Ok(config)
}

pub fn harness_save(body: &Value) -> SettingsResult {
    let _guard = CONFIG_LOCK.lock().map_err(|_| error(500, "配置状态不可用"))?;
    let runtime = body["runtime"].as_str().ok_or_else(|| error(422, "缺少执行引擎"))?;
    if runtime.is_empty() || runtime.len() > 128 || !runtime.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')) { return Err(error(400, "执行引擎标识无效")); }
    let sandbox = body["sandbox"].as_str().unwrap_or("workspace-write");
    if !matches!(sandbox, "read-only" | "workspace-write") { return Err(error(400, "请选择只读或工作区写入权限")); }
    let model = body.get("model").map(|value| value.as_str().ok_or_else(|| error(422, "模型名称必须为字符串"))).transpose()?.unwrap_or("");
    if model.chars().count() > 240 || model.chars().any(char::is_control) { return Err(error(400, "模型名称无效")); }
    if runtime_override().is_some() { return Err(error(409, "启动环境固定了底座，请修改 EASEL_AGENT_RUNTIME 后重启")); }
    let mut config = harness_config();
    config["runtime"] = json!(runtime);
    config["sandbox"] = json!(sandbox);
    config["models"][runtime] = json!(model.trim());
    config["codex_auto_approve"] = json!(false);
    write_atomic(&harness_file(), &serde_json::to_vec_pretty(&config).map_err(|cause| error(500, cause.to_string()))?)?;
    harness_get()
}

pub fn bootstrap() -> SettingsResult {
    Ok(json!({"modelConfigured":false,"envKeyOk":false,"routeOk":false,"routeDetail":"模型和 Agent 状态由 Otools 宿主提供","gatewayUp":false,"installWarnings":[],"runtime":harness_config()["runtime"],"authVerified":false}))
}

pub fn model_channels() -> SettingsResult { crate::chat_routes::upgrade_required() }
pub fn model_save(_body: &Value) -> SettingsResult { crate::chat_routes::upgrade_required() }

pub fn proxy_get() -> SettingsResult {
    let fixed = std::env::var("EASEL_PROXY").ok();
    let proxy = fixed.clone().or_else(|| env_file().get("EASEL_PROXY").cloned()).unwrap_or_default();
    Ok(json!({"proxy":proxy,"source":if fixed.is_some() {"env"} else if proxy.is_empty() {"none"} else {"envfile"}}))
}

pub fn proxy_save(body: &Value) -> SettingsResult {
    if std::env::var_os("EASEL_PROXY").is_some() { return Err(error(409, "启动环境固定了代理，请修改启动环境后重启")); }
    let proxy = body["proxy"].as_str().unwrap_or("").trim();
    if !proxy.is_empty() {
        let value = reqwest::Url::parse(proxy).map_err(|_| error(400, "代理地址无效"))?;
        if !matches!(value.scheme(), "http" | "https" | "socks5" | "socks5h") || value.host_str().is_none()
            || !value.username().is_empty() || value.password().is_some() || value.port() == Some(0)
            || value.query().is_some() || value.fragment().is_some() || !matches!(value.path(), "" | "/")
            || proxy.chars().any(|character| character.is_control() || character.is_whitespace()) || proxy.contains('\\')
        { return Err(error(400, "代理地址需为不含用户名密码的 HTTP(S) 或 SOCKS5 地址")); }
    }
    write_env(&HashMap::from([("EASEL_PROXY".to_string(), proxy.to_string())]))?;
    Ok(json!({"ok":true,"proxy":proxy,"source":if proxy.is_empty() {"none"} else {"envfile"}}))
}

pub fn write_env(updates: &HashMap<String, String>) -> Result<(), ApiError> {
    let _guard = CONFIG_LOCK.lock().map_err(|_| error(500, "配置状态不可用"))?;
    if updates.values().any(|value| value.contains(['\n', '\r', '\0'])) { return Err(error(400, "配置值不能包含换行或 NUL")); }
    let path = workspace::project_root().join(".env");
    let original = fs::read_to_string(&path).unwrap_or_default();
    let mut output = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in original.lines() {
        let key = line.trim().split_once('=').map(|(key, _)| key.trim().trim_start_matches("export "));
        if let Some(key) = key.filter(|key| updates.contains_key(*key) && !key.starts_with('#')) {
            seen.insert(key.to_string());
            if !updates[key].is_empty() { output.push(format!("{key}={}", updates[key])); }
        } else { output.push(line.to_string()); }
    }
    for (key, value) in updates { if !seen.contains(key) && !value.is_empty() { output.push(format!("{key}={value}")); } }
    write_atomic(&path, (output.join("\n") + "\n").as_bytes())
}

pub fn available_models(_body: &Value) -> SettingsResult { crate::chat_routes::upgrade_required() }
pub fn selftest(_body: &Value) -> SettingsResult { crate::chat_routes::upgrade_required() }

pub fn harness_models(_runtime: &str) -> SettingsResult {
    crate::chat_routes::upgrade_required()
}
