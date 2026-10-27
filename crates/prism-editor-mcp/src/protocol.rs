//! 最小 MCP（Model Context Protocol）stdio 传输：JSON-RPC 2.0，一行一条消息。
//!
//! 只实现 agent 实际用得到的子集：`initialize`、`tools/list`、`tools/call`、
//! `ping`。没有 resource / prompt / sampling 能力，也没有进度通知 —— 工具都是
//! 同步的，调用完直接返回结果。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 一条进来的 JSON-RPC 请求（通知没有 `id`）。
#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// 工具的静态描述，直接序列化成 MCP 的 `tools/list` 条目。
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

pub fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn failure(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() },
    })
}

/// 工具执行结果 → MCP 的 `content` 数组。
pub fn text_content(text: impl Into<String>, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text.into() }],
        "isError": is_error,
    })
}

pub fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2025-06-18",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "prism-editor-mcp", "version": env!("CARGO_PKG_VERSION") },
    })
}
