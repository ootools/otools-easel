//! /api/chat/* 与 /api/skill 路由实现:任务化事件流(上游 app.py 语义)。
//!
//! 事件契约(前端 lib/api.ts streamChat 已适配):
//! - 事件三元组 {id, event, data} 同步写入 `outputs/_jobs/{turnId}.jsonl`(重放)
//!   并推 native 事件 topic `easel-job-stream`(payload 附 turnId)
//! - 事件种类:token/thinking/activity/question/question_closed/heartbeat/error/done
//! - done 负载 {sessionKey};error 负载为 JSON 字符串;token/thinking/activity 为 JSON 字符串
//! - 单会话单任务(忙即拒);POST /api/chat/stop 可中止

use crate::agent::{self, AgentEvent, AppServer, SpawnOptions};
use crate::chat::{build_body, endpoint_for, feed_delta};
use crate::harness::{build_user_message, developer_instructions, direct_settings, load_harness};
use crate::workspace::{self, project_root};
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const JOB_EVENT_TOPIC: &str = "easel-job-stream";
const HEARTBEAT_SECS: u64 = 20;

// ── 任务注册表 ──────────────────────────────────────────────────────────────

struct JobHandle {
    #[allow(dead_code)]
    session_key: String,
    aborted: Arc<AtomicBool>,
    /// codex 中断所需 (thread_id, turn_id);直连模式无
    interrupt: Mutex<Option<(String, String)>>,
}

struct CodexSession {
    server: Arc<tokio::sync::Mutex<AppServer>>,
    thread_id: Mutex<Option<String>>,
    events_rx: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<AgentEvent>>,
    busy: AtomicBool,
}

static JOBS: OnceLock<Mutex<HashMap<String, Arc<JobHandle>>>> = OnceLock::new();
static SESSION_BUSY: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
static CODEX_SESSIONS: OnceLock<Mutex<HashMap<String, Arc<CodexSession>>>> = OnceLock::new();
static EVENT_SEQ: AtomicU64 = AtomicU64::new(0);

fn jobs() -> &'static Mutex<HashMap<String, Arc<JobHandle>>> {
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn session_busy() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    SESSION_BUSY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn codex_sessions() -> &'static Mutex<HashMap<String, Arc<CodexSession>>> {
    CODEX_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn has_running_jobs() -> bool {
    jobs().lock().map(|jobs| !jobs.is_empty()).unwrap_or(true)
}

pub fn close_idle_sessions() {
    let idle = codex_sessions().lock().map(|mut sessions| {
        let keys = sessions.iter().filter(|(_, session)| !session.busy.load(Ordering::SeqCst))
            .map(|(key, _)| key.clone()).collect::<Vec<_>>();
        keys.into_iter().filter_map(|key| sessions.remove(&key)).collect::<Vec<_>>()
    }).unwrap_or_default();
    for session in idle {
        crate::runtime().spawn(async move { session.server.lock().await.terminate().await; });
    }
}

fn jobs_dir() -> PathBuf {
    project_root().join("outputs").join("_jobs")
}

fn sessions_dir() -> PathBuf {
    project_root().join("sessions")
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0)
}

// ── 事件日志(重放 + 桥推送)────────────────────────────────────────────────

struct EventLog {
    turn_id: String,
    file: std::sync::Mutex<std::io::BufWriter<std::fs::File>>,
}

impl EventLog {
    fn open(turn_id: &str) -> Result<Self, String> {
        let dir = jobs_dir();
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{turn_id}.jsonl")))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            turn_id: turn_id.to_string(),
            file: std::sync::Mutex::new(std::io::BufWriter::new(file)),
        })
    }

    /// 记录并推送一条事件;data 为已序列化好的负载字符串。
    fn emit(&self, event: &str, data: &str) -> u64 {
        let id = EVENT_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        let line = json!({ "id": id, "event": event, "data": data });
        if let Ok(mut writer) = self.file.lock() {
            let _ = writeln!(writer, "{line}");
            let _ = writer.flush();
        }
        let payload = json!({
            "turnId": self.turn_id,
            "id": id,
            "event": event,
            "data": data,
        });
        crate::chat::push_bridge_event(JOB_EVENT_TOPIC, payload);
        id
    }

    fn emit_str(&self, event: &str, value: &str) -> u64 {
        let data = Value::String(value.to_string()).to_string();
        self.emit(event, &data)
    }
}

// ── POST /api/chat/stream ───────────────────────────────────────────────────

pub fn chat_stream(body: &Value) -> Result<Value, (u16, String)> {
    let mut message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let attachments_empty = body.get("attachments").and_then(Value::as_array).map(|items| items.is_empty()).unwrap_or(true);
    if message.is_empty() && attachments_empty {
        return Err((400, "消息不能为空".to_string()));
    }
    let attachments = body.get("attachments").cloned().unwrap_or(Value::Null);
    let context = crate::files::attachment_context(body.get("sessionId").and_then(Value::as_str), &attachments)
        .map_err(|error| (error.status, error.detail))?;
    if !context.is_empty() { message = format!("{message}\n\n{context}"); }
    if message.trim().is_empty() { return Err((400, "消息不能为空".to_string())); }
    let persona = body.get("persona").and_then(Value::as_str).unwrap_or_default().to_string();
    let session_key = body
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("session-{}", now_millis()));
    let turn_id = body
        .get("turnId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("turn-{}-{}", now_millis(), EVENT_SEQ.fetch_add(1, Ordering::Relaxed)));

    let busy = session_busy()
        .lock()
        .ok()
        .and_then(|guard| guard.get(&session_key).cloned());
    if let Some(flag) = busy {
        if flag.load(Ordering::SeqCst) {
            return Err((409, "此会话正在执行,请等待本轮完成".to_string()));
        }
    }
    let flag = {
        let mut guard = session_busy()
            .lock()
            .map_err(|_| (500, "任务表初始化失败".to_string()))?;
        guard
            .entry(session_key.clone())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone()
    };
    flag.store(true, Ordering::SeqCst);

    let handle = Arc::new(JobHandle {
        session_key: session_key.clone(),
        aborted: Arc::new(AtomicBool::new(false)),
        interrupt: Mutex::new(None),
    });
    jobs().lock().ok().map(|mut guard| guard.insert(turn_id.clone(), handle.clone()));

    let attachments = body.get("attachments").cloned().unwrap_or(Value::Null);
    let skill_name = body.get("skillName").and_then(Value::as_str).unwrap_or_default().to_string();

    let job_turn_id = turn_id.clone();
    let job_session_key = session_key.clone();
    std::thread::Builder::new()
        .name(format!("easel-job-{turn_id}"))
        .spawn(move || {
            let outcome = run_job(
                &job_turn_id,
                &job_session_key,
                &message,
                &persona,
                &skill_name,
                &attachments,
                flag.clone(),
                handle.clone(),
            );
            if let Err(message) = outcome {
                // 失败也记日志;事件流异常时前端至少能从 last/重放看到错误。
                eprintln!("[easel] job {job_turn_id} 失败:{message}");
            }
            flag.store(false, Ordering::SeqCst);
            jobs().lock().ok().map(|mut guard| guard.remove(&job_turn_id));
        })
        .map_err(|error| (500, format!("任务线程启动失败:{error}")))?;

    Ok(json!({ "turn_id": turn_id, "sessionKey": session_key }))
}

#[allow(clippy::too_many_arguments)]
fn run_job(
    turn_id: &str,
    session_key: &str,
    message: &str,
    persona: &str,
    skill_name: &str,
    attachments: &Value,
    _busy_flag: Arc<AtomicBool>,
    handle: Arc<JobHandle>,
) -> Result<(), String> {
    let log = EventLog::open(turn_id)?;
    let harness = load_harness();
    let persona_option = if persona.trim().is_empty() { None } else { Some(persona.trim()) };
    let user_message = {
        let mut composed = build_user_message(persona_option, message);
        if !skill_name.trim().is_empty() {
            composed = format!(
                "{composed}\n\n[本轮指定技能:{skill_name}] 请先读 skills/builtin/{skill_name}/SKILL.md 并按其执行。"
            );
        }
        if let Some(array) = attachments.as_array() {
            let names: Vec<String> = array
                .iter()
                .filter_map(|item| item.get("name").and_then(Value::as_str).map(str::to_string))
                .collect();
            if !names.is_empty() {
                composed = format!("{composed}\n\n[随附素材]{}", names.join("、"));
            }
        }
        composed
    };

    log.emit_str("activity", &format!("执行引擎:{}", harness.runtime));
    let outcome: Result<String, String> = match harness.runtime.as_str() {
        "codex" => run_codex_turn(
            &log,
            session_key,
            turn_id,
            &harness,
            persona_option,
            &user_message,
            handle,
        ),
        "api" => run_direct_turn(&log, turn_id, &harness, persona_option, &user_message),
        "openclaw" => Err("OpenClaw 引擎未随本插件提供;请在 设置 → Agent 底座 选择 Codex / 仅聊天模式".to_string()),
        other => Err(format!("执行引擎 {other} 的适配器将在后续批次接入")),
    };

    match outcome {
        Ok(text) => {
            save_last_turn(session_key, turn_id, "completed", &text);
            log.emit("done", &json!({ "sessionKey": session_key }).to_string());
        }
        Err(message) if message == "__aborted__" => {
            save_last_turn(session_key, turn_id, "stopped", "");
            log.emit("done", &json!({ "sessionKey": session_key }).to_string());
        }
        Err(message) => {
            save_last_turn(session_key, turn_id, "failed", &message);
            log.emit_str("error", &message);
        }
    }
    Ok(())
}

fn save_last_turn(session_key: &str, turn_id: &str, status: &str, text: &str) {
    let dir = sessions_dir().join(session_key);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let payload = json!({ "status": status, "text": text, "turn_id": turn_id });
    let _ = std::fs::write(dir.join("last.json"), payload.to_string());
}

// ── codex 执行底座 ──────────────────────────────────────────────────────────

fn codex_session(session_key: &str) -> Result<Arc<CodexSession>, String> {
    if let Ok(guard) = codex_sessions().lock() {
        if let Some(session) = guard.get(session_key) {
            return Ok(session.clone());
        }
    }
    let binary = resolve_codex_binary();
    if binary == "codex" && !PathBuf::from("codex").is_file() {
        // PATH 上没有时仍交给 codex 自身报错;已知安装位置缺失则明确提示。
        let home = std::env::var("HOME").unwrap_or_default();
        let bundled = PathBuf::from(&home).join(".codex/bin/codex");
        let app_bundle = PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex");
        if !bundled.is_file() && !app_bundle.is_file() {
            return Err("未找到 codex CLI:请安装(npm install -g @openai/codex)或设置 OTOOLS_CODEX_PATH".to_string());
        }
    }
    let root = workspace::ensure_ready()?;
    let (server, events_rx) = AppServer::spawn(SpawnOptions {
        binary,
        cwd: root,
        env: HashMap::new(),
        args: vec!["app-server".to_string()],
    })
    .map_err(|error| error.to_string())?;
    let session = Arc::new(CodexSession {
        server: Arc::new(tokio::sync::Mutex::new(server)),
        thread_id: Mutex::new(None),
        events_rx: tokio::sync::Mutex::new(events_rx),
        busy: AtomicBool::new(false),
    });
    codex_sessions()
        .lock()
        .ok()
        .map(|mut guard| guard.insert(session_key.to_string(), session.clone()));
    Ok(session)
}

fn resolve_codex_binary() -> String {
    if let Ok(path) = std::env::var("OTOOLS_CODEX_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() && PathBuf::from(trimmed).is_file() {
            return trimmed.to_string();
        }
    }
    let path_env = std::env::var("PATH").unwrap_or_default();
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    for dir in path_env.split(['/', '\\']) {
        let candidate = PathBuf::from(dir).join(name);
        if candidate.is_file() {
            return candidate.to_string_lossy().to_string();
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let mut known = vec![PathBuf::from(&home).join(".codex").join("bin").join("codex")];
    if cfg!(target_os = "macos") {
        known.push(PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"));
    }
    known
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "codex".to_string())
}

#[allow(clippy::too_many_arguments)]
fn run_codex_turn(
    log: &EventLog,
    session_key: &str,
    _turn_id: &str,
    harness: &crate::harness::HarnessSettings,
    persona: Option<&str>,
    user_message: &str,
    handle: Arc<JobHandle>,
) -> Result<String, String> {
    if harness.sandbox != "read-only" && harness.sandbox != "workspace-write" {
        return Err(format!("不支持的沙箱策略:{}", harness.sandbox));
    }
    let session = codex_session(session_key)?;
    if session
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("此 Agent 会话正在执行,请等待本轮完成".to_string());
    }

    let instructions = developer_instructions("codex", persona);
    let model = harness.model("codex");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let outcome = runtime.block_on(async {
        let mut events = session.events_rx.lock().await;
        {
            let mut server = session.server.lock().await;
            server.ensure_initialized().await.map_err(|error| error.to_string())?;
        }
        let root = workspace::ensure_ready()?;
        let options = agent::thread_options(&root, &harness.sandbox, &instructions, model.as_deref())
            .map_err(|error| error.to_string())?;
        let thread_id = {
            let existing = session.thread_id.lock().ok().and_then(|guard| guard.clone());
            let server = session.server.lock().await;
            match existing {
                Some(thread_id) => server
                    .thread_resume(&thread_id, &options, Duration::from_secs(60))
                    .await
                    .map_err(|error| error.to_string())?,
                None => {
                    let thread_id = server
                        .thread_start(&options, Duration::from_secs(60))
                        .await
                        .map_err(|error| error.to_string())?;
                    if let Ok(mut guard) = session.thread_id.lock() {
                        *guard = Some(thread_id.clone());
                    }
                    thread_id
                }
            }
        };
        let policy = agent::sandbox_policy(&harness.sandbox, &root, false).map_err(|error| error.to_string())?;
        let agent_turn_id = {
            let server = session.server.lock().await;
            server
                .turn_start(&thread_id, user_message, &policy, model.as_deref(), Duration::from_secs(60))
                .await
                .map_err(|error| error.to_string())?
        };
        if let Ok(mut guard) = handle.interrupt.lock() {
            *guard = Some((thread_id.clone(), agent_turn_id.clone()));
        }

        let deadline = std::time::Instant::now() + Duration::from_secs(1800);
        let mut last_heartbeat = std::time::Instant::now();
        let mut text = String::new();
        let mut saw_agent_message = false;
        loop {
            if handle.aborted.load(Ordering::SeqCst) {
                return Err("__aborted__".to_string());
            }
            if std::time::Instant::now() >= deadline {
                return Err("Agent 本轮超时(1800s)".to_string());
            }
            if last_heartbeat.elapsed() >= Duration::from_secs(HEARTBEAT_SECS) {
                log.emit_str("heartbeat", "运行中");
                last_heartbeat = std::time::Instant::now();
            }
            let event = match tokio::time::timeout(Duration::from_millis(500), events.recv()).await {
                Ok(Some(event)) => event,
                Ok(None) => return Err("Agent 协议连接已断开".to_string()),
                Err(_) => continue,
            };
            match event {
                AgentEvent::TextDelta { delta, turn_id: event_turn, .. } => {
                    if event_turn_is(&event_turn, &agent_turn_id) {
                        saw_agent_message = true;
                        text.push_str(&delta);
                        log.emit_str("token", &delta);
                    }
                }
                AgentEvent::ReasoningDelta { delta, turn_id: event_turn, .. } => {
                    if event_turn_is(&event_turn, &agent_turn_id) {
                        log.emit_str("thinking", &delta);
                    }
                }
                AgentEvent::ItemStarted { item, turn_id: event_turn, .. } => {
                    if event_turn_is(&event_turn, &agent_turn_id) {
                        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("tool");
                        if !matches!(item_type, "agentMessage" | "reasoning" | "userMessage") {
                            log.emit_str("activity", &format!("工具:{item_type}"));
                        }
                    }
                }
                AgentEvent::ItemCompleted { item, turn_id: event_turn, .. } => {
                    if item.get("type").and_then(Value::as_str) == Some("agentMessage")
                        && event_turn_is(&event_turn, &agent_turn_id)
                        && !saw_agent_message
                    {
                        let complete = item.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
                        text.push_str(&complete);
                        log.emit_str("token", &complete);
                    }
                }
                AgentEvent::ServerRequest { id, method, params } => {
                    if turn_id_of_params(&params) != Some(agent_turn_id.as_str()) {
                        continue;
                    }
                    handle_server_request(log, &session, &id, &method, &params, harness.codex_auto_approve).await;
                }
                AgentEvent::TurnCompleted { turn, .. } => {
                    if turn.get("id").and_then(Value::as_str) != Some(agent_turn_id.as_str()) {
                        continue;
                    }
                    let status = turn.get("status").and_then(Value::as_str).unwrap_or("");
                    if status == "completed" {
                        return Ok(text);
                    }
                    let message = turn
                        .get("error")
                        .and_then(|error| error.get("message"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("Agent 本轮未完成:{status}"));
                    return Err(message);
                }
                AgentEvent::Error { message } => return Err(message),
                AgentEvent::ReaderFailed { message } => return Err(message),
            }
        }
    });

    // (interrupt 已在 async 体内收口前尽力执行)
    if let Ok(mut guard) = handle.interrupt.lock() {
        *guard = None;
    }
    session.busy.store(false, Ordering::SeqCst);
    outcome
}

fn event_turn_is(event_turn: &Value, turn_id: &str) -> bool {
    match event_turn {
        Value::Null => true,
        value => value.as_str() == Some(turn_id),
    }
}

fn turn_id_of_params(params: &Value) -> Option<&str> {
    params.get("turnId").and_then(Value::as_str)
}

async fn handle_server_request(
    log: &EventLog,
    session: &Arc<CodexSession>,
    id: &Value,
    method: &str,
    params: &Value,
    auto_approve: bool,
) {
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            let decision = if auto_approve { "accept" } else { "decline" };
            let note = if auto_approve { "已自动通过普通操作授权" } else { "已拒绝授权(工作区外操作)" };
            log.emit_str("activity", note);
            session.server.lock().await.respond(id, &json!({ "decision": decision })).await.ok();
        }
        "item/tool/requestUserInput" => {
            // 问答题:注册挂起,经 question 事件转 Web,等待 answer 路由回包。
            let questions = params.get("questions").cloned().unwrap_or_else(|| json!([]));
            let question_id = format!("q-{}", now_millis());
            let payload = json!({
                "id": question_id,
                "questions": questions,
                "expiresAtMs": now_millis() + 10 * 60 * 1000,
            });
            log.emit("question", &payload.to_string());
            let (tx, rx) = std::sync::mpsc::channel();
            crate::chat::register_question(&question_id, tx);
            let server = session.server.clone();
            let _ = server.clone();
            let answers = tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(600)).ok())
                .await
                .unwrap_or_default()
                .unwrap_or_else(|| json!({ "answers": {} }));
            session.server.lock().await.respond(id, &answers).await.ok();
            log.emit("question_closed", &json!({ "id": question_id, "status": "answered" }).to_string());
        }
        _ => {
            log.emit_str("activity", "Agent 请求了暂不支持交互的确认,已拒绝");
            session.server.lock().await.reject(id, "Unsupported or invalid client request").await.ok();
        }
    }
}

// ── 仅聊天模式(直连模型 API)────────────────────────────────────────────────

fn run_direct_turn(
    log: &EventLog,
    turn_id: &str,
    _harness: &crate::harness::HarnessSettings,
    persona: Option<&str>,
    user_message: &str,
) -> Result<String, String> {
    let settings = direct_settings()
        .ok_or_else(|| "直连模型未配置:请到 OTools 设置 → AI → ai-switch 网关(或旧 AI 配置)完成配置".to_string())?;
    let system = developer_instructions("api", persona);
    let request = json!({
        "session_id": format!("direct-{turn_id}"),
        "provider": settings.provider,
        "base_url": settings.base_url,
        "api_key": settings.api_key,
        "model": settings.model,
        "messages": [{ "role": "user", "content": user_message }],
        "system": system,
        "temperature": 0.7,
    });
    let stream_request = parse_stream_request(&request)?;
    let parsed = build_body(&stream_request);
    let (url, bearer, api_key_header) =
        endpoint_for(&stream_request).map_err(|error| error.to_string())?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|error| error.to_string())?;
    let mut request_builder = client.post(&url).json(&parsed);
    if let Some(bearer) = bearer.filter(|value| !value.is_empty()) {
        request_builder = request_builder.bearer_auth(bearer);
    }
    if let Some(api_key) = api_key_header.filter(|value| !value.is_empty()) {
        request_builder = request_builder.header("api-key", api_key);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let response = request_builder
            .send()
            .await
            .map_err(|error| format!("请求模型服务失败:{error}"))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("模型服务拒绝({status}):{}", body.chars().take(300).collect::<String>()));
        }
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut text = String::new();
        let mut finished = false;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| format!("流中断:{error}"))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(position) = buffer.find('\n') {
                let line = buffer[..position].trim().to_string();
                buffer = buffer[position + 1..].to_string();
                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                let Some(data) = line.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data == "[DONE]" {
                    finished = true;
                    break;
                }
                if let Ok(value) = serde_json::from_str::<Value>(data) {
                    let (delta, reasoning, delta_finished) = feed_delta(&value);
                    if !reasoning.is_empty() {
                        log.emit_str("thinking", &reasoning);
                    }
                    if !delta.is_empty() {
                        text.push_str(&delta);
                        log.emit_str("token", &delta);
                    }
                    if delta_finished {
                        finished = true;
                    }
                }
            }
            if finished {
                break;
            }
        }
        if text.trim().is_empty() {
            return Err("模型返回空回复".to_string());
        }
        Ok(text)
    })
}

// chat.rs 的请求结构体复用:直接反序列化同一形状
fn parse_stream_request(value: &Value) -> Result<crate::chat::ChatStreamRequest, String> {
    serde_json::from_value(value.clone()).map_err(|error| format!("解析直连请求失败:{error}"))
}

// ── 其余聊天路由 ────────────────────────────────────────────────────────────

/// GET /api/chat/jobs/{turn_id}/events?after=N
pub fn job_events(turn_id: &str, after: u64) -> Result<Value, (u16, String)> {
    let path = jobs_dir().join(format!("{}.jsonl", turn_id.trim_start_matches('/')));
    if turn_id.contains("..") || !path.is_file() {
        return Err((404, "任务记录不存在".to_string()));
    }
    let text = std::fs::read_to_string(&path).map_err(|error| (500, error.to_string()))?;
    let mut events: Vec<Value> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            let id = value.get("id").and_then(Value::as_u64).unwrap_or(0);
            if id > after {
                events.push(json!({
                    "id": id,
                    "event": value.get("event").cloned().unwrap_or(Value::Null),
                    "data": value.get("data").cloned().unwrap_or(Value::Null),
                }));
            }
        }
    }
    Ok(json!({ "events": events }))
}

/// GET /api/chat/last/{session_id}?turn_id=
pub fn chat_last(session_key: &str) -> Result<Value, (u16, String)> {
    if session_key.contains("..") || session_key.trim().is_empty() {
        return Err((400, "非法会话标识".to_string()));
    }
    let path = sessions_dir().join(session_key.trim_start_matches('/')).join("last.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .map_err(|error| (500, error.to_string())),
        Err(_) => Err((404, "会话无记录".to_string())),
    }
}

/// POST /api/chat/stop
pub fn chat_stop(body: &Value) -> Result<Value, (u16, String)> {
    let turn_id = body.get("turnId").and_then(Value::as_str).unwrap_or_default().to_string();
    let session_key = body.get("sessionId").and_then(Value::as_str).unwrap_or_default().to_string();
    let mut stopped = false;
    if let Ok(guard) = jobs().lock() {
        for (job_turn, handle) in guard.iter() {
            let matched = (!turn_id.is_empty() && job_turn == &turn_id)
                || (!session_key.is_empty() && handle.session_key == session_key);
            if matched {
                handle.aborted.store(true, Ordering::SeqCst);
                stopped = true;
            }
        }
    }
    Ok(json!({ "stopped": stopped }))
}

/// POST /api/chat/question/answer
pub fn question_answer(body: &Value) -> Result<Value, (u16, String)> {
    let question_id = body.get("questionId").and_then(Value::as_str).unwrap_or_default().to_string();
    if question_id.is_empty() {
        return Err((400, "questionId 不能为空".to_string()));
    }
    let answers = body.get("answers").cloned().unwrap_or_else(|| json!({}));
    let resolved = crate::chat::answer_question(&question_id, json!({ "answers": answers }));
    if resolved {
        Ok(json!({ "ok": true, "status": "answered" }))
    } else {
        Ok(json!({ "ok": false, "error": "问答题不存在或已过期", "status": "missing" }))
    }
}

/// POST /api/chat/question/status
pub fn question_status(body: &Value) -> Result<Value, (u16, String)> {
    let ids: Vec<String> = body
        .get("questionIds")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({ "ok": true, "questions": crate::chat::questions_status(&ids) }))
}

/// POST /api/skill:技能单跑 = 以技能为指令的一轮任务(上游 easel skill 同语义)。
pub fn skill_run(body: &Value) -> Result<Value, (u16, String)> {
    let name = body.get("name").and_then(Value::as_str).unwrap_or_default().trim().to_string();
    if name.is_empty() || name.contains("..") || name.contains('/') {
        return Err((400, "非法技能名".to_string()));
    }
    let input = body.get("input").and_then(Value::as_str).unwrap_or_default().to_string();
    let persona = body.get("persona").and_then(Value::as_str).unwrap_or_default().to_string();
    let message = if input.trim().is_empty() {
        format!("请执行技能 {name}。")
    } else {
        format!("请执行技能 {name}。任务输入:{input}")
    };
    let mut job_body = json!({ "message": message, "persona": persona, "skillName": name });
    if let Some(session_id) = body.get("sessionId") {
        job_body["sessionId"] = session_id.clone();
    }
    chat_stream(&job_body)
}
