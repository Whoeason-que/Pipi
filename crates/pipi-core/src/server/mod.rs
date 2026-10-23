//! Pipi Web/远程运行时。
//!
//! 默认只绑定 127.0.0.1，适合由 Tailscale Serve 代理：
//!   cargo run -p pipi-core --features server --bin pipi-server
//!   tailscale serve 1421
//!
//! 浏览器与桌面端复用同一组 command 名称和事件 payload：服务端只做 transport
//! （鉴权 / WS / 静态资源），所有 Agent 执行都经过共享的 `RuntimeState`。

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Json, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};

use crate::events::{EventHub, RuntimeEvent};
use crate::runtime::RuntimeState;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::watch;

mod assets;
mod lifecycle;
pub use assets::{AssetResolver, WebAsset, WebAssets};
pub use lifecycle::{ServerHandle, start};
use tokio::sync::broadcast;
use tower_http::services::{ServeDir, ServeFile};

const DEFAULT_ADDR: &str = "127.0.0.1:1421";
const PIPI_TOKEN_HEADER: &str = "x-pipi-token";

#[derive(Clone)]
struct AppState {
    runtime: Arc<RuntimeState>,
    events: EventHub,
    assets: WebAssets,
    shutdown: watch::Receiver<bool>,
    clients: Arc<AtomicUsize>,
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

fn check_request_authorized(
    headers: &header::HeaderMap,
    uri: &axum::http::Uri,
    expected: &str,
) -> bool {
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

    let router = Router::new().nest("/api", api_routes);
    let router = match &state.assets {
        WebAssets::Directory(root) => router.fallback_service(
            ServeDir::new(root).not_found_service(ServeFile::new(root.join("index.html"))),
        ),
        WebAssets::Embedded(_) => router.fallback(assets::embedded_asset),
    };
    router
        .layer(axum::middleware::from_fn(same_origin))
        .with_state(state)
}

/// 启动 Web/远程宿主：读环境变量、建运行时、serve。
///
/// bin `pipi-server`（`--features server`）只调它。
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
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
        return Err("PIPI_AUTH_TOKEN cannot contain control characters or semicolons".into());
    }

    let _lease = crate::remote::HostLease::acquire()?;
    let runtime = Arc::new(RuntimeState::new(tokio::runtime::Handle::current()));
    let mut server = start(runtime, addr, auth_token, WebAssets::Directory(web_root)).await?;
    println!("Pipi Web server listening on http://{}", server.address());
    server.wait().await?;
    Ok(())
}

// 浏览器 Origin 与 Host 必须相同。无 Origin 的 CLI/代理仍须通过令牌鉴权。
async fn same_origin(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let origin = origin
            .to_str()
            .ok()
            .and_then(|s| s.parse::<axum::http::Uri>().ok());
        let host = req
            .headers()
            .get(header::HOST)
            .and_then(|s| s.to_str().ok());
        let allowed = origin.as_ref().is_some_and(|origin| {
            origin.scheme_str()
                == Some(if is_https_request(req.headers()) {
                    "https"
                } else {
                    "http"
                })
                && origin.authority().map(|a| a.as_str()) == host
        });
        if !allowed {
            return (StatusCode::FORBIDDEN, "Cross-site requests forbidden").into_response();
        }
    }
    next.run(req).await
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
    if let Some(proto) = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        && proto.eq_ignore_ascii_case("https")
    {
        return true;
    }
    if let Some(cf) = headers.get("cf-visitor").and_then(|v| v.to_str().ok())
        && cf.contains("https")
    {
        return true;
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
            let secure = if is_https_request(&headers) {
                "; Secure"
            } else {
                ""
            };
            if let Ok(cookie_val) = HeaderValue::from_str(&format!(
                "pipi_token={expected}; HttpOnly; Path=/; SameSite=Lax{secure}"
            )) {
                response
                    .headers_mut()
                    .insert(header::SET_COOKIE, cookie_val);
            }
            response
        } else {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "ok": false, "error": "Invalid token, please check and retry" })),
            )
                .into_response()
        }
    } else {
        Json(json!({ "ok": true })).into_response()
    }
}

async fn auth_logout_handler(headers: header::HeaderMap) -> Response {
    let mut response = Json(json!({ "ok": true })).into_response();
    let secure = if is_https_request(&headers) {
        "; Secure"
    } else {
        ""
    };
    if let Ok(cookie_val) = HeaderValue::from_str(&format!(
        "pipi_token=; HttpOnly; Path=/; Max-Age=0; SameSite=Lax{secure}"
    )) {
        response
            .headers_mut()
            .insert(header::SET_COOKIE, cookie_val);
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
            Json(json!({ "error": "PIPI_AUTH_TOKEN is required", "authRequired": true })),
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
            response
                .headers_mut()
                .insert(header::SET_COOKIE, cookie_val);
        }
    }
    response
}

async fn ws_events(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let receiver = state.events.subscribe();
    let pending = state.runtime.pending_approvals();
    ws.on_upgrade(move |socket| {
        stream_events(socket, receiver, state.shutdown, state.clients, pending)
    })
}

async fn stream_events(
    mut socket: WebSocket,
    mut receiver: broadcast::Receiver<RuntimeEvent>,
    mut shutdown: watch::Receiver<bool>,
    clients: Arc<AtomicUsize>,
    pending: Vec<crate::events::ApprovalRequestEnvelope>,
) {
    struct ClientCount(Arc<AtomicUsize>);
    impl Drop for ClientCount {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::Relaxed);
        }
    }
    clients.fetch_add(1, Ordering::Relaxed);
    let _count = ClientCount(clients);
    if *shutdown.borrow() {
        return;
    }
    for envelope in pending {
        if let Ok(text) = serde_json::to_string(&RuntimeEvent::ApprovalRequest(envelope))
            && !send_frame(&mut socket, WsMessage::Text(text.into()), &mut shutdown).await
        {
            return;
        }
    }
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = ticker.tick() => {
                if !send_frame(&mut socket, WsMessage::Ping(Default::default()), &mut shutdown).await {
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
                        if !send_frame(&mut socket, WsMessage::Text(text.into()), &mut shutdown).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("pipi-server: websocket receiver lagged by {n} events; reconnect required");
                        break;
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

async fn send_frame(
    socket: &mut WebSocket,
    frame: WsMessage,
    shutdown: &mut watch::Receiver<bool>,
) -> bool {
    if *shutdown.borrow() {
        return false;
    }
    tokio::select! {
        _ = shutdown.changed() => false,
        result = tokio::time::timeout(std::time::Duration::from_secs(5), socket.send(frame)) => matches!(result, Ok(Ok(()))),
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
    let emitter = state.events.emitter();
    crate::commands::dispatch(
        &state.runtime,
        emitter,
        request.command.trim(),
        request.args.clone(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state(token: Option<String>) -> AppState {
        let events = EventHub::default();
        AppState {
            runtime: Arc::new(RuntimeState::new(tokio::runtime::Handle::current())),
            events,
            assets: WebAssets::Directory(PathBuf::from("target/test-web-root")),
            shutdown: watch::channel(false).1,
            clients: Arc::new(AtomicUsize::new(0)),
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body2 = axum::body::to_bytes(res2.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let cookie = res_correct
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("pipi_token=my-secret-token"));

        // Logout
        let req_logout = Request::builder()
            .method("POST")
            .uri("/api/auth/logout")
            .body(Body::empty())
            .unwrap();
        let res_logout = app.oneshot(req_logout).await.unwrap();
        assert_eq!(res_logout.status(), StatusCode::OK);
        let clear_cookie = res_logout
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
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
        let req_root = Request::builder().uri("/").body(Body::empty()).unwrap();
        let res_root = app.oneshot(req_root).await.unwrap();
        assert_ne!(res_root.status(), StatusCode::UNAUTHORIZED);
    }
}
