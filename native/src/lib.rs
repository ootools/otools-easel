//! `otools-easel` 的 native 入口（cdylib）。
//!
//! 导出 `otools_plugin_invoke` / `otools_plugin_free` 供 OTools 宿主按
//! `lib/Windows.dll` / `lib/macOS.dylib` / `lib/Linux.so` 加载。边界内用
//! `catch_unwind` 兜住 panic（否则会 `process::abort` 拖垮宿主）——与
//! `ot-plugin-ffi` 的守卫等价。本仓库故意不依赖 otools 单体仓里的 crate，
//! 以便独立 CI 能直接 `cargo build`（同 free-model-otools 的做法）。

pub mod agent;
pub mod chat;
pub mod chat_routes;
pub mod environment;
pub mod files;
pub mod harness;
pub mod process;
pub mod publishing;
pub mod settings;
pub mod skills;
pub mod router;
pub mod storage;
pub mod trends;
pub mod workspace;

use serde_json::{json, Value};
use std::sync::OnceLock;

/// 全局多线程 runtime：native 没有 Tauri handle，所有异步 IO 都靠它。
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("创建 native runtime 失败")
    })
}

pub fn dispatch(method: &str, payload: Value) -> Value {
    macro_rules! ok {
        ($expr:expr) => {
            json!({ "ok": true, "data": $expr })
        };
    }

    match method {
        "easel_ping" => ok!(json!({ "pong": true })),
        "easel_next_session" => ok!(json!({ "sessionId": chat::next_session_id() })),
        "easel_chat_stream" => {
            let req: chat::ChatStreamRequest = match serde_json::from_value(payload) {
                Ok(req) => req,
                Err(error) => {
                    return json!({ "ok": false, "error": format!("解析请求失败：{error}") });
                }
            };
            chat::spawn_chat_stream(req);
            json!({ "ok": true, "data": { "started": true } })
        }
        "api" => {
            let method = payload.get("method").and_then(Value::as_str).unwrap_or("GET").to_string();
            let path = payload.get("path").and_then(Value::as_str).unwrap_or("/").to_string();
            let body = payload.get("body").cloned().unwrap_or(Value::Null);
            match router::route(&method, &path, &body) {
                Ok(data) => json!({ "ok": true, "data": data }),
                Err(error) => json!({ "ok": false, "error": error.detail, "status": error.status }),
            }
        }
        "easel_chat_abort" => {
            let session_id = payload
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            chat::abort_stream(&session_id);
            ok!(json!({ "aborted": true }))
        }
        "poll_events" => ok!(chat::poll_events()),
        "__otools_shutdown__" => {
            // 运行时随宿主进程退出自然回收；这里只清空中止表。
            ok!(json!({ "stopped": true }))
        }
        _ => json!({ "ok": false, "error": format!("未知方法：{method}") }),
    }
}

// ── FFI 边界 ──────────────────────────────────────────────────────────────

fn invoke_impl(input_ptr: *const u8, input_len: usize, output_len: *mut usize) -> *mut u8 {
    if input_ptr.is_null() || output_len.is_null() {
        return std::ptr::null_mut();
    }
    let input = unsafe { std::slice::from_raw_parts(input_ptr, input_len) };
    let parsed: Value = serde_json::from_slice(input)
        .unwrap_or_else(|error| json!({ "ok": false, "error": format!("非法输入：{error}") }));
    let (method, payload, plugin_dir) = match &parsed {
        Value::Object(map) => (
            map.get("method").and_then(Value::as_str).unwrap_or("").to_string(),
            map.get("payload").cloned().unwrap_or(Value::Null),
            map.get("pluginDir").and_then(Value::as_str).unwrap_or("").to_string(),
        ),
        _ => (String::new(), Value::Null, String::new()),
    };
    if !plugin_dir.is_empty() {
        workspace::set_plugin_dir(&plugin_dir);
    }
    let response = dispatch(&method, payload);
    let mut output = serde_json::to_vec(&response)
        .unwrap_or_else(|_| br#"{"ok":false,"error":"serialize failed"}"#.to_vec());
    let len = output.len();
    unsafe {
        *output_len = len;
    }
    let ptr = output.as_mut_ptr();
    std::mem::forget(output);
    ptr
}

#[no_mangle]
pub extern "C" fn otools_plugin_invoke(
    input_ptr: *const u8,
    input_len: usize,
    output_len: *mut usize,
) -> *mut u8 {
    // 跨 FFI 边界绝不能让 panic unwind 到宿主（会 abort）。catch_unwind 兜底。
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        invoke_impl(input_ptr, input_len, output_len)
    }));
    match result {
        Ok(ptr) => ptr,
        Err(_) => {
            let message = br#"{"ok":false,"error":"native panic"}"#;
            let mut out = message.to_vec();
            let len = out.len();
            unsafe {
                if !output_len.is_null() {
                    *output_len = len;
                }
            }
            let ptr = out.as_mut_ptr();
            std::mem::forget(out);
            ptr
        }
    }
}

#[no_mangle]
pub extern "C" fn otools_plugin_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        let _ = Vec::from_raw_parts(ptr, len, len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_registers_all_methods() {
        for method in ["easel_ping", "easel_next_session", "easel_chat_abort", "poll_events"] {
            let response = dispatch(method, Value::Null);
            assert_eq!(
                response.get("ok").and_then(Value::as_bool),
                Some(true),
                "方法应注册成功：{method}"
            );
        }
    }

    #[test]
    fn chat_stream_validates_payload() {
        let response = dispatch("easel_chat_stream", json!({ "sessionId": "x" }));
        assert_eq!(response.get("ok").and_then(Value::as_bool), Some(false));
        let error = response.get("error").and_then(Value::as_str).unwrap_or_default();
        assert!(error.contains("解析请求失败"));
    }

    #[test]
    fn unknown_method_reported() {
        let response = dispatch("nope", Value::Null);
        assert_eq!(response.get("ok").and_then(Value::as_bool), Some(false));
        assert!(response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .starts_with("未知方法"));
    }
}
