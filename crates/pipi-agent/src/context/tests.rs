use super::*;
use crate::test_support::{assistant_text, tool_result};

#[test]
fn estimation_heuristic() {
    // 4 字符 ≈ 1 token
    assert_eq!(estimate_tokens(&Message::user_text("abcdefgh")), 2);
    // 图片按 4800 chars 计
    let image = Message::ToolResult {
        tool_call_id: "t".into(),
        tool_name: "read".into(),
        content: vec![ToolResultContent::Image {
            data: "x".into(),
            mime_type: "image/png".into(),
        }],
        is_error: false,
        details: None,
        timestamp: 0,
    };
    assert_eq!(estimate_tokens(&image), ESTIMATED_IMAGE_CHARS as u64 / 4);
}

#[test]
fn prune_cuts_at_user_boundaries_only() {
    // user(长) → assistant → toolResult → user(短) → assistant(短)
    let messages = vec![
        Message::user_text("x".repeat(10_000)),
        assistant_text(&"y".repeat(100)),
        tool_result("t1", "bash", &"z".repeat(100)),
        Message::user_text("short question"),
        assistant_text("answer"),
    ];
    // 预算只够放下后两条
    let (pruned, dropped) = prune_oldest(&messages, 900);
    assert_eq!(dropped, 3);
    assert_eq!(pruned.len(), 2);
    assert_eq!(pruned[0].role(), "user");
    // 预算充足：不动
    let (pruned, dropped) = prune_oldest(&messages, 99_900);
    assert_eq!((pruned.len(), dropped), (5, 0));
}

#[test]
fn prune_never_orphans_tool_results() {
    // 首条就是 assistant + toolResult（无 user 边界在尾部预算内）
    let messages = vec![
        Message::user_text("x".repeat(10_000)),
        assistant_text(&"y".repeat(10_000)),
        tool_result("t1", "bash", &"z".repeat(10_000)),
    ];
    let (pruned, dropped) = prune_oldest(&messages, 500);
    assert_eq!((pruned.len(), dropped), (3, 0));
}

#[test]
fn env_context_renders_tags_and_escapes_hostile_workspace_path() {
    let ctx = environment_context(Path::new("/home/u/proj"), &SandboxMode::WorkspaceWrite);
    assert!(ctx.contains("<environment_context>"));
    assert!(ctx.contains("<cwd>/home/u/proj</cwd>"));
    assert!(ctx.contains("<sandbox>workspace-write</sandbox>"));
    assert!(ctx.contains("<date>20"));

    // 工作目录来自用户/仓库，必须转义，不能破坏 XML 结构
    let hostile = environment_context(
        Path::new("/tmp/a<&>\"\'\nnext"),
        &SandboxMode::WorkspaceWrite,
    );
    assert!(hostile.contains("<cwd>/tmp/a&lt;&amp;&gt;&quot;&apos;\\nnext</cwd>"));
    assert!(!hostile.contains("<cwd>/tmp/a<&>\"\'"));
}
