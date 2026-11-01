//! 测试用的假 MCP 服务器（stdio）。
//!
//! 也方便手工验证：把它的路径填进 `agent.json` 的 `mcpServers[].command` 就能
//! 在真实会话里看到 `echo` / `fail` / `image` / `slow` 四个工具。
//!
//! 模式参数：
//! - 默认：正常应答；
//! - `--die-after-init`：回完 `initialize` 就退出（测连接关闭）；
//! - `--no-tools`：`tools/list` 返回空列表。

use std::io::{self, BufRead, Write};
use std::time::Duration;

use serde_json::{Value, json};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // 通知（没有 id）只需要接收，不用回。
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let method = message["method"].as_str().unwrap_or_default();
        match method {
            "initialize" => {
                write(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": "2025-06-18",
                            "capabilities": { "tools": { "listChanged": true } },
                            "serverInfo": { "name": "mock-mcp", "version": "0.1.0" },
                        },
                    }),
                );
                if mode == "--die-after-init" {
                    return;
                }
            }
            "ping" => write(
                &mut stdout,
                json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            ),
            // 服务器主动请求客户端：用于测 roots 应答（当前只测不崩）。
            "mock/ask-client" => write(
                &mut stdout,
                json!({
                    "jsonrpc": "2.0",
                    "id": "server-1",
                    "method": "roots/list",
                }),
            ),
            "tools/list" => {
                let tools = if mode == "--no-tools" {
                    json!([])
                } else {
                    json!([
                        {
                            "name": "echo",
                            "description": "Echo the message back",
                            "inputSchema": {
                                "type": "object",
                                "properties": { "message": { "type": "string" } },
                                "required": ["message"],
                            },
                        },
                        {
                            "name": "fail",
                            "description": "Always fails",
                            "inputSchema": { "type": "object", "properties": {} },
                        },
                        {
                            "name": "image",
                            "description": "Return an image",
                            "inputSchema": { "type": "object", "properties": {} },
                        },
                        {
                            "name": "slow",
                            "description": "Sleep before answering",
                            "inputSchema": { "type": "object", "properties": {} },
                        },
                    ])
                };
                write(
                    &mut stdout,
                    json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tools } }),
                );
            }
            "tools/call" => {
                let name = message["params"]["name"].as_str().unwrap_or_default();
                let args = &message["params"]["arguments"];
                let result = match name {
                    "echo" => json!({
                        "content": [{
                            "type": "text",
                            "text": format!("echo: {}", args["message"].as_str().unwrap_or_default()),
                        }],
                    }),
                    "fail" => json!({
                        "content": [{ "type": "text", "text": "tool failed on purpose" }],
                        "isError": true,
                    }),
                    "image" => json!({
                        "content": [{
                            "type": "image",
                            "data": "aGVsbG8=",
                            "mimeType": "image/png",
                        }],
                    }),
                    "slow" => {
                        std::thread::sleep(Duration::from_secs(2));
                        json!({ "content": [{ "type": "text", "text": "slow done" }] })
                    }
                    other => {
                        write(
                            &mut stdout,
                            json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32602, "message": format!("unknown tool: {other}") },
                            }),
                        );
                        continue;
                    }
                };
                write(
                    &mut stdout,
                    json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                );
            }
            other => write(
                &mut stdout,
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("unknown method: {other}") },
                }),
            ),
        }
    }
}

fn write(stdout: &mut io::Stdout, message: Value) {
    let mut line = serde_json::to_string(&message).expect("test message is serializable");
    line.push('\n');
    let _ = stdout.write_all(line.as_bytes());
    let _ = stdout.flush();
}
