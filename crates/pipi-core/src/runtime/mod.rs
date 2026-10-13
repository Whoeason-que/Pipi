//! 宿主负责产品装配、多会话索引与调度；单会话内核在 pipi-agent。
use crate::approval::{ApprovalDecision, ApprovalGate, InteractiveApprover};
use crate::background::BackgroundTaskManager;
use crate::events::{
    AgentEventEnvelope, EventEmitter, RuntimeEvent, SessionChangedEnvelope, SessionErrorEnvelope,
    SessionSwitchedEnvelope, StatsEventEnvelope,
};
use crate::settings::load_settings;
use crate::tools::agent::{
    AgentRunResult, AgentRunner, ChildProgressTx, CreateAgentTool, ReadAgentTool, RunAgentTool,
    result_from_messages,
};
use crate::tools::background::{
    BackgroundAgentSessionSink, BackgroundTaskOwner, BackgroundTaskService,
    ManageBackgroundTaskTool, QueryBackgroundTasksTool, SubmitBackgroundTaskTool,
};
use pipi_agent::agent_loop::{AgentEvent, ToolExecutionMode};
use pipi_agent::session::{
    AbortPolicy, AgentSession, CompactionPolicy, CompactionRequest, RunRequest, RunTimeout,
    SessionEvent, SessionEventKind, SessionHooks, SessionStorage,
};
use pipi_protocol::session::{SessionLog, SessionRepo, SessionSummary, validate_session_id};
use pipi_protocol::{AbortSignal, Message, Model, StreamOptions};
use pipi_provider::provider_for;
use pipi_session::MemoryLog;
use pipi_venv::agents::{self, AgentDefinition};
use serde::Serialize;
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct HostSession {
    agent: AgentDefinition,
    engine: AgentSession,
    temporary: bool,
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
    sessions: Arc<Mutex<HashMap<SessionKey, HostSession>>>,
    approval: Arc<ApprovalGate>,
    background: BackgroundTaskManager,
}

mod child;
mod compaction;
mod events;
mod execution;
mod lifecycle;
mod model;
mod slots;
mod store;
use child::RuntimeAgentRunner;
pub(crate) use child::RuntimeChildRunner;
pub use child::run_agent_once;
use events::{make_rekey, make_session_change_sink, make_session_hooks};
use model::{ResolvedModel, append_declared_env, resolve_model, run_stream_options};
pub use slots::{SessionSnapshot, list_sessions};
pub(crate) use store::agent_repo;
use store::session_repo;
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub agent_name: String,
    pub session_id: String,
    /// true = 设置工作台的内存测试会话，不对应任何落盘会话。
    pub temporary: bool,
    pub running: bool,
    /// 该 session 当前仍在运行的托管后台任务数；它们会阻止会话槽被释放。
    pub background_tasks: usize,
    pub run_id: usize,
    pub model: Option<Model>,
    pub is_custom_model: bool,
}
