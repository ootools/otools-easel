//! Prompt stack: workspace instructions, runtime capabilities and profile context.

use crate::workspace::project_root;
use std::fs;
use std::path::PathBuf;

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
        runtime if runtime != "api" => [
            "## 运行环境说明",
            "- 当前执行引擎:具备本地文件读取与工作区写入能力的 Agent CLI。",
            "- 工作区根目录即当前工作目录;所有产物一律写到其下 outputs/<主题名>/,并在回复结尾列出产物相对路径。",
            "- 技能库在 skills/builtin/<技能名>/SKILL.md(执行流程)与 references/(参考资料);执行任务前先读对应 SKILL.md。",
            "- 媒体脚本所需的模型、地址和 Key 已由宿主注入本次任务环境；不要 source 工作区 .env 覆盖这些值，不要打印凭据。未选择的媒体模型不可自动回落。",
            "- 权限遵循宿主和所选引擎策略；不要假定工作区外可写，遇到授权请求等待用户确认。",
        ]
        .join("\n"),
        _ => [
            "## 运行环境说明",
            "- 当前执行引擎:仅聊天模式(宿主 AI 服务),没有本地文件、终端与脚本执行能力。",
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
