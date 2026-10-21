//! 走公开命令入口验证路径边界、普通/未出生仓库、删除/特殊文件名与子工作区。
use std::{ffi::OsString, path::Path};

use pipi_core::commands::{
    self, ListWorkspaceFilesArgs, PreviewWorkspaceFileArgs, WorkspaceChangesArgs, WorkspaceDiffArgs,
};
use pipi_test_support::{HOME_LOCK, remove_env_var, set_env_var, temp_dir};

struct Environment(Vec<(&'static str, Option<OsString>)>);
impl Environment {
    fn isolated(home: &Path) -> Self {
        let saved = ["HOME", "PIPI_HOME", "AV_HOME"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        set_env_var("HOME", home);
        set_env_var("PIPI_HOME", home.join(".pipi"));
        set_env_var("AV_HOME", home.join(".av"));
        Self(saved)
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            match value {
                Some(value) => set_env_var(key, value),
                None => remove_env_var(key),
            }
        }
    }
}
fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
async fn patch(path: &str) -> pipi_core::workspace::WorkspaceDiff {
    commands::workspace_diff(WorkspaceDiffArgs {
        agent_name: "preview".into(),
        path: path.into(),
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn workspace_commands_cover_file_preview_and_git_lifecycle_without_changing_files() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = temp_dir("workspace-command-home");
    let _env = Environment::isolated(&home);
    let root = home.join("project");
    std::fs::create_dir_all(&root).unwrap();
    pipi_venv::agents::create_agent(
        "preview",
        "test",
        Some(root.to_str().unwrap()),
        None,
        None,
        None,
    )
    .unwrap();
    std::fs::write(root.join("中文 file.txt"), "original\n").unwrap();
    let listing = commands::list_workspace_files(ListWorkspaceFilesArgs {
        agent_name: "preview".into(),
        path: "".into(),
    })
    .await
    .unwrap();
    assert!(
        listing
            .entries
            .iter()
            .any(|item| item.path == "中文 file.txt")
    );
    let file = commands::preview_workspace_file(PreviewWorkspaceFileArgs {
        agent_name: "preview".into(),
        path: "中文 file.txt".into(),
    })
    .await
    .unwrap();
    assert_eq!(file.content.as_deref(), Some("original\n"));
    assert_eq!(file.version.len(), 64);
    for path in ["../outside", ".git/config", "/etc/passwd"] {
        assert!(
            commands::preview_workspace_file(PreviewWorkspaceFileArgs {
                agent_name: "preview".into(),
                path: path.into()
            })
            .await
            .is_err()
        );
        assert!(
            commands::workspace_diff(WorkspaceDiffArgs {
                agent_name: "preview".into(),
                path: path.into()
            })
            .await
            .is_err()
        );
    }
    assert!(
        !commands::workspace_changes(WorkspaceChangesArgs {
            agent_name: "preview".into()
        })
        .await
        .unwrap()
        .is_repo
    );
    git(&root, &["init"]);
    git(&root, &["config", "user.name", "test"]);
    git(&root, &["config", "user.email", "test@example.invalid"]);
    assert_eq!(patch("中文 file.txt").await.base, "untracked");
    git(&root, &["add", "中文 file.txt"]);
    assert!(patch("中文 file.txt").await.patch.contains("+original"));
    git(&root, &["commit", "-m", "initial"]);
    std::fs::write(root.join("中文 file.txt"), "staged\n").unwrap();
    git(&root, &["add", "中文 file.txt"]);
    std::fs::write(root.join("中文 file.txt"), "current\n").unwrap();
    let diff = patch("中文 file.txt").await;
    assert_eq!(diff.base, "HEAD");
    assert!(diff.patch.contains("-original"));
    assert!(diff.patch.contains("+current"));
    assert!(!diff.patch.contains("+staged"));
    std::fs::write(root.join("--odd\nfile.txt"), "new\n").unwrap();
    std::fs::remove_file(root.join("中文 file.txt")).unwrap();
    let changes = commands::workspace_changes(WorkspaceChangesArgs {
        agent_name: "preview".into(),
    })
    .await
    .unwrap();
    assert!(
        changes
            .files
            .iter()
            .any(|item| item.path == "--odd\nfile.txt")
    );
    assert!(
        changes
            .files
            .iter()
            .any(|item| item.path == "中文 file.txt" && item.status.contains('D'))
    );
    assert!(patch("中文 file.txt").await.patch.contains("-original"));
    assert!(patch("--odd\nfile.txt").await.patch.contains("+new"));
    // Agent 指向子目录时，不能把父仓库其余文件列进来。
    std::fs::create_dir(root.join("nested")).unwrap();
    std::fs::write(root.join("nested/inside.txt"), "inside\n").unwrap();
    let mut def = pipi_venv::agents::load_agent("preview").unwrap();
    def.workspace = Some(root.join("nested").to_string_lossy().into_owned());
    pipi_venv::agents::save_agent(&def).unwrap();
    let changes = commands::workspace_changes(WorkspaceChangesArgs {
        agent_name: "preview".into(),
    })
    .await
    .unwrap();
    assert_eq!(changes.files.len(), 1);
    assert_eq!(changes.files[0].path, "inside.txt");
    assert_eq!(
        std::fs::read_to_string(root.join("--odd\nfile.txt")).unwrap(),
        "new\n"
    );
    std::fs::remove_dir_all(&home).unwrap();
}
