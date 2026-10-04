use super::*;
use crate::permissions::PermissionsConfig;
use pipi_protocol::AbortSignal;
use std::sync::Arc;

async fn glob_fixture() -> (PathBuf, ToolContext, PathBuf) {
    let base = std::env::temp_dir().join(format!("pipi-glob-{}", pipi_test_support::new_id()));
    let workspace = base.join("workspace");
    tokio::fs::create_dir_all(workspace.join("src/deep"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(workspace.join(".hidden"))
        .await
        .unwrap();
    tokio::fs::write(workspace.join("src/a.rs"), "a")
        .await
        .unwrap();
    tokio::fs::write(workspace.join("src/deep/b.rs"), "b")
        .await
        .unwrap();
    tokio::fs::write(workspace.join("README.md"), "r")
        .await
        .unwrap();
    tokio::fs::write(workspace.join(".hidden/secret.rs"), "s")
        .await
        .unwrap();
    let outside = base.join("outside.rs");
    tokio::fs::write(&outside, "o").await.unwrap();

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

async fn glob(ctx: &ToolContext, args: Value) -> Result<ToolOutput, String> {
    GlobTool.execute(ctx, &args, &|_| {}).await
}

#[tokio::test]
async fn glob_matches_relative_pattern_and_skips_hidden() {
    let (base, ctx, _outside) = glob_fixture().await;
    let out = glob(&ctx, json!({ "pattern": "**/*.rs" })).await.unwrap();
    let ToolResultContent::Text { text } = &out.content[0] else {
        panic!("expected text");
    };
    assert!(text.contains("src/a.rs"));
    assert!(text.contains("src/deep/b.rs"));
    assert!(!text.contains("secret"));
    assert!(!text.contains("outside.rs"));
    tokio::fs::remove_dir_all(base).await.unwrap();
}

#[tokio::test]
async fn glob_single_star_does_not_cross_directories() {
    let (base, ctx, _outside) = glob_fixture().await;
    let out = glob(&ctx, json!({ "pattern": "src/*.rs" })).await.unwrap();
    let ToolResultContent::Text { text } = &out.content[0] else {
        panic!("expected text");
    };
    assert!(text.contains("src/a.rs"));
    assert!(!text.contains("src/deep/b.rs"));
    tokio::fs::remove_dir_all(base).await.unwrap();
}

#[tokio::test]
async fn glob_search_root_must_be_within_trusted_roots() {
    let (base, ctx, outside) = glob_fixture().await;
    // 显式指定工作目录外的根 → 拒绝
    let err = glob(
        &ctx,
        json!({ "pattern": "*.rs", "path": outside.parent().unwrap().join("..") }),
    )
    .await;
    // base 本身不在 workspace 内，直接给 base 也应拒绝
    assert!(err.is_err());
    let _ = glob(
        &ctx,
        json!({ "pattern": "*.rs", "path": base.to_string_lossy().into_owned() }),
    )
    .await
    .unwrap_err();
    tokio::fs::remove_dir_all(base).await.unwrap();
}

#[tokio::test]
async fn glob_no_match_reports_cleanly() {
    let (base, ctx, _outside) = glob_fixture().await;
    let out = glob(&ctx, json!({ "pattern": "**/*.nonexistent" }))
        .await
        .unwrap();
    let ToolResultContent::Text { text } = &out.content[0] else {
        panic!("expected text");
    };
    assert!(text.contains("No files found"));
    tokio::fs::remove_dir_all(base).await.unwrap();
}

// collect_matches 的修改时间排序（纯同步逻辑直接测）
#[test]
fn collect_matches_sorts_by_mtime_desc() {
    let dir = std::env::temp_dir().join(format!("pipi-glob-mtime-{}", pipi_test_support::new_id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/old.rs"), "o").unwrap();
    std::fs::write(dir.join("src/new.rs"), "n").unwrap();
    // 保证 mtime 有差异
    let old = SystemTime::now() - std::time::Duration::from_secs(60);
    let file = std::fs::File::options()
        .append(true)
        .open(dir.join("src/old.rs"))
        .unwrap();
    file.set_modified(old).unwrap();
    let pattern = Pattern::new("src/*.rs").unwrap();
    let matches = collect_matches(&dir, &pattern);
    assert_eq!(matches.len(), 2);
    assert!(matches[0].0.ends_with("new.rs"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn single_star_does_not_cross_separator() {
    let pattern = Pattern::new("src/*.rs").unwrap();
    assert!(pattern.matches_with("src/a.rs", MATCH_OPTIONS));
    assert!(!pattern.matches_with("src/deep/b.rs", MATCH_OPTIONS));
    let deep = Pattern::new("src/**/*.rs").unwrap();
    assert!(deep.matches_with("src/deep/b.rs", MATCH_OPTIONS));
    assert!(deep.matches_with("src/a.rs", MATCH_OPTIONS));
}
