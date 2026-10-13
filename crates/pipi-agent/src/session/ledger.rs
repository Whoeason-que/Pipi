//! 尾部恢复与异常终态，只通过注入的存储端口记账。
use super::*;

/// 载入自愈：会话尾部若留下未被回答的工具调用（上一次运行被进程中断 ——
/// 应用重启、强杀，结果没来得及落盘），追加合成的失败结果条目。
///
/// 只处理**尾部**：最后一条 assistant 后面只能有已完成的 toolResult，缺失的
/// 调用追加合成结果；会话记录被修复成合法状态（append-only，
/// 不重写任何已有条目）。历史中段的悬挂无法原地修复（树的顺序不可变），
/// 由发送前的 [`pipi_context::context::repair_tool_pairing`] 兜底 —— 对齐 pi 的
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

pub(super) async fn append_terminal(
    session: &SessionHandle,
    model: &Model,
    text: String,
    reason: StopReason,
) -> Result<Vec<Message>, String> {
    let terminal = Message::assistant_error(text, model.display_name(), reason);
    let history = if let Some(repo) = &session.repo {
        rebuild_messages(&active_path(&repo.entries(&session.session_id()?)?))
    } else {
        session
            .messages
            .lock()
            .expect("session history poisoned")
            .clone()
    };
    let mut writer = session.writer.lock().map_err(|e| e.to_string())?;
    let mut appended = settle_unanswered_tail(&mut **writer, &history)
        .map_err(|e| format!("无法修复终态前的工具调用: {e}"))?;
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
