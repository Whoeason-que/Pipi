//! 会话级待发箱：当前任务完成后逐条起跑；延迟只决定最早起跑时间。
//! 宿主持有消息和计时器，浏览器/视图的生命周期不会取消待发消息。
use super::*;
use crate::events::{PromptQueueEnvelope, PromptQueueSnapshot, QueuedPrompt};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::time::Duration;
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
        let now = pipi_protocol::now_millis();
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
            background_result: None,
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

    /// Keep completion notifications typed until they are written to the session ledger.
    pub(super) fn queue_background_results(
        &self,
        agent_name: &str,
        session_id: &str,
        results: Vec<pipi_protocol::BackgroundTaskResult>,
    ) -> Result<(), String> {
        validate_session_id(session_id)?;
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        self.ensure_not_shutting_down()?;
        let key = SessionKey::new(agent_name, session_id);
        if !sessions.contains_key(&key) {
            let def = agents::load_agent(agent_name)?;
            let repo = session_repo(&def)?;
            if !repo.exists(session_id)? {
                return Err("Session does not exist".into());
            }
            let session = Self::load_session_object(&def, repo, session_id)?;
            sessions.insert(key.clone(), session);
        }
        let session = sessions.get_mut(&key).ok_or("Session is not open")?;
        for result in results {
            if self.background.was_observed(&result.job_id)
                || session.queue.snapshot.messages.iter().any(|m| {
                    m.background_result
                        .as_ref()
                        .is_some_and(|r| r.job_id == result.job_id)
                })
            {
                continue;
            }
            let label = result
                .target_agent
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(if result.command.is_empty() {
                    &result.job_id
                } else {
                    &result.command
                });
            session.queue.snapshot.messages.push(QueuedPrompt {
                id: format!("bg-{}", result.job_id),
                text: label.to_string(),
                send_at: pipi_protocol::now_millis(),
                image_count: 0,
                content: None,
                background_result: Some(result),
            });
        }
        self.publish_queue(session)?;
        self.start_queue_worker(session);
        Ok(())
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
        let now = pipi_protocol::now_millis();
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
        // Consecutive ready background results share a run, but never overtake a
        // ready user message. An Agent query may have observed results since enqueue.
        let mut indices = Vec::new();
        let input = if queue.snapshot.messages[index].background_result.is_some() {
            let mut results = Vec::new();
            for (i, item) in queue.snapshot.messages.iter().enumerate().skip(index) {
                if item.send_at > now {
                    continue;
                }
                let Some(result) = &item.background_result else {
                    break;
                };
                indices.push(i);
                if !self.background.was_observed(&result.job_id) {
                    results.push(result.clone());
                }
                if indices.len() == 8 {
                    break;
                }
            }
            if results.is_empty() {
                for i in indices.into_iter().rev() {
                    queue.snapshot.messages.remove(i);
                }
                let _ = self.publish_queue(session);
                return Some(Duration::ZERO);
            }
            super::execution::PromptInput::BackgroundResults(results)
        } else {
            indices.push(index);
            super::execution::PromptInput::User {
                prompt: queue.snapshot.messages[index].text.clone(),
                content: queue.snapshot.messages[index].content.clone(),
                model: None,
            }
        };
        let result = self.send_prompt_locked(
            &mut sessions,
            &key.agent_name,
            Some(&key.session_id),
            input,
            self.events.emitter(),
        );
        let session = sessions.get_mut(&key)?;
        match result {
            Ok(_) => {
                for i in indices.into_iter().rev() {
                    session.queue.snapshot.messages.remove(i);
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{ApiMode, ProviderConfig, Settings, save_settings};
    use pipi_protocol::session::{EntryKind, SessionRepo};
    use pipi_protocol::{Api, BackgroundTaskKind, BackgroundTaskResult, BackgroundTaskStatus};
    use serde_json::json;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Instant;

    struct Environment(
        Vec<(&'static str, Option<std::ffi::OsString>)>,
        std::path::PathBuf,
    );
    impl Environment {
        fn new() -> Self {
            let home = pipi_test_support::temp_dir("pipi-background-queue");
            let vars = [
                ("HOME", home.clone()),
                ("PIPI_HOME", home.join("pipi")),
                ("AV_HOME", home.join("av")),
            ]
            .into_iter()
            .map(|(key, path)| {
                let previous = std::env::var_os(key);
                pipi_test_support::set_env_var(key, path);
                (key, previous)
            })
            .collect();
            Self(vars, home)
        }
    }
    impl Drop for Environment {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                if let Some(value) = value {
                    pipi_test_support::set_env_var(key, value);
                } else {
                    pipi_test_support::remove_env_var(key);
                }
            }
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }

    #[test]
    fn queued_results_are_batched_and_persisted_as_background_entries() {
        let _guard = pipi_test_support::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let env = Environment::new();
        let workspace = env.1.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let (server_count, server_stop) = (count.clone(), stopped.clone());
        let server = std::thread::spawn(move || {
            while !server_stop.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                let mut headers = Vec::new();
                let mut byte = [0];
                while !headers.ends_with(b"\r\n\r\n") {
                    if stream.read_exact(&mut byte).is_err() {
                        return;
                    }
                    headers.push(byte[0]);
                }
                let size = String::from_utf8_lossy(&headers)
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                let mut body = vec![0; size];
                if stream.read_exact(&mut body).is_err() {
                    return;
                }
                server_count.fetch_add(1, Ordering::Release);
                let chunk = json!({"id":"fixture","object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{"role":"assistant","content":"done"},"finish_reason":null}]});
                let end = json!({"id":"fixture","object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
                let body = format!("data: {chunk}\n\ndata: {end}\n\ndata: [DONE]\n\n");
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let model = Model {
            id: "fixture".into(),
            name: "Fixture".into(),
            api: Api::OpenAICompletions,
            base_url: url.clone(),
            max_tokens: 256,
            context_window: 0,
            reasoning_effort: None,
        };
        agents::create_agent(
            "queue-results",
            "fixture",
            workspace.to_str(),
            None,
            Some("fixture"),
            Some(model),
        )
        .unwrap();
        let settings = Settings {
            providers: vec![ProviderConfig {
                id: "local".into(),
                name: "Local".into(),
                api: Api::OpenAICompletions,
                api_mode: ApiMode::Manual,
                base_url: url,
                env_key: None,
                api_key: Some("fixture-key".into()),
            }],
            ..Default::default()
        };
        save_settings(&settings).unwrap();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(RuntimeState::new(rt.handle().clone()));
        state.start_background_waker();
        let id = state.create_session("queue-results").unwrap().session_id;
        state
            .set_prompt_queue_paused("queue-results", &id, true)
            .unwrap();
        let results: Vec<_> = (0..10)
            .map(|i| BackgroundTaskResult {
                job_id: format!("job-{i}"),
                kind: BackgroundTaskKind::Agent,
                status: BackgroundTaskStatus::Completed,
                command: String::new(),
                target_agent: Some("reviewer".into()),
                exit_code: None,
                error: None,
                output: format!("result {i}"),
            })
            .collect();
        state
            .queue_background_results("queue-results", &id, results.clone())
            .unwrap();
        state
            .queue_background_results("queue-results", &id, results)
            .unwrap();
        assert_eq!(
            state
                .open_session("queue-results", &id)
                .unwrap()
                .queue
                .messages
                .len(),
            10
        );
        // A real job completes while paused; a later model query makes its
        // already-queued notification redundant before the next run starts.
        let owner = BackgroundTaskOwner::new("queue-results", &id, 0);
        let job = rt
            .block_on(
                state
                    .background
                    .submit(crate::tools::background::BackgroundTaskSpec {
                        owner: owner.clone(),
                        command: "printf observed".into(),
                        cwd: workspace.clone(),
                        env: Default::default(),
                        sandbox: pipi_protocol::permissions::SandboxPolicy::for_workspace(
                            pipi_protocol::permissions::SandboxMode::DangerFullAccess,
                            &workspace,
                        ),
                        subscribe: true,
                    }),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while state
            .open_session("queue-results", &id)
            .unwrap()
            .queue
            .messages
            .len()
            < 11
        {
            assert!(Instant::now() < deadline, "completion was not queued");
            std::thread::sleep(Duration::from_millis(20));
        }
        rt.block_on(state.background.query(
            &owner,
            crate::tools::background::BackgroundTaskQuery {
                job_id: Some(job.job_id),
                observe: true,
                include_completed: true,
                limit: 20,
                ..Default::default()
            },
        ))
        .unwrap();
        state
            .set_prompt_queue_paused("queue-results", &id, false)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let snapshot = state.open_session("queue-results", &id).unwrap();
            if snapshot.queue.messages.is_empty()
                && !snapshot.info.running
                && count.load(Ordering::Acquire) == 2
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "background results did not finish: {:?}",
                snapshot.queue
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        rt.block_on(state.shutdown()).unwrap();
        let agent = agents::load_agent("queue-results").unwrap();
        let entries = super::super::agent_repo(&agent)
            .unwrap()
            .entries(&id)
            .unwrap();
        let jobs: Vec<_> = entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                EntryKind::BackgroundResult { result } => Some(result.job_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(jobs.len(), 10);
        for i in 0..10 {
            assert_eq!(
                jobs.iter().filter(|id| **id == format!("job-{i}")).count(),
                1
            );
        }
        stopped.store(true, Ordering::Release);
        server.join().unwrap();
    }
}
