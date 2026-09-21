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
use pipi_core::approval::ApprovalDecision;
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

#[derive(Debug, Deserialize)]
struct LoginRequest {
    token: String,
}

fn check_request_authorized(headers: &header::HeaderMap, uri: &axum::http::Uri, expected: &str) -> bool {
    let header_token = headers
        .get(PIPI_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());
    let bearer_token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);
    let query_token = uri.query().and_then(|query| {
        query
            .split('&')
            .find_map(|part| part.strip_prefix("token="))
    });
    let cookie_token = headers
        .get(header::COOKIE)
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

fn build_app(state: AppState) -> Router {
    let index_file = state.web_root.join("index.html");
    let static_service = ServeDir::new(&state.web_root).not_found_service(ServeFile::new(index_file));

    let protected_api_routes = Router::new()
        .route("/events", get(ws_events))
        .route("/invoke", post(invoke_handler))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    let public_api_routes = Router::new()
        .route("/health", get(|| async { Json(json!({ "ok": true })) }))
        .route("/auth/status", get(auth_status_handler))
        .route("/auth/login", post(auth_login_handler))
        .route("/auth/logout", post(auth_logout_handler));

    let api_routes = Router::new()
        .merge(protected_api_routes)
        .merge(public_api_routes);

    let cors = tower_http::cors::CorsLayer::new()
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers(tower_http::cors::AllowHeaders::mirror_request())
        .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
        .allow_credentials(true);

    Router::new()
        .nest("/api", api_routes)
        .fallback_service(static_service)
        .layer(cors)
        .with_state(state)
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
        runtime: Arc::new(RuntimeState::new(tokio::runtime::Handle::current())),
        events,
        web_root: web_root.clone(),
        auth_token,
    };

    let app = build_app(state.clone());

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

async fn auth_status_handler(
    State(state): State<AppState>,
    req: axum::extract::Request,
) -> Response {
    if let Some(expected) = state.auth_token.as_deref() {
        let authorized = check_request_authorized(req.headers(), req.uri(), expected);
        Json(json!({
            "authRequired": true,
            "authenticated": authorized
        }))
        .into_response()
    } else {
        Json(json!({
            "authRequired": false,
            "authenticated": true
        }))
        .into_response()
    }
}

fn is_https_request(headers: &header::HeaderMap) -> bool {
    if let Some(proto) = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) {
        if proto.eq_ignore_ascii_case("https") {
            return true;
        }
    }
    if let Some(cf) = headers.get("cf-visitor").and_then(|v| v.to_str().ok()) {
        if cf.contains("https") {
            return true;
        }
    }
    false
}

async fn auth_login_handler(
    State(state): State<AppState>,
    headers: header::HeaderMap,
    Json(payload): Json<LoginRequest>,
) -> Response {
    if let Some(expected) = state.auth_token.as_deref() {
        if payload.token.trim() == expected {
            let mut response = Json(json!({ "ok": true })).into_response();
            let secure = if is_https_request(&headers) { "; Secure" } else { "" };
            if let Ok(cookie_val) = HeaderValue::from_str(&format!(
                "pipi_token={expected}; HttpOnly; Path=/; SameSite=Lax{secure}"
            )) {
                response.headers_mut().insert(header::SET_COOKIE, cookie_val);
            }
            response
        } else {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "ok": false, "error": "Token 错误，请核对后重试" })),
            )
                .into_response()
        }
    } else {
        Json(json!({ "ok": true })).into_response()
    }
}

async fn auth_logout_handler(headers: header::HeaderMap) -> Response {
    let mut response = Json(json!({ "ok": true })).into_response();
    let secure = if is_https_request(&headers) { "; Secure" } else { "" };
    if let Ok(cookie_val) = HeaderValue::from_str(&format!(
        "pipi_token=; HttpOnly; Path=/; Max-Age=0; SameSite=Lax{secure}"
    )) {
        response.headers_mut().insert(header::SET_COOKIE, cookie_val);
    }
    response
}

async fn auth_middleware(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let Some(expected) = state.auth_token.as_deref() else {
        return next.run(req).await;
    };

    let authorized = check_request_authorized(req.headers(), req.uri(), expected);

    if !authorized {
        let mut response = (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "需要 PIPI_AUTH_TOKEN", "authRequired": true })),
        )
            .into_response();
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return response;
    }

    let is_https = is_https_request(req.headers());
    let query_token = req.uri().query().and_then(|query| {
        query
            .split('&')
            .find_map(|part| part.strip_prefix("token="))
    });
    let is_query_auth = query_token == Some(expected);
    let mut response = next.run(req).await;
    if is_query_auth {
        let secure = if is_https { "; Secure" } else { "" };
        if let Ok(cookie_val) = HeaderValue::from_str(&format!(
            "pipi_token={expected}; HttpOnly; Path=/; SameSite=Lax{secure}"
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
        "list_agents" => to_value(agents::list_agents_bootstrapped()?),
        "model_catalog" => {
            let refresh = optional_value::<bool>(args, "refresh")?.unwrap_or(false);
            to_value(pipi_core::catalog::load_catalog(refresh).await?)
        }
        "create_agent" => {
            let name = required_string(args, "name")?;
            let description = required_string(args, "description")?;
            let workspace = optional_string(args, "workspace")?;
            let permissions = optional_value::<PermissionsConfig>(args, "permissions")?;
            let model = optional_string(args, "model")?;
            let provider = optional_value::<pipi_core::types::Model>(args, "provider")?;
            to_value(agents::create_agent(
                &name,
                &description,
                workspace.as_deref(),
                permissions,
                model.as_deref(),
                provider,
            )?)
        }
        "load_agent" => {
            let name = required_string(args, "name")?;
            to_value(agents::load_agent(&name)?)
        }
        "save_agent" => {
            let definition: AgentDefinition = required_value(args, "def")?;
            state.runtime.save_agent_definition(&definition)?;
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
        "session_info" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            to_value(state.runtime.session_info(&agent_name, &session_id)?)
        }
        "session_infos" => to_value(state.runtime.session_infos()?),
        "ensure_test_session" => {
            let agent_name = required_string(args, "agentName")?;
            to_value(state.runtime.ensure_test_session(&agent_name)?)
        }
        "reset_test_session" => {
            let agent_name = required_string(args, "agentName")?;
            to_value(state.runtime.reset_test_session(&agent_name)?)
        }
        "session_running" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            to_value(state.runtime.session_running(&agent_name, &session_id))
        }
        "stop_run" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            state.runtime.stop_run(&agent_name, &session_id)?;
            Ok(Value::Null)
        }
        "new_session" => {
            let agent_name = required_string(args, "agentName")?;
            state.runtime.new_session(&agent_name)?;
            Ok(Value::Null)
        }
        "send_prompt" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = optional_string(args, "sessionId")?;
            let prompt = required_string(args, "prompt")?;
            let model = optional_value::<pipi_core::types::Model>(args, "model")?;
            let events = state.events.clone();
            let emitter: EventEmitter = Arc::new(move |event| {
                let _ = events.send(event);
            });
            state
                .runtime
                .send_prompt(&agent_name, session_id.as_deref(), &prompt, model, emitter)?;
            Ok(Value::Null)
        }
        "compact_now" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            let events = state.events.clone();
            let emitter: EventEmitter = Arc::new(move |event| {
                let _ = events.send(event);
            });
            state.runtime.compact_now(&agent_name, &session_id, emitter)?;
            Ok(Value::Null)
        }
        "steer" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            let message = required_string(args, "message")?;
            state.runtime.steer(&agent_name, &session_id, &message)?;
            Ok(Value::Null)
        }
        "resolve_approval" => {
            let request_id = required_string(args, "requestId")?;
            let decision = required_string(args, "decision")?;
            state
                .runtime
                .resolve_approval(&request_id, decision.parse::<ApprovalDecision>()?)?;
            Ok(Value::Null)
        }
        "set_session_model" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            let model = optional_value::<pipi_core::types::Model>(args, "model")?;
            state
                .runtime
                .set_session_model(&agent_name, &session_id, model)?;
            Ok(Value::Null)
        }
        "session_messages" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            to_value(state.runtime.session_messages(&agent_name, &session_id).await?)
        }
        "session_stats" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            to_value(state.runtime.session_stats(&agent_name, &session_id)?)
        }
        "list_archived_agents" => to_value(agents::list_archived_agents()?),
        "list_agent_files" => {
            let agent_name = required_string(args, "agentName")?;
            to_value(agents::list_agent_md_files(&agent_name)?)
        }
        "read_agent_file" => {
            let agent_name = required_string(args, "agentName")?;
            let rel_path = required_string(args, "relPath")?;
            to_value(agents::read_agent_file(&agent_name, &rel_path)?)
        }
        "write_agent_file" => {
            let agent_name = required_string(args, "agentName")?;
            let rel_path = required_string(args, "relPath")?;
            let content = required_string(args, "content")?;
            state
                .runtime
                .write_agent_file(&agent_name, &rel_path, &content)?;
            Ok(Value::Null)
        }
        "archive_agent" => {
            let name = required_string(args, "name")?;
            state.runtime.archive_agent(&name)?;
            Ok(Value::Null)
        }
        "restore_agent" => {
            let name = required_string(args, "name")?;
            state.runtime.restore_agent(&name)?;
            Ok(Value::Null)
        }
        "delete_agent" => {
            let name = required_string(args, "name")?;
            state.runtime.delete_agent(&name)?;
            Ok(Value::Null)
        }
        "delete_archived_agent" => {
            let name = required_string(args, "name")?;
            state.runtime.delete_archived_agent(&name)?;
            Ok(Value::Null)
        }
        "list_archived_sessions" => {
            let agent_name = required_string(args, "agentName")?;
            to_value(state.runtime.list_archived_sessions(&agent_name)?)
        }
        "archive_session" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            state.runtime.archive_session(&agent_name, &session_id)?;
            Ok(Value::Null)
        }
        "restore_session" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            state.runtime.restore_session(&agent_name, &session_id)?;
            Ok(Value::Null)
        }
        "delete_session" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            state.runtime.delete_session(&agent_name, &session_id)?;
            Ok(Value::Null)
        }
        "delete_archived_session" => {
            let agent_name = required_string(args, "agentName")?;
            let session_id = required_string(args, "sessionId")?;
            state.runtime.delete_archived_session(&agent_name, &session_id)?;
            Ok(Value::Null)
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state(token: Option<String>) -> AppState {
        let (events, _) = broadcast::channel(16);
        AppState {
            runtime: Arc::new(RuntimeState::new(tokio::runtime::Handle::current())),
            events,
            web_root: PathBuf::from("target/test-web-root"),
            auth_token: token,
        }
    }

    #[tokio::test]
    async fn test_auth_status_disabled() {
        let app = build_app(test_state(None));
        let req = Request::builder()
            .uri("/api/auth/status")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authRequired"], false);
        assert_eq!(json["authenticated"], true);
    }

    #[tokio::test]
    async fn test_auth_status_enabled() {
        let app = build_app(test_state(Some("my-secret-token".into())));
        let req = Request::builder()
            .uri("/api/auth/status")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authRequired"], true);
        assert_eq!(json["authenticated"], false);

        // With valid token in header
        let req2 = Request::builder()
            .uri("/api/auth/status")
            .header(PIPI_TOKEN_HEADER, "my-secret-token")
            .body(Body::empty())
            .unwrap();
        let res2 = app.oneshot(req2).await.unwrap();
        assert_eq!(res2.status(), StatusCode::OK);
        let body2 = axum::body::to_bytes(res2.into_body(), usize::MAX).await.unwrap();
        let json2: Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2["authRequired"], true);
        assert_eq!(json2["authenticated"], true);
    }

    #[tokio::test]
    async fn test_login_and_logout() {
        let app = build_app(test_state(Some("my-secret-token".into())));

        // Wrong token
        let req_wrong = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"token":"wrong"}"#))
            .unwrap();
        let res_wrong = app.clone().oneshot(req_wrong).await.unwrap();
        assert_eq!(res_wrong.status(), StatusCode::UNAUTHORIZED);

        // Correct token
        let req_correct = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"token":"my-secret-token"}"#))
            .unwrap();
        let res_correct = app.clone().oneshot(req_correct).await.unwrap();
        assert_eq!(res_correct.status(), StatusCode::OK);
        let cookie = res_correct.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap();
        assert!(cookie.contains("pipi_token=my-secret-token"));

        // Logout
        let req_logout = Request::builder()
            .method("POST")
            .uri("/api/auth/logout")
            .body(Body::empty())
            .unwrap();
        let res_logout = app.oneshot(req_logout).await.unwrap();
        assert_eq!(res_logout.status(), StatusCode::OK);
        let clear_cookie = res_logout.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap();
        assert!(clear_cookie.contains("Max-Age=0"));
    }

    #[tokio::test]
    async fn test_protected_routes_and_static_fallback() {
        let app = build_app(test_state(Some("my-secret-token".into())));

        // Invoke blocked without token
        let req_invoke = Request::builder()
            .method("POST")
            .uri("/api/invoke")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"command":"get_settings"}"#))
            .unwrap();
        let res_invoke = app.clone().oneshot(req_invoke).await.unwrap();
        assert_eq!(res_invoke.status(), StatusCode::UNAUTHORIZED);

        // Root path is not blocked by auth (does not return 401)
        let req_root = Request::builder()
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let res_root = app.oneshot(req_root).await.unwrap();
        assert_ne!(res_root.status(), StatusCode::UNAUTHORIZED);
    }
}
