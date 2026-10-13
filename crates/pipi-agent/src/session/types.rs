//! 宿主注入的运行素材与宿主无关的观察接口。
use super::*;

/// 具体载体由宿主构造；临时会话只注入内存 log。
pub struct SessionStorage {
    pub log: Box<dyn SessionLog>,
    pub repo: Option<Arc<dyn SessionRepo>>,
}

#[derive(Debug, Clone, Copy)]
pub enum AbortPolicy {
    Reset,
    Preserve,
}

#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub session_id: String,
    pub temporary: bool,
    pub running: bool,
    pub run_id: usize,
    /// 绑定声明，不能被本轮运行期解析出的协议覆盖。
    pub selected_model: Option<Model>,
}

#[derive(Debug, Clone)]
pub struct SessionEvent {
    pub session_id: String,
    pub run_id: usize,
    pub kind: SessionEventKind,
}

#[derive(Debug, Clone)]
pub enum SessionEventKind {
    Agent(AgentEvent),
    Stats(SessionStats),
    Error(String),
    Switched {
        to_session_id: String,
        archived: bool,
    },
}

/// 新账本已安装、内部锁已释放；宿主先搬索引，再发换号事件。
pub type SessionSwitchHook = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// 钩子只活在本轮任务中，不能存入长期会话状态形成宿主索引引用环。
#[derive(Clone)]
pub struct SessionHooks {
    pub emit: Arc<dyn Fn(SessionEvent) + Send + Sync>,
    pub rekey: Option<SessionSwitchHook>,
}

impl Default for SessionHooks {
    fn default() -> Self {
        Self {
            emit: Arc::new(|_| {}),
            rekey: None,
        }
    }
}

impl SessionHooks {
    pub(super) fn agent(&self, session_id: &str, run_id: usize, event: AgentEvent) {
        (self.emit)(SessionEvent {
            session_id: session_id.into(),
            run_id,
            kind: SessionEventKind::Agent(event),
        });
    }

    pub(super) fn error(&self, session_id: &str, run_id: usize, message: String) {
        (self.emit)(SessionEvent {
            session_id: session_id.into(),
            run_id,
            kind: SessionEventKind::Error(message),
        });
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CompactionPolicy {
    pub fork_before_compact: bool,
    pub archive_original: bool,
}

/// 超时文案由宿主提供，内核不需要认识子 Agent 等产品概念。
pub struct RunTimeout {
    pub duration: std::time::Duration,
    pub message: String,
}

pub struct RunRequest {
    pub prompt: String,
    pub system_prompt: String,
    pub model: Model,
    pub provider: Arc<dyn Provider>,
    pub tools: Arc<ToolRegistry>,
    pub tool_context: ToolContext,
    pub options: StreamOptions,
    pub retry: RetryPolicy,
    pub tool_execution: ToolExecutionMode,
    pub budget: Option<Budget>,
    pub compaction: Option<CompactionPolicy>,
    /// 有上限的一次性运行：超时写错误终态，中止时补齐尚未生成的 assistant 终态。
    pub timeout: Option<RunTimeout>,
}

pub struct CompactionRequest {
    pub model: Model,
    pub provider: Arc<dyn Provider>,
    pub options: StreamOptions,
    pub retry: RetryPolicy,
    pub budget: Budget,
    pub policy: CompactionPolicy,
}

#[derive(Debug)]
pub struct RunOutcome {
    pub session_id: String,
    pub messages: Vec<Message>,
}
