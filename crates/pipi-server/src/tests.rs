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
