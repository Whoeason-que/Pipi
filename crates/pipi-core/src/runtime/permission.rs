//! Session-bound approval plumbing. No model-accessible tool can resolve it.
use super::*;
use crate::approvals::{ApprovalDecision, ApprovalOwner, SessionToolGate};
use crate::events::ApprovalEnvelope;
use pipi_protocol::permissions::PermissionMode;

/// Ephemeral per-session overrides outlive an idle HostSession slot, but never
/// survive host restart. The run and queued prompts take immutable snapshots.
#[derive(Clone, Copy, Default)]
struct ModeEntry {
    mode: Option<PermissionMode>,
    revision: u64,
}

#[derive(Clone, Default)]
pub(super) struct SessionModes(Arc<Mutex<HashMap<SessionKey, ModeEntry>>>);

impl SessionModes {
    pub(super) fn get(&self, key: &SessionKey) -> Result<Option<PermissionMode>, String> {
        Ok(self
            .0
            .lock()
            .map_err(|e| e.to_string())?
            .get(key)
            .and_then(|entry| entry.mode))
    }
    pub(super) fn set(&self, key: SessionKey, mode: Option<PermissionMode>) -> Result<(), String> {
        let mut modes = self.0.lock().map_err(|e| e.to_string())?;
        let entry = modes.entry(key).or_default();
        if entry.mode != mode {
            entry.mode = mode;
            entry.revision += 1;
        }
        Ok(())
    }
    pub(super) fn revision(&self, key: &SessionKey) -> Result<u64, String> {
        Ok(self
            .0
            .lock()
            .map_err(|e| e.to_string())?
            .get(key)
            .map_or(0, |entry| entry.revision))
    }
    pub(super) fn forget(&self, key: &SessionKey) {
        if let Ok(mut modes) = self.0.lock() {
            modes.remove(key);
        }
    }
    pub(super) fn rekey(&self, old: &SessionKey, new: &str) {
        if let Ok(mut modes) = self.0.lock()
            && let Some(mode) = modes.remove(old)
        {
            modes.insert(SessionKey::new(&old.agent_name, new), mode);
        }
    }
}

impl RuntimeState {
    /// Change only the next run; the active run retains its immutable snapshot.
    pub fn set_session_permission_mode(
        &self,
        agent_name: &str,
        session_id: &str,
        mode: Option<PermissionMode>,
        confirm_full_access: bool,
    ) -> Result<SessionInfo, String> {
        validate_session_id(session_id)?;
        if mode == Some(PermissionMode::FullAccess) && !confirm_full_access {
            return Err("Full access requires explicit confirmation".into());
        }
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        self.ensure_not_shutting_down()?;
        let key = SessionKey::new(agent_name, session_id);
        let session = sessions.get(&key).ok_or("Session is not open")?;
        self.permission_modes.set(key, mode)?;
        let info = self.session_info_of(session)?;
        self.events.emitter()(RuntimeEvent::SessionPermission(info.clone()));
        Ok(info)
    }
    pub(super) fn session_tool_gate(
        &self,
        mode: PermissionMode,
        key: &SessionKey,
        run_id: usize,
        reviewer: Option<crate::approvals::ModelReviewer>,
        review_instructions: String,
    ) -> Arc<dyn pipi_protocol::tools::ToolGate> {
        let owner = ApprovalOwner {
            agent_name: key.agent_name.clone(),
            session_id: key.session_id.clone(),
            run_id,
        };
        let emitter = self.events.emitter();
        let runtime = self.clone();
        let audit_owner = owner.clone();
        Arc::new(SessionToolGate {
            mode,
            owner,
            broker: self.approvals.clone(),
            reviewer,
            review_instructions,
            audit_review: Arc::new(move |request, review| {
                use sha2::{Digest, Sha256};
                let sessions = runtime.sessions.lock().map_err(|e| e.to_string())?;
                let session = sessions
                    .get(&SessionKey::new(
                        &audit_owner.agent_name,
                        &audit_owner.session_id,
                    ))
                    .ok_or("Session is not open")?;
                let snapshot = session.engine.snapshot()?;
                if !snapshot.running || snapshot.run_id != audit_owner.run_id {
                    return Err("Review belongs to an inactive run".into());
                }
                let digest = Sha256::digest(
                    serde_json::to_vec(&request.arguments).map_err(|e| e.to_string())?,
                )
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
                session
                    .engine
                    .record_approval(&pipi_protocol::permissions::ApprovalAudit {
                        approval_id: request.approval_id.clone(),
                        run_id: audit_owner.run_id,
                        tool_call_id: request.tool_call_id.clone(),
                        tool_name: request.tool_name.clone(),
                        arguments_sha256: digest,
                        decision: match review.verdict {
                            crate::approvals::ReviewVerdict::Allow => "allow",
                            crate::approvals::ReviewVerdict::Deny => "deny",
                            crate::approvals::ReviewVerdict::AskUser => "ask_user",
                        }
                        .into(),
                        reviewer: "model".into(),
                        reason: review.reason.clone(),
                    })
            }),
            publish: Arc::new(move |request| {
                emitter(RuntimeEvent::SessionApproval(ApprovalEnvelope {
                    agent_name: request.owner.agent_name.clone(),
                    session_id: request.owner.session_id.clone(),
                    run_id: request.owner.run_id,
                    approval_id: request.approval_id.clone(),
                    request: Some(request.clone()),
                }));
            }),
        })
    }

    pub fn answer_approval(
        &self,
        agent_name: &str,
        session_id: &str,
        run_id: usize,
        approval_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        validate_session_id(session_id)?;
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions
            .get(&SessionKey::new(agent_name, session_id))
            .ok_or("Session is not open")?;
        let snapshot = session.engine.snapshot()?;
        if !snapshot.running || snapshot.run_id != run_id {
            return Err("Approval belongs to an inactive run".into());
        }
        let owner = ApprovalOwner {
            agent_name: agent_name.into(),
            session_id: session_id.into(),
            run_id,
        };
        self.approvals
            .resolve_with(&owner, approval_id, decision, |request| {
                use sha2::{Digest, Sha256};
                let payload = serde_json::to_vec(&request.arguments).map_err(|e| e.to_string())?;
                let digest = Sha256::digest(&payload)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                session
                    .engine
                    .record_approval(&pipi_protocol::permissions::ApprovalAudit {
                        approval_id: approval_id.into(),
                        run_id,
                        tool_call_id: request.tool_call_id.clone(),
                        tool_name: request.tool_name.clone(),
                        arguments_sha256: digest,
                        decision: match decision {
                            ApprovalDecision::AllowOnce => "allow_once",
                            ApprovalDecision::Deny => "deny",
                        }
                        .into(),
                        reviewer: "user".into(),
                        reason: request.reason.clone(),
                    })
            })?;
        self.events.emitter()(RuntimeEvent::SessionApproval(ApprovalEnvelope {
            agent_name: agent_name.into(),
            session_id: session_id.into(),
            run_id,
            approval_id: approval_id.into(),
            request: None,
        }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn overrides_are_session_scoped_and_follow_compaction_rekey() {
        let modes = SessionModes::default();
        let first = SessionKey::new("a", "s1");
        let second = SessionKey::new("a", "s2");
        modes
            .set(first.clone(), Some(PermissionMode::Plan))
            .unwrap();
        modes
            .set(second.clone(), Some(PermissionMode::Ask))
            .unwrap();
        modes.rekey(&first, "fork");
        assert_eq!(modes.get(&first).unwrap(), None);
        assert_eq!(
            modes.get(&SessionKey::new("a", "fork")).unwrap(),
            Some(PermissionMode::Plan)
        );
        assert_eq!(modes.get(&second).unwrap(), Some(PermissionMode::Ask));
        modes.set(second.clone(), None).unwrap();
        assert_eq!(modes.get(&second).unwrap(), None);
    }

    #[tokio::test]
    async fn queued_prompt_keeps_the_permission_mode_chosen_at_enqueue() {
        let runtime = RuntimeState::new(tokio::runtime::Handle::current());
        let agent: AgentDefinition = serde_json::from_value(json!({"name":"approval"})).unwrap();
        let log = Box::new(MemoryLog::new());
        let id = log.session_id().to_string();
        let engine = AgentSession::create(
            SessionStorage { log, repo: None },
            None,
            None,
            AbortSignal::new(),
        )
        .unwrap();
        let reservation = engine.reserve_run(AbortPolicy::Reset).unwrap();
        let key = SessionKey::new(&agent.name, &id);
        let mut queue = queue::PromptQueue::default();
        queue.snapshot.paused = true;
        runtime.sessions.lock().unwrap().insert(
            key.clone(),
            HostSession {
                agent,
                engine,
                temporary: true,
                queue,
                questions: vec![],
                questions_revision: 0,
                question_changed: Arc::new(tokio::sync::Notify::new()),
                active_permission_mode: None,
            },
        );
        runtime
            .permission_modes
            .set(key.clone(), Some(PermissionMode::Ask))
            .unwrap();
        runtime
            .queue_prompt("approval", &id, "queued-1", "hello", None)
            .unwrap();
        runtime
            .permission_modes
            .set(key.clone(), Some(PermissionMode::Plan))
            .unwrap();
        let sessions = runtime.sessions.lock().unwrap();
        assert_eq!(
            sessions[&key].queue.snapshot.messages[0].permission_mode,
            Some(PermissionMode::Ask)
        );
        assert_eq!(
            runtime.permission_modes.get(&key).unwrap(),
            Some(PermissionMode::Plan)
        );
        drop(sessions);
        drop(reservation);
    }

    #[tokio::test]
    async fn approval_is_bound_to_the_running_session_and_denial_reaches_gate() {
        let runtime = RuntimeState::new(tokio::runtime::Handle::current());
        let agent: AgentDefinition = serde_json::from_value(json!({"name":"approval"})).unwrap();
        let log = Box::new(MemoryLog::new());
        let id = log.session_id().to_string();
        let engine = AgentSession::create(
            SessionStorage { log, repo: None },
            None,
            None,
            AbortSignal::new(),
        )
        .unwrap();
        let reservation = engine.reserve_run(AbortPolicy::Reset).unwrap();
        let run_id = reservation.run_id();
        let key = SessionKey::new(&agent.name, &id);
        runtime.sessions.lock().unwrap().insert(
            key.clone(),
            HostSession {
                agent,
                engine,
                temporary: true,
                queue: Default::default(),
                questions: vec![],
                questions_revision: 0,
                question_changed: Arc::new(tokio::sync::Notify::new()),
                active_permission_mode: None,
            },
        );
        let mut events = runtime.events.subscribe();
        let gate =
            runtime.session_tool_gate(PermissionMode::Ask, &key, run_id, None, String::new());
        let task = tokio::spawn(async move {
            gate.authorize(
                "call-1",
                "bash",
                &json!({"command":"pwd"}),
                &AbortSignal::new(),
            )
            .await
        });
        let approval = loop {
            if let RuntimeEvent::SessionApproval(event) = events.recv().await.unwrap() {
                break event;
            }
        };
        assert!(approval.request.is_some());
        assert!(
            runtime
                .answer_approval(
                    "approval",
                    &id,
                    run_id + 1,
                    &approval.approval_id,
                    ApprovalDecision::AllowOnce
                )
                .is_err()
        );
        runtime
            .answer_approval(
                "approval",
                &id,
                run_id,
                &approval.approval_id,
                ApprovalDecision::Deny,
            )
            .unwrap();
        assert!(task.await.unwrap().unwrap_err().contains("denied"));
        assert!(
            matches!(events.recv().await.unwrap(), RuntimeEvent::SessionApproval(event) if event.request.is_none())
        );
        drop(reservation);
    }
}
