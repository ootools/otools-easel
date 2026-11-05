//! Browser-automation orchestration: login/publish/analytics spawn the upstream
//! Python skill scripts (Playwright layer stays in Python); Rust only builds the
//! argv, manages async jobs, and reads status/QR/SMS files.

use crate::process::{self, capture};
use crate::router::ApiError;
use crate::workspace::{self, project_root};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

fn error(status: u16, detail: impl Into<String>) -> ApiError { ApiError { status, detail: detail.into() } }


fn python() -> Option<PathBuf> {
    let root = workspace::project_root();
    let venv = if cfg!(windows) { root.join(".venv/Scripts/python.exe") } else { root.join(".venv/bin/python") };
    venv.is_file().then_some(venv).or_else(|| process::executable("python3")).or_else(|| process::executable("python"))
}

fn scripts() -> PathBuf { workspace::project_root().join("skills/shared/scripts") }
fn login_dir() -> PathBuf { workspace::project_root().join("outputs/_login") }
fn publish_dir() -> PathBuf { workspace::project_root().join("outputs/_publish") }
fn analytics_dir() -> PathBuf { workspace::project_root().join("outputs/_analytics") }

struct Platform { key: &'static str, name: &'static str, backend: &'static str, wp: &'static str, login_window: bool }
const PLATFORMS: &[Platform] = &[
    Platform{key:"xiaohongshu",name:"小红书",backend:"xhs",wp:"",login_window:false},
    Platform{key:"kuaishou",name:"快手",backend:"web",wp:"kuaishou",login_window:false},
    Platform{key:"weixin-channels",name:"微信视频号",backend:"web",wp:"weixin-channels",login_window:false},
    Platform{key:"zhihu",name:"知乎",backend:"web",wp:"zhihu",login_window:false},
    Platform{key:"juejin",name:"掘金",backend:"web",wp:"juejin",login_window:false},
    Platform{key:"twitter-x",name:"推特 X",backend:"web",wp:"twitter-x",login_window:true},
    Platform{key:"bilibili",name:"B站",backend:"biliup",wp:"",login_window:false},
    Platform{key:"douyin",name:"抖音",backend:"douyin",wp:"",login_window:false},
    Platform{key:"toutiao",name:"今日头条",backend:"toutiao",wp:"",login_window:false},
    Platform{key:"wechat-oa",name:"微信公众号",backend:"wechat-oa",wp:"",login_window:false},
];

fn platform(key: &str) -> std::result::Result<&'static Platform, ApiError> {
    PLATFORMS.iter().find(|p| p.key == key).ok_or_else(|| error(404, "未知平台"))
}

fn login_marker(platform: &str) -> Value {
    let path = login_dir().join(format!("{platform}.json"));
    std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_else(|| json!({"state":"unknown","message":""}))
}

fn logged_in(platform: &str) -> bool { login_marker(platform)["state"].as_str() == Some("success") }

// ── 登录 ────────────────────────────────────────────────────────────────────

static LOGIN_PROCESSES: OnceLock<Mutex<HashMap<String, Arc<AtomicOwned>>>> = OnceLock::new();
struct AtomicOwned { child_pid: Mutex<u32>, killed: Mutex<bool> }

fn login_processes() -> &'static Mutex<HashMap<String, Arc<AtomicOwned>>> {
    LOGIN_PROCESSES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn login_start(platform_key: &str, restart: bool) -> std::result::Result<Value, ApiError> {
    let platform = platform(platform_key)?;
    if platform.backend == "wechat-oa" {
        let configured = wechat_credentials_configured();
        return Ok(json!({"mode":"credentials","configured":configured,"message":"微信公众号请填写 AppID / AppSecret"}));
    }
    let mut registry = login_processes().lock().map_err(|_| error(500, "登录状态不可用"))?;
    if let Some(existing) = registry.get(platform_key) {
        let pid = existing.child_pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).to_string();
        let alive = Path::new("/proc").join(&pid).exists() && !*existing.killed.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if alive && !restart { drop(registry); return login_status_impl(platform_key); }
        let _ = std::process::Command::new("kill").args(["-9", &pid]).status();
        registry.remove(platform_key);
    }
    let dir = login_dir();
    std::fs::create_dir_all(&dir).map_err(|cause| error(500, cause.to_string()))?;
    let qr = dir.join(format!("{platform_key}.png"));
    let status = dir.join(format!("{platform_key}.json"));
    let code = dir.join(format!("{platform_key}.code"));
    for path in [&qr, &status] { let _ = std::fs::remove_file(path); }
    if platform.backend == "douyin" { let _ = std::fs::remove_file(&code); }
    let python = python().ok_or_else(|| error(409, "请先安装 Python"))?;
    let scripts = scripts();
    let timeout = "300";
    let argv: Vec<String> = match platform.backend {
        "xhs" => vec!["login".into(), "--no-proxy".into(), "--qr-out".into(), qr.to_string_lossy().into_owned(), "--status-file".into(), status.to_string_lossy().into_owned(), "--timeout".into(), timeout.into()],
        "biliup" => vec!["login".into(), "--qr-out".into(), qr.to_string_lossy().into_owned(), "--status-file".into(), status.to_string_lossy().into_owned(), "--cookie".into(), workspace::project_root().join("cookies.json").to_string_lossy().into_owned(), "--timeout".into(), timeout.into()],
        "douyin" => vec!["login".into(), "--no-proxy".into(), "--qr-out".into(), qr.to_string_lossy().into_owned(), "--status-file".into(), status.to_string_lossy().into_owned(), "--sms-code-file".into(), code.to_string_lossy().into_owned(), "--timeout".into(), timeout.into()],
        "toutiao" => vec!["login".into(), "--qr-out".into(), qr.to_string_lossy().into_owned(), "--status-file".into(), status.to_string_lossy().into_owned(), "--timeout".into(), timeout.into()],
        _ => {
            let mut argv = vec!["login-qr".into(), "--platform".into(), platform.wp.into(), "--qr-out".into(), qr.to_string_lossy().into_owned(), "--status-file".into(), status.to_string_lossy().into_owned(), "--timeout".into(), timeout.into()];
            if platform.login_window { argv.push("--headed".into()); }
            argv
        }
    };
    let script_path = match platform.backend {
        "xhs" => scripts.join("xhs_publish.py"),
        "biliup" => scripts.join("bili_login.py"),
        "douyin" => scripts.join("douyin_publish.py"),
        "toutiao" => workspace::project_root().join("skills/builtin/skill-toutiao-publisher/scripts/toutiao_publish.py"),
        _ => scripts.join("web_publisher.py"),
    };
    let log = dir.join(format!("{platform_key}.log"));
    let mut log_file = std::fs::OpenOptions::new().create(true).append(true).open(&log).map_err(|cause| error(500, cause.to_string()))?;
    writeln!(log_file, "\n===== login start {} =====", chrono::Local::now().format("%H:%M:%S")).ok();
    let mut command = std::process::Command::new(python);
    command.arg(&script_path).args(&argv).current_dir(workspace::project_root())
        .env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1")
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    { use std::os::unix::process::CommandExt; command.process_group(0); }
    let child = command.spawn().map_err(|cause| error(500, cause.to_string()))?;
    let pid = child.id();
    registry.insert(platform_key.to_string(), Arc::new(AtomicOwned { child_pid: Mutex::new(pid), killed: Mutex::new(false) }));
    drop(registry);
    login_status_impl(platform_key)
}


pub fn login_status_impl(platform_key: &str) -> std::result::Result<Value, ApiError> {
    let platform = platform(platform_key)?;
    let mut data = login_marker(platform_key);
    if !matches!(data["state"].as_str().unwrap_or("unknown"), "success" | "expired" | "error") {
        if let Some(existing) = login_processes().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(platform_key) {
            let pid = existing.child_pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).to_string();
            let alive = Path::new("/proc").join(&pid).exists();
            if !alive {
                data = json!({"state":"error","message":format!("登录程序异常退出（退出码未知），请查看 outputs/_login/{platform_key}.log")});
            }
        }
    }
    if matches!(data["state"].as_str().unwrap_or(""), "qr_ready" | "window_login") {
        let png = login_dir().join(format!("{platform_key}.png"));
        if png.is_file() {
            data["qr"] = json!(format!("_login/{platform_key}.png"));
            data["qrTs"] = json!(std::fs::metadata(&png).ok().and_then(|meta| meta.modified().ok()).and_then(|date| date.duration_since(std::time::UNIX_EPOCH).ok()).map(|date| date.as_millis() as u64).unwrap_or(0));
        }
    }
    if data["qr"].as_str().unwrap_or("").is_empty() { data["qr"] = json!(""); data["qrTs"] = json!(0); }
    Ok(data)
}

pub fn login_sms(platform_key: &str, code: &str) -> std::result::Result<Value, ApiError> {
    platform(platform_key)?;
    let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if !(4..=8).contains(&digits.len()) { return Err(error(400, "验证码应为 4-8 位数字")); }
    std::fs::create_dir_all(login_dir()).map_err(|cause| error(500, cause.to_string()))?;
    std::fs::write(login_dir().join(format!("{platform_key}.code")), &digits).map_err(|cause| error(500, cause.to_string()))?;
    Ok(json!({"ok":true}))
}

fn wechat_credentials_configured() -> bool {
    let path = workspace::project_root().join("skills/builtin/skill-wechat-publisher/wechat-publisher.yaml");
    path.is_file() && std::fs::read_to_string(path).map(|text| text.contains("app_id:") && !text.contains("app_id: ''")).unwrap_or(false)
}

// ── 发布编排 ────────────────────────────────────────────────────────────────

struct PublishJob { value: Mutex<Value> }
static PUBLISH_JOBS: OnceLock<Mutex<HashMap<String, Arc<PublishJob>>>> = OnceLock::new();
fn publish_jobs() -> &'static Mutex<HashMap<String, Arc<PublishJob>>> { PUBLISH_JOBS.get_or_init(|| Mutex::new(HashMap::new())) }

pub fn publish(platform_key: &str, body: &Value) -> std::result::Result<Value, ApiError> {
    let platform = platform(platform_key)?;
    let title = body["title"].as_str().unwrap_or_default().trim().to_string();
    let content = body["body"].as_str().unwrap_or_default().to_string();
    if title.is_empty() && content.is_empty() { return Err(error(400, "标题/正文不能为空")); }
    let media = body["media"].as_array().cloned().unwrap_or_default();
    let tags = body["tags"].as_str().unwrap_or_default().to_string();
    let is_draft = platform_key == "toutiao" && body["draft"].as_bool().unwrap_or(false);
    let mut images = Vec::new();
    let mut videos = Vec::new();
    for item in media.iter().filter_map(Value::as_str) {
        let path = crate::files::output_path(item)?;
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or_default().to_ascii_lowercase();
        if ["mp4","mov","webm","m4v"].contains(&ext.as_str()) { videos.push(path.to_string_lossy().into_owned()) }
        else if ["png","jpg","jpeg","gif","webp","bmp","svg"].contains(&ext.as_str()) { images.push(path.to_string_lossy().into_owned()) }
    }
    if ["xiaohongshu","douyin","kuaishou","weixin-channels","bilibili","wechat-oa"].contains(&platform_key) && images.is_empty() && videos.is_empty() {
        return Err(error(400, format!("{} 需附带图片或视频", platform.name)));
    }
    if !images.is_empty() && !videos.is_empty() { return Err(error(400, "同一条内容不能同时发图片和视频，请二选一")); }
    if ["douyin","weixin-channels","bilibili"].contains(&platform_key) && videos.is_empty() { return Err(error(400, format!("{} 只能发视频，请附带一个视频文件", platform.name))); }
    let title_final = if title.is_empty() { content.chars().take(20).collect() } else { title };
    let python = python().ok_or_else(|| error(409, "请先安装 Python"))?;
    let scripts = scripts();
    let root = workspace::project_root();
    let mut argv: Vec<String> = match platform_key {
        "xiaohongshu" => {
            let mut argv = if !videos.is_empty() {
                vec![scripts.join("xhs_publish.py").to_string_lossy().into_owned(), "publish-video".into(), "--no-proxy".into(), "--video".into(), videos[0].clone()]
            } else {
                vec![scripts.join("xhs_publish.py").to_string_lossy().into_owned(), "publish".into(), "--no-proxy".into(), "--images".into(), images.join(",")]
            };
            argv.extend(["--title".into(), title_final.clone(), "--content".into(), content.clone(), "--tags".into(), tags.clone(), "--exec".into()]);
            argv
        }
        "bilibili" => {
            let tag = tags.replace('#', "").replace('，', ",").trim().trim_matches(',').to_string();
            let tag = if tag.is_empty() { "日常".to_string() } else { tag };
            let mut argv = vec!["biliup".into(), "-u".into(), root.join("cookies.json").to_string_lossy().into_owned(), "upload".into(), videos[0].clone(), "--title".into(), title_final.chars().take(80).collect(), "--tid".into(), "36".into(), "--copyright".into(), "1".into(), "--tag".into(), tag];
            if !content.trim().is_empty() { argv.extend(["--desc".into(), content.chars().take(2000).collect()]); }
            argv
        }
        "douyin" => {
            let mut argv = if !videos.is_empty() {
                vec![scripts.join("douyin_publish.py").to_string_lossy().into_owned(), "publish-video".into(), "--no-proxy".into(), "--video".into(), videos[0].clone()]
            } else {
                vec![scripts.join("douyin_publish.py").to_string_lossy().into_owned(), "publish".into(), "--no-proxy".into(), "--images".into(), images.join(",")]
            };
            argv.extend(["--title".into(), title_final.clone(), "--content".into(), content.clone(), "--tags".into(), tags.clone(), "--exec".into(), "--status-file".into(), publish_dir().join("douyin.json").to_string_lossy().into_owned(), "--sms-code-file".into(), publish_dir().join("douyin.code").to_string_lossy().into_owned()]);
            argv
        }
        "toutiao" => {
            let toutiao_scripts = root.join("skills/builtin/skill-toutiao-publisher/scripts");
            let mut argv = vec![toutiao_scripts.join("toutiao_publish.py").to_string_lossy().into_owned(), "--content".into(), content.clone(), "--title".into(), title_final.clone(), "--tags".into(), tags.clone()];
            if !images.is_empty() { argv.extend(["--images".into(), images.join(",")]); }
            argv.push(if is_draft { "--draft".into() } else { "--exec".into() });
            argv
        }
        "wechat-oa" => {
            if !logged_in("wechat-oa") { return Err(error(400, "公众号后台未登录：请先在账号页点「登录公众号后台」扫码")); }
            if images.is_empty() { return Err(error(400, "公众号文章需要一张封面图，请附带至少一张图片")); }
            let wx_scripts = root.join("skills/builtin/skill-wechat-publisher/scripts");
            std::fs::create_dir_all(publish_dir()).map_err(|cause| error(500, cause.to_string()))?;
            let stamp = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
            let md = publish_dir().join(format!("wechat-oa-{stamp}.md"));
            let mut body_md = content.clone();
            for extra in images.iter().skip(1) { body_md = format!("{body_md}\n\n![]({extra})"); }
            std::fs::write(&md, format!("# {title_final}\n\n{body_md}\n")).map_err(|cause| error(500, cause.to_string()))?;
            let html = publish_dir().join(format!("wechat-oa-{stamp}.html"));
            let output = capture(&python, &[wx_scripts.join("html_converter.py").to_string_lossy().into_owned(), md.to_string_lossy().into_owned(), "-o".into(), html.to_string_lossy().into_owned()], &root, Duration::from_secs(60), None).map_err(|cause| error(500, cause))?;
            if !output.success || !html.is_file() { return Err(error(500, format!("排版失败：{}", output.stderr.chars().take(200).collect::<String>()))); }
            vec![scripts.join("weixin_mp_stats.py").to_string_lossy().into_owned(), "publish".into(), "--proxy".into(), String::new(), "--html".into(), html.to_string_lossy().into_owned(), "--cover".into(), images[0].clone(), "--title".into(), title_final.clone(), "--digest".into(), content.trim().chars().take(100).collect(), "--author".into(), String::new()]
        }
        _ => {
            let mut argv = vec![scripts.join("web_publisher.py").to_string_lossy().into_owned(), "publish".into(), "--platform".into(), platform.wp.into(), "--title".into(), title_final.clone(), "--desc".into(), content.clone(), "--tags".into(), tags.clone(), "--exec".into()];
            let media_path = videos.first().or_else(|| images.first());
            if let Some(media) = media_path { argv.extend(["--media".into(), media.clone()]); }
            argv
        }
    };
    if platform.backend == "xhs" || platform.backend == "douyin" {
        for arg in argv.iter_mut() { if arg == "--no-proxy" { *arg = "--no-proxy".into(); } }
    }
    let _ = &mut argv;
    // 异步发布：douyin 走 status-file 轮询；其余同步捕获(600s)
    let job_id = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
    if platform_key == "douyin" {
        let job = Arc::new(PublishJob { value: Mutex::new(json!({"mode":"publish","state":"starting","message":"提交发布任务…","platform":platform_key})) });
        publish_jobs().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(platform_key.to_string(), job.clone());
        let publish_dir = publish_dir();
        let job_python = python.clone();
        let job_logs = job.clone();
        std::thread::Builder::new().name(format!("easel-publish-{platform_key}")).spawn(move || {
            let program = &job_python;
            let script = argv[0].clone();
            let output = capture(Path::new(&script), &argv[1..], &root, Duration::from_secs(600), Some(Arc::new(move |line: String| {
                if let Ok(mut value) = job_logs.value.lock() { value["message"] = json!(line); }
            })));
            if let Ok(mut value) = job.value.lock() {
                match output {
                    Ok(output) if output.success => { value["state"] = json!("success"); value["message"] = json!("发布成功"); }
                    Ok(output) => { value["state"] = json!("fail"); value["message"] = json!(output.stderr.chars().take(300).collect::<String>()); }
                    Err(cause) => { value["state"] = json!("fail"); value["message"] = json!(cause); }
                }
            }
        }).map_err(|cause| error(500, cause.to_string()))?;
        return Ok(json!({"mode":"publish","state":"starting","message":"提交发布任务…","platform":platform_key}));
    }
    // 同步发布（600s 内完成）
    let program = python;
    let script = argv[0].clone();
    let output = capture(Path::new(&script), &argv[1..], &root, Duration::from_secs(600), None).map_err(|cause| error(500, cause))?;
    let ok = output.success;
    let mut detail = output.stderr.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    if platform_key == "toutiao" && ok {
        let receipt: Value = serde_json::from_str(&output.stdout).unwrap_or(Value::Null);
        let verified = receipt.get(if is_draft { "draft_saved" } else { "published" }).and_then(Value::as_bool).unwrap_or(false);
        if !verified { detail = "操作结果未核实，请到平台后台核对，先别重试。".into(); }
    }
    append_publish_log(platform_key, &argv, &output.stdout, &output.stderr);
    if ok { record_schedule(platform_key, &title_final, &content, if is_draft { "draft" } else { "published" }); }
    Ok(json!({"ok":ok,"message":if ok {if is_draft {"已存草稿"} else {"发布成功"}} else {"发布失败（见 detail）"},"detail":detail}))
}

fn append_publish_log(platform_key: &str, argv: &[String], stdout: &str, stderr: &str) {
    let path = workspace::project_root().join("outputs/_publish.log");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "\n===== {} {platform_key} =====", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        let _ = writeln!(file, "CMD: {}", argv.join(" "));
        let _ = writeln!(file, "STDOUT:\n{}", stdout.chars().rev().take(2000).collect::<String>().chars().rev().collect::<String>());
        let _ = writeln!(file, "STDERR:\n{}", stderr.chars().rev().take(2000).collect::<String>().chars().rev().collect::<String>());
    }
}

fn record_schedule(platform_key: &str, title: &str, body: &str, status: &str) {
    let name = PLATFORMS.iter().find(|p| p.key == platform_key).map(|p| p.name).unwrap_or(platform_key);
    let path = workspace::project_root().join("outputs/_schedule.json");
    let items: Vec<Value> = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let mut items = items;
    items.push(json!({
        "id": uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
        "title": title, "date": chrono::Local::now().format("%Y-%m-%d").to_string(),
        "platform": name, "time": "", "status": status,
        "note": format!("【一键发布】\n{body}"), "kind": "content", "url": "", "source": "chat",
        "event_type": "", "end_date": "",
    }));
    let _ = std::fs::write(path, serde_json::to_vec_pretty(&items).unwrap_or_default());
}

// ── publish status / sms ────────────────────────────────────────────────────

pub fn publish_status(platform_key: &str) -> std::result::Result<Value, ApiError> {
    platform(platform_key)?;
    let status_file = publish_dir().join(format!("{platform_key}.json"));
    let mut data = std::fs::read_to_string(&status_file).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).unwrap_or_else(|| json!({"state":"unknown","message":""}));
    data["mode"] = json!("publish");
    Ok(data)
}

pub fn publish_sms(platform_key: &str, code: &str) -> std::result::Result<Value, ApiError> {
    platform(platform_key)?;
    let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if !(4..=8).contains(&digits.len()) { return Err(error(400, "验证码应为 4-8 位数字")); }
    std::fs::create_dir_all(publish_dir()).map_err(|cause| error(500, cause.to_string()))?;
    std::fs::write(publish_dir().join(format!("{platform_key}.code")), &digits).map_err(|cause| error(500, cause.to_string()))?;
    Ok(json!({"ok":true}))
}

// ── analytics ───────────────────────────────────────────────────────────────

const ANALYTICS_PLATFORMS: &[&str] = &["xiaohongshu","douyin","kuaishou","zhihu","bilibili","wechat-oa","twitter-x","juejin"];

pub fn analytics_platforms() -> Value {
    let items: Vec<Value> = ANALYTICS_PLATFORMS.iter().map(|key| {
        let platform = PLATFORMS.iter().find(|p| p.key == *key);
        json!({"platform":key,"name":platform.map(|p|p.name).unwrap_or(key),"loggedIn":logged_in(key)})
    }).collect();
    json!(items)
}

pub fn analytics_fetch(platform_key: &str) -> std::result::Result<Value, ApiError> {
    if !ANALYTICS_PLATFORMS.contains(&platform_key) { return Err(error(404, "该平台暂不支持数据抓取")); }
    let python = python().ok_or_else(|| error(409, "请先安装 Python"))?;
    let scripts = scripts();
    let root = workspace::project_root();
    let timeout = if platform_key == "wechat-oa" { Duration::from_secs(180) } else { Duration::from_secs(90) };
    let argv: Vec<String> = match platform_key {
        "bilibili" => vec![scripts.join("bili_login.py").to_string_lossy().into_owned(), "stats".into(), "--cookie".into(), root.join("cookies.json").to_string_lossy().into_owned()],
        "twitter-x" => vec![scripts.join("x_stats.py").to_string_lossy().into_owned(), "fetch".into()],
        "juejin" => vec![scripts.join("juejin_stats.py").to_string_lossy().into_owned(), "fetch".into()],
        "wechat-oa" => vec![scripts.join("weixin_mp_stats.py").to_string_lossy().into_owned(), "stats".into(), "--proxy".into(), String::new(), "--count".into(), "20".into()],
        _ => vec![scripts.join("account_stats.py").to_string_lossy().into_owned(), "fetch".into(), "--platform".into(), platform_key.to_string()],
    };
    let output = capture(Path::new(&argv[0]), &argv[1..], &root, timeout, None).map_err(|cause| {
        if cause.contains("超时") { error(504, "抓取超时，已停止浏览器；请检查网络，关闭正在进行的登录或校验后重试") } else { error(502, cause) }
    })?;
    for line in output.stdout.lines().rev() {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') { continue; }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            if !value.is_object() { continue; }
            if let Some(err) = value.get("error").and_then(Value::as_str) {
                return Err(error(502, err.chars().take(240).collect::<String>()));
            }
            if value["loggedIn"].as_bool() == Some(true)
                && ["followers","likes","following","posts"].iter().all(|key| value.get(key).map(|v| v.is_null()).unwrap_or(true))
                && value.get("metrics").map(|v| v.is_null()).unwrap_or(true)
                && value.get("notes").map(|v| v.is_null()).unwrap_or(true)
            {
                return Err(error(502, "账号已登录，但平台未返回可用数据；请检查网络或平台维护状态后重试"));
            }
            return Ok(value);
        }
    }
    let detail = output.stderr.trim().lines().last().unwrap_or("未取到数据");
    Err(error(502, format!("未取到数据（可能未登录或平台改版）：{}", detail.chars().take(120).collect::<String>())))
}
