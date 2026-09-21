//! 会话并发的端到端回归：会话槽的键是 **(Agent 名, 会话 id)**。
//!
//! 覆盖（都是这两轮改造新增的行为）：
//! 1. 多个 Agent 同时各跑一轮：后启动的不会顶掉前面的；
//! 2. **同一个 Agent 的多条会话同时跑**（每条独立，互不顶掉）；
//! 3. 守卫按会话：同一条会话的第二轮被拒，别的会话照跑；
//! 4. `stop_run` / `new_session` 的作用域：只停指定那条、只释放空闲的；
//! 5. 事件带身份、各写各的会话文件，互不串。
//!
//! 端点：接受连接后先回一个内容分片再保持静默 —— 让各轮都停在半途，
//! 「都 running」才是一个真实可观察的状态。

use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipi_core::agents;
use pipi_core::runtime::{EventEmitter, RuntimeEvent, RuntimeState};
use pipi_core::session::SessionWriter;
use pipi_core::settings::{save_settings, ProviderConfig, Settings, Theme};
use pipi_core::types::{Api, Message, Model};

/// 本文件的用例都改写进程级 `HOME`，必须串行执行（同 `session_repair.rs`）。
static HOME_LOCK: Mutex<()> = Mutex::new(());

fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(1) => {
                buffer.push(byte[0]);
                if buffer.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            _ => return String::new(),
        }
    }
    let headers = String::from_utf8_lossy(&buffer).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    let _ = stream.read_exact(&mut body);
    String::from_utf8_lossy(&body).to_string()
}

/// 回一个内容分片后保持静默：这一轮停在「运行中」，直到被 stop。
fn spawn_stalling_endpoint(label: &'static str) -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for mut stream in listener.incoming().flatten() {
            let _ = read_request(&mut stream);
            let chunk = format!(
                r#"{{"id":"c1","object":"chat.completion.chunk","model":"mock","choices":[{{"index":0,"delta":{{"content":"开头-{label}"}},"finish_reason":null}}]}}"#
            );
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n");
            let _ = stream.write_all(format!("data: {chunk}\n\n").as_bytes());
            let _ = stream.flush();
            held.push(stream);
        }
    });
    addr
}

fn tool_call_response(name: &str, arguments: serde_json::Value) -> String {
    let start = serde_json::json!({
        "id": "chatcmpl-tool",
        "object": "chat.completion.chunk",
        "model": "mock",
        "choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": [{
            "index": 0,
            "id": "call-write",
            "type": "function",
            "function": {"name": name, "arguments": arguments.to_string()}
        }]}, "finish_reason": null}]
    });
    let end = serde_json::json!({
        "id": "chatcmpl-tool",
        "object": "chat.completion.chunk",
        "model": "mock",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    });
    format!("data: {start}\n\ndata: {end}\n\ndata: [DONE]\n\n")
}

fn text_response(text: &str) -> String {
    let start = serde_json::json!({
        "id": "chatcmpl-text",
        "object": "chat.completion.chunk",
        "model": "mock",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": null}]
    });
    let end = serde_json::json!({
        "id": "chatcmpl-text",
        "object": "chat.completion.chunk",
        "model": "mock",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    });
    format!("data: {start}\n\ndata: {end}\n\ndata: [DONE]\n\n")
}

/// 第一轮要求真实执行 write，第二轮正常收尾。
fn spawn_write_endpoint() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let request_index = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = read_request(&mut stream);
            let index = request_index.fetch_add(1, Ordering::SeqCst);
            let body = if index == 0 {
                tool_call_response(
                    "write",
                    serde_json::json!({
                        "path": "temporary-side-effect.txt",
                        "content": "this change is real"
                    }),
                )
            } else {
                text_response("文件已写入")
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    addr
}

fn temp_home() -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!(
        "pipi-concurrent-sessions-{}-{}",
        std::process::id(),
        pipi_core::session::new_id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("创建临时 HOME");
    std::env::set_var("HOME", &home);
    home
}

/// 播种一个指向指定端点的 Agent（各自一个模型，互不影响）。
fn fixture_agent(name: &str, base_url: &str) {
    let model_id = format!("{name}-model");
    let model = Model {
        id: model_id.clone(),
        name: "Mock".into(),
        api: Api::OpenAICompletions,
        base_url: base_url.into(),
        max_tokens: 256,
        context_window: 0,
    };
    agents::create_agent(name, "并发回归", None, None, Some(&model_id), Some(model))
        .expect("播种测试 Agent");
}

fn write_settings(base_urls: &[String]) {
    // 每个端点一条 provider：provider 按 (api, base_url) 匹配，用到的都要登记
    let providers = base_urls
        .iter()
        .map(|base_url| ProviderConfig {
            id: format!("local-mock-{}", port_of(base_url)),
            name: "Local Mock".into(),
            api: Api::OpenAICompletions,
            base_url: base_url.clone(),
            env_key: None,
            api_key: Some("test-key".into()),
        })
        .collect();
    save_settings(&Settings {
        theme: Theme::Dark,
        providers,
        default_provider_id: None,
        compaction: Default::default(),
    })
    .expect("写入测试设置");
}

/// 用端口给 provider 取一个稳定的 id。
fn port_of(base_url: &str) -> String {
    base_url
        .trim_end_matches("/v1")
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// 该 Agent 名下打开着的会话 id 集合（排序后，断言好写）。
fn open_session_ids(state: &RuntimeState, agent_name: &str) -> Vec<String> {
    let mut ids: Vec<String> = state
        .session_infos()
        .expect("读取会话列表")
        .into_iter()
        .filter(|info| info.agent_name == agent_name)
        .map(|info| info.session_id)
        .collect();
    ids.sort_unstable();
    ids
}

/// 该 Agent 是否有会话正在跑。
fn any_session_running(state: &RuntimeState, agent_name: &str) -> bool {
    state
        .session_infos()
        .expect("读取会话列表")
        .iter()
        .any(|info| info.agent_name == agent_name && info.running)
}

/// 往该 Agent 的会话目录里预置一条带历史的会话文件，返回它的 id。
fn seed_session(agent_name: &str, prompt: &str) -> String {
    let sessions_dir = agents::load_agent(agent_name)
        .expect("加载 Agent")
        .sessions_dir()
        .expect("会话目录");
    let mut writer = SessionWriter::create(&sessions_dir).expect("创建会话文件");
    writer
        .append_message(&Message::user_text(prompt))
        .expect("写入种子消息");
    writer
        .path()
        .file_stem()
        .and_then(|value| value.to_str())
        .expect("会话 id")
        .to_string()
}

/// 记录事件种类的共享 sink：用来验证事件不串。
type EventLog = Arc<Mutex<Vec<String>>>;

fn recording_sink() -> (EventEmitter, EventLog) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink_log = log.clone();
    let sink: EventEmitter = Arc::new(move |event: RuntimeEvent| {
        let label = match &event {
            RuntimeEvent::AgentEvent(envelope) => {
                let kind = match &envelope.event {
                    pipi_core::agent_loop::AgentEvent::AgentStart => "agent_start",
                    pipi_core::agent_loop::AgentEvent::MessageEnd { .. } => "message_end",
                    pipi_core::agent_loop::AgentEvent::AgentEnd { .. } => "agent_end",
                    _ => "other",
                };
                format!("{}:{}:{}", envelope.agent_name, envelope.session_id, kind)
            }
            RuntimeEvent::SessionStats(envelope) => {
                format!("{}:{}:stats", envelope.agent_name, envelope.session_id)
            }
            RuntimeEvent::SessionError(envelope) => format!(
                "{}:{}:error({})",
                envelope.agent_name, envelope.session_id, envelope.message
            ),
            RuntimeEvent::ApprovalRequest(envelope) => {
                format!("{}:{}:approval", envelope.agent_name, envelope.session_id)
            }
            RuntimeEvent::SessionSwitched(envelope) => format!(
                "{}:{}:switched({})",
                envelope.agent_name, envelope.session_id, envelope.to_session_id
            ),
        };
        sink_log.lock().unwrap().push(label);
    });
    (sink, log)
}

fn wait_until(deadline_ms: u64, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(deadline_ms);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    condition()
}

fn read_session_file(agent_name: &str, session_id: &str) -> String {
    let sessions_dir = agents::load_agent(agent_name)
        .expect("加载 Agent")
        .sessions_dir()
        .expect("会话目录");
    std::fs::read_to_string(sessions_dir.join(format!("{session_id}.jsonl"))).expect("读取会话文件")
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("构建注入运行时")
}

#[test]
fn multiple_agents_run_at_the_same_time_and_stop_independently() {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = temp_home();
    // 三个而不是两个：路数是「几条会话几路」，不是写死的双路
    let agents = ["agent-a", "agent-b", "agent-c"];
    let prompts = ["A 的任务", "B 的任务", "C 的任务"];
    let mut bases = Vec::new();
    for (index, name) in agents.iter().enumerate() {
        let label = ["A", "B", "C"][index];
        let base = format!("http://{}/v1", spawn_stalling_endpoint(label));
        fixture_agent(name, &base);
        bases.push(base);
    }
    write_settings(&bases);

    let runtime = runtime();
    let state = RuntimeState::new(runtime.handle().clone());
    let (sink, events) = recording_sink();

    // 三个依次开跑（各自新建会话）；每一个都不该顶掉前面的
    let mut session_ids = Vec::new();
    for (index, name) in agents.iter().enumerate() {
        state
            .send_prompt(name, None, prompts[index], None, sink.clone())
            .unwrap_or_else(|error| panic!("{name} 应当被接受：{error}"));
        assert!(
            wait_until(5_000, || any_session_running(&state, name)),
            "{name} 应当进入运行中"
        );
        session_ids.push(open_session_ids(&state, name)[0].clone());
        for earlier in &agents[..=index] {
            assert!(
                any_session_running(&state, earlier),
                "{earlier} 不应被后来者顶掉（{name} 刚启动）"
            );
        }
    }

    let infos = state.session_infos().expect("读取会话列表");
    let mut running: Vec<&str> = infos
        .iter()
        .filter(|info| info.running)
        .map(|info| info.agent_name.as_str())
        .collect();
    running.sort_unstable();
    assert_eq!(
        running,
        vec!["agent-a", "agent-b", "agent-c"],
        "三路会话应当同时在跑: {infos:?}"
    );

    // 同一条会话的第二轮仍然被拒（同会话串行是不变量）
    let rejected = state.send_prompt(
        "agent-a",
        Some(&session_ids[0]),
        "再来一轮",
        None,
        sink.clone(),
    );
    assert!(rejected.is_err(), "同一条会话不允许并发两轮");
    let message = rejected.unwrap_err();
    assert!(
        message.contains("正在运行"),
        "拒绝理由应当说明在运行：{message}"
    );

    // stop_run 只停指定那条
    for (index, name) in agents.iter().enumerate() {
        state
            .stop_run(name, &session_ids[index])
            .unwrap_or_else(|error| panic!("停止 {name}: {error}"));
        assert!(
            wait_until(5_000, || !state.session_running(name, &session_ids[index])),
            "{name} 应当停下来"
        );
        for (later_index, later) in agents[index + 1..].iter().enumerate() {
            let later_id = &session_ids[index + 1 + later_index];
            assert!(
                state.session_running(later, later_id),
                "停止 {name} 不应影响 {later}"
            );
        }
    }

    // 事件不串：每条事件都能归属到正确的 (Agent, 会话)
    let labels: Vec<String> = events.lock().unwrap().clone();
    for (index, name) in agents.iter().enumerate() {
        let expected = format!("{name}:{}:agent_start", session_ids[index]);
        assert!(
            labels.iter().any(|label| label == &expected),
            "{name} 的 agent_start 应当带自己的会话 id：{labels:?}"
        );
    }
    for label in &labels {
        let belongs = agents
            .iter()
            .zip(&session_ids)
            .any(|(name, id)| label.starts_with(&format!("{name}:{id}:")));
        assert!(belongs, "事件必须能归属到某条会话：{label}");
    }

    // 各写各的文件：每个 Agent 的会话文件只含自己的输入
    for (index, name) in agents.iter().enumerate() {
        let text = read_session_file(name, &session_ids[index]);
        assert!(
            text.contains(prompts[index]),
            "{name} 的文件应当有自己的输入"
        );
        for (other_index, prompt) in prompts.iter().enumerate() {
            if other_index != index {
                assert!(
                    !text.contains(prompt),
                    "{name} 的文件不应包含其他 Agent 的输入（{prompt}）"
                );
            }
        }
    }

    let _ = std::fs::remove_dir_all(home);
}

/// 同一个 Agent 的两条会话可以同时跑 —— 这是「多对话」的核心：
/// 一条在跑不再是「这个 Agent 被占用」，它的其他会话照常开跑。
#[test]
fn same_agent_runs_two_sessions_in_parallel() {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = temp_home();
    let base = format!("http://{}/v1", spawn_stalling_endpoint("S"));
    fixture_agent("agent-solo", &base);
    write_settings(std::slice::from_ref(&base));

    let runtime = runtime();
    let state = RuntimeState::new(runtime.handle().clone());
    let (sink, events) = recording_sink();

    let first = seed_session("agent-solo", "第一条：历史里的输入");
    let second = seed_session("agent-solo", "第二条：历史里的输入");
    state
        .open_session("agent-solo", &first)
        .expect("打开第一条");
    state
        .open_session("agent-solo", &second)
        .expect("打开第二条");
    let expected_ids = {
        let mut ids = vec![first.clone(), second.clone()];
        ids.sort_unstable();
        ids
    };
    assert_eq!(
        open_session_ids(&state, "agent-solo"),
        expected_ids,
        "同一个 Agent 的多条会话应当能同时打开"
    );

    // 两条同时开跑
    state
        .send_prompt(
            "agent-solo",
            Some(&first),
            "第一条：跑起来",
            None,
            sink.clone(),
        )
        .expect("第一条应当被接受");
    assert!(
        wait_until(5_000, || state.session_running("agent-solo", &first)),
        "第一条应当进入运行中"
    );
    state
        .send_prompt(
            "agent-solo",
            Some(&second),
            "第二条：也跑起来",
            None,
            sink.clone(),
        )
        .expect("同一个 Agent 的第二条会话也应当被接受");
    assert!(
        wait_until(5_000, || state.session_running("agent-solo", &second)),
        "第二条应当进入运行中"
    );

    // 两条都在跑：同一个 Agent 名下两个 running
    let infos = state.session_infos().expect("读取会话列表");
    let running: Vec<&str> = infos
        .iter()
        .filter(|info| info.running)
        .map(|info| info.session_id.as_str())
        .collect();
    assert_eq!(running.len(), 2, "两条会话应当同时在跑: {infos:?}");
    assert!(
        state.session_running("agent-solo", &first) && state.session_running("agent-solo", &second),
        "第一条不应被第二条顶掉"
    );

    // 守卫按会话：同一条的第二轮被拒，另一条不受影响
    let rejected = state.send_prompt("agent-solo", Some(&first), "再来一轮", None, sink.clone());
    assert!(rejected.is_err(), "同一条会话不允许并发两轮");
    assert!(
        state.session_running("agent-solo", &second),
        "拒绝第一条的第二轮不应影响第二条"
    );

    // 停一条不影响另一条
    state.stop_run("agent-solo", &first).expect("停止第一条");
    assert!(
        wait_until(5_000, || !state.session_running("agent-solo", &first)),
        "第一条应当停下来"
    );
    assert!(
        state.session_running("agent-solo", &second),
        "停止第一条不应影响第二条"
    );

    // 各写各的文件
    let first_text = read_session_file("agent-solo", &first);
    let second_text = read_session_file("agent-solo", &second);
    assert!(first_text.contains("第一条：跑起来"), "{first_text}");
    assert!(!first_text.contains("第二条：跑起来"), "{first_text}");
    assert!(second_text.contains("第二条：也跑起来"), "{second_text}");
    assert!(!second_text.contains("第一条：跑起来"), "{second_text}");

    // 事件带会话身份
    let labels: Vec<String> = events.lock().unwrap().clone();
    assert!(
        labels
            .iter()
            .any(|label| label == &format!("agent-solo:{first}:agent_start")),
        "{labels:?}"
    );
    assert!(
        labels
            .iter()
            .any(|label| label == &format!("agent-solo:{second}:agent_start")),
        "{labels:?}"
    );

    state.stop_run("agent-solo", &second).expect("停止第二条");
    assert!(wait_until(5_000, || !any_session_running(
        &state,
        "agent-solo"
    )));

    let _ = std::fs::remove_dir_all(home);
}

/// `new_session` 只释放**空闲**的会话：正在跑的那条留着（后台运行不会被
/// 「离开这个 Agent」清掉）；归档占用检查按 Agent 名判定。
#[test]
fn new_session_releases_idle_sessions_only() {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = temp_home();
    let base = format!("http://{}/v1", spawn_stalling_endpoint("I"));
    fixture_agent("agent-idle", &base);
    fixture_agent("agent-other", &base);
    write_settings(std::slice::from_ref(&base));

    let runtime = runtime();
    let state = RuntimeState::new(runtime.handle().clone());
    let (sink, _events) = recording_sink();

    let running_session = seed_session("agent-idle", "要跑的那条");
    let idle_session = seed_session("agent-idle", "闲着的另一条");
    state
        .open_session("agent-idle", &running_session)
        .expect("打开要跑的");
    state
        .open_session("agent-idle", &idle_session)
        .expect("打开闲着的");

    state
        .send_prompt(
            "agent-idle",
            Some(&running_session),
            "跑起来",
            None,
            sink.clone(),
        )
        .expect("应当被接受");
    assert!(
        wait_until(5_000, || state
            .session_running("agent-idle", &running_session)),
        "应当进入运行中"
    );

    // 释放：空闲的走了，跑着的留着
    state.new_session("agent-idle").expect("释放空闲会话");
    assert_eq!(
        open_session_ids(&state, "agent-idle"),
        vec![running_session.clone()],
        "只应留下正在跑的那条"
    );
    assert!(state.session_running("agent-idle", &running_session));

    // 其他 Agent 的会话不受影响
    state
        .send_prompt("agent-other", None, "别人的任务", None, sink.clone())
        .expect("应当被接受");
    assert!(wait_until(5_000, || any_session_running(
        &state,
        "agent-other"
    )));
    let other_id = open_session_ids(&state, "agent-other")[0].clone();
    state.new_session("agent-idle").expect("再释放一次");
    assert_eq!(
        open_session_ids(&state, "agent-other"),
        vec![other_id.clone()],
        "释放 agent-idle 不应动到别的 Agent"
    );

    // 归档 Agent 的占用检查按 Agent 名：还有会话打开 → 拒绝
    assert!(
        state.archive_agent("agent-idle").is_err(),
        "agent-idle 仍有会话打开，归档应被拒"
    );

    // 跑着的停下来之后，释放就干净了：可以归档
    state
        .stop_run("agent-idle", &running_session)
        .expect("停止");
    assert!(wait_until(5_000, || !any_session_running(
        &state,
        "agent-idle"
    )));
    state.new_session("agent-idle").expect("释放剩余的");
    assert!(
        open_session_ids(&state, "agent-idle").is_empty(),
        "空闲的都应被释放"
    );
    assert!(
        state.archive_agent("agent-idle").is_ok(),
        "已无打开的会话，应当可以归档"
    );
    assert!(
        state.archive_agent("agent-other").is_err(),
        "agent-other 仍在跑，归档应被拒"
    );

    // 收尾：停掉 agent-other，别让测试挂在半路
    state
        .stop_run("agent-other", &other_id)
        .expect("停止 agent-other");
    assert!(wait_until(5_000, || !any_session_running(
        &state,
        "agent-other"
    )));

    let _ = std::fs::remove_dir_all(home);
}

/// 设置工作台的测试会话与正式会话共享运行时协议和并发守卫，但账本只在内存中：
/// 不进入列表、不创建 jsonl，切页式 `new_session` 也不会把它释放。
#[test]
fn temporary_test_session_is_memory_only_and_independent() {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = temp_home();
    let base = format!("http://{}/v1", spawn_stalling_endpoint("T"));
    fixture_agent("agent-test", &base);
    write_settings(std::slice::from_ref(&base));

    let runtime = runtime();
    let state = RuntimeState::new(runtime.handle().clone());
    let (sink, _events) = recording_sink();

    let temporary = state
        .ensure_test_session("agent-test")
        .expect("创建临时测试");
    assert!(temporary.temporary);
    assert!(temporary.session_id.starts_with("test-"));
    assert_eq!(
        state
            .ensure_test_session("agent-test")
            .expect("重复水合临时测试")
            .session_id,
        temporary.session_id,
        "应用运行期内应复用同一个临时测试"
    );
    assert!(
        pipi_core::runtime::list_sessions("agent-test")
            .expect("读取正式会话列表")
            .is_empty(),
        "临时测试不应进入会话列表"
    );

    state
        .send_prompt(
            "agent-test",
            Some(&temporary.session_id),
            "临时测试输入",
            None,
            sink.clone(),
        )
        .expect("临时测试应当启动");
    assert!(wait_until(5_000, || state
        .session_running("agent-test", &temporary.session_id)));

    let definition = agents::load_agent("agent-test").expect("读取 Agent");
    assert!(
        state.save_agent_definition(&definition).is_err(),
        "临时测试运行中不得保存设置"
    );
    assert!(
        pipi_core::runtime::list_sessions("agent-test")
            .expect("读取正式会话列表")
            .is_empty(),
        "运行多轮所用的用户消息也不能创建正式文件"
    );

    // 正式会话与临时测试是两条独立会话，可以同时运行。
    state
        .send_prompt("agent-test", None, "正式会话输入", None, sink.clone())
        .expect("正式会话应当并发启动");
    assert!(wait_until(5_000, || {
        state
            .session_infos()
            .expect("读取打开会话")
            .iter()
            .filter(|info| info.agent_name == "agent-test" && info.running)
            .count()
            == 2
    }));
    let persistent = state
        .session_infos()
        .expect("读取打开会话")
        .into_iter()
        .find(|info| info.agent_name == "agent-test" && !info.temporary)
        .expect("正式会话信息");
    assert_eq!(
        pipi_core::runtime::list_sessions("agent-test")
            .expect("读取正式会话列表")
            .len(),
        1,
        "列表中只能出现正式会话"
    );

    state
        .stop_run("agent-test", &temporary.session_id)
        .expect("停止临时测试");
    assert!(wait_until(5_000, || !state
        .session_running("agent-test", &temporary.session_id)));
    assert!(
        state.session_running("agent-test", &persistent.session_id),
        "停止临时测试不能影响正式会话"
    );
    assert!(
        !runtime
            .block_on(state.session_messages("agent-test", &temporary.session_id))
            .expect("读取临时上下文")
            .is_empty(),
        "停止后仍应能重新水合临时消息"
    );

    let replacement = state
        .reset_test_session("agent-test")
        .expect("重置空闲临时测试");
    assert_ne!(replacement.session_id, temporary.session_id);
    assert!(replacement.temporary);
    assert!(
        runtime
            .block_on(state.session_messages("agent-test", &replacement.session_id))
            .expect("读取新临时上下文")
            .is_empty(),
        "重置后的上下文应为空"
    );

    // 离开普通会话会释放空闲正式会话，但必须保留临时工作台上下文。
    state.new_session("agent-test").expect("释放空闲会话");
    assert!(state
        .session_info("agent-test", &replacement.session_id)
        .expect("读取临时会话")
        .is_some());

    state
        .stop_run("agent-test", &persistent.session_id)
        .expect("停止正式会话");
    assert!(wait_until(5_000, || !state
        .session_running("agent-test", &persistent.session_id)));

    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn temporary_test_keeps_real_tool_side_effects() {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = temp_home();
    let workspace = home.join("tool-workspace");
    std::fs::create_dir_all(&workspace).expect("创建工具工作目录");
    let base = format!("http://{}/v1", spawn_write_endpoint());
    let model = Model {
        id: "tool-model".into(),
        name: "Tool Model".into(),
        api: Api::OpenAICompletions,
        base_url: base.clone(),
        max_tokens: 256,
        context_window: 0,
    };
    agents::create_agent(
        "agent-tool-test",
        "临时测试工具副作用",
        workspace.to_str(),
        None,
        Some("tool-model"),
        Some(model),
    )
    .expect("播种测试 Agent");
    write_settings(std::slice::from_ref(&base));

    let runtime = runtime();
    let state = RuntimeState::new(runtime.handle().clone());
    let (sink, _events) = recording_sink();
    let temporary = state
        .ensure_test_session("agent-tool-test")
        .expect("创建临时测试");
    state
        .send_prompt(
            "agent-tool-test",
            Some(&temporary.session_id),
            "请写文件",
            None,
            sink,
        )
        .expect("启动临时测试");

    let changed_file = workspace.join("temporary-side-effect.txt");
    assert!(
        wait_until(5_000, || changed_file.is_file()
            && !state.session_running("agent-tool-test", &temporary.session_id)),
        "临时测试应实际执行已保存权限允许的 write 工具"
    );
    assert_eq!(
        std::fs::read_to_string(&changed_file).expect("读取工具产物"),
        "this change is real"
    );
    assert!(
        pipi_core::runtime::list_sessions("agent-tool-test")
            .expect("读取正式会话列表")
            .is_empty(),
        "真实工具副作用不应让临时聊天账本落盘"
    );

    let replacement = state
        .reset_test_session("agent-tool-test")
        .expect("清空临时聊天记录");
    assert_ne!(replacement.session_id, temporary.session_id);
    assert!(changed_file.is_file(), "清空聊天记录不能回滚工具副作用");

    let _ = std::fs::remove_dir_all(home);
}
