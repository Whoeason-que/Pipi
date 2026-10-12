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

use crate::approval::{ApprovalDecision, ApprovalGate, InteractiveApprover};
use crate::background::BackgroundTaskManager;
use crate::events::{
    AgentEventEnvelope, EventEmitter, RuntimeEvent, SessionChangedEnvelope, SessionErrorEnvelope,
    SessionSwitchedEnvelope, StatsEventEnvelope,
};
use crate::settings::load_settings;
use crate::tools::agent::{
    AgentRunResult, AgentRunStatus, AgentRunner, ChildProgressTx, CreateAgentTool, ReadAgentTool,
    RunAgentTool, result_from_messages,
};
use crate::tools::background::{
    BackgroundAgentSessionSink, BackgroundTaskOwner, BackgroundTaskService,
    ManageBackgroundTaskTool, QueryBackgroundTasksTool, SubmitBackgroundTaskTool,
};
use pipi_agent::agent_loop::Emitter as LoopEmitter;
use pipi_agent::agent_loop::{
    AgentContext, AgentEvent, AgentLoopConfig, MessageQueue, ToolExecutionMode, run_agent_loop,
};
use pipi_agent::stats::SessionStatsTracker;
use pipi_protocol::session::{
    EntryKind, SessionLog, SessionRepo, SessionSummary, active_model_from_entries, active_path,
    rebuild_messages,
};
use pipi_protocol::{AbortSignal, Message, Model, StopReason, StreamOptions};
use pipi_provider::provider_for;
use pipi_session::{JsonlSessionRepo, SessionWriter};
use pipi_venv::agents::{self, AgentDefinition};

static NEXT_RUN_ID: AtomicUsize = AtomicUsize::new(1);

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

/// 共享的会话账本句柄。
///
/// 运行时只依赖 [`SessionLog`] 接口；具体载体由 [`store::session_repo`] 注入
/// （今天只有 JSONL）。`Box` 是为了让压缩分叉能就地换掉整个账本
/// （`*guard = forked`）—— `dyn` 本身是不可变的定长占位，换不了。
type SharedLog = Arc<Mutex<Box<dyn SessionLog>>>;

struct Session {
    agent: AgentDefinition,
    messages: Arc<tokio::sync::Mutex<Vec<Message>>>,
    writer: SharedLog,
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
/// 字符集规则属于存储契约，实现在 `pipi_protocol::session`。
pub fn validate_session_id(session_id: &str) -> Result<(), String> {
    pipi_protocol::session::validate_session_id(session_id)
}

mod compaction;
mod events;
mod execution;
mod slots;
mod store;

use compaction::{CompactionContext, CompactionTrigger, make_rekey, run_compaction};
use events::{make_emitter, make_session_change_sink};
pub(crate) use execution::RuntimeChildRunner;
pub use execution::{CHILD_RUN_TIMEOUT_SECS, run_agent_once};
use execution::{ResolvedModel, resolve_model, settle_unanswered_tail, writer_session_id};
pub use slots::list_sessions;
use store::session_repo;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::pending;
    use std::sync::{Arc, Barrier};
    use std::thread;

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
}
