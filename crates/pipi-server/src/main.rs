//! Pipi Web/远程运行时。
//!
//! 默认只绑定 127.0.0.1，适合由 Tailscale Serve 代理：
//!   cargo run -p pipi-server
//!   tailscale serve 1421
//!
//! 浏览器与桌面端复用同一组 command 名称和事件 payload。服务端不直接
//! 暴露 pipi-core 的内部结构，所有 Agent 执行都经过共享 RuntimeState。

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Json, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use pipi_core::agents::{self, AgentDefinition, PermissionsConfig};

use pipi_core::runtime::{self, EventEmitter, RuntimeEvent, RuntimeState};
use pipi_core::settings::{self, Settings};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tower_http::services::{ServeDir, ServeFile};

const DEFAULT_ADDR: &str = "127.0.0.1:1421";
const PIPI_TOKEN_HEADER: &str = "x-pipi-token";

#[derive(Clone)]
struct AppState {
    runtime: Arc<RuntimeState>,
    events: broadcast::Sender<RuntimeEvent>,
    web_root: PathBuf,
    auth_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InvokeRequest {
    command: String,
    #[serde(default)]
    args: Value,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = env::var("PIPI_SERVER_ADDR")
        .unwrap_or_else(|_| DEFAULT_ADDR.to_string())
        .parse()?;
    let web_root = PathBuf::from(env::var("PIPI_WEB_ROOT").unwrap_or_else(|_| "dist".into()));
    let auth_token = env::var("PIPI_AUTH_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty());
    if auth_token
        .as_deref()
        .is_some_and(|token| token.chars().any(|ch| ch.is_ascii_control() || ch == ';'))
    {
        return Err("PIPI_AUTH_TOKEN 不能包含控制字符或分号".into());
    }

    let (events, _) = broadcast::channel(256);
    let state = AppState {
        runtime: Arc::new(RuntimeState::default()),
        events,
        web_root: web_root.clone(),
        auth_token,
    };

    let index_file = web_root.join("index.html");
    let static_service = ServeDir::new(&web_root).not_found_service(ServeFile::new(index_file));

    let api_routes = Router::new()
        .route("/events", get(ws_events))
        .route("/invoke", post(invoke_handler));

    let protected_routes = Router::new()
        .nest("/api", api_routes)
        .fallback_service(static_service)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    let app = Router::new()
        .route("/api/health", get(|| async { Json(json!({ "ok": true })) }))
        .merge(protected_routes)
        .with_state(state.clone());

    let listener = TcpListener::bind(addr).await?;
    println!(
        "Pipi Web server listening on http://{addr}; web root: {}",
        state.web_root.display()
    );
    if state.auth_token.is_some() {
        println!("Pipi Web server authentication: PIPI_AUTH_TOKEN enabled");
    } else {
        println!("Pipi Web server authentication: disabled (keep the listener on localhost)");
    }

    axum::serve(listener, app).await?;
    Ok(())
}

async fn auth_middleware(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let Some(expected) = state.auth_token.as_deref() else {
        return next.run(req).await;
    };

    let header_token = req
        .headers()
        .get(PIPI_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());
    let bearer_token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);
    let query_token = req.uri().query().and_then(|query| {
        query
            .split('&')
            .find_map(|part| part.strip_prefix("token="))
    });
    let cookie_token = req
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .split(';')
                .find_map(|part| part.trim().strip_prefix("pipi_token="))
        });

    let authorized = [header_token, bearer_token, query_token, cookie_token]
        .into_iter()
        .flatten()
        .any(|candidate| candidate == expected);

    if !authorized {
        let mut response = (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "需要 PIPI_AUTH_TOKEN" })),
        )
            .into_response();
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return response;
    }

    let is_query_auth = query_token == Some(expected);
    let mut response = next.run(req).await;
    if is_query_auth {
        if let Ok(cookie_val) = HeaderValue::from_str(&format!(
            "pipi_token={expected}; HttpOnly; Path=/; SameSite=Lax"
        )) {
            response.headers_mut().insert(header::SET_COOKIE, cookie_val);
        }
    }
    response
}

async fn ws_events(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| stream_events(socket, state.events.subscribe()))
}

async fn stream_events(mut socket: WebSocket, mut receiver: broadcast::Receiver<RuntimeEvent>) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if socket.send(WsMessage::Ping(Default::default())).await.is_err() {
                    break;
                }
            }
            received = receiver.recv() => {
                match received {
                    Ok(event) => {
                        let text = match serde_json::to_string(&event) {
                            Ok(text) => text,
                            Err(_) => continue,
                        };
                        if socket.send(WsMessage::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("pipi-server: websocket receiver lagged by {n} events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

async fn invoke_handler(
    State(state): State<AppState>,
    Json(request): Json<InvokeRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    match invoke_command(request, &state).await {
        Ok(value) => Ok(Json(value)),
        Err(err) => Err((StatusCode::BAD_REQUEST, Json(json!({ "error": err })))),
    }
}

async fn invoke_command(request: InvokeRequest, state: &AppState) -> Result<Value, String> {
    let command = request.command.trim();
    let args = &request.args;
    match command {
        "list_agents" => to_value(agents::list_agents()?),
        "create_agent" => {
            let name = required_string(args, "name")?;
            let description = required_string(args, "description")?;
            let workspace = optional_string(args, "workspace")?;
            let permissions = optional_value::<PermissionsConfig>(args, "permissions")?;
            to_value(agents::create_agent(
                &name,
                &description,
                workspace.as_deref(),
                permissions,
            )?)
        }
        "load_agent" => {
            let name = required_string(args, "name")?;
            to_value(agents::load_agent(&name)?)
        }
        "save_agent" => {
            let definition: AgentDefinition = required_value(args, "def")?;
            agents::save_agent(&definition)?;
            Ok(Value::Null)
        }
        "get_settings" => to_value(settings::load_settings()),
        "save_settings" => {
            let settings: Settings = required_value(args, "settings")?;
            settings::save_settings(&settings)?;
            Ok(Value::Null)
        }
        "list_sessions" => {
            let agent_name = required_string(args, "agentName")?;
            to_value(runtime::list_sessions(&agent_name)?)
        }
        "open_session" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            state.runtime.open_session(&agent_name, &session_id)?;
            Ok(Value::Null)
        }
        "fork_session" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            let up_to_entry_id = optional_string(args, "upToEntryId")?;
            to_value(state.runtime.fork_session(
                &agent_name,
                &session_id,
                up_to_entry_id.as_deref(),
            )?)
        }
        "session_info" => to_value(state.runtime.session_info()?),
        "session_running" => to_value(state.runtime.session_running()),
        "stop_run" => {
            state.runtime.stop_run()?;
            Ok(Value::Null)
        }
        "new_session" => {
            state.runtime.new_session()?;
            Ok(Value::Null)
        }
        "send_prompt" => {
            let agent_name = required_string(args, "agentName")?;
            let prompt = required_string(args, "prompt")?;
            let events = state.events.clone();
            let emitter: EventEmitter = Arc::new(move |event| {
                let _ = events.send(event);
            });
            state.runtime.send_prompt(&agent_name, &prompt, emitter)?;
            Ok(Value::Null)
        }
        "session_messages" => to_value(state.runtime.session_messages().await?),
        "session_stats" => to_value(state.runtime.session_stats()?),
        _ => Err(format!("未知命令: {command}")),
    }
}

fn to_value<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

fn required_string(args: &Value, name: &str) -> Result<String, String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("缺少字符串参数: {name}"))
}

fn optional_string(args: &Value, name: &str) -> Result<Option<String>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| format!("参数 {name} 必须是字符串或 null")),
    }
}

fn required_value<T: DeserializeOwned>(args: &Value, name: &str) -> Result<T, String> {
    let value = args.get(name).ok_or_else(|| format!("缺少参数: {name}"))?;
    serde_json::from_value(value.clone()).map_err(|error| format!("参数 {name} 无效: {error}"))
}

fn optional_value<T: DeserializeOwned>(args: &Value, name: &str) -> Result<Option<T>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| format!("参数 {name} 无效: {error}")),
    }
}
