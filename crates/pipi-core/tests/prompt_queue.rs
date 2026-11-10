//! 待发箱黑盒回归：实际宿主运行、HTTP provider 与会话账本，不用前端计时器。
use pipi_core::runtime::RuntimeState;
use pipi_core::settings::{ApiMode, ProviderConfig, Settings, save_settings};
use pipi_protocol::{Api, Message, Model};
use pipi_venv::agents;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Endpoint {
    base: String,
    requests: Arc<Mutex<Vec<(u64, Value)>>>,
    release: Option<mpsc::Sender<()>>,
    stop: Arc<AtomicBool>,
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
fn endpoint(gated: bool, fail_first: bool) -> Endpoint {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let (release, wait) = mpsc::channel();
    std::thread::spawn(move || {
        while !done.load(Ordering::Acquire) {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
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
                        .and_then(|s| s.trim().parse::<usize>().ok())
                })
                .unwrap();
            let mut body = vec![0; size];
            stream.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            let index = {
                let mut seen = seen.lock().unwrap();
                let index = seen.len();
                seen.push((now_ms(), request));
                index
            };
            if index == 0 && gated {
                let _ = wait.recv_timeout(Duration::from_secs(5));
            }
            if index == 0 && fail_first {
                let body = r#"{"error":{"message":"fixture failure","type":"server_error"}}"#;
                let _ = write!(
                    stream,
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                continue;
            }
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
    Endpoint {
        base,
        requests,
        release: Some(release),
        stop,
    }
}

struct Environment(
    Vec<(&'static str, Option<std::ffi::OsString>)>,
    std::path::PathBuf,
);
impl Environment {
    fn new() -> Self {
        let home = pipi_test_support::temp_dir("pipi-prompt-queue");
        let mut vars = Vec::new();
        for (key, path) in [
            ("HOME", home.clone()),
            ("PIPI_HOME", home.join("pipi")),
            ("AV_HOME", home.join("av")),
        ] {
            vars.push((key, std::env::var_os(key)));
            pipi_test_support::set_env_var(key, path);
        }
        Self(vars, home)
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (key, previous) in &self.0 {
            if let Some(value) = previous {
                pipi_test_support::set_env_var(key, value);
            } else {
                pipi_test_support::remove_env_var(key);
            }
        }
        let _ = std::fs::remove_dir_all(&self.1);
    }
}
fn fixture(base: &str) {
    let workspace = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let model = Model {
        id: "fixture".into(),
        name: "Fixture".into(),
        api: Api::OpenAICompletions,
        base_url: base.into(),
        max_tokens: 256,
        context_window: 0,
        reasoning_effort: None,
    };
    agents::create_agent(
        "queued",
        "queue test",
        workspace.to_str(),
        None,
        Some("fixture"),
        Some(model),
    )
    .unwrap();
    let mut settings = Settings::default();
    settings.retry.max_attempts = 1;
    settings.providers = vec![ProviderConfig {
        id: "local".into(),
        name: "Local".into(),
        api: Api::OpenAICompletions,
        api_mode: ApiMode::Manual,
        base_url: base.into(),
        env_key: None,
        api_key: Some("fixture-key".into()),
    }];
    save_settings(&settings).unwrap();
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}
#[track_caller]
fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !predicate() {
        assert!(Instant::now() < deadline, "condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn users(request: &Value) -> String {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| m["content"].to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn images_survive_immediate_steering_queue_and_session_replay() {
    let _lock = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _env = Environment::new();
    let mut endpoint = endpoint(true, false);
    fixture(&endpoint.base);
    let rt = runtime();
    let state = RuntimeState::new(rt.handle().clone());
    let id = state.create_session("queued").unwrap().session_id;
    let image = pipi_protocol::UserImage {
        data: "iVBORw0KGgppbWFnZQ==".into(),
        mime_type: "image/png".into(),
    };
    state
        .send_prompt_with_images(
            "queued",
            Some(&id),
            "start",
            None,
            state.events().emitter(),
            std::slice::from_ref(&image),
        )
        .unwrap();
    until(|| endpoint.requests.lock().unwrap().len() == 1);
    state
        .steer_with_images("queued", &id, "guide", std::slice::from_ref(&image))
        .unwrap();
    let queued = state
        .queue_prompt_with_images(
            "queued",
            &id,
            "with-image",
            "later",
            None,
            std::slice::from_ref(&image),
        )
        .unwrap();
    assert_eq!(queued.queue.messages[0].image_count, 1);
    assert!(
        !serde_json::to_string(&queued)
            .unwrap()
            .contains(&image.data)
    );
    endpoint.release.take().unwrap().send(()).unwrap();
    until(|| {
        state
            .session_info("queued", &id)
            .unwrap()
            .is_some_and(|s| !s.running && endpoint.requests.lock().unwrap().len() >= 3)
    });
    let requests = endpoint.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .all(|(_, request)| request.to_string().contains(&image.data))
    );
    drop(requests);
    let snapshot = state.open_session("queued", &id).unwrap();
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|message| matches!(
                message,
                Message::User {
                    content: pipi_protocol::UserContent::Parts(_),
                    ..
                }
            ))
            .count(),
        3
    );
}

#[test]
fn queue_waits_for_task_and_guidance_then_starts_one_run_per_message_without_duplicates() {
    let _lock = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _env = Environment::new();
    let mut endpoint = endpoint(true, false);
    fixture(&endpoint.base);
    let rt = runtime();
    let state = RuntimeState::new(rt.handle().clone());
    let id = state.create_session("queued").unwrap().session_id;
    state
        .send_prompt("queued", Some(&id), "start", None, state.events().emitter())
        .unwrap();
    until(|| endpoint.requests.lock().unwrap().len() == 1);
    state
        .queue_prompt("queued", &id, "first", "queue-a", None)
        .unwrap();
    state
        .queue_prompt("queued", &id, "first", "queue-a", None)
        .unwrap();
    state
        .queue_prompt("queued", &id, "second", "queue-b", None)
        .unwrap();
    state.steer("queued", &id, "guide-current").unwrap();
    assert_eq!(endpoint.requests.lock().unwrap().len(), 1);
    assert_eq!(
        state
            .open_session("queued", &id)
            .unwrap()
            .queue
            .messages
            .len(),
        2
    );
    assert!(
        !state
            .open_session("queued", &id)
            .unwrap()
            .messages
            .iter()
            .any(|m| matches!(m, Message::User { .. })
                && serde_json::to_string(m).unwrap().contains("queue-a"))
    );
    endpoint.release.take().unwrap().send(()).unwrap();
    until(|| {
        state
            .session_info("queued", &id)
            .unwrap()
            .is_some_and(|s| !s.running && endpoint.requests.lock().unwrap().len() == 4)
    });
    let requests = endpoint.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(!users(&requests[1].1).contains("queue-a"));
    assert!(users(&requests[1].1).contains("guide-current"));
    assert!(users(&requests[2].1).contains("queue-a"));
    assert!(!users(&requests[2].1).contains("queue-b"));
    assert!(users(&requests[3].1).contains("queue-b"));
    drop(requests);
    // 已经发送的提交也幂等；同 id 不允许换内容。
    state
        .queue_prompt("queued", &id, "first", "queue-a", None)
        .unwrap();
    assert!(
        state
            .queue_prompt("queued", &id, "first", "different", None)
            .is_err()
    );
    assert!(
        state
            .open_session("queued", &id)
            .unwrap()
            .queue
            .messages
            .is_empty()
    );
    rt.block_on(state.shutdown()).unwrap();
}

#[test]
fn delay_does_not_block_ready_messages_or_other_sessions_and_queued_slots_cannot_be_released() {
    let _lock = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _env = Environment::new();
    let endpoint = endpoint(false, false);
    fixture(&endpoint.base);
    let rt = runtime();
    let state = RuntimeState::new(rt.handle().clone());
    let id = state.create_session("queued").unwrap().session_id;
    let other = state.create_session("queued").unwrap().session_id;
    let due = now_ms() + 700;
    state
        .queue_prompt("queued", &id, "delayed", "later", Some(due))
        .unwrap();
    state.new_session("queued").unwrap();
    assert!(state.session_info("queued", &id).unwrap().is_some());
    assert!(state.archive_session("queued", &id).is_err());
    assert!(state.delete_agent("queued").is_err());
    // new_session 可以释放别的空闲槽；重新打开另一条仍可独立发送。
    state.open_session("queued", &other).unwrap();
    state
        .queue_prompt("queued", &id, "ready", "now", None)
        .unwrap();
    state
        .queue_prompt("queued", &other, "other", "parallel", None)
        .unwrap();
    until(|| endpoint.requests.lock().unwrap().len() >= 2);
    {
        let requests = endpoint.requests.lock().unwrap();
        assert!(
            requests[..2]
                .iter()
                .all(|(_, r)| !users(r).contains("later"))
        );
    }
    until(|| {
        state
            .session_info("queued", &id)
            .unwrap()
            .is_some_and(|s| !s.running && endpoint.requests.lock().unwrap().len() == 3)
    });
    let requests = endpoint.requests.lock().unwrap();
    let delayed = requests
        .iter()
        .find(|(_, r)| users(r).contains("later"))
        .unwrap();
    assert!(delayed.0 >= due, "delayed message was sent early");
    drop(requests);
    state
        .queue_prompt("queued", &id, "cancel", "never", Some(now_ms() + 60_000))
        .unwrap();
    state.cancel_queued_prompt("queued", &id, "cancel").unwrap();
    assert!(
        state
            .open_session("queued", &id)
            .unwrap()
            .queue
            .messages
            .is_empty()
    );
    rt.block_on(state.shutdown()).unwrap();
    assert!(
        state
            .queue_prompt("queued", &id, "quit", "must reject", None)
            .is_err()
    );
}

#[test]
fn stop_and_failure_pause_pending_messages_until_explicit_resume() {
    let _lock = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _env = Environment::new();
    let endpoint = endpoint(false, true);
    fixture(&endpoint.base);
    let rt = runtime();
    let state = RuntimeState::new(rt.handle().clone());
    let id = state.create_session("queued").unwrap().session_id;
    state.set_prompt_queue_paused("queued", &id, true).unwrap();
    state
        .queue_prompt("queued", &id, "fail", "will fail", None)
        .unwrap();
    state
        .queue_prompt("queued", &id, "next", "after failure", None)
        .unwrap();
    state.set_prompt_queue_paused("queued", &id, false).unwrap();
    until(|| {
        let q = state.open_session("queued", &id).unwrap().queue;
        q.paused && q.error.is_some()
    });
    assert_eq!(endpoint.requests.lock().unwrap().len(), 1);
    assert_eq!(
        state
            .open_session("queued", &id)
            .unwrap()
            .queue
            .messages
            .len(),
        1
    );
    state.set_prompt_queue_paused("queued", &id, false).unwrap();
    until(|| {
        state
            .session_info("queued", &id)
            .unwrap()
            .is_some_and(|s| !s.running && endpoint.requests.lock().unwrap().len() == 2)
    });
    state
        .queue_prompt("queued", &id, "future", "after stop", Some(now_ms() + 100))
        .unwrap();
    state.stop_run("queued", &id).unwrap();
    std::thread::sleep(Duration::from_millis(160));
    assert_eq!(endpoint.requests.lock().unwrap().len(), 2);
    assert!(state.open_session("queued", &id).unwrap().queue.paused);
    state.set_prompt_queue_paused("queued", &id, false).unwrap();
    until(|| {
        state
            .session_info("queued", &id)
            .unwrap()
            .is_some_and(|s| !s.running && endpoint.requests.lock().unwrap().len() == 3)
    });
    rt.block_on(state.shutdown()).unwrap();
}

#[test]
fn queue_rejects_invalid_inputs_and_bounds_pending_memory() {
    let _lock = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _env = Environment::new();
    fixture("http://127.0.0.1:1/v1");
    let rt = runtime();
    let state = RuntimeState::new(rt.handle().clone());
    let id = state.create_session("queued").unwrap().session_id;
    state.set_prompt_queue_paused("queued", &id, true).unwrap();
    assert!(
        state
            .queue_prompt("queued", &id, "bad/id", "text", None)
            .is_err()
    );
    assert!(
        state
            .queue_prompt("queued", &id, "empty", "  ", None)
            .is_err()
    );
    assert!(
        state
            .queue_prompt("queued", &id, "large", &"x".repeat(65537), None)
            .is_err()
    );
    assert!(
        state
            .queue_prompt("queued", &id, "future", "text", Some(u64::MAX))
            .is_err()
    );
    for index in 0..100 {
        state
            .queue_prompt("queued", &id, &format!("q-{index}"), "text", None)
            .unwrap();
    }
    assert!(
        state
            .queue_prompt("queued", &id, "overflow", "text", None)
            .is_err()
    );
    assert_eq!(
        state
            .open_session("queued", &id)
            .unwrap()
            .queue
            .messages
            .len(),
        100
    );
    rt.block_on(state.shutdown()).unwrap();
}

#[test]
fn stopping_before_any_results_pauses_future_notifications_until_a_new_manual_turn() {
    let _lock = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _env = Environment::new();
    let mut endpoint = endpoint(true, false);
    fixture(&endpoint.base);
    let rt = runtime();
    let state = RuntimeState::new(rt.handle().clone());
    let id = state.create_session("queued").unwrap().session_id;
    state
        .send_prompt("queued", Some(&id), "start", None, state.events().emitter())
        .unwrap();
    until(|| endpoint.requests.lock().unwrap().len() == 1);
    state.stop_run("queued", &id).unwrap();
    endpoint.release.take().unwrap().send(()).unwrap();
    until(|| {
        state
            .session_info("queued", &id)
            .unwrap()
            .is_some_and(|s| !s.running)
    });
    assert!(state.open_session("queued", &id).unwrap().queue.paused);
    state
        .send_prompt(
            "queued",
            Some(&id),
            "new work",
            None,
            state.events().emitter(),
        )
        .unwrap();
    assert!(!state.open_session("queued", &id).unwrap().queue.paused);
    until(|| {
        state
            .session_info("queued", &id)
            .unwrap()
            .is_some_and(|s| !s.running)
    });
    rt.block_on(state.shutdown()).unwrap();
}

#[test]
fn real_model_tool_questions_wait_or_resume_through_the_session_queue() {
    use pipi_protocol::interaction::{QuestionAnswer, QuestionState};
    let _lock = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _env = Environment::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten().take(5) {
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
                        .and_then(|s| s.trim().parse::<usize>().ok())
                })
                .unwrap();
            let mut body = vec![0; size];
            stream.read_exact(&mut body).unwrap();
            let index = {
                let mut requests = seen.lock().unwrap();
                let index = requests.len();
                requests.push(serde_json::from_slice(&body).unwrap());
                index
            };
            let (start, end) = if index == 0 || index == 2 {
                (
                    json!({"id":"fixture","object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":format!("question-{index}"),"type":"function","function":{"name":"ask_question","arguments":json!({"question":"Which?","options":["A"],"sync":index == 0}).to_string()}}]},"finish_reason":null}]}),
                    json!({"id":"fixture","object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
                )
            } else {
                (
                    json!({"id":"fixture","object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{"role":"assistant","content":"done"},"finish_reason":null}]}),
                    json!({"id":"fixture","object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
                )
            };
            let body = format!("data: {start}\n\ndata: {end}\n\ndata: [DONE]\n\n");
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    fixture(&base);
    let rt = runtime();
    let state = RuntimeState::new(rt.handle().clone());
    let sync_id = state.create_session("queued").unwrap().session_id;
    state
        .send_prompt(
            "queued",
            Some(&sync_id),
            "first",
            None,
            state.events().emitter(),
        )
        .unwrap();
    until(|| {
        !state
            .open_session("queued", &sync_id)
            .unwrap()
            .questions
            .is_empty()
    });
    let q = state.open_session("queued", &sync_id).unwrap().questions[0].clone();
    assert!(q.question.sync && matches!(q.status, QuestionState::Pending));
    assert_eq!(requests.lock().unwrap().len(), 1);
    state
        .answer_question(
            "queued",
            &sync_id,
            &q.question_id,
            QuestionAnswer {
                selected_option: Some("A".into()),
                text: "detail".into(),
            },
        )
        .unwrap();
    until(|| {
        state
            .session_info("queued", &sync_id)
            .unwrap()
            .is_some_and(|i| !i.running)
            && requests.lock().unwrap().len() == 2
    });
    assert!(requests.lock().unwrap()[1].to_string().contains("detail"));
    let async_id = state.create_session("queued").unwrap().session_id;
    state
        .send_prompt(
            "queued",
            Some(&async_id),
            "second",
            None,
            state.events().emitter(),
        )
        .unwrap();
    until(|| {
        state
            .session_info("queued", &async_id)
            .unwrap()
            .is_some_and(|i| !i.running)
            && requests.lock().unwrap().len() == 4
    });
    let q = state.open_session("queued", &async_id).unwrap().questions[0].clone();
    assert!(!q.question.sync && matches!(q.status, QuestionState::Pending));
    state
        .answer_question(
            "queued",
            &async_id,
            &q.question_id,
            QuestionAnswer {
                selected_option: None,
                text: "custom".into(),
            },
        )
        .unwrap();
    until(|| {
        state
            .session_info("queued", &async_id)
            .unwrap()
            .is_some_and(|i| !i.running)
            && requests.lock().unwrap().len() == 5
    });
    assert!(requests.lock().unwrap()[4].to_string().contains("custom"));
    assert!(matches!(
        state.open_session("queued", &async_id).unwrap().questions[0].status,
        QuestionState::Delivered { .. }
    ));
    rt.block_on(state.shutdown()).unwrap();
}

#[test]
fn plan_and_ask_modes_gate_real_tool_calls_before_any_file_write() {
    use pipi_core::approvals::ApprovalDecision;
    use pipi_protocol::permissions::PermissionMode;
    let _lock = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _env = Environment::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten().take(6) {
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
                        .and_then(|s| s.trim().parse::<usize>().ok())
                })
                .unwrap();
            let mut body = vec![0; size];
            stream.read_exact(&mut body).unwrap();
            let index = {
                let mut requests = seen.lock().unwrap();
                let index = requests.len();
                requests.push(serde_json::from_slice(&body).unwrap());
                index
            };
            let (start, end) = if index % 2 == 0 {
                (
                    json!({"id":"fixture","object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":format!("write-{index}"),"type":"function","function":{"name":"write","arguments":json!({"path":"guarded.txt","content":"created"}).to_string()}}]},"finish_reason":null}]}),
                    json!({"id":"fixture","object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
                )
            } else {
                (
                    json!({"id":"fixture","object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{"role":"assistant","content":"done"},"finish_reason":null}]}),
                    json!({"id":"fixture","object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
                )
            };
            let body = format!("data: {start}\n\ndata: {end}\n\ndata: [DONE]\n\n");
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    fixture(&base);
    let rt = runtime();
    let state = RuntimeState::new(rt.handle().clone());
    let id = state.create_session("queued").unwrap().session_id;
    let target =
        std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("workspace/guarded.txt");
    state
        .set_session_permission_mode("queued", &id, Some(PermissionMode::Plan), false)
        .unwrap();
    state
        .send_prompt("queued", Some(&id), "plan", None, state.events().emitter())
        .unwrap();
    until(|| {
        state
            .session_info("queued", &id)
            .unwrap()
            .is_some_and(|s| !s.running)
            && requests.lock().unwrap().len() >= 2
    });
    assert!(!target.exists());
    state
        .set_session_permission_mode("queued", &id, Some(PermissionMode::Ask), false)
        .unwrap();
    for (round, decision) in [
        (0, ApprovalDecision::Deny),
        (1, ApprovalDecision::AllowOnce),
    ] {
        state
            .send_prompt("queued", Some(&id), "edit", None, state.events().emitter())
            .unwrap();
        until(|| {
            !state
                .open_session("queued", &id)
                .unwrap()
                .approvals
                .is_empty()
        });
        let snapshot = state.open_session("queued", &id).unwrap();
        let approval = &snapshot.approvals[0];
        assert_eq!(approval.tool_name, "write");
        assert!(!target.exists());
        state
            .answer_approval(
                "queued",
                &id,
                snapshot.info.run_id,
                &approval.approval_id,
                decision,
            )
            .unwrap();
        until(|| {
            state
                .session_info("queued", &id)
                .unwrap()
                .is_some_and(|s| !s.running)
                && requests.lock().unwrap().len() >= 4 + round * 2
        });
    }
    assert_eq!(std::fs::read_to_string(target).unwrap(), "created");
    rt.block_on(state.shutdown()).unwrap();
}

#[test]
fn auto_review_uses_a_separate_tool_free_request_and_records_the_decision() {
    use pipi_protocol::permissions::PermissionMode;
    use pipi_protocol::session::{EntryKind, SessionRepo};
    let _lock = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _env = Environment::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten().take(3) {
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
                        .and_then(|s| s.trim().parse::<usize>().ok())
                })
                .unwrap();
            let mut body = vec![0; size];
            stream.read_exact(&mut body).unwrap();
            let index = {
                let mut requests = seen.lock().unwrap();
                let index = requests.len();
                requests.push(serde_json::from_slice(&body).unwrap());
                index
            };
            let delta = if index == 0 {
                json!({"role":"assistant","tool_calls":[{"index":0,"id":"review-write","type":"function","function":{"name":"write","arguments":json!({"path":"reviewed.txt","content":"created"}).to_string()}}]})
            } else if index == 1 {
                json!({"role":"assistant","content":r#"{"verdict":"allow","reason":"The user requested this file"}"#})
            } else {
                json!({"role":"assistant","content":"done"})
            };
            let start = json!({"id":"fixture","object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":delta,"finish_reason":null}]});
            let end = json!({"id":"fixture","object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":if index == 0 { "tool_calls" } else { "stop" }}]});
            let body = format!("data: {start}\n\ndata: {end}\n\ndata: [DONE]\n\n");
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    fixture(&base);
    let rt = runtime();
    let state = RuntimeState::new(rt.handle().clone());
    let id = state.create_session("queued").unwrap().session_id;
    state
        .set_session_permission_mode("queued", &id, Some(PermissionMode::AutoReview), false)
        .unwrap();
    state
        .send_prompt(
            "queued",
            Some(&id),
            "Create reviewed.txt",
            None,
            state.events().emitter(),
        )
        .unwrap();
    until(|| {
        state
            .session_info("queued", &id)
            .unwrap()
            .is_some_and(|s| !s.running)
            && requests.lock().unwrap().len() >= 3
    });
    let target =
        std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("workspace/reviewed.txt");
    assert_eq!(std::fs::read_to_string(target).unwrap(), "created");
    let requests = requests.lock().unwrap();
    assert!(
        requests[1]
            .get("tools")
            .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty))
    );
    assert!(requests[1].to_string().contains("Create reviewed.txt"));
    drop(requests);
    let dir = agents::load_agent("queued")
        .unwrap()
        .sessions_dir()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let repo = pipi_session::GitSessionRepo::new(dir.join("sessions.git"));
    let entries = repo.entries(&id).unwrap();
    assert!(entries.iter().any(|entry| matches!(&entry.kind, EntryKind::Approval { audit } if audit.reviewer == "model" && audit.decision == "allow" && audit.tool_call_id == "review-write")));
    rt.block_on(state.shutdown()).unwrap();
}
