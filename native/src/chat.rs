//! 流式对话：OpenAI 兼容 /v1/chat/completions SSE + Azure 部署线协议。
//!
//! 与宿主 `ot-ai-generate-text` 的线协议保持一致（openai 家族 base_url 原样拼
//! `/chat/completions`；azure 走 deployments 路径 + api-key 头），但支持真多轮
//! messages 数组与逐 token 流式——这是仅聊天模式之上的插件自有能力。
//! 进度通过全局事件队列 `poll_events` 暴露给宿主前端（native 没有 Tauri handle）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const CHAT_EVENT: &str = "easel-chat-stream";

static EVENT_QUEUE: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static ABORTS: OnceLock<Mutex<HashMap<String, tokio::task::AbortHandle>>> = OnceLock::new();
static SESSION_SEQ: AtomicU64 = AtomicU64::new(1);

fn push_event(payload: Value) {
    push_bridge_event(CHAT_EVENT, payload);
}

/// 通用桥事件(chat 流与任务事件流共用同一队列,宿主 poll_events 抽干)。
pub(crate) fn push_bridge_event(topic: &str, payload: Value) {
    if let Ok(mut q) = EVENT_QUEUE.lock() {
        q.push(json!({ "topic": topic, "payload": payload }));
    }
}

// ── 问答题注册表(问答题桥:job 事件 → Web 回答 → codex 回包)───────────────

struct PendingQuestion {
    #[allow(dead_code)]
    created_ms: u128,
    responder: std::sync::mpsc::Sender<Value>,
}

static QUESTIONS: OnceLock<Mutex<HashMap<String, PendingQuestion>>> = OnceLock::new();

fn questions() -> &'static Mutex<HashMap<String, PendingQuestion>> {
    QUESTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn register_question(question_id: &str, responder: std::sync::mpsc::Sender<Value>) {
    if let Ok(mut guard) = questions().lock() {
        guard.insert(
            question_id.to_string(),
            PendingQuestion { created_ms: now_millis_pub(), responder },
        );
    }
}

pub(crate) fn answer_question(question_id: &str, answers: Value) -> bool {
    let pending = questions()
        .lock()
        .ok()
        .and_then(|mut guard| guard.remove(question_id));
    match pending {
        Some(entry) => entry.responder.send(answers).is_ok(),
        None => false,
    }
}

pub(crate) fn questions_status(ids: &[String]) -> Value {
    let mut result = serde_json::Map::new();
    if let Ok(guard) = questions().lock() {
        for id in ids {
            let status = if guard.contains_key(id) { "pending" } else { "missing" };
            result.insert(id.clone(), Value::String(status.to_string()));
        }
    }
    Value::Object(result)
}

pub(crate) fn now_millis_pub() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0)
}

/// 宿主事件桥：抽干缓冲的 UI 事件。
pub fn poll_events() -> Vec<Value> {
    EVENT_QUEUE
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

fn aborts() -> &'static Mutex<HashMap<String, tokio::task::AbortHandle>> {
    ABORTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 请求一次新的流式会话 id（前端发起每条消息时调用）。
pub fn next_session_id() -> String {
    format!("easel-s{}", SESSION_SEQ.fetch_add(1, Ordering::Relaxed))
}

/// 中止进行中的流（幂等：不存在时静默成功）。
pub fn abort_stream(session_id: &str) {
    if let Ok(mut map) = aborts().lock() {
        if let Some(handle) = map.remove(session_id) {
            handle.abort();
        }
    }
}

fn aborts_cleanup(session_id: &str) {
    if let Ok(mut map) = aborts().lock() {
        map.remove(session_id);
    }
}

// ── 请求/事件模型 ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ChatMessageInput {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatStreamRequest {
    pub session_id: String,
    /// openai | ollama | aliyun-bailian | azure（与宿主 ai_settings.provider 同名）
    #[serde(default)]
    pub provider: Option<String>,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    pub model: String,
    pub messages: Vec<ChatMessageInput>,
    #[serde(default)]
    pub system: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u16>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct StreamEvent {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,
    pub done: bool,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn emit(event: &StreamEvent) {
    push_event(serde_json::to_value(event).unwrap_or_else(|_| json!({ "topic": CHAT_EVENT })));
}

// ── 线协议（对齐宿主 ot-ai-generate-text）───────────────────────────────────

pub(crate) fn normalize_provider(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "aliyun-bailian" | "dashscope" | "aliyun" | "bailian" => "openai".to_string(),
        other => other.to_string(),
  }
}

/// 产出 (url, bearer_key, api_key_header)：与 async-openai 的 OpenAIConfig/AzureConfig 行为一致。
pub fn endpoint_for(req: &ChatStreamRequest) -> Result<(String, Option<String>, Option<String>), String> {
    let provider = normalize_provider(req.provider.as_deref().unwrap_or("openai"));
    let base = req.base_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return Err("Base URL 不能为空：请先在 OTools 设置 → AI 完成配置".to_string());
    }
    if provider == "azure" {
        // AzureConfig: {base}/openai/deployments/{model}/chat/completions?api-version=...
        let api_version = std::env::var("OT_EASEL_AZURE_API_VERSION")
            .unwrap_or_else(|_| "2024-02-01".to_string());
        let url = format!(
            "{base}/openai/deployments/{}/chat/completions?api-version={api_version}",
            req.model
        );
        return Ok((url, None, Some(req.api_key.clone().unwrap_or_default())));
    }
    Ok((format!("{base}/chat/completions"), Some(req.api_key.clone().unwrap_or_default()), None))
}

/// 构建上游请求体（真多轮 messages + 可选 system）。
pub fn build_body(req: &ChatStreamRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = req.system.as_deref() {
        let system = system.trim();
        if !system.is_empty() {
            messages.push(json!({ "role": "system", "content": system }));
        }
    }
    for message in &req.messages {
        messages.push(json!({ "role": message.role, "content": message.content }));
    }
    let mut body = json!({ "model": req.model, "messages": messages, "stream": true });
    if let Some(temperature) = req.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(max_tokens) = req.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    body
}

/// 从一行 chat/completions SSE 数据中提取增量。
/// 返回 (text 增量, reasoning 增量, 是否结束)。无法解析的行静默忽略。
pub fn feed_delta(value: &Value) -> (String, String, bool) {
    let choices = match value.get("choices").and_then(Value::as_array) {
        Some(choices) => choices,
        None => {
            // 非流式响应或错误负载里出现 finish 提示时同样收口。
            let finished = value.get("finished").and_then(Value::as_bool).unwrap_or(false);
            return (String::new(), String::new(), finished);
        }
    };
    let Some(first) = choices.first() else {
        return (String::new(), String::new(), false);
    };
    let delta = first.get("delta").cloned().unwrap_or(Value::Null);
    let text = delta
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let reasoning = delta
        .get("reasoning_content")
        .or_else(|| delta.get("reasoning"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let finished = first
        .get("finish_reason")
        .and_then(Value::as_str)
        .is_some_and(|reason| !reason.is_empty() && reason != "null");
    (text, reasoning, finished)
}

// ── 流式执行 ────────────────────────────────────────────────────────────────

fn fail(req: &ChatStreamRequest, message: &str) {
    emit(&StreamEvent {
        session_id: req.session_id.clone(),
        kind: "error".to_string(),
        content: String::new(),
        done: true,
        success: false,
        error: Some(message.to_string()),
    });
}

/// 启动流式任务并登记中止句柄（dispatch 层入口；进度走事件队列）。
pub fn spawn_chat_stream(req: ChatStreamRequest) {
    let session_id = req.session_id.clone();
    let handle = crate::runtime().spawn(async move {
        chat_stream(req).await;
    });
    if let Ok(mut map) = aborts().lock() {
        map.insert(session_id, handle.abort_handle());
    }
}

/// 发起一次流式对话（后台任务），进度通过事件队列暴露。
pub async fn chat_stream(req: ChatStreamRequest) {    let (url, bearer, api_key_header) = match endpoint_for(&req) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            fail(&req, &error);
            return;
        }
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            fail(&req, &format!("初始化 HTTP 客户端失败：{error}"));
            return;
        }
    };

    let mut request = client.post(&url).json(&build_body(&req));
    if let Some(bearer) = bearer.filter(|value| !value.is_empty()) {
        request = request.bearer_auth(bearer);
    }
    if let Some(api_key) = api_key_header.filter(|value| !value.is_empty()) {
        request = request.header("api-key", api_key);
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            fail(&req, &format!("请求模型服务失败：{error}"));
            aborts_cleanup(&req.session_id);
            return;
        }
    };

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let detail = if text.is_empty() {
            format!("模型服务拒绝，状态码 {status}")
        } else {
            format!("模型服务拒绝（{status}）：{}", text.chars().take(300).collect::<String>())
        };
        fail(&req, &detail);
        aborts_cleanup(&req.session_id);
        return;
    }

    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut finished = false;

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                fail(&req, &format!("流中断：{error}"));
                aborts_cleanup(&req.session_id);
                return;
            }
        };
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(position) = buffer.find('\n') {
            let line = buffer[..position].trim().to_string();
            buffer = buffer[position + 1..].to_string();
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                finished = true;
                break;
            }
            let Ok(value) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            let (text, reasoning, delta_finished) = feed_delta(&value);
            if !reasoning.is_empty() {
                emit(&StreamEvent {
                    session_id: req.session_id.clone(),
                    kind: "reasoning".to_string(),
                    content: reasoning,
                    done: false,
                    success: true,
                    error: None,
                });
            }
            if !text.is_empty() {
                emit(&StreamEvent {
                    session_id: req.session_id.clone(),
                    kind: "text".to_string(),
                    content: text,
                    done: false,
                    success: true,
                    error: None,
                });
            }
            if delta_finished {
                finished = true;
            }
        }
        if finished {
            break;
        }
    }

    emit(&StreamEvent {
        session_id: req.session_id.clone(),
        kind: "done".to_string(),
        content: String::new(),
        done: true,
        success: true,
        error: None,
    });
    aborts_cleanup(&req.session_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_delta_extracts_content_and_finish() {
        let value = serde_json::from_str::<Value>(
            r#"{"choices":[{"delta":{"content":"你好"},"finish_reason":null}]}"#,
        )
        .unwrap();
        let (text, reasoning, finished) = feed_delta(&value);
        assert_eq!(text, "你好");
        assert!(reasoning.is_empty());
        assert!(!finished);

        let value = serde_json::from_str::<Value>(
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        )
        .unwrap();
        let (text, reasoning, finished) = feed_delta(&value);
        assert!(text.is_empty() && reasoning.is_empty() && finished);
    }

    #[test]
    fn feed_delta_extracts_reasoning() {
        let value = serde_json::from_str::<Value>(
            r#"{"choices":[{"delta":{"reasoning_content":"思考中"}}]}"#,
        )
        .unwrap();
        let (text, reasoning, finished) = feed_delta(&value);
        assert!(text.is_empty());
        assert_eq!(reasoning, "思考中");
        assert!(!finished);
    }

    #[test]
    fn endpoint_openai_appends_chat_completions() {
        let req = ChatStreamRequest {
            session_id: "s1".into(),
            provider: Some("aliyun-bailian".into()),
            base_url: "https://dashscope.example/compatible-mode/v1/".into(),
            api_key: Some("sk-test".into()),
            model: "qwen-max".into(),
            messages: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
        };
        let (url, bearer, api_key_header) = endpoint_for(&req).unwrap();
        assert_eq!(url, "https://dashscope.example/compatible-mode/v1/chat/completions");
        assert_eq!(bearer.as_deref(), Some("sk-test"));
        assert!(api_key_header.is_none());
    }

    #[test]
    fn endpoint_azure_uses_deployments_path() {
        let req = ChatStreamRequest {
            session_id: "s1".into(),
            provider: Some("azure".into()),
            base_url: "https://demo.openai.azure.com".into(),
            api_key: Some("az-key".into()),
            model: "gpt-4o".into(),
            messages: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
        };
        let (url, bearer, api_key_header) = endpoint_for(&req).unwrap();
        assert_eq!(
            url,
            "https://demo.openai.azure.com/openai/deployments/gpt-4o/chat/completions?api-version=2024-02-01"
        );
        assert!(bearer.is_none());
        assert_eq!(api_key_header.as_deref(), Some("az-key"));
    }

    #[test]
    fn endpoint_rejects_empty_base_url() {
        let req = ChatStreamRequest {
            session_id: "s1".into(),
            provider: None,
            base_url: "  ".into(),
            api_key: None,
            model: "m".into(),
            messages: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
        };
        assert!(endpoint_for(&req).is_err());
    }

    #[test]
    fn build_body_prepends_system_and_keeps_history() {
        let req = ChatStreamRequest {
            session_id: "s1".into(),
            provider: None,
            base_url: "http://127.0.0.1:11434/v1".into(),
            api_key: Some("ollama".into()),
            model: "qwen2.5:14b".into(),
            messages: vec![
                ChatMessageInput { role: "user".into(), content: "第一问".into() },
                ChatMessageInput { role: "assistant".into(), content: "第一答".into() },
                ChatMessageInput { role: "user".into(), content: "第二问".into() },
            ],
            system: Some("你是 Easel。".into()),
            temperature: Some(0.7),
            max_tokens: Some(1024),
        };
        let body = build_body(&req);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["content"], "第一问");
        assert_eq!(messages[3]["content"], "第二问");
        assert_eq!(body["stream"], true);
        assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 1e-6);
        assert_eq!(body["max_tokens"], 1024);
    }

    #[test]
    fn events_are_queued_and_drained() {
        let before = poll_events().len();
        emit(&StreamEvent {
            session_id: "s-test".into(),
            kind: "text".into(),
            content: "hi".into(),
            done: false,
            success: true,
            error: None,
        });
        let drained = poll_events();
        assert_eq!(drained.len(), before + 1);
        let envelope = &drained[drained.len() - 1];
        assert_eq!(envelope["topic"], CHAT_EVENT);
        assert_eq!(envelope["payload"]["sessionId"], "s-test");
        assert_eq!(envelope["payload"]["kind"], "text");
        // 再抽一次应为空
        assert!(poll_events().is_empty());
    }
}

#[cfg(test)]
mod e2e {
    use super::*;

    /// 端到端:本地 mock SSE 服务 → chat_stream → 事件队列出现 text/reasoning/done。
    #[tokio::test(flavor = "multi_thread")]
    async fn chat_stream_parses_sse_end_to_end() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"想\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        });

        let request = ChatStreamRequest {
            session_id: "t-e2e".into(),
            provider: Some("openai".into()),
            base_url: format!("http://{addr}/v1"),
            api_key: Some("k".into()),
            model: "test-model".into(),
            messages: vec![ChatMessageInput { role: "user".into(), content: "hi".into() }],
            system: Some("sys".into()),
            temperature: Some(0.5),
            max_tokens: None,
        };
        chat_stream(request).await;
        server.join().unwrap();

        // 事件队列是全局的,并行测试可能并发抽干;循环收集直到出现 done 或超时。
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut kinds: Vec<String> = Vec::new();
        while std::time::Instant::now() < deadline {
            for envelope in poll_events() {
                let payload = &envelope["payload"];
                if payload["sessionId"] == "t-e2e" {
                    kinds.push(payload["kind"].as_str().unwrap_or_default().to_string());
                }
            }
            if kinds.last().map(|kind| kind == "done").unwrap_or(false) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(kinds.contains(&"text".to_string()), "缺少 text 事件: {kinds:?}");
        assert!(kinds.contains(&"reasoning".to_string()), "缺少 reasoning 事件: {kinds:?}");
        assert_eq!(kinds.last().map(String::as_str), Some("done"), "事件顺序应以 done 收口: {kinds:?}");
    }
}
