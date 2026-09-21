//! Agent 会话运行时。
//!
//! 这里承载 Tauri 桌面端与 Web 远程服务共同使用的会话状态、Agent loop
//! 编排、消息落盘和事件协议。宿主只需要把 RuntimeEvent 转发到自己的
//! 事件系统即可，不应重复实现 Agent 执行逻辑。

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::agent_loop::Emitter as LoopEmitter;
use crate::agent_loop::{
    run_agent_loop, AgentContext, AgentEvent, AgentLoopConfig, MessageQueue, ToolExecutionMode,
};
use crate::agents::{self, AgentDefinition};
use crate::approval::{
    ApprovalDecision, ApprovalGate, ApprovalRequestEnvelope, InteractiveApprover,
};
use crate::provider::provider_for;
pub use crate::session::SessionSummary;
use crate::session::{list_session_summaries, load_session, EntryKind, SessionWriter};
use crate::settings::load_settings;
use crate::stats::SessionStatsTracker;
use crate::tools::agent::{
    result_from_messages, AgentRunResult, AgentRunStatus, AgentRunner, ChildProgressTx,
    CreateAgentTool, ReadAgentTool, RunAgentTool,
};
use crate::tools::ToolRegistry;
use crate::types::{AbortSignal, Message, Model, StopReason, StreamOptions};

static NEXT_RUN_ID: AtomicUsize = AtomicUsize::new(1);

/// 发给宿主的 Agent 事件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventEnvelope {
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
    pub event: AgentEvent,
}

/// 发给宿主的统计快照。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsEventEnvelope {
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
    pub stats: crate::stats::SessionStats,
}

/// 发给宿主的会话错误。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionErrorEnvelope {
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
    pub message: String,
}

/// 压缩换会话后发给宿主：当前会话已切到新 id（原会话可能已归档）。
///
/// **envelope 用的是切换前的 session_id** —— 前端此刻的身份还是旧的，
/// 用新 id 会先被 `eventMatchesSession` 丢掉；新 id 放在 payload 里。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSwitchedEnvelope {
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
    /// 切换后的会话 id（前端应把身份更新为它）。
    pub to_session_id: String,
    /// 原会话是否已移入归档。
    pub archived: bool,
}

/// 宿主只需将这个协议映射为 Tauri event 或 WebSocket frame。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "payload")]
pub enum RuntimeEvent {
    #[serde(rename = "agent-event")]
    AgentEvent(AgentEventEnvelope),
    #[serde(rename = "session-stats")]
    SessionStats(StatsEventEnvelope),
    #[serde(rename = "session-error")]
    SessionError(SessionErrorEnvelope),
    #[serde(rename = "approval-request")]
    ApprovalRequest(ApprovalRequestEnvelope),
    #[serde(rename = "session-switched")]
    SessionSwitched(SessionSwitchedEnvelope),
}

/// 运行时事件接收器。
pub type EventEmitter = Arc<dyn Fn(RuntimeEvent) + Send + Sync>;

/// 一个（可能跨多轮的）会话的运行状态。
///
/// 低位是 running 标志，其余位是代际；结束操作只允许清除自己开始的代际。
struct RunState {
    value: AtomicUsize,
    run_id: AtomicUsize,
}

impl RunState {
    const RUNNING_BIT: usize = 1;

    fn new() -> Self {
        Self {
            value: AtomicUsize::new(0),
            run_id: AtomicUsize::new(NEXT_RUN_ID.load(Ordering::Acquire).saturating_sub(1)),
        }
    }

    fn begin(&self) -> usize {
        let mut current = self.value.load(Ordering::Acquire);
        loop {
            let generation = current >> 1;
            let next_generation = generation.wrapping_add(1);
            let next = (next_generation << 1) | Self::RUNNING_BIT;
            match self
                .value
                .compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    let run_id = NEXT_RUN_ID.fetch_add(1, Ordering::AcqRel);
                    self.run_id.store(run_id, Ordering::Release);
                    return next;
                }
                Err(actual) => current = actual,
            }
        }
    }

    fn finish(&self, token: usize) {
        let idle = token & !Self::RUNNING_BIT;
        let _ = self
            .value
            .compare_exchange(token, idle, Ordering::AcqRel, Ordering::Acquire);
    }

    fn is_running(&self) -> bool {
        self.value.load(Ordering::Acquire) & Self::RUNNING_BIT != 0
    }

    fn current_run_id(&self) -> usize {
        self.run_id.load(Ordering::Acquire)
    }
}

/// 一个运行代际的守卫；无论正常返回、取消还是 panic 都会清理状态。
struct RunningGuard {
    state: Arc<RunState>,
    token: usize,
}

impl RunningGuard {
    fn new(state: Arc<RunState>) -> Self {
        let token = state.begin();
        Self { state, token }
    }
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.state.finish(self.token);
    }
}

struct Session {
    agent: AgentDefinition,
    messages: Arc<tokio::sync::Mutex<Vec<Message>>>,
    writer: Arc<Mutex<SessionWriter>>,
    /// 设置工作台的测试会话只活在 RuntimeState 内，不创建 sessions/*.jsonl。
    temporary: bool,
    stats: Arc<Mutex<SessionStatsTracker>>,
    abort: AbortSignal,
    running: Arc<RunState>,
    model: Arc<Mutex<Option<Model>>>,
    /// 运行中插话队列（pi 的 steering）。运行结束后仍留在队列里的消息会在
    /// 下一次 send_prompt 开头被收割，保证不丢。
    steering: MessageQueue,
}

/// 打开中的会话的键：**Agent 名 + 会话 id**。
///
/// 同一个 Agent 可以同时开多条会话（每条独立运行）；会话 id 会变（压缩分叉出新
/// 会话）—— 那时把条目搬到新键即可，`SessionSwitched` 已经把这个变化告诉前端。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub agent_name: String,
    pub session_id: String,
}

impl SessionKey {
    pub fn new(agent_name: &str, session_id: &str) -> Self {
        Self {
            agent_name: agent_name.to_string(),
            session_id: session_id.to_string(),
        }
    }
}

/// 压缩分叉把会话换成新 id 之后，把 map 里的条目从旧键搬到新键（同一个 Session
/// 对象：运行守卫 / abort / writer 跟着走）。运行任务里调用的回调，所以要 `'static`。
pub type SessionRekey = Arc<dyn Fn(&SessionKey, &str) + Send + Sync>;

/// 共享会话状态。Tauri 与 Web 服务各自持有一个实例。
pub struct RuntimeState {
    /// 跑 Agent 循环用的 Tokio 运行时句柄，由宿主注入（桌面壳用 Tauri 的运行时，
    /// Web 服务用自己的运行时）。核心不依赖宿主框架，也不能假设「调用线程已在
    /// reactor 里」：桌面壳的 Tauri command 跑在 GTK 主线程上，那里没有运行时
    /// 上下文，直接 `tokio::spawn` 会 panic（there is no reactor running）。
    runtime: tokio::runtime::Handle,
    /// 打开中的会话，按 **(Agent 名, 会话 id)** 索引。同一个 Agent 可以同时开多条
    /// 会话，每条各自一个运行守卫、各自一个 abort 与 writer —— 互不干扰。
    ///
    /// `Arc` 是为了让压缩分叉换会话 id 时能在运行任务里把条目搬到新键（见
    /// `SessionRekey`）。
    sessions: Arc<Mutex<HashMap<SessionKey, Session>>>,
    approval: Arc<ApprovalGate>,
}

/// 校验会话 ID：必须是单一、稳定的文件名（`<id>.jsonl` 的 stem）。
pub fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("非法会话 ID".into());
    }
    Ok(())
}

impl RuntimeState {
    /// 用宿主运行时句柄构造。句柄必须指向多线程、IO/time 驱动齐全的运行时。
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self {
            runtime,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            approval: Arc::new(ApprovalGate::new()),
        }
    }

    /// 把一轮运行交给注入的运行时执行（不依赖调用线程的 reactor 上下文）。
    fn spawn_run<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn(task);
    }

    fn session_file_path(
        &self,
        agent_name: &str,
        session_id: &str,
    ) -> Result<std::path::PathBuf, String> {
        let def = agents::load_agent(agent_name)?;
        let dir = def
            .sessions_dir()
            .ok_or_else(|| "无法解析会话目录".to_string())?;
        // 会话目录必须是真实目录（非符号链接），否则 rename/remove 会沿链接波及外部文件
        agents::ensure_real_directory(&dir, "sessions 目录")?;
        Ok(dir.join(format!("{session_id}.jsonl")))
    }

    /// 校验会话文件：必须是真实文件（非符号链接）；不存在返回 false。
    fn ensure_real_session_file(path: &std::path::Path) -> Result<bool, String> {
        agents::ensure_real_file(path, "会话文件")
    }

    // ============ 会话归档 / 恢复 / 删除 ============
    // 归档 = 移动 `sessions/<id>.jsonl` 到 `sessions/.archive/<id>.jsonl`，
    // 与 Agent 归档同一套「文件即真相」语义。删除不可恢复，UI 层需确认。
    // 注意：占用检查与文件操作必须在同一把 sessions 锁内完成，
    // 否则检查与移动之间可能被并发 open_session 抢跑。
    // （跨进程限制：桌面壳与 Web 服务同时跑时各有 RuntimeState，进程间的
    // 并发打开不在本锁覆盖范围内 —— 产品形态是二选一，已知限制。）

    /// 归档会话：移动到 `sessions/.archive/`。
    ///
    /// 占用检查留在这一层（会话正打开时拒绝）；实际移动是
    /// [`crate::session::archive_session_file`] —— 压缩换会话后的内部归档
    /// 走那个自由函数（那时旧 id 已不再是打开的会话）。
    pub fn archive_session(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.contains_key(&SessionKey::new(agent_name, session_id)) {
            return Err("该会话当前已打开，请先返回再归档".into());
        }
        let src = self.session_file_path(agent_name, session_id)?;
        if !Self::ensure_real_session_file(&src)? {
            return Err("会话不存在".into());
        }
        let Some(dir) = src.parent() else {
            return Err("无法解析会话目录".into());
        };
        crate::session::archive_session_file(dir, session_id).map(|_| ())
    }

    /// 恢复归档会话：移回 `sessions/`。
    pub fn restore_session(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let _sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let live_dir = self
            .session_file_path(agent_name, session_id)?
            .parent()
            .ok_or_else(|| "无法解析会话目录".to_string())?
            .to_path_buf();
        let src = live_dir
            .join(agents::ARCHIVE_DIR)
            .join(format!("{session_id}.jsonl"));
        if !Self::ensure_real_session_file(&src)? {
            return Err("归档区没有该会话".into());
        }
        let dst = live_dir.join(format!("{session_id}.jsonl"));
        if dst.exists() {
            return Err("活跃区已存在同名会话".into());
        }
        std::fs::rename(&src, &dst).map_err(|e| format!("恢复会话失败: {e}"))
    }

    /// 彻底删除会话（`sessions/<id>.jsonl`）。不可恢复；UI 层需确认。
    pub fn delete_session(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.contains_key(&SessionKey::new(agent_name, session_id)) {
            return Err("该会话当前已打开，请先返回再删除".into());
        }
        let path = self.session_file_path(agent_name, session_id)?;
        if !Self::ensure_real_session_file(&path)? {
            return Err("会话不存在".into());
        }
        std::fs::remove_file(&path).map_err(|e| format!("删除会话失败: {e}"))
    }

    /// 删除已归档会话（`sessions/.archive/<id>.jsonl`）。不可恢复。
    pub fn delete_archived_session(
        &self,
        agent_name: &str,
        session_id: &str,
    ) -> Result<(), String> {
        validate_session_id(session_id)?;
        let _sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let def = agents::load_agent(agent_name)?;
        let archive_dir = def
            .sessions_dir()
            .ok_or_else(|| "无法解析会话目录".to_string())?
            .join(agents::ARCHIVE_DIR);
        if !agents::ensure_real_directory(&archive_dir, "归档目录")? {
            return Err("归档区没有该会话".into());
        }
        let path = archive_dir.join(format!("{session_id}.jsonl"));
        if !Self::ensure_real_session_file(&path)? {
            return Err("归档区没有该会话".into());
        }
        std::fs::remove_file(&path).map_err(|e| format!("删除归档会话失败: {e}"))
    }

    /// 列出已归档会话摘要。
    pub fn list_archived_sessions(&self, agent_name: &str) -> Result<Vec<SessionSummary>, String> {
        let def = agents::load_agent(agent_name)?;
        let dir = def
            .sessions_dir()
            .ok_or_else(|| "无法解析会话目录".to_string())?
            .join(agents::ARCHIVE_DIR);
        match agents::ensure_real_directory(&dir, "归档目录") {
            Ok(false) => return Ok(Vec::new()),
            Ok(true) => {}
            Err(error) => return Err(error),
        }
        Ok(list_session_summaries(&dir))
    }

    // ============ Agent 归档 / 恢复 / 删除（运行时占用检查） ============

    /// 归档 Agent：目录移入 `.archive/`。该 Agent 有会话打开时拒绝。
    /// 占用检查与目录移动在同一把 sessions 锁内，避免检查后被并发打开抢跑。
    pub fn archive_agent(&self, name: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.iter().any(|(key, session)| {
            key.agent_name == name && session.temporary && session.running.is_running()
        }) {
            return Err(format!("Agent「{name}」的临时测试仍在运行，请先停止再归档"));
        }
        if sessions
            .iter()
            .any(|(key, session)| key.agent_name == name && !session.temporary)
        {
            return Err(format!(
                "Agent「{name}」仍有会话打开，请先打开其他会话或新建会话，再归档"
            ));
        }
        // 空闲临时测试没有文件需要保留，也不应让访问过设置页的 Agent 永久无法归档。
        sessions.retain(|key, session| !(key.agent_name == name && session.temporary));
        drop(sessions);
        agents::archive_agent(name)
    }

    /// 恢复归档 Agent（归档时已保证无会话打开，这里只做文件移动）。
    pub fn restore_agent(&self, name: &str) -> Result<(), String> {
        agents::restore_agent(name)
    }

    /// 彻底删除 Agent。该 Agent 有会话打开时拒绝；不可恢复。
    pub fn delete_agent(&self, name: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.iter().any(|(key, session)| {
            key.agent_name == name && session.temporary && session.running.is_running()
        }) {
            return Err(format!("Agent「{name}」的临时测试仍在运行，请先停止再删除"));
        }
        if sessions
            .iter()
            .any(|(key, session)| key.agent_name == name && !session.temporary)
        {
            return Err(format!(
                "Agent「{name}」仍有会话打开，请先打开其他会话或新建会话，再删除"
            ));
        }
        sessions.retain(|key, session| !(key.agent_name == name && session.temporary));
        drop(sessions);
        agents::delete_agent(name)
    }

    /// 保存 Agent 定义时，临时测试必须空闲；成功后丢弃旧测试上下文，确保下一轮
    /// 从完整的新定义（模型、权限、环境与工作目录）重新构造。
    pub fn save_agent_definition(&self, def: &AgentDefinition) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.iter().any(|(key, session)| {
            key.agent_name == def.name && session.temporary && session.running.is_running()
        }) {
            return Err("临时测试仍在运行，请先停止再保存设置".into());
        }
        agents::save_agent(def)?;
        sessions.retain(|key, session| !(key.agent_name == def.name && session.temporary));
        Ok(())
    }

    /// 写入 AGENTS.md / memory 后同样让旧测试上下文失效，避免界面声称已经测试了
    /// 尚未注入的文件内容。
    pub fn write_agent_file(
        &self,
        agent_name: &str,
        rel_path: &str,
        content: &str,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.iter().any(|(key, session)| {
            key.agent_name == agent_name && session.temporary && session.running.is_running()
        }) {
            return Err("临时测试仍在运行，请先停止再保存文件".into());
        }
        agents::write_agent_file(agent_name, rel_path, content)?;
        sessions.retain(|key, session| !(key.agent_name == agent_name && session.temporary));
        Ok(())
    }

    /// 彻底删除已归档 Agent。不可恢复。
    pub fn delete_archived_agent(&self, name: &str) -> Result<(), String> {
        agents::delete_archived_agent(name)
    }
}

fn resolve_model(
    def: &AgentDefinition,
    session_model: Option<&Model>,
    env: Option<&std::collections::BTreeMap<String, String>>,
) -> Result<(Model, String), String> {
    let target = session_model
        .cloned()
        .or_else(|| def.provider.clone())
        .ok_or_else(|| {
            "该 Agent 还未配置默认模型，且当前会话未选择模型（请在 Agent 详情页绑定，或在会话顶部选择模型）".to_string()
        })?;
    let settings = load_settings();
    let api_key = settings
        .providers
        .iter()
        .find(|p| p.api == target.api && p.base_url == target.base_url)
        .and_then(|p| match env {
            // 与 bash 子进程消费同一份 resolved env；无契约上下文时回退进程环境
            Some(env) => p.resolve_api_key_in(env),
            None => p.resolve_api_key(),
        })
        .ok_or_else(|| {
            format!(
                "提供商 {} 未配置 API 密钥（设置 → 模型提供商）",
                target.base_url
            )
        })?;
    Ok((
        Model {
            id: target.id.clone(),
            name: target.name.clone(),
            api: target.api,
            base_url: target.base_url.clone(),
            max_tokens: if target.max_tokens == 0 {
                8192
            } else {
                target.max_tokens
            },
            context_window: target.context_window,
        },
        api_key,
    ))
}

fn writer_session_id(writer: &Arc<Mutex<SessionWriter>>) -> Result<String, String> {
    let writer = writer
        .lock()
        .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
    Ok(writer.session_id().to_string())
}

/// 载入自愈：会话尾部若留下未被回答的工具调用（上一次运行被进程中断 ——
/// 应用重启、强杀，结果没来得及落盘），追加合成的失败结果条目。
///
/// 只处理**尾部**：悬挂的 assistant 消息此时就是文件 tip，追加的 tool 结果
/// 成为它的子节点，消息顺序天然正确，会话记录被修复成合法状态（append-only，
/// 不重写任何已有条目）。历史中段的悬挂无法原地修复（树的顺序不可变），
/// 由发送前的 [`crate::context::repair_tool_pairing`] 兜底 —— 对齐 pi 的
/// post-tools / recovery 在回合结束时结算「orphaned / aborted」调用的做法。
fn settle_unanswered_tail(
    writer: &mut SessionWriter,
    messages: &[Message],
) -> Result<Vec<Message>, String> {
    let Some(last) = messages.last() else {
        return Ok(Vec::new());
    };
    let calls: Vec<(String, String)> = last
        .tool_calls()
        .iter()
        .filter_map(|call| match call {
            crate::types::ContentBlock::ToolCall { id, name, .. } => {
                Some((id.clone(), name.clone()))
            }
            _ => None,
        })
        .collect();
    if calls.is_empty() {
        return Ok(Vec::new());
    }
    let mut appended = Vec::with_capacity(calls.len());
    for (id, name) in calls {
        let result = crate::context::missing_tool_result(&id, &name);
        writer
            .append_message(&result)
            .map_err(|error| error.to_string())?;
        appended.push(result);
    }
    Ok(appended)
}

/// 投影式压缩流水线的 transform 钩子（组装请求时应用，不落盘）。
fn projection_transform(
    budget: crate::compaction::Budget,
) -> crate::agent_loop::TransformContextHook {
    Arc::new(move |messages: Vec<Message>| crate::compaction::project(messages, budget))
}

/// 把「保留区间下标」换算成条目 id（compaction 条目的 `keep_from_entry`）。
///
/// 内存历史与落盘条目必须一一对应（见 `session::replay` 的 id 语义）。两边
/// 长度不一致说明二者脱节 —— 此时退回空 id：宁可只留摘要，也不让回放去猜
/// 一个错误的位置（猜错会让保留段整体错位）。
fn keep_from_entry_id(
    writer: &Arc<Mutex<SessionWriter>>,
    history: &[Message],
    keep_from: Option<usize>,
) -> String {
    let Some(index) = keep_from else {
        return String::new();
    };
    let Ok(writer) = writer.lock() else {
        return String::new();
    };
    let ids = writer.message_ids();
    if ids.len() != history.len() {
        eprintln!(
            "pipi: 压缩时内存历史（{} 条）与落盘条目（{} 条）不一致，本轮只保留摘要",
            history.len(),
            ids.len()
        );
        return String::new();
    }
    ids.get(index).cloned().unwrap_or_default()
}

/// 压缩的触发方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompactionTrigger {
    /// turn 边界：历史达到触发线才压。
    Auto,
    /// 用户手动点：不管阈值都压（历史太短时返回可读错误）。
    Manual,
}

/// 跑一次压缩所需的全部素材 —— 自动（turn 边界）与手动（UI 按钮）共用，
/// 保证「摘要 → 分叉/归档 → 换会话 → 落事件」只有一条实现。
struct CompactionContext<'a> {
    agent_name: &'a str,
    /// 换会话后就地改成新 id，后续事件才会带新身份。
    session_id: &'a mut String,
    run_id: usize,
    model: &'a Model,
    api_key: &'a str,
    budget: crate::compaction::Budget,
    messages: &'a Arc<tokio::sync::Mutex<Vec<Message>>>,
    writer: &'a Arc<Mutex<SessionWriter>>,
    stats: &'a Arc<Mutex<SessionStatsTracker>>,
    abort: AbortSignal,
    sessions_dir: Option<&'a std::path::Path>,
    settings: crate::settings::CompactionSettings,
    /// 摘要调用失败重发策略（与对话共用，见 `retry` 模块）。
    retry: crate::retry::RetryPolicy,
    sink: &'a EventEmitter,
    trigger: CompactionTrigger,
    /// 压缩分叉换了会话 id 后，把 map 里的条目搬到新键（None = 不需要，如子 Agent）。
    rekey: Option<SessionRekey>,
}

/// 构造「把条目从旧键搬到新键」的回调（同一把锁、同一个 Session 对象）。
fn make_rekey(sessions: Arc<Mutex<HashMap<SessionKey, Session>>>) -> SessionRekey {
    Arc::new(move |old: &SessionKey, new_session_id: &str| {
        let Ok(mut map) = sessions.lock() else {
            return;
        };
        if let Some(session) = map.remove(old) {
            map.insert(SessionKey::new(&old.agent_name, new_session_id), session);
        }
    })
}

/// 压缩本体。`Ok(false)` 表示按触发方式判断「无需压缩」（自动且未达触发线）——
/// 调用方不应把它当失败；`Err` 才是真的没压成（手动时的「历史还不用压」也走这里）。
async fn run_compaction(ctx: CompactionContext<'_>) -> Result<bool, String> {
    let history = ctx.messages.lock().await.clone();
    if ctx.trigger == CompactionTrigger::Auto
        && !crate::compaction::needs_compaction(&history, ctx.budget)
    {
        return Ok(false);
    }
    // session_id 显式传入：换会话后就地改写它，之后的 envelope 才会带新身份
    let agent_name = ctx.agent_name.to_string();
    let run_id = ctx.run_id;
    let envelope = |session_id: &str, event: AgentEvent| {
        RuntimeEvent::AgentEvent(AgentEventEnvelope {
            agent_name: agent_name.clone(),
            session_id: session_id.to_string(),
            run_id,
            event,
        })
    };
    (ctx.sink)(envelope(ctx.session_id, AgentEvent::CompactionStart));
    let tokens_before = crate::context::estimate_context_tokens(&history);

    // 摘要是一次性提示词：不带会话标识（供应商按会话做缓存写入，这份 prompt
    // 不会被复用），用量另计入会话账本。
    let summary_options = StreamOptions {
        api_key: Some(ctx.api_key.to_string()),
        temperature: None,
        max_tokens: None,
        timeout_secs: 300,
        session_id: None,
    };
    // provider 实例要活到 compact 调用结束（provider_for 返回 Arc）
    let summarizer = provider_for(ctx.model.api);
    let strategy_env = crate::compaction::StrategyEnv {
        provider: summarizer.as_ref(),
        model: ctx.model,
        options: &summary_options,
        abort: ctx.abort.clone(),
        retry: ctx.retry,
    };
    let compacted = crate::compaction::compact(&history, ctx.budget, &strategy_env).await?;

    // 落盘的摘要正文是**未包裹**的原文（回放时统一包裹，见 session::summary_message）。
    let summary = compacted
        .messages
        .first()
        .and_then(crate::session::summary_text)
        .unwrap_or_default()
        .to_string();
    // 保留区间的起点条目 id：回放据此留住这段原文（缺了它就退回旧行为）。
    let keep_from_entry = keep_from_entry_id(ctx.writer, &history, compacted.keep_from);
    let source_tip = ctx
        .writer
        .lock()
        .ok()
        .and_then(|writer| writer.tip_id().map(str::to_string))
        .unwrap_or_default();
    let record = crate::session::CompactionRecord {
        summary: &summary,
        strategy: compacted.strategy,
        keep_from_entry: &keep_from_entry,
        source_tip: &source_tip,
        usage: compacted.usage,
    };
    // 落盘（分叉 / 原地由设置决定）。失败则内存历史不动 —— live 与落盘必须
    // 一致，否则重开会话会退回未压缩。
    let switched = persist_compaction(ctx.writer, ctx.sessions_dir, &record, ctx.settings)?;
    *ctx.messages.lock().await = compacted.messages;
    if let Some(usage) = compacted.usage {
        if let Ok(mut tracker) = ctx.stats.lock() {
            tracker.record_ledger(&usage);
        }
    }
    let tokens_after = crate::context::estimate_context_tokens(&ctx.messages.lock().await);
    // 换会话：先发 SessionSwitched（envelope 用旧 id，前端此刻身份还是旧的），
    // 把身份切到新 id，再发后续事件 —— 顺序反了会导致前端收不到切换、running 卡住。
    if let Some(new_id) = &switched {
        // 先把 map 里的条目搬到新键（同一条会话、新 id），再让前端与后续事件跟上 ——
        // 否则键会指向旧 id：归档/删除的占用检查与 session_infos 会各说各话。
        if let Some(rekey) = &ctx.rekey {
            rekey(&SessionKey::new(ctx.agent_name, ctx.session_id), new_id);
        }
        (ctx.sink)(RuntimeEvent::SessionSwitched(SessionSwitchedEnvelope {
            agent_name: ctx.agent_name.to_string(),
            session_id: ctx.session_id.clone(),
            run_id: ctx.run_id,
            to_session_id: new_id.clone(),
            archived: ctx.settings.archive_original,
        }));
        *ctx.session_id = new_id.clone();
    }
    (ctx.sink)(envelope(
        ctx.session_id,
        AgentEvent::CompactionEnd {
            summary,
            replaced: compacted.replaced as u64,
            strategy: compacted.strategy.to_string(),
            tokens_before,
            tokens_after,
        },
    ));
    Ok(true)
}

/// 把压缩结果落盘，返回 `Some(new_session_id)` 表示已换到新会话。
///
/// 两条路径：
/// - **分叉**（`fork_before_compact`）：新会话文件 = 原文件活跃路径的完整拷贝 +
///   压缩条目；随后把 writer 换过去（旧文件在这时关闭），最后按设置归档原文件。
/// - **原地**（关闭分叉，或分叉失败时的回退）：在当前文件上追加压缩条目。
///
/// 分叉失败不是致命错误（回退原地）；原地追加失败才是 `Err`，此时调用方不得
/// 替换内存历史 —— live 与落盘必须一致。
fn persist_compaction(
    writer: &Arc<Mutex<SessionWriter>>,
    sessions_dir: Option<&std::path::Path>,
    record: &crate::session::CompactionRecord<'_>,
    settings: crate::settings::CompactionSettings,
) -> Result<Option<String>, String> {
    // 临时测试上下文需要摘要替换，但绝不能分叉或写入 sessions 目录。内存账本
    // 仍追加 compaction entry，以保持 message id 映射与正式会话一致。
    {
        let mut guard = writer
            .lock()
            .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
        if guard.is_temporary() {
            guard
                .append_compaction(record)
                .map_err(|error| format!("无法更新临时压缩账本: {error}"))?;
            return Ok(None);
        }
    }

    let mut fork_error: Option<String> = None;
    if settings.fork_before_compact {
        match fork_and_write_compaction(writer, sessions_dir, record, settings.archive_original) {
            Ok(new_id) => return Ok(Some(new_id)),
            Err(error) => fork_error = Some(error),
        }
    }
    let mut guard = writer
        .lock()
        .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
    guard
        .append_compaction(record)
        .map_err(|error| format!("无法写入压缩条目: {error}"))?;
    if let Some(error) = fork_error {
        eprintln!("pipi: 压缩分叉失败，已改为原地压缩: {error}");
    }
    Ok(None)
}

/// 分叉出新会话并把压缩条目写进去，然后归档原会话（可关）。
///
/// 顺序很关键：**先在内存外写好新文件，再换 writer**（换的那一刻旧文件句柄关闭），
/// 最后才 rename 归档 —— 否则 Linux 上仍打开的 fd 会继续往被移动的文件追加。
fn fork_and_write_compaction(
    writer: &Arc<Mutex<SessionWriter>>,
    sessions_dir: Option<&std::path::Path>,
    record: &crate::session::CompactionRecord<'_>,
    archive_original: bool,
) -> Result<String, String> {
    let sessions_dir = sessions_dir.ok_or_else(|| "无法解析会话目录".to_string())?;
    let (source_path, source_id) = {
        let guard = writer
            .lock()
            .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
        let path = guard
            .persistent_path()
            .ok_or_else(|| "临时测试会话不能分叉".to_string())?
            .to_path_buf();
        let id = guard.session_id().to_string();
        (path, id)
    };

    let mut forked = crate::session::fork_session(&source_path, sessions_dir, None)?;
    let new_id = forked
        .path()
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| "无法解析新会话 ID".to_string())?
        .to_string();
    if let Err(error) = forked.append_compaction(record) {
        // 写失败就删掉半成品，别在会话列表里留下空壳
        let path = forked.path().to_path_buf();
        drop(forked);
        let _ = std::fs::remove_file(&path);
        return Err(format!("无法把压缩条目写入新会话: {error}"));
    }
    // 溯源标记：回答「这个会话从哪来」（回放中性，见 EntryKind::Custom）
    let _ = forked.append_custom(&format!("compaction-fork:{source_id}"));

    {
        let mut guard = writer
            .lock()
            .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
        let previous = std::mem::replace(&mut *guard, forked);
        drop(previous); // 关闭原文件句柄 —— 归档前必须做到
    }

    if archive_original {
        if let Err(error) = crate::session::archive_session_file(sessions_dir, &source_id) {
            // 归档失败不回滚：新会话已经可用，原会话留在活跃列表里即可
            eprintln!("pipi: 原会话归档失败（保留在活跃列表）: {error}");
        }
    }
    Ok(new_id)
}

fn make_emitter(
    sink: EventEmitter,
    writer: Arc<Mutex<SessionWriter>>,
    stats: Arc<Mutex<SessionStatsTracker>>,
    agent_name: String,
    session_id: String,
    run_id: usize,
) -> LoopEmitter {
    Arc::new(move |event: AgentEvent| {
        // AgentEnd 延迟到 run loop 回写完整历史后统一发送。
        if matches!(&event, AgentEvent::AgentEnd { .. }) {
            return;
        }
        sink(RuntimeEvent::AgentEvent(AgentEventEnvelope {
            agent_name: agent_name.clone(),
            session_id: session_id.clone(),
            run_id,
            event: event.clone(),
        }));
        if let AgentEvent::MessageEnd { message } = &event {
            match writer.lock() {
                Ok(mut writer) => {
                    if let Err(error) = writer.append_message(message) {
                        sink(RuntimeEvent::SessionError(SessionErrorEnvelope {
                            agent_name: agent_name.clone(),
                            session_id: session_id.clone(),
                            run_id,
                            message: format!("会话消息落盘失败: {error}"),
                        }));
                    }
                }
                Err(error) => {
                    sink(RuntimeEvent::SessionError(SessionErrorEnvelope {
                        agent_name: agent_name.clone(),
                        session_id: session_id.clone(),
                        run_id,
                        message: format!("无法锁定会话写入器: {error}"),
                    }));
                }
            }
            if message.role() == "assistant" {
                match stats.lock() {
                    Ok(mut tracker) => {
                        tracker.record(message);
                        sink(RuntimeEvent::SessionStats(StatsEventEnvelope {
                            agent_name: agent_name.clone(),
                            session_id: session_id.clone(),
                            run_id,
                            stats: tracker.snapshot(),
                        }));
                    }
                    Err(error) => {
                        sink(RuntimeEvent::SessionError(SessionErrorEnvelope {
                            agent_name: agent_name.clone(),
                            session_id: session_id.clone(),
                            run_id,
                            message: format!("无法更新会话统计: {error}"),
                        }));
                    }
                }
            }
        }
    })
}

/// 不占用 [`RuntimeState`] 当前会话槽的一次性 Agent 执行器。
///
/// `run_agent` 在父循环的工具调用内 await 本执行器；目标 Agent 使用独立的新
/// session，并且只注册基础工具，所以第一阶段不会递归调用其他 Agent。
struct RuntimeAgentRunner;

#[async_trait::async_trait]
impl AgentRunner for RuntimeAgentRunner {
    async fn run_once(
        &self,
        agent_name: &str,
        prompt: &str,
        abort: AbortSignal,
        progress: Option<ChildProgressTx>,
    ) -> Result<AgentRunResult, String> {
        run_agent_once_inner(
            agent_name,
            prompt,
            abort,
            std::time::Duration::from_secs(CHILD_RUN_TIMEOUT_SECS),
            progress,
        )
        .await
    }
}

/// child session 一旦建立，后续的配置/环境错误也要成为可读取的运行输出，
/// 不能只给父 Agent 返回一个瞬时错误并留下空 JSONL。
fn persist_agent_start_failure(
    writer: &Arc<Mutex<SessionWriter>>,
    definition: &AgentDefinition,
    session_id: &str,
    prompt: &str,
    error: String,
) -> Result<AgentRunResult, String> {
    let user_message = Message::user_text(prompt);
    let model_name = definition
        .provider
        .as_ref()
        .map(|model| model.display_name().to_string())
        .filter(|name| !name.is_empty())
        .or_else(|| (!definition.model.is_empty()).then(|| definition.model.clone()))
        .unwrap_or_else(|| "unconfigured".into());
    let failure = Message::assistant_error(error, &model_name, StopReason::Error);
    {
        let mut writer = writer
            .lock()
            .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
        writer
            .append_message(&user_message)
            .map_err(|error| format!("无法写入用户消息: {error}"))?;
        writer
            .append_message(&failure)
            .map_err(|error| format!("无法写入 Agent 启动错误: {error}"))?;
    }
    Ok(result_from_messages(
        &definition.name,
        session_id,
        &[failure],
    ))
}

/// 在目标 Agent 下创建一个独立 session 并同步运行到结束。
/// 不占用 UI 当前会话槽，也不向 child 注入 Agent 组合工具。
pub async fn run_agent_once(
    agent_name: &str,
    prompt: &str,
    abort: AbortSignal,
) -> Result<AgentRunResult, String> {
    run_agent_once_inner(
        agent_name,
        prompt,
        abort,
        std::time::Duration::from_secs(CHILD_RUN_TIMEOUT_SECS),
        None,
    )
    .await
}

/// 子 Agent 单次运行的时间上限。到时终止并落盘终态，避免父会话无限阻塞。
pub const CHILD_RUN_TIMEOUT_SECS: u64 = 600;

/// `run_agent_once` 的可注入版本：`timeout` 供测试收紧，`progress` 把子运行
/// 里程碑（工具调用、轮次完成）转发给父 Agent 的工具更新流。
pub(crate) async fn run_agent_once_inner(
    agent_name: &str,
    prompt: &str,
    abort: AbortSignal,
    timeout: std::time::Duration,
    progress: Option<ChildProgressTx>,
) -> Result<AgentRunResult, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("空消息".into());
    }

    let definition = agents::load_agent(agent_name)?;
    let sessions_dir = definition
        .sessions_dir()
        .ok_or_else(|| "无法解析会话目录".to_string())?;
    let writer = Arc::new(Mutex::new(
        SessionWriter::create(&sessions_dir).map_err(|error| error.to_string())?,
    ));
    let session_id = writer_session_id(&writer)?;

    let (tool_context, resolved_env) =
        match agents::build_tool_context(&definition, Some(session_id.clone()), abort.clone()) {
            Ok(context) => context,
            Err(error) => {
                return persist_agent_start_failure(
                    &writer,
                    &definition,
                    &session_id,
                    prompt,
                    error,
                )
            }
        };
    {
        let declared = resolved_env
            .provenance
            .iter()
            .filter(|(_, source)| source.as_str() != av::resolve::PROCESS_SOURCE)
            .map(|(key, source)| crate::session::EnvDeclared {
                key: key.clone(),
                source: source.clone(),
            })
            .collect();
        writer
            .lock()
            .map_err(|error| format!("无法锁定会话写入器: {error}"))?
            .append_env(declared)
            .map_err(|error| format!("无法写入环境记账: {error}"))?;
    }

    let (model, api_key) = match resolve_model(&definition, None, Some(&resolved_env.vars)) {
        Ok(resolved) => resolved,
        Err(error) => {
            return persist_agent_start_failure(&writer, &definition, &session_id, prompt, error)
        }
    };
    // 有意只构造基础工具。即使目标 agent.json 显式启用了 Agent 组合工具，
    // 它作为 child 运行时也不会拿到这些工具，从而把首版嵌套深度固定为 1。
    let registry = Arc::new(ToolRegistry::for_context(&tool_context));
    let wire_tools = registry.wire_tools();
    let system_prompt = match agents::build_system_prompt_with_tools(&definition, &wire_tools) {
        Ok(system_prompt) => system_prompt,
        Err(error) => {
            return persist_agent_start_failure(&writer, &definition, &session_id, prompt, error)
        }
    };
    let user_message = Message::user_text(prompt);
    writer
        .lock()
        .map_err(|error| format!("无法锁定会话写入器: {error}"))?
        .append_message(&user_message)
        .map_err(|error| format!("无法写入用户消息: {error}"))?;

    let context = AgentContext {
        system_prompt,
        messages: vec![user_message],
    };
    let context_window = model.context_window;
    let stats = Arc::new(Mutex::new(SessionStatsTracker::new(
        (context_window > 0).then_some(context_window),
    )));
    let result_writer = writer.clone();
    // 子运行进度：把里程碑转发给父工具的 on_update（父会话里能看到 child
    // 在做什么）；无观察者时事件照旧丢弃。
    let progress_sink: EventEmitter = {
        let progress = progress.clone();
        let turns = Arc::new(AtomicUsize::new(0));
        Arc::new(move |event| {
            let RuntimeEvent::AgentEvent(envelope) = event else {
                return;
            };
            let Some(progress) = &progress else {
                return;
            };
            match envelope.event {
                AgentEvent::ToolExecutionStart { tool_name, .. } => {
                    let _ = progress.send(crate::tools::ToolOutput::text(format!(
                        "子 Agent 正在调用工具 {tool_name}"
                    )));
                }
                AgentEvent::MessageEnd { message } if message.role() == "assistant" => {
                    let turn = turns.fetch_add(1, Ordering::AcqRel) + 1;
                    let _ = progress.send(crate::tools::ToolOutput::text(format!(
                        "子 Agent 完成第 {turn} 轮回复"
                    )));
                }
                _ => {}
            }
        })
    };
    let emitter = make_emitter(
        progress_sink,
        writer,
        stats,
        definition.name.clone(),
        session_id.clone(),
        NEXT_RUN_ID.fetch_add(1, Ordering::AcqRel),
    );
    let config = AgentLoopConfig {
        model: model.clone(),
        provider: provider_for(model.api),
        tools: registry,
        tool_context,
        options: StreamOptions {
            api_key: Some(api_key),
            temperature: None,
            max_tokens: Some(model.max_tokens),
            timeout_secs: 300,
            session_id: Some(session_id.clone()),
        },
        retry: load_settings().retry.policy(),
        tool_execution: ToolExecutionMode::Parallel,
        steering: MessageQueue::new(),
        follow_up: MessageQueue::new(),
        before_tool_call: None,
        after_tool_call: None,
        // 子 Agent 运行同样走投影式压缩流水线（清旧工具输出 → 硬裁），
        // 阈值用子 Agent 自己的 agent.json 配置
        transform_context: (context_window > 0).then(|| {
            projection_transform(crate::compaction::Budget::from_window(
                context_window,
                definition.compact_threshold_percent(),
            ))
        }),
    };
    let abort_for_result = abort.clone();
    // 超时取消整个循环 future（bash 子进程有 kill_on_drop 兜底），并落盘
    // 可读取的终态，避免 read_agent 把超时任务永远报成 pending。
    let new_messages = match tokio::time::timeout(
        timeout,
        run_agent_loop(Vec::new(), context, config, emitter, abort),
    )
    .await
    {
        Ok(new_messages) => new_messages,
        Err(_) => {
            let timeout_error = Message::assistant_error(
                format!("子任务运行超时（{} 秒），已终止", timeout.as_secs()),
                model.display_name(),
                StopReason::Error,
            );
            result_writer
                .lock()
                .map_err(|error| format!("无法锁定会话写入器: {error}"))?
                .append_message(&timeout_error)
                .map_err(|error| format!("无法写入超时状态: {error}"))?;
            return Ok(result_from_messages(
                &definition.name,
                &session_id,
                &[timeout_error],
            ));
        }
    };
    let mut result = result_from_messages(&definition.name, &session_id, &new_messages);
    // 如果取消恰好发生在一次工具调用完成之后，loop 没有机会再生成一条
    // assistant aborted 消息。补写终态，避免 read_agent 永远把已结束任务报成 pending。
    if abort_for_result.is_aborted() && result.status == AgentRunStatus::Pending {
        let aborted = Message::assistant_error("已中止", model.display_name(), StopReason::Aborted);
        result_writer
            .lock()
            .map_err(|error| format!("无法锁定会话写入器: {error}"))?
            .append_message(&aborted)
            .map_err(|error| format!("无法写入 Agent 中止状态: {error}"))?;
        result = result_from_messages(&definition.name, &session_id, &[aborted]);
    }
    Ok(result)
}

/// 列出指定 Agent 的会话摘要。
pub fn list_sessions(agent_name: &str) -> Result<Vec<SessionSummary>, String> {
    let def = agents::load_agent(agent_name)?;
    let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
    Ok(list_session_summaries(&dir))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub agent_name: String,
    pub session_id: String,
    /// true = 设置工作台的内存测试会话，不对应 sessions/*.jsonl。
    pub temporary: bool,
    pub running: bool,
    pub run_id: usize,
    pub model: Option<Model>,
    pub is_custom_model: bool,
}

impl RuntimeState {
    /// 从会话文件载入一条会话（含尾部自愈与账本重建）。不插入 map —— 调用方
    /// 在全部前置步骤成功后再插入。
    fn load_session_object(
        def: &AgentDefinition,
        path: &std::path::Path,
    ) -> Result<Session, String> {
        let entries = load_session(path).map_err(|e| e.to_string())?;
        let active = crate::session::active_path(&entries);
        let mut messages = crate::session::rebuild_messages(&active);
        let active_model = crate::session::active_model_from_entries(&entries);
        let effective_model = active_model.as_ref().or(def.provider.as_ref());
        let context_max = effective_model
            .map(|provider| provider.context_window)
            .filter(|window| *window > 0);
        // 载入自愈：上次运行被进程中断时，尾部会留下无人回答的工具调用 ——
        // 补上合成的失败结果并落盘，否则之后每次请求都会被端点按协议拒绝。
        let mut writer = SessionWriter::open(path).map_err(|e| e.to_string())?;
        match settle_unanswered_tail(&mut writer, &messages) {
            Ok(appended) => messages.extend(appended),
            Err(error) => eprintln!("pipi: 会话尾部修复失败（继续载入）: {error}"),
        }
        let mut tracker = SessionStatsTracker::new(context_max);
        for message in &messages {
            tracker.record(message);
        }
        // 摘要压缩的用量也进账本（pi 把摘要成本计入会话总量）：只加累计值，
        // 不改写「最近一次调用」口径 —— 摘要 prompt 不是当前上下文占用。
        for entry in &active {
            if let EntryKind::Compaction {
                usage: Some(usage), ..
            } = &entry.kind
            {
                tracker.record_ledger(usage);
            }
        }
        Ok(Session {
            agent: def.clone(),
            messages: Arc::new(tokio::sync::Mutex::new(messages)),
            writer: Arc::new(Mutex::new(writer)),
            temporary: false,
            stats: Arc::new(Mutex::new(tracker)),
            abort: AbortSignal::new(),
            running: Arc::new(RunState::new()),
            model: Arc::new(Mutex::new(active_model)),
            steering: MessageQueue::new(),
        })
    }

    /// 新建一条会话文件并构造会话对象（返回新会话 id）。不插入 map。
    fn create_session_object(
        def: &AgentDefinition,
        model: Option<&Model>,
    ) -> Result<(Session, String), String> {
        let sessions_dir = def.sessions_dir().ok_or("无法解析会话目录")?;
        let mut writer = SessionWriter::create(&sessions_dir).map_err(|e| e.to_string())?;
        let session_id = writer.session_id().to_string();
        let active_model = model.cloned();
        let effective_model = active_model.as_ref().or(def.provider.as_ref());
        let context_max = effective_model
            .map(|provider| provider.context_window)
            .filter(|window| *window > 0);
        if let Some(target) = &active_model {
            if Some(target) != def.provider.as_ref() {
                writer
                    .append_model_change(target)
                    .map_err(|e| e.to_string())?;
            }
        }
        let session = Session {
            agent: def.clone(),
            messages: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            writer: Arc::new(Mutex::new(writer)),
            temporary: false,
            stats: Arc::new(Mutex::new(SessionStatsTracker::new(context_max))),
            abort: AbortSignal::new(),
            running: Arc::new(RunState::new()),
            model: Arc::new(Mutex::new(active_model)),
            steering: MessageQueue::new(),
        };
        Ok((session, session_id))
    }

    /// 构造设置工作台的临时测试会话。不创建目录或文件，完整复用 Agent 的模型、
    /// 工具、权限、工作目录与环境契约；只有对话账本本身是易失的。
    fn create_test_session_object(def: &AgentDefinition) -> (Session, String) {
        let writer = SessionWriter::temporary();
        let session_id = writer.session_id().to_string();
        let context_max = def
            .provider
            .as_ref()
            .map(|provider| provider.context_window)
            .filter(|window| *window > 0);
        let session = Session {
            agent: def.clone(),
            messages: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            writer: Arc::new(Mutex::new(writer)),
            temporary: true,
            stats: Arc::new(Mutex::new(SessionStatsTracker::new(context_max))),
            abort: AbortSignal::new(),
            running: Arc::new(RunState::new()),
            model: Arc::new(Mutex::new(None)),
            steering: MessageQueue::new(),
        };
        (session, session_id)
    }

    /// 返回该 Agent 在应用运行期唯一的临时测试会话；没有则在内存中创建。
    pub fn ensure_test_session(&self, agent_name: &str) -> Result<SessionInfo, String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if let Some(session) = sessions.iter().find_map(|(key, session)| {
            (key.agent_name == agent_name && session.temporary).then_some(session)
        }) {
            return Self::session_info_of(session);
        }

        // 定义读取也放在 sessions 锁内，与 save_agent_definition 的「写文件 +
        // 丢弃旧测试」串行，避免并发保存时用保存前的快照新建测试会话。
        let definition = agents::load_agent(agent_name)?;
        let (session, session_id) = Self::create_test_session_object(&definition);
        let info = Self::session_info_of(&session)?;
        sessions.insert(SessionKey::new(agent_name, &session_id), session);
        Ok(info)
    }

    /// 丢弃该 Agent 的空闲临时上下文并用最新保存的 Agent 定义新建一条。
    pub fn reset_test_session(&self, agent_name: &str) -> Result<SessionInfo, String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.iter().any(|(key, session)| {
            key.agent_name == agent_name && session.temporary && session.running.is_running()
        }) {
            return Err("临时测试仍在运行，请先停止再清空或保存设置".into());
        }
        sessions.retain(|key, session| !(key.agent_name == agent_name && session.temporary));
        let definition = agents::load_agent(agent_name)?;
        let (session, session_id) = Self::create_test_session_object(&definition);
        let info = Self::session_info_of(&session)?;
        sessions.insert(SessionKey::new(agent_name, &session_id), session);
        Ok(info)
    }

    /// 打开（续写）一个已有会话。已经打开时是幂等的空操作 —— 同一个 Agent 的
    /// 多条会话可以同时打开，打开其中一条不影响别的。
    pub fn open_session(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let def = agents::load_agent(agent_name)?;
        let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
        let path = dir.join(format!("{session_id}.jsonl"));
        if !path.is_file() {
            return Err("会话不存在".into());
        }

        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let key = SessionKey::new(agent_name, session_id);
        if sessions.contains_key(&key) {
            return Ok(());
        }
        let session = Self::load_session_object(&def, &path)?;
        sessions.insert(key, session);
        Ok(())
    }

    /// 分叉一个会话（可指定截断至某个 entry_id，留空则分叉到当前 tip）。
    pub fn fork_session(
        &self,
        agent_name: &str,
        session_id: &str,
        up_to_entry_id: Option<&str>,
    ) -> Result<SessionInfo, String> {
        if session_id.is_empty()
            || !session_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err("非法会话 ID".into());
        }
        let def = agents::load_agent(agent_name)?;
        let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
        let source_path = dir.join(format!("{session_id}.jsonl"));
        if !source_path.is_file() {
            return Err("源会话不存在".into());
        }

        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        // 源会话正在跑时不允许分叉（分叉会复制一份半途的历史）
        if sessions
            .get(&SessionKey::new(agent_name, session_id))
            .is_some_and(|session| session.running.is_running())
        {
            return Err(format!("会话「{session_id}」正在运行，请先停止再分叉"));
        }

        let writer = crate::session::fork_session(&source_path, &dir, up_to_entry_id)?;
        let new_session_path = writer.path().to_path_buf();
        let new_session_id = new_session_path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "无法解析新会话 ID".to_string())?
            .to_string();

        let entries = load_session(&new_session_path).map_err(|e| e.to_string())?;
        let active = crate::session::active_path(&entries);
        let mut messages = crate::session::rebuild_messages(&active);
        let active_model = crate::session::active_model_from_entries(&entries);
        let effective_model = active_model.clone().or_else(|| def.provider.clone());
        let context_max = effective_model
            .as_ref()
            .map(|provider| provider.context_window)
            .filter(|window| *window > 0);
        // 分叉出的会话同样载入自愈：源会话尾部若留下无主工具调用，分叉会
        // 把它原样复制过来（见 settle_unanswered_tail 的说明）。
        let mut writer = writer;
        match settle_unanswered_tail(&mut writer, &messages) {
            Ok(appended) => messages.extend(appended),
            Err(error) => eprintln!("pipi: 会话尾部修复失败（继续载入）: {error}"),
        }
        let mut tracker = SessionStatsTracker::new(context_max);
        for message in &messages {
            tracker.record(message);
        }
        // 摘要压缩的用量也进账本（分叉同样继承）
        for entry in &active {
            if let EntryKind::Compaction {
                usage: Some(usage), ..
            } = &entry.kind
            {
                tracker.record_ledger(usage);
            }
        }

        let run_state = Arc::new(RunState::new());
        // 分叉出的会话作为**新的一条**打开（源会话保持原样，不再被顶掉）
        sessions.insert(
            SessionKey::new(agent_name, &new_session_id),
            Session {
                agent: def,
                messages: Arc::new(tokio::sync::Mutex::new(messages)),
                writer: Arc::new(Mutex::new(writer)),
                temporary: false,
                stats: Arc::new(Mutex::new(tracker)),
                abort: AbortSignal::new(),
                running: run_state.clone(),
                model: Arc::new(Mutex::new(active_model.clone())),
                steering: MessageQueue::new(),
            },
        );

        Ok(SessionInfo {
            agent_name: agent_name.to_string(),
            session_id: new_session_id,
            temporary: false,
            running: false,
            run_id: run_state.current_run_id(),
            model: effective_model,
            is_custom_model: active_model.is_some(),
        })
    }

    /// 某条会话（Agent + 会话 id）的信息；没有打开返回 None。
    pub fn session_info(
        &self,
        agent_name: &str,
        session_id: &str,
    ) -> Result<Option<SessionInfo>, String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|error| format!("无法读取会话槽: {error}"))?;
        match sessions.get(&SessionKey::new(agent_name, session_id)) {
            Some(session) => Ok(Some(Self::session_info_of(session)?)),
            None => Ok(None),
        }
    }

    /// 所有打开中的会话。前端据此知道「哪些 Agent 正在跑」——多 Agent 并发下
    /// 运行态是集合而不是单值。
    pub fn session_infos(&self) -> Result<Vec<SessionInfo>, String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|error| format!("无法读取会话槽: {error}"))?;
        let mut infos = Vec::with_capacity(sessions.len());
        for session in sessions.values() {
            infos.push(Self::session_info_of(session)?);
        }
        Ok(infos)
    }

    fn session_info_of(session: &Session) -> Result<SessionInfo, String> {
        let writer = session
            .writer
            .lock()
            .map_err(|error| format!("无法读取会话路径: {error}"))?;
        let session_id = writer.session_id().to_string();
        let custom_model = session.model.lock().ok().and_then(|m| m.clone());
        let effective_model = custom_model
            .clone()
            .or_else(|| session.agent.provider.clone());
        let is_custom_model = custom_model.is_some();
        Ok(SessionInfo {
            agent_name: session.agent.name.clone(),
            session_id,
            temporary: session.temporary,
            running: session.running.is_running(),
            run_id: session.running.current_run_id(),
            model: effective_model,
            is_custom_model,
        })
    }

    /// 为某条会话设置或切换模型配置。
    /// 传入 None 时恢复为 Agent 默认模型。
    pub fn set_session_model(
        &self,
        agent_name: &str,
        session_id: &str,
        model: Option<Model>,
    ) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let Some(session) = sessions.get(&SessionKey::new(agent_name, session_id)) else {
            return Err(format!("会话「{session_id}」没有打开"));
        };
        if session.running.is_running() {
            return Err("会话正在运行，请等待完成或先停止后再切换模型".into());
        }

        let mut current_model_slot = session.model.lock().map_err(|e| e.to_string())?;
        let effective_target = model.clone().or_else(|| session.agent.provider.clone());

        // 校验目标模型的 API 密钥是否已配置
        if let Some(target) = &effective_target {
            let _ = resolve_model(&session.agent, Some(target), None)?;
        }

        let has_changed = *current_model_slot != model;
        if has_changed {
            if let Some(target) = &effective_target {
                let mut writer = session.writer.lock().map_err(|e| e.to_string())?;
                writer
                    .append_model_change(target)
                    .map_err(|e| e.to_string())?;
            }
            if let Some(target) = &effective_target {
                if let Ok(mut tracker) = session.stats.lock() {
                    let context_max = if target.context_window > 0 {
                        Some(target.context_window)
                    } else {
                        None
                    };
                    tracker.set_context_max(context_max);
                }
            }
            *current_model_slot = model;
        }
        Ok(())
    }

    /// 某条会话是否正在跑一轮。
    pub fn session_running(&self, agent_name: &str, session_id: &str) -> bool {
        self.sessions
            .lock()
            .map(|sessions| {
                sessions
                    .get(&SessionKey::new(agent_name, session_id))
                    .map(|session| session.running.is_running())
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// 停止某条会话的当前一轮；这条会话没有打开时是空操作。只停这一条 ——
    /// 同一个 Agent 的其他会话照跑。
    pub fn stop_run(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if let Some(session) = sessions.get(&SessionKey::new(agent_name, session_id)) {
            session.abort.abort();
        }
        Ok(())
    }

    /// 手动压缩某条会话（UI「立即压缩」）：跳过阈值预检，其余与自动压缩完全同一条
    /// 路径（摘要 → 分叉/归档 → 换会话）。
    ///
    /// 工作交给注入的运行时异步执行，结果通过事件回报 —— 前端据
    /// `compaction_start/end`、`session-switched`、`session-error` 更新界面。
    /// 这里会占住 running 位：与正在跑的一轮互斥，且 `stop_run` 能中止摘要调用。
    pub fn compact_now(
        &self,
        agent_name: &str,
        session_id: &str,
        event_sink: EventEmitter,
    ) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let key = SessionKey::new(agent_name, session_id);
        let Some(session) = sessions.get(&key) else {
            return Err(format!("会话「{session_id}」没有打开"));
        };
        if session.running.is_running() {
            return Err("会话正在运行，请先停止再压缩".into());
        }
        let def = session.agent.clone();
        let current_model = session.model.lock().ok().and_then(|model| model.clone());
        // 与发送消息同一套解析：会话模型优先，回退 Agent 默认
        let (model, api_key) = resolve_model(&def, current_model.as_ref(), None)?;
        let budget = crate::compaction::Budget::from_window(
            model.context_window,
            def.compact_threshold_percent(),
        );
        let sessions_dir = def.sessions_dir();
        let runtime_settings = load_settings();
        let settings = runtime_settings.compaction;
        let retry = runtime_settings.retry.policy();
        let messages = session.messages.clone();
        let writer = session.writer.clone();
        let stats = session.stats.clone();
        let abort = session.abort.clone();
        let running = session.running.clone();
        abort.reset();
        // 锁内占位：避免释放锁后与新一轮 send_prompt 抢跑
        let running_guard = RunningGuard::new(running.clone());
        let run_token = running_guard.token;
        let run_id = running.current_run_id();
        let sink = event_sink.clone();
        let sessions_map = self.sessions.clone();
        let rekey = make_rekey(sessions_map);

        self.spawn_run(async move {
            let _running_guard = running_guard;
            let mut session_id = writer_session_id(&writer).unwrap_or_default();
            // 发一对 Agent 事件：前端在 compaction_start 会把 running 置 true，
            // 而只有 agent_end 会清 —— 手动压缩没有 agent loop，缺了它界面会
            // 永久停在「运行中」（停止按钮、模型切换、新会话全被禁）。
            sink(RuntimeEvent::AgentEvent(AgentEventEnvelope {
                agent_name: def.name.clone(),
                session_id: session_id.clone(),
                run_id,
                event: AgentEvent::AgentStart,
            }));
            let outcome = run_compaction(CompactionContext {
                agent_name: &def.name,
                session_id: &mut session_id,
                run_id,
                model: &model,
                api_key: &api_key,
                budget,
                messages: &messages,
                writer: &writer,
                stats: &stats,
                abort: abort.clone(),
                sessions_dir: sessions_dir.as_deref(),
                settings,
                retry,
                sink: &sink,
                trigger: CompactionTrigger::Manual,
                rekey: Some(rekey),
            })
            .await;
            if let Err(error) = outcome {
                // 手动路径要让人看到原因（例如「历史还不用压」），走会话错误通道
                sink(RuntimeEvent::SessionError(SessionErrorEnvelope {
                    agent_name: def.name.clone(),
                    session_id: session_id.clone(),
                    run_id,
                    message: format!("压缩未执行：{error}"),
                }));
            }
            running.finish(run_token);
            sink(RuntimeEvent::AgentEvent(AgentEventEnvelope {
                agent_name: def.name.clone(),
                session_id,
                run_id,
                event: AgentEvent::AgentEnd {
                    messages: Vec::new(),
                },
            }));
        });
        Ok(())
    }

    /// 运行中插话（pi 的 steering）：消息注入该会话正在跑的下一轮上下文。
    /// 仅在会话正在运行时接受；消息本体由 loop 注入时经 emitter 落盘。
    /// 运行结束瞬间提交的插话由收割逻辑接住，不会丢。
    pub fn steer(&self, agent_name: &str, session_id: &str, message: &str) -> Result<(), String> {
        let message = message.trim();
        if message.is_empty() {
            return Err("空消息".into());
        }
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let Some(session) = sessions.get(&SessionKey::new(agent_name, session_id)) else {
            return Err(format!("会话「{session_id}」没有打开"));
        };
        if !session.running.is_running() {
            return Err("会话未在运行，请直接发送消息".into());
        }
        session.steering.push(Message::user_text(message));
        Ok(())
    }

    /// 回传一次审批请求的用户决定。请求已过期（超时 / 中止）时返回 Err。
    pub fn resolve_approval(
        &self,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        self.approval.resolve(request_id, decision)
    }

    /// 释放某个 Agent 名下**所有空闲**的会话（不落盘，文件留在磁盘上）。
    /// 正在跑的那条保留 —— 后台运行不会因为「离开这个 Agent」被清掉。
    pub fn new_session(&self, agent_name: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        sessions.retain(|key, session| {
            key.agent_name != agent_name || session.temporary || session.running.is_running()
        });
        Ok(())
    }

    pub async fn session_messages(
        &self,
        agent_name: &str,
        session_id: &str,
    ) -> Result<Vec<Message>, String> {
        let messages = {
            let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
            sessions
                .get(&SessionKey::new(agent_name, session_id))
                .map(|session| session.messages.clone())
        };
        match messages {
            Some(messages) => Ok(messages.lock().await.clone()),
            None => Ok(Vec::new()),
        }
    }

    pub fn session_stats(
        &self,
        agent_name: &str,
        session_id: &str,
    ) -> Result<crate::stats::SessionStats, String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        match sessions.get(&SessionKey::new(agent_name, session_id)) {
            Some(session) => Ok(session.stats.lock().map_err(|e| e.to_string())?.snapshot()),
            None => Ok(Default::default()),
        }
    }

    /// 启动一轮 Agent；完成后的事件通过 event_sink 广播给宿主。
    ///
    /// `session_id`：
    /// - `Some(id)` —— 在这条会话上跑：已打开就复用，没打开就从文件载入；
    /// - `None` —— 新建一条会话（新文件）。
    ///
    /// 并发口径：守卫是**按会话**的 —— 同一条会话同时只能跑一轮（硬不变量）；
    /// 同一个 Agent 的不同会话、不同 Agent 的会话都各自独立，可以同时跑。
    pub fn send_prompt(
        &self,
        agent_name: &str,
        session_id: Option<&str>,
        prompt: &str,
        model: Option<Model>,
        event_sink: EventEmitter,
    ) -> Result<(), String> {
        if let Some(id) = session_id {
            validate_session_id(id)?;
        }
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return Err("空消息".into());
        }

        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let def = agents::load_agent(agent_name)?;
        // 目标会话：显式指定且已打开 → 复用；否则要载入或新建
        let reuse_key = session_id
            .map(|id| SessionKey::new(agent_name, id))
            .filter(|key| sessions.contains_key(key));
        if let Some(key) = &reuse_key {
            if sessions[key].running.is_running() {
                return Err(format!(
                    "会话「{}」正在运行，请等待完成或先停止",
                    key.session_id
                ));
            }
        }
        // 载入 / 新建都在 map 之外完成，全部前置步骤成功后才插进 map ——
        // 中途失败必须保持 map 不变，否则半成品会被下一次 send_prompt 当成可复用的。
        let mut created: Option<(SessionKey, Session)> = None;
        if reuse_key.is_none() {
            let (key, session) = match session_id {
                Some(id) => {
                    let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
                    let path = dir.join(format!("{id}.jsonl"));
                    if !path.is_file() {
                        return Err("会话不存在".into());
                    }
                    (
                        SessionKey::new(agent_name, id),
                        Self::load_session_object(&def, &path)?,
                    )
                }
                None => {
                    let (session, new_id) = Self::create_session_object(&def, model.as_ref())?;
                    (SessionKey::new(agent_name, &new_id), session)
                }
            };
            created = Some((key, session));
        }
        let session = created
            .as_ref()
            .map(|(_, session)| session)
            .or_else(|| reuse_key.as_ref().map(|key| &sessions[key]))
            .ok_or("无法创建会话")?;
        // 若复用已有会话且传入了明确的模型变更请求
        if reuse_key.is_some() {
            if let Some(target) = &model {
                let mut current_model_slot = session.model.lock().map_err(|e| e.to_string())?;
                if current_model_slot.as_ref() != Some(target) {
                    let mut writer = session.writer.lock().map_err(|e| e.to_string())?;
                    writer
                        .append_model_change(target)
                        .map_err(|e| e.to_string())?;
                    if let Ok(mut tracker) = session.stats.lock() {
                        let context_max = if target.context_window > 0 {
                            Some(target.context_window)
                        } else {
                            None
                        };
                        tracker.set_context_max(context_max);
                    }
                    *current_model_slot = Some(target.clone());
                }
            }
        }

        let def = session.agent.clone();
        let current_session_model = session.model.lock().ok().and_then(|m| m.clone());

        let user_message = Message::user_text(prompt);
        let messages = session.messages.clone();
        let writer = session.writer.clone();
        let stats = session.stats.clone();
        let running = session.running.clone();
        let abort = session.abort.clone();
        let steering = session.steering.clone();
        // 收割上一轮结束后仍滞留在 steering 队列里的插话：随本轮一起送入模型
        //（由 emitter 的 MessageEnd 落盘，不会丢）。
        let queued = steering.drain();

        // av 环境契约：会话启动时解析一次（requires fail-closed + AV_* 注入）
        let (mut tool_context, resolved_env) =
            agents::build_tool_context(&def, writer_session_id(&writer).ok(), abort.clone())?;
        // 环境记账：每会话至多一条（writer 幂等去重）；只记声明键（非 process 来源）
        {
            let declared = resolved_env
                .provenance
                .iter()
                .filter(|(_, source)| source.as_str() != av::resolve::PROCESS_SOURCE)
                .map(|(key, source)| crate::session::EnvDeclared {
                    key: key.clone(),
                    source: source.clone(),
                })
                .collect();
            let mut writer = writer
                .lock()
                .map_err(|e| format!("无法锁定会话写入器: {e}"))?;
            writer
                .append_env(declared)
                .map_err(|e| format!("无法写入环境记账: {e}"))?;
        }
        // provider key 与工具子进程消费同一份 resolved env
        let (model, api_key) = resolve_model(
            &def,
            current_session_model.as_ref(),
            Some(&resolved_env.vars),
        )?;

        let mut registry = ToolRegistry::for_context(&tool_context);
        if tool_context.permissions.tool_enabled("create_agent") {
            registry.push(Arc::new(CreateAgentTool::new(model.clone())));
        }
        if tool_context.permissions.tool_enabled("run_agent") {
            registry.push(Arc::new(RunAgentTool::new(
                def.name.clone(),
                Arc::new(RuntimeAgentRunner),
            )));
        }
        if tool_context.permissions.tool_enabled("read_agent") {
            registry.push(Arc::new(ReadAgentTool));
        }
        let registry = Arc::new(registry);
        let wire_tools = registry.wire_tools();
        // api_key 随 config 被移走；压缩摘要调用还要用一份
        let compaction_api_key = api_key.clone();
        abort.reset();
        let running_guard = RunningGuard::new(running.clone());
        let run_token = running_guard.token;
        let run_id = running.current_run_id();
        // 可变：压缩换会话后要改成新 id，随后的 CompactionEnd / AgentEnd 才能
        // 被前端按新身份收下（见 SessionSwitched 事件）
        let mut session_id = writer_session_id(&writer)?;
        // 压缩时的分叉 / 归档需要会话目录与设置（都在同步段取好，move 进任务）
        let sessions_dir = def.sessions_dir();
        let runtime_settings = load_settings();
        let compaction_settings = runtime_settings.compaction;
        let retry_policy = runtime_settings.retry.policy();
        let system_prompt = agents::build_system_prompt_with_tools(&def, &wire_tools)?;

        // 交互审批通道：桌面 / Web 的交互运行才接入；子 Agent 运行不注入，
        // 白名单未命中一律拒绝（fail-closed）。
        tool_context.approver = Some(Arc::new(InteractiveApprover::new(
            self.approval.clone(),
            event_sink.clone(),
            def.name.clone(),
            session_id.clone(),
            run_id,
            abort.clone(),
        )));

        // 压缩预算集中一处（窗口 × Agent 的阈值百分比）：投影式与替换式共用
        let budget = crate::compaction::Budget::from_window(
            model.context_window,
            def.compact_threshold_percent(),
        );
        let config = AgentLoopConfig {
            model: model.clone(),
            provider: provider_for(model.api),
            tools: registry,
            tool_context,
            options: StreamOptions {
                api_key: Some(api_key),
                temperature: None,
                max_tokens: Some(model.max_tokens),
                timeout_secs: 300,
                // 会话标识：需要它的供应商（如 OpenCode Go）据此做路由与缓存
                session_id: Some(session_id.clone()),
            },
            retry: retry_policy,
            tool_execution: ToolExecutionMode::Parallel,
            steering: steering.clone(),
            follow_up: MessageQueue::new(),
            before_tool_call: None,
            after_tool_call: None,
            // 请求前的投影式压缩流水线（pi 的 transformContext 位置）：先清旧
            // 工具输出，仍超预算再硬裁剪 —— 便宜的先用尽，摘要留到 turn 边界。
            // 投影是确定性的、不落盘，回放时重算即可，所以这里改历史不会造成
            // 「live 与重开不一致」。
            transform_context: (model.context_window > 0).then(|| projection_transform(budget)),
        };

        // 首条消息先落盘（崩溃也会留下用户输入）；写入失败必须阻止启动本轮。
        {
            let mut writer = writer
                .lock()
                .map_err(|e| format!("无法锁定会话写入器: {e}"))?;
            writer
                .append_message(&user_message)
                .map_err(|e| format!("无法写入用户消息: {e}"))?;
        }

        let completion_sink = event_sink.clone();
        // make_emitter 会按值取走 writer；压缩阶段还要用它，先克隆
        let compaction_writer = writer.clone();
        // 摘要调用的用量要进会话账本；make_emitter 会按值取走 stats，先克隆
        let ledger_stats = stats.clone();
        let emitter = make_emitter(
            event_sink,
            writer,
            stats,
            def.name.clone(),
            session_id.clone(),
            run_id,
        );
        // 压缩分叉换 id 后把 map 条目搬到新键
        let rekey = make_rekey(self.sessions.clone());
        // 显式 spawn 到注入的运行时上：调用线程可能根本不在运行时里
        // （桌面壳的 Tauri command 跑在 GTK 主线程），此时 `tokio::spawn` 会 panic。
        self.spawn_run(async move {
            let _running_guard = running_guard;
            messages.lock().await.push(user_message);

            // 收割循环：循环结束后迟到的 steering（用户在收尾流式期间插话）
            // 不会丢 —— 当作下一轮 prompt 自动续跑，直到队列排空或已中止。
            let mut prompts = queued;
            let mut completion_messages: Vec<Message> = Vec::new();
            loop {
                let context = AgentContext {
                    system_prompt: system_prompt.clone(),
                    messages: messages.lock().await.clone(),
                };
                let new_messages = run_agent_loop(
                    std::mem::take(&mut prompts), // 首轮为空：用户消息已并入 context
                    context,
                    config.clone(),
                    emitter.clone(),
                    abort.clone(),
                )
                .await;

                completion_messages.extend(new_messages.clone());
                messages.lock().await.extend(new_messages);

                let leftover = steering.drain();
                if leftover.is_empty() || abort.is_aborted() {
                    break;
                }
                prompts = leftover;
            }

            // turn 边界的摘要压缩（自动）：历史达到触发线时把旧轮次压成摘要并
            // 持久化。失败不阻塞会话 —— 请求前的投影流水线（清旧工具输出 + 硬裁）
            // 仍在；压缩完成前会话保持 running（见下方 finish），避免压缩期间
            // 新请求读到未压缩历史。
            let outcome = run_compaction(CompactionContext {
                agent_name: &def.name,
                session_id: &mut session_id,
                run_id,
                model: &model,
                api_key: &compaction_api_key,
                budget,
                messages: &messages,
                writer: &compaction_writer,
                stats: &ledger_stats,
                abort: abort.clone(),
                sessions_dir: sessions_dir.as_deref(),
                settings: compaction_settings,
                retry: retry_policy,
                sink: &completion_sink,
                trigger: CompactionTrigger::Auto,
                rekey: Some(rekey),
            })
            .await;
            if let Err(error) = outcome {
                eprintln!("pipi: 上下文压缩失败（本轮跳过）: {error}");
            }

            running.finish(run_token);

            completion_sink(RuntimeEvent::AgentEvent(AgentEventEnvelope {
                agent_name: def.name,
                session_id,
                run_id,
                event: AgentEvent::AgentEnd {
                    messages: completion_messages,
                },
            }));
        });

        if let Some((key, session)) = created {
            sessions.insert(key, session);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::pending;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn temporary_compaction_updates_memory_ledger_without_forking() {
        let mut ledger = SessionWriter::temporary();
        let keep_from = ledger
            .append_message(&Message::user_text("保留这条"))
            .unwrap();
        ledger
            .append_message(&Message::assistant_text("回答", "mock"))
            .unwrap();
        let source_tip = ledger.tip_id().unwrap().to_string();
        let writer = Arc::new(Mutex::new(ledger));
        let record = crate::session::CompactionRecord {
            summary: "内存摘要",
            strategy: "summary",
            keep_from_entry: &keep_from,
            source_tip: &source_tip,
            usage: None,
        };

        let switched = persist_compaction(
            &writer,
            None,
            &record,
            crate::settings::CompactionSettings {
                fork_before_compact: true,
                archive_original: true,
            },
        )
        .expect("临时账本压缩");

        assert!(switched.is_none(), "临时压缩不能切换或分叉会话");
        let ledger = writer.lock().unwrap();
        assert!(ledger.is_temporary());
        assert!(ledger.persistent_path().is_none());
        assert_ne!(ledger.tip_id(), Some(source_tip.as_str()));
    }

    #[test]
    fn overlapping_generations_keep_new_run_visible() {
        let state = Arc::new(RunState::new());
        let old_guard = RunningGuard::new(state.clone());
        let new_guard = RunningGuard::new(state.clone());

        drop(old_guard);
        assert!(state.is_running());

        drop(new_guard);
        assert!(!state.is_running());
    }

    #[test]
    fn tokio_abort_drops_running_guard() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(RunState::new());
        let task_state = state.clone();

        runtime.block_on(async move {
            let handle = tokio::spawn(async move {
                let _guard = RunningGuard::new(task_state);
                pending::<()>().await;
            });
            tokio::task::yield_now().await;
            assert!(state.is_running());

            handle.abort();
            assert!(handle.await.is_err());
            assert!(!state.is_running());
        });
    }

    #[test]
    fn tokio_panic_drops_running_guard() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(RunState::new());
        let task_state = state.clone();

        runtime.block_on(async move {
            let handle = tokio::spawn(async move {
                let _guard = RunningGuard::new(task_state);
                panic!("test panic");
            });

            assert!(handle.await.is_err());
            assert!(!state.is_running());
        });
    }

    #[test]
    fn running_state_is_visible_across_threads() {
        let state = Arc::new(RunState::new());
        let started = Arc::new(Barrier::new(2));
        let observed = Arc::new(Barrier::new(2));
        let finished = Arc::new(Barrier::new(2));
        let observer_state = state.clone();
        let observer_started = started.clone();
        let observer_observed = observed.clone();
        let observer_finished = finished.clone();

        let observer = thread::spawn(move || {
            observer_started.wait();
            assert!(observer_state.is_running());
            observer_observed.wait();
            observer_finished.wait();
            assert!(!observer_state.is_running());
        });

        let guard = RunningGuard::new(state.clone());
        started.wait();
        observed.wait();
        drop(guard);
        finished.wait();
        observer.join().unwrap();
    }

    /// 回归：桌面壳的 Tauri command 跑在 GTK 主线程上，那里没有 reactor。
    /// 核心必须把运行 spawn 到注入的运行时上，而不是依赖调用线程的上下文 ——
    /// 修复前这里会 panic（there is no reactor running），且 panic 发生在主线程、
    /// 直接中止整个进程。
    #[test]
    fn spawn_run_works_without_ambient_reactor() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let state = RuntimeState::new(runtime.handle().clone());
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = done.clone();

        // 关键：不套 runtime.block_on —— 模拟「调用线程不在运行时里」
        state.spawn_run(async move {
            flag.store(true, Ordering::SeqCst);
        });
        assert!(!done.load(Ordering::SeqCst), "任务不该在调用线程上同步执行");

        runtime.block_on(async {
            tokio::task::yield_now().await;
        });
        assert!(
            done.load(Ordering::SeqCst),
            "任务必须落在注入的运行时上执行"
        );
    }

    #[test]
    fn resolve_model_prefers_session_model_over_agent_default() {
        // 该测试依赖「HOME 下没有 settings.json → 默认 provider 表」，
        // 且 HOME 是进程级状态：持全局锁 + 指向空目录，保证确定性。
        let _guard = crate::HOME_LOCK.lock().unwrap();
        let previous_home = std::env::var("HOME").unwrap_or_default();
        let empty_home = std::env::temp_dir().join(format!(
            "pipi-resolve-model-home-{}-{}",
            std::process::id(),
            crate::session::new_id()
        ));
        std::fs::create_dir_all(empty_home.join(".pipi")).unwrap();
        std::env::set_var("HOME", &empty_home);

        let default_model = Model {
            id: "gpt-4o".into(),
            name: "GPT-4o".into(),
            api: crate::types::Api::OpenAICompletions,
            base_url: "https://api.openai.com/v1".into(),
            max_tokens: 4096,
            context_window: 128000,
        };
        let session_model = Model {
            id: "claude-sonnet-4-5".into(),
            name: "Claude Sonnet".into(),
            api: crate::types::Api::AnthropicMessages,
            base_url: "https://api.anthropic.com".into(),
            max_tokens: 8192,
            context_window: 200000,
        };

        let def = AgentDefinition {
            name: "test-agent".into(),
            description: String::new(),
            model: "gpt-4o".into(),
            provider: Some(default_model.clone()),
            workspace: None,
            permissions: Default::default(),
            mcp_servers: Vec::new(),
            compact_threshold_percent: 75,
        };

        // 未配置 key 时的校验
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");

        // 回退默认模型
        let err = super::resolve_model(&def, None, None).unwrap_err();
        assert!(err.contains("未配置 API 密钥"));

        // 优先会话覆盖模型
        let err = super::resolve_model(&def, Some(&session_model), None).unwrap_err();
        assert!(err.contains("api.anthropic.com"));

        // 配上 key 后成功解析
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");
        let (resolved, key) = super::resolve_model(&def, Some(&session_model), None).unwrap();
        assert_eq!(resolved.id, "claude-sonnet-4-5");
        assert_eq!(key, "test-key");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::set_var("HOME", &previous_home);
        let _ = std::fs::remove_dir_all(&empty_home);
    }
}

#[cfg(test)]
mod child_timeout_tests {
    use super::*;

    /// 子 Agent 挂死（端点接受连接但不回包）时，运行时限必须终止本轮并把
    /// 超时终态落盘 —— read_agent 读到 Failed 而不是永远的 pending。
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn child_run_times_out_and_persists_terminal_state() {
        let _guard = crate::HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            // 故意持有每条连接：请求悬挂，直到 tokio 超时触发
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                held.push(stream);
            }
        });

        let home = std::env::temp_dir().join(format!(
            "pipi-child-timeout-{}-{}",
            std::process::id(),
            crate::session::new_id()
        ));
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::env::set_var("HOME", &home);

        let base_url = format!("http://{addr}/v1");
        crate::settings::save_settings(&crate::settings::Settings {
            theme: crate::settings::Theme::Dark,
            providers: vec![crate::settings::ProviderConfig {
                id: "silent".into(),
                name: "Silent".into(),
                api: crate::types::Api::OpenAICompletions,
                base_url: base_url.clone(),
                env_key: None,
                api_key: Some("test-key".into()),
            }],
            default_provider_id: None,
            compaction: Default::default(),
            retry: Default::default(),
        })
        .unwrap();

        agents::create_agent(
            "timeout-worker",
            "A worker whose endpoint never responds",
            Some(workspace.to_str().unwrap()),
            Some(crate::permissions::PermissionsConfig {
                tools: vec!["read".into()],
                bash: Default::default(),
                sandbox: crate::permissions::SandboxMode::WorkspaceWrite,
            }),
            None,
            Some(crate::types::Model {
                id: "silent-model".into(),
                name: "Silent Model".into(),
                api: crate::types::Api::OpenAICompletions,
                base_url,
                max_tokens: 64,
                context_window: 4096,
            }),
        )
        .unwrap();

        let result = run_agent_once_inner(
            "timeout-worker",
            "hang",
            AbortSignal::new(),
            std::time::Duration::from_millis(300),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.status, AgentRunStatus::Failed);
        assert!(
            result.response.contains("超时"),
            "应报超时: {}",
            result.response
        );

        // 终态已落盘：read_agent 读到 Failed 而不是 pending
        let reread =
            crate::tools::agent::read_agent_output("timeout-worker", Some(&result.session_id))
                .unwrap();
        assert_eq!(reread.status, AgentRunStatus::Failed);

        let _ = std::fs::remove_dir_all(home);
    }
}
