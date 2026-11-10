//! loop 事件的账本与用量投影；宿主回调不能在内部锁中执行。
use super::*;

pub(super) fn make_emitter(
    hooks: SessionHooks,
    writer: SharedLog,
    stats: Arc<Mutex<SessionStatsTracker>>,
    session_id: String,
    run_id: usize,
    abort: AbortSignal,
) -> Emitter {
    let last_checkpoint = Mutex::new(None::<std::time::Instant>);
    Arc::new(move |event| {
        if matches!(event, AgentEvent::AgentEnd { .. }) {
            return;
        }
        if let AgentEvent::MessageUpdate {
            message: message @ Message::Assistant { .. },
        } = &event
        {
            let due = {
                let mut last = last_checkpoint.lock().expect("checkpoint timer poisoned");
                let due = last.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(1));
                if due {
                    *last = Some(std::time::Instant::now());
                }
                due
            };
            if due {
                let result = writer
                    .lock()
                    .map_err(|e| format!("Cannot lock session writer: {e}"))
                    .and_then(|mut w| {
                        w.checkpoint_partial(message)
                            .map_err(|e| format!("Cannot save streaming response: {e}"))
                    });
                if let Err(error) = result {
                    hooks.error(&session_id, run_id, error);
                    abort.abort();
                }
            }
        }
        if matches!(event, AgentEvent::RetryStart { .. }) {
            let result = writer
                .lock()
                .map_err(|e| e.to_string())
                .and_then(|mut w| w.clear_partial().map_err(|e| e.to_string()));
            if let Err(error) = result {
                hooks.error(&session_id, run_id, error);
                abort.abort();
            }
            *last_checkpoint.lock().expect("checkpoint timer poisoned") = None;
        }
        if let AgentEvent::MessageEnd { message } = &event {
            let result = writer
                .lock()
                .map_err(|e| format!("Cannot lock session writer: {e}"))
                .and_then(|mut w| {
                    w.append_message(message)
                        .map_err(|e| format!("Failed to persist session message: {e}"))?;
                    if matches!(message, Message::Assistant { .. }) {
                        w.clear_partial()
                            .map_err(|e| format!("Failed to clear streaming snapshot: {e}"))?;
                    }
                    Ok(())
                });
            if matches!(message, Message::Assistant { .. }) {
                *last_checkpoint.lock().expect("checkpoint timer poisoned") = None;
            }
            if let Err(error) = result {
                hooks.error(&session_id, run_id, error);
                abort.abort();
            }
            if message.role() == "assistant" {
                let snapshot = stats
                    .lock()
                    .map_err(|e| format!("Cannot update session stats: {e}"))
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
        hooks.agent(&session_id, run_id, event);
    })
}
