//! 载入自愈：被进程中断的会话（尾部留下无主工具调用）在打开时被修复 ——
//! 补上合成的失败结果并落盘，否则之后每次请求都会被端点按协议 400 拒绝
//!（"An assistant message with 'tool_calls' must be followed by tool messages"）。

use std::sync::{Arc, Mutex};

use pipi_core::permissions::{BashPermissions, PermissionsConfig, SandboxMode};
use pipi_core::types::{ContentBlock, Message, StopReason, ToolResultContent, Usage};

static HOME_LOCK: Mutex<()> = Mutex::new(());

fn assistant_with_call(id: &str, name: &str) -> Message {
    Message::Assistant {
        content: vec![ContentBlock::ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: serde_json::json!({"command": "sleep 300"}),
        }],
        api: String::new(),
        provider: String::new(),
        model: "m".into(),
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
        duration_ms: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_a_session_repairs_interrupted_tail_once() {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = std::env::temp_dir().join(format!(
        "pipi-session-repair-{}-{}",
        std::process::id(),
        pipi_core::session::new_id()
    ));
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::env::set_var("HOME", &home);

    pipi_core::agents::create_agent(
        "repair-worker",
        "A worker whose run got killed",
        Some(workspace.to_str().unwrap()),
        Some(PermissionsConfig {
            tools: vec!["read".into()],
            bash: BashPermissions::default(),
            sandbox: SandboxMode::WorkspaceWrite,
        }),
        None,
        None,
    )
    .unwrap();
    let definition = pipi_core::agents::load_agent("repair-worker").unwrap();
    let sessions_dir = definition.sessions_dir().unwrap();

    // 被中断的会话：user → assistant(tool_calls)，结果没落盘（进程被杀）
    let session_id = {
        let mut writer = pipi_core::session::SessionWriter::create(&sessions_dir).unwrap();
        writer.append_message(&Message::user_text("跑一下")).unwrap();
        writer
            .append_message(&assistant_with_call("call-1", "bash"))
            .unwrap();
        writer
            .path()
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    };

    let runtime = pipi_core::runtime::RuntimeState::new(tokio::runtime::Handle::current());
    runtime.open_session("repair-worker", &session_id).unwrap();

    let messages = runtime.session_messages("repair-worker", &session_id).await.unwrap();
    let roles: Vec<&str> = messages.iter().map(|message| message.role()).collect();
    assert_eq!(
        roles,
        vec!["user", "assistant", "toolResult"],
        "打开会话时应补上缺失的工具结果"
    );
    match messages.last().unwrap() {
        Message::ToolResult {
            tool_call_id,
            is_error,
            content,
            ..
        } => {
            assert_eq!(tool_call_id, "call-1");
            assert!(is_error, "合成结果必须是失败态");
            assert!(matches!(
                &content[0],
                ToolResultContent::Text { text } if text.contains("工具结果缺失")
            ));
        }
        other => panic!("expected synthesized toolResult, got {other:?}"),
    }

    // 落盘：重新从文件读取也是配对完整的（自愈写回，不只是内存）
    let path = sessions_dir.join(format!("{session_id}.jsonl"));
    let entries = pipi_core::session::load_session(&path).unwrap();
    let rebuilt = pipi_core::session::rebuild_messages(&pipi_core::session::active_path(&entries));
    assert_eq!(
        rebuilt.iter().map(|message| message.role()).collect::<Vec<_>>(),
        vec!["user", "assistant", "toolResult"],
    );

    // 幂等：重复打开不会重复追加
    runtime.new_session("repair-worker").unwrap();
    runtime.open_session("repair-worker", &session_id).unwrap();
    let again = runtime.session_messages("repair-worker", &session_id).await.unwrap();
    assert_eq!(again.len(), 3, "重复打开不应重复补结果：{again:?}");

    let _ = std::fs::remove_dir_all(home);
}

/// 尾部完整（正常的轮次）与中段悬挂都不应被尾部修复动到 —— 前者无需修，
/// 后者由发送前修复兜底（见 agent_loop 的 wire 测试）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_tail_is_left_untouched() {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = std::env::temp_dir().join(format!(
        "pipi-session-intact-{}-{}",
        std::process::id(),
        pipi_core::session::new_id()
    ));
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::env::set_var("HOME", &home);

    pipi_core::agents::create_agent(
        "intact-worker",
        "A worker with a clean history",
        Some(workspace.to_str().unwrap()),
        Some(PermissionsConfig {
            tools: vec!["read".into()],
            bash: BashPermissions::default(),
            sandbox: SandboxMode::WorkspaceWrite,
        }),
        None,
        None,
    )
    .unwrap();
    let definition = pipi_core::agents::load_agent("intact-worker").unwrap();
    let sessions_dir = definition.sessions_dir().unwrap();

    let session_id = {
        let mut writer = pipi_core::session::SessionWriter::create(&sessions_dir).unwrap();
        writer.append_message(&Message::user_text("你好")).unwrap();
        writer
            .append_message(&Message::assistant_text("回复", "m"))
            .unwrap();
        writer
            .path()
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    };

    let runtime = Arc::new(pipi_core::runtime::RuntimeState::new(
        tokio::runtime::Handle::current(),
    ));
    runtime.open_session("intact-worker", &session_id).unwrap();
    let messages = runtime.session_messages("intact-worker", &session_id).await.unwrap();
    assert_eq!(
        messages.iter().map(|message| message.role()).collect::<Vec<_>>(),
        vec!["user", "assistant"],
        "完整历史不应被改动"
    );

    let _ = std::fs::remove_dir_all(home);
}
