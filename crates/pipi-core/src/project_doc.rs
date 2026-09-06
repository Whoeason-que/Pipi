//! 项目文档发现 —— 移植自 openai/codex `core/src/agents_md.rs`（Apache-2.0）。
//!
//! codex 的语义：从工作目录向上找到项目根（含 `.git` 等标记的最近祖先），
//! 收集从根到工作目录每一层的 AGENTS.md，按根→近的顺序拼接，受字节预算
//! 限制。这样 monorepo 里的根级说明和子目录说明都能进上下文。
//!
//! 简化：候选文件名只保留 `AGENTS.md`（上游还支持 CLAUDE.md 等回退名与
//! 全局用户级文件，后者由 Agent 自己的 AGENTS.md 承担）。

use std::fs;
use std::path::{Path, PathBuf};

pub const AGENTS_MD_FILENAME: &str = "AGENTS.md";

/// 单个项目文档的字节预算（对齐 codex 的 project_doc_max_bytes 默认值）。
pub const DEFAULT_PROJECT_DOC_MAX_BYTES: usize = 16 * 1024;

/// 从 start 向上找最近的项目根（含 `.git` 的目录）；找不到返回 None，
/// 此时只收集 start 本身的 AGENTS.md。
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cursor = start;
    loop {
        if cursor.join(".git").exists() {
            return Some(cursor.to_path_buf());
        }
        cursor = cursor.parent()?;
    }
}

/// 收集从项目根到 workspace 每一层的 AGENTS.md，按根→近排序。
/// 超出 max_bytes 的文档截断（单文档截断 + 总预算）。
pub fn collect_project_docs(workspace: &Path, max_bytes: usize) -> Vec<(PathBuf, String)> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut cursor = workspace.to_path_buf();
    loop {
        dirs.push(cursor.clone());
        match cursor.parent() {
            Some(parent) => cursor = parent.to_path_buf(),
            None => break,
        }
    }

    // 根据项目根把搜索范围截断到 [root, workspace]；无根时只看 workspace
    let root = find_project_root(workspace);
    if let Some(root) = &root {
        dirs.retain(|d| d.starts_with(root));
    } else {
        dirs.truncate(1);
    }
    dirs.reverse(); // 根 → 近

    let mut docs = Vec::new();
    let mut remaining = max_bytes;
    for dir in dirs {
        if remaining == 0 {
            break;
        }
        let candidate = dir.join(AGENTS_MD_FILENAME);
        if !candidate.is_file() {
            continue;
        }
        let Ok(text) = fs::read_to_string(&candidate) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let taken: String = if text.len() > remaining {
            truncate_to_char_boundary(&text, remaining)
        } else {
            text.clone()
        };
        remaining = remaining.saturating_sub(taken.len());
        docs.push((candidate, taken));
    }
    docs
}

/// 在 UTF-8 字符边界上截断。
fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> String {
    if max_bytes >= s.len() {
        return s.to_string();
    }
    let mut idx = max_bytes;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    s[..idx].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::new_id;

    fn temp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pipi-docs-{}", new_id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn collects_from_root_to_workspace() {
        let root = temp_root();
        fs::create_dir_all(root.join(".git")).unwrap();
        let sub = root.join("crates/app");
        fs::create_dir_all(&sub).unwrap();
        fs::write(root.join("AGENTS.md"), "root rules").unwrap();
        fs::write(sub.join("AGENTS.md"), "app rules").unwrap();

        let docs = collect_project_docs(&sub, DEFAULT_PROJECT_DOC_MAX_BYTES);
        assert_eq!(docs.len(), 2);
        assert!(docs[0].0.ends_with("AGENTS.md"));
        assert_eq!(docs[0].1, "root rules");
        assert_eq!(docs[1].1, "app rules");
    }

    #[test]
    fn no_git_marker_only_workspace_doc() {
        let root = temp_root();
        let sub = root.join("deep/nested");
        fs::create_dir_all(&sub).unwrap();
        fs::write(root.join("AGENTS.md"), "should be ignored").unwrap();
        fs::write(sub.join("AGENTS.md"), "local only").unwrap();

        let docs = collect_project_docs(&sub, DEFAULT_PROJECT_DOC_MAX_BYTES);
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].1, "local only");
    }

    #[test]
    fn byte_budget_truncates() {
        let root = temp_root();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("AGENTS.md"), "x".repeat(5000)).unwrap();
        let docs = collect_project_docs(&root, 100);
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].1.len(), 100);
    }

    #[test]
    fn char_boundary_safe_truncation() {
        let s = "中文内容测试".repeat(100);
        let cut = truncate_to_char_boundary(&s, 7);
        assert!(s.starts_with(&cut));
        // 截断点必须是字符边界
        assert!(cut.chars().count() > 0);
    }
}
