//! 会话级待发箱：当前任务完成后逐条起跑；延迟只决定最早起跑时间。
//! 宿主持有消息和计时器，浏览器/视图的生命周期不会取消待发消息。
use super::*;
use crate::events::{PromptQueueEnvelope, PromptQueueSnapshot, QueuedPrompt};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

const MAX_DELAY_MS: u64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Default)]
pub(super) struct PromptQueue {
    pub snapshot: PromptQueueSnapshot,
    pub wake: Arc<Notify>,
    pub active_run: Option<usize>,
    worker: bool,
    // 重试同一个提交不会重复发送，包括响应丢失后消息已开始运行的情况。
    accepted: VecDeque<AcceptedPrompt>,
}

struct AcceptedPrompt {
    id: String,
    text: String,
    send_at: u64,
    digest: [u8; 32],
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl RuntimeState {
    pub(super) fn queue_envelope(
        &self,
        session: &HostSession,
    ) -> Result<PromptQueueEnvelope, String> {
        let info = session.engine.snapshot()?;
        Ok(PromptQueueEnvelope {
            agent_name: session.agent.name.clone(),
            session_id: info.session_id,
            run_id: info.run_id,
            queue: session.queue.snapshot.clone(),
        })
    }

    pub(super) fn publish_queue(
        &self,
        session: &mut HostSession,
    ) -> Result<PromptQueueEnvelope, String> {
        session.queue.snapshot.revision += 1;
        let envelope = self.queue_envelope(session)?;
        self.events.emitter()(RuntimeEvent::PromptQueue(envelope.clone()));
        Ok(envelope)
    }

    pub fn queue_prompt(
        &self,
        agent_name: &str,
        session_id: &str,
        message_id: &str,
        text: &str,
        send_at: Option<u64>,
    ) -> Result<PromptQueueEnvelope, String> {
        self.queue_prompt_with_images(agent_name, session_id, message_id, text, send_at, &[])
    }
    pub fn queue_prompt_with_images(
        &self,
        agent_name: &str,
        session_id: &str,
        message_id: &str,
        text: &str,
        send_at: Option<u64>,
        images: &[pipi_protocol::UserImage],
    ) -> Result<PromptQueueEnvelope, String> {
        let content = attachments::user_content(text, images)?;
        let digest: [u8; 32] =
            Sha256::digest(serde_json::to_vec(images).map_err(|e| e.to_string())?).into();
        validate_session_id(session_id)?;
        if message_id.is_empty()
            || message_id.len() > 80
            || !message_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err("Invalid queued message ID".into());
        }
        let text = text.trim();
        if (text.is_empty() && content.is_none()) || text.len() > 64 * 1024 {
            return Err("Queued message must contain 1 to 65536 bytes".into());
        }
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        self.ensure_not_shutting_down()?;
        let session = sessions
            .get_mut(&SessionKey::new(agent_name, session_id))
            .ok_or("Session is not open; reopen it before queueing messages")?;
        if let Some(previous) = session.queue.accepted.iter().find(|m| m.id == message_id) {
            if previous.text != text
                || previous.digest != digest
                || send_at.is_some_and(|at| at != previous.send_at)
            {
                return Err("Queued message ID was already used with different content".into());
            }
            return self.queue_envelope(session);
        }
        let now = now_ms();
        let send_at = send_at.unwrap_or(now);
        if send_at > now.saturating_add(MAX_DELAY_MS) {
            return Err("Messages can be delayed by at most 30 days".into());
        }
        if session.queue.snapshot.messages.len() >= 100 {
            return Err("The session already has 100 queued messages".into());
        }
        let queued_bytes: usize = session
            .queue
            .snapshot
            .messages
            .iter()
            .map(|item| {
                item.content.as_ref().map_or(0, |content| match content {
                    pipi_protocol::UserContent::Parts(parts) => parts
                        .iter()
                        .map(|part| match part {
                            pipi_protocol::UserContentBlock::Image { data, .. } => data.len(),
                            _ => 0,
                        })
                        .sum(),
                    _ => 0,
                })
            })
            .sum();
        if queued_bytes + images.iter().map(|image| image.data.len()).sum::<usize>()
            > 8 * 1024 * 1024
        {
            return Err("Queued images exceed the 8 MiB limit".into());
        }
        let message = QueuedPrompt {
            id: message_id.into(),
            text: text.into(),
            send_at,
            image_count: images.len(),
            content,
        };
        session.queue.accepted.push_back(AcceptedPrompt {
            id: message_id.into(),
            text: text.into(),
            send_at,
            digest,
        });
        if session.queue.accepted.len() > 256 {
            session.queue.accepted.pop_front();
        }
        session.queue.snapshot.messages.push(message);
        let envelope = self.publish_queue(session)?;
        self.start_queue_worker(session);
        Ok(envelope)
    }

    pub fn cancel_queued_prompt(
        &self,
        agent_name: &str,
        session_id: &str,
        message_id: &str,
    ) -> Result<PromptQueueEnvelope, String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions
            .get_mut(&SessionKey::new(agent_name, session_id))
            .ok_or("Session is not open")?;
        // 已发送与已取消都幂等；开始运行的消息需通过 stop_run 停止。
        session
            .queue
            .snapshot
            .messages
            .retain(|m| m.id != message_id);
        let envelope = self.publish_queue(session)?;
        session.queue.wake.notify_one();
        Ok(envelope)
    }

    pub fn set_prompt_queue_paused(
        &self,
        agent_name: &str,
        session_id: &str,
        paused: bool,
    ) -> Result<PromptQueueEnvelope, String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        self.ensure_not_shutting_down()?;
        let session = sessions
            .get_mut(&SessionKey::new(agent_name, session_id))
            .ok_or("Session is not open")?;
        session.queue.snapshot.paused = paused;
        session.queue.snapshot.error = None;
        let envelope = self.publish_queue(session)?;
        self.start_queue_worker(session);
        Ok(envelope)
    }

    fn start_queue_worker(&self, session: &mut HostSession) {
        session.queue.wake.notify_one();
        if session.queue.worker {
            return;
        }
        session.queue.worker = true;
        let wake = session.queue.wake.clone();
        let runtime = self.clone();
        self.spawn_run(async move {
            loop {
                let tick_runtime = runtime.clone();
                let tick_wake = wake.clone();
                // 准备一轮会读文件、运行 git，不占用异步执行线程。
                let wait = runtime
                    .runtime
                    .spawn_blocking(move || tick_runtime.queue_tick(&tick_wake))
                    .await;
                let Ok(Some(wait)) = wait else {
                    break;
                };
                tokio::select! {
                    _ = wake.notified() => {},
                    _ = tokio::time::sleep(wait) => {},
                }
            }
        });
    }

    fn queue_tick(&self, wake: &Arc<Notify>) -> Option<Duration> {
        let mut sessions = self.sessions.lock().ok()?;
        let key = sessions.iter().find_map(|(key, session)| {
            Arc::ptr_eq(&session.queue.wake, wake).then(|| key.clone())
        })?;
        let session = sessions.get_mut(&key)?;
        let queue = &mut session.queue;
        if self.shutting_down.load(Ordering::Acquire)
            || queue.snapshot.paused
            || queue.snapshot.messages.is_empty()
        {
            queue.worker = false;
            return None;
        }
        if queue.active_run.is_some() || session.engine.snapshot().ok()?.running {
            return Some(Duration::from_secs(1));
        }
        let now = now_ms();
        // 未来的定时消息不阻塞已经到时/立即排队的消息；到时消息保持提交顺序。
        let Some(index) = queue
            .snapshot
            .messages
            .iter()
            .position(|m| m.send_at <= now)
        else {
            let next = queue.snapshot.messages.iter().map(|m| m.send_at).min()?;
            return Some(Duration::from_millis(next.saturating_sub(now).min(60_000)));
        };
        let prompt = queue.snapshot.messages[index].text.clone();
        let content = queue.snapshot.messages[index].content.clone();
        let result = self.send_prompt_locked(
            &mut sessions,
            &key.agent_name,
            Some(&key.session_id),
            super::execution::PromptInput::User {
                prompt,
                content,
                model: None,
            },
            self.events.emitter(),
        );
        let session = sessions.get_mut(&key)?;
        match result {
            Ok(_) => {
                session.queue.snapshot.messages.remove(index);
            }
            Err(error) => {
                session.queue.snapshot.paused = true;
                session.queue.snapshot.error = Some(error);
            }
        }
        let _ = self.publish_queue(session);
        Some(Duration::from_secs(1))
    }

    pub(super) fn finish_queued_run(
        &self,
        wake: &Arc<Notify>,
        run_id: usize,
        result: Result<pipi_agent::session::RunOutcome, String>,
    ) {
        let Ok(mut sessions) = self.sessions.lock() else {
            return;
        };
        let Some(session) = sessions
            .values_mut()
            .find(|s| Arc::ptr_eq(&s.queue.wake, wake))
        else {
            return;
        };
        if session.queue.active_run != Some(run_id) {
            return;
        }
        session.queue.active_run = None;
        let error = match result {
            Err(error) => Some(error),
            Ok(outcome) => outcome
                .messages
                .iter()
                .rev()
                .find_map(|message| match message {
                    Message::Assistant { stop_reason, .. } => Some(match stop_reason {
                        pipi_protocol::StopReason::Error => {
                            Some("The previous run failed; resume the queue when ready".into())
                        }
                        pipi_protocol::StopReason::Aborted => {
                            Some("The previous run was stopped; resume the queue when ready".into())
                        }
                        _ => None,
                    }),
                    _ => None,
                })
                .flatten(),
        };
        if let Some(error) = error.filter(|_| !session.queue.snapshot.messages.is_empty()) {
            session.queue.snapshot.paused = true;
            session.queue.snapshot.error = Some(error);
            let _ = self.publish_queue(session);
        }
        wake.notify_one();
    }
}
