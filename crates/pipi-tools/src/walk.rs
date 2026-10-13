//! glob / grep 共用的遍历栈：搜索根解析、隐藏项过滤、文件遍历、阻塞调度与
//! 结果截断整形。
//!
//! 两个工具都只读、遍历范围受 `resolve_read_path` 的根集合约束（工作目录 +
//! 受信任读取根），防 `..` 与符号链接逃逸。改遍历语义（点文件约定、符号链接
//! 策略、截断提示的形状）只改这里；两个工具的措辞差异（截断建议、无结果文案）
//! 由调用方传入或留在各自的 `execute`。

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use serde_json::json;
use walkdir::{DirEntry, WalkDir};

use crate::truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, truncate_head};
use pipi_protocol::ToolResultContent;
use pipi_protocol::tools::{ToolContext, ToolOutput, resolve_read_path};

/// 解析搜索根：显式 `path` 必须落在受信任根集合内，缺省用工作目录；
/// 结果必须是目录（搜索根是文件时两个工具都按同一文案报错）。
pub(crate) fn resolve_search_root(
    ctx: &ToolContext,
    path: Option<&str>,
) -> Result<PathBuf, String> {
    let root = match path {
        Some(p) if !p.is_empty() => resolve_read_path(&ctx.workspace, &ctx.read_roots, p)?,
        _ => std::fs::canonicalize(&ctx.workspace)
            .map_err(|e| format!("无法解析工作目录 {}：{e}", ctx.workspace.display()))?,
    };
    if !root.is_dir() {
        return Err(format!("搜索根 {} 不是目录", root.display()));
    }
    Ok(root)
}

/// 遍历 `root` 下的普通文件：不跟随符号链接，跳过点目录与点文件，逐个把
/// 「条目 + 相对 `root` 的路径（`/` 分隔）」交给 `visit`；`visit` 返回
/// [`ControlFlow::Break`] 时提前停止（各工具的条数上限用它实现）。
pub(crate) fn walk_visible_files(
    root: &Path,
    mut visit: impl FnMut(&DirEntry, &str) -> ControlFlow<()>,
) {
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        if is_hidden(relative) {
            continue;
        }
        let relative = relative.to_string_lossy().replace('\\', "/");
        if visit(&entry, &relative).is_break() {
            return;
        }
    }
}

/// 相对路径落在点目录里或本身就是点文件（点前缀默认跳过）。
fn is_hidden(relative: &Path) -> bool {
    relative
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
}

/// 把一次同步遍历放到阻塞线程池执行（目录 IO 不上事件循环）；`label` 用于
/// 错误文案（`glob 遍历` → `glob 遍历失败：…`）。
pub(crate) async fn run_walk<F, T>(label: &str, walk: F) -> Result<T, String>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(walk)
        .await
        .map_err(|e| format!("{label}失败：{e}"))
}

/// 结果截断：超限时保留头部并附「Showing first N of M matches」与调用方给的
/// 收窄建议（两个工具的措辞不同，逐字保持），`details` 标记 truncation。
pub(crate) fn truncate_matches(text: String, total: usize, advice: &str) -> ToolOutput {
    let truncation = truncate_head(&text, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
    if !truncation.truncated {
        return ToolOutput::text(text);
    }
    ToolOutput {
        content: vec![ToolResultContent::Text {
            text: format!(
                "{}\n\n[Showing first {} of {total} matches. {advice}]",
                truncation.content, truncation.output_lines
            ),
        }],
        details: Some(json!({ "truncation": { "truncated": true } })),
        terminate: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_relative_paths_are_detected() {
        assert!(is_hidden(Path::new(".git/config")));
        assert!(is_hidden(Path::new("src/.env")));
        assert!(!is_hidden(Path::new("src/main.rs")));
    }

    #[test]
    fn walk_skips_hidden_files_and_stops_on_break() {
        let dir = pipi_test_support::temp_dir("pipi-walk");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join(".hidden.txt"), "h").unwrap();
        std::fs::write(dir.join("sub/b.txt"), "b").unwrap();
        std::fs::write(dir.join("sub/.hidden.txt"), "h").unwrap();

        let mut visited: Vec<String> = Vec::new();
        walk_visible_files(&dir, |_, relative| {
            visited.push(relative.to_string());
            ControlFlow::Continue(())
        });
        visited.sort();
        assert_eq!(visited, vec!["a.txt", "sub/b.txt"]);

        // Break 立即停止遍历
        let mut seen = 0usize;
        walk_visible_files(&dir, |_, _| {
            seen += 1;
            ControlFlow::Break(())
        });
        assert_eq!(seen, 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn truncate_matches_keeps_head_and_appends_advice() {
        let long = (1..=DEFAULT_MAX_LINES + 10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = truncate_matches(long, 2500, "Use a more specific pattern.");
        let ToolResultContent::Text { text } = &out.content[0] else {
            panic!("expected text");
        };
        assert!(
            text.ends_with("[Showing first 2000 of 2500 matches. Use a more specific pattern.]"),
            "{text:?}"
        );
        assert!(out.details.is_some());

        // 未超限：原文返回，无 details
        let out = truncate_matches("short".to_string(), 1, "advice");
        let ToolResultContent::Text { text } = &out.content[0] else {
            panic!("expected text");
        };
        assert_eq!(text, "short");
        assert!(out.details.is_none());
    }
}
