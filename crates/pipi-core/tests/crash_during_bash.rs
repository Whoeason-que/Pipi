//! A real host process dies while Bash is running, then the same session is
//! opened and used again. All data and the mock model are local to this test.
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pipi_core::events::{EventEmitter, RuntimeEvent};
use pipi_core::runtime::RuntimeState;
use pipi_core::settings::{ApiMode, ProviderConfig, Settings, Theme, save_settings};
use pipi_protocol::permissions::SandboxMode;
use pipi_protocol::{Api, Message, Model, StopReason};
use pipi_venv::agents;

const AGENT: &str = "crash-bash-fixture";

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

fn read_request(stream: &mut std::net::TcpStream) {
    let mut headers = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).unwrap_or(0) == 1 {
        headers.push(byte[0]);
        if headers.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let headers = String::from_utf8_lossy(&headers).to_ascii_lowercase();
    let len = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    stream.read_exact(&mut body).unwrap();
}

fn start_model() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            read_request(&mut stream);
            let start = serde_json::json!({"id":"mock","object":"chat.completion.chunk","model":"mock","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call-crash-bash","type":"function","function":{"name":"bash","arguments":serde_json::json!({"command":"printf 'command-started\\n'; /bin/sleep 8"}).to_string()}}]},"finish_reason":null}]});
            let end = serde_json::json!({"id":"mock","object":"chat.completion.chunk","model":"mock","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]});
            let body = format!("data: {start}\n\ndata: {end}\n\ndata: [DONE]\n\n");
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    format!("http://{addr}/v1")
}

#[test]
fn crash_host_child() {
    let Ok(session_id) = std::env::var("PIPI_CRASH_TEST_SESSION") else {
        return;
    };
    let ready = PathBuf::from(std::env::var_os("PIPI_CRASH_TEST_READY").unwrap());
    let prompt = std::env::var("PIPI_CRASH_TEST_PROMPT").unwrap();
    let runtime = runtime();
    let host = RuntimeState::new(runtime.handle().clone());
    let sink: EventEmitter = Arc::new(move |event| {
        if matches!(event, RuntimeEvent::AgentEvent(ref envelope)
            if matches!(envelope.event, pipi_agent::agent_loop::AgentEvent::ToolExecutionUpdate { .. }))
        {
            std::fs::write(&ready, b"tool-running").unwrap();
        }
    });
    host.send_prompt(AGENT, Some(&session_id), &prompt, None, sink)
        .unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn killed_during_bash_recovers_twice_in_the_host() {
    let _guard = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let home = pipi_test_support::temp_dir("pipi-real-bash-crash");
    pipi_test_support::set_env_var("HOME", &home);
    pipi_test_support::set_env_var("PIPI_HOME", home.join("pipi"));
    pipi_test_support::set_env_var("AV_HOME", home.join("av"));
    let base = start_model();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let model = Model {
        id: "mock".into(),
        name: "Mock".into(),
        api: Api::OpenAICompletions,
        base_url: base.clone(),
        max_tokens: 256,
        context_window: 8192,
        reasoning_effort: None,
    };
    agents::create_agent(
        AGENT,
        "crash fixture",
        workspace.to_str(),
        None,
        Some("mock"),
        Some(model),
    )
    .unwrap();
    let mut def = agents::load_agent(AGENT).unwrap();
    def.permissions.sandbox = SandboxMode::DangerFullAccess;
    agents::save_agent(&def).unwrap();
    save_settings(&Settings {
        notifications: Default::default(),
        theme: Theme::Dark,
        providers: vec![ProviderConfig {
            id: "mock".into(),
            name: "Mock".into(),
            api: Api::OpenAICompletions,
            api_mode: ApiMode::Auto,
            base_url: base,
            env_key: None,
            api_key: Some("test".into()),
        }],
        default_provider_id: None,
        compaction: Default::default(),
        retry: Default::default(),
    })
    .unwrap();
    let rt = runtime();
    let host = RuntimeState::new(rt.handle().clone());
    let session_id = host.create_session(AGENT).unwrap().session_id;
    drop(host);

    for (round, prompt) in [
        (1, "first interrupted turn"),
        (2, "second interrupted turn"),
    ] {
        let ready = home.join(format!("ready-{round}"));
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_host_child", "--nocapture"])
            .env("PIPI_CRASH_TEST_SESSION", &session_id)
            .env("PIPI_CRASH_TEST_READY", &ready)
            .env("PIPI_CRASH_TEST_PROMPT", prompt)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !ready.exists() && Instant::now() < deadline {
            assert!(
                child.try_wait().unwrap().is_none(),
                "child exited before bash produced output"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(ready.exists(), "bash did not produce output before timeout");
        child.kill().unwrap();
        child.wait().unwrap();
        let restarted = RuntimeState::new(rt.handle().clone());
        let snapshot = restarted.open_session(AGENT, &session_id).unwrap();
        let json = serde_json::to_string(&snapshot.messages).unwrap();
        assert!(json.contains(prompt), "missing interrupted user prompt");
        assert!(json.contains("call-crash-bash"), "missing tool call");
        assert!(snapshot.messages.iter().any(|m| matches!(m, Message::ToolResult { tool_call_id, is_error:true, .. } if tool_call_id == "call-crash-bash")), "missing unknown tool result");
        assert!(
            matches!(
                snapshot.messages.last(),
                Some(Message::Assistant {
                    stop_reason: StopReason::Aborted,
                    ..
                })
            ),
            "missing interruption marker"
        );
        if round == 2 {
            assert!(
                json.contains("first interrupted turn"),
                "lost first recovery"
            );
        }
        println!(
            "round {round}: recovered {} host messages",
            snapshot.messages.len()
        );
    }
    std::fs::remove_dir_all(home).ok();
}
