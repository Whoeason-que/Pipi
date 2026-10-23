//! 工具批次准备、执行与结果结算。
use super::*;

pub(super) struct Batch {
    pub(super) messages: Vec<Message>,
    pub(super) terminate: bool,
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
pub(super) async fn fail_truncated_tool_calls(
    tool_calls: &[ContentBlock],
    emit: &Emitter,
) -> Batch {
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
        timestamp: pipi_protocol::now_millis(),
    }
}

/// 执行一批工具调用（对应 pi 的 executeToolCalls）。
/// bash 这类有副作用的工具强制走顺序模式。
pub(super) async fn execute_tool_calls(
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
            result: error_output("Content block is not a tool call"),
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
            };
        }
    };
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
            result: error_output("Content block is not a tool call"),
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
            Prepared::Ready { tool, call, args } => {
                run_tool(tool, &config.tool_context, &call, args, emit, abort).await
            }
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
            Prepared::Ready { .. } => exec_iter.next().expect("future 数与 Ready 数一致"),
        };
        terminates.push(outcome.result.terminate);
        messages.push(emit_finalized(call, &outcome, emit));
    }

    Batch {
        terminate: !messages.is_empty() && terminates.iter().all(|&t| t),
        messages,
    }
}
