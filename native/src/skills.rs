//! Skill catalog and detail contracts consumed by the original React workbench.

use crate::{router::ApiError, settings, workspace};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

fn error(status: u16, detail: impl Into<String>) -> ApiError { ApiError { status, detail: detail.into() } }

fn skill_dir(name: &str) -> Result<PathBuf, ApiError> {
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\', ':']) || name.chars().any(char::is_control) {
        return Err(error(400, "技能名称无效"));
    }
    let root = workspace::ensure_ready().map_err(|cause| error(500, cause))?.join("skills/builtin");
    for candidate in [name.to_string(), format!("skill-{name}")] {
        let path = root.join(candidate);
        if path.join("SKILL.md").is_file() {
            let actual = path.canonicalize().map_err(|cause| error(500, cause.to_string()))?;
            let root = root.canonicalize().map_err(|cause| error(500, cause.to_string()))?;
            if actual.parent() != Some(root.as_path()) { return Err(error(403, "非法路径")); }
            return Ok(path);
        }
    }
    Err(error(404, format!("SKILL '{name}' 不存在")))
}

fn parse(text: &str) -> (String, String, String) {
    let normalized = text.trim_start_matches('\u{feff}');
    let lines = normalized.lines().collect::<Vec<_>>();
    if lines.first().map(|line| line.trim()) != Some("---") { return (String::new(), String::new(), normalized.to_string()); }
    let Some(end) = lines.iter().enumerate().skip(1).find(|(_, line)| line.trim() == "---").map(|(index, _)| index) else {
        return (String::new(), String::new(), normalized.to_string());
    };
    let mut description = String::new();
    let mut layer = String::new();
    let mut index = 1;
    while index < end {
        let line = lines[index].trim();
        if let Some(value) = line.strip_prefix("layer:") { layer = value.trim().trim_matches(['\'', '"']).to_string(); }
        if let Some(value) = line.strip_prefix("description:") {
            let value = value.trim();
            if value.starts_with(['>', '|']) {
                index += 1;
                let mut block = Vec::new();
                while index < end && (lines[index].starts_with(char::is_whitespace) || lines[index].trim().is_empty()) {
                    if !lines[index].trim().is_empty() { block.push(lines[index].trim()); }
                    index += 1;
                }
                description = block.join(" ");
                continue;
            }
            description = value.trim_matches(['\'', '"']).to_string();
        }
        index += 1;
    }
    (description, layer, lines[end + 1..].join("\n").trim().to_string())
}

fn model_slot(name: &str) -> Option<&'static str> {
    match name {
        "ai-image-gen" | "ecom-details-image" | "short-drama" => Some("images"),
        "ai-video-gen" => Some("video"),
        "voice-clone" => Some("tts"),
        "ai-music" => Some("music"),
        _ => None,
    }
}

fn api_status(name: &str) -> (bool, bool) {
    let Some(slot) = model_slot(name) else { return (name == "paper-explainer", true); };
    let configured = settings::gateway().map(|config| {
        !config["apiKey"].as_str().unwrap_or("").trim().is_empty()
            && !config["endpoints"][slot].as_str().unwrap_or("").trim().is_empty()
    }).unwrap_or(false);
    (true, configured)
}

fn api_spec(name: &str, configured: bool) -> Value {
    let Some(slot) = model_slot(name) else { return Value::Null; };
    let key = match slot { "images" => "IMG", "video" => "VIDEO", "tts" => "VOICE", _ => "MUSIC" };
    let config = settings::gateway().unwrap_or(Value::Null);
    let base = config["baseUrl"].as_str().and_then(|base| settings::gateway_root(base).ok()).map(|base| format!("{base}/v1")).unwrap_or_default();
    let model = config["endpoints"][slot].as_str().unwrap_or("");
    json!({
        "label": "ai-switch 网关",
        "settings": [],
        "providers": [{
            "id": "ai-switch", "name": "OTools ai-switch 网关", "keys": [
                {"env":format!("{key}_API_KEY"),"label":"网关 Key","required":true,"secret":true,"configured":!config["apiKey"].as_str().unwrap_or("").is_empty(),"masked":if config["apiKey"].as_str().unwrap_or("").is_empty(){""}else{"«已配置»"},"choices":[]},
                {"env":format!("{key}_BASE_URL"),"label":"网关地址","required":true,"secret":false,"configured":!base.is_empty(),"masked":base,"choices":[]},
                {"env":format!("{key}_MODEL"),"label":"模型","required":true,"secret":false,"configured":configured,"masked":model,"choices":[]}
            ]
        }]
    })
}

fn metadata(path: &Path, detail: bool) -> Result<Value, ApiError> {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let raw = fs::read_to_string(path.join("SKILL.md")).map_err(|cause| error(500, cause.to_string()))?;
    let (description, layer, body) = parse(&raw);
    let (needs_api, configured) = api_status(&name);
    let mut value = json!({"name":name,"description":description,"layer":layer,"needsApi":needs_api,"apiConfigured":configured});
    if detail { value["body"] = json!(body); value["apiSpec"] = if needs_api { api_spec(&name, configured) } else { Value::Null }; }
    Ok(value)
}

pub fn list() -> Result<Value, ApiError> {
    let root = workspace::ensure_ready().map_err(|cause| error(500, cause))?.join("skills/builtin");
    if !root.is_dir() { return Ok(json!([])); }
    let mut paths = fs::read_dir(&root).map_err(|cause| error(500, cause.to_string()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) && entry.path().join("SKILL.md").is_file())
        .map(|entry| entry.path()).collect::<Vec<_>>();
    paths.sort();
    let skills = paths.iter().map(|path| metadata(path, false)).collect::<Result<Vec<_>, _>>()?;
    Ok(json!(skills))
}

pub fn detail(name: &str) -> Result<Value, ApiError> {
    metadata(&skill_dir(name)?, true)
}
