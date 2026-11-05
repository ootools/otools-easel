//! Codex and ai-switch settings with the original React settings contracts.

use crate::{agent, chat_routes, process, router::ApiError, workspace};
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

pub fn host_config_path() -> PathBuf { home().join(".otools/config.json") }

pub fn host_config() -> Value {
    fs::read_to_string(host_config_path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_else(|| json!({}))
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

pub fn codex_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("OTOOLS_CODEX_PATH") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    process::executable("codex").or_else(|| {
        let mut known = vec![home().join(".codex/bin/codex")];
        #[cfg(target_os = "macos")]
        { known.push("/Applications/ChatGPT.app/Contents/Resources/codex".into()); }
        known.into_iter().find(|path| path.is_file())
    })
}

pub fn harness_get() -> SettingsResult {
    let mut config = harness_config();
    config["locked"] = json!(runtime_override().is_some());
    config["agents"] = json!([
        {"id":"api","label":"仅聊天模式","protocol":"模型 API 直连","installed":true,"install_hint":"在 OTools 设置中配置 ai-switch 网关"},
        {"id":"codex","label":"Codex","protocol":"app-server","installed":codex_binary().is_some(),"install_hint":"安装 @openai/codex，或设置 OTOOLS_CODEX_PATH"}
    ]);
    Ok(config)
}

pub fn harness_save(body: &Value) -> SettingsResult {
    let _guard = CONFIG_LOCK.lock().map_err(|_| error(500, "配置状态不可用"))?;
    let runtime = body["runtime"].as_str().ok_or_else(|| error(422, "缺少执行引擎"))?;
    if !matches!(runtime, "api" | "codex") { return Err(error(400, "当前 OTools 移植仅提供 Codex 与仅聊天模式")); }
    let sandbox = body["sandbox"].as_str().unwrap_or("workspace-write");
    if !matches!(sandbox, "read-only" | "workspace-write") { return Err(error(400, "请选择只读或工作区写入权限")); }
    let model = body.get("model").map(|value| value.as_str().ok_or_else(|| error(422, "模型名称必须为字符串"))).transpose()?.unwrap_or("");
    if model.chars().count() > 240 || model.chars().any(char::is_control) { return Err(error(400, "模型名称无效")); }
    if runtime_override().is_some() { return Err(error(409, "启动环境固定了底座，请修改 EASEL_AGENT_RUNTIME 后重启")); }
    if chat_routes::has_running_jobs() { return Err(error(409, "请等待当前任务完成或停止后切换底座")); }
    if runtime == "codex" && codex_binary().is_none() { return Err(error(409, "请先安装对应 Agent CLI，再切换")); }
    let approve = body.get("codex_auto_approve").filter(|value| !value.is_null()).map(|value| value.as_bool().ok_or_else(|| error(422, "授权选项必须为布尔值"))).transpose()?;
    let mut config = harness_config();
    config["runtime"] = json!(runtime);
    config["sandbox"] = json!(sandbox);
    config["models"][runtime] = json!(model.trim());
    if runtime == "codex" { if let Some(approve) = approve { config["codex_auto_approve"] = json!(approve); } }
    write_atomic(&harness_file(), &serde_json::to_vec_pretty(&config).map_err(|cause| error(500, cause.to_string()))?)?;
    chat_routes::close_idle_sessions();
    harness_get()
}

pub fn gateway_root(base: &str) -> Result<String, ApiError> {
    let base = base.trim().trim_end_matches('/');
    let base = base.strip_suffix("/v1").unwrap_or(base);
    let parsed = reqwest::Url::parse(base).map_err(|_| error(400, "网关地址无效"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none()
        || !parsed.username().is_empty() || parsed.password().is_some() || parsed.port() == Some(0)
        || parsed.query().is_some() || parsed.fragment().is_some() || base.chars().any(|character| character.is_control() || character.is_whitespace()) || base.contains('\\')
    { return Err(error(400, "网关地址无效")); }
    Ok(base.to_string())
}

pub fn gateway() -> Option<Value> {
    let config = host_config();
    let value = config.get("ai_switch")?.clone();
    gateway_root(value["baseUrl"].as_str()?).ok()?;
    Some(value)
}

pub fn bootstrap() -> SettingsResult {
    let runtime = harness_config()["runtime"].as_str().unwrap_or("codex").to_string();
    let configured = if runtime == "codex" { codex_binary().is_some() } else {
        gateway().map(|config| !config["apiKey"].as_str().unwrap_or("").trim().is_empty() && !config["endpoints"]["text"].as_str().unwrap_or("").trim().is_empty()).unwrap_or(false)
    };
    let warnings = fs::read_to_string(workspace::project_root().join("outputs/_install/last-install.json")).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|document| document.get("warnings").cloned()).unwrap_or_else(|| json!([]));
    Ok(json!({"modelConfigured":configured,"envKeyOk":configured,"routeOk":configured,"routeDetail":if runtime == "codex" {"CLI 检测结果不代表认证成功；请发送消息验证模型可用性"} else {"ai-switch 网关配置状态；尚未执行模型请求"},"gatewayUp":configured,"openclawConfigExists":false,"installWarnings":warnings,"runtime":runtime,"authVerified":false}))
}

fn masked(key: &str) -> String {
    if key.trim().is_empty() { String::new() } else { "«已配置»".to_string() }
}

fn slot_for(channel: &str) -> Result<&'static str, ApiError> {
    match channel { "chat" => Ok("text"), "image" => Ok("images"), "video" => Ok("video"), "speech" => Ok("tts"), "transcribe" => Ok("stt"), "embeddings" => Ok("embeddings"), _ => Err(error(400, "未知模型通道")) }
}

pub fn model_channels() -> SettingsResult {
    let config = gateway().unwrap_or_else(|| json!({"baseUrl":"http://127.0.0.1:19527","apiKey":"","endpoints":{}}));
    let root = gateway_root(config["baseUrl"].as_str().unwrap_or("http://127.0.0.1:19527"))?;
    let key = config["apiKey"].as_str().unwrap_or("");
    let mut channels = Map::new();
    for channel in ["chat", "transcribe", "image", "video", "speech", "embeddings"] {
        let slot = slot_for(channel)?;
        let model = config["endpoints"][slot].as_str().unwrap_or("");
        channels.insert(channel.into(), json!({"rows":[{
            "slot":"ai-switch","order":1,"name":"ai-switch 网关","sub":"OTools 宿主配置","type":"openai","protocol":"openai","model":model,"baseUrl":format!("{root}/v1"),"keyMasked":masked(key),"role":"主","result":if key.is_empty() || model.is_empty() {"缺配置"} else {"已配置"},"modelEditable":true,"baseEditable":true,"baseOptional":false,"deletable":false
        }]}));
    }
    channels.insert("music".into(), json!({"rows":[]}));
    Ok(json!({"transport":"api","channels":channels,"primary":config["endpoints"]["text"].as_str().unwrap_or("")}))
}

pub fn model_save(body: &Value) -> SettingsResult {
    let channel = body["channel"].as_str().ok_or_else(|| error(422, "缺少模型通道"))?;
    let slot = slot_for(channel)?;
    let rows = body["rows"].as_array().ok_or_else(|| error(422, "缺少模型配置行"))?;
    if rows.len() != 1 || rows[0]["slot"] != "ai-switch" { return Err(error(400, "请通过 ai-switch 网关配置此通道，上游提供商由网关管理")); }
    let row = &rows[0];
    let model = row["model"].as_str().ok_or_else(|| error(422, "模型名称必须为字符串"))?.trim();
    if model.chars().count() > 240 || model.chars().any(char::is_control) { return Err(error(400, "模型名称无效")); }
    let _guard = CONFIG_LOCK.lock().map_err(|_| error(500, "配置状态不可用"))?;
    let mut config = host_config();
    if !config.is_object() { return Err(error(409, "宿主配置格式无效")); }
    let existing = config.get("ai_switch").cloned().unwrap_or_else(|| json!({"baseUrl":"","apiKey":"","endpoints":{}}));
    let base = row["baseUrl"].as_str().unwrap_or(existing["baseUrl"].as_str().unwrap_or(""));
    let root = gateway_root(base)?;
    let new_key = row["key"].as_str().unwrap_or("").trim();
    let old_key = existing["apiKey"].as_str().unwrap_or("");
    if new_key.chars().any(char::is_control) { return Err(error(400, "Key 不能包含控制字符")); }
    if new_key.is_empty() && !old_key.is_empty() && gateway_root(existing["baseUrl"].as_str().unwrap_or("")).ok().as_deref() != Some(&root) { return Err(error(400, "更换网关地址时必须重新填写 Key")); }
    if !config["ai_switch"].is_object() { config["ai_switch"] = json!({"endpoints":{}}); }
    config["ai_switch"]["baseUrl"] = json!(root);
    config["ai_switch"]["apiKey"] = json!(if new_key.is_empty() {old_key} else {new_key});
    if !config["ai_switch"]["endpoints"].is_object() { config["ai_switch"]["endpoints"] = json!({}); }
    config["ai_switch"]["endpoints"][slot] = json!(model);
    if channel == "chat" { config["ai_settings"] = json!({"provider":"openai","baseUrl":format!("{root}/v1"),"apiKey":config["ai_switch"]["apiKey"],"model":model}); }
    write_atomic(&host_config_path(), &serde_json::to_vec_pretty(&config).map_err(|cause| error(500, cause.to_string()))?)?;
    let mut response = model_channels()?;
    response["ok"] = json!(true);
    response["note"] = json!("已保存到 OTools ai-switch 网关配置；下一轮生效");
    Ok(response)
}

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

pub fn available_models(body: &Value) -> SettingsResult {
    let config = gateway().ok_or_else(|| error(409, "请先在 OTools 配置 ai-switch 网关"))?;
    let stored_root = gateway_root(config["baseUrl"].as_str().unwrap_or(""))?;
    let requested = body["baseUrl"].as_str().unwrap_or("").trim();
    let root = if requested.is_empty() { stored_root.clone() } else { gateway_root(requested)? };
    let explicit = body["key"].as_str().unwrap_or("").trim();
    if root != stored_root { return Err(error(400, "模型发现必须对接已配置的 ai-switch 网关；先保存新地址与 Key")); }
    let key = if explicit.is_empty() { config["apiKey"].as_str().unwrap_or("") } else { explicit };
    if key.is_empty() { return Err(error(400, "网关 Key 未配置")); }
    let value = crate::runtime().block_on(async {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).redirect(reqwest::redirect::Policy::none()).build().map_err(|cause| error(500, cause.to_string()))?;
        let response = client.get(format!("{root}/v1/models")).bearer_auth(key).send().await.map_err(|cause| error(502, cause.without_url().to_string()))?;
        if !response.status().is_success() { return Err(error(response.status().as_u16(), "网关模型列表请求失败")); }
        let bytes = response.bytes().await.map_err(|_| error(502, "网关响应读取失败"))?;
        if bytes.len() > 2 * 1024 * 1024 { return Err(error(502, "模型列表响应过大")); }
        serde_json::from_slice::<Value>(&bytes).map_err(|_| error(502, "模型列表响应不是有效 JSON"))
    })?;
    let models: Vec<String> = value["data"].as_array().ok_or_else(|| error(502, "模型列表缺少 data"))?.iter().filter_map(|row| row["id"].as_str().map(str::to_string)).collect();
    Ok(json!({"baseUrl":format!("{root}/v1"),"models":models,"fetchedAt":chrono::Utc::now().timestamp()}))
}

pub fn selftest(body: &Value) -> SettingsResult {
    let channel = body["channel"].as_str().unwrap_or("chat");
    let slot = slot_for(channel)?;
    let mut results = Vec::new();
    if let Some(config) = gateway() {
        let root = gateway_root(config["baseUrl"].as_str().unwrap_or(""))?;
        let key = config["apiKey"].as_str().unwrap_or("");
        let model = config["endpoints"][slot].as_str().unwrap_or("");
        if !key.is_empty() && !model.is_empty() {
            let start = Instant::now();
            let probe: Result<(u16, bool, String), ApiError> = crate::runtime().block_on(async {
                let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).redirect(reqwest::redirect::Policy::none()).build().map_err(|cause| error(500, cause.to_string()))?;
                let response = client.get(format!("{root}/v1/models")).bearer_auth(key).send().await.map_err(|cause| error(502, cause.without_url().to_string()))?;
                let status = response.status();
                let success = status.is_success();
                let body = if success { String::new() } else { response.text().await.unwrap_or_default().chars().take(120).collect() };
                Ok((status.as_u16(), success, body))
            });
            let ms = start.elapsed().as_millis() as u64;
            match probe {
                Ok((_, true, _)) => results.push(json!({
                    "baseUrl": format!("{root}/v1"), "ok": true, "ms": ms,
                    "detail": format!("模型列表探测成功；未执行生成调用（模型 {model}）")
                })),
                Ok((status, false, body)) => results.push(json!({
                    "baseUrl": format!("{root}/v1"), "ok": false, "ms": ms,
                    "detail": format!("网关返回 {status}：{body}")
                })),
                Err(cause) => results.push(json!({
                    "baseUrl": format!("{root}/v1"), "ok": false, "ms": ms, "detail": cause.detail
                })),
            }
        }
    }
    Ok(json!({"channel":channel,"results":results,"testedAt":chrono::Utc::now().timestamp()}))
}

pub fn harness_models(runtime: &str) -> SettingsResult {
    if runtime != "codex" { return Err(error(400, "此底座不支持原生模型列表")); }
    if gateway().is_some() {
        let value = available_models(&json!({}))?;
        let models: Vec<Value> = value["models"].as_array().unwrap().iter().map(|id| json!({"id":id,"label":id,"default":false})).collect();
        return Ok(json!({"runtime":"codex","models":models}));
    }
    let binary = codex_binary().ok_or_else(|| error(409, "Codex CLI 未安装"))?;
    let value = crate::runtime().block_on(async {
        let (mut server, events) = agent::AppServer::spawn(agent::SpawnOptions { binary:binary.to_string_lossy().into_owned(), cwd:workspace::project_root(), env:HashMap::new(), args:vec!["app-server".into()] }).map_err(|cause| error(409, cause.to_string()))?;
        let result = tokio::time::timeout(Duration::from_secs(25), async {
            server.ensure_initialized().await.map_err(|cause| error(409, cause.to_string()))?;
            server.model_list().await.map_err(|cause| error(409, cause.to_string()))
        }).await.map_err(|_| error(504, "原生模型列表读取超时"));
        server.terminate().await;
        drop(events);
        result?
    })?;
    let rows = value.get("data").or_else(|| value.get("models")).and_then(Value::as_array).cloned().unwrap_or_default();
    let models: Vec<Value> = rows.iter().filter_map(|row| {
        let id = row.get("model").or_else(|| row.get("id"))?.as_str()?;
        Some(json!({"id":id,"label":row.get("displayName").or_else(|| row.get("name")).and_then(Value::as_str).unwrap_or(id),"default":row["isDefault"].as_bool().unwrap_or(false)}))
    }).collect();
    Ok(json!({"runtime":"codex","models":models}))
}
