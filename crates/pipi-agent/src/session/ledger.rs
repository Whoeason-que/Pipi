//! 尾部自愈与异常终态，只通过注入的存储端口记账。
use super::*;

/// 会话尾部若留下未被回答的工具调用，追加合成的失败结果条目并返回它们。
///
/// 触发它的真实路径是**异常终态**：超时 / 中止收尾时，本轮的 assistant 与部分
/// 工具结果已经在缓冲里，紧接着就要提交 —— 提交点只保证「整轮」，不保证
/// 「配对完整」（超时的那个调用永远不会产出结果）。所以悬挂调用要在提交之前
/// 补成合成失败结果，落盘的记录才是合法形状；载入时再兜一道（幂等）。
///
/// 只处理**尾部**：最后一条 assistant 后面只能有已完成的 toolResult，缺失的
/// 调用追加合成结果（append-only，不重写任何已有条目）。历史中段的悬挂无法
/// 原地修复（树的顺序不可变），由发送前的
/// [`pipi_context::context::repair_tool_pairing`] 兜底 —— 对齐 pi 的
/// post-tools / recovery 在回合结束时结算「orphaned / aborted」调用的做法。
pub(super) fn settle_unanswered_tail(
    writer: &mut dyn SessionLog,
    messages: &[Message],
) -> Result<Vec<Message>, String> {
    let Some(index) = messages
        .iter()
        .rposition(|m| matches!(m, Message::Assistant { .. }))
    else {
        return Ok(Vec::new());
    };
    let tail = &messages[index + 1..];
    if !tail.iter().all(|m| matches!(m, Message::ToolResult { .. })) {
        return Ok(Vec::new());
    }
    let calls: Vec<(String, String)> = messages[index]
        .tool_calls()
        .iter()
        .filter_map(|call| match call {
            pipi_protocol::ContentBlock::ToolCall { id, name, .. }
                if !tail.iter().any(
                    |m| matches!(m, Message::ToolResult { tool_call_id, .. } if tool_call_id == id),
                ) =>
            {
                Some((id.clone(), name.clone()))
            }
            _ => None,
        })
        .collect();
    if calls.is_empty() {
        return Ok(Vec::new());
    }
    let mut appended = Vec::with_capacity(calls.len());
    for (id, name) in calls {
        let result = pipi_context::context::missing_tool_result(&id, &name);
        writer
            .append_message(&result)
            .map_err(|error| error.to_string())?;
        appended.push(result);
    }
    Ok(appended)
}

/// 追加一条 assistant 失败终态（超时 / 中止收尾），返回写进账本的消息
/// （尾部合成的工具结果在前，终态在后）。
pub(super) fn append_terminal(
    session: &SessionHandle,
    model: &Model,
    text: String,
    reason: StopReason,
    partial: Option<Message>,
) -> Result<Vec<Message>, String> {
    let terminal = match partial {
        Some(partial) => crate::agent_loop::interrupted_message(partial, text, reason),
        None => Message::assistant_error(text, model.display_name(), reason),
    };
    let history = session
        .messages
        .lock()
        .expect("session history poisoned")
        .clone();
    let mut writer = session.writer.lock().map_err(|e| e.to_string())?;
    let mut appended = settle_unanswered_tail(&mut **writer, &history)
        .map_err(|e| format!("Cannot repair tool calls before terminal state: {e}"))?;
    writer
        .append_message(&terminal)
        .map_err(|e| e.to_string())?;
    drop(writer);
    session
        .stats
        .lock()
        .map_err(|e| e.to_string())?
        .record(&terminal);
    appended.push(terminal);
    Ok(appended)
}
