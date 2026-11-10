//! 内核事件到宿主身份信封的适配，以及宿主索引重键。
use super::*;

pub(super) fn make_session_hooks(
    sink: EventEmitter,
    agent_name: String,
    rekey: Option<SessionRekey>,
) -> SessionHooks {
    let rekey = rekey.map(|callback| {
        let name = agent_name.clone();
        Arc::new(move |old: &str, new: &str| callback(&SessionKey::new(&name, old), new))
            as Arc<dyn Fn(&str, &str) + Send + Sync>
    });
    SessionHooks {
        rekey,
        emit: Arc::new(
            move |SessionEvent {
                      session_id,
                      run_id,
                      kind,
                  }| {
                let agent_name = agent_name.clone();
                sink(match kind {
                    SessionEventKind::Agent(event) => {
                        RuntimeEvent::AgentEvent(AgentEventEnvelope {
                            agent_name,
                            session_id,
                            run_id,
                            event,
                        })
                    }
                    SessionEventKind::Stats(stats) => {
                        RuntimeEvent::SessionStats(StatsEventEnvelope {
                            agent_name,
                            session_id,
                            run_id,
                            stats,
                        })
                    }
                    SessionEventKind::Error(message) => {
                        RuntimeEvent::SessionError(SessionErrorEnvelope {
                            agent_name,
                            session_id,
                            run_id,
                            message,
                        })
                    }
                    SessionEventKind::Switched {
                        to_session_id,
                        archived,
                    } => RuntimeEvent::SessionSwitched(SessionSwitchedEnvelope {
                        agent_name,
                        session_id,
                        run_id,
                        to_session_id,
                        archived,
                    }),
                });
            },
        ),
    }
}

pub(super) fn make_rekey(
    sessions: Arc<Mutex<HashMap<SessionKey, HostSession>>>,
    background: BackgroundTaskManager,
    browser: Arc<std::sync::OnceLock<Arc<dyn crate::tools::browser::BrowserBackend>>>,
) -> SessionRekey {
    Arc::new(move |old, new| {
        // Browser lease changes and the session index share this lock: an old
        // UI request cannot obtain a fresh lease between rekey and map update.
        let Ok(mut map) = sessions.lock() else {
            return;
        };
        background.rekey_session(&old.agent_name, &old.session_id, new);
        if let Some(browser) = browser.get() {
            browser.rekey_session(&old.agent_name, &old.session_id, new);
        }
        // 压缩分叉换了会话 id：把 notebook 带过去，右栏与模型都还能看到同一份
        // 工作文档（旧文件不删，原会话归档后仍可读）。
        if let Err(error) = pipi_venv::agents::carry_notebook(&old.agent_name, &old.session_id, new)
        {
            eprintln!("pipi: failed to carry the notebook to the forked session: {error}");
        }
        if let Some(session) = map.remove(old) {
            map.insert(SessionKey::new(&old.agent_name, new), session);
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
