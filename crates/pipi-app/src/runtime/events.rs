//! 事件封装与消息/用量落盘。

use super::*;

pub(super) fn make_emitter(
    sink: EventEmitter,
    writer: Arc<Mutex<SessionWriter>>,
    stats: Arc<Mutex<SessionStatsTracker>>,
    agent_name: String,
    session_id: String,
    run_id: usize,
) -> LoopEmitter {
    Arc::new(move |event: AgentEvent| {
        // AgentEnd 延迟到 run loop 回写完整历史后统一发送。
        if matches!(&event, AgentEvent::AgentEnd { .. }) {
            return;
        }
        sink(RuntimeEvent::AgentEvent(AgentEventEnvelope {
            agent_name: agent_name.clone(),
            session_id: session_id.clone(),
            run_id,
            event: event.clone(),
        }));
        if let AgentEvent::MessageEnd { message } = &event {
            match writer.lock() {
                Ok(mut writer) => {
                    if let Err(error) = writer.append_message(message) {
                        sink(RuntimeEvent::SessionError(SessionErrorEnvelope {
                            agent_name: agent_name.clone(),
                            session_id: session_id.clone(),
                            run_id,
                            message: format!("会话消息落盘失败: {error}"),
                        }));
                    }
                }
                Err(error) => {
                    sink(RuntimeEvent::SessionError(SessionErrorEnvelope {
                        agent_name: agent_name.clone(),
                        session_id: session_id.clone(),
                        run_id,
                        message: format!("无法锁定会话写入器: {error}"),
                    }));
                }
            }
            if message.role() == "assistant" {
                match stats.lock() {
                    Ok(mut tracker) => {
                        tracker.record(message);
                        sink(RuntimeEvent::SessionStats(StatsEventEnvelope {
                            agent_name: agent_name.clone(),
                            session_id: session_id.clone(),
                            run_id,
                            stats: tracker.snapshot(),
                        }));
                    }
                    Err(error) => {
                        sink(RuntimeEvent::SessionError(SessionErrorEnvelope {
                            agent_name: agent_name.clone(),
                            session_id: session_id.clone(),
                            run_id,
                            message: format!("无法更新会话统计: {error}"),
                        }));
                    }
                }
            }
        }
    })
}

pub(super) fn make_session_change_sink(event_sink: EventEmitter) -> BackgroundAgentSessionSink {
    Arc::new(move |agent_name, session_id, run_id| {
        event_sink(RuntimeEvent::SessionChanged(SessionChangedEnvelope {
            agent_name,
            session_id,
            run_id,
        }));
    })
}
