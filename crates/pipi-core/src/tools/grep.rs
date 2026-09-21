//! grep 工具：正则搜索文件内容。Pipi 新增（pi 无此工具）。
//!
//! 只读：搜索范围受 `resolve_read_path` 的根集合约束（工作目录 +
//! 受信任读取根），防 `..` 与符号链接逃逸；输出复用 truncate 头部截断。

use std::path::Path;

use async_trait::async_trait;
use regex::{Regex, RegexBuilder};
use serde_json::{json, Value};
use walkdir::WalkDir;

use super::{resolve_read_path, AgentTool, ToolContext, ToolOutput};
use crate::truncate::{truncate_head, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
use crate::types::ToolResultContent;

/// 单次搜索的文件数上限（截断前的硬上限）。
const MAX_FILES: usize = 2000;
/// 单个文件的最大读取字节数（跳过超大文件，避免读入巨型日志）。
const MAX_FILE_BYTES: u64 = 1024 * 1024;

pub struct GrepTool;

/// 判断文件是否可能是二进制：含 NUL 字节即按二进制跳过（同 git grep 的启发式）。
fn looks_like_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

/// 编译正则；`case_insensitive` 时忽略大小写；非法正则返回错误。
fn compile_pattern(pattern: &str, case_insensitive: bool) -> Result<Regex, String> {
    RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .build()
        .map_err(|e| format!("非法正则表达式 {pattern:?}：{e}"))
}

/// 文件名是否匹配 include 模式。多模式以 `,` 分隔；支持 `*.{ts,tsx}` 式
/// 花括号展开（对齐 opencode 描述的用法习惯）。注意先展开括号再按逗号
/// 分割 —— 括号内本身可能含逗号。
fn matches_include(file_name: &str, include: Option<&str>) -> bool {
    let Some(spec) = include else {
        return true;
    };
    expand_braces(&spec.replace('\\', "/"))
        .into_iter()
        .flat_map(|part| {
            part.split(',')
                .map(|glob| expand_braces(glob.trim()))
                .collect::<Vec<_>>()
        })
        .filter(|patterns| patterns.iter().all(|p| !p.is_empty()))
        .any(|patterns| {
            patterns.iter().any(|pattern| {
                glob::Pattern::new(pattern)
                    .map(|p| p.matches_path(file_name.as_ref()))
                    .unwrap_or(false)
            })
        })
}

/// 展开 `*.{ts,tsx}` → `["*.ts", "*.tsx"]`。仅处理单组、无嵌套的花括号；
/// 其余输入原样返回。
fn expand_braces(spec: &str) -> Vec<String> {
    let (Some(open), Some(close)) = (spec.find('{'), spec.rfind('}')) else {
        return vec![spec.to_string()];
    };
    if close <= open + 1 || spec[open + 1..close].contains('{') {
        return vec![spec.to_string()];
    }
    spec[open + 1..close]
        .split(',')
        .map(|alternative| {
            format!(
                "{}{}{}",
                &spec[..open],
                alternative.trim(),
                &spec[close + 1..]
            )
        })
        .collect()
}

/// 在单文件中搜索，返回 `相对路径:行号: 行内容` 行列表与是否被跳过。
fn search_file(abs: &Path, display: &str, regex: &Regex) -> Result<(Vec<String>, usize), String> {
    let bytes = std::fs::read(abs).map_err(|e| format!("无法读取 {}: {e}", display))?;
    if looks_like_binary(&bytes) {
        return Ok((Vec::new(), 0));
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        if regex.is_match(line) {
            lines.push(format!("{}:{}: {}", display, i + 1, line.trim_end()));
        }
    }
    Ok((lines, 1))
}

/// 在 root 下遍历搜索。返回 (匹配行, 搜索的文件数)。
fn search_tree(
    root: &Path,
    workspace: &Path,
    regex: &Regex,
    include: Option<&str>,
) -> (Vec<String>, usize) {
    let mut output: Vec<String> = Vec::new();
    let mut searched = 0usize;
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if searched >= MAX_FILES {
            break;
        }
        let relative = match entry.path().strip_prefix(root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        if relative
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy();
        if !matches_include(&file_name, include) {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let display = entry
            .path()
            .strip_prefix(workspace)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        searched += 1;
        if let Ok((lines, _)) = search_file(entry.path(), &display, regex) {
            output.extend(lines);
        }
    }
    (output, searched)
}

#[async_trait]
impl AgentTool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> String {
        format!(
            "Search file contents with a regular expression. Returns matching lines as \"path:line: text\". Skips hidden files, binary files and files over {}KB. Use \"include\" to filter by filename glob (e.g. \"*.ts\" or \"*.rs,*.toml\"). Output is truncated to {DEFAULT_MAX_LINES} lines or {}KB.",
            MAX_FILE_BYTES / 1024,
            DEFAULT_MAX_BYTES / 1024
        )
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regular expression to search for (Rust regex syntax)" },
                "path": { "type": "string", "description": "Optional search root directory (relative or absolute, defaults to workspace)" },
                "include": { "type": "string", "description": "Optional filename glob filter, e.g. *.ts or *.rs,*.toml" },
                "case_insensitive": { "type": "boolean", "description": "Case-insensitive matching (default false)" }
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
        let pattern = args["pattern"].as_str().ok_or("缺少 pattern")?;
        let case_insensitive = args["case_insensitive"].as_bool().unwrap_or(false);
        let regex = compile_pattern(pattern, case_insensitive)?;
        let include = args["include"].as_str().map(|s| s.to_string());

        let root = match args["path"].as_str() {
            Some(p) if !p.is_empty() => resolve_read_path(&ctx.workspace, &ctx.read_roots, p)?,
            _ => std::fs::canonicalize(&ctx.workspace)
                .map_err(|e| format!("无法解析工作目录 {}：{e}", ctx.workspace.display()))?,
        };
        if !root.is_dir() {
            return Err(format!("搜索根 {} 不是目录", root.display()));
        }

        let workspace = ctx.workspace.clone();
        let (output, searched) = tokio::task::spawn_blocking(move || {
            search_tree(&root, &workspace, &regex, include.as_deref())
        })
        .await
        .map_err(|e| format!("grep 搜索失败：{e}"))?;

        let total_matches = output.len();
        let mut text = if total_matches == 0 {
            format!("No matches found for \"{pattern}\" (searched {searched} file(s)).")
        } else {
            output.join("\n")
        };

        let truncation = truncate_head(&text, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        let mut details: Option<Value> = None;
        if truncation.truncated {
            text = format!(
                "{}\n\n[Showing first {} of {total_matches} matches. Narrow the pattern or add an include filter.]",
                truncation.content, truncation.output_lines
            );
            details = Some(json!({ "truncation": { "truncated": true } }));
        }

        Ok(ToolOutput {
            content: vec![ToolResultContent::Text { text }],
            details,
            terminate: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::PermissionsConfig;
    use crate::types::AbortSignal;
    use std::path::PathBuf;
    use std::sync::Arc;

    async fn grep_fixture() -> (PathBuf, ToolContext, PathBuf) {
        let base = std::env::temp_dir().join(format!("pipi-grep-{}", crate::session::new_id()));
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
}
