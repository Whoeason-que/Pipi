//! Pi-compatible project context resource loading.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::ContextFile;

/// Candidate context file names, in Pi's precedence order.
pub const CONTEXT_FILE_CANDIDATES: [&str; 5] = [
    "AGENTS.override.md",
    "AGENTS.md",
    "AGENTS.MD",
    "CLAUDE.md",
    "CLAUDE.MD",
];

/// Load Agent-global context followed by project context at the default budget.
pub fn load_project_context_files(agent_dir: Option<&Path>, cwd: &Path) -> Vec<ContextFile> {
    load_project_context_files_with_budget(agent_dir, cwd, crate::DEFAULT_PROJECT_DOC_MAX_BYTES)
}

/// Load Agent-global context followed by project context with an explicit project budget.
///
/// The byte budget applies only to project files. Agent-global context is loaded first and
/// remains unbounded, matching the separation between the two resource scopes.
pub fn load_project_context_files_with_budget(
    agent_dir: Option<&Path>,
    cwd: &Path,
    project_max_bytes: usize,
) -> Vec<ContextFile> {
    let mut loaded = Vec::new();
    let mut seen = HashSet::new();

    if let Some(agent_dir) = agent_dir {
        load_directory_context(agent_dir, None, &mut seen, &mut loaded);
    }
    loaded.extend(load_project_scope_files_with_budget(cwd, project_max_bytes));
    loaded
}

/// Load only the project scope (root → cwd chain) with an explicit byte budget.
///
/// 供 av 契约的 instructions 三态使用：声明层替换项目发现时，缺省路径仍走这里。
pub fn load_project_scope_files(cwd: &Path, project_max_bytes: usize) -> Vec<ContextFile> {
    load_project_scope_files_with_budget(cwd, project_max_bytes)
}

fn load_project_scope_files_with_budget(cwd: &Path, project_max_bytes: usize) -> Vec<ContextFile> {
    let mut loaded = Vec::new();
    let mut seen = HashSet::new();

    let (project_root, directories) = project_directories(cwd);
    let Some(project_root) = project_root else {
        return loaded;
    };
    let Ok(canonical_project_root) = fs::canonicalize(&project_root) else {
        return loaded;
    };

    let mut remaining = project_max_bytes;
    for directory in directories {
        if remaining == 0 {
            break;
        }
        load_directory_context(
            &directory,
            Some((&canonical_project_root, &mut remaining)),
            &mut seen,
            &mut loaded,
        );
    }
    loaded
}

/// Load only Agent-global context for agents that do not have a workspace.
pub fn load_agent_context_files(agent_dir: Option<&Path>) -> Vec<ContextFile> {
    let Some(agent_dir) = agent_dir else {
        return Vec::new();
    };
    let mut loaded = Vec::new();
    load_directory_context(agent_dir, None, &mut HashSet::new(), &mut loaded);
    loaded
}

fn project_directories(cwd: &Path) -> (Option<PathBuf>, Vec<PathBuf>) {
    let root = crate::find_project_root(cwd);
    let Some(root) = root else {
        return (Some(cwd.to_path_buf()), vec![cwd.to_path_buf()]);
    };

    let mut directories = Vec::new();
    let mut cursor = cwd.to_path_buf();
    loop {
        directories.push(cursor.clone());
        if cursor == root {
            break;
        }
        let Some(parent) = cursor.parent() else {
            return (None, Vec::new());
        };
        cursor = parent.to_path_buf();
    }
    directories.reverse();
    (Some(root), directories)
}

fn load_directory_context(
    directory: &Path,
    project_scope: Option<(&Path, &mut usize)>,
    seen: &mut HashSet<PathBuf>,
    loaded: &mut Vec<ContextFile>,
) {
    let canonical_allowed_root = match project_scope {
        Some((root, _)) => root.to_path_buf(),
        None => match fs::canonicalize(directory) {
            Ok(root) => root,
            Err(_) => return,
        },
    };

    for name in CONTEXT_FILE_CANDIDATES {
        let candidate = directory.join(name);
        let Ok(canonical_path) = fs::canonicalize(&candidate) else {
            continue;
        };
        if !canonical_path.starts_with(&canonical_allowed_root) {
            continue;
        }
        let Ok(metadata) = fs::metadata(&canonical_path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(bytes) = fs::read(&canonical_path) else {
            continue;
        };
        let Ok(mut content) = String::from_utf8(bytes) else {
            continue;
        };
        if let Some(stripped) = content.strip_prefix('\u{feff}') {
            content = stripped.to_string();
        }
        if content.trim().is_empty() {
            continue;
        }
        if !seen.insert(canonical_path) {
            continue;
        }

        if let Some((_, remaining)) = project_scope {
            if *remaining == 0 {
                return;
            }
            let content = truncate_to_char_boundary(&content, *remaining);
            if content.is_empty() {
                return;
            }
            *remaining = remaining.saturating_sub(content.len());
            loaded.push(ContextFile {
                path: candidate.display().to_string(),
                content,
            });
        } else {
            loaded.push(ContextFile {
                path: candidate.display().to_string(),
                content,
            });
        }
        break;
    }
}

/// 在 UTF-8 字符边界上截断（声明式上下文文件与发现路径共用）。
pub fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> String {
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
#[path = "resources_tests.rs"]
mod tests;
