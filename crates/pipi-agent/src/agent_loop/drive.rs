//! 轮次驱动与队列收割。
use super::*;

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
    let mut has_more_tool_calls = true;

    while has_more_tool_calls || !pending.is_empty() {
        // 注入 steering 消息
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
            break;
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
            break;
        }
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
