//! Pipi Web/远程运行时。
//!
//! 默认只绑定 127.0.0.1，适合由 Tailscale Serve 代理：
//!   cargo run -p pipi-server
//!   tailscale serve 1421
//!
//! 浏览器与桌面端复用同一组 command 名称和事件 payload。服务端不直接
//! 暴露 pipi-core 的内部结构，所有 Agent 执行都经过共享 RuntimeState。

use std::convert::Infallible;
use std::env;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{
    self, HeaderName, HeaderValue, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, SET_COOKIE,
    WWW_AUTHENTICATE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use pipi_core::agents::{self, AgentDefinition, PermissionsConfig};
use pipi_core::runtime::{self, EventEmitter, RuntimeEvent, RuntimeState};
use pipi_core::settings::{self, Settings};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::{
    handshake::derive_accept_key,
    protocol::{Message as WsMessage, Role},
};
use tokio_tungstenite::WebSocketStream;

const DEFAULT_ADDR: &str = "127.0.0.1:1421";
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const PIPI_TOKEN_HEADER: HeaderName = HeaderName::from_static("x-pipi-token");

type HttpResponse = Response<Full<Bytes>>;

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
        web_root,
        auth_token,
    };
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

    loop {
        let (stream, peer) = listener.accept().await?;
        let connection_state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = serve_connection(stream, connection_state).await {
                eprintln!("pipi-server: connection {peer} failed: {error}");
            }
        });
    }
}

async fn serve_connection(
    stream: TcpStream,
    state: AppState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let io = TokioIo::new(stream);
    let service = service_fn(move |request| handle_request(request, state.clone()));
    http1::Builder::new()
        .serve_connection(io, service)
        .with_upgrades()
        .await?;
    Ok(())
}

async fn handle_request(
    request: Request<Incoming>,
    state: AppState,
) -> Result<HttpResponse, Infallible> {
    Ok(route_request(request, state).await)
}

async fn route_request(request: Request<Incoming>, state: AppState) -> HttpResponse {
    let path = request.uri().path();

    if request.method() == Method::OPTIONS && path.starts_with("/api/") {
        return empty_response(StatusCode::NO_CONTENT);
    }

    if path == "/api/health" {
        return if request.method() == Method::GET {
            json_response(StatusCode::OK, &json!({ "ok": true }))
        } else {
            error_response(StatusCode::METHOD_NOT_ALLOWED, "只支持 GET")
        };
    }

    if path == "/api/events" {
        if request.method() != Method::GET {
            return error_response(StatusCode::METHOD_NOT_ALLOWED, "只支持 GET");
        }
        if !authorized(&request, &state) {
            return unauthorized_response();
        }
        return websocket_upgrade(request, state);
    }

    if path == "/api/invoke" {
        if request.method() != Method::POST {
            return error_response(StatusCode::METHOD_NOT_ALLOWED, "只支持 POST");
        }
        if !authorized(&request, &state) {
            return unauthorized_response();
        }
        return invoke_response(request, state).await;
    }

    if path.starts_with("/api/") {
        return error_response(StatusCode::NOT_FOUND, "未知 API");
    }

    if request.method() != Method::GET && request.method() != Method::HEAD {
        return error_response(StatusCode::METHOD_NOT_ALLOWED, "只支持 GET");
    }
    if !authorized(&request, &state) {
        return unauthorized_response();
    }
    static_response(request, state).await
}

async fn invoke_response(request: Request<Incoming>, state: AppState) -> HttpResponse {
    let body = match Limited::new(request.into_body(), MAX_BODY_BYTES)
        .collect()
        .await
    {
        Ok(body) => body.to_bytes(),
        Err(_) => return error_response(StatusCode::PAYLOAD_TOO_LARGE, "请求体过大"),
    };
    let request: InvokeRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("请求 JSON 无效: {error}"))
        }
    };
    match invoke_command(request, &state).await {
        Ok(value) => json_response(StatusCode::OK, &value),
        Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
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

fn json_response<T: Serialize>(status: StatusCode, value: &T) -> HttpResponse {
    let body = serde_json::to_vec(value)
        .unwrap_or_else(|_| br#"{"error":"serialization failed"}"#.to_vec());
    response_with_body(status, "application/json; charset=utf-8", Bytes::from(body))
}

fn error_response(status: StatusCode, message: &str) -> HttpResponse {
    json_response(status, &json!({ "error": message }))
}

fn empty_response(status: StatusCode) -> HttpResponse {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .expect("valid empty response")
}

fn response_with_body(status: StatusCode, content_type: &str, body: Bytes) -> HttpResponse {
    let content_length = body.len().to_string();
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_LENGTH, content_length)
        .body(Full::new(body))
        .expect("valid HTTP response")
}

fn unauthorized_response() -> HttpResponse {
    let mut response = error_response(StatusCode::UNAUTHORIZED, "需要 PIPI_AUTH_TOKEN");
    response
        .headers_mut()
        .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

fn authorized(request: &Request<Incoming>, state: &AppState) -> bool {
    let Some(expected) = state.auth_token.as_deref() else {
        return true;
    };
    let header_token = request
        .headers()
        .get(PIPI_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());
    let bearer_token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);
    let query_token = request.uri().query().and_then(|query| {
        query
            .split('&')
            .find_map(|part| part.strip_prefix("token="))
    });
    let cookie_token = request
        .headers()
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .split(';')
                .find_map(|part| part.trim().strip_prefix("pipi_token="))
        });
    [header_token, bearer_token, query_token, cookie_token]
        .into_iter()
        .flatten()
        .any(|candidate| candidate == expected)
}

fn websocket_upgrade(request: Request<Incoming>, state: AppState) -> HttpResponse {
    let key = request
        .headers()
        .get(header::SEC_WEBSOCKET_KEY)
        .and_then(|value| value.to_str().ok())
        .expect("websocket key checked before upgrade");
    let accept_key = derive_accept_key(key.as_bytes());
    let receiver = state.events.subscribe();
    let upgrade = hyper::upgrade::on(request);
    tokio::spawn(async move {
        match upgrade.await {
            Ok(upgraded) => {
                let socket =
                    WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None)
                        .await;
                stream_events(socket, receiver).await;
            }
            Err(error) => eprintln!("pipi-server: websocket upgrade failed: {error}"),
        }
    });
    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(header::CONNECTION, "Upgrade")
        .header(header::UPGRADE, "websocket")
        .header(header::SEC_WEBSOCKET_ACCEPT, accept_key)
        .body(Full::new(Bytes::new()))
        .expect("valid websocket response")
}

async fn stream_events<S>(
    mut socket: WebSocketStream<S>,
    mut receiver: broadcast::Receiver<RuntimeEvent>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        tokio::select! {
            event = receiver.recv() => {
                let event = match event {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let text = match serde_json::to_string(&event) {
                    Ok(text) => text,
                    Err(error) => {
                        eprintln!("pipi-server: serialize websocket event failed: {error}");
                        continue;
                    }
                };
                if socket.send(WsMessage::Text(text.into())).await.is_err() {
                    break;
                }
            }
            message = socket.next() => {
                match message {
                    Some(Ok(WsMessage::Close(frame))) => {
                        let _ = socket.close(frame).await;
                        break;
                    }
                    None | Some(Err(_)) => break,
                    Some(Ok(WsMessage::Ping(payload))) => {
                        if socket.send(WsMessage::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Text(_)))
                    | Some(Ok(WsMessage::Binary(_)))
                    | Some(Ok(WsMessage::Pong(_)))
                    | Some(Ok(WsMessage::Frame(_))) => {}
                }
            }
        }
    }
}

async fn static_response(request: Request<Incoming>, state: AppState) -> HttpResponse {
    let requested_path = request.uri().path();
    let Some(relative) = safe_relative_path(requested_path) else {
        return error_response(StatusCode::BAD_REQUEST, "非法路径");
    };
    let index = state.web_root.join("index.html");
    let candidate = if relative.as_os_str().is_empty() {
        index.clone()
    } else {
        state.web_root.join(&relative)
    };
    let (path, body) = match tokio::fs::read(&candidate).await {
        Ok(body) => (candidate, body),
        Err(_)
            if !requested_path
                .rsplit('/')
                .next()
                .unwrap_or("")
                .contains('.') =>
        {
            match tokio::fs::read(&index).await {
                Ok(body) => (index, body),
                Err(error) => {
                    return error_response(
                        StatusCode::NOT_FOUND,
                        &format!("找不到 Web 构建产物: {error}"),
                    )
                }
            }
        }
        Err(_) => return error_response(StatusCode::NOT_FOUND, "文件不存在"),
    };
    let is_head = request.method() == Method::HEAD;
    let body_bytes = Bytes::from(body);
    let content_length = body_bytes.len().to_string();
    let response_body = if is_head { Bytes::new() } else { body_bytes };
    let mut response = response_with_body(StatusCode::OK, mime_type(&path), response_body);
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&content_length).expect("content length is valid"),
    );
    if requested_path.starts_with("/assets/") {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    set_auth_cookie(&mut response, &request, &state);
    response
}

fn safe_relative_path(path: &str) -> Option<PathBuf> {
    let path = path.strip_prefix('/')?;
    let mut relative = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(value) if !value.to_string_lossy().contains('\\') => {
                relative.push(value)
            }
            Component::CurDir => {}
            Component::RootDir | Component::ParentDir | Component::Prefix(_) => return None,
            Component::Normal(_) => return None,
        }
    }
    Some(relative)
}

fn mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn set_auth_cookie(response: &mut HttpResponse, request: &Request<Incoming>, state: &AppState) {
    let Some(expected) = state.auth_token.as_deref() else {
        return;
    };
    let Some(query) = request.uri().query() else {
        return;
    };
    let Some(token) = query
        .split('&')
        .find_map(|part| part.strip_prefix("token="))
    else {
        return;
    };
    if token != expected {
        return;
    }
    if let Ok(value) = HeaderValue::from_str(&format!(
        "pipi_token={expected}; Path=/; HttpOnly; SameSite=Strict"
    )) {
        response.headers_mut().insert(SET_COOKIE, value);
    }
}

#[cfg(test)]
mod tests {
    use super::{mime_type, safe_relative_path};

    #[test]
    fn rejects_paths_that_escape_web_root() {
        assert!(safe_relative_path("/assets/app.js").is_some());
        assert!(safe_relative_path("/../Cargo.toml").is_none());
        assert!(safe_relative_path("/assets/../../Cargo.toml").is_none());
        assert!(safe_relative_path("/assets\\\\..\\\\Cargo.toml").is_none());
    }

    #[test]
    fn infers_common_web_content_types() {
        assert_eq!(
            mime_type(std::path::Path::new("index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            mime_type(std::path::Path::new("app.js")),
            "text/javascript; charset=utf-8"
        );
    }
}
