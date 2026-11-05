mod support;

use otools_easel_native::router::route;
use serde_json::{json, Value};
use support::TestWorkspace;

#[test]
fn harness_settings_preserve_model_map_and_reject_unsupported_or_unsafe_changes() {
    let workspace = TestWorkspace::new("harness");
    let current = route("GET", "/api/settings/harness", &Value::Null).unwrap();
    assert_eq!(current["runtime"], "codex");
    assert!(current["agents"].as_array().unwrap().iter().any(|agent| agent["id"] == "codex"));
    assert!(!current["agents"].as_array().unwrap().iter().any(|agent| agent["id"] == "openclaw"));
    let saved = route("POST", "/api/settings/harness", &json!({"runtime":"api","model":" text-test ","sandbox":"read-only","codex_auto_approve":false})).unwrap();
    assert_eq!(saved["models"]["api"], "text-test");
    assert_eq!(saved["sandbox"], "read-only");
    assert!(saved["agents"].is_array());
    let persisted: Value = serde_json::from_str(&std::fs::read_to_string(workspace.root.join("outputs/_harness/config.json")).unwrap()).unwrap();
    assert_eq!(persisted["models"]["api"], "text-test");
    assert!(persisted.get("model").is_none());
    for input in [
        json!({"runtime":"openclaw","model":"m","sandbox":"read-only"}),
        json!({"runtime":"api","model":"m","sandbox":"danger-full-access"}),
        json!({"runtime":"api","model":"a\nb","sandbox":"read-only"}),
        json!({"runtime":"api","model":"m","sandbox":"read-only","codex_auto_approve":"true"}),
    ] {
        assert!(route("POST", "/api/settings/harness", &input).is_err());
    }
    workspace.write(".env", "EASEL_AGENT_RUNTIME=codex\n");
    assert_eq!(route("POST", "/api/settings/harness", &json!({"runtime":"api","model":"m","sandbox":"read-only"})).unwrap_err().status, 409);
}

#[test]
fn model_channels_use_host_gateway_without_exposing_keys_or_replacing_other_settings() {
    let workspace = TestWorkspace::new("models");
    workspace.write("home/.otools/config.json", json!({
        "unrelated":{"preserve":true},
        "ai_switch":{"baseUrl":"http://127.0.0.1:19527","apiKey":"sk-test-never-expose-secret", "endpoints":{"text":"text-a","images":"image-a","video":"video-a","tts":"speech-a","embeddings":"embed-a","stt":"transcribe-a"}}
    }).to_string());
    let channels = route("GET", "/api/settings/models", &Value::Null).unwrap();
    for (channel, model) in [("chat","text-a"),("image","image-a"),("video","video-a"),("speech","speech-a"),("transcribe","transcribe-a")] {
        let row = &channels["channels"][channel]["rows"][0];
        assert_eq!(row["model"], model);
        assert_eq!(row["slot"], "ai-switch");
        assert_eq!(row["baseUrl"], "http://127.0.0.1:19527/v1");
        assert!(!row["keyMasked"].as_str().unwrap().is_empty());
    }
    assert!(!channels.to_string().contains("sk-test-never-expose-secret"));
    let saved = route("POST", "/api/settings/models/save", &json!({"channel":"image","rows":[{"slot":"ai-switch","model":"image-b","baseUrl":"http://127.0.0.1:19527/v1","key":""}]})).unwrap();
    assert_eq!(saved["channels"]["image"]["rows"][0]["model"], "image-b");
    let config: Value = serde_json::from_str(&std::fs::read_to_string(workspace.root.join("home/.otools/config.json")).unwrap()).unwrap();
    assert_eq!(config["unrelated"]["preserve"], true);
    assert_eq!(config["ai_switch"]["apiKey"], "sk-test-never-expose-secret");
    assert_eq!(config["ai_switch"]["endpoints"]["video"], "video-a");
    let refused = route("POST", "/api/settings/models/save", &json!({"channel":"image","rows":[{"slot":"ai-switch","model":"image-b","baseUrl":"https://different.example/v1","key":""}]})).unwrap_err();
    assert_eq!(refused.status, 400);
}

#[test]
fn bootstrap_and_proxy_settings_follow_real_configuration_and_validate_urls() {
    let workspace = TestWorkspace::new("bootstrap");
    workspace.write("outputs/_harness/config.json", r#"{"runtime":"api","models":{},"sandbox":"read-only"}"#);
    let missing = route("GET", "/api/settings/bootstrap", &Value::Null).unwrap();
    assert_eq!(missing["modelConfigured"], false);
    assert_eq!(missing["authVerified"], false);
    workspace.write("home/.otools/config.json", r#"{"ai_switch":{"baseUrl":"http://127.0.0.1:19527/v1","apiKey":"sk-test","endpoints":{"text":"text-a"}}}"#);
    let configured = route("GET", "/api/settings/bootstrap", &Value::Null).unwrap();
    assert_eq!(configured["modelConfigured"], true);
    assert_eq!(configured["authVerified"], false);
    let saved = route("POST", "/api/settings/proxy", &json!({"proxy":"http://127.0.0.1:7890"})).unwrap();
    assert_eq!(saved, json!({"ok":true,"proxy":"http://127.0.0.1:7890","source":"envfile"}));
    assert_eq!(route("GET", "/api/settings/proxy", &Value::Null).unwrap()["source"], "envfile");
    for proxy in ["http://user:pass@127.0.0.1:7890", "file:///etc", "http://127.0.0.1:0", "http://127.0.0.1/path", "http://a\nX=bad"] {
        assert_eq!(route("POST", "/api/settings/proxy", &json!({"proxy":proxy})).unwrap_err().status, 400);
    }
    let cleared = route("POST", "/api/settings/proxy", &json!({"proxy":""})).unwrap();
    assert_eq!(cleared["source"], "none");
}

#[test]
fn original_skill_cards_have_array_status_and_alias_details_without_frontmatter() {
    let workspace = TestWorkspace::new("skills");
    workspace.write("skills/builtin/skill-quality-gate/SKILL.md", "---\nname: quality-gate\ndescription: >-\n  发布质量检查\n  确保可靠\nlayer: publish\n---\n\n# 正文\n流程\n");
    workspace.write("skills/builtin/ai-video-gen/SKILL.md", "---\nname: ai-video-gen\ndescription: 视频生成\nlayer: produce\n---\n# 视频\n");
    workspace.write("skills/skills-manifest.json", r#"{"skills":["skills/builtin/skill-quality-gate/SKILL.md","skills/builtin/ai-video-gen/SKILL.md"]}"#);
    let skills = route("GET", "/api/skills", &Value::Null).unwrap();
    let skills = skills.as_array().expect("原版技能库消费数组");
    assert_eq!(skills[0]["name"], "ai-video-gen");
    assert_eq!(skills[0]["needsApi"], true);
    assert_eq!(skills[0]["apiConfigured"], false);
    assert_eq!(skills[1]["name"], "skill-quality-gate");
    assert_eq!(skills[1]["apiConfigured"], true);
    let detail = route("GET", "/api/skill/quality-gate", &Value::Null).unwrap();
    assert_eq!(detail["name"], "skill-quality-gate");
    assert_eq!(detail["description"], "发布质量检查 确保可靠");
    assert_eq!(detail["body"], "# 正文\n流程");
    assert_eq!(detail["apiSpec"], Value::Null);
    workspace.write("home/.otools/config.json", r#"{"ai_switch":{"baseUrl":"http://127.0.0.1:19527","apiKey":"sk-test","endpoints":{"video":"video-a"}}}"#);
    let video = route("GET", "/api/skill/ai-video-gen", &Value::Null).unwrap();
    assert_eq!(video["apiConfigured"], true);
    assert!(video["apiSpec"]["providers"].is_array());
    assert!(!video.to_string().contains("sk-test"));
}

#[test]
#[cfg(unix)]
fn install_job_rechecks_tool_reports_version_and_keeps_only_recent_logs() {
    use std::os::unix::fs::PermissionsExt;
    let workspace = TestWorkspace::new("install-success");
    let python = workspace.root.join(".venv/bin/python");
    workspace.write(".venv/bin/python", "#!/bin/sh\ncase \"$*\" in\n  *'-m pip install'*) i=0; while [ $i -lt 70 ]; do printf 'step-%s\\n' \"$i\" >&2; i=$((i+1)); done; : > .fake-fw-ready; exit 0;;\n  *faster_whisper*) if [ -f .fake-fw-ready ]; then printf '1.2-fixture\\n'; exit 0; fi; exit 1;;\n  *--version*) printf 'Python 3.11-fixture\\n'; exit 0;;\n  *) exit 1;;\nesac\n");
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
    let started = route("POST", "/api/env/install", &json!({"id":"fw"})).unwrap();
    assert_eq!(started["state"], "running");
    let path = format!("/api/env/job/{}", started["jobId"].as_str().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let final_job = loop {
        let job = route("GET", &path, &Value::Null).unwrap();
        if job["state"] != "running" { break job; }
        assert!(std::time::Instant::now() < deadline, "安装任务应完成");
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(final_job["state"], "ok");
    assert_eq!(final_job["result"]["version"], "1.2-fixture");
    assert_eq!(final_job["lines"].as_array().unwrap().len(), 40);
    assert!(final_job["ended"].is_number());
    let checked = route("GET", "/api/env/tools?refresh=1", &Value::Null).unwrap();
    let fw = checked["tools"].as_array().unwrap().iter().find(|tool| tool["id"] == "fw").unwrap();
    assert_eq!(fw["state"], "ok");
}

#[test]
#[cfg(unix)]
fn failed_install_is_reported_as_failed_and_keeps_the_tool_missing() {
    use std::os::unix::fs::PermissionsExt;
    let workspace = TestWorkspace::new("install-failure");
    workspace.write(".venv/bin/python", "#!/bin/sh\nprintf 'fixture installation unavailable\\n' >&2\nexit 1\n");
    std::fs::set_permissions(workspace.root.join(".venv/bin/python"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let started = route("POST", "/api/env/install", &json!({"id":"fw"})).unwrap();
    let path = format!("/api/env/job/{}", started["jobId"].as_str().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let final_job = loop {
        let job = route("GET", &path, &Value::Null).unwrap();
        if job["state"] != "running" { break job; }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(final_job["state"], "fail");
    assert_eq!(final_job["result"]["state"], "fail");
    assert!(final_job["result"]["detail"].as_str().unwrap().contains("失败"));
    assert!(final_job["ended"].is_number());
}

#[test]
fn model_selftest_and_discovery_only_contact_the_configured_gateway_without_generation() {
    use std::io::{Read, Write};
    let workspace = TestWorkspace::new("model-selftest");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    workspace.write("home/.otools/config.json", json!({"ai_switch":{"baseUrl":format!("http://{addr}"),"apiKey":"sk-fixture","endpoints":{"text":"model-fixture"}}}).to_string());
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 4096];
            let length = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..length]);
            assert!(request.starts_with("GET /v1/models HTTP/1.1"));
            assert!(request.to_ascii_lowercase().contains("authorization: bearer sk-fixture"));
            let payload = r#"{"data":[{"id":"model-fixture"}]}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}", payload.len(), payload).unwrap();
        }
    });
    let catalog = route("POST", "/api/settings/models/available", &json!({"slot":"ai-switch","baseUrl":"","key":""})).unwrap();
    assert_eq!(catalog["models"], json!(["model-fixture"]));
    let checked = route("POST", "/api/settings/models/selftest", &json!({"channel":"chat"})).unwrap();
    assert_eq!(checked["channel"], "chat");
    assert_eq!(checked["results"][0]["ok"], true);
    assert!(checked["results"][0]["detail"].as_str().unwrap().contains("未执行"));
    assert!(!checked.to_string().contains("sk-fixture"));
    server.join().unwrap();
}

#[test]
fn environment_install_rejects_unknown_or_option_like_ids_and_jobs_have_explicit_404() {
    let _workspace = TestWorkspace::new("environment");
    for id in ["--help", "missing", "node;rm", "../python"] {
        assert_eq!(route("POST", "/api/env/install", &json!({"id":id})).unwrap_err().status, 400);
    }
    assert_eq!(route("GET", "/api/env/job/nonexistent", &Value::Null).unwrap_err().status, 404);
}
