#![cfg(feature = "server")]
//! 真实回环 HTTP/WS 与单个运行时的黑盒回归，不调用外部模型。
use pipi_core::{
    commands,
    events::{EventEmitter, RuntimeEvent},
    remote::{CommandSource, RemoteAccess, RemoteAccessConfig, RemoteAccessControl},
    runtime::RuntimeState,
    server::{self, WebAsset, WebAssets},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

struct Home {
    path: std::path::PathBuf,
    previous: Option<std::ffi::OsString>,
}
impl Home {
    fn new() -> Self {
        let path = pipi_test_support::temp_dir("pipi-remote");
        let previous = std::env::var_os("PIPI_HOME");
        pipi_test_support::set_env_var("PIPI_HOME", &path);
        Self { path, previous }
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        if let Some(value) = &self.previous {
            pipi_test_support::set_env_var("PIPI_HOME", value);
        } else {
            pipi_test_support::remove_env_var("PIPI_HOME");
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
fn assets() -> WebAssets {
    WebAssets::Embedded(Arc::new(|path| {
        (path == "index.html").then(|| WebAsset {
            bytes: b"<html>embedded Pipi</html>".to_vec(),
            content_type: "text/html".into(),
        })
    }))
}

async fn http(
    addr: std::net::SocketAddr,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
    origin: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let data = body.as_ref().map(Value::to_string).unwrap_or_default();
    let request = format!(
        "{} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}{}\r\n{data}",
        if body.is_some() { "POST" } else { "GET" },
        data.len(),
        token
            .map(|t| format!("X-Pipi-Token: {t}\r\n"))
            .unwrap_or_default(),
        origin
            .map(|o| format!("Origin: {o}\r\n"))
            .unwrap_or_default()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut result = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut result))
        .await
        .unwrap()
        .unwrap();
    let response = String::from_utf8(result).unwrap();
    (
        response.split_whitespace().nth(1).unwrap().parse().unwrap(),
        response.split_once("\r\n\r\n").unwrap().1.to_string(),
    )
}
async fn invoke(
    addr: std::net::SocketAddr,
    token: &str,
    command: &str,
    args: Value,
) -> (u16, Value) {
    let (code, body) = http(
        addr,
        "/api/invoke",
        Some(token),
        Some(json!({"command":command,"args":args})),
        None,
    )
    .await;
    (code, serde_json::from_str(&body).unwrap())
}
async fn websocket(addr: std::net::SocketAddr, token: &str) -> TcpStream {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(format!("GET /api/events HTTP/1.1\r\nHost: {addr}\r\nX-Pipi-Token: {token}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").as_bytes()).await.unwrap();
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        headers.push(stream.read_u8().await.unwrap());
    }
    assert!(
        String::from_utf8(headers)
            .unwrap()
            .starts_with("HTTP/1.1 101")
    );
    stream
}
async fn frame(stream: &mut TcpStream) -> Option<Value> {
    loop {
        let mut head = [0u8; 2];
        if stream.read_exact(&mut head).await.is_err() {
            return None;
        }
        let mut size = (head[1] & 127) as usize;
        if size == 126 {
            size = stream.read_u16().await.unwrap() as usize;
        }
        if size == 127 {
            size = stream.read_u64().await.unwrap() as usize;
        }
        assert!(size < 1024 * 1024);
        let mut bytes = vec![0; size];
        stream.read_exact(&mut bytes).await.unwrap();
        match head[0] & 15 {
            1 => return Some(serde_json::from_slice(&bytes).unwrap()),
            8 => return None,
            9 => {
                let _ = stream.write_all(&[0x8a, 0x80, 0, 0, 0, 0]).await;
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn management_is_local_only_rotation_disconnects_clients_and_conflicts_preserve_service() {
    let _guard = pipi_test_support::HOME_LOCK.lock().unwrap();
    let home = Home::new();
    let runtime = Arc::new(RuntimeState::new(tokio::runtime::Handle::current()));
    let remote = RemoteAccess::new(&runtime, assets());
    runtime.attach_remote_access(remote.clone()).unwrap();
    assert!(!remote.status().await.local_running);
    let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    remote
        .configure(RemoteAccessConfig {
            port,
            ..Default::default()
        })
        .await
        .unwrap();
    let local = remote.set_enabled(true).await.unwrap();
    assert!(local.local_running);
    assert!(!local.tunnel_connected, "本地监听不代表公网 Tunnel 已连接");
    assert!(!local.tunnel_running);
    let addr = ([127, 0, 0, 1], port).into();
    let token = remote.token(false).await.unwrap();
    assert_eq!(token.len(), 64);
    assert_eq!(http(addr, "/", None, None, None).await.0, 200);
    assert_eq!(
        http(
            addr,
            "/api/invoke",
            None,
            Some(json!({"command":"get_settings"})),
            None
        )
        .await
        .0,
        401
    );
    assert_eq!(
        http(
            addr,
            "/api/health",
            None,
            None,
            Some("https://evil.example")
        )
        .await
        .0,
        403
    );
    assert_eq!(
        http(
            addr,
            "/api/health",
            None,
            None,
            Some(&format!("https://{addr}"))
        )
        .await
        .0,
        403
    );
    assert_eq!(
        http(
            addr,
            "/api/health",
            None,
            None,
            Some(&format!("http://{addr}"))
        )
        .await
        .0,
        200
    );
    for (command, args) in [
        ("remote_access_status", json!({})),
        (
            "configure_remote_access",
            json!({"config":{"port":1,"startWithApp":true,"publicUrl":""}}),
        ),
        ("set_remote_access_enabled", json!({"enabled":false})),
        ("remote_access_token", json!({"rotate":true})),
    ] {
        let (code, body) = invoke(addr, &token, command, args).await;
        assert_eq!(code, 400);
        assert!(
            body["error"].as_str().unwrap().contains("only allowed")
                || body["error"].as_str().unwrap().contains("仅允许")
        );
    }
    let blocker = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let blocked_port = blocker.local_addr().unwrap().port();
    assert!(
        remote
            .configure(RemoteAccessConfig {
                port: blocked_port,
                ..Default::default()
            })
            .await
            .is_err()
    );
    assert_eq!(remote.status().await.config.port, port);
    assert_eq!(
        invoke(addr, &token, "session_infos", json!({})).await.0,
        200
    );
    let mut ws = websocket(addr, &token).await;
    let next = remote.token(true).await.unwrap();
    assert_ne!(next, token);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), frame(&mut ws))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        invoke(addr, &token, "session_infos", json!({})).await.0,
        401
    );
    assert_eq!(invoke(addr, &next, "session_infos", json!({})).await.0, 200);
    assert!(
        !std::fs::read_to_string(home.path.join("remote.json"))
            .unwrap()
            .contains(&next)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(home.path.join("remote-token"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    remote.set_enabled(false).await.unwrap();
    assert!(TcpStream::connect(addr).await.is_err());
    assert!(!remote.status().await.local_running);
    // 只填域名不会建立公网连接，失败必须回收已启动的本机服务。
    remote
        .configure(RemoteAccessConfig {
            port,
            public_url: "https://pipi.example.com".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        remote.set_enabled(true).await.unwrap_err().contains("credentials")
            || remote.set_enabled(true).await.unwrap_err().contains("凭据")
    );
    assert!(!remote.status().await.local_running);
    assert!(TcpStream::connect(addr).await.is_err());
    // 无效配置不自动监听；显式保存可修复。
    std::fs::write(home.path.join("remote.json"), "{broken").unwrap();
    let broken = RemoteAccess::new(&runtime, assets());
    broken.restore().await;
    assert!(!broken.status().await.local_running);
    assert!(broken.status().await.last_error.is_some());
    assert!(broken.set_enabled(true).await.is_err());
    assert!(
        commands::remote_access_token(
            &runtime,
            CommandSource::Web,
            commands::RemoteAccessTokenArgs { rotate: false }
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn web_and_desktop_share_runs_events_and_stop_while_transport_can_restart() {
    let _guard = pipi_test_support::HOME_LOCK.lock().unwrap();
    let home = Home::new();
    let workspace = home.path.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let mock = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", mock.local_addr().unwrap());
    let endpoint = tokio::spawn(async move {
        let (mut stream, _) = mock.accept().await.unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(stream.read_u8().await.unwrap());
        }
        let length = String::from_utf8(headers)
            .unwrap()
            .lines()
            .find_map(|s| {
                s.to_lowercase()
                    .strip_prefix("content-length:")
                    .map(|s| s.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap();
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n").await.unwrap();
        std::future::pending::<()>().await;
    });
    let model = pipi_protocol::Model {
        id: "embedded-mock".into(),
        name: "Mock".into(),
        api: pipi_protocol::Api::OpenAICompletions,
        base_url: base_url.clone(),
        max_tokens: 128,
        context_window: 0,
    };
    pipi_venv::agents::create_agent(
        "shared",
        "",
        workspace.to_str(),
        None,
        Some(&model.id),
        Some(model.clone()),
    )
    .unwrap();
    let mut settings = pipi_core::settings::Settings::default();
    settings
        .providers
        .push(pipi_core::settings::ProviderConfig {
            id: "mock".into(),
            name: "Mock".into(),
            api: model.api,
            api_mode: Default::default(),
            base_url,
            env_key: None,
            api_key: Some("fixture".into()),
        });
    pipi_core::settings::save_settings(&settings).unwrap();
    let runtime = Arc::new(RuntimeState::new(tokio::runtime::Handle::current()));
    let desktop_events = Arc::new(Mutex::new(Vec::<RuntimeEvent>::new()));
    let recorded = desktop_events.clone();
    let sink: EventEmitter = Arc::new(move |event| recorded.lock().unwrap().push(event));
    runtime.events().set_desktop_sink(sink);
    let token = "fixture-auth-token";
    let server = server::start(
        runtime.clone(),
        "127.0.0.1:0".parse().unwrap(),
        Some(token.into()),
        assets(),
    )
    .await
    .unwrap();
    let mut ws = websocket(server.address(), token).await;
    let (code, info) = invoke(
        server.address(),
        token,
        "send_prompt",
        json!({"agentName":"shared","prompt":"fixture"}),
    )
    .await;
    assert_eq!(code, 200, "{info}");
    let id = info["sessionId"].as_str().unwrap();
    assert!(runtime.session_info("shared", id).unwrap().unwrap().running);
    let event = tokio::time::timeout(Duration::from_secs(3), frame(&mut ws))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event["payload"]["sessionId"], id);
    assert!(!desktop_events.lock().unwrap().is_empty());
    assert_eq!(
        invoke(
            server.address(),
            token,
            "send_prompt",
            json!({"agentName":"shared","sessionId":id,"prompt":"duplicate"})
        )
        .await
        .0,
        400
    );
    server.stop().await.unwrap();
    assert!(
        runtime.session_info("shared", id).unwrap().unwrap().running,
        "停止 transport 不能停止运行"
    );
    let server = server::start(
        runtime.clone(),
        "127.0.0.1:0".parse().unwrap(),
        Some(token.into()),
        assets(),
    )
    .await
    .unwrap();
    let (code, infos) = invoke(server.address(), token, "session_infos", json!({})).await;
    assert_eq!(code, 200);
    assert_eq!(infos[0]["sessionId"], id);
    // 桌面入口停止 Web 发起的同一轮。
    commands::stop_run(
        &runtime,
        commands::StopRunArgs {
            agent_name: "shared".into(),
            session_id: id.into(),
        },
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while runtime.session_info("shared", id).unwrap().unwrap().running {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(desktop_events.lock().unwrap().iter().any(|event| matches!(event, RuntimeEvent::AgentEvent(envelope) if matches!(envelope.event, pipi_agent::agent_loop::AgentEvent::AgentEnd { .. }))));
    server.stop().await.unwrap();
    endpoint.abort();
}
