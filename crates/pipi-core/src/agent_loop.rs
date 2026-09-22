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
    pub terminate: bool,
}

pub type BeforeToolCallHook =
    Arc<dyn Fn(&ContentBlock, &Value) -> Option<BeforeToolCallOutcome> + Send + Sync>;
pub type AfterToolCallHook = Arc<dyn Fn(&ContentBlock, &mut ToolOutput, &mut bool) + Send + Sync>;

/// 上下文变换（pi 的 transformContext 钩子）：每次 LLM 调用前对消息历史
/// 做变换（裁剪、注入等）。默认恒等；见 [`crate::context::prune_transform`]。
pub type TransformContextHook = Arc<dyn Fn(Vec<Message>) -> Vec<Message> + Send + Sync>;

/// Clone 用于「一次运行结束收割迟到 steering 后再起一轮」的续跑场景。
#[derive(Clone)]
pub struct AgentLoopConfig {
    pub model: Model,
    pub provider: Arc<dyn Provider>,
    pub tools: Arc<ToolRegistry>,
    pub tool_context: ToolContext,
    pub options: StreamOptions,
    /// 失败重发策略（全局设置 `retry` 段；分类见 [`crate::retry`]）。
    pub retry: crate::retry::RetryPolicy,
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
///
/// 失败的可重试错误在这里重发整轮请求（分类与退避见 [`crate::retry`]，判定规则：
/// - 终态错误（请求不合法、鉴权、上下文超限、配额耗尽……）直接收尾；
/// - **已经向用户输出过正文（文本 / 思维链增量）就不再重放** —— 重复文本比失败更糟，
///   这条对齐 hermes 的「已吐过 delta 不重试」；
/// - 例外：只有工具调用块（参数被打断）时允许重放 —— 半截参数绝不交给工具执行；
/// - 无进展超时最多额外重试 1 次（每次尝试都要花掉整个 timeout 预算）；
/// - 用户中止永远不重试。
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
    // 发送前最后一道闸：修复工具配对（缺失结果补合成错误、孤儿结果丢弃）。
    // 破损历史无论来自进程中断、批次中止还是手工编辑，都会让端点按协议 400。
    let effective_messages = crate::context::repair_tool_pairing(effective_messages);
    let wire = Context {
        system_prompt: (!context.system_prompt.is_empty()).then(|| context.system_prompt.clone()),
        messages: effective_messages,
        tools: config.tools.wire_tools(),
    };

    let finish = |emit: &Emitter, message: Message| -> Message {
        emit(AgentEvent::MessageEnd {
            message: message.clone(),
        });
        message
    };
    let aborted = |emit: &Emitter| -> Message {
        finish(
            emit,
            Message::assistant_error("已中止", config.model.display_name(), StopReason::Aborted),
        )
    };
    let failed = |emit: &Emitter, message: String| -> Message {
        finish(
            emit,
            Message::assistant_error(message, config.model.display_name(), StopReason::Error),
        )
    };

    let mut failures: u32 = 0;
    loop {
        let mut rx = config
            .provider
            .stream(&config.model, &wire, &config.options, abort.clone())
            .await;

        // 部分消息累积：Text/Thinking 增量填充；工具调用参数以 Value::String
        // （原始 JSON 片段）暂存，ToolCallEnd 时替换为解析后的块。
        let mut blocks: Vec<ContentBlock> = Vec::new();
        // 本次尝试是否已经向用户输出过正文 —— 决定失败后能不能重放。
        let mut prose_visible = false;
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

        let failure: (String, Option<crate::retry::RetryHint>) = loop {
            match rx.recv().await {
                None => {
                    if abort.is_aborted() {
                        return aborted(emit);
                    }
                    // channel 关闭且用户没中止：流被截断（provider 侧的
                    // `流提前结束` 会走上面的 Error 分支，这里是兜底）
                    break (
                        STREAM_TRUNCATED.into(),
                        Some(crate::retry::RetryHint::plain()),
                    );
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
                    prose_visible = true;
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
                    prose_visible = true;
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
                    return finish(emit, *message);
                }
                Some(StreamEvent::Error { message, retry }) => break (message, retry),
            }
        };

        // —— 失败收尾：先判终态，再判能不能重放 ——
        let (message, hint) = failure;
        let Some(hint) = hint else {
            return failed(emit, message);
        };
        if abort.is_aborted() {
            return aborted(emit);
        }
        if prose_visible {
            // 已经输出过正文：重放会让用户看到重复内容，按终态报错（用户可手动继续）
            return failed(emit, format!("{message}（已输出部分内容，未自动重试）"));
        }

        failures += 1;
        match crate::retry::retry_decision(&config.retry, failures, Some(hint)) {
            crate::retry::RetryDecision::Retry { delay } => {
                emit(AgentEvent::RetryStart {
                    attempt: failures,
                    max_attempts: config.retry.max_attempts,
                    delay_ms: delay.as_millis() as u64,
                    cause: message,
                });
                if sleep_or_abort(delay, abort).await {
                    return aborted(emit);
                }
            }
            crate::retry::RetryDecision::GiveUp(reason) => {
                return failed(emit, give_up_message(&message, failures, reason));
            }
        }
    }
}

/// 流被截断（没等到结束帧）时的兜底说明。
const STREAM_TRUNCATED: &str = "流提前结束：未收到结束帧（连接被中断或代理截断）";

/// 退避等待，期间响应中止。返回 `true` = 被中止。
async fn sleep_or_abort(delay: std::time::Duration, abort: &AbortSignal) -> bool {
    tokio::select! {
        _ = abort.wait_aborted() => true,
        _ = tokio::time::sleep(delay) => false,
    }
}

/// 放弃重试后的错误文案：说清「试了几次」与「为什么不再试」。
fn give_up_message(cause: &str, failures: u32, reason: crate::retry::GiveUpReason) -> String {
    match reason {
        crate::retry::GiveUpReason::PolicyExhausted => {
            format!("已重试 {failures} 次仍失败：{cause}")
        }
        crate::retry::GiveUpReason::TimeoutBudget => {
            format!("请求连续超时（{failures} 次），不再重试：{cause}")
        }
        crate::retry::GiveUpReason::ServerDelayTooLong(after_ms) => format!(
            "服务端要求 {} 秒后再试，超过重试上限，已停止：{cause}",
            after_ms / 1_000
        ),
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
        // 中止不变量：批次里每个调用都必须留下结果。abort 之后的调用由
        // prepare_call 直接判为「Operation aborted」，继续走完循环即可 ——
        // 静默丢弃会让会话历史留下无主的 tool_call，供应商按协议 400
        //（与 pi 一致：abort 也产出错误结果，见 createErrorToolResult）。
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
        // 同 execute_sequential：abort 不丢弃剩余调用，每个调用都要有结果
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

    /// 可重试 / 终态错误的一轮（只发一个 Error 事件）。
    fn error_turn(message: &str, retryable: bool) -> Vec<StreamEvent> {
        vec![StreamEvent::Error {
            message: message.into(),
            retry: retryable.then(crate::retry::RetryHint::plain),
        }]
    }

    /// 已吐出正文增量、随后失败的一轮（验证「已输出正文不重放」）。
    fn prose_then_error_turn(text: &str, message: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::TextDelta {
                content_index: 0,
                delta: text.into(),
            },
            StreamEvent::Error {
                message: message.into(),
                retry: Some(crate::retry::RetryHint::plain()),
            },
        ]
    }

    /// 工具调用参数只吐了一半就失败的一轮（Q 会话里实测的故障形态）。
    fn truncated_tool_turn(message: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::ToolCallStart {
                content_index: 0,
                id: "t1".into(),
                name: "count".into(),
            },
            StreamEvent::ToolCallDelta {
                content_index: 0,
                delta: "{\"n\": 1".into(),
            },
            StreamEvent::Error {
                message: message.into(),
                retry: Some(crate::retry::RetryHint::plain()),
            },
        ]
    }

    /// 快速重试策略：次数按用例给，退避压到 1ms 让测试不等待。
    fn fast_retry(max_attempts: u32) -> crate::retry::RetryPolicy {
        crate::retry::RetryPolicy {
            max_attempts,
            base_delay_ms: 1,
            max_delay_ms: 5,
        }
    }

    /// 收集事件的 sink：返回（事件列表, Emitter）。
    fn event_sink() -> (Arc<Mutex<Vec<AgentEvent>>>, Emitter) {
        let sink: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink2 = sink.clone();
        let emit: Emitter = Arc::new(move |event: AgentEvent| {
            sink2.lock().unwrap().push(event);
        });
        (sink, emit)
    }

    fn retry_events(events: &[AgentEvent]) -> Vec<(u32, u32)> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::RetryStart {
                    attempt,
                    max_attempts,
                    ..
                } => Some((*attempt, *max_attempts)),
                _ => None,
            })
            .collect()
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
                resolved_env: Arc::new(std::collections::BTreeMap::new()),
                abort: AbortSignal::new(),
                approver: None,
            },
            options: StreamOptions::default(),
            // 既有用例都是单次尝试的脚本化 provider：默认关掉重试，
            // 需要重试的用例自己指定策略
            retry: crate::retry::RetryPolicy::NONE,
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

    // ----- 可重试错误的重发 -----

    #[tokio::test]
    async fn retryable_error_is_retried_until_success() {
        let provider = Arc::new(ScriptedProvider {
            turns: vec![
                error_turn("流错误: status 429 Too Many Requests", true),
                stop_turn("恢复后的回复"),
            ],
            call: AtomicUsize::new(0),
        });
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.retry = fast_retry(3);

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(provider.call.load(Ordering::SeqCst), 2, "应重发一次");
        let last = messages.last().expect("至少一条消息");
        match last {
            Message::Assistant {
                content,
                stop_reason,
                error_message,
                ..
            } => {
                assert_eq!(*stop_reason, StopReason::Stop);
                assert!(error_message.is_none());
                assert!(
                    matches!(&content[0], ContentBlock::Text { text } if text == "恢复后的回复")
                );
            }
            other => panic!("期望成功的助手消息，得到 {other:?}"),
        }
        let events = sink.lock().unwrap();
        assert_eq!(
            retry_events(&events),
            vec![(1, 3)],
            "应发出一次 retry_start"
        );
    }

    #[tokio::test]
    async fn terminal_error_is_not_retried() {
        let provider = Arc::new(ScriptedProvider {
            turns: vec![
                error_turn("流错误: status 400 Bad Request", false),
                stop_turn("不该走到这里"),
            ],
            call: AtomicUsize::new(0),
        });
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.retry = fast_retry(3);

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(provider.call.load(Ordering::SeqCst), 1, "终态错误不应重发");
        match messages.last().expect("至少一条消息") {
            Message::Assistant {
                stop_reason,
                error_message,
                ..
            } => {
                assert_eq!(*stop_reason, StopReason::Error);
                assert!(error_message.as_deref().unwrap_or_default().contains("400"));
            }
            other => panic!("期望错误消息，得到 {other:?}"),
        }
        assert!(retry_events(&sink.lock().unwrap()).is_empty());
    }

    #[tokio::test]
    async fn exhausted_retries_say_how_many_attempts() {
        let provider = Arc::new(ScriptedProvider {
            turns: vec![error_turn("流错误: 连接重置", true)],
            call: AtomicUsize::new(0),
        });
        let (_sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.retry = fast_retry(3);

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(provider.call.load(Ordering::SeqCst), 3, "3 次尝试后放弃");
        match messages.last().expect("至少一条消息") {
            Message::Assistant { error_message, .. } => {
                let message = error_message.clone().unwrap_or_default();
                assert!(message.contains("已重试 3 次仍失败"), "{message}");
            }
            other => panic!("期望错误消息，得到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn prose_output_blocks_retry() {
        let provider = Arc::new(ScriptedProvider {
            turns: vec![
                prose_then_error_turn("半截正文", "流错误: 连接重置"),
                stop_turn("重放会产生重复文本"),
            ],
            call: AtomicUsize::new(0),
        });
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.retry = fast_retry(3);

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(
            provider.call.load(Ordering::SeqCst),
            1,
            "已经输出过正文就不该重放"
        );
        match messages.last().expect("至少一条消息") {
            Message::Assistant { error_message, .. } => {
                let message = error_message.clone().unwrap_or_default();
                assert!(message.contains("已输出部分内容，未自动重试"), "{message}");
            }
            other => panic!("期望错误消息，得到 {other:?}"),
        }
        assert!(retry_events(&sink.lock().unwrap()).is_empty());
    }

    #[tokio::test]
    async fn truncated_tool_call_without_prose_is_retried() {
        let provider = Arc::new(ScriptedProvider {
            turns: vec![
                truncated_tool_turn("流错误: tool call `count` arrived with malformed JSON input"),
                stop_turn("重发后的回复"),
            ],
            call: AtomicUsize::new(0),
        });
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.retry = fast_retry(3);

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(
            provider.call.load(Ordering::SeqCst),
            2,
            "工具参数截断应重发"
        );
        match messages.last().expect("至少一条消息") {
            Message::Assistant { stop_reason, .. } => assert_eq!(*stop_reason, StopReason::Stop),
            other => panic!("期望成功的助手消息，得到 {other:?}"),
        }
        let events = sink.lock().unwrap();
        assert_eq!(retry_events(&events), vec![(1, 3)]);
        // 半截参数绝不交给工具执行
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolExecutionStart { .. })),
            "被截断的工具调用不应执行"
        );
    }

    #[tokio::test]
    async fn abort_during_backoff_stops_immediately() {
        let provider = Arc::new(ScriptedProvider {
            turns: vec![error_turn("流错误: 连接重置", true)],
            call: AtomicUsize::new(0),
        });
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        // 退避 5 秒：若不与 abort 竞速，这个用例会一直等下去
        config.retry = crate::retry::RetryPolicy {
            max_attempts: 5,
            base_delay_ms: 5_000,
            max_delay_ms: 5_000,
        };

        let abort = AbortSignal::new();
        let abort2 = abort.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            abort2.abort();
        });

        let started = std::time::Instant::now();
        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            abort,
        )
        .await;

        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "退避期间中止应立刻生效，实际等了 {:?}",
            started.elapsed()
        );
        match messages.last().expect("至少一条消息") {
            Message::Assistant { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::Aborted)
            }
            other => panic!("期望中止消息，得到 {other:?}"),
        }
        assert_eq!(retry_events(&sink.lock().unwrap()), vec![(1, 5)]);
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
        config.transform_context = Some(Arc::new(prune_transform(80)));
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

    // ----- 中止不变量：批次里每个调用都必须留下结果 -----

    fn two_call_turn(id1: &str, id2: &str) -> Vec<StreamEvent> {
        let call1 = ContentBlock::ToolCall {
            id: id1.into(),
            name: "count".into(),
            arguments: json!({"n": 1}),
        };
        let call2 = ContentBlock::ToolCall {
            id: id2.into(),
            name: "count".into(),
            arguments: json!({"n": 2}),
        };
        vec![
            StreamEvent::Start,
            StreamEvent::ToolCallEnd {
                content_index: 0,
                call: call1.clone(),
            },
            StreamEvent::ToolCallEnd {
                content_index: 1,
                call: call2.clone(),
            },
            StreamEvent::Done {
                reason: StopReason::ToolUse,
                usage: Usage::default(),
                message: Box::new(assistant_message(vec![call1, call2], StopReason::ToolUse)),
            },
        ]
    }

    /// 在交付助手消息前触发中止的 provider：模拟「模型已产出工具调用，
    /// 但批次执行前用户按了停止」。
    struct AbortingProvider {
        inner: ScriptedProvider,
        abort: AbortSignal,
    }

    #[async_trait::async_trait]
    impl Provider for AbortingProvider {
        async fn stream(
            &self,
            model: &Model,
            context: &Context,
            options: &StreamOptions,
            abort: AbortSignal,
        ) -> crate::provider::EventStream {
            self.abort.abort();
            self.inner.stream(model, context, options, abort).await
        }
    }

    async fn aborted_batch_yields_results_for_every_call(mode: ToolExecutionMode) {
        let registry = Arc::new(ToolRegistry::new(vec![Arc::new(CountTool {
            calls: Arc::new(Mutex::new(Vec::new())),
        })]));
        let abort = AbortSignal::new();
        let provider = Arc::new(AbortingProvider {
            inner: ScriptedProvider {
                turns: vec![two_call_turn("a1", "a2"), stop_turn("never")],
                call: AtomicUsize::new(0),
            },
            abort: abort.clone(),
        });
        let mut config = test_config(provider, registry, None);
        config.tool_execution = mode;
        let emit: Emitter = Arc::new(|_| {});

        let new_messages = run_agent_loop(
            vec![Message::user_text("go")],
            AgentContext::default(),
            config,
            emit,
            abort,
        )
        .await;

        let answered: Vec<(&str, bool)> = new_messages
            .iter()
            .filter_map(|message| match message {
                Message::ToolResult {
                    tool_call_id,
                    is_error,
                    ..
                } => Some((tool_call_id.as_str(), *is_error)),
                _ => None,
            })
            .collect();
        // 每个工具调用都要有结果（否则历史留下无主 tool_call，端点按协议 400）
        assert_eq!(
            answered,
            vec![("a1", true), ("a2", true)],
            "中止后仍应为每个调用产出错误结果"
        );
    }

    #[tokio::test]
    async fn aborted_sequential_batch_answers_every_call() {
        aborted_batch_yields_results_for_every_call(ToolExecutionMode::Sequential).await;
    }

    #[tokio::test]
    async fn aborted_parallel_batch_answers_every_call() {
        aborted_batch_yields_results_for_every_call(ToolExecutionMode::Parallel).await;
    }

    // ----- 发送前配对修复：provider 实际收到的历史必须合法 -----

    /// 记录 provider 每次请求看到的消息角色/toolCallId 的 provider。
    struct CaptureProvider {
        inner: ScriptedProvider,
        seen: Arc<Mutex<Vec<Vec<(String, String)>>>>,
    }

    #[async_trait::async_trait]
    impl Provider for CaptureProvider {
        async fn stream(
            &self,
            model: &Model,
            context: &Context,
            options: &StreamOptions,
            abort: AbortSignal,
        ) -> crate::provider::EventStream {
            let roles = context
                .messages
                .iter()
                .map(|message| match message {
                    Message::User { .. } => ("user".to_string(), String::new()),
                    Message::Assistant { .. } => ("assistant".to_string(), String::new()),
                    Message::ToolResult {
                        tool_call_id,
                        is_error,
                        ..
                    } => (
                        if *is_error { "tool-error" } else { "tool" }.to_string(),
                        tool_call_id.clone(),
                    ),
                })
                .collect();
            self.seen.lock().unwrap().push(roles);
            self.inner.stream(model, context, options, abort).await
        }
    }

    #[tokio::test]
    async fn wire_history_repairs_dangling_tool_call_before_send() {
        let registry = Arc::new(ToolRegistry::new(vec![]));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(CaptureProvider {
            inner: ScriptedProvider {
                turns: vec![stop_turn("ok")],
                call: AtomicUsize::new(0),
            },
            seen: seen.clone(),
        });
        let config = test_config(provider, registry, None);
        let emit: Emitter = Arc::new(|_| {});

        // 历史里的破损（进程中断留下的中段悬挂）：assistant 带 tool_calls
        // 之后直接跟 user 消息，没有任何 tool 结果。
        let dangling = Message::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                arguments: json!({"command": "sleep 300"}),
            }],
            api: String::new(),
            provider: String::new(),
            model: "m".into(),
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 0,
            duration_ms: None,
        };
        let context = AgentContext {
            system_prompt: String::new(),
            messages: vec![
                Message::user_text("hi"),
                dangling,
                Message::user_text("继续"),
            ],
        };

        run_agent_loop(Vec::new(), context, config, emit, AbortSignal::new()).await;

        let seen = seen.lock().unwrap();
        let first_request = &seen[0];
        assert_eq!(
            first_request.clone(),
            vec![
                ("user".to_string(), String::new()),
                ("assistant".to_string(), String::new()),
                ("tool-error".to_string(), "c1".to_string()),
                ("user".to_string(), String::new()),
            ],
            "发出去的历史里，悬挂的 tool_calls 必须被补上结果"
        );
    }
}
