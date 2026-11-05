//! vendored 自 OTools 宿主 `crates/ot-agent`(Apache-2.0,本团队代码)。
//!
//! 独立插件仓库不依赖宿主单体仓,按 free-model-otools 先例以 vendored 方式
//! 携带 codex app-server 客户端;升级 = 从宿主仓同步这三个文件。

pub mod codex;
pub mod protocol;

pub use codex::{AgentError, AgentEvent, AppServer, SpawnOptions};
pub use protocol::{parse_line, Incoming, RpcError};

use serde_json::{json, Value};
use std::path::Path;

/// codex 沙箱策略(Easel codex.py 同款:只读 / 工作区写入)。
pub fn sandbox_policy(kind: &str, cwd: &Path, network_access: bool) -> Result<Value, String> {
    match kind {
        "read-only" => Ok(json!({ "type": "readOnly" })),
        "workspace-write" => Ok(json!({
            "type": "workspaceWrite",
            "writableRoots": [cwd.to_string_lossy()],
            "networkAccess": network_access,
        })),
        other => Err(format!("不支持的沙箱策略:{other}(codex 只允许 read-only / workspace-write)")),
    }
}

/// thread/start|resume 的公共参数。
pub fn thread_options(
    cwd: &Path,
    sandbox: &str,
    developer_instructions: &str,
    model: Option<&str>,
) -> Result<Value, String> {
    let mut options = json!({
        "cwd": cwd.to_string_lossy(),
        "approvalPolicy": "on-request",
        "sandbox": sandbox,
        "developerInstructions": developer_instructions,
    });
    if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
        options["model"] = json!(model);
    }
    Ok(options)
}
