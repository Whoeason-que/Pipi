//! 连接、握手与请求多路复用。
//!
//! 结构是「一个读任务 + 一把写锁 + 一张 pending 表」：读任务独占 stdout，
//! 响应按 id 唤醒等待者，服务器发来的请求（`ping` / `roots/list`）就地应答，
//! 通知推进工具版本号。调用方从不直接碰管道，所以并发调用安全。

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex as AsyncMutex, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::error::McpError;
use crate::protocol::{self, CallToolResult, Inbound, McpToolDef, ServerInfo};

/// 保留的 stderr 尾部行数（服务器崩溃时用于诊断）。
const STDERR_TAIL_LINES: usize = 200;
/// `tools/list` 翻页上限，防止服务器给出循环 cursor。
const MAX_TOOL_PAGES: usize = 32;
/// 关闭时等服务器自己退出的宽限（先关 stdin，再 TERM，最后 KILL）。
const SHUTDOWN_GRACE: Duration = Duration::from_millis(1500);

type Writer = Box<dyn AsyncWrite + Unpin + Send>;
type Reader = Box<dyn AsyncRead + Unpin + Send>;

/// 连接参数。`request_timeout` 是所有请求的默认超时（单次调用可覆盖）。
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub client_name: String,
    pub client_version: String,
    pub initialize_timeout: Duration,
    pub request_timeout: Duration,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        ConnectOptions {
            client_name: "pipi".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            initialize_timeout: Duration::from_secs(20),
            request_timeout: Duration::from_secs(60),
        }
    }
}

struct Inner {
    /// 写入侧：一次一整行，避免两条消息交错。`None` = stdin 已关闭。
    writer: AsyncMutex<Option<Writer>>,
    /// 只有 `connect` 起的连接才有子进程；进程组收尾用它。
    child: AsyncMutex<Option<Child>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, McpError>>>>,
    next_id: AtomicU64,
    closed: AtomicBool,
    stderr: Mutex<VecDeque<String>>,
    info: RwLock<ServerInfo>,
    /// `notifications/tools/list_changed` 的版本号。
    changed: watch::Sender<u64>,
    request_timeout: Duration,
}

/// 一个 MCP 服务器连接。可克隆：底层共享同一张 pending 表与同一条管道。
#[derive(Clone)]
pub struct McpClient {
    inner: Arc<Inner>,
}

impl McpClient {
    /// spawn 服务器并完成 `initialize` 握手。
    ///
    /// `command` 由调用方装配（沙箱、env、工作目录都在这里之前决定）；本函数
    /// 只补 stdin/stdout/stderr 与进程组。握手失败时会把刚起的进程收干净。
    pub async fn connect(
        mut command: Command,
        options: ConnectOptions,
    ) -> Result<McpClient, McpError> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| McpError::Spawn(error.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Spawn("server stdin was not piped".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Spawn("server stdout was not piped".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| McpError::Spawn("server stderr was not piped".into()))?;
        let client = Self::from_parts(
            Box::new(stdin),
            Box::new(stdout),
            Some(Box::new(stderr)),
            Some(child),
            options.request_timeout,
        );
        if let Err(error) = client.initialize(&options).await {
            let stderr = client.stderr_tail();
            client.shutdown().await;
            return Err(if stderr.trim().is_empty() {
                error
            } else {
                McpError::Initialize {
                    error: Box::new(error),
                    stderr,
                }
            });
        }
        Ok(client)
    }

    fn from_parts(
        stdin: Writer,
        stdout: Reader,
        stderr: Option<Reader>,
        child: Option<Child>,
        request_timeout: Duration,
    ) -> Self {
        let (changed, _) = watch::channel(0u64);
        let inner = Arc::new(Inner {
            writer: AsyncMutex::new(Some(stdin)),
            child: AsyncMutex::new(child),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            stderr: Mutex::new(VecDeque::new()),
            info: RwLock::new(ServerInfo::default()),
            changed,
            request_timeout,
        });
        spawn_reader(inner.clone(), stdout);
        if let Some(stderr) = stderr {
            spawn_stderr(inner.clone(), stderr);
        }
        McpClient { inner }
    }

    async fn initialize(&self, options: &ConnectOptions) -> Result<(), McpError> {
        let params = json!({
            "protocolVersion": protocol::PROTOCOL_VERSION,
            // 只用工具：不声明 sampling / elicitation / roots 能力。
            "capabilities": {},
            "clientInfo": {
                "name": options.client_name,
                "version": options.client_version,
            },
        });
        let result = self
            .request_with("initialize", params, options.initialize_timeout)
            .await?;
        let version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if version.is_empty() {
            return Err(McpError::Protocol(
                "initialize response is missing protocolVersion".into(),
            ));
        }
        // 日期串按字典序就是时间序。
        if version.as_str() < protocol::MIN_PROTOCOL_VERSION {
            return Err(McpError::Protocol(format!(
                "server speaks MCP {version}, older than the supported {}",
                protocol::MIN_PROTOCOL_VERSION
            )));
        }
        let server = result.get("serverInfo").cloned().unwrap_or(Value::Null);
        {
            let mut info = lock_info_mut(&self.inner);
            *info = ServerInfo {
                name: server
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                version: server
                    .get("version")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                protocol_version: version,
            };
        }
        self.notify("notifications/initialized", json!({})).await
    }

    /// 服务器声明支持的全部工具（自动翻页）。
    pub async fn tools(&self) -> Result<Vec<McpToolDef>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_TOOL_PAGES {
            let params = match cursor.as_deref() {
                Some(cursor) if !cursor.is_empty() => json!({ "cursor": cursor }),
                _ => json!({}),
            };
            let result = self.request("tools/list", params).await?;
            let page = result.get("tools").cloned().unwrap_or_else(|| json!([]));
            let page: Vec<McpToolDef> = serde_json::from_value(page).map_err(|error| {
                McpError::Protocol(format!("invalid tools/list response: {error}"))
            })?;
            tools.extend(page);
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.as_deref().is_none_or(str::is_empty) {
                break;
            }
        }
        Ok(tools)
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<CallToolResult, McpError> {
        self.call_tool_with_timeout(name, arguments, self.inner.request_timeout)
            .await
    }

    pub async fn call_tool_with_timeout(
        &self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<CallToolResult, McpError> {
        self.call_tool_cancellable(name, arguments, timeout, None)
            .await
    }

    /// 可取消的工具调用。取消时移除等待者并发 `notifications/cancelled`
    /// （MCP 的取消是建议性的：服务器可能仍然执行完，只是我们不再等结果）。
    pub async fn call_tool_cancellable(
        &self,
        name: &str,
        arguments: Value,
        timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<CallToolResult, McpError> {
        let arguments = if arguments.is_object() {
            arguments
        } else {
            json!({})
        };
        let result = self
            .request_cancellable(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
                timeout,
                cancel,
            )
            .await?;
        Ok(protocol::parse_call_result(&result))
    }

    pub async fn ping(&self) -> Result<(), McpError> {
        self.request("ping", json!({})).await.map(|_| ())
    }

    pub fn server_info(&self) -> ServerInfo {
        lock_info(&self.inner).clone()
    }

    /// 服务器 stderr 的尾部（最近 [`STDERR_TAIL_LINES`] 行），用于失败诊断。
    pub fn stderr_tail(&self) -> String {
        let tail = lock_stderr(&self.inner);
        tail.iter().cloned().collect::<Vec<_>>().join("\n")
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// 工具列表版本号的观察者；`changed()` 返回后重新 [`tools`](Self::tools)。
    pub fn watch_tools(&self) -> watch::Receiver<u64> {
        self.inner.changed.subscribe()
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        write_message(
            &self.inner,
            &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
        )
        .await
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        self.request_with(method, params, self.inner.request_timeout)
            .await
    }

    async fn request_with(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.request_cancellable(method, params, timeout, None)
            .await
    }

    async fn request_cancellable(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value, McpError> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(McpError::Closed);
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        lock_pending(&self.inner).insert(id, sender);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(error) = write_message(&self.inner, &message).await {
            lock_pending(&self.inner).remove(&id);
            return Err(error);
        }
        let outcome = match cancel {
            Some(token) => {
                tokio::select! {
                    // 取消优先：用户点了停止就不该再等响应。
                    biased;
                    _ = token.cancelled() => {
                        self.abandon_request(id, method).await;
                        return Err(McpError::Cancelled);
                    }
                    result = tokio::time::timeout(timeout, receiver) => result,
                }
            }
            None => tokio::time::timeout(timeout, receiver).await,
        };
        match outcome {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(error))) => Err(error),
            // 发送端被读任务丢弃：连接已经结束。
            Ok(Err(_)) => Err(McpError::Closed),
            Err(_) => {
                // 迟到的响应找不到等待者，会被读任务直接丢弃。
                lock_pending(&self.inner).remove(&id);
                Err(McpError::Timeout {
                    method: method.to_string(),
                    after: timeout,
                })
            }
        }
    }

    /// 放弃一次已发出的请求：移除等待者，并告知服务器（尽最大努力）。
    async fn abandon_request(&self, id: u64, method: &str) {
        lock_pending(&self.inner).remove(&id);
        let _ = write_message(
            &self.inner,
            &json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": {
                    "requestId": id,
                    "reason": format!("{method} cancelled by the client"),
                },
            }),
        )
        .await;
    }

    /// 关闭连接：关 stdin（正常退出约定）→ 整组 TERM → 整组 KILL。
    ///
    /// 只杀直接子进程会留下 `npx` / `uvx` 的孙进程继续持有管道。
    pub async fn shutdown(&self) {
        let writer = self.inner.writer.lock().await.take();
        drop(writer);
        let child = self.inner.child.lock().await.take();
        let Some(mut child) = child else {
            return;
        };
        let Some(pid) = child.id() else {
            let _ = child.wait().await;
            return;
        };
        if tokio::time::timeout(SHUTDOWN_GRACE, child.wait())
            .await
            .is_ok()
        {
            return;
        }
        let _ = signal_group(pid, GroupSignal::Term);
        if tokio::time::timeout(SHUTDOWN_GRACE, child.wait())
            .await
            .is_ok()
        {
            return;
        }
        let _ = signal_group(pid, GroupSignal::Kill);
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

/// 写一条消息（一行 JSON，MCP stdio 的分帧方式）。
async fn write_message(inner: &Inner, message: &Value) -> Result<(), McpError> {
    let mut guard = inner.writer.lock().await;
    let Some(writer) = guard.as_mut() else {
        return Err(McpError::Closed);
    };
    let mut line = serde_json::to_vec(message)
        .map_err(|error| McpError::Protocol(format!("failed to encode request: {error}")))?;
    line.push(b'\n');
    writer
        .write_all(&line)
        .await
        .map_err(|error| transport_error("write to", error))?;
    writer
        .flush()
        .await
        .map_err(|error| transport_error("flush to", error))
}

/// 管道已断（服务器先退出）与真正的传输故障要分开：前者对调用方就是
/// [`McpError::Closed`]（可以重连），后者是诊断信息。
fn transport_error(action: &str, error: std::io::Error) -> McpError {
    if error.kind() == std::io::ErrorKind::BrokenPipe {
        McpError::Closed
    } else {
        McpError::Io(format!("failed to {action} server: {error}"))
    }
}

fn spawn_reader(inner: Arc<Inner>, stdout: Reader) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => handle_line(&inner, &line).await,
                Ok(None) => break,
                Err(_) => break,
            }
        }
        // 连接结束：唤醒所有等待者，别让调用方等到超时。
        inner.closed.store(true, Ordering::SeqCst);
        let pending = std::mem::take(&mut *lock_pending(&inner));
        for (_, sender) in pending {
            let _ = sender.send(Err(McpError::Closed));
        }
    });
}

fn spawn_stderr(inner: Arc<Inner>, stderr: Reader) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut tail = lock_stderr(&inner);
            if tail.len() == STDERR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    });
}

async fn handle_line(inner: &Arc<Inner>, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        // 半行或非 JSON 输出：跳过而不是拖垮整个连接（服务器日志常混进 stdout）。
        Err(_) => return,
    };
    match value {
        Value::Array(messages) => {
            for message in messages {
                handle_message(inner, message).await;
            }
        }
        message => handle_message(inner, message).await,
    }
}

async fn handle_message(inner: &Arc<Inner>, value: Value) {
    let Ok(Inbound {
        id,
        method,
        result,
        error,
    }) = serde_json::from_value::<Inbound>(value)
    else {
        return;
    };
    match (id, method) {
        // 服务器发来的请求：我们只支持 ping 与 roots/list，其余按 JSON-RPC 拒绝。
        (Some(id), Some(method)) => respond_to_request(inner, id, &method).await,
        // 我们发出的请求的响应。
        (Some(id), None) => {
            let Some(id) = id.as_u64() else {
                return;
            };
            let Some(sender) = lock_pending(inner).remove(&id) else {
                return;
            };
            let outcome = match error {
                Some(error) => Err(McpError::Rpc {
                    code: error.code,
                    message: error.message,
                }),
                None => Ok(result.unwrap_or(Value::Null)),
            };
            let _ = sender.send(outcome);
        }
        (None, Some(method)) => {
            if method == "notifications/tools/list_changed" {
                let version = *inner.changed.borrow();
                let _ = inner.changed.send(version.wrapping_add(1));
            }
        }
        (None, None) => {}
    }
}

async fn respond_to_request(inner: &Arc<Inner>, id: Value, method: &str) {
    let response = match method {
        "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
        "roots/list" => json!({ "jsonrpc": "2.0", "id": id, "result": { "roots": [] } }),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("Method not supported by Pipi: {method}"),
            },
        }),
    };
    // 写失败说明连接已经坏了；读任务随后会收到 EOF 并收尾。
    let _ = write_message(inner, &response).await;
}

fn lock_pending(
    inner: &Inner,
) -> MutexGuard<'_, HashMap<u64, oneshot::Sender<Result<Value, McpError>>>> {
    inner
        .pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_stderr(inner: &Inner) -> MutexGuard<'_, VecDeque<String>> {
    inner
        .stderr
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_info(inner: &Inner) -> std::sync::RwLockReadGuard<'_, ServerInfo> {
    inner
        .info
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_info_mut(inner: &Inner) -> std::sync::RwLockWriteGuard<'_, ServerInfo> {
    inner
        .info
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Copy)]
enum GroupSignal {
    Term,
    Kill,
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: GroupSignal) -> Result<(), McpError> {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    let signal = match signal {
        GroupSignal::Term => Signal::SIGTERM,
        GroupSignal::Kill => Signal::SIGKILL,
    };
    match killpg(Pid::from_raw(pid as i32), signal) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(error) => Err(McpError::Io(format!(
            "failed to signal server process group {pid}: {error}"
        ))),
    }
}

#[cfg(not(unix))]
fn signal_group(_pid: u32, _signal: GroupSignal) -> Result<(), McpError> {
    Ok(())
}
