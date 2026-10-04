use super::*;
use crate::permissions::PermissionsConfig;
use pipi_protocol::AbortSignal;
use std::path::PathBuf;
use std::sync::Arc;

async fn grep_fixture() -> (PathBuf, ToolContext, PathBuf) {
    let base = std::env::temp_dir().join(format!("pipi-grep-{}", pipi_test_support::new_id()));
    let workspace = base.join("workspace");
    tokio::fs::create_dir_all(workspace.join("src"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(workspace.join(".hidden"))
        .await
        .unwrap();
    tokio::fs::write(workspace.join("src/a.rs"), "fn main() {}\nlet x = 42;\n")
        .await
        .unwrap();
    tokio::fs::write(workspace.join("src/b.ts"), "const answer = 42;\n")
        .await
        .unwrap();
    tokio::fs::write(workspace.join("README.md"), "# Readme\nanswer is 42\n")
        .await
        .unwrap();
    tokio::fs::write(workspace.join(".hidden/secret.rs"), "let secret = 42;\n")
        .await
        .unwrap();
    // 二进制文件（含 NUL）
    tokio::fs::write(workspace.join("bin.dat"), b"\x00\x01binary\x00")
        .await
        .unwrap();
    let outside = base.join("outside.rs");
    tokio::fs::write(&outside, "let outside = 42;\n")
        .await
        .unwrap();

    let ctx = ToolContext {
        workspace: workspace.clone(),
        memory_dir: None,
        read_roots: vec![],
        permissions: Arc::new(PermissionsConfig::default()),
        sandbox: crate::permissions::SandboxMode::DangerFullAccess,
        resolved_env: Arc::new(std::collections::BTreeMap::new()),
        abort: AbortSignal::new(),
        approver: None,
    };
    (base, ctx, outside)
}

async fn grep(ctx: &ToolContext, args: Value) -> Result<ToolOutput, String> {
    GrepTool.execute(ctx, &args, &|_| {}).await
}

#[tokio::test]
async fn grep_finds_matches_with_location() {
    let (base, ctx, _outside) = grep_fixture().await;
    let out = grep(&ctx, json!({ "pattern": "42" })).await.unwrap();
    let ToolResultContent::Text { text } = &out.content[0] else {
        panic!("expected text");
    };
    assert!(text.contains("src/a.rs:2: let x = 42;"));
    assert!(text.contains("src/b.ts:1: const answer = 42;"));
    assert!(text.contains("README.md:2: answer is 42"));
    // 跳过点目录与二进制
    assert!(!text.contains("secret"));
    assert!(!text.contains("bin.dat"));
    assert!(!text.contains("outside.rs"));
    tokio::fs::remove_dir_all(base).await.unwrap();
}

#[tokio::test]
async fn grep_include_filters_by_filename() {
    let (base, ctx, _outside) = grep_fixture().await;
    let out = grep(&ctx, json!({ "pattern": "42", "include": "*.rs" }))
        .await
        .unwrap();
    let ToolResultContent::Text { text } = &out.content[0] else {
        panic!("expected text");
    };
    assert!(text.contains("src/a.rs"));
    assert!(!text.contains("b.ts"));
    assert!(!text.contains("README.md"));
    tokio::fs::remove_dir_all(base).await.unwrap();
}

#[tokio::test]
async fn grep_case_insensitive_option() {
    let (base, ctx, _outside) = grep_fixture().await;
    // 大小写敏感时 ANSWER 匹配不到任何行
    let sensitive = grep(&ctx, json!({ "pattern": "ANSWER" })).await.unwrap();
    let ToolResultContent::Text { text } = &sensitive.content[0] else {
        panic!("expected text");
    };
    assert!(text.contains("No matches found"));
    // 忽略大小写后命中 answer / Answer
    let out = grep(
        &ctx,
        json!({ "pattern": "ANSWER", "case_insensitive": true }),
    )
    .await
    .unwrap();
    let ToolResultContent::Text { text } = &out.content[0] else {
        panic!("expected text");
    };
    assert!(text.contains("answer is 42"));
    assert!(text.contains("const answer = 42"));
    tokio::fs::remove_dir_all(base).await.unwrap();
}

#[tokio::test]
async fn grep_invalid_regex_and_no_match() {
    let (base, ctx, _outside) = grep_fixture().await;
    let err = grep(&ctx, json!({ "pattern": "([" })).await;
    assert!(err.is_err());
    let out = grep(&ctx, json!({ "pattern": "does-not-exist-xyz" }))
        .await
        .unwrap();
    let ToolResultContent::Text { text } = &out.content[0] else {
        panic!("expected text");
    };
    assert!(text.contains("No matches found"));
    tokio::fs::remove_dir_all(base).await.unwrap();
}

#[tokio::test]
async fn grep_search_root_must_be_within_workspace() {
    let (base, ctx, outside) = grep_fixture().await;
    let err = grep(
        &ctx,
        json!({ "pattern": "42", "path": base.to_string_lossy().into_owned() }),
    )
    .await;
    assert!(err.is_err());
    // 工作目录内的子目录正常
    let out = grep(&ctx, json!({ "pattern": "42", "path": "src" }))
        .await
        .unwrap();
    let ToolResultContent::Text { text } = &out.content[0] else {
        panic!("expected text");
    };
    assert!(text.contains("src/a.rs"));
    assert!(!text.contains("README.md"));
    let _ = outside;
    tokio::fs::remove_dir_all(base).await.unwrap();
}

#[test]
fn binary_detection_and_include_parsing() {
    assert!(looks_like_binary(b"ab\x00cd"));
    assert!(!looks_like_binary(b"plain text"));
    assert!(matches_include("a.rs", Some("*.rs,*.toml")));
    assert!(!matches_include("a.rs", Some("*.ts")));
    assert!(matches_include("anything.txt", None));
    assert!(matches_include("a.rs", Some(" *.rs , *.toml ")));
    // 花括号展开
    assert_eq!(
        expand_braces("*.{ts,tsx}"),
        vec!["*.ts".to_string(), "*.tsx".to_string()]
    );
    assert_eq!(expand_braces("*.rs"), vec!["*.rs".to_string()]);
    assert_eq!(
        expand_braces("prefix-{a,b}-suffix"),
        vec!["prefix-a-suffix".to_string(), "prefix-b-suffix".to_string()]
    );
    assert!(matches_include("App.tsx", Some("*.{ts,tsx}")));
    assert!(!matches_include("App.js", Some("*.{ts,tsx}")));
}
