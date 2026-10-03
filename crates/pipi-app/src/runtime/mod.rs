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

use crate::approval::{
    ApprovalDecision, ApprovalGate, ApprovalRequestEnvelope, InteractiveApprover,
};
use crate::background::BackgroundTaskManager;
use pipi_core::agent_loop::Emitter as LoopEmitter;
use pipi_core::agent_loop::{
    AgentContext, AgentEvent, AgentLoopConfig, MessageQueue, ToolExecutionMode, run_agent_loop,
};
use pipi_core::agents::{self, AgentDefinition};
pub use pipi_core::session::SessionSummary;
use pipi_core::session::{EntryKind, SessionWriter, list_session_summaries, load_session};
use pipi_core::settings::load_settings;
use pipi_core::stats::SessionStatsTracker;
use pipi_core::tools::agent::{
    AgentRunResult, AgentRunStatus, AgentRunner, ChildProgressTx, CreateAgentTool, ReadAgentTool,
    RunAgentTool, result_from_messages,
};
use pipi_core::tools::background::{
    BackgroundAgentSessionSink, BackgroundTaskOwner, BackgroundTaskService,
    ManageBackgroundTaskTool, QueryBackgroundTasksTool, SubmitBackgroundTaskTool,
};
use pipi_protocol::{AbortSignal, Message, Model, StopReason, StreamOptions};
use pipi_provider::provider_for;
use pipi_tools::ToolRegistry;

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
    pub stats: pipi_core::stats::SessionStats,
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

/// 一个独立 child session 已经创建并写入磁盘，宿主应刷新对应 Agent 的会话列表。
///
/// 这条事件不绑定当前打开的父会话：后台 `run_agent` 可能在父轮结束后才创建
/// child，前端也可能已经切到了别的 Agent。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionChangedEnvelope {
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
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
    #[serde(rename = "session-changed")]
    SessionChanged(SessionChangedEnvelope),
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
    background: BackgroundTaskManager,
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

mod compaction;
mod events;
mod execution;
mod slots;
#[cfg(test)]
mod tests;

#[cfg(test)]
use compaction::compaction_stream_options;
use compaction::{CompactionContext, CompactionTrigger, make_rekey, run_compaction};
use events::{make_emitter, make_session_change_sink};
#[cfg(test)]
use execution::append_child_timeout_terminal;
#[cfg(test)]
use execution::run_agent_once_inner;
pub(crate) use execution::run_agent_once_inner_with_sink;
pub use execution::{CHILD_RUN_TIMEOUT_SECS, run_agent_once};
use execution::{resolve_model, settle_unanswered_tail, writer_session_id};
pub use slots::list_sessions;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub agent_name: String,
    pub session_id: String,
    /// true = 设置工作台的内存测试会话，不对应 sessions/*.jsonl。
    pub temporary: bool,
    pub running: bool,
    /// 该 session 当前仍在运行的托管后台任务数；它们会阻止会话槽被释放。
    pub background_tasks: usize,
    pub run_id: usize,
    pub model: Option<Model>,
    pub is_custom_model: bool,
}
