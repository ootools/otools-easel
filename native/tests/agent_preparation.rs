mod support;
use otools_easel_native::router::route;
use serde_json::{json, Value};
use support::TestWorkspace;

#[test]
fn prepare_preserves_context_and_resolves_skill_alias_without_starting_execution() {
    let workspace = TestWorkspace::new("agent-prepare");
    workspace.write("agents/workspace/SOUL.md", "Be accurate");
    workspace.write("profiles/Writer/identity.md", "Technical writer");
    workspace.write("skills/builtin/skill-persona-check/SKILL.md", "# Review persona");
    let result = route("POST", "/api/agent/prepare", &json!({
        "message":"Review this", "persona":"Writer", "skillName":"persona-check", "sessionId":"chat-1",
        "messages":[{"role":"user","content":"Earlier question"},{"role":"assistant","content":"Earlier answer"}]
    })).unwrap();
    assert_eq!(result["runtime"], "codex");
    assert_eq!(result["sandbox"], "workspace-write");
    assert!(result["instructions"].as_str().unwrap().contains("Be accurate"));
    assert!(result["prompt"].as_str().unwrap().contains("skills/builtin/skill-persona-check/SKILL.md"));
    assert!(result["prompt"].as_str().unwrap().contains("Writer"));
    assert_eq!(result["messages"][1]["content"][0]["text"], "Earlier question");
    assert!(!workspace.root.join("outputs/_jobs").exists());
    assert!(route("POST", "/api/agent/prepare", &json!({"message":"x","skillName":"../escape"})).is_err());
}

#[test]
fn legacy_execution_routes_require_upgrade_and_never_enqueue() {
    let workspace = TestWorkspace::new("agent-upgrade");
    for path in ["/api/chat/stream", "/api/skill", "/api/chat/stop", "/api/chat/question/answer"] {
        assert_eq!(route("POST", path, &json!({"message":"hello","name":"test"})).unwrap_err().status, 426);
    }
    assert!(!workspace.root.join("outputs/_jobs").exists());
}

#[test]
fn profile_baseline_is_available_before_enhancement_and_run_link_survives_polling() {
    let workspace = TestWorkspace::new("profile-host");
    let built = route("POST", "/api/profile/build", &json!({"name":"Writer","form":{"direction":"Science"}})).unwrap();
    assert_eq!(built["created"], true);
    assert!(workspace.root.join("profiles/Writer/identity.md").is_file());
    let path = "/api/profile/build/status/Writer";
    assert_eq!(route("GET", path, &Value::Null).unwrap()["state"], "pending");
    let linked = route("PUT", path, &json!({"runId":"host-run","sessionId":"host-session","hostStatus":"running"})).unwrap();
    assert_eq!(linked["state"], "running");
    assert_eq!(route("GET", path, &Value::Null).unwrap()["runId"], "host-run");
    assert!(route("PUT", path, &json!({"runId":"other","hostStatus":"completed"})).is_err());
    assert_eq!(route("PUT", path, &json!({"runId":"host-run","hostStatus":"completed","log":"Finished"})).unwrap()["state"], "done");
    assert!(route("PUT", "/api/profile/build/status/..%2Foutside", &json!({"hostStatus":"completed"})).is_err());
}

#[test]
fn plain_chat_inlines_text_attachments_and_refuses_unreadable_binary_content() {
    let workspace = TestWorkspace::new("plain-attachment");
    workspace.write("outputs/_harness/config.json", r#"{"runtime":"api"}"#);
    let uploaded = route("POST", "/api/upload", &json!({"fields":{"sessionId":"text"},"files":[{"name":"notes.txt","contentBase64":"SGVsbG8gZmlsZQ=="}]})).unwrap();
    let prepared = route("POST", "/api/agent/prepare", &json!({"message":"Summarize","sessionId":"text","attachments":uploaded["files"]})).unwrap();
    assert!(prepared["prompt"].as_str().unwrap().contains("Hello file"));
    let image = route("POST", "/api/upload", &json!({"fields":{"sessionId":"image"},"files":[{"name":"photo.png","contentBase64":"aGk="}]})).unwrap();
    assert_eq!(route("POST", "/api/agent/prepare", &json!({"message":"Analyze","sessionId":"image","attachments":image["files"]})).unwrap_err().status, 400);
}
