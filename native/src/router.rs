//! `/api/*` 路由器:逐路由复刻上游 `web/app.py`(FastAPI)语义。
//!
//! 契约:前端 lib/api.ts 的 request/streamChat 全部经 dispatch('api',
//! {method, path, body}) 到这里;错误返回 (status, detail),桥拆成
//! throw Error(detail),与 HTTP 非 2xx 行为一致。
//!
//! 首批覆盖:status / personas CRUD / skills / outputs / harness 设置;
//! chat、trends、calendar/ideas、publishing、accounts 逐批跟进(见
//! reference/app.py 路由清单)。

use crate::chat_routes;
use crate::environment;
use crate::files;
use crate::settings;
use crate::trends;
use crate::storage;
use crate::workspace::project_root;
use serde_json::{json, Value};
use std::fs;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub detail: String,
}

impl ApiError {
    fn new(status: u16, detail: impl Into<String>) -> Self {
        Self { status, detail: detail.into() }
    }
}

type ApiResult = Result<Value, ApiError>;

fn ioerr(error: std::io::Error) -> ApiError {
    ApiError::new(500, error.to_string())
}

/// 路由分发入口(逐路径对齐 app.py)。
pub fn route(method: &str, path: &str, body: &Value) -> ApiResult {
    let method = method.to_ascii_uppercase();
    let (path, query) = match path.split_once('?') {
        Some((path, query)) => (path, query),
        None => (path, ""),
    };
    let decoded = path.trim_matches('/').split('/')
        .map(|segment| percent_encoding::percent_decode_str(segment).decode_utf8()
            .map(|segment| segment.into_owned())
            .map_err(|_| ApiError::new(400, "路径编码无效")))
        .collect::<Result<Vec<_>, _>>()?;
    let segments: Vec<&str> = decoded.iter().map(String::as_str).collect();
    let query_map = parse_query(query);

    match (method.as_str(), segments.as_slice()) {
        ("GET", ["api", "status"]) => status(),
        ("GET", ["api", "personas"]) => personas_list(),
        ("GET", ["api", "persona", name]) => persona_detail(name),
        ("GET", ["api", "persona", name, "files"]) => persona_files(name),
        ("PUT", ["api", "persona", name, "file"]) => persona_file_save(name, body),
        ("DELETE", ["api", "persona", name]) => persona_delete(name),
        ("GET", ["api", "skills"]) => crate::skills::list(),
        ("GET", ["api", "skill", name]) => crate::skills::detail(name),
        ("GET", ["api", "outputs"]) => files::outputs_tree(),
        ("GET", ["api", "output", rest @ ..]) => files::output_content(&rest.join("/")),
        ("DELETE", ["api", "output", rest @ ..]) => files::output_delete(&rest.join("/")),
        ("GET", ["api", "media", rest @ ..]) => files::media(
            &rest.join("/"),
            query_map.get("offset").map(|value| value.parse::<u64>().map_err(|_| ApiError::new(400, "读取位置无效"))).transpose()?,
            query_map.get("length").map(|value| value.parse::<usize>().map_err(|_| ApiError::new(400, "读取长度无效"))).transpose()?,
        ),
        ("GET", ["api", "upload", "limits"]) => Ok(json!({"ok":true,"max_mb":50})),
        ("POST", ["api", "upload", "start"]) => files::upload_start(body),
        ("POST", ["api", "upload", "chunk"]) => files::upload_chunk(body),
        ("POST", ["api", "upload", "finish"]) => files::upload_finish(body),
        ("POST", ["api", "upload", "abort"]) => files::upload_abort(body),
        ("POST", ["api", "upload"]) => files::upload(body, false),
        ("POST", ["api", "upload", "local"]) => files::upload(body, true),
        ("GET", ["api", "chat", "jobs", turn_id, "events"]) => {
            let after = query_map.get("after").and_then(|value| value.parse::<u64>().ok()).unwrap_or(0);
            chat_routes::job_events(turn_id, after).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("GET", ["api", "chat", "last", session_key]) => {
            chat_routes::chat_last(session_key).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("POST", ["api", "chat", "stream"]) => {
            chat_routes::chat_stream(body).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("POST", ["api", "chat", "stop"]) => {
            chat_routes::chat_stop(body).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("POST", ["api", "chat", "question", "answer"]) => {
            chat_routes::question_answer(body).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("POST", ["api", "chat", "question", "status"]) => {
            chat_routes::question_status(body).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("POST", ["api", "skill"]) => {
            chat_routes::skill_run(body).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("GET", ["api", "trends"]) => Ok(trends::trends_list(
            query_map.get("platforms").map(String::as_str),
            query_map.get("limit").map(String::as_str),
        )),
        ("GET", ["api", "trends", "sources"]) => Ok(trends::trend_sources()),
        ("GET", ["api", "trends", "manage"]) => {
            trends::trend_manage_get().map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("POST", ["api", "trends", "manage"]) => {
            trends::trend_manage_save(body).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("GET", ["api", "schedule"]) => Ok(json!(storage::schedule_list())),
        ("POST", ["api", "schedule"]) => {
            storage::schedule_create(body).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("PUT", ["api", "schedule", sid]) => {
            storage::schedule_update(sid, body).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("DELETE", ["api", "schedule", sid]) => {
            storage::schedule_delete(sid).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("GET", ["api", "schedule", "context"]) => {
            let days = query_map.get("days").and_then(|value| value.parse::<u32>().ok()).unwrap_or(14);
            Ok(storage::schedule_context(days))
        }
        ("GET", ["api", "ideas"]) => Ok(json!(storage::ideas_list())),
        ("POST", ["api", "ideas"]) => {
            storage::ideas_create(body).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("PUT", ["api", "ideas", iid]) => {
            storage::ideas_update(iid, body).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("DELETE", ["api", "ideas", iid]) => {
            storage::ideas_delete(iid).map_err(|(status, detail)| ApiError::new(status, detail))
        }
        ("GET", ["api", "env", "tools"]) => environment::tools(query_map.get("refresh").map(|value| value == "1" || value == "true").unwrap_or(false)),
        ("POST", ["api", "env", "install"]) => environment::start_install(body),
        ("GET", ["api", "env", "job", id]) => environment::job_status(id),
        ("GET", ["api", "settings", "bootstrap"]) => settings::bootstrap(),
        ("GET", ["api", "settings", "harness"]) => settings::harness_get(),
        ("POST", ["api", "settings", "harness"]) => settings::harness_save(body),
        ("POST", ["api", "settings", "harness", "models", runtime]) => settings::harness_models(runtime),
        ("GET", ["api", "settings", "models"]) => settings::model_channels(),
        ("POST", ["api", "settings", "models", "save"]) => settings::model_save(body),
        ("POST", ["api", "settings", "models", "available"]) => settings::available_models(body),
        ("POST", ["api", "settings", "models", "selftest"]) => settings::selftest(body),
        ("GET", ["api", "settings", "proxy"]) => settings::proxy_get(),
        ("POST", ["api", "settings", "proxy"]) => settings::proxy_save(body),
        // 待移植批次:chat*/trends*/schedule/ideas/publishing/accounts/analytics/upload/env…
        _ => Err(ApiError::new(
            501,
            format!("该接口尚未移植:{method} {path}(逐批跟进,见 reference/app.py)"),
        )),
    }
}

// ── 工具 ────────────────────────────────────────────────────────────────────

fn parse_query(query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            let decode = |text: &str| percent_encoding::percent_decode_str(&text.replace('+', " "))
                .decode_utf8_lossy().into_owned();
            map.insert(decode(key), decode(value));
        }
    }
    map
}




/// 画像目录名校验(上游 `_valid_persona_name` 同款语义:禁路径穿越与非法字符)。
fn valid_persona_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['.', '_'])
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(|character| character.is_control())
}

// ── GET /api/status ─────────────────────────────────────────────────────────

fn status() -> ApiResult {
    let bootstrap = settings::bootstrap()?;
    Ok(json!({
        "gateway": bootstrap["modelConfigured"],
        "transport": if bootstrap["runtime"] == "api" { "api" } else { "codex" },
        "runtime": bootstrap["runtime"],
        "skills": crate::skills::list()?,
        "personas": personas_list()?,
    }))
}

// ── personas CRUD(app.py:1442-1514)────────────────────────────────────────

fn personas_dir() -> PathBuf {
    project_root().join("profiles")
}

fn personas_list() -> ApiResult {
    let dir = personas_dir();
    let mut items = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !valid_persona_name(&name) || !entry.file_type().map_err(ioerr)?.is_dir() {
                continue;
            }
            let description = fs::read_to_string(entry.path().join("identity.md"))
                .unwrap_or_default()
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("<!--"))
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect::<String>();
            items.push(json!({ "name": name, "description": description }));
        }
    }
    items.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    Ok(json!(items))
}

fn persona_directory(name: &str) -> Result<PathBuf, ApiError> {
    if !valid_persona_name(name) {
        return Err(ApiError::new(400, "画像名非法"));
    }
    let root = personas_dir().canonicalize().map_err(|_| ApiError::new(404, "画像不存在"))?;
    let dir = root.join(name).canonicalize().map_err(|_| ApiError::new(404, "画像不存在"))?;
    if dir.parent() != Some(root.as_path()) || !dir.is_dir() {
        return Err(ApiError::new(403, "非法路径"));
    }
    Ok(dir)
}

fn persona_detail(name: &str) -> ApiResult {
    let files = persona_files(name)?;
    let parts = files["files"].as_array().unwrap().iter()
        .filter_map(|file| file["content"].as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Err(ApiError::new(404, "画像不存在"));
    }
    Ok(json!({ "name": name, "content": parts.join("\n\n---\n\n") }))
}

fn persona_files(name: &str) -> ApiResult {
    let dir = persona_directory(name)?;
    let mut ordered = vec!["identity.md", "style.md", "audience.md", "platforms.md", "preferences.md", "memory.md"]
        .into_iter().map(str::to_string).collect::<Vec<_>>();
    let mut extra = fs::read_dir(&dir).map_err(ioerr)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !ordered.contains(name))
        .collect::<Vec<_>>();
    extra.sort();
    ordered.extend(extra);
    let mut files = Vec::new();
    for filename in ordered {
        let path = dir.join(&filename);
        if path.exists() && !path.canonicalize().map_err(ioerr)?.starts_with(&dir) {
            return Err(ApiError::new(403, "非法路径"));
        }
        files.push(json!({
            "filename": filename,
            "content": if path.is_file() { fs::read_to_string(path).map_err(ioerr)? } else { String::new() },
        }));
    }
    Ok(json!({ "name": name, "files": files }))
}

fn persona_file_save(name: &str, body: &Value) -> ApiResult {
    let dir = persona_directory(name)?;
    let filename = body.get("filename").and_then(Value::as_str)
        .ok_or_else(|| ApiError::new(422, "缺少 filename"))?;
    let content = body.get("content").and_then(Value::as_str)
        .ok_or_else(|| ApiError::new(422, "缺少 content"))?;
    if !filename.ends_with(".md") || filename.starts_with('.') || filename.contains(['/', '\\', ':'])
        || filename.chars().any(|character| character.is_control())
    {
        return Err(ApiError::new(400, "文件名非法"));
    }
    let path = dir.join(filename);
    if fs::symlink_metadata(&path).map(|meta| meta.file_type().is_symlink()).unwrap_or(false) {
        return Err(ApiError::new(403, "非法路径"));
    }
    let temp = dir.join(format!(".{filename}.{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temp, content).map_err(ioerr)?;
    if let Err(cause) = fs::rename(&temp, &path) {
        let _ = fs::remove_file(temp);
        return Err(ioerr(cause));
    }
    Ok(json!({ "ok": true, "filename": filename }))
}

fn persona_delete(name: &str) -> ApiResult {
    if !valid_persona_name(name) {
        return Err(ApiError::new(400, "非法画像名称"));
    }
    let dir = personas_dir().join(name);
    if !dir.is_dir() {
        return Err(ApiError::new(404, format!("画像不存在:{name}")));
    }
    fs::remove_dir_all(&dir).map_err(ioerr)?;
    Ok(json!({ "ok": true, "deleted": name }))
}

// ── skills(app.py:1516-1540;frontmatter 解析与上游一致)────────────────────




// ── harness 设置(app.py:1870-1906;outputs/_harness/config.json)────────────





#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    fn temp_root(tag: &str) -> PathBuf {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!("easel-router-{tag}-{}", std::process::id()));
        std::env::set_var("OTOOLS_EASEL_ROOT", &dir);
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn personas_crud_roundtrip() {
        temp_root("personas");
        // bootstrap 从种子目录拉 _template(测试环境无 dist → 手工建)
        let dir = personas_dir();
        fs::create_dir_all(dir.join("_template")).unwrap();
        for key in ["identity", "style", "audience", "platforms", "preferences", "memory"] {
            fs::write(dir.join("_template").join(format!("{key}.md")), format!("# {key}")).unwrap();
        }
        fs::create_dir_all(dir.join("科技数码达人")).unwrap();
        for key in ["identity", "memory"] {
            fs::write(dir.join("科技数码达人").join(format!("{key}.md")), "内容").unwrap();
        }

        let list = personas_list().unwrap();
        assert_eq!(list[0]["name"], "科技数码达人");

        let files = persona_files("科技数码达人").unwrap();
        assert_eq!(files["files"][0]["content"], "内容");
        assert_eq!(files["files"][1]["content"], "");

        persona_file_save("科技数码达人", &json!({"filename": "style.md", "content": "新风格"})).unwrap();
        let files = persona_files("科技数码达人").unwrap();
        assert_eq!(files["files"][1]["content"], "新风格");

        assert!(persona_file_save("科技数码达人", &json!({"filename": "../evil.md", "content": ""})).is_err());
        persona_delete("科技数码达人").unwrap();
        assert!(persona_files("科技数码达人").is_err());
    }

    #[test]
    fn persona_name_guard_rejects_traversal_and_templates() {
        assert!(valid_persona_name("科技达人"));
        assert!(!valid_persona_name("../escape"));
        assert!(!valid_persona_name("_template"));
        assert!(!valid_persona_name(""));
    }
}
