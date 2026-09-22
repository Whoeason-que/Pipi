//! 项目文档发现 —— 移植自 openai/codex `core/src/agents_md.rs`（Apache-2.0）。
//!
//! `harness::resources` 负责当前运行时的完整 context candidate 发现：
//! Agent 全局文件、AGENTS/CLAUDE 候选名、根→近顺序、去重与预算。
//! 本模块保留 Codex 风格的 project-doc facade，供旧调用方继续获得项目层文档。
//!
//! 兼容 facade 仍以 `AGENTS.md` 作为公开常量和返回路径语义；新的 harness
//! prompt 构建路径不再直接依赖这里的旧拼装逻辑。

use std::path::{Path, PathBuf};

pub const AGENTS_MD_FILENAME: &str = "AGENTS.md";

pub use pipi_harness::{find_project_root, DEFAULT_PROJECT_DOC_MAX_BYTES};

/// 兼容收集 API：委托给 harness resource loader，按 Agent 全局 context、
/// 项目根到 cwd 的顺序返回 context 文件；`max_bytes` 仅限制项目资源。
pub fn collect_project_docs(workspace: &Path, max_bytes: usize) -> Vec<(PathBuf, String)> {
    crate::harness::resources::load_project_context_files_with_budget(None, workspace, max_bytes)
        .into_iter()
        .map(|file| (PathBuf::from(file.path), file.content))
        .collect()
}

/// 在 UTF-8 字符边界上截断。
#[cfg(test)]
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
    use std::fs;

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
