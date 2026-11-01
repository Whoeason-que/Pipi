//! 客户端错误。宿主把 `Display` 文本直接写进工具结果或连接状态，所以每种
//! 失败都要说清「哪一步、哪台服务器、为什么」。

use std::fmt;
use std::time::Duration;

#[derive(Debug)]
pub enum McpError {
    /// 子进程起不来（可执行文件不存在、权限不足、沙箱拒绝）。
    Spawn(String),
    /// 管道读写失败。
    Io(String),
    /// 连接已关闭（服务器退出、stdin 已关）。
    Closed,
    /// 请求超时。带上方法名，方便判断是 `initialize` 卡住还是单次调用太慢。
    Timeout { method: String, after: Duration },
    /// 对端发来的消息不符合协议（JSON 解析失败、缺字段）。
    Protocol(String),
    /// JSON-RPC 错误响应。
    Rpc { code: i64, message: String },
    /// 握手失败，附带服务器 stderr 尾部（服务器起不来的原因几乎都在那里）。
    Initialize {
        error: Box<McpError>,
        stderr: String,
    },
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            McpError::Spawn(detail) => write!(f, "failed to start MCP server: {detail}"),
            McpError::Io(detail) => write!(f, "MCP transport error: {detail}"),
            McpError::Closed => write!(f, "MCP server connection is closed"),
            McpError::Timeout { method, after } => write!(
                f,
                "MCP request \"{method}\" timed out after {}ms",
                after.as_millis()
            ),
            McpError::Protocol(detail) => write!(f, "invalid MCP message: {detail}"),
            McpError::Rpc { code, message } => {
                write!(f, "MCP server returned error {code}: {message}")
            }
            McpError::Initialize { error, stderr } => write!(
                f,
                "MCP initialization failed: {error}\nserver stderr:\n{stderr}"
            ),
        }
    }
}

impl std::error::Error for McpError {}
