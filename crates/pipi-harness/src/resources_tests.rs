use super::*;
use crate::ContextFile;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

fn temp_root(label: &str) -> PathBuf {
    let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("pipi-context-{label}-{}-{id}", std::process::id()));
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
    std::os::windows::fs::symlink_file(project.join("AGENTS.md"), cwd.join("AGENTS.md")).unwrap();

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
    std::os::unix::fs::symlink(project.join("AGENTS.md"), cwd.join("AGENTS.override.md")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(project.join("AGENTS.md"), cwd.join("AGENTS.override.md"))
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
