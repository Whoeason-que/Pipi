use super::*;
use crate::test_support::tool_result;
use pipi_protocol::{ContentBlock, StopReason, Usage};

fn assistant_with_calls(calls: &[(&str, &str)]) -> Message {
    Message::Assistant {
        content: calls
            .iter()
            .map(|(id, name)| ContentBlock::ToolCall {
                id: (*id).into(),
                name: (*name).into(),
                arguments: serde_json::json!({}),
            })
            .collect(),
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
        duration_ms: None,
    }
}

fn role_ids(messages: &[Message]) -> Vec<(String, String)> {
    messages
        .iter()
        .map(|message| match message {
            Message::User { .. } => ("user".into(), String::new()),
            Message::Assistant { .. } => ("assistant".into(), String::new()),
            Message::ToolResult {
                tool_call_id,
                is_error,
                ..
            } => (
                if *is_error { "tool-error" } else { "tool" }.into(),
                tool_call_id.clone(),
            ),
        })
        .collect()
}

#[test]
fn healthy_history_is_unchanged() {
    let messages = vec![
        Message::user_text("hi"),
        assistant_with_calls(&[("c1", "bash"), ("c2", "read")]),
        tool_result("c1", "bash", "out"),
        tool_result("c2", "read", "out"),
        Message::assistant_text("done", "m"),
    ];
    let repaired = repair_tool_pairing(messages.clone());
    assert_eq!(repaired, messages);
}

#[test]
fn missing_results_are_synthesized_right_after_the_assistant() {
    let messages = vec![
        Message::user_text("hi"),
        assistant_with_calls(&[("c1", "bash")]),
        Message::user_text("继续"),
        Message::assistant_text("ok", "m"),
    ];
    let repaired = repair_tool_pairing(messages);
    assert_eq!(
        role_ids(&repaired),
        vec![
            ("user".to_string(), String::new()),
            ("assistant".to_string(), String::new()),
            ("tool-error".to_string(), "c1".to_string()),
            ("user".to_string(), String::new()),
            ("assistant".to_string(), String::new()),
        ]
    );
}

#[test]
fn partial_batch_is_completed_in_order() {
    let messages = vec![
        assistant_with_calls(&[("c1", "bash"), ("c2", "read"), ("c3", "grep")]),
        tool_result("c1", "bash", "out"),
    ];
    let repaired = repair_tool_pairing(messages);
    assert_eq!(
        role_ids(&repaired),
        vec![
            ("assistant".to_string(), String::new()),
            ("tool".to_string(), "c1".to_string()),
            ("tool-error".to_string(), "c2".to_string()),
            ("tool-error".to_string(), "c3".to_string()),
        ]
    );
}

#[test]
fn orphan_tool_results_are_dropped() {
    let messages = vec![
        Message::user_text("hi"),
        tool_result("ghost", "bash", "out"),
        assistant_with_calls(&[("c1", "bash")]),
        tool_result("c1", "bash", "out"),
    ];
    let repaired = repair_tool_pairing(messages);
    assert_eq!(
        role_ids(&repaired),
        vec![
            ("user".to_string(), String::new()),
            ("assistant".to_string(), String::new()),
            ("tool".to_string(), "c1".to_string()),
        ]
    );
}

#[test]
fn histories_without_tools_pass_through() {
    let messages = vec![
        Message::user_text("a"),
        Message::assistant_text("b", "m"),
        Message::user_text("c"),
    ];
    assert_eq!(repair_tool_pairing(messages.clone()), messages);
}
