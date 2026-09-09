//! Agent 主循环。移植自 `packages/agent/src/agent-loop.ts`。
//!
//! 保留的核心语义：
//! - 事件流（agent / turn / message / tool 四层生命周期）
//! - steering（运行中插话）与 follow-up（结束后追加）两条消息队列
//! - 工具批次执行：顺序 / 并行；全部结果 terminate 才提前终止
//! - stopReason 为 length 时拒绝执行所有工具调用（参数可能被截断）
//! - before/after 工具钩子（Pipi 的命令权限在 bash 工具内部检查，钩子供扩展用）
//!
//! 有意简化（与上游差异，见 lib.rs）：convertToLlm 为恒等（暂无自定义消息
//! 类型）；未实现 transformContext / prepareNextTurn / shouldStopAfterTurn；
//! 并行批次按助手消息中的顺序（而非完成顺序）回报结果。

use std::sync::{Arc, Mutex};

use futures_util::future::join_all;
use serde::Serialize;
use serde_json::{json, Value};

use crate::provider::Provider;
use crate::tools::{validate_args, AgentTool, ToolContext, ToolOutput, ToolRegistry};
use crate::types::{
    AbortSignal, ContentBlock, Context, Message, Model, StopReason, StreamEvent, StreamOptions,
    ToolResultContent,
};

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
    pub terminate: bool,
}

pub type BeforeToolCallHook =
    Arc<dyn Fn(&ContentBlock, &Value) -> Option<BeforeToolCallOutcome> + Send + Sync>;
pub type AfterToolCallHook = Arc<dyn Fn(&ContentBlock, &mut ToolOutput, &mut bool) + Send + Sync>;

/// 上下文变换（pi 的 transformContext 钩子）：每次 LLM 调用前对消息历史
/// 做变换（裁剪、注入等）。默认恒等；见 [`crate::context::prune_transform`]。
pub type TransformContextHook = Arc<dyn Fn(Vec<Message>) -> Vec<Message> + Send + Sync>;

pub struct AgentLoopConfig {
    pub model: Model,
    pub provider: Arc<dyn Provider>,
    pub tools: Arc<ToolRegistry>,
    pub tool_context: ToolContext,
    pub options: StreamOptions,
    pub tool_execution: ToolExecutionMode,
    pub steering: MessageQueue,
    pub follow_up: MessageQueue,
    pub before_tool_call: Option<BeforeToolCallHook>,
    pub after_tool_call: Option<AfterToolCallHook>,
    pub transform_context: Option<TransformContextHook>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentContext {
    pub system_prompt: String,
    pub messages: Vec<Message>,
}

struct Batch {
    messages: Vec<Message>,
    terminate: bool,
}

/// 主循环入口（对应 pi 的 runAgentLoop）。返回本次运行新增的全部消息。
pub async fn run_agent_loop(
    prompts: Vec<Message>,
    mut context: AgentContext,
    config: AgentLoopConfig,
    emit: Emitter,
    abort: AbortSignal,
) -> Vec<Message> {
    let mut new_messages: Vec<Message> = prompts.clone();
    for prompt in &prompts {
        context.messages.push(prompt.clone());
    }

    emit(AgentEvent::AgentStart);
    emit(AgentEvent::TurnStart);
    for prompt in prompts {
        emit(AgentEvent::MessageStart {
            message: prompt.clone(),
        });
        emit(AgentEvent::MessageEnd { message: prompt });
    }

    let mut pending: Vec<Message> = config.steering.drain();
    'outer: loop {
        let mut has_more_tool_calls = true;

        while has_more_tool_calls || !pending.is_empty() {
            // 注入 steering / follow-up 消息
            for message in pending.drain(..) {
                emit(AgentEvent::MessageStart {
                    message: message.clone(),
                });
                emit(AgentEvent::MessageEnd {
                    message: message.clone(),
                });
                context.messages.push(message.clone());
                new_messages.push(message);
            }

            // 流式获取助手回复
            let message = stream_assistant_response(&context, &config, &emit, &abort).await;
            let reason = stop_reason_of(&message);
            let aborted_or_errored =
                matches!(reason, Some(StopReason::Error) | Some(StopReason::Aborted));
            new_messages.push(message.clone());
            context.messages.push(message.clone());

            if aborted_or_errored {
                emit(AgentEvent::TurnEnd {
                    message: Box::new(message),
                    tool_results: Vec::new(),
                });
                break 'outer;
            }

            let tool_calls: Vec<ContentBlock> = message.tool_calls().into_iter().cloned().collect();

            let mut tool_results: Vec<Message> = Vec::new();
            has_more_tool_calls = false;
            if !tool_calls.is_empty() {
                // stopReason 为 length 意味着输出被 token 上限截断，参数可能不完整，
                // 全部拒绝执行（照搬 pi 的 failToolCallsFromTruncatedMessage）。
                let batch = if reason == Some(StopReason::Length) {
                    fail_truncated_tool_calls(&tool_calls, &emit).await
                } else {
                    execute_tool_calls(&tool_calls, &config, &emit, &abort).await
                };
                has_more_tool_calls = !batch.terminate && !abort.is_aborted();
                tool_results = batch.messages;
                for result in &tool_results {
                    context.messages.push(result.clone());
                    new_messages.push(result.clone());
                }
            }

            emit(AgentEvent::TurnEnd {
                message: Box::new(message),
                tool_results,
            });

            pending = config.steering.drain();
            if abort.is_aborted() {
                break 'outer;
            }
        }

        // 本来要停了 —— 检查 follow-up 队列
        let follow_up = config.follow_up.drain();
        if !follow_up.is_empty() {
            pending = follow_up;
            continue;
        }
        break;
    }

    emit(AgentEvent::AgentEnd {
        messages: new_messages.clone(),
    });
    new_messages
}

fn stop_reason_of(message: &Message) -> Option<StopReason> {
    match message {
        Message::Assistant { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    }
}

/// 从 provider 事件流构建一条助手消息（对应 pi 的 streamAssistantResponse）。
async fn stream_assistant_response(
    context: &AgentContext,
    config: &AgentLoopConfig,
    emit: &Emitter,
    abort: &AbortSignal,
) -> Message {
    // transformContext 钩子（pi）：LLM 边界前对历史做变换（裁剪等）。
    // 会话内的 messages 不动 —— 只影响本次请求。
    let effective_messages = match &config.transform_context {
        Some(hook) => hook(context.messages.clone()),
        None => context.messages.clone(),
    };
    let wire = Context {
        system_prompt: (!context.system_prompt.is_empty()).then(|| context.system_prompt.clone()),
        messages: effective_messages,
        tools: config.tools.wire_tools(),
    };
    let mut rx = config
        .provider
        .stream(&config.model, &wire, &config.options, abort.clone())
        .await;

    // 部分消息累积：Text/Thinking 增量填充；工具调用参数以 Value::String
    // （原始 JSON 片段）暂存，ToolCallEnd 时替换为解析后的块。
    let mut blocks: Vec<ContentBlock> = Vec::new();
    let partial = |blocks: &[ContentBlock]| -> Message {
        Message::Assistant {
            content: blocks.to_vec(),
            api: config.model.api.as_str().to_string(),
            provider: "pending".into(),
            model: config.model.display_name().to_string(),
            usage: Default::default(),
            stop_reason: StopReason::Pending,
            error_message: None,
            timestamp: crate::types::now_millis(),
            duration_ms: None,
        }
    };

    loop {
        match rx.recv().await {
            None => {
                let message = Message::assistant_error(
                    "已中止",
                    config.model.display_name(),
                    StopReason::Aborted,
                );
                emit(AgentEvent::MessageEnd {
                    message: message.clone(),
                });
                return message;
            }
            Some(StreamEvent::Start) => {
                emit(AgentEvent::MessageStart {
                    message: partial(&blocks),
                });
            }
            Some(StreamEvent::TextDelta { delta, .. }) => {
                match blocks.last_mut() {
                    Some(ContentBlock::Text { text }) => text.push_str(&delta),
                    _ => blocks.push(ContentBlock::Text { text: delta }),
                }
                emit(AgentEvent::MessageUpdate {
                    message: partial(&blocks),
                });
            }
            Some(StreamEvent::ThinkingDelta { delta, .. }) => {
                match blocks.last_mut() {
                    Some(ContentBlock::Thinking { thinking, .. }) => thinking.push_str(&delta),
                    _ => blocks.push(ContentBlock::Thinking {
                        thinking: delta,
                        thinking_signature: None,
                    }),
                }
                emit(AgentEvent::MessageUpdate {
                    message: partial(&blocks),
                });
            }
            Some(StreamEvent::ToolCallStart { id, name, .. }) => {
                blocks.push(ContentBlock::ToolCall {
                    id,
                    name,
                    arguments: json!({}),
                });
                emit(AgentEvent::MessageUpdate {
                    message: partial(&blocks),
                });
            }
            Some(StreamEvent::ToolCallDelta { delta, .. }) => {
                if let Some(ContentBlock::ToolCall { arguments, .. }) = blocks.last_mut() {
                    let raw = arguments.as_str().map(str::to_string).unwrap_or_default();
                    *arguments = Value::String(format!("{raw}{delta}"));
                }
            }
            Some(StreamEvent::ToolCallEnd { call, .. }) => {
                if let Some(pos) = blocks
                    .iter()
                    .rposition(|b| matches!(b, ContentBlock::ToolCall { .. }))
                {
                    blocks[pos] = call;
                } else {
                    blocks.push(call);
                }
                emit(AgentEvent::MessageUpdate {
                    message: partial(&blocks),
                });
            }
            Some(StreamEvent::Done { message, .. }) => {
                emit(AgentEvent::MessageEnd {
                    message: *message.clone(),
                });
                return *message;
            }
            Some(StreamEvent::Error { message }) => {
                let m = Message::assistant_error(
                    message,
                    config.model.display_name(),
                    StopReason::Error,
                );
                emit(AgentEvent::MessageEnd { message: m.clone() });
                return m;
            }
        }
    }
}

struct ExecutedOutcome {
    result: ToolOutput,
    is_error: bool,
}

fn error_output(message: impl Into<String>) -> ToolOutput {
    ToolOutput {
        content: vec![ToolResultContent::Text {
            text: message.into(),
        }],
        details: None,
        terminate: false,
    }
}

/// length 截断时统一拒绝执行（照搬 pi 的文案）。
async fn fail_truncated_tool_calls(tool_calls: &[ContentBlock], emit: &Emitter) -> Batch {
    let mut messages = Vec::new();
    for call in tool_calls {
        let Some((call_id, call_name, _)) = call_parts(call) else {
            continue;
        };
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: call_id.clone(),
            tool_name: call_name.clone(),
            args: json!({}),
        });
        let result = error_output(format!(
            "Tool call \"{call_name}\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments."
        ));
        emit(AgentEvent::ToolExecutionEnd {
            tool_call_id: call_id.clone(),
            tool_name: call_name.clone(),
            result: result.clone(),
            is_error: true,
        });
        let message = tool_result_message(&call_id, &call_name, &result, true);
        emit(AgentEvent::MessageStart {
            message: message.clone(),
        });
        emit(AgentEvent::MessageEnd {
            message: message.clone(),
        });
        messages.push(message);
    }
    Batch {
        messages,
        terminate: false,
    }
}

fn call_parts(call: &ContentBlock) -> Option<(String, String, Value)> {
    match call {
        ContentBlock::ToolCall {
            id,
            name,
            arguments,
        } => Some((id.clone(), name.clone(), arguments.clone())),
        _ => None,
    }
}

fn tool_result_message(
    tool_call_id: &str,
    tool_name: &str,
    result: &ToolOutput,
    is_error: bool,
) -> Message {
    Message::ToolResult {
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        content: result.content.clone(),
        is_error,
        details: result.details.clone(),
        timestamp: crate::types::now_millis(),
    }
}

/// 执行一批工具调用（对应 pi 的 executeToolCalls）。
/// bash 这类有副作用的工具强制走顺序模式。
async fn execute_tool_calls(
    tool_calls: &[ContentBlock],
    config: &AgentLoopConfig,
    emit: &Emitter,
    abort: &AbortSignal,
) -> Batch {
    let has_sequential_tool = tool_calls.iter().any(|call| match call {
        ContentBlock::ToolCall { name, .. } => config
            .tools
            .get(name)
            .map(|t| t.requires_sequential())
            .unwrap_or(false),
        _ => false,
    });

    if config.tool_execution == ToolExecutionMode::Sequential || has_sequential_tool {
        execute_sequential(tool_calls, config, emit, abort).await
    } else {
        execute_parallel(tool_calls, config, emit, abort).await
    }
}

/// 单个调用的准备阶段：查找工具、校验参数、before 钩子。
/// Immediate 表示已确定结果（未找到 / 校验失败 / 被钩子拦截 / 已中止）。
enum Prepared {
    Ready {
        tool: Arc<dyn AgentTool>,
        call: ContentBlock,
        args: Value,
    },
    Immediate {
        result: ToolOutput,
        is_error: bool,
    },
}

fn prepare_call(call: &ContentBlock, config: &AgentLoopConfig, abort: &AbortSignal) -> Prepared {
    let Some((_, call_name, arguments)) = call_parts(call) else {
        return Prepared::Immediate {
            result: error_output("内容块不是工具调用"),
            is_error: true,
        };
    };
    let Some(tool) = config.tools.get(&call_name) else {
        return Prepared::Immediate {
            result: error_output(format!("Tool {call_name} not found")),
            is_error: true,
        };
    };
    let args = match validate_args(&tool.parameters(), &arguments) {
        Ok(()) => arguments,
        Err(e) => {
            return Prepared::Immediate {
                result: error_output(e),
                is_error: true,
            }
        }
    };
    if let Some(hook) = &config.before_tool_call {
        if let Some(outcome) = hook(call, &args) {
            if outcome.block {
                return Prepared::Immediate {
                    result: error_output(
                        outcome
                            .reason
                            .unwrap_or_else(|| "Tool execution was blocked".into()),
                    ),
                    is_error: true,
                };
            }
        }
    }
    if abort.is_aborted() {
        return Prepared::Immediate {
            result: error_output("Operation aborted"),
            is_error: true,
        };
    }
    Prepared::Ready {
        tool,
        call: call.clone(),
        args,
    }
}

async fn run_tool(
    tool: Arc<dyn AgentTool>,
    ctx: &ToolContext,
    call: &ContentBlock,
    args: Value,
    emit: &Emitter,
    abort: &AbortSignal,
) -> ExecutedOutcome {
    let Some((call_id, call_name, _)) = call_parts(call) else {
        return ExecutedOutcome {
            result: error_output("内容块不是工具调用"),
            is_error: true,
        };
    };
    if abort.is_aborted() {
        return ExecutedOutcome {
            result: error_output("Operation aborted"),
            is_error: true,
        };
    }
    let emit_for_update = emit.clone();
    let on_update = move |partial: ToolOutput| {
        emit_for_update(AgentEvent::ToolExecutionUpdate {
            tool_call_id: call_id.clone(),
            tool_name: call_name.clone(),
            partial,
        });
    };
    let outcome = match tool.execute(ctx, &args, &on_update).await {
        Ok(result) => ExecutedOutcome {
            result,
            is_error: false,
        },
        Err(e) => ExecutedOutcome {
            result: error_output(e),
            is_error: true,
        },
    };
    if abort.is_aborted() && !outcome.is_error {
        return ExecutedOutcome {
            result: error_output("Operation aborted"),
            is_error: true,
        };
    }
    outcome
}

fn finalize_outcome(
    call: &ContentBlock,
    mut outcome: ExecutedOutcome,
    config: &AgentLoopConfig,
) -> ExecutedOutcome {
    if let Some(hook) = &config.after_tool_call {
        hook(call, &mut outcome.result, &mut outcome.is_error);
    }
    outcome
}

fn emit_finalized(call: &ContentBlock, outcome: &ExecutedOutcome, emit: &Emitter) -> Message {
    let Some((call_id, call_name, _)) = call_parts(call) else {
        unreachable!("tool call always has parts here");
    };
    emit(AgentEvent::ToolExecutionEnd {
        tool_call_id: call_id.clone(),
        tool_name: call_name.clone(),
        result: outcome.result.clone(),
        is_error: outcome.is_error,
    });
    let message = tool_result_message(&call_id, &call_name, &outcome.result, outcome.is_error);
    emit(AgentEvent::MessageStart {
        message: message.clone(),
    });
    emit(AgentEvent::MessageEnd {
        message: message.clone(),
    });
    message
}

async fn execute_sequential(
    tool_calls: &[ContentBlock],
    config: &AgentLoopConfig,
    emit: &Emitter,
    abort: &AbortSignal,
) -> Batch {
    let mut messages: Vec<Message> = Vec::new();
    let mut terminates: Vec<bool> = Vec::new();

    for call in tool_calls {
        let Some((call_id, call_name, args)) = call_parts(call) else {
            continue;
        };
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: call_id.clone(),
            tool_name: call_name.clone(),
            args,
        });

        let finalized = match prepare_call(call, config, abort) {
            Prepared::Immediate { result, is_error } => ExecutedOutcome { result, is_error },
            Prepared::Ready { tool, call, args } => finalize_outcome(
                &call,
                run_tool(tool, &config.tool_context, &call, args, emit, abort).await,
                config,
            ),
        };
        terminates.push(finalized.result.terminate);
        messages.push(emit_finalized(call, &finalized, emit));

        if abort.is_aborted() {
            break;
        }
    }

    Batch {
        terminate: !messages.is_empty() && terminates.iter().all(|&t| t),
        messages,
    }
}

async fn execute_parallel(
    tool_calls: &[ContentBlock],
    config: &AgentLoopConfig,
    emit: &Emitter,
    abort: &AbortSignal,
) -> Batch {
    // 准备阶段串行（与 pi 一致）；执行阶段并发，结果按助手消息顺序回报。
    let mut prepared: Vec<(ContentBlock, Prepared)> = Vec::new();
    for call in tool_calls {
        let Some((call_id, call_name, args)) = call_parts(call) else {
            continue;
        };
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: call_id.clone(),
            tool_name: call_name.clone(),
            args,
        });
        prepared.push((call.clone(), prepare_call(call, config, abort)));
        if abort.is_aborted() {
            break;
        }
    }

    let mut futures = Vec::new();
    for (_, p) in &prepared {
        if let Prepared::Ready { tool, call, args } = p {
            let ctx = config.tool_context.clone();
            let emit2 = emit.clone();
            let abort2 = abort.clone();
            let call2 = call.clone();
            let args2 = args.clone();
            let tool2 = tool.clone();
            futures
                .push(async move { run_tool(tool2, &ctx, &call2, args2, &emit2, &abort2).await });
        }
    }
    let executed = join_all(futures).await;

    let mut exec_iter = executed.into_iter();
    let mut messages: Vec<Message> = Vec::new();
    let mut terminates: Vec<bool> = Vec::new();
    for (call, p) in &prepared {
        let outcome = match p {
            Prepared::Immediate { result, is_error } => ExecutedOutcome {
                result: result.clone(),
                is_error: *is_error,
            },
            Prepared::Ready {
                call: ready_call, ..
            } => {
                let outcome = exec_iter.next().expect("future 数与 Ready 数一致");
                finalize_outcome(ready_call, outcome, config)
            }
        };
        terminates.push(outcome.result.terminate);
        messages.push(emit_finalized(call, &outcome, emit));
    }

    Batch {
        terminate: !messages.is_empty() && terminates.iter().all(|&t| t),
        messages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Api, Usage};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ----- 测试用的脚本化 Provider 与计数工具 -----

    struct ScriptedProvider {
        turns: Vec<Vec<StreamEvent>>,
        call: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Provider for ScriptedProvider {
        async fn stream(
            &self,
            _model: &Model,
            _context: &Context,
            _options: &StreamOptions,
            _abort: AbortSignal,
        ) -> crate::provider::EventStream {
            let (tx, rx) = tokio::sync::mpsc::channel(64);
            let index = self.call.fetch_add(1, Ordering::SeqCst);
            let turn = self
                .turns
                .get(index)
                .cloned()
                .unwrap_or_else(|| self.turns.last().cloned().unwrap_or_default());
            tokio::spawn(async move {
                for ev in turn {
                    let _ = tx.send(ev).await;
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                }
            });
            rx
        }
    }

    struct CountTool {
        calls: Arc<Mutex<Vec<Value>>>,
    }

    #[async_trait::async_trait]
    impl AgentTool for CountTool {
        fn name(&self) -> &'static str {
            "count"
        }
        fn description(&self) -> String {
            "count things".into()
        }
        fn parameters(&self) -> Value {
            json!({"type": "object", "properties": {"n": {"type": "number"}}, "required": ["n"]})
        }
        async fn execute(
            &self,
            _ctx: &ToolContext,
            args: &Value,
            _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
        ) -> Result<ToolOutput, String> {
            self.calls.lock().unwrap().push(args.clone());
            Ok(ToolOutput::text(format!("count={}", args["n"])))
        }
    }

    fn test_model() -> Model {
        Model {
            id: "mock".into(),
            name: "mock".into(),
            api: Api::OpenAICompletions,
            base_url: "http://localhost".into(),
            max_tokens: 1024,
            context_window: 0,
        }
    }

    fn assistant_message(content: Vec<ContentBlock>, reason: StopReason) -> Message {
        Message::Assistant {
            content,
            api: "openai-completions".into(),
            provider: "mock".into(),
            model: "mock".into(),
            usage: Usage::default(),
            stop_reason: reason,
            error_message: None,
            timestamp: 0,
            duration_ms: None,
        }
    }

    fn tool_use_turn(call_id: &str, n: i64) -> Vec<StreamEvent> {
        let call = ContentBlock::ToolCall {
            id: call_id.into(),
            name: "count".into(),
            arguments: json!({"n": n}),
        };
        vec![
            StreamEvent::Start,
            StreamEvent::ToolCallEnd {
                content_index: 0,
                call: call.clone(),
            },
            StreamEvent::Done {
                reason: StopReason::ToolUse,
                usage: Usage::default(),
                message: Box::new(assistant_message(vec![call], StopReason::ToolUse)),
            },
        ]
    }

    fn stop_turn(text: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::Start,
            StreamEvent::Done {
                reason: StopReason::Stop,
                usage: Usage::default(),
                message: Box::new(assistant_message(
                    vec![ContentBlock::Text { text: text.into() }],
                    StopReason::Stop,
                )),
            },
        ]
    }

    fn test_config(
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
        before: Option<BeforeToolCallHook>,
    ) -> AgentLoopConfig {
        AgentLoopConfig {
            model: test_model(),
            provider,
            tools,
            tool_context: ToolContext {
                workspace: std::env::temp_dir(),
                memory_dir: None,
                read_roots: Vec::new(),
                permissions: Arc::new(Default::default()),
                sandbox: crate::permissions::SandboxMode::DangerFullAccess,
                abort: AbortSignal::new(),
            },
            options: StreamOptions::default(),
            tool_execution: ToolExecutionMode::Sequential,
            steering: MessageQueue::new(),
            follow_up: MessageQueue::new(),
            before_tool_call: before,
            after_tool_call: None,
            transform_context: None,
        }
    }

    #[tokio::test]
    async fn loop_executes_tool_then_stops() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let registry = Arc::new(ToolRegistry::new(vec![Arc::new(CountTool {
            calls: calls.clone(),
        })]));

        let provider = Arc::new(ScriptedProvider {
            turns: vec![tool_use_turn("t1", 3), stop_turn("done")],
            call: AtomicUsize::new(0),
        });

        let sink: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink2 = sink.clone();
        let emit: Emitter = Arc::new(move |event: AgentEvent| {
            sink2.lock().unwrap().push(event);
        });

        let config = test_config(provider, registry, None);
        let new_messages = run_agent_loop(
            vec![Message::user_text("please count")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        // user → assistant(toolUse) → toolResult → assistant(stop)
        assert_eq!(new_messages.len(), 4);
        assert_eq!(new_messages[0].role(), "user");
        assert_eq!(new_messages[1].role(), "assistant");
        assert_eq!(new_messages[2].role(), "toolResult");
        match &new_messages[2] {
            Message::ToolResult {
                content, is_error, ..
            } => {
                assert!(!is_error);
                assert!(
                    matches!(&content[0], ToolResultContent::Text { text } if text == "count=3")
                );
            }
            other => panic!("expected toolResult, got {other:?}"),
        }
        assert_eq!(new_messages[3].role(), "assistant");
        assert_eq!(*calls.lock().unwrap(), vec![json!({"n": 3})]);

        let events = sink.lock().unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolExecutionStart { tool_name, .. } if tool_name == "count")));
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolExecutionEnd {
                is_error: false,
                ..
            }
        )));
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
    }

    #[tokio::test]
    async fn before_hook_can_block_tool() {
        let registry = Arc::new(ToolRegistry::new(vec![Arc::new(CountTool {
            calls: Arc::new(Mutex::new(Vec::new())),
        })]));
        let provider = Arc::new(ScriptedProvider {
            turns: vec![tool_use_turn("t1", 1), stop_turn("ok")],
            call: AtomicUsize::new(0),
        });
        let config = test_config(
            provider,
            registry,
            Some(Arc::new(|_call, _args| {
                Some(BeforeToolCallOutcome {
                    block: true,
                    reason: Some("不允许".into()),
                    terminate: false,
                })
            })),
        );
        let emit: Emitter = Arc::new(|_| {});

        let new_messages = run_agent_loop(
            vec![Message::user_text("go")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        match &new_messages[2] {
            Message::ToolResult {
                is_error, content, ..
            } => {
                assert!(is_error);
                assert!(
                    matches!(&content[0], ToolResultContent::Text { text } if text == "不允许")
                );
            }
            other => panic!("expected blocked toolResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn length_stop_rejects_tool_calls() {
        let registry = Arc::new(ToolRegistry::new(vec![Arc::new(CountTool {
            calls: Arc::new(Mutex::new(Vec::new())),
        })]));
        let length_turn = vec![StreamEvent::Done {
            reason: StopReason::Length,
            usage: Usage::default(),
            message: Box::new(assistant_message(
                vec![ContentBlock::ToolCall {
                    id: "t1".into(),
                    name: "count".into(),
                    arguments: json!({"n": 1}),
                }],
                StopReason::Length,
            )),
        }];
        let provider = Arc::new(ScriptedProvider {
            turns: vec![length_turn, stop_turn("retry done")],
            call: AtomicUsize::new(0),
        });
        let config = test_config(provider, registry, None);
        let emit: Emitter = Arc::new(|_| {});

        let new_messages = run_agent_loop(
            vec![Message::user_text("go")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        match &new_messages[2] {
            Message::ToolResult {
                is_error, content, ..
            } => {
                assert!(is_error);
                assert!(
                    matches!(&content[0], ToolResultContent::Text { text } if text.contains("token limit"))
                );
            }
            other => panic!("expected rejected toolResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transform_context_prunes_before_llm_call() {
        use crate::context::prune_transform;
        use std::sync::atomic::AtomicUsize;

        // 记录 provider 每次看到的请求长度；第一轮结束后推 follow-up
        // 强制出现第二次 LLM 调用
        struct RecordingProvider {
            inner: ScriptedProvider,
            seen: Arc<Mutex<Vec<usize>>>,
            follow_up: MessageQueue,
        }
        #[async_trait::async_trait]
        impl Provider for RecordingProvider {
            async fn stream(
                &self,
                model: &Model,
                context: &Context,
                options: &StreamOptions,
                abort: AbortSignal,
            ) -> crate::provider::EventStream {
                if self.inner.call.load(Ordering::SeqCst) == 0 {
                    self.follow_up.push(Message::user_text("follow-up"));
                }
                self.seen.lock().unwrap().push(context.messages.len());
                self.inner.stream(model, context, options, abort).await
            }
        }

        let registry = Arc::new(ToolRegistry::new(vec![]));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let follow_up = MessageQueue::new();
        let provider = Arc::new(RecordingProvider {
            inner: ScriptedProvider {
                turns: vec![stop_turn("first"), stop_turn("second")],
                call: AtomicUsize::new(0),
            },
            seen: seen.clone(),
            follow_up: follow_up.clone(),
        });

        let mut config = test_config(provider, registry, None);
        config.follow_up = follow_up;
        // 预算很小：第二次调用时历史超限，transform 应裁掉旧轮次
        config.transform_context = Some(Arc::new(prune_transform(80, 0)));
        let emit: Emitter = Arc::new(|_| {});

        run_agent_loop(
            vec![Message::user_text("x".repeat(500))],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], 1);
        // 第二次请求被裁剪：完整历史是 3 条（user/assistant/follow-up），
        // 超预算后只应看到 follow-up 这一条
        assert_eq!(*seen.last().unwrap(), 1);
    }

    #[tokio::test]
    async fn steering_message_is_injected() {
        let registry = Arc::new(ToolRegistry::new(vec![]));
        // 第一轮调用时往 steering 队列塞消息，模拟「运行中用户插话」
        let steering = MessageQueue::new();
        let steering_for_provider = steering.clone();
        struct PushProvider {
            steering: MessageQueue,
            inner: ScriptedProvider,
        }
        #[async_trait::async_trait]
        impl Provider for PushProvider {
            async fn stream(
                &self,
                model: &Model,
                context: &Context,
                options: &StreamOptions,
                abort: AbortSignal,
            ) -> crate::provider::EventStream {
                // 只在第一轮推 —— 否则每轮都注入新消息，循环永不停止
                if self.inner.call.load(Ordering::SeqCst) == 0 {
                    self.steering.push(Message::user_text("wait, also do X"));
                }
                self.inner.stream(model, context, options, abort).await
            }
        }
        let provider = Arc::new(PushProvider {
            steering: steering_for_provider,
            inner: ScriptedProvider {
                turns: vec![stop_turn("first"), stop_turn("second")],
                call: AtomicUsize::new(0),
            },
        });

        let mut config = test_config(provider, registry, None);
        config.steering = steering;
        let emit: Emitter = Arc::new(|_| {});

        let new_messages = run_agent_loop(
            vec![Message::user_text("start")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        // user → assistant(first) → user(steering) → assistant(second)
        assert_eq!(new_messages.len(), 4);
        assert_eq!(new_messages[2].role(), "user");
        match &new_messages[2] {
            Message::User { content, .. } => assert!(content.contains("also do X")),
            other => panic!("expected steering user message, got {other:?}"),
        }
    }
}
