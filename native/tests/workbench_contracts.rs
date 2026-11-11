mod support;

use base64::Engine as _;
use otools_easel_native::router::route;
use serde_json::{json, Value};
use support::TestWorkspace;

fn get(path: &str) -> Value {
    route("GET", path, &Value::Null).unwrap()
}

#[test]
fn original_persona_contract_supports_encoded_names_and_extra_markdown() {
    let workspace = TestWorkspace::new("profiles");
    workspace.write("profiles/科技达人/identity.md", "# 定位\n\n分享科学知识\n");
    workspace.write("profiles/科技达人/style.md", "# 风格\n简洁");
    workspace.write("profiles/科技达人/z-notes.md", "额外说明");
    workspace.write("profiles/_template/identity.md", "模板");
    workspace.write("profiles/_private/identity.md", "私有");

    let list = get("/api/personas");
    assert_eq!(list, json!([{ "name": "科技达人", "description": "分享科学知识" }]));

    let detail = get("/api/persona/%E7%A7%91%E6%8A%80%E8%BE%BE%E4%BA%BA");
    assert_eq!(detail["name"], "科技达人");
    assert_eq!(detail["content"], "# 定位\n\n分享科学知识\n\n---\n\n# 风格\n简洁\n\n---\n\n额外说明");

    let files = get("/api/persona/科技达人/files");
    let files = files["files"].as_array().expect("原 React 编辑器消费文件数组");
    assert_eq!(files[0], json!({ "filename": "identity.md", "content": "# 定位\n\n分享科学知识\n" }));
    assert_eq!(files[5]["filename"], "memory.md");
    assert_eq!(files[6]["filename"], "z-notes.md");

    let saved = route("PUT", "/api/persona/科技达人/file", &json!({"filename":"z-notes.md", "content":"已更新"})).unwrap();
    assert_eq!(saved, json!({"ok":true, "filename":"z-notes.md"}));
    assert_eq!(std::fs::read_to_string(workspace.root.join("profiles/科技达人/z-notes.md")).unwrap(), "已更新");
    assert!(route("PUT", "/api/persona/科技达人/file", &json!({"filename":"../escape.md", "content":"bad"})).is_err());
}

#[test]
fn original_content_library_contract_is_nested_with_verified_deliverables() {
    let workspace = TestWorkspace::new("outputs");
    workspace.write("outputs/项目一/.easel.json", r#"{"title":"新选题","platform":"小红书","kind":"article","status":"ready","cover":"封面.png","deliverables":["文案.md","missing.md"]}"#);
    workspace.write("outputs/项目一/文案.md", "# 成稿\n内容");
    workspace.write("outputs/项目一/封面.png", b"image");
    workspace.write("outputs/项目一/assets/_base.mp4", b"video");
    workspace.write("outputs/项目一/.hidden", "hidden");
    workspace.write("outputs/_login/token.json", "secret");
    workspace.write("outputs/_schedule.json", "[]");
    workspace.write("outputs/analytics/data.json", "secret");
    workspace.write("outputs/loose.md", "not a project");

    let tree = get("/api/outputs");
    let projects = tree.as_array().expect("原 React 内容库消费项目数组");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["type"], "dir");
    assert_eq!(projects[0]["fileCount"], 3);
    assert_eq!(projects[0]["meta"]["title"], "新选题");
    assert_eq!(projects[0]["meta"]["cover"], "项目一/封面.png");
    assert_eq!(projects[0]["meta"]["deliverablePaths"], json!(["项目一/文案.md"]));
    let children = projects[0]["children"].as_array().unwrap();
    let assets = children.iter().find(|node| node["name"] == "assets").unwrap();
    assert_eq!(assets["children"][0]["name"], "_base.mp4");
    assert_eq!(assets["children"][0]["kind"], "video");

    assert_eq!(get("/api/output/%E9%A1%B9%E7%9B%AE%E4%B8%80/%E6%96%87%E6%A1%88.md"), json!({"path":"项目一/文案.md","content":"# 成稿\n内容","kind":"text","isBinary":false}));
    assert_eq!(get("/api/output/项目一/封面.png"), json!({"path":"项目一/封面.png","content":"","kind":"image","isBinary":true}));
    let denied = route("DELETE", "/api/output/_login/token.json", &Value::Null).unwrap_err();
    assert_eq!(denied.status, 403);
    assert!(workspace.root.join("outputs/_login/token.json").exists());
}

#[test]
fn upload_contract_retains_duplicates_and_binds_refs_to_one_session() {
    let workspace = TestWorkspace::new("upload");
    let first = base64::engine::general_purpose::STANDARD.encode("第一张");
    let second = base64::engine::general_purpose::STANDARD.encode("第二张");
    let uploaded = route("POST", "/api/upload", &json!({
        "__formData":true,
        "fields":{"sessionId":"session-A"},
        "files":[
            {"field":"files","name":"素材.png","contentBase64":first},
            {"field":"files","name":"素材.png","contentBase64":second}
        ]
    })).unwrap();
    assert_eq!(get("/api/upload/limits"), json!({"ok":true,"max_mb":50}));
    assert_eq!(uploaded["ok"], true);
    let files = uploaded["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["name"], "素材.png");
    assert_eq!(files[1]["name"], "素材 (2).png");
    assert_ne!(files[0]["id"], files[1]["id"]);
    assert_eq!(std::fs::read_to_string(workspace.root.join("outputs").join(files[0]["path"].as_str().unwrap())).unwrap(), "第一张");
    assert_eq!(std::fs::read_to_string(workspace.root.join("outputs").join(files[1]["path"].as_str().unwrap())).unwrap(), "第二张");
    assert_eq!(files[0]["path"].as_str().unwrap().split('/').count(), 4);
    let invalid = route("POST", "/api/upload", &json!({"__formData":true,"fields":{"sessionId":"A"},"files":[{"field":"files","name":"evil.exe","contentBase64":"AA=="}]})).unwrap_err();
    assert_eq!(invalid.status, 400);
}

#[test]
fn chunk_upload_preserves_large_file_offsets_and_cleans_cancelled_files() {
    let workspace = TestWorkspace::new("chunk-upload");
    let started = route("POST", "/api/upload/start", &json!({
        "sessionId":"session-chunks", "local":true,
        "files":[{"name":"clip.mp4","size":8}]
    })).unwrap();
    let id = started["uploadId"].as_str().unwrap();
    let first = route("POST", "/api/upload/chunk", &json!({"uploadId":id,"index":0,"offset":0,"contentBase64":"YWJjZA=="})).unwrap();
    assert_eq!(first["received"], 4);
    assert_eq!(route("POST", "/api/upload/chunk", &json!({"uploadId":id,"index":0,"offset":2,"contentBase64":"ZWZnaA=="})).unwrap_err().status, 409);
    let last = route("POST", "/api/upload/chunk", &json!({"uploadId":id,"index":0,"offset":4,"contentBase64":"ZWZnaA=="})).unwrap();
    assert_eq!(last["received"], 8);
    let finished = route("POST", "/api/upload/finish", &json!({"uploadId":id})).unwrap();
    let saved = &finished["files"][0];
    assert_eq!(saved["name"], "clip.mp4");
    assert_eq!(std::fs::read(workspace.root.join("outputs").join(saved["path"].as_str().unwrap())).unwrap(), b"abcdefgh");
    assert!(otools_easel_native::files::attachment_context(Some("session-chunks"), &finished["files"]).is_ok());
    let started = route("POST", "/api/upload/start", &json!({"sessionId":"session-chunks","local":true,"files":[{"name":"cancel.mp4","size":6}]})).unwrap();
    let cancelled_id = started["uploadId"].as_str().unwrap();
    route("POST", "/api/upload/chunk", &json!({"uploadId":cancelled_id,"index":0,"offset":0,"contentBase64":"YWJj"})).unwrap();
    let cancelled_path = workspace.root.join("outputs").join(started["files"][0]["path"].as_str().unwrap());
    route("POST", "/api/upload/abort", &json!({"uploadId":cancelled_id})).unwrap();
    assert!(!cancelled_path.exists());
    let oversized = route("POST", "/api/upload/start", &json!({"sessionId":"A","local":false,"files":[{"name":"oversize.mp4","size":52428801}]})).unwrap_err();
    assert_eq!(oversized.status, 413);
}

#[test]
fn uploads_reject_executable_extensions_not_in_the_upstream_allowlist() {
    let _workspace = TestWorkspace::new("upload-extensions");
    for name in ["script.py", "script.js", "page.html", "styles.css", "run.exe"] {
        let failure = route("POST", "/api/upload", &json!({"fields":{"sessionId":"A"},"files":[{"name":name,"contentBase64":"aGk="}]})).unwrap_err();
        assert_eq!(failure.status, 400, "{name}");
    }
}

#[test]
fn chat_refuses_cross_session_attachments_before_starting_an_agent() {
    let workspace = TestWorkspace::new("attachment-owner");
    workspace.write("outputs/_harness/config.json", r#"{"runtime":"api","models":{},"sandbox":"read-only"}"#);
    let uploaded = route("POST", "/api/upload", &json!({"fields":{"sessionId":"session-A"},"files":[{"name":"素材.png","contentBase64":"aGk="}]})).unwrap();
    let attachments = uploaded["files"].clone();
    let result = route("POST", "/api/agent/prepare", &json!({"message":"使用素材","sessionId":"session-B","attachments":attachments}));
    assert_eq!(result.unwrap_err().status, 403);
    let mut changed = attachments.clone();
    changed[0]["id"] = json!("fake");
    assert_eq!(route("POST", "/api/agent/prepare", &json!({"message":"使用素材","sessionId":"session-A","attachments":changed})).unwrap_err().status, 403);
    let context = otools_easel_native::files::attachment_context(Some("session-A"), &attachments).unwrap();
    assert!(context.contains(attachments[0]["path"].as_str().unwrap()));
}

#[test]
fn media_chunks_and_path_guards_keep_access_within_outputs() {
    let workspace = TestWorkspace::new("media");
    workspace.write("outputs/项目/clip.mp4", b"abcdefgh");
    let meta = get("/api/media/项目/clip.mp4");
    assert_eq!(meta["mimeType"], "video/mp4");
    assert_eq!(meta["size"], 8);
    let chunk = get("/api/media/项目/clip.mp4?offset=2&length=3");
    assert_eq!(chunk["offset"], 2);
    assert_eq!(chunk["eof"], false);
    assert_eq!(base64::engine::general_purpose::STANDARD.decode(chunk["contentBase64"].as_str().unwrap()).unwrap(), b"cde");
    let chunk = get("/api/media/项目/clip.mp4?offset=5&length=3");
    assert_eq!(chunk["eof"], true);
    for path in ["/api/output/../secret", "/api/output/%2e%2e/secret", "/api/output/%2fetc/passwd", "/api/output/项目/%2e%2e/%2e%2e/secret"] {
        assert_eq!(route("GET", path, &Value::Null).unwrap_err().status, 403, "{path}");
    }
    #[cfg(unix)]
    {
        let outside = workspace.root.join("secret.txt");
        std::fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, workspace.root.join("outputs/项目/leak.txt")).unwrap();
        assert_eq!(route("GET", "/api/media/项目/leak.txt", &Value::Null).unwrap_err().status, 403);
    }
}
