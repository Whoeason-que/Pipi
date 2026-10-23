//! `pipi-tools` 的 glob 工具：按模式匹配文件路径。Pipi 新增（pi 无此工具）。
//!
//! 只读：遍历范围受 `resolve_read_path` 的根集合约束（工作目录 +
//! 受信任读取根），防 `..` 与符号链接逃逸；遍历骨架与输出截断整形在
//! [`crate::walk`]（与 grep 共用）。

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use glob::Pattern;
use serde_json::{Value, json};

use crate::walk::{resolve_search_root, run_walk, truncate_matches, walk_visible_files};
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput};

/// 单次列出的路径上限（截断前的硬上限，对齐 opencode 的 limit=100：
/// 结果太多会刷爆上下文，模型应改用更精确的 pattern）。
const MAX_MATCHES: usize = 100;

/// glob 匹配选项：`*` 不跨 `/`（`**` 才跨），对齐 shell 直觉。
const MATCH_OPTIONS: glob::MatchOptions = glob::MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: true,
};

pub struct GlobTool;

/// 在 `root` 下遍历并收集匹配 `pattern` 的文件路径（相对 root 返回）。
/// pattern 匹配的是相对路径；`**` 可跨目录。跳过点目录与符号链接
/// （过滤在 [`walk_visible_files`]）。
fn collect_matches(root: &Path, pattern: &Pattern) -> Vec<(PathBuf, SystemTime)> {
    let mut matches: Vec<(PathBuf, SystemTime)> = Vec::new();
    walk_visible_files(root, |entry, relative| {
        if !pattern.matches_with(relative, MATCH_OPTIONS) {
            return ControlFlow::Continue(());
        }
        let modified = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        matches.push((entry.path().to_path_buf(), modified));
        // 达到上限立即停走
        if matches.len() >= MAX_MATCHES {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    // 最近修改的排前面，方便模型优先看活跃文件
    matches.sort_by_key(|&(_, modified)| std::cmp::Reverse(modified));
    matches
}

#[async_trait]
impl AgentTool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> String {
        "Fast file pattern matching. Supports glob patterns like \"src/**/*.ts\" or \"**/*.md\". Returns matching file paths sorted by modification time (newest first). Hidden files/directories (dot-prefixed) are skipped.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern relative to the search root, e.g. src/**/*.rs" },
                "path": { "type": "string", "description": "Optional search root directory (relative or absolute, defaults to workspace)" }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or("Missing pattern")?
            .trim()
            .to_string();
        if pattern.is_empty() {
            return Err("pattern cannot be empty".into());
        }
        // pattern 里的反斜杠统一按分隔符处理（Windows 路径输入也友好）
        let pattern = pattern.replace('\\', "/");
        let compiled =
            Pattern::new(&pattern).map_err(|e| format!("Invalid glob pattern {pattern:?}: {e}"))?;

        // 搜索根：显式 path 或 workspace，两者都必须在受信任根集合内
        let root = resolve_search_root(ctx, args["path"].as_str())?;

        let matches = run_walk("glob traversal", move || collect_matches(&root, &compiled)).await?;

        let total = matches.len();
        let text = if total == 0 {
            format!("No files found matching pattern \"{pattern}\".")
        } else {
            let lines: Vec<String> = matches
                .iter()
                .map(|(path, _)| {
                    path.strip_prefix(&ctx.workspace)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .replace('\\', "/")
                })
                .collect();
            format!("Found {} file(s):\n{}", lines.len(), lines.join("\n"))
        };

        Ok(truncate_matches(
            text,
            total,
            "Use a more specific pattern.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::AbortSignal;
    use pipi_protocol::ToolResultContent;
    use pipi_protocol::permissions::PermissionsConfig;
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
            sandbox: pipi_protocol::permissions::SandboxMode::DangerFullAccess,
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
        let dir =
            std::env::temp_dir().join(format!("pipi-glob-mtime-{}", pipi_test_support::new_id()));
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
}
