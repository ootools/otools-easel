//! codex app-server 客户端:spawn `codex app-server` 子进程,stdio JSON-RPC 双向通道。
//! (vendored 自 OTools 宿主 crates/ot-agent,随上游同步)
//!
//! 事件模型(对端通知,经 events 通道转发):
//! - `AgentEvent::TextDelta` / `ReasoningDelta`:`item/agentMessage/delta`、`item/reasoning/*`
//! - `AgentEvent::ItemStarted` / `ItemCompleted`:`item/started`、`item/completed`
//! - `AgentEvent::TurnCompleted`:`turn/completed`
//! - `AgentEvent::Error`:`error` 通知
//! - `AgentEvent::ServerRequest`:对端请求(权限确认/问答题),**消费方必须**用
//!   [`AppServer::respond`] 回包,否则对端会挂起等待
//!
//! 线协议细节与 Easel `easel/harnesses/codex.py` 同源:initialize → initialized →
//! thread/start|resume → turn/start,turn 期间事件按 threadId/turnId 关联由消费方过滤。

use super::protocol::{
    error_frame, notification_frame, parse_line, request_frame, success_frame, Incoming,
    ProtocolError, RpcError,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::time;

pub const CLIENT_NAME: &str = "otools";

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("无法启动 Agent CLI({0}):请检查安装与 PATH")]
    Spawn(String, #[source] std::io::Error),
    #[error("Agent 进程已退出,请检查 CLI 登录与模型配置")]
    Exited,
    #[error("Agent 协议错误:{0}")]
    Protocol(String),
    #[error("Agent 协议请求超时:{0}")]
    Timeout(String),
    #[error("RPC 错误:{0}")]
    Rpc(#[from] RpcError),
}

/// 透传给 codex 子进程的事件负载。
#[derive(Debug, Clone)]
pub struct SpawnOptions {
    pub binary: String,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    /// 追加到命令行的参数,默认 ["app-server"]
    pub args: Vec<String>,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            binary: "codex".to_string(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            env: HashMap::new(),
            args: vec!["app-server".to_string()],
        }
    }
}

/// 发往消费方的事件(通知与对端请求)。
#[derive(Debug, Clone)]
pub enum AgentEvent {
    TextDelta {
        thread_id: Value,
        turn_id: Value,
        delta: String,
    },
    ReasoningDelta {
        thread_id: Value,
        turn_id: Value,
        delta: String,
    },
    ItemStarted {
        thread_id: Value,
        turn_id: Value,
        item: Value,
    },
    ItemCompleted {
        thread_id: Value,
        turn_id: Value,
        item: Value,
    },
    TurnCompleted {
        thread_id: Value,
        turn: Value,
    },
    /// 对端 error 通知(willRetry 的可忽略,由消费方判断)
    Error {
        message: String,
    },
    /// 对端请求(权限确认/问答题),必须 respond
    ServerRequest {
        id: Value,
        method: String,
        params: Value,
    },
    /// 协议读取失败,会话已不可用
    ReaderFailed {
        message: String,
    },
}

struct PendingCall {
    sender: oneshot::Sender<Result<Value, AgentError>>,
}

pub struct AppServer {
    child: Arc<Mutex<Child>>,
    stdin_tx: mpsc::UnboundedSender<String>,
    pending: Arc<Mutex<HashMap<i64, PendingCall>>>,
    next_id: AtomicI64,
    /// initialize 是否已完成(Easel codex.py 的 rpc.initialized 语义)
    initialized: bool,
}

impl AppServer {
    /// 启动子进程并开始事件泵。消费方通过 `events` 通道收事件。
    pub fn spawn(
        options: SpawnOptions,
    ) -> Result<(Self, mpsc::UnboundedReceiver<AgentEvent>), AgentError> {
        let mut command = Command::new(&options.binary);
        command
            .args(&options.args)
            .current_dir(&options.cwd)
            .envs(&options.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        {
            // 与宿主同进程组的插件子进程需要独立进程组,便于整组终止。
            command.process_group(0);
        }

        let mut child = command
            .spawn()
            .map_err(|error| AgentError::Spawn(options.binary.clone(), error))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentError::Protocol("子进程缺少 stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::Protocol("子进程缺少 stdout".to_string()))?;
        // stderr 直接排干,避免管道写满阻塞子进程。
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut buffer = Vec::new();
                loop {
                    match reader.read_until(b'\n', &mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => buffer.clear(),
                    }
                }
            });
        }

        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();

        // stdin 写任务:串行写,保证帧完整。
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(frame) = stdin_rx.recv().await {
                if stdin.write_all(frame.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let pending: Arc<Mutex<HashMap<i64, PendingCall>>> = Arc::new(Mutex::new(HashMap::new()));

        // stdout 读任务:解帧并路由。
        let pending_for_reader = pending.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                match parse_line(line.trim_end()) {
                    Ok(incoming) => match incoming {
                        Incoming::Response { id, result, error } => {
                            let Some(number) = id.as_i64() else { continue };
                            let sender = pending_for_reader
                                .lock()
                                .ok()
                                .and_then(|mut guard| guard.remove(&number))
                                .map(|pending| pending.sender);
                            if let Some(sender) = sender {
                                let outcome = match error {
                                    Some(error) => Err(AgentError::Rpc(error)),
                                    None => Ok(result.unwrap_or(Value::Null)),
                                };
                                let _ = sender.send(outcome);
                            }
                        }
                        Incoming::Notification { method, params } => {
                            let event = map_notification(&method, params);
                            if let Some(event) = event {
                                let _ = events_tx.send(event);
                            }
                        }
                        Incoming::Request { id, method, params } => {
                            let _ =
                                events_tx.send(AgentEvent::ServerRequest { id, method, params });
                        }
                    },
                    Err(ProtocolError::InvalidJson(_)) | Err(ProtocolError::NotAnObject) => {
                        let _ = events_tx.send(AgentEvent::Error {
                            message: "Agent 返回了无效 JSON-RPC 数据".to_string(),
                        });
                        break;
                    }
                }
            }
            // 通道关闭:让所有挂起调用失败。
            if let Ok(mut guard) = pending_for_reader.lock() {
                for (_, pending) in guard.drain() {
                    let _ = pending.sender.send(Err(AgentError::Exited));
                }
            }
            let _ = events_tx.send(AgentEvent::ReaderFailed {
                message: "Agent 协议连接已断开,请检查 CLI 登录、模型与版本".to_string(),
            });
        });

        Ok((
            Self {
                child: Arc::new(Mutex::new(child)),
                stdin_tx,
                pending,
                next_id: AtomicI64::new(1),
                initialized: false,
            },
            events_rx,
        ))
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub async fn terminate(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.start_kill();
        }
    }

    fn allocate_id(&self) -> i64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn send_raw(&self, frame: String) -> Result<(), AgentError> {
        self.stdin_tx.send(frame).map_err(|_| AgentError::Exited)
    }

    /// 发起一次请求并等待响应。
    pub async fn call(
        &self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Value, AgentError> {
        let id = self.allocate_id();
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .ok()
            .map(|mut guard| guard.insert(id, PendingCall { sender: tx }));
        self.send_raw(request_frame(id, method, params)).await?;
        match time::timeout(timeout, rx).await {
            Err(_) => {
                if let Ok(mut guard) = self.pending.lock() {
                    guard.remove(&id);
                }
                Err(AgentError::Timeout(method.to_string()))
            }
            Ok(Err(_)) => Err(AgentError::Exited),
            Ok(Ok(outcome)) => outcome,
        }
    }

    /// 发送通知帧(initialized 等)。
    pub async fn notify(&self, method: &str, params: &Value) -> Result<(), AgentError> {
        self.send_raw(notification_frame(method, params)).await
    }

    /// 回应对端请求(权限确认/问答题)。
    pub async fn respond(&self, id: &Value, result: &Value) -> Result<(), AgentError> {
        self.send_raw(success_frame(id, result)).await
    }

    /// 回绝对端请求(不支持的交互)。
    pub async fn reject(&self, id: &Value, message: &str) -> Result<(), AgentError> {
        self.send_raw(error_frame(id, -32601, message)).await
    }

    /// initialize + initialized 握手(幂等)。
    pub async fn ensure_initialized(&mut self) -> Result<(), AgentError> {
        if self.initialized {
            return Ok(());
        }
        let params = json!({
            "clientInfo": { "name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "experimentalApi": true },
        });
        self.call("initialize", &params, Duration::from_secs(60))
            .await?;
        self.notify("initialized", &json!({})).await?;
        self.initialized = true;
        Ok(())
    }

    /// thread/start:cwd、approvalPolicy、sandbox、developerInstructions(常驻指令)、可选 model。
    pub async fn thread_start(
        &self,
        options: &Value,
        timeout: Duration,
    ) -> Result<String, AgentError> {
        let result = self.call("thread/start", options, timeout).await?;
        thread_id_of(&result)
    }

    /// thread/resume:按原生会话 ID 续接。
    pub async fn thread_resume(
        &self,
        thread_id: &str,
        options: &Value,
        timeout: Duration,
    ) -> Result<String, AgentError> {
        let mut params = options.clone();
        params["threadId"] = json!(thread_id);
        let result = self.call("thread/resume", &params, timeout).await?;
        thread_id_of(&result)
    }

    /// turn/start:发起一轮。事件(含早到的)由消费方按 turn_id 关联。
    pub async fn turn_start(
        &self,
        thread_id: &str,
        prompt: &str,
        sandbox_policy: &Value,
        model: Option<&str>,
        timeout: Duration,
    ) -> Result<String, AgentError> {
        let mut params = json!({
            "threadId": thread_id,
            "input": [{ "type": "text", "text": prompt }],
            "approvalPolicy": "on-request",
            "sandboxPolicy": sandbox_policy,
        });
        if let Some(model) = model {
            params["model"] = json!(model);
        }
        let result = self.call("turn/start", &params, timeout).await?;
        result
            .get("turn")
            .and_then(|turn| turn.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| AgentError::Protocol("turn/start 未返回本轮 ID".to_string()))
    }

    /// turn/interrupt:中止进行中的一轮(幂等失败静默)。
    pub async fn turn_interrupt(&self, thread_id: &str, turn_id: &str) -> Result<(), AgentError> {
        let params = json!({ "threadId": thread_id, "turnId": turn_id });
        match self
            .call("turn/interrupt", &params, Duration::from_secs(5))
            .await
        {
            Ok(_) => Ok(()),
            Err(AgentError::Timeout(_)) | Err(AgentError::Exited) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// model/list:原生配置可见模型(设置页"拉取模型"用)。
    pub async fn model_list(&self) -> Result<Value, AgentError> {
        self.call("model/list", &json!({}), Duration::from_secs(60))
            .await
    }
}

fn thread_id_of(result: &Value) -> Result<String, AgentError> {
    result
        .get("thread")
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AgentError::Protocol("thread/start 未返回原生会话 ID".to_string()))
}

fn map_notification(method: &str, params: Value) -> Option<AgentEvent> {
    let thread_id = params.get("threadId").cloned().unwrap_or(Value::Null);
    let turn_id = params.get("turnId").cloned().unwrap_or(Value::Null);
    match method {
        "item/agentMessage/delta" => Some(AgentEvent::TextDelta {
            thread_id,
            turn_id,
            delta: params
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            Some(AgentEvent::ReasoningDelta {
                thread_id,
                turn_id,
                delta: params
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        }
        "item/started" => Some(AgentEvent::ItemStarted {
            thread_id,
            turn_id,
            item: params.get("item").cloned().unwrap_or(Value::Null),
        }),
        "item/completed" => Some(AgentEvent::ItemCompleted {
            thread_id,
            turn_id,
            item: params.get("item").cloned().unwrap_or(Value::Null),
        }),
        "turn/completed" => Some(AgentEvent::TurnCompleted {
            thread_id,
            turn: params.get("turn").cloned().unwrap_or(Value::Null),
        }),
        "error" => {
            // codex 的 error 通知形状随版本变化:优先 message 字段,否则带原始负载。
            let message = params
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("Agent 报告错误:{}", params));
            Some(AgentEvent::Error { message })
        }
        _ => None,
    }
}
