//! L5 单会话运行内核：宿主注入能力与账本，内核拥有运行、恢复、记账和压缩。
//! 不读取 Agent 定义、设置或进程环境；不构造具体 provider、工具或会话载体。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::agent_loop::{
    AgentContext, AgentEvent, AgentLoopConfig, Emitter, MessageQueue, ToolExecutionMode,
    run_agent_loop,
};
use crate::stats::{SessionStats, SessionStatsTracker};
use pipi_context::compaction::Budget;
use pipi_context::retry::RetryPolicy;
use pipi_protocol::provider::Provider;
use pipi_protocol::session::{
    CompactionRecord, EntryKind, EnvDeclared, SessionEntry, SessionLog, SessionRepo,
    active_model_from_entries, active_path, rebuild_messages,
};
use pipi_protocol::tools::{ToolContext, ToolRegistry};
use pipi_protocol::{AbortSignal, Message, Model, StopReason, StreamOptions};

mod compaction;
mod events;
mod ledger;
mod run;
mod state;
mod types;

pub use run::{PreparedRun, RunReservation};
pub use state::AgentSession;
pub use types::{
    AbortPolicy, CompactionPolicy, CompactionRequest, RunOutcome, RunRequest, RunTimeout,
    SessionEvent, SessionEventKind, SessionHooks, SessionSnapshot, SessionStorage,
    SessionSwitchHook,
};

use state::{RunningGuard, SessionHandle, SharedLog};
