use super::*;

fn edit(old: &str, new: &str) -> Edit {
    Edit {
        old_text: old.into(),
        new_text: new.into(),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_edit_blocks_symlink_to_outside_file() {
    let root = std::env::temp_dir().join(format!("pipi-edit-link-{}", pipi_test_support::new_id()));
    let workspace = root.join("workspace");
    let outside = root.join("outside.txt");
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    tokio::fs::write(&outside, "old").await.unwrap();
    std::os::unix::fs::symlink(&outside, workspace.join("link.txt")).unwrap();

    let ctx = ToolContext {
        workspace,
        memory_dir: None,
        read_roots: Vec::new(),
        permissions: std::sync::Arc::new(crate::permissions::PermissionsConfig::default()),
        sandbox: crate::permissions::SandboxMode::WorkspaceWrite,
        resolved_env: std::sync::Arc::new(std::collections::BTreeMap::new()),
        abort: pipi_protocol::AbortSignal::new(),
        approver: None,
    };
    let result = EditTool
        .execute(
            &ctx,
            &json!({
                "path": "link.txt",
                "edits": [{"oldText": "old", "newText": "changed"}]
            }),
            &|_| {},
        )
        .await;

    assert!(result.is_err());
    assert_eq!(tokio::fs::read_to_string(&outside).await.unwrap(), "old");
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[test]
fn applies_unique_replacement() {
    let out = apply_edits(
        "fn main() {\n    println!(\"hi\");\n}\n",
        &[edit("println!(\"hi\")", "println!(\"hello\")")],
        "m.rs",
    )
    .unwrap();
    assert!(out.contains("println!(\"hello\")"));
    assert!(!out.contains("\"hi\""));
}

#[test]
fn applies_multiple_sorted_edits() {
    let out = apply_edits("aaa bbb ccc", &[edit("bbb", "B"), edit("aaa", "A")], "f").unwrap();
    assert_eq!(out, "A B ccc");
}

#[test]
fn rejects_missing_and_ambiguous() {
    assert!(apply_edits("abc", &[edit("xyz", "1")], "f").is_err());
    assert!(apply_edits("abab", &[edit("ab", "1")], "f").is_err());
}

#[test]
fn rejects_overlapping_edits() {
    // 两个 edit 的匹配位置分别为 0..3 和 2..5，重叠
    let err = apply_edits("abcde", &[edit("abc", "X"), edit("cde", "Y")], "f");
    assert!(err.is_err());
}

#[test]
fn line_ending_and_bom_helpers() {
    assert_eq!(detect_line_ending("a\r\nb"), "\r\n");
    assert_eq!(detect_line_ending("a\nb"), "\n");
    assert_eq!(normalize_to_lf("a\r\nb"), "a\nb");
    assert_eq!(restore_line_endings("a\nb".into(), "\r\n"), "a\r\nb");
    let (bom, rest) = strip_bom("\u{FEFF}hi");
    assert!(bom);
    assert_eq!(rest, "hi");
}

#[test]
fn diff_summary_reports_first_changed_line() {
    let (diff, first) = diff_summary("a\nb\nc", "a\nB\nc");
    assert_eq!(first, Some(2));
    assert!(diff.contains("-b"));
    assert!(diff.contains("+B"));
}
