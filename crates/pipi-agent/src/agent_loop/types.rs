//! 循环配置与事件接口。
use super::*;

/// 对外事件。与 pi 的 AgentEvent 同名同层；`Serialize` 后可直接发给前端。
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        messages: Vec<Message>,
    },
    TurnStart,
    TurnEnd {
        message: Box<Message>,
        tool_results: Vec<Message>,
    },
    MessageStart {
        message: Message,
    },
    MessageUpdate {
        message: Message,
    },
    MessageEnd {
        message: Message,
    },
    /// 可重试错误触发了重发：`attempt` 是第几次尝试（从 1 起），
    /// `delay_ms` 是即将等待的退避时长，`cause` 触发原因。
    /// 前端据此丢弃本条已渲染的半截助手消息（只可能是工具调用块，不会是正文）。
    RetryStart {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        cause: String,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: Value,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        partial: ToolOutput,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: ToolOutput,
        is_error: bool,
    },
    /// 摘要式上下文压缩开始（runtime 在 turn 边界触发）。
    CompactionStart,
    /// 压缩完成；`summary` 为摘要正文，`replaced` 为被替换的消息条数，
    /// `strategy` 为产出它的策略名，`tokens_before/after` 是整段历史的
    /// token 估算（UI 用它显示省了多少）。
    CompactionEnd {
        summary: String,
        replaced: u64,
        strategy: String,
        tokens_before: u64,
        tokens_after: u64,
    },
}

pub type Emitter = Arc<dyn Fn(AgentEvent) + Send + Sync>;

/// steering / follow-up 消息队列。
#[derive(Clone, Default)]
pub struct MessageQueue(Arc<Mutex<Vec<Message>>>);

impl MessageQueue {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&self, message: Message) {
        self.0.lock().unwrap().push(message);
    }
    pub fn drain(&self) -> Vec<Message> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    Sequential,
    Parallel,
}

/// `beforeToolCall` 钩子的返回值。block 为 true 时拒绝执行该工具调用。
#[derive(Debug, Clone)]
pub struct BeforeToolCallOutcome {
    pub block: bool,
    pub reason: Option<String>,
}

pub type BeforeToolCallHook =
    Arc<dyn Fn(&ContentBlock, &Value) -> Option<BeforeToolCallOutcome> + Send + Sync>;

/// 上下文变换（pi 的 transformContext 钩子）：每次 LLM 调用前对消息历史
/// 做变换。运行期恒等（`None`）；历史裁剪由压缩管线的投影承担。
pub type TransformContextHook = Arc<dyn Fn(Vec<Message>) -> Vec<Message> + Send + Sync>;

/// Clone 用于「一次运行结束收割迟到 steering 后再起一轮」的续跑场景。
#[derive(Clone)]
pub struct AgentLoopConfig {
    pub model: Model,
    pub provider: Arc<dyn Provider>,
    pub tools: Arc<ToolRegistry>,
    pub tool_context: ToolContext,
    pub options: StreamOptions,
    /// 失败重发策略（全局设置 `retry` 段；分类见 [`pipi_context::retry`]）。
    pub retry: pipi_context::retry::RetryPolicy,
    pub tool_execution: ToolExecutionMode,
    pub steering: MessageQueue,
    pub follow_up: MessageQueue,
    pub before_tool_call: Option<BeforeToolCallHook>,
    pub transform_context: Option<TransformContextHook>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentContext {
    pub system_prompt: String,
    pub messages: Vec<Message>,
}
