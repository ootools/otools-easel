//! codex app-server 私有 stdio JSON-RPC 帧协议(换行分隔 JSON,无 jsonrpc 2.0 信封;
//! 通用 ACP 才带信封,见 Easel harnesses/rpc.py 同源约定)。
//!
//! 消息三类:
//! - 请求(带 id+method):client→server 与 server→client 双向
//! - 通知(仅 method):单向事件
//! - 响应(仅 id):对端请求的回包

use serde_json::{Map, Value};

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("无效 JSON-RPC 数据")]
    InvalidJson(#[source] serde_json::Error),
    #[error("消息不是 JSON 对象")]
    NotAnObject,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// 对端发来的请求(需要回包)
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// 对端通知(单向事件)
    Notification { method: String, params: Value },
    /// 我方请求的响应
    Response {
        id: Value,
        result: Option<Value>,
        error: Option<RpcError>,
    },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("RPC 错误 {code}: {message}")]
pub struct RpcError {
    pub code: Value,
    pub message: String,
}

/// 解析一行换行分隔的 JSON-RPC 消息。
pub fn parse_line(line: &str) -> Result<Incoming, ProtocolError> {
    let value: Value = serde_json::from_str(line).map_err(ProtocolError::InvalidJson)?;
    let map = value.as_object().ok_or(ProtocolError::NotAnObject)?;
    let method = map
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let id = map.get("id").cloned().filter(|v| !v.is_null());

    match (method, id) {
        (Some(method), Some(id)) => Ok(Incoming::Request {
            id,
            method,
            params: params_of(map),
        }),
        (Some(method), None) => Ok(Incoming::Notification {
            method,
            params: params_of(map),
        }),
        (None, Some(id)) => {
            let error = map
                .get("error")
                .and_then(Value::as_object)
                .map(|error| RpcError {
                    code: error.get("code").cloned().unwrap_or(Value::Null),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                });
            Ok(Incoming::Response {
                id,
                result: map.get("result").cloned(),
                error,
            })
        }
        (None, None) => Err(ProtocolError::NotAnObject),
    }
}

fn params_of(map: &Map<String, Value>) -> Value {
    map.get("params").cloned().unwrap_or(Value::Null)
}

/// 我方请求帧。
pub fn request_frame(id: i64, method: &str, params: &Value) -> String {
    serde_json::json!({ "id": id, "method": method, "params": params }).to_string()
}

/// 我方通知帧。
pub fn notification_frame(method: &str, params: &Value) -> String {
    serde_json::json!({ "method": method, "params": params }).to_string()
}

/// 对对端请求的成功回包。
pub fn success_frame(id: &Value, result: &Value) -> String {
    serde_json::json!({ "id": id, "result": result }).to_string()
}

/// 对对端请求的错误回包。
pub fn error_frame(id: &Value, code: i64, message: &str) -> String {
    serde_json::json!({ "id": id, "error": { "code": code, "message": message } }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_request_notification_response() {
        let incoming = parse_line(r#"{"id":7,"method":"item/commandExecution/requestApproval","params":{"command":"ls"}}"#).unwrap();
        match incoming {
            Incoming::Request { id, method, params } => {
                assert_eq!(id, json!(7));
                assert_eq!(method, "item/commandExecution/requestApproval");
                assert_eq!(params, json!({"command":"ls"}));
            }
            other => panic!("期望 Request,实际 {other:?}"),
        }

        let incoming =
            parse_line(r#"{"method":"item/agentMessage/delta","params":{"delta":"hi"}}"#).unwrap();
        assert!(matches!(incoming, Incoming::Notification { .. }));

        let incoming = parse_line(r#"{"id":7,"result":{"thread":{"id":"th"}}}"#).unwrap();
        match incoming {
            Incoming::Response { result, error, .. } => {
                assert_eq!(result, Some(json!({"thread":{"id":"th"}})));
                assert!(error.is_none());
            }
            other => panic!("期望 Response,实际 {other:?}"),
        }
    }

    #[test]
    fn parses_error_response_and_null_id() {
        let incoming =
            parse_line(r#"{"id":3,"error":{"code":-32602,"message":"bad params"}}"#).unwrap();
        match incoming {
            Incoming::Response { error, .. } => {
                assert_eq!(error.unwrap().code, json!(-32602));
            }
            other => panic!("期望 Response,实际 {other:?}"),
        }
        // id 为 null 视作通知(codex 全局 error 事件常带 null id)
        let incoming = parse_line(r#"{"method":"error","id":null,"params":{}}"#).unwrap();
        assert!(matches!(incoming, Incoming::Notification { .. }));
    }

    #[test]
    fn rejects_non_object_lines() {
        assert!(parse_line("not json").is_err());
        assert!(parse_line("[1,2]").is_err());
        assert!(parse_line("\"str\"").is_err());
    }

    #[test]
    fn frames_roundtrip() {
        let frame = request_frame(1, "initialize", &json!({"clientInfo":{"name":"otools"}}));
        assert!(parse_line(&frame).is_ok());
        let frame = notification_frame("initialized", &json!({}));
        assert!(parse_line(&frame).is_ok());
        let frame = success_frame(&json!(9), &json!({"decision":"accept"}));
        assert!(parse_line(&frame).is_ok());
        let frame = error_frame(&json!(9), -32601, "unsupported");
        assert!(parse_line(&frame).is_ok());
    }
}
