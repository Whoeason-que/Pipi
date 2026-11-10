//! 连接的宿主级缓存：按 (Agent, 服务器) 保存启动参数、连接与工具清单。
//!
//! 生命周期：`prepare` 每轮被调用一次（同步），配置指纹变化时断开旧连接并把
//! 新连接交给后台任务；`call` 在工具真正被调用时兜底重连。注册表装配只读缓存，
//! 所以「第一轮没有工具」在重启后由磁盘快照兜住（见 `cache`）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use pipi_mcp::{ConnectOptions, McpClient, McpToolDef};
use pipi_protocol::permissions::{SandboxMode, SandboxPolicy};
use pipi_venv::agents::{AgentDefinition, McpServerConfig, expand_tilde};
use serde::{Deserialize, Serialize};

use super::cache;

/// 每个 Agent 一次装配最多声明多少个 MCP 工具。工具描述要进提示词，几十个
/// 服务器的全量清单会把上下文吃光；超出的部分直接不声明（见 README）。
const MAX_TOOLS_PER_AGENT: usize = 64;
/// 工具清单 watcher 的核对周期（连接被替换后让它退出）。
const WATCH_TICK: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ServerKey {
    agent_name: String,
    server_name: String,
    strict: bool,
}

impl ServerKey {
    fn new(agent_name: &str, server_name: &str) -> Self {
        Self {
            agent_name: agent_name.to_string(),
            server_name: server_name.to_string(),
            strict: false,
        }
    }
    fn cache_name(&self) -> String {
        if !self.strict {
            return self.server_name.clone();
        }
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(self.server_name.as_bytes());
        format!(
            "strict-{}",
            hash.iter().map(|v| format!("{v:02x}")).collect::<String>()
        )[..63]
            .to_string()
    }
}

/// 一台服务器最终怎么起：命令、参数、环境、工作目录、沙箱。
#[derive(Debug, Clone)]
struct SpawnPlan {
    program: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: PathBuf,
    sandbox: SandboxMode,
    strict: bool,
}

impl SpawnPlan {
    /// 配置指纹：任何一项变化都意味着工具清单可能变了，必须重连。
    fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        // Length-delimited JSON avoids ambiguous field boundaries. Never persist
        // the resolved environment or expanded arguments in the cache.
        let payload = serde_json::json!({
            "program": self.program, "args": self.args, "env": self.env,
            "cwd": self.cwd, "sandbox": self.sandbox,
        });
        let digest =
            Sha256::digest(serde_json::to_vec(&payload).expect("spawn plan is serializable"));
        format!(
            "v2:{}",
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
    }
}

#[derive(Default)]
struct Slot {
    plan: Option<SpawnPlan>,
    fingerprint: String,
    client: Option<Arc<McpClient>>,
    tools: Vec<McpToolDef>,
    connecting: bool,
    error: Option<String>,
    /// 每台服务器一把：并发调用同一台时只建立一条连接。
    connect_lock: Arc<tokio::sync::Mutex<()>>,
    /// `parallelCalls: false`（默认）时串行化工具调用：很多服务器假设调用不并发。
    call_lock: Arc<tokio::sync::Mutex<()>>,
    parallel_calls: bool,
    call_timeout: Duration,
    startup_timeout: Duration,
}

/// 一条可注册的工具声明。
#[derive(Debug, Clone)]
pub struct McpToolSpec {
    /// 注册名 `mcp__<server>__<tool>`（已被供应商字符集限制过）。
    pub wire_name: String,
    pub server: String,
    pub tool: McpToolDef,
    pub(super) strict: bool,
}

/// 一台服务器的连接状态（给 Agent 工作台展示）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    pub name: String,
    pub enabled: bool,
    pub connected: bool,
    pub tools: usize,
    #[serde(default)]
    pub declared_tools: Vec<String>,
    pub error: Option<String>,
}

struct Inner {
    servers: Mutex<HashMap<ServerKey, Slot>>,
}

impl Inner {
    fn servers(&self) -> MutexGuard<'_, HashMap<ServerKey, Slot>> {
        self.servers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Clone)]
pub struct McpManager {
    inner: Arc<Inner>,
    runtime: tokio::runtime::Handle,
}

impl McpManager {
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self {
            inner: Arc::new(Inner {
                servers: Mutex::new(HashMap::new()),
            }),
            runtime,
        }
    }

    /// 按 Agent 定义刷新服务器集合（同步，可在持有会话锁的路径上调用）。
    ///
    /// - 启用的服务器：配置变了就断开重连，没连过就排后台连接；
    /// - 停用或从定义里删掉的服务器：断开并移除；
    /// - 配置错误（缺 env 引用、命令为空）不失败本轮，只记在状态里。
    pub fn prepare(&self, def: &AgentDefinition, env: &BTreeMap<String, String>) {
        self.prepare_inner(def, env, false);
    }

    /// A separate process pool with an upper bound on sandbox permissions.
    pub fn prepare_workspace(&self, def: &AgentDefinition, env: &BTreeMap<String, String>) {
        self.prepare_inner(def, env, true);
    }

    fn prepare_inner(&self, def: &AgentDefinition, env: &BTreeMap<String, String>, strict: bool) {
        let agent_name = def.name.clone();
        cache::purge_legacy(&agent_name);
        let mut pending: Vec<(ServerKey, Arc<tokio::sync::Mutex<()>>)> = Vec::new();
        {
            let mut servers = self.inner.servers();
            let mut seen = HashSet::new();
            for server in &def.mcp_servers {
                let mut key = ServerKey::new(&agent_name, &server.name);
                key.strict = strict;
                if !seen.insert(server.name.clone()) {
                    continue;
                }
                if !server.enabled {
                    // Disabled servers may still have a legacy cache containing
                    // expanded secrets; discard that cache without enabling it.
                    let _ = cache::load(&agent_name, &key.cache_name(), "v2:disabled");
                    if let Some(slot) = servers.remove(&key)
                        && let Some(client) = slot.client
                    {
                        self.shutdown_client(client);
                    }
                    continue;
                }
                let mut plan = match plan_for(def, server, env) {
                    Ok(plan) => plan,
                    Err(error) => {
                        let slot = servers.entry(key).or_default();
                        if let Some(client) = slot.client.take() {
                            self.shutdown_client(client);
                        }
                        slot.plan = None;
                        slot.tools.clear();
                        slot.error = Some(error);
                        continue;
                    }
                };
                if strict {
                    plan.strict = true;
                    if plan.sandbox == SandboxMode::DangerFullAccess {
                        plan.sandbox = SandboxMode::WorkspaceWrite;
                    }
                }
                let fingerprint = plan.fingerprint();
                let slot = servers.entry(key.clone()).or_default();
                if slot.fingerprint != fingerprint {
                    if let Some(client) = slot.client.take() {
                        self.shutdown_client(client);
                    }
                    slot.fingerprint = fingerprint;
                    slot.plan = Some(plan);
                    // 新配置下先沿用磁盘快照，连接成功后会被真实清单覆盖。
                    slot.tools = cache::load(&agent_name, &key.cache_name(), &slot.fingerprint);
                    slot.error = None;
                } else if slot.plan.is_none() {
                    slot.plan = Some(plan);
                }
                // 超时与并发开关不影响进程本身，但每次 prepare 都跟着配置走。
                let (call_timeout, startup_timeout) = timeouts(server);
                slot.call_timeout = call_timeout;
                slot.startup_timeout = startup_timeout;
                slot.parallel_calls = server.parallel_calls;
                let needs_connect = !slot.connecting
                    && slot.client.as_ref().is_none_or(|client| client.is_closed());
                if needs_connect {
                    pending.push((key, slot.connect_lock.clone()));
                }
            }
            // 定义里已经不存在的服务器。
            let stale: Vec<ServerKey> = servers
                .keys()
                .filter(|key| {
                    key.agent_name == agent_name
                        && key.strict == strict
                        && !seen.contains(&key.server_name)
                })
                .cloned()
                .collect();
            for key in stale {
                if let Some(slot) = servers.remove(&key)
                    && let Some(client) = slot.client
                {
                    self.shutdown_client(client);
                }
            }
        }
        for (key, connect_lock) in pending {
            self.spawn_connect(key, connect_lock);
        }
    }

    /// 当前可以声明的工具（缓存或实时清单，已去重、有上限、顺序稳定）。
    pub fn tools_for(&self, agent_name: &str) -> Vec<McpToolSpec> {
        self.tools_for_inner(agent_name, false)
    }
    pub fn workspace_tools_for(&self, agent_name: &str) -> Vec<McpToolSpec> {
        self.tools_for_inner(agent_name, true)
    }
    fn tools_for_inner(&self, agent_name: &str, strict: bool) -> Vec<McpToolSpec> {
        let servers = self.inner.servers();
        let mut specs = Vec::new();
        for (key, slot) in servers
            .iter()
            .filter(|(key, _)| key.agent_name == agent_name && key.strict == strict)
        {
            for tool in &slot.tools {
                if tool.name.trim().is_empty() {
                    continue;
                }
                specs.push(McpToolSpec {
                    wire_name: super::tool::wire_name(&key.server_name, &tool.name),
                    server: key.server_name.clone(),
                    tool: tool.clone(),
                    strict,
                });
            }
        }
        // 顺序必须稳定：工具定义进请求，顺序抖动会让提示词缓存每一轮都失效。
        specs.sort_by(|left, right| {
            left.wire_name
                .cmp(&right.wire_name)
                .then_with(|| left.tool.name.cmp(&right.tool.name))
        });
        let mut seen = HashSet::new();
        specs.retain(|spec| seen.insert(spec.wire_name.clone()));
        specs.truncate(MAX_TOOLS_PER_AGENT);
        specs
    }

    /// 调用一台服务器的工具；连接已断时重连一次。
    /// 调用一台服务器的工具；连接已断时重连一次，用户中止时立刻放弃。
    ///
    /// `parallelCalls: false`（默认）时按服务器串行：同一时间只允许一个调用
    /// 在途，避免有状态服务器被并发请求踩坏。
    pub async fn call(
        &self,
        agent_name: &str,
        server_name: &str,
        tool: &str,
        arguments: serde_json::Value,
        abort: &pipi_protocol::AbortSignal,
    ) -> Result<pipi_mcp::CallToolResult, String> {
        self.call_scoped(agent_name, server_name, tool, arguments, abort, false)
            .await
    }
    pub(super) async fn call_scoped(
        &self,
        agent_name: &str,
        server_name: &str,
        tool: &str,
        arguments: serde_json::Value,
        abort: &pipi_protocol::AbortSignal,
        strict: bool,
    ) -> Result<pipi_mcp::CallToolResult, String> {
        let mut key = ServerKey::new(agent_name, server_name);
        key.strict = strict;
        let (parallel, timeout, lock) = {
            let servers = self.inner.servers();
            let slot = servers.get(&key).ok_or_else(|| {
                format!(
                    "MCP server \"{}\" is not configured for this Agent",
                    key.server_name
                )
            })?;
            (
                slot.parallel_calls,
                slot.call_timeout,
                slot.call_lock.clone(),
            )
        };
        let token = abort.token();
        let _serial = if parallel {
            None
        } else {
            Some(tokio::select! {
                biased;
                _ = token.cancelled() => return Err("Operation aborted before MCP call was sent".into()),
                guard = lock.lock_owned() => guard,
            })
        };
        // The connection task is independently owned: cancelling a waiter must
        // not drop an in-progress spawn and leak its child process.
        let manager = self.clone();
        let connect_key = key.clone();
        let connect = self
            .runtime
            .spawn(async move { manager.ensure_client(&connect_key).await });
        let client = tokio::select! {
            biased;
            _ = token.cancelled() => return Err("Operation aborted before MCP call was sent".into()),
            result = connect => result.map_err(|e| format!("MCP connection task failed: {e}"))??,
        };
        if token.is_cancelled() {
            return Err("Operation aborted before MCP call was sent".into());
        }
        // Once a request may have reached the server, never replay it: a closed
        // connection cannot tell us whether an external side effect completed.
        match client.call_tool_cancellable(tool, arguments, timeout, Some(&token)).await {
            Ok(result) => Ok(result),
            Err(pipi_mcp::McpError::Cancelled) => Err("Operation aborted; the MCP server may still execute the request. Verify its outcome before retrying.".into()),
            Err(error @ (pipi_mcp::McpError::Closed | pipi_mcp::McpError::Io(_) | pipi_mcp::McpError::Timeout { .. } | pipi_mcp::McpError::Protocol(_))) => Err(format!("MCP call outcome is unknown; verify before retrying. {}", describe(&error, &client.stderr_tail()))),
            Err(error) => Err(describe(&error, &client.stderr_tail())),
        }
    }

    pub fn status(&self, agent_name: &str) -> Vec<McpServerStatus> {
        let declared = self.tools_for(agent_name);
        let servers = self.inner.servers();
        let mut statuses: Vec<McpServerStatus> = servers
            .iter()
            .filter(|(key, _)| key.agent_name == agent_name && !key.strict)
            .map(|(key, slot)| McpServerStatus {
                name: key.server_name.clone(),
                // 槽只为启用的服务器存在（停用的在 prepare 里移除）。
                enabled: true,
                connected: slot
                    .client
                    .as_ref()
                    .is_some_and(|client| !client.is_closed()),
                tools: slot.tools.len(),
                declared_tools: declared.iter().filter(|spec| spec.server == key.server_name).map(|spec| spec.wire_name.clone()).collect(),
                error: {
                    let mut warnings = Vec::new();
                    if let Some(error) = &slot.error { warnings.push(error.clone()); }
                    let available = slot.tools.len();
                    let registered = declared.iter().filter(|spec| spec.server == key.server_name).count();
                    if registered < available {
                        warnings.push(format!("{} of {} tools unavailable (name collision, invalid name, or 64-tool limit)", available - registered, available));
                    }
                    let oversized = slot.tools.iter().filter(|tool| serde_json::to_vec(&tool.input_schema).map_or(true, |schema| schema.len() > super::tool::MAX_SCHEMA_BYTES)).count();
                    if oversized > 0 { warnings.push(format!("{oversized} tool schema(s) exceed the size limit and are degraded")); }
                    (!warnings.is_empty()).then(|| warnings.join("; "))
                },
            })
            .collect();
        statuses.sort_by(|left, right| left.name.cmp(&right.name));
        statuses
    }

    /// Agent 被删除/归档时收掉它的所有服务器进程。
    pub fn shutdown_agent(&self, agent_name: &str) {
        let mut servers = self.inner.servers();
        let keys: Vec<ServerKey> = servers
            .keys()
            .filter(|key| key.agent_name == agent_name)
            .cloned()
            .collect();
        for key in keys {
            if let Some(slot) = servers.remove(&key)
                && let Some(client) = slot.client
            {
                self.shutdown_client(client);
            }
        }
    }

    /// 宿主退出：等待所有连接收尾。
    pub async fn shutdown_all(&self) {
        let clients: Vec<Arc<McpClient>> = {
            let mut servers = self.inner.servers();
            servers
                .drain()
                .filter_map(|(_, slot)| slot.client)
                .collect()
        };
        for client in clients {
            client.shutdown().await;
        }
    }

    fn spawn_connect(&self, key: ServerKey, connect_lock: Arc<tokio::sync::Mutex<()>>) {
        let manager = self.clone();
        self.runtime.spawn(async move {
            let _guard = connect_lock.lock().await;
            // 等锁期间可能已经被别的调用连上了。
            if manager.live_client(&key).is_some() {
                return;
            }
            if let Err(error) = manager.connect_now(&key).await {
                manager.record_error(&key, error);
            }
        });
    }

    async fn ensure_client(&self, key: &ServerKey) -> Result<Arc<McpClient>, String> {
        let (cached, connect_lock) = {
            let servers = self.inner.servers();
            let slot = servers.get(key).ok_or_else(|| {
                format!(
                    "MCP server \"{}\" is not configured for this Agent",
                    key.server_name
                )
            })?;
            (slot.client.clone(), slot.connect_lock.clone())
        };
        if let Some(client) = cached.filter(|client| !client.is_closed()) {
            return Ok(client);
        }
        let _guard = connect_lock.lock().await;
        if let Some(client) = self.live_client(key) {
            return Ok(client);
        }
        self.connect_now(key).await?;
        self.live_client(key)
            .ok_or_else(|| format!("MCP server \"{}\" did not connect", key.server_name))
    }

    fn live_client(&self, key: &ServerKey) -> Option<Arc<McpClient>> {
        self.inner
            .servers()
            .get(key)
            .and_then(|slot| slot.client.clone())
            .filter(|client| !client.is_closed())
    }

    async fn connect_now(&self, key: &ServerKey) -> Result<(), String> {
        let plan = {
            let servers = self.inner.servers();
            servers
                .get(key)
                .and_then(|slot| slot.plan.clone())
                .ok_or_else(|| {
                    format!(
                        "MCP server \"{}\" has no valid launch configuration",
                        key.server_name
                    )
                })?
        };
        if let Some(slot) = self.inner.servers().get_mut(key) {
            slot.connecting = true;
        }
        let outcome = self.connect_with_plan(key, &plan).await;
        if let Some(slot) = self.inner.servers().get_mut(key) {
            slot.connecting = false;
            if let Err(error) = &outcome {
                slot.error = Some(error.clone());
            }
        }
        outcome
    }

    async fn connect_with_plan(&self, key: &ServerKey, plan: &SpawnPlan) -> Result<(), String> {
        let command = build_command(plan)?;
        let (call_timeout, startup_timeout) = {
            let servers = self.inner.servers();
            let slot = servers.get(key);
            (
                slot.map_or(Duration::from_secs(60), |slot| slot.call_timeout),
                slot.map_or(Duration::from_secs(20), |slot| slot.startup_timeout),
            )
        };
        let options = ConnectOptions {
            initialize_timeout: startup_timeout,
            request_timeout: call_timeout,
            ..ConnectOptions::default()
        };
        let client = McpClient::connect(command, options)
            .await
            .map_err(|error| describe(&error, ""))?;
        let tools = match client.tools().await {
            Ok(tools) => tools,
            Err(error) => {
                let message = describe(&error, &client.stderr_tail());
                client.shutdown().await;
                return Err(message);
            }
        };
        let client = Arc::new(client);
        let fingerprint = plan.fingerprint();
        let installed = {
            let mut servers = self.inner.servers();
            match servers.get_mut(key) {
                // 连接期间配置又变了：这次结果作废，下一次 prepare 会重连。
                Some(slot) if slot.fingerprint == fingerprint => {
                    slot.client = Some(client.clone());
                    slot.tools = tools.clone();
                    slot.error = None;
                    true
                }
                _ => false,
            }
        };
        if !installed {
            client.shutdown().await;
            return Ok(());
        }
        cache::store(&key.agent_name, &key.cache_name(), &fingerprint, &tools);
        self.spawn_tool_watcher(key.clone(), client, fingerprint);
        Ok(())
    }

    /// 跟随服务器的 `notifications/tools/list_changed`：重列并更新槽位与快照。
    ///
    /// 连接被替换（重连 / 配置变化）时自然退出：`changed()` 在发送端析构后
    /// 返回错误，而且每次更新前都核对还是不是当前连接。
    fn spawn_tool_watcher(&self, key: ServerKey, client: Arc<McpClient>, fingerprint: String) {
        let mut changes = client.watch_tools();
        let manager = self.clone();
        self.runtime.spawn(async move {
            loop {
                // 周期性核对：连接被替换后 `changed()` 不会再触发（发送端还活着），
                // 靠这个 tick 让孤儿 watcher 退出。
                let notified = match tokio::time::timeout(WATCH_TICK, changes.changed()).await {
                    Ok(Ok(())) => true,
                    Ok(Err(_)) => return,
                    Err(_) => false,
                };
                if client.is_closed() || !manager.is_current_client(&key, &client) {
                    return;
                }
                if !notified {
                    continue;
                }
                let Ok(tools) = client.tools().await else {
                    // 列不出来说明连接已经坏了；下一次调用会走重连。
                    return;
                };
                manager.install_tools(&key, &fingerprint, &client, tools);
            }
        });
    }

    fn is_current_client(&self, key: &ServerKey, client: &Arc<McpClient>) -> bool {
        self.inner
            .servers()
            .get(key)
            .and_then(|slot| slot.client.as_ref())
            .is_some_and(|live| Arc::ptr_eq(live, client))
    }

    fn install_tools(
        &self,
        key: &ServerKey,
        fingerprint: &str,
        client: &Arc<McpClient>,
        tools: Vec<McpToolDef>,
    ) {
        let mut servers = self.inner.servers();
        let Some(slot) = servers.get_mut(key) else {
            return;
        };
        if slot.fingerprint != fingerprint
            || !slot
                .client
                .as_ref()
                .is_some_and(|live| Arc::ptr_eq(live, client))
        {
            return;
        }
        slot.tools = tools.clone();
        drop(servers);
        cache::store(&key.agent_name, &key.cache_name(), fingerprint, &tools);
    }

    fn record_error(&self, key: &ServerKey, error: String) {
        if let Some(slot) = self.inner.servers().get_mut(key) {
            slot.error = Some(error);
        }
    }

    fn shutdown_client(&self, client: Arc<McpClient>) {
        self.runtime.spawn(async move { client.shutdown().await });
    }
}

/// MCP 服务器用的子进程环境：去掉按会话变化的键。
///
/// 服务器按 (Agent, 名字) 缓存、被该 Agent 的所有会话共享；把 `AV_SESSION`
/// 带进去会让两路会话的配置指纹交替变化，互相触发重连。
fn server_env(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut env = env.clone();
    env.remove("AV_SESSION");
    env
}

/// 单台服务器的 (调用超时, 握手超时)，缺省 60s / 20s。
fn timeouts(server: &McpServerConfig) -> (Duration, Duration) {
    (
        Duration::from_secs(server.timeout_seconds.unwrap_or(60)),
        Duration::from_secs(server.startup_timeout_seconds.unwrap_or(20)),
    )
}

/// 把一条服务器声明变成启动计划；`${VAR}` 只从 av 契约解析出的环境里取值，
/// 缺值即失败（秘密值永远引用式，不内联）。
fn plan_for(
    def: &AgentDefinition,
    server: &McpServerConfig,
    env: &BTreeMap<String, String>,
) -> Result<SpawnPlan, String> {
    // MCP 服务器是 Agent 级进程（跨会话共享），不能带按会话变化的环境变量：
    // 否则同一 Agent 的两路会话会互相把对方踢着重连。`${AV_SESSION}` 引用会
    // 因此 fail-closed，这是有意的。
    let env = &server_env(env);
    let program = expand_tilde(expand_env_refs(server.command.trim(), env)?.as_str());
    if program.trim().is_empty() {
        return Err(format!(
            "MCP server \"{}\" has an empty command",
            server.name
        ));
    }
    let args = server
        .args
        .iter()
        .map(|arg| expand_env_refs(arg, env).map(|arg| expand_tilde(&arg)))
        .collect::<Result<Vec<_>, _>>()?;
    let mut spawn_env = env.clone();
    for (key, value) in &server.env {
        spawn_env.insert(key.clone(), expand_env_refs(value, env)?);
    }
    let cwd = def
        .resolve_workspace()
        .ok_or("Cannot resolve workspace directory")?;
    let sandbox = server.sandbox.unwrap_or(def.permissions.sandbox);
    Ok(SpawnPlan {
        program,
        args,
        env: spawn_env,
        cwd,
        sandbox,
        strict: false,
    })
}

/// 展开 `${NAME}`；不接受未声明的变量（fail-closed），也不做 shell 求值。
fn expand_env_refs(value: &str, env: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(format!("unterminated variable reference in \"{value}\""));
        };
        let name = &after[..end];
        let resolved = env
            .get(name)
            .ok_or_else(|| format!("environment variable {name} is not declared in agent.toml"))?;
        out.push_str(resolved);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn build_command(plan: &SpawnPlan) -> Result<pipi_mcp::Command, String> {
    let mut policy = SandboxPolicy::for_workspace(plan.sandbox, &plan.cwd);
    if plan.strict {
        policy.allow_env_overrides = false;
    }
    let mut command =
        pipi_sandbox::tokio_agent_command(&policy, &plan.program, &plan.args, &plan.env)?;
    // 与 bash 同一条口径：子进程环境完全来自契约解析结果，不读进程环境。
    command.current_dir(&plan.cwd);
    Ok(command)
}

/// 错误 + stderr 尾部：服务器起不来时，原因几乎总在 stderr 里。
fn describe(error: &pipi_mcp::McpError, stderr: &str) -> String {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        error.to_string()
    } else {
        format!("{error}\nserver stderr:\n{stderr}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::permissions::SandboxMode;

    fn definition() -> AgentDefinition {
        serde_json::from_value(serde_json::json!({
            "name": "agent",
            "workspace": "/tmp/pipi-mcp-test-workspace",
            "permissions": { "tools": ["read"], "sandbox": "workspace-write" },
        }))
        .expect("test definition is valid")
    }

    fn server(value: serde_json::Value) -> McpServerConfig {
        serde_json::from_value(value).expect("test server is valid")
    }

    #[tokio::test]
    async fn restricted_and_default_connections_never_share_tool_slots_or_cache_names() {
        let manager = McpManager::new(tokio::runtime::Handle::current());
        let legacy = ServerKey::new("agent", "server");
        let mut restricted = legacy.clone();
        restricted.strict = true;
        assert_ne!(legacy.cache_name(), restricted.cache_name());
        let tool = |name: &str| McpToolDef {
            name: name.into(),
            title: None,
            description: None,
            input_schema: serde_json::json!({"type":"object"}),
        };
        {
            let mut slots = manager.inner.servers();
            slots.insert(
                legacy,
                Slot {
                    tools: vec![tool("legacy")],
                    ..Default::default()
                },
            );
            slots.insert(
                restricted,
                Slot {
                    tools: vec![tool("restricted")],
                    ..Default::default()
                },
            );
        }
        assert_eq!(manager.tools_for("agent")[0].tool.name, "legacy");
        assert_eq!(
            manager.workspace_tools_for("agent")[0].tool.name,
            "restricted"
        );
        assert!(!manager.tools_for("agent")[0].strict);
        assert!(manager.workspace_tools_for("agent")[0].strict);
    }

    #[test]
    fn sandbox_defaults_to_the_agent_and_can_be_overridden() {
        let def = definition();
        let env = BTreeMap::new();
        let inherited = plan_for(
            &def,
            &server(serde_json::json!({ "name": "a", "command": "npx" })),
            &env,
        )
        .unwrap();
        assert_eq!(inherited.sandbox, SandboxMode::WorkspaceWrite);
        let overridden = plan_for(
            &def,
            &server(serde_json::json!({
                "name": "b",
                "command": "npx",
                "sandbox": "danger-full-access",
            })),
            &env,
        )
        .unwrap();
        assert_eq!(overridden.sandbox, SandboxMode::DangerFullAccess);
    }

    #[test]
    fn env_references_come_from_the_contract_and_missing_ones_fail() {
        let def = definition();
        let env: BTreeMap<String, String> = [("TOKEN".to_string(), "s3cret".to_string())]
            .into_iter()
            .collect();
        let plan = plan_for(
            &def,
            &server(serde_json::json!({
                "name": "a",
                "command": "server",
                "args": ["--token", "${TOKEN}"],
                "env": { "API_KEY": "Bearer ${TOKEN}" },
            })),
            &env,
        )
        .unwrap();
        assert_eq!(plan.args, ["--token", "s3cret"]);
        assert_eq!(plan.env["API_KEY"], "Bearer s3cret");
        assert_eq!(plan.env["TOKEN"], "s3cret");

        let error = plan_for(
            &def,
            &server(serde_json::json!({
                "name": "a",
                "command": "server",
                "args": ["${MISSING}"],
            })),
            &env,
        )
        .unwrap_err();
        assert!(error.contains("MISSING"));
    }

    #[test]
    fn fingerprint_changes_with_any_launch_detail() {
        let def = definition();
        let env = BTreeMap::new();
        let first = plan_for(
            &def,
            &server(serde_json::json!({ "name": "a", "command": "npx" })),
            &env,
        )
        .unwrap();
        let same = plan_for(
            &def,
            &server(serde_json::json!({ "name": "a", "command": "npx" })),
            &env,
        )
        .unwrap();
        let changed = plan_for(
            &def,
            &server(serde_json::json!({
                "name": "a",
                "command": "npx",
                "args": ["-y", "server"],
            })),
            &env,
        )
        .unwrap();
        assert_eq!(first.fingerprint(), same.fingerprint());
        assert_ne!(first.fingerprint(), changed.fingerprint());
        let mut secret_plan = first.clone();
        secret_plan
            .env
            .insert("TOKEN".into(), "synthetic-secret".into());
        secret_plan.args.push("another-synthetic-secret".into());
        let fingerprint = secret_plan.fingerprint();
        assert!(fingerprint.starts_with("v2:"));
        assert!(!fingerprint.contains("synthetic-secret"));
    }
}
