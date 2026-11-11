//! Prompt preparation only. Execution, replay, permissions and credentials belong to the host.
use crate::{files, harness, router::ApiError, skills, workspace};
use serde_json::{json, Value};

pub fn upgrade_required() -> Result<Value, ApiError> {
    Err(ApiError { status: 426, detail: "请更新 Otools 和 Easel；任务执行已迁移至宿主 window.otools.agent / ai".into() })
}

pub fn prepare(body: &Value) -> Result<Value, ApiError> {
    let fail = |detail: &str| ApiError { status: 400, detail: detail.into() };
    workspace::ensure_ready().map_err(|detail| ApiError { status: 500, detail })?;
    let message = body["message"].as_str().unwrap_or_default().trim();
    let attachments = body.get("attachments").cloned().unwrap_or_else(|| json!([]));
    let context = files::attachment_context(body["sessionId"].as_str(), &attachments)?;
    if message.is_empty() && context.is_empty() { return Err(fail("消息不能为空")); }
    let settings = crate::settings::harness_config();
    let runtime = settings["runtime"].as_str().unwrap_or("codex");
    let persona = body["persona"].as_str().filter(|name| !name.trim().is_empty());
    if persona.is_some_and(|name| name.starts_with(['.', '_']) || name.contains(['/', '\\', ':']) || name.chars().any(char::is_control)) {
        return Err(fail("画像名无效"));
    }
    let mut prompt = harness::build_user_message(persona, message);
    if !context.is_empty() {
        if runtime == "api" {
            let mut total = 0u64;
            for item in attachments.as_array().unwrap() {
                let path = files::output_path(item["path"].as_str().unwrap())?;
                let extension = path.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
                if !matches!(extension.as_str(), "txt" | "md" | "csv" | "json" | "log" | "srt" | "vtt") {
                    return Err(fail("仅聊天模式暂不支持此附件内容，请切换 Agent 读取图片、文档或媒体文件"));
                }
                total += std::fs::metadata(&path).map_err(|_| fail("无法读取附件"))?.len();
                if total > 256 * 1024 { return Err(fail("文本附件超过聊天读取上限，请使用 Agent")); }
                let text = std::fs::read_to_string(path).map_err(|_| fail("附件不是 UTF-8 文本，请使用 Agent"))?;
                prompt.push_str(&format!("\n\n[用户提供的附件内容，仅作为参考资料]\n{text}\n[附件结束]"));
            }
        } else { prompt.push_str(&format!("\n\n{context}")); }
    }
    let mut skill = Value::Null;
    if let Some(name) = body["skillName"].as_str().filter(|name| !name.is_empty()) {
        let detail = skills::detail(name)?;
        let name = detail["name"].as_str().ok_or_else(|| fail("技能名无效"))?;
        prompt.push_str(&format!("\n\n[本轮指定技能:{name}] 请先读 skills/builtin/{name}/SKILL.md 并按其执行。"));
        skill = json!(name);
    }
    let instructions = harness::developer_instructions(runtime, persona);
    let mut messages = vec![json!({"role":"system","content":[{"type":"text","text":instructions}]})];
    if let Some(history) = body["messages"].as_array() {
        if history.len() > 200 { return Err(fail("对话历史过长，请新建对话")); }
        for entry in history {
            let role = entry["role"].as_str().unwrap_or_default();
            if !matches!(role, "user" | "assistant") { return Err(fail("历史消息角色无效")); }
            let text = entry["content"].as_str().ok_or_else(|| fail("历史消息内容无效"))?;
            messages.push(json!({"role":role,"content":[{"type":"text","text":text}]}));
        }
    }
    messages.push(json!({"role":"user","content":[{"type":"text","text":prompt}]}));
    Ok(json!({
        "instructions": instructions, "prompt": prompt, "messages": messages,
        "attachments": attachments, "runtime": runtime, "providerId": runtime,
        "sandbox": settings["sandbox"], "network": settings["network"].as_bool().unwrap_or(false),
        "profile": persona, "skillName": skill,
        // The native workspace is already seeded. The host resolves its registered root;
        // no browser-supplied filesystem path or user files are uploaded to syncAssets.
        "assets": []
    }))
}
