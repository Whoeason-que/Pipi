//! 本地通知策略。桌面与 Web Push 共用同一份运行结果、身份与去重。
use std::collections::VecDeque;

use pipi_agent::agent_loop::AgentEvent;
use pipi_protocol::{Message, StopReason};
use serde::{Deserialize, Serialize};

use crate::events::{EventEmitter, RuntimeEvent};

#[cfg(feature = "server")]
pub mod push;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotificationSettings {
    pub desktop_enabled: bool,
    pub completed: bool,
    pub failed: bool,
    pub approval: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            desktop_enabled: false,
            completed: true,
            failed: true,
            approval: true,
        }
    }
}

impl NotificationSettings {
    pub fn allows(&self, kind: NotificationKind) -> bool {
        match kind {
            NotificationKind::Completed => self.completed,
            NotificationKind::Failed => self.failed,
            NotificationKind::Approval => self.approval,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationKind {
    Completed,
    Failed,
    Approval,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationEnvelope {
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
    pub id: String,
    pub kind: NotificationKind,
    pub title: String,
    pub body: String,
}

impl NotificationEnvelope {
    pub fn new(agent: &str, session: &str, run: usize, kind: NotificationKind) -> Self {
        Self {
            agent_name: agent.into(),
            session_id: session.into(),
            run_id: run,
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            title: format!("Pipi · {agent}"),
            // 不把对话、命令或供应商错误发到锁屏。
            body: match kind {
                NotificationKind::Completed => "Reply completed",
                NotificationKind::Failed => "Run failed. Open Pipi for details.",
                NotificationKind::Approval => "Waiting for your approval",
            }
            .into(),
        }
    }
}

#[derive(Default)]
pub(crate) struct NotificationPolicy {
    // 有界审批请求缓存；重发同一个审批 envelope 不会重复提醒。
    seen: VecDeque<(String, String, usize, String)>,
}

impl NotificationPolicy {
    pub(crate) fn observe(&mut self, event: &RuntimeEvent) -> Option<NotificationEnvelope> {
        if let RuntimeEvent::SessionSwitched(meta) = event {
            for (agent, session, run, _) in &mut self.seen {
                if agent == &meta.agent_name && session == &meta.session_id && *run == meta.run_id {
                    session.clone_from(&meta.to_session_id);
                }
            }
            return None;
        }
        let (agent, session, run, kind, discriminator) = match event {
            RuntimeEvent::ApprovalRequest(meta) => (
                &meta.agent_name,
                &meta.session_id,
                meta.run_id,
                NotificationKind::Approval,
                meta.request_id.clone(),
            ),
            _ => return None,
        };
        let key = (agent.clone(), session.clone(), run, discriminator);
        if self.seen.contains(&key) {
            return None;
        }
        self.seen.push_back(key);
        if self.seen.len() > 512 {
            self.seen.pop_front();
        }
        Some(NotificationEnvelope::new(agent, session, run, kind))
    }
}

/// 只装配到用户 prompt 的运行入口。手动压缩不经过它；错误等最终结果确认后
/// 才通知，因此摘要失败后继续正常收尾不会误报「运行失败」。
pub(crate) fn run_emitter(sink: EventEmitter, temporary: bool) -> EventEmitter {
    let state = std::sync::Mutex::new((false, false)); // (observed error, notified)
    std::sync::Arc::new(move |event| {
        let notice = if temporary {
            None
        } else {
            let mut state = state.lock().unwrap();
            match &event {
                RuntimeEvent::SessionError(_) => {
                    state.0 = true;
                    None
                }
                RuntimeEvent::AgentEvent(meta) => match &meta.event {
                    AgentEvent::AgentEnd { messages } if !state.1 => {
                        state.1 = true;
                        let stop = messages.iter().rev().find_map(|message| match message {
                            Message::Assistant { stop_reason, .. } => Some(*stop_reason),
                            _ => None,
                        });
                        let kind = match stop {
                            Some(StopReason::Stop | StopReason::Length) => {
                                Some(NotificationKind::Completed)
                            }
                            Some(StopReason::Error) => Some(NotificationKind::Failed),
                            None if state.0 => Some(NotificationKind::Failed),
                            _ => None,
                        };
                        kind.map(|kind| {
                            NotificationEnvelope::new(
                                &meta.agent_name,
                                &meta.session_id,
                                meta.run_id,
                                kind,
                            )
                        })
                    }
                    _ => None,
                },
                _ => None,
            }
        };
        sink(event);
        if let Some(notice) = notice {
            sink(RuntimeEvent::Notification(notice));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{AgentEventEnvelope, SessionErrorEnvelope, SessionSwitchedEnvelope};

    fn end(session: &str, run: usize, messages: Vec<Message>) -> RuntimeEvent {
        RuntimeEvent::AgentEvent(AgentEventEnvelope {
            agent_name: "a".into(),
            session_id: session.into(),
            run_id: run,
            event: AgentEvent::AgentEnd { messages },
        })
    }

    #[test]
    fn stop_temporary_runs_and_compaction_are_silent() {
        for (temporary, messages) in [
            (false, vec![]),
            (
                false,
                vec![Message::assistant_error(
                    "stopped",
                    "m",
                    StopReason::Aborted,
                )],
            ),
            (
                true,
                vec![Message::assistant_error("secret", "m", StopReason::Error)],
            ),
        ] {
            let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let output = seen.clone();
            let sink = run_emitter(
                std::sync::Arc::new(move |event| output.lock().unwrap().push(event)),
                temporary,
            );
            sink(end("s", 1, messages));
            assert!(
                !seen
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| matches!(event, RuntimeEvent::Notification(_)))
            );
        }
        let mut policy = NotificationPolicy::default();
        assert!(policy.observe(&end("s", 1, vec![])).is_none());
    }

    #[test]
    fn final_result_wins_over_recoverable_compaction_error_and_uses_new_id() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = seen.clone();
        let sink = run_emitter(
            std::sync::Arc::new(move |event| output.lock().unwrap().push(event)),
            false,
        );
        sink(RuntimeEvent::SessionError(SessionErrorEnvelope {
            agent_name: "a".into(),
            session_id: "old".into(),
            run_id: 1,
            message: "summary error".into(),
        }));
        let success = Message::assistant_error("", "m", StopReason::Stop);
        sink(end("new", 1, vec![success.clone()]));
        sink(end("new", 1, vec![success]));
        let seen = seen.lock().unwrap();
        let notices: Vec<_> = seen
            .iter()
            .filter_map(|event| {
                if let RuntimeEvent::Notification(notice) = event {
                    Some(notice)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, NotificationKind::Completed);
        assert_eq!(notices[0].session_id, "new");
    }

    #[test]
    fn infrastructure_failure_without_assistant_is_reported_once() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = seen.clone();
        let sink = run_emitter(
            std::sync::Arc::new(move |event| output.lock().unwrap().push(event)),
            false,
        );
        sink(RuntimeEvent::SessionError(SessionErrorEnvelope {
            agent_name: "a".into(),
            session_id: "s".into(),
            run_id: 1,
            message: "secret".into(),
        }));
        sink(end("s", 1, vec![]));
        sink(end("s", 1, vec![]));
        let seen = seen.lock().unwrap();
        let notices: Vec<_> = seen
            .iter()
            .filter_map(|event| {
                if let RuntimeEvent::Notification(notice) = event {
                    Some(notice)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, NotificationKind::Failed);
        assert!(!notices[0].body.contains("secret"));
    }

    #[test]
    fn approval_rekey_preserves_dedup_and_sessions_are_independent() {
        let mut policy = NotificationPolicy::default();
        let approval = |s: &str| {
            RuntimeEvent::ApprovalRequest(crate::events::ApprovalRequestEnvelope {
                agent_name: "a".into(),
                session_id: s.into(),
                run_id: 1,
                request_id: "request".into(),
                command: "secret".into(),
                missing: vec![],
            })
        };
        assert!(policy.observe(&approval("old")).is_some());
        policy.observe(&RuntimeEvent::SessionSwitched(SessionSwitchedEnvelope {
            agent_name: "a".into(),
            session_id: "old".into(),
            run_id: 1,
            to_session_id: "new".into(),
            archived: true,
        }));
        assert!(policy.observe(&approval("new")).is_none());
        assert!(policy.observe(&approval("other")).is_some());
    }
}
