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
    load_project_context_files_with_budget(
        agent_dir,
        cwd,
        crate::project_doc::DEFAULT_PROJECT_DOC_MAX_BYTES,
    )
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
    let root = crate::project_doc::find_project_root(cwd);
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
    use crate::harness::ContextFile;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn temp_root(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("pipi-context-{label}-{}", crate::session::new_id()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    fn contents(files: &[ContextFile]) -> Vec<&str> {
        files.iter().map(|file| file.content.as_str()).collect()
    }

    #[test]
    fn candidate_precedence_includes_override_and_case_variants() {
        let root = temp_root("precedence");
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(project.join(".git")).unwrap();

        fs::write(project.join("AGENTS.override.md"), "override").unwrap();
        fs::write(project.join("AGENTS.md"), "agents").unwrap();
        fs::write(project.join("AGENTS.MD"), "agents-upper").unwrap();
        fs::write(project.join("CLAUDE.md"), "claude").unwrap();
        fs::write(project.join("CLAUDE.MD"), "claude-upper").unwrap();

        let files = load_project_context_files(None, &project);

        assert_eq!(contents(&files), vec!["override"]);
        assert!(files[0].path.ends_with("AGENTS.override.md"));
        cleanup(&root);
    }

    #[test]
    fn non_file_candidate_falls_through_to_case_variant() {
        let root = temp_root("fallback");
        let project = root.join("project");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(project.join("AGENTS.override.md")).unwrap();
        fs::write(project.join("AGENTS.md"), "agents").unwrap();
        fs::write(project.join("AGENTS.MD"), "agents-upper").unwrap();

        let files = load_project_context_files(None, &project);

        assert_eq!(contents(&files), vec!["agents"]);
        assert!(files[0].path.ends_with("AGENTS.md"));
        cleanup(&root);
    }

    #[test]
    fn global_context_is_first_then_project_root_to_cwd() {
        let root = temp_root("ordering");
        let agent = root.join("agent");
        let project = root.join("project");
        let cwd = project.join("crates/app");
        fs::create_dir_all(&agent).unwrap();
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(&cwd).unwrap();

        fs::write(agent.join("AGENTS.md"), "global").unwrap();
        fs::write(project.join("AGENTS.md"), "project-root").unwrap();
        fs::write(cwd.join("AGENTS.md"), "cwd").unwrap();

        let files = load_project_context_files(Some(&agent), &cwd);

        assert_eq!(contents(&files), vec!["global", "project-root", "cwd"]);
        cleanup(&root);
    }

    #[test]
    fn canonical_duplicate_is_suppressed() {
        let root = temp_root("dedupe");
        let project = root.join("project");
        let cwd = project.join("nested");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(project.join("AGENTS.md"), "one").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(project.join("AGENTS.md"), cwd.join("AGENTS.md")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(project.join("AGENTS.md"), cwd.join("AGENTS.md"))
            .unwrap();

        let files = load_project_context_files(None, &cwd);

        assert_eq!(contents(&files), vec!["one"]);
        cleanup(&root);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn duplicate_higher_precedence_candidate_falls_through_to_distinct_candidate() {
        let root = temp_root("duplicate-fallback");
        let project = root.join("project");
        let cwd = project.join("nested");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(project.join("AGENTS.md"), "ancestor").unwrap();
        fs::write(cwd.join("AGENTS.md"), "cwd").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(project.join("AGENTS.md"), cwd.join("AGENTS.override.md"))
            .unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(
            project.join("AGENTS.md"),
            cwd.join("AGENTS.override.md"),
        )
        .unwrap();

        let files = load_project_context_files(None, &cwd);

        assert_eq!(contents(&files), vec!["ancestor", "cwd"]);
        cleanup(&root);
    }

    #[test]
    fn strips_utf8_bom_before_returning_content() {
        let root = temp_root("bom");
        let project = root.join("project");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::write(project.join("AGENTS.md"), b"\xef\xbb\xbfproject rules").unwrap();

        let files = load_project_context_files(None, &project);

        assert_eq!(contents(&files), vec!["project rules"]);
        cleanup(&root);
    }

    #[test]
    fn project_context_obeys_total_utf8_safe_byte_budget() {
        let root = temp_root("budget");
        let project = root.join("project");
        let cwd = project.join("nested");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(project.join("AGENTS.md"), "根规则").unwrap();
        fs::write(cwd.join("AGENTS.md"), "cwd rules").unwrap();

        let files = load_project_context_files_with_budget(None, &cwd, 10);

        assert_eq!(contents(&files), vec!["根规则", "c"]);
        assert!(files[1].content.is_char_boundary(files[1].content.len()));
        assert!(files.iter().map(|file| file.content.len()).sum::<usize>() <= 10);
        cleanup(&root);
    }

    #[test]
    fn project_context_skips_empty_utf8_truncation_without_blocking_later_files() {
        let root = temp_root("empty-truncation");
        let project = root.join("project");
        let cwd = project.join("nested");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(project.join("AGENTS.md"), "根规则").unwrap();
        fs::write(cwd.join("AGENTS.md"), "c").unwrap();

        let files = load_project_context_files_with_budget(None, &cwd, 1);

        assert_eq!(contents(&files), vec!["c"]);
        assert!(files.iter().all(|file| !file.content.is_empty()));
        cleanup(&root);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn rejects_project_candidate_symlink_outside_project_root() {
        let root = temp_root("external-link");
        let project = root.join("project");
        let external = root.join("external");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("AGENTS.override.md"), "external").unwrap();
        fs::write(project.join("AGENTS.md"), "safe fallback").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(
            external.join("AGENTS.override.md"),
            project.join("AGENTS.override.md"),
        )
        .unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(
            external.join("AGENTS.override.md"),
            project.join("AGENTS.override.md"),
        )
        .unwrap();

        let files = load_project_context_files(None, &project);

        assert_eq!(contents(&files), vec!["safe fallback"]);
        cleanup(&root);
    }
}
