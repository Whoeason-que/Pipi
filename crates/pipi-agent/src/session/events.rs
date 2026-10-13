//! loop 事件的账本与用量投影；宿主回调不能在内部锁中执行。
use super::*;

pub(super) fn make_emitter(
    hooks: SessionHooks,
    writer: SharedLog,
    stats: Arc<Mutex<SessionStatsTracker>>,
    session_id: String,
    run_id: usize,
) -> Emitter {
    Arc::new(move |event| {
        if matches!(event, AgentEvent::AgentEnd { .. }) {
            return;
        }
        hooks.agent(&session_id, run_id, event.clone());
        if let AgentEvent::MessageEnd { message } = &event {
            let result = writer
                .lock()
                .map_err(|e| format!("无法锁定会话写入器: {e}"))
                .and_then(|mut w| {
                    w.append_message(message)
                        .map_err(|e| format!("会话消息落盘失败: {e}"))
                });
            if let Err(error) = result {
                hooks.error(&session_id, run_id, error);
            }
            if message.role() == "assistant" {
                let snapshot = stats
                    .lock()
                    .map_err(|e| format!("无法更新会话统计: {e}"))
                    .map(|mut tracker| {
                        tracker.record(message);
                        tracker.snapshot()
                    });
                match snapshot {
                    Ok(stats) => (hooks.emit)(SessionEvent {
                        session_id: session_id.clone(),
                        run_id,
                        kind: SessionEventKind::Stats(stats),
                    }),
                    Err(error) => hooks.error(&session_id, run_id, error),
                }
            }
        }
    })
}
