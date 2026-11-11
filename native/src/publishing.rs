//! Accounts registry, login markers, profile build, session delete, artifact archive.
//! Browser automation (Playwright) stays in the Python skill scripts; Rust only orchestrates.

use crate::files::output_path;
use crate::router::ApiError;
use crate::workspace;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};


fn error(status: u16, detail: impl Into<String>) -> ApiError { ApiError { status, detail: detail.into() } }

// ── 账号注册表（上游 LOGIN_RUNNERS 同款）────────────────────────────────────

const ACCOUNTS: &[(&str, &str, &str)] = &[
    ("xiaohongshu", "小红书", "xhs"),
    ("kuaishou", "快手", "web"),
    ("weixin-channels", "微信视频号", "web"),
    ("zhihu", "知乎", "web"),
    ("juejin", "掘金", "web"),
    ("twitter-x", "推特 X", "web"),
    ("bilibili", "B站", "biliup"),
    ("douyin", "抖音", "douyin"),
    ("toutiao", "今日头条", "toutiao"),
    ("wechat-oa", "微信公众号", "wechat-oa"),
];

fn login_dir() -> PathBuf { workspace::project_root().join("outputs/_login") }

fn login_marker(platform: &str) -> Value {
    let path = login_dir().join(format!("{platform}.json"));
    fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_else(|| json!({"state":"unknown","message":""}))
}

fn logged_in(platform: &str, backend: &str) -> bool {
    match backend {
        "biliup" => workspace::project_root().join("cookies.json").is_file(),
        _ => login_marker(platform)["state"].as_str() == Some("success"),
    }
}

pub fn accounts() -> Value {
    let items: Vec<Value> = ACCOUNTS.iter().map(|(platform, name, backend)| json!({
        "platform": platform, "name": name, "backend": backend,
        "supported": *backend != "unsupported",
        "loggedIn": logged_in(platform, backend),
        "note": "",
    })).collect();
    json!(items)
}

pub fn login_status(platform: &str) -> Result<Value, ApiError> {
    if !ACCOUNTS.iter().any(|(pf, _, _)| *pf == platform) { return Err(error(404, "未知平台")); }
    let mut data = login_marker(platform);
    let state = data["state"].as_str().unwrap_or("unknown").to_string();
    let mut qr = String::new();
    let mut qr_ts = 0u64;
    if matches!(state.as_str(), "qr_ready" | "window_login") {
        let png = login_dir().join(format!("{platform}.png"));
        if png.is_file() {
            qr = format!("_login/{platform}.png");
            qr_ts = fs::metadata(&png).ok().and_then(|meta| meta.modified().ok())
                .and_then(|date| date.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|date| date.as_millis() as u64).unwrap_or(0);
        }
    }
    data["qr"] = json!(qr);
    data["qrTs"] = json!(qr_ts);
    Ok(data)
}

// ── session 删除 ────────────────────────────────────────────────────────────

pub fn delete_session(key: &str) -> Result<Value, ApiError> {
    if key.trim().is_empty() || key.contains("..") || key.contains('/') || key.contains('\\') {
        return Err(error(400, "非法会话标识"));
    }
    let dir = workspace::project_root().join("sessions").join(key);
    let mut deleted = false;
    if dir.is_dir() {
        fs::remove_dir_all(&dir).map_err(|cause| error(500, cause.to_string()))?;
        deleted = true;
    }
    let job_file = workspace::project_root().join("outputs/_jobs").join(format!("{key}.jsonl"));
    if job_file.is_file() { let _ = fs::remove_file(&job_file); }
    if deleted { Ok(json!({"deleted": true })) } else { Ok(json!({"deleted": false, "reason": "session not found"})) }
}

// ── profile build（基线确定性写入 + 后台 agent 增强）────────────────────────

fn valid_new_persona(name: &str) -> Result<(), ApiError> {
    if name.trim().is_empty() { return Err(error(400, "画像名不能为空")); }
    if name.contains(['/', '\\']) { return Err(error(400, "画像名不能包含 / 或 \\ 字符")); }
    if name.chars().any(char::is_control) || name.contains(':') || name.starts_with(['.', '_']) { return Err(error(400, "画像名不能以 . 或 _ 开头")); }
    Ok(())
}

fn form_text(form: &Value, key: &str, default: &str) -> String {
    match form.get(key) {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("、"),
        Some(Value::String(text)) if !text.trim().is_empty() => text.trim().to_string(),
        _ => default.to_string(),
    }
}

fn write_baseline(root: &PathBuf, name: &str, form: &Value) {
    let dir = root.join("profiles").join(name);
    fs::create_dir_all(&dir).unwrap();
    let g = |key: &str, default: &str| form_text(form, key, default);
    let direction = { let v = g("direction", ""); if v.is_empty() { "[待补充]".into() } else { v } };
    let reason = { let v = g("reason", ""); if v.is_empty() { "[待补充]".into() } else { v } };
    let goal = g("goal", "");
    let formats = g("formats", "");
    let tone = { let v = g("tone", ""); if v.is_empty() { "[待分析]".into() } else { v } };
    let likes = g("likes", "");
    let avoid = g("avoid", "");
    let platforms = form.get("platforms").and_then(Value::as_array).map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()).unwrap_or_default();
    let links = form.get("links").and_then(Value::as_object).cloned().unwrap_or_default();

    fs::write(dir.join("identity.md"), format!("# 身份定位\n\n## 我是谁\n\n{direction}\n\n## 差异化\n\n{reason}\n\n## 内容方向\n\n{direction}{}{}\n",
        if formats.is_empty() { String::new() } else { format!("（形式：{formats}）") },
        if goal.is_empty() { String::new() } else { format!("\n运营目标：{goal}") })).unwrap();
    fs::write(dir.join("style.md"), format!("# 内容风格\n\n## 语气\n\n{tone}\n\n## 开头结构\n\n[待 AI 分析已发内容]\n\n## 视觉风格\n\n[待 AI 分析]\n\n## 内容节奏\n\n{}{}\n\n## 标志性元素\n\n[待 AI 分析]\n",
        if formats.is_empty() { "[待补充]".to_string() } else { formats },
        "")).unwrap();
    fs::write(dir.join("audience.md"), "# 目标受众\n\n## 核心人群\n\n[待 AI 分析/待补充]\n\n## 兴趣标签\n\n[待补充]\n\n## 痛点\n\n[待补充]\n\n## 互动特征\n\n[待 AI 分析已发内容]\n").unwrap();
    let plat_lines: Vec<String> = platforms.iter().map(|p| {
        let url = links.get(p).and_then(Value::as_str).unwrap_or("[待补充]");
        format!("## {p}\n\n主页：{url}\n粉丝量级 / 内容形式：[待补充]\n")
    }).collect();
    fs::write(dir.join("platforms.md"), format!("# 平台运营\n\n{}\n", if plat_lines.is_empty() { "[待补充]".to_string() } else { plat_lines.join("\n") })).unwrap();
    fs::write(dir.join("preferences.md"), format!("# 偏好与红线\n\n## 要做的\n\n{direction}\n\n## 不做的\n\n{}{}\n\n## 合规底线\n\n{}{}\n",
        avoid, if avoid.is_empty() { "[待补充]" } else { "" }, avoid, if avoid.is_empty() { "[待补充]" } else { "" })).unwrap();
    fs::write(dir.join("memory.md"), format!("# 经验沉淀\n\n## 内容洞察\n\n{}\n\n## 踩过的坑\n\n[待积累]\n",
        if likes.is_empty() { "[待 AI 分析已收藏/点赞]".to_string() } else { format!("喜欢的内容/对标：{likes}") })).unwrap();
}

fn profile_status_path(name: &str) -> PathBuf {
    workspace::project_root().join("outputs/_profile_build").join(format!("{name}.json"))
}

fn write_profile_status(name: &str, payload: &Value) -> Result<(), ApiError> {
    let path = profile_status_path(name);
    fs::create_dir_all(path.parent().unwrap()).map_err(|cause| error(500, cause.to_string()))?;
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temp, payload.to_string()).map_err(|cause| error(500, cause.to_string()))?;
    fs::rename(temp, path).map_err(|cause| error(500, cause.to_string()))
}

pub fn profile_build(body: &Value) -> Result<Value, ApiError> {
    let name = body["name"].as_str().unwrap_or_default().trim().to_string();
    valid_new_persona(&name)?;
    let root = workspace::ensure_ready().map_err(|cause| error(500, cause))?;
    let dir = root.join("profiles").join(&name);
    if dir.exists() { return Err(error(409, format!("画像「{name}」已存在，请换一个名字"))); }
    let form = body.get("form").cloned().unwrap_or_else(|| json!({}));
    write_baseline(&root, &name, &form);
    let instruction = format!(
        "请执行 skill-profile-builder 完善已存在的画像「{name}」。用户已通过表单提供基线六维文件，请分析社媒链接（抓不到就标注[待补充]，勿臆造），润色补全各维度文件，并给出一句话完成度摘要。");
    let turn_id = uuid::Uuid::new_v4().to_string();
    let preparation = json!({"message":instruction,"persona":name,"skillName":"skill-profile-builder","sessionId":format!("profile-{turn_id}")});
    write_profile_status(&name, &json!({"state":"pending","log":"基线画像已可用，等待宿主增强任务启动","turnId":turn_id,"preparation":preparation}))?;
    Ok(json!({"created":true,"name":name,"async":true,"status":"pending","turnId":turn_id,"preparation":preparation}))
}

pub fn profile_build_status(name: &str) -> Result<Value, ApiError> {
    valid_new_persona(name)?;
    Ok(fs::read_to_string(profile_status_path(name)).ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({"state":"unknown","log":""})))
}

pub fn profile_build_update(name: &str, body: &Value) -> Result<Value, ApiError> {
    let mut status = profile_build_status(name)?;
    if status["state"] == "unknown" { return Err(error(404, "画像构建记录不存在")); }
    if let Some(id) = body["runId"].as_str() {
        if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) { return Err(error(400, "任务标识无效")); }
        if status["runId"].as_str().is_some_and(|existing| existing != id) { return Err(error(409, "画像已关联另一宿主任务")); }
        status["runId"] = json!(id);
    }
    if let Some(id) = body["sessionId"].as_str() { status["sessionId"] = json!(id); }
    let host_status = body["hostStatus"].as_str().unwrap_or("running");
    let state = match host_status {
        "completed" if status["runId"].is_string() => "done",
        "running" if status["runId"].is_string() => "running",
        "failed" | "cancelled" | "interrupted" => "failed",
        _ => return Err(error(400, "宿主任务状态无效")),
    };
    status["state"] = json!(state);
    status["hostStatus"] = json!(host_status);
    status["log"] = json!(body["log"].as_str().unwrap_or(if state == "done" { "AI 增强完成" } else if state == "failed" { "增强未完成；基线画像仍可用" } else { "宿主正在增强画像" }));
    write_profile_status(name, &status)?;
    Ok(status)
}

// ── artifacts archive（ZIP 索引）────────────────────────────────────────────

pub fn artifact_archive(path: &str) -> Result<Value, ApiError> {
    let full = output_path(path)?;
    if full.extension().and_then(|ext| ext.to_str()).unwrap_or_default().to_ascii_lowercase() != "zip" {
        return Err(error(400, "仅支持 ZIP 产物索引"));
    }
    let outputs = crate::files::outputs_root()?;
    let file = fs::File::open(&full).map_err(|cause| error(400, format!("无法读取压缩包索引：{cause}")))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|cause| error(400, format!("无法读取压缩包索引：{cause}")))?;
    if archive.len() > 1000 { return Err(error(400, "压缩包文件过多，暂不自动预览")); }
    let allowed: &[&str] = &["png","jpg","jpeg","gif","webp","svg","avif","bmp","ico","html","htm","mp4","webm","mov","m4v","mp3","wav","ogg","m4a","flac","pdf"];
    let mut artifacts = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else { continue };
        let name = entry.name().to_string();
        let parts: Vec<&str> = name.split('/').collect();
        if entry.is_dir() || name.starts_with('/') || name.contains('\\')
            || parts.iter().any(|part| part.is_empty() || part.starts_with(['.', '_']))
            || name.chars().any(|character| character.is_control())
            || !allowed.contains(&Path::new(&name).extension().and_then(|ext| ext.to_str()).unwrap_or_default().to_ascii_lowercase().as_str())
        { continue }
        let parent_dir = full.parent().unwrap().to_path_buf();
        let entry_path = PathBuf::from(&name);
        let Ok(relative) = entry_path.strip_prefix(&parent_dir) else { continue };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if fs::metadata(outputs.join(&relative)).map(|meta| meta.len() == entry.size()).unwrap_or(false)
            && seen.insert(relative.clone())
        {
            artifacts.push(relative);
            if artifacts.len() >= 100 { break; }
        }
    }
    Ok(json!({ "paths": artifacts }))
}
