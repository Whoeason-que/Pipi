//! 流式响应、故障重试与协议协商。
use super::*;

/// 从 provider 事件流构建一条助手消息（对应 pi 的 streamAssistantResponse）。
///
/// 失败的可重试错误在这里重发整轮请求（分类与退避见 [`pipi_context::retry`]，判定规则：
/// - 终态错误（请求不合法、鉴权、上下文超限、配额耗尽……）直接收尾；
/// - **已经向用户输出过正文（文本 / 思维链增量）就不再重放** —— 重复文本比失败更糟，
///   这条对齐 hermes 的「已吐过 delta 不重试」；
/// - 例外：只有工具调用块（参数被打断）时允许重放 —— 半截参数绝不交给工具执行；
/// - 无进展超时最多额外重试 1 次（每次尝试都要花掉整个 timeout 预算）；
/// - 用户中止永远不重试；
/// - **协议协商**（仅 `config.options.auto_protocol`）：端点回报「模型不支持这个协议」时
///   换另一种协议再试一次。它不是故障重试（不占重试预算、不发 RetryStart、
///   最多一次），但同样受「已输出正文不重放」与中止判定约束。
pub(super) async fn stream_assistant_response(
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
    let effective_messages = pipi_context::context::repair_tool_pairing(effective_messages);
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
    // 本轮请求用的模型副本：协议协商会换协议（`attempt_model.api`），其余字段与
    // 声明一致。`config` 不可变，协商态只活在这个函数里。
    let mut attempt_model = config.model.clone();
    // 本轮换过协议没有（同一次请求最多换一次；同一进程里之后靠 provider 的
    // 进程内记忆直接命中，不再试探）
    let mut switched_protocol = false;
    loop {
        let mut rx = config
            .provider
            .stream(&attempt_model, &wire, &config.options, abort.clone())
            .await;

        // 部分消息累积：Text/Thinking 增量填充；工具调用参数以 Value::String
        // （原始 JSON 片段）暂存，ToolCallEnd 时替换为解析后的块。
        let mut blocks: Vec<ContentBlock> = Vec::new();
        // 本次尝试是否已经向用户输出过正文 —— 决定失败后能不能重放。
        let mut prose_visible = false;
        // 半截消息的身份：按本次尝试实际使用的协议/模型填
        let attempt_api = attempt_model.api.as_str().to_string();
        let attempt_name = attempt_model.display_name().to_string();
        let partial = |blocks: &[ContentBlock]| -> Message {
            Message::Assistant {
                content: blocks.to_vec(),
                api: attempt_api.clone(),
                provider: "pending".into(),
                model: attempt_name.clone(),
                usage: Default::default(),
                stop_reason: StopReason::Pending,
                error_message: None,
                timestamp: pipi_protocol::now_millis(),
                duration_ms: None,
            }
        };

        let failure: (String, Option<pipi_protocol::RetryHint>, bool) = loop {
            match rx.recv().await {
                None => {
                    if abort.is_aborted() {
                        return aborted(emit);
                    }
                    // channel 关闭且用户没中止：流被截断（provider 侧的
                    // `流提前结束` 会走上面的 Error 分支，这里是兜底）
                    break (
                        STREAM_TRUNCATED.into(),
                        Some(pipi_protocol::RetryHint::plain()),
                        false,
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
                Some(StreamEvent::Error {
                    message,
                    retry,
                    protocol_mismatch,
                }) => break (message, retry, protocol_mismatch),
            }
        };

        // —— 失败收尾：协议协商 → 终态 → 能不能重放 ——
        let (message, hint, protocol_mismatch) = failure;

        // 协议协商（仅 auto）：端点说这个模型不支持当前协议 —— 换另一种协议再试
        // 一次。不占重试预算、不发 RetryStart（这不是故障重试）；已中止或已输出
        // 正文时按既有不变量收尾。
        if protocol_mismatch && config.options.auto_protocol && !switched_protocol {
            if abort.is_aborted() {
                return aborted(emit);
            }
            if prose_visible {
                return failed(emit, format!("{message}（已输出部分内容，未自动重试）"));
            }
            switched_protocol = true;
            attempt_model.api = alternate_api(attempt_model.api);
            continue;
        }

        let Some(hint) = hint else {
            return failed(
                emit,
                if protocol_mismatch {
                    if switched_protocol {
                        format!("{message}（Anthropic / OpenAI 两种协议均已尝试）")
                    } else {
                        // 协议被用户钉死：不改写，只提示「自动」模式能逐模型选协议
                        format!(
                            "{message}（该提供商的协议已固定；改为「自动」模式可按模型选择协议）"
                        )
                    }
                } else {
                    message
                },
            );
        };
        if abort.is_aborted() {
            return aborted(emit);
        }
        if prose_visible {
            // 已经输出过正文：重放会让用户看到重复内容，按终态报错（用户可手动继续）
            return failed(emit, format!("{message}（已输出部分内容，未自动重试）"));
        }

        failures += 1;
        match pipi_context::retry::retry_decision(&config.retry, failures, Some(hint)) {
            pipi_context::retry::RetryDecision::Retry { delay } => {
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
            pipi_context::retry::RetryDecision::GiveUp(reason) => {
                return failed(emit, give_up_message(&message, failures, reason));
            }
        }
    }
}

/// 流被截断（没等到结束帧）时的兜底说明。
const STREAM_TRUNCATED: &str = "流提前结束：未收到结束帧（连接被中断或代理截断）";

/// 协议协商的另一半：Pipi 只支持这两种 wire，换协议就是它们互切。
fn alternate_api(api: Api) -> Api {
    match api {
        Api::AnthropicMessages => Api::OpenAICompletions,
        Api::OpenAICompletions => Api::AnthropicMessages,
    }
}

/// 退避等待，期间响应中止。返回 `true` = 被中止。
async fn sleep_or_abort(delay: std::time::Duration, abort: &AbortSignal) -> bool {
    tokio::select! {
        _ = abort.wait_aborted() => true,
        _ = tokio::time::sleep(delay) => false,
    }
}

/// 放弃重试后的错误文案：说清「试了几次」与「为什么不再试」。
fn give_up_message(
    cause: &str,
    failures: u32,
    reason: pipi_context::retry::GiveUpReason,
) -> String {
    match reason {
        pipi_context::retry::GiveUpReason::PolicyExhausted => {
            format!("已重试 {failures} 次仍失败：{cause}")
        }
        pipi_context::retry::GiveUpReason::TimeoutBudget => {
            format!("请求连续超时（{failures} 次），不再重试：{cause}")
        }
        pipi_context::retry::GiveUpReason::ServerDelayTooLong(after_ms) => format!(
            "服务端要求 {} 秒后再试，超过重试上限，已停止：{cause}",
            after_ms / 1_000
        ),
    }
}
