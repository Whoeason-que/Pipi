//! 压缩流水线的回归语料。
//!
//! 两件事：
//! 1. 合成一段「像真实会话」的长历史（多轮、工具输出有大有小、有失败、有图片、
//!    有单个超长 turn），在多个预算档下跑投影式流水线，断言三条硬不变量：
//!    配对破损不增加、历史不增长、投影幂等（可重算 —— 回放时重跑必得同一结果）。
//! 2. 若本机存在真实会话文件（`~/.pipi/agents/*/sessions/*.jsonl`），把它也喂进
//!    同一套断言 —— 真实数据的形状永远比合成的更奇怪。没有就跳过（CI 上不失败）。

use pipi_core::compaction::{project, Budget, DEFAULT_THRESHOLD_PERCENT};
use pipi_core::context::estimate_context_tokens;
use pipi_core::session::{active_path, load_session, replay};
use pipi_core::types::{ContentBlock, Message, StopReason, ToolResultContent};

// ---------------------------------------------------------------------------
// 合成语料
// ---------------------------------------------------------------------------

fn assistant(content: Vec<ContentBlock>) -> Message {
    let has_call = content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolCall { .. }));
    Message::Assistant {
        content,
        api: String::new(),
        provider: String::new(),
        model: "corpus".into(),
        usage: Default::default(),
        stop_reason: if has_call {
            StopReason::ToolUse
        } else {
            StopReason::Stop
        },
        error_message: None,
        timestamp: 0,
        duration_ms: None,
    }
}

fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> ContentBlock {
    ContentBlock::ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
    }
}

fn tool_result(id: &str, name: &str, text: String, is_error: bool) -> Message {
    Message::ToolResult {
        tool_call_id: id.into(),
        tool_name: name.into(),
        content: vec![ToolResultContent::Text { text }],
        is_error,
        details: None,
        timestamp: 0,
    }
}

/// 一段形状接近真实编码会话的历史：40 轮，含大文件读取、失败命令、图片结果，
/// 以及末尾一个「单 turn 内 12 次工具调用」的长跑（切点在尾部找不到 user 边界）。
fn synthetic_session() -> Vec<Message> {
    let mut messages = Vec::new();
    for turn in 0..40 {
        messages.push(Message::user_text(format!(
            "第 {turn} 轮：看一下 src/module_{turn}.rs 并跑一下测试"
        )));
        let call_id = format!("call_{turn}_a");
        messages.push(assistant(vec![
            ContentBlock::Thinking {
                thinking: format!("先读文件再跑测试。第 {turn} 轮没什么特别的。"),
                thinking_signature: None,
            },
            ContentBlock::Text {
                text: format!("我先读 module_{turn}.rs。"),
            },
            tool_call(
                &call_id,
                "read",
                serde_json::json!({ "path": format!("src/module_{turn}.rs") }),
            ),
        ]));
        // 大输出（真实读文件常见几 KB ~ 几十 KB）
        messages.push(tool_result(
            &call_id,
            "read",
            format!("// module_{turn}\n{}", "fn f() {}\n".repeat(900)),
            false,
        ));

        let test_id = format!("call_{turn}_b");
        messages.push(assistant(vec![
            ContentBlock::Text {
                text: "再跑测试。".into(),
            },
            tool_call(&test_id, "bash", serde_json::json!({ "cmd": "cargo test" })),
        ]));
        if turn % 7 == 3 {
            // 失败命令：输出不大但必须留下失败标记
            messages.push(tool_result(
                &test_id,
                "bash",
                format!("error: could not compile `module_{turn}`"),
                true,
            ));
            let fix_id = format!("call_{turn}_c");
            messages.push(assistant(vec![
                ContentBlock::Text {
                    text: "编译失败，我改一下。".into(),
                },
                tool_call(
                    &fix_id,
                    "edit",
                    serde_json::json!({ "path": format!("src/module_{turn}.rs") }),
                ),
            ]));
            messages.push(tool_result(
                &fix_id,
                "edit",
                format!("已修改 src/module_{turn}.rs"),
                false,
            ));
        } else {
            messages.push(tool_result(
                &test_id,
                "bash",
                format!("test result: ok. {} passed", 10 + turn),
                false,
            ));
        }
        if turn % 11 == 5 {
            // 图片结果（估算按 4800 字符计）
            let shot_id = format!("call_{turn}_img");
            messages.push(assistant(vec![
                ContentBlock::Text {
                    text: "截个图看看。".into(),
                },
                tool_call(&shot_id, "screenshot", serde_json::json!({})),
            ]));
            messages.push(Message::ToolResult {
                tool_call_id: shot_id,
                tool_name: "screenshot".into(),
                content: vec![ToolResultContent::Image {
                    data: "AAAA".repeat(500),
                    mime_type: "image/png".into(),
                }],
                is_error: false,
                details: None,
                timestamp: 0,
            });
        }
    }

    // 末尾长跑：一个 turn 里连续 12 次工具调用（尾部没有 user 边界）
    messages.push(Message::user_text("最后把整个 crate 过一遍"));
    for step in 0..12 {
        let id = format!("long_{step}");
        messages.push(assistant(vec![
            ContentBlock::Text {
                text: format!("第 {step} 步。"),
            },
            tool_call(&id, "bash", serde_json::json!({ "cmd": "cargo check" })),
        ]));
        messages.push(tool_result(
            &id,
            "bash",
            "warning: unused import\n".repeat(300),
            false,
        ));
    }
    messages
}

// ---------------------------------------------------------------------------
// 不变量断言
// ---------------------------------------------------------------------------

/// 破损度：无主 tool_call 数 + 孤儿 tool_result 数。
fn damage(messages: &[Message]) -> (usize, usize) {
    use std::collections::HashSet;
    let calls: HashSet<&str> = messages
        .iter()
        .flat_map(|message| message.tool_calls())
        .filter_map(|block| match block {
            ContentBlock::ToolCall { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let results: HashSet<&str> = messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult { tool_call_id, .. } => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    (
        calls.difference(&results).count(),
        results.difference(&calls).count(),
    )
}

/// 对一段历史跑多档预算的投影流水线，逐档断言不变量。
fn assert_pipeline_invariants(label: &str, messages: &[Message]) {
    let base = damage(messages);
    let before_tokens = estimate_context_tokens(messages);
    // 从「几乎不压」到「压到很小」扫一遍预算
    for window in [
        before_tokens * 4,
        before_tokens + 1,
        before_tokens / 2,
        before_tokens / 4,
        before_tokens / 10,
    ] {
        let budget = Budget::from_window(window.max(1), DEFAULT_THRESHOLD_PERCENT);
        let projected = project(messages.to_vec(), budget);

        let after = damage(&projected);
        assert!(
            after.0 <= base.0 && after.1 <= base.1,
            "{label}（窗口 {window}）：投影增加了配对破损 {base:?} → {after:?}"
        );
        let after_tokens = estimate_context_tokens(&projected);
        assert!(
            after_tokens <= before_tokens,
            "{label}（窗口 {window}）：投影后反而变大 {before_tokens} → {after_tokens}"
        );
        // 投影是确定性的：重跑必得同一结果（回放靠这一点，live 才与重开一致）
        let again = project(messages.to_vec(), budget);
        assert_eq!(
            projected, again,
            "{label}（窗口 {window}）：投影不可重算（两次结果不同）"
        );
        // 幂等：对结果再投影一次不变形
        let twice = project(projected.clone(), budget);
        assert_eq!(
            projected, twice,
            "{label}（窗口 {window}）：投影不幂等（二次应用改变结果）"
        );
    }
}

#[test]
fn synthetic_long_session_keeps_invariants() {
    let messages = synthetic_session();
    assert!(
        messages.len() > 200,
        "语料应足够长，实际 {} 条",
        messages.len()
    );
    assert_eq!(damage(&messages), (0, 0), "合成语料本身必须是健康的");
    assert_pipeline_invariants("合成语料", &messages);
}

#[test]
fn synthetic_session_projection_shrinks_when_budget_is_tight() {
    let messages = synthetic_session();
    let before = estimate_context_tokens(&messages);
    // 预算压到 1/4：应当明显变小（清理 + 硬裁都在起作用）
    let budget = Budget::from_window(before / 4, DEFAULT_THRESHOLD_PERCENT);
    let projected = project(messages, budget);
    let after = estimate_context_tokens(&projected);
    assert!(after < before, "预算收紧后应当变小：{before} → {after}");
    assert_eq!(damage(&projected), (0, 0), "压缩不得破坏配对");
}

/// 真实会话语料：本机存在就喂进同一套断言，没有就跳过。
#[test]
fn real_session_corpus_keeps_invariants() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let agents_dir = std::path::Path::new(&home).join(".pipi/agents");
    let Ok(agents) = std::fs::read_dir(&agents_dir) else {
        eprintln!("跳过：本机没有 ~/.pipi/agents");
        return;
    };
    let mut checked = 0;
    for agent in agents.flatten() {
        let sessions = agent.path().join("sessions");
        let Ok(files) = std::fs::read_dir(&sessions) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(entries) = load_session(&path) else {
                continue;
            };
            if entries.is_empty() {
                continue;
            }
            let active = active_path(&entries);
            // 回放的两条硬要求：消息与条目 id 一一对应（runtime 换算
            // keep_from_entry 依赖它）、id 不重复
            let (messages, ids) = replay(&active);
            assert_eq!(
                messages.len(),
                ids.len(),
                "{}：回放出的消息数与条目 id 数不一致",
                path.display()
            );
            let unique: std::collections::HashSet<&String> = ids.iter().collect();
            assert_eq!(unique.len(), ids.len(), "{}：条目 id 重复", path.display());
            assert_pipeline_invariants(&format!("{}", path.display()), &messages);
            checked += 1;
        }
    }
    eprintln!("真实会话语料：检查了 {checked} 个会话文件");
}
