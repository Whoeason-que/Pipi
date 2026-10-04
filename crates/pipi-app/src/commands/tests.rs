use std::collections::HashSet;
use std::sync::Arc;

use super::*;

#[test]
fn commands_are_registered_once() {
    let unique: HashSet<&str> = COMMANDS.iter().copied().collect();
    assert_eq!(unique.len(), COMMANDS.len(), "COMMANDS 存在重复命令名");
    assert_eq!(
        COMMANDS.len(),
        47,
        "宿主命令数应为 47（前端 CommandMap 同步）"
    );
}

#[tokio::test]
async fn unknown_command_is_rejected() {
    let runtime = RuntimeState::new(tokio::runtime::Handle::current());
    let emitter: EventEmitter = Arc::new(|_| {});
    let error = dispatch(&runtime, emitter, "no_such_command", Value::Null)
        .await
        .expect_err("未知命令必须报错");
    assert!(
        error.contains("no_such_command"),
        "错误信息应包含命令名：{error}"
    );
}

#[tokio::test]
async fn dispatch_parses_flat_camel_case_args() {
    let runtime = RuntimeState::new(tokio::runtime::Handle::current());
    let emitter: EventEmitter = Arc::new(|_| {});
    let value = dispatch(
        &runtime,
        emitter,
        "session_running",
        serde_json::json!({ "agentName": "ghost", "sessionId": "none" }),
    )
    .await
    .expect("session_running 应当成功");
    assert_eq!(value, Value::Bool(false));
}
