//! Session-scoped questions. Journal transitions before exposing them to UI.
use super::*;
use crate::events::{QuestionsEnvelope, QueuedPrompt, QueuedQuestionAnswer};
use pipi_protocol::interaction::{
    Question, QuestionAnswer, QuestionEvent, QuestionState, apply_question_event,
};

pub(super) fn recovered_queue(
    questions: &[pipi_protocol::interaction::QuestionRecord],
) -> super::queue::PromptQueue {
    let mut queue = super::queue::PromptQueue::default();
    for record in questions {
        if let QuestionState::Answered { answer } = &record.status
            && !record.question.sync
        {
            queue.snapshot.messages.push(QueuedPrompt {
                id: format!("question-{}", record.question_id),
                text: "Question answer".into(),
                send_at: pipi_protocol::now_millis(),
                image_count: 0,
                content: None,
                background_result: None,
                question_answer: Some(QueuedQuestionAnswer {
                    question_id: record.question_id.clone(),
                    answer: answer.clone(),
                }),
                permission_mode: None,
            });
        }
    }
    if !queue.snapshot.messages.is_empty() {
        queue.snapshot.paused = true;
        queue.snapshot.error =
            Some("Recovered question answers are waiting; resume the queue to continue".into());
    }
    queue
}

impl RuntimeState {
    pub(super) fn question_envelope(
        &self,
        session: &HostSession,
    ) -> Result<QuestionsEnvelope, String> {
        let info = session.engine.snapshot()?;
        Ok(QuestionsEnvelope {
            agent_name: session.agent.name.clone(),
            session_id: info.session_id,
            run_id: info.run_id,
            questions: session.questions.clone(),
            revision: session.questions_revision,
        })
    }

    fn update_question(
        &self,
        session: &mut HostSession,
        event: &QuestionEvent,
    ) -> Result<QuestionsEnvelope, String> {
        let mut next = session.questions.clone();
        apply_question_event(&mut next, event)?;
        session.engine.record_question(event)?;
        session.questions = next;
        session.questions_revision += 1;
        session.question_changed.notify_waiters();
        let envelope = self.question_envelope(session)?;
        self.events.emitter()(RuntimeEvent::SessionQuestions(envelope.clone()));
        Ok(envelope)
    }

    pub(crate) fn request_question(
        &self,
        key: &SessionKey,
        run_id: usize,
        question: Question,
    ) -> Result<(String, Arc<tokio::sync::Notify>), String> {
        question.validate()?;
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions.get_mut(key).ok_or("Session is not open")?;
        let info = session.engine.snapshot()?;
        if !info.running || info.run_id != run_id {
            return Err("Question belongs to an inactive run".into());
        }
        if session
            .questions
            .iter()
            .filter(|q| matches!(q.status, QuestionState::Pending))
            .count()
            >= 20
        {
            return Err("Session already has 20 pending questions".into());
        }
        let question_id = pipi_protocol::new_id();
        self.update_question(
            session,
            &QuestionEvent::Requested {
                question_id: question_id.clone(),
                run_id,
                question,
            },
        )?;
        Ok((question_id, session.question_changed.clone()))
    }

    pub(crate) fn question_state(
        &self,
        key: &SessionKey,
        question_id: &str,
    ) -> Result<QuestionState, String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions.get(key).ok_or("Session is not open")?;
        Ok(session
            .questions
            .iter()
            .find(|q| q.question_id == question_id)
            .ok_or("Question does not exist")?
            .status
            .clone())
    }

    pub(crate) fn finish_sync_question(
        &self,
        key: &SessionKey,
        question_id: &str,
        cancel: bool,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions.get_mut(key).ok_or("Session is not open")?;
        let event = if cancel {
            QuestionEvent::Cancelled {
                question_id: question_id.into(),
            }
        } else {
            QuestionEvent::Delivered {
                question_id: question_id.into(),
                answer: None,
                timestamp: pipi_protocol::now_millis(),
            }
        };
        self.update_question(session, &event)?;
        Ok(())
    }

    pub fn answer_question(
        &self,
        agent_name: &str,
        session_id: &str,
        question_id: &str,
        answer: QuestionAnswer,
    ) -> Result<QuestionsEnvelope, String> {
        validate_session_id(session_id)?;
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        self.ensure_not_shutting_down()?;
        let key = SessionKey::new(agent_name, session_id);
        let session = sessions.get_mut(&key).ok_or("Session is not open")?;
        let record = session
            .questions
            .iter()
            .find(|q| q.question_id == question_id)
            .ok_or("Question does not exist")?;
        if matches!(&record.status, QuestionState::Answered { answer: previous } | QuestionState::Delivered { answer: previous } if previous == &answer)
        {
            return self.question_envelope(session);
        }
        let sync = record.question.sync;
        if !sync && session.queue.snapshot.messages.len() >= 100 {
            return Err("The session already has 100 queued messages".into());
        }
        let permission_mode = self.permission_modes.get(&key)?;
        let result = self.update_question(
            session,
            &QuestionEvent::Answered {
                question_id: question_id.into(),
                answer: answer.clone(),
            },
        )?;
        if !sync {
            session.queue.snapshot.messages.push(QueuedPrompt {
                id: format!("question-{question_id}"),
                text: "Question answer".into(),
                send_at: pipi_protocol::now_millis(),
                image_count: 0,
                content: None,
                background_result: None,
                question_answer: Some(QueuedQuestionAnswer {
                    question_id: question_id.into(),
                    answer,
                }),
                permission_mode,
            });
            self.publish_queue(session)?;
            self.start_queue_worker(session);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::interaction::QuestionAnswer;
    use pipi_protocol::tools::{AgentTool, ToolContext};
    use serde_json::json;

    #[tokio::test]
    async fn sync_waits_for_answer_and_async_answers_are_queued_once() {
        let runtime = RuntimeState::new(tokio::runtime::Handle::current());
        let agent: AgentDefinition = serde_json::from_value(json!({"name":"questions"})).unwrap();
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
        let tool = crate::tools::question::AskQuestionTool {
            runtime: runtime.clone(),
            key: key.clone(),
            run_id,
        };
        let ctx = ToolContext {
            workspace: std::env::temp_dir(),
            memory_dir: None,
            notebook_path: None,
            read_roots: vec![],
            permissions: Arc::new(Default::default()),
            sandbox: Default::default(),
            sandbox_policy_override: None,
            resolved_env: Arc::new(Default::default()),
            abort: AbortSignal::new(),
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let hub = runtime.events().clone();
        hub.set_desktop_sink(Arc::new(move |event| {
            if let RuntimeEvent::SessionQuestions(e) = event {
                let _ = tx.send(e);
            }
        }));
        let task = tokio::spawn(async move {
            tool.execute(
                &ctx,
                &json!({"question":"Which?", "options":["A"], "sync":true}),
                &|_| {},
            )
            .await
        });
        let event = rx.recv().await.unwrap();
        let question_id = event.questions[0].question_id.clone();
        assert!(!task.is_finished());
        let answer = QuestionAnswer {
            selected_option: Some("A".into()),
            text: "details".into(),
        };
        assert!(
            runtime
                .answer_question("other", &id, &question_id, answer.clone())
                .is_err()
        );
        runtime
            .answer_question("questions", &id, &question_id, answer.clone())
            .unwrap();
        assert!(task.await.unwrap().unwrap().content.iter().any(|c| matches!(c, pipi_protocol::ToolResultContent::Text { text } if text.contains("details"))));
        assert!(matches!(
            runtime.question_state(&key, &question_id).unwrap(),
            QuestionState::Delivered { .. }
        ));
        let (async_id, _) = runtime
            .request_question(
                &key,
                run_id,
                Question {
                    question: "Later?".into(),
                    options: vec![],
                    sync: false,
                },
            )
            .unwrap();
        let custom = QuestionAnswer {
            selected_option: None,
            text: "custom".into(),
        };
        runtime
            .answer_question("questions", &id, &async_id, custom.clone())
            .unwrap();
        runtime
            .answer_question("questions", &id, &async_id, custom)
            .unwrap();
        let sessions = runtime.sessions.lock().unwrap();
        assert_eq!(sessions[&key].queue.snapshot.messages.len(), 1);
        drop(sessions);
        drop(reservation);
    }
}
