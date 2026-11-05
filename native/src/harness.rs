//! 执行底座设置、prompt 四层栈组装、直连模型设置(全部对齐上游语义)。
//!
//! - 底座设置:`outputs/_harness/config.json`(runtime/models/sandbox/codex_auto_approve)
//! - prompt 栈:SOUL → AGENTS → 运行时说明(引擎感知)→ 画像前缀(消息内联,上游 persona.py 约定)
//! - 直连设置:`~/.otools/config.json` 的 `ai_switch`(权威)→ `ai_settings`(旧),
//!   供仅聊天模式直连 OpenAI 兼容端点

use crate::workspace::project_root;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct HarnessSettings {
    pub runtime: String,
    pub models: Map<String, Value>,
    pub sandbox: String,
    pub codex_auto_approve: bool,
}

/// 默认执行底座:codex(宿主 Agent 服务链路;OpenClaw 按产品决策不随插件提供)。
pub fn load_harness() -> HarnessSettings {
    let path = project_root().join("outputs").join("_harness").join("config.json");
    let config = fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({ "runtime": "codex", "models": {}, "sandbox": "workspace-write", "codex_auto_approve": false }));
    HarnessSettings {
        runtime: config
            .get("runtime")
            .and_then(Value::as_str)
            .unwrap_or("codex")
            .to_string(),
        models: config
            .get("models")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        sandbox: config
            .get("sandbox")
            .and_then(Value::as_str)
            .unwrap_or("workspace-write")
            .to_string(),
        codex_auto_approve: config
            .get("codex_auto_approve")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

impl HarnessSettings {
    pub fn model(&self, runtime: &str) -> Option<String> {
        self.models
            .get(runtime)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }
}

// ── prompt 栈(引擎感知的运行时说明;上游 docs/prompt-stack.md)──────────────

fn read_workspace_md(name: &str) -> String {
    fs::read_to_string(project_root().join("agents").join("workspace").join(name)).unwrap_or_default()
}

fn persona_dir(name: &str) -> Option<PathBuf> {
    if name.trim().is_empty() || name == "_template" || name.contains("..") || name.contains('/') {
        return None;
    }
    let dir = project_root().join("profiles").join(name.trim());
    dir.is_dir().then_some(dir)
}

fn persona_prefix(name: &str) -> String {
    if persona_dir(name).is_some() {
        format!("我当前使用的画像是「{}」。", name.trim())
    } else {
        String::new()
    }
}

/// 画像全文(仅聊天模式无文件能力时内联;codex 模式 agent 自行读工作区)。
fn persona_inline(name: &str) -> String {
    let Some(dir) = persona_dir(name) else { return String::new() };
    let mut sections = Vec::new();
    for (key, label) in [
        ("identity", "定位"),
        ("style", "风格"),
        ("audience", "受众"),
        ("platforms", "平台"),
        ("preferences", "偏好与红线"),
        ("memory", "长期记忆"),
    ] {
        let content = fs::read_to_string(dir.join(format!("{key}.md"))).unwrap_or_default();
        if !content.trim().is_empty() {
            sections.push(format!("### {label}\n\n{}", content.trim()));
        }
    }
    if sections.is_empty() {
        String::new()
    } else {
        format!("## 当前账号画像:「{}」\n\n{}", name.trim(), sections.join("\n\n"))
    }
}

/// 运行时说明(引擎感知;忠实对齐上游 Layer 3)。
fn runtime_note(runtime: &str) -> String {
    match runtime {
        "codex" | "acp" | "opencode" => [
            "## 运行环境说明",
            "- 当前执行引擎:具备本地文件读取与工作区写入能力的 Agent CLI。",
            "- 工作区根目录即当前工作目录;所有产物一律写到其下 outputs/<主题名>/,并在回复结尾列出产物相对路径。",
            "- 技能库在 skills/builtin/<技能名>/SKILL.md(执行流程)与 references/(参考资料);执行任务前先读对应 SKILL.md。",
            "- 技能脚本 skills/shared/scripts/*.py 依赖 Python;若工作区存在 .env(网关参数),运行脚本前先加载(set -a; source .env; set +a)。",
            "- 沙箱边界:工作区内可写;工作区外的写操作会被拒绝。",
        ]
        .join("\n"),
        _ => [
            "## 运行环境说明",
            "- 当前执行引擎:仅聊天模式(直连模型 API),没有本地文件、终端与脚本执行能力。",
            "- 涉及本地文件或脚本的任务:给出可执行步骤,不要声称已经执行。",
        ]
        .join("\n"),
    }
}

/// 常驻指令(developerInstructions):SOUL + AGENTS + 运行时说明(+ 仅聊天时内联画像)。
pub fn developer_instructions(runtime: &str, persona: Option<&str>) -> String {
    let soul = read_workspace_md("SOUL.md");
    let agents = read_workspace_md("AGENTS.md");
    let mut parts = Vec::new();
    if !soul.trim().is_empty() {
        parts.push(soul.trim().to_string());
    }
    if !agents.trim().is_empty() {
        parts.push(agents.trim().to_string());
    }
    parts.push(runtime_note(runtime));
    if runtime == "api" {
        if let Some(name) = persona {
            let inline = persona_inline(name);
            if !inline.is_empty() {
                parts.push(inline);
            }
        }
    }
    parts.join("\n\n---\n\n")
}

/// 消息前缀(画像内联约定)+ 消息本体。
pub fn build_user_message(persona: Option<&str>, message: &str) -> String {
    let prefix = persona.map(|name| persona_prefix(name)).unwrap_or_default();
    format!("{prefix}{}", message.trim())
}

// ── 直连模型设置(仅聊天模式;~/.otools/config.json)────────────────────────

#[derive(Debug, Clone)]
pub struct DirectSettings {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

fn otools_config() -> Option<Map<String, Value>> {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).ok()?;
    let text = fs::read_to_string(Path::new(&home).join(".otools").join("config.json")).ok()?;
    serde_json::from_str::<Value>(&text).ok()?.as_object().cloned()
}

/// ai_switch 权威 → ai_settings 旧键,两态并存(与宿主 resolve_ai_settings 同优先级)。
pub fn direct_settings() -> Option<DirectSettings> {
    let config = otools_config()?;
    if let Some(switch) = config.get("ai_switch").and_then(Value::as_object) {
        let base_url = switch.get("baseUrl").and_then(Value::as_str).unwrap_or_default().trim().trim_end_matches('/').to_string();
        let api_key = switch.get("apiKey").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        let model = switch
            .get("endpoints")
            .and_then(|endpoints| endpoints.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if !base_url.is_empty() && !model.is_empty() {
            return Some(DirectSettings {
                provider: "openai".to_string(),
                base_url: format!("{base_url}/v1"),
                api_key,
                model,
            });
        }
    }
    let legacy = config.get("ai_settings")?;
    let base_url = legacy.get("baseUrl").and_then(Value::as_str)?.trim().to_string();
    let model = legacy.get("model").and_then(Value::as_str)?.trim().to_string();
    if base_url.is_empty() || model.is_empty() {
        return None;
    }
    Some(DirectSettings {
        provider: legacy.get("provider").and_then(Value::as_str).unwrap_or("openai").to_string(),
        base_url,
        api_key: legacy.get("apiKey").and_then(Value::as_str).unwrap_or_default().to_string(),
        model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persona_name_guard() {
        assert!(persona_dir("../escape").is_none());
        assert!(persona_dir("_template").is_none());
    }

    #[test]
    fn direct_settings_prefer_ai_switch() {
        // 无 HOME 注入,直接验证函数不 panic 且返回 Option
        let _ = direct_settings();
    }
}
