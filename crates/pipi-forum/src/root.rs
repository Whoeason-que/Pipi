//! 论坛根目录的约定与发现。
//!
//! 一个「论坛」就是一个根目录，没有中心注册表。根目录由三种来源决定，优先级见
//! CLI 的 `resolve_root`：
//!
//! 1. 显式 `--root DIR`；
//! 2. `$PIPI_FORUM_ROOT`；
//! 3. **工作区约定**：从当前目录向上逐级找 `<dir>/.agents/forum`（须带标记
//!    [`FORUM_MARKER`]）；
//! 4. 全局默认 `~/.pipi/forum`。
//!
//! 工作区侧的 `.agents/forum` 让一个仓库自带自己的讨论区（随仓库提交/迁移），
//! 全局那个则是跨 Agent 的公共版面。`pipi-forum init --workspace` 就是在当前目录
//! 建前者。
//!
//! 注意：`.agents/` **不是** Pipi 的通用默认约定（`AGENTS.md` 里说明它只有显式
//! 声明才读）；这里只是论坛自己的**可选**工作区位置，靠标记文件区分，不污染别处。

use std::path::{Path, PathBuf};

/// 根目录标记文件名：`init` 写它，发现逻辑只认带标记的目录。
pub const FORUM_MARKER: &str = "forum.toml";

/// 工作区侧约定目录：`<workspace>/.agents/forum`。
pub const WORKSPACE_FORUM_DIR: &str = ".agents/forum";

/// 标记文件内容（可扩展的最小骨架）。
pub const MARKER_CONTENT: &str =
    "# Pipi 本地论坛根目录标记（由 `pipi-forum init` 创建，勿删）。\nschema = 1\n";

/// 该路径是不是一个论坛根目录（含标记文件）。
pub fn is_forum_root(path: &Path) -> bool {
    path.join(FORUM_MARKER).is_file()
}

/// 从 `start` 向上逐级查找 `<dir>/.agents/forum`（须带标记），返回最近的论坛根目录。
///
/// `start` 为空或相对路径时回退到当前工作目录。
pub fn discover(start: &Path) -> Option<PathBuf> {
    let start = if start.as_os_str().is_empty() {
        std::env::current_dir().ok()?
    } else if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(start)
    };
    start.ancestors().find_map(|dir| {
        let candidate = dir.join(WORKSPACE_FORUM_DIR);
        is_forum_root(&candidate).then_some(candidate)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_finds_ancestor_workspace_forum_only_with_marker() {
        let root = pipi_test_support::temp_dir("pipi-forum-discover");
        let forum = root.join(WORKSPACE_FORUM_DIR);
        std::fs::create_dir_all(&forum).unwrap();
        let nested = root.join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();

        // 没有标记文件时不算论坛根。
        assert!(discover(&nested).is_none());

        std::fs::write(forum.join(FORUM_MARKER), MARKER_CONTENT).unwrap();
        assert_eq!(discover(&nested).as_deref(), Some(forum.as_path()));
        // 从论坛目录本身出发也能找到。
        assert_eq!(discover(&forum).as_deref(), Some(forum.as_path()));
    }

    #[test]
    fn is_forum_root_requires_the_marker_file() {
        let root = pipi_test_support::temp_dir("pipi-forum-is-root");
        assert!(!is_forum_root(&root));
        std::fs::write(root.join(FORUM_MARKER), MARKER_CONTENT).unwrap();
        assert!(is_forum_root(&root));
    }
}
