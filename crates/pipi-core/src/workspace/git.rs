use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::{io::AsyncReadExt, process::Command};

use super::paths::{relative, resolve, root};

const OUTPUT_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChange {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChanges {
    pub workspace: String,
    pub is_repo: bool,
    pub files: Vec<WorkspaceChange>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDiff {
    pub path: String,
    pub base: String,
    pub patch: String,
    pub truncated: bool,
}

/// 固定只读 Git 命令；去掉环境注入，禁用锁、fsmonitor、hooks、外部 diff/textconv。
/// stdout 有硬上限，超时/超限会杀掉子进程，不能在远程请求中无限读输出。
async fn git(dir: &Path, overrides: &[String], args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    command
        .current_dir(dir)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    command.args([
        "--no-pager",
        "--literal-pathspecs",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.hooksPath=/dev/null",
    ]);
    for value in overrides {
        command.arg("-c").arg(value);
    }
    command.args(args);
    let mut child = command.spawn().map_err(|e| format!("Cannot run Git: {e}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("Cannot read Git output")?
        .take(OUTPUT_LIMIT + 1);
    let operation = async {
        let mut output = Vec::new();
        stdout
            .read_to_end(&mut output)
            .await
            .map_err(|e| e.to_string())?;
        if output.len() as u64 > OUTPUT_LIMIT {
            return Err("Git output exceeded 1 MiB, please use an external Git tool".into());
        }
        let status = child.wait().await.map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("Cannot read Git status or diff".into());
        }
        Ok(output)
    };
    match tokio::time::timeout(Duration::from_secs(8), operation).await {
        Ok(Ok(output)) => Ok(output),
        result => {
            let _ = child.kill().await;
            match result {
                Ok(Err(error)) => Err(error),
                _ => Err("Git execution timed out, please use an external Git tool".into()),
            }
        }
    }
}

struct Repository {
    root: PathBuf,
    overrides: Vec<String>,
}

async fn repository(workspace: &Path) -> Result<Option<Repository>, String> {
    let Ok(output) = git(workspace, &[], &["rev-parse", "--show-toplevel"]).await else {
        return Ok(None);
    };
    let root = PathBuf::from(
        String::from_utf8(output)
            .map_err(|_| "Git root directory is not valid UTF-8")?
            .trim_end_matches('\n'),
    );
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    if !workspace.starts_with(&root) {
        return Err("Git root directory does not match workspace directory".into());
    }
    // Git 在判断内容是否改变时可能执行 clean/process filter；读取配置本身不运行它们。
    let mut overrides = Vec::new();
    let config = git(
        workspace,
        &[],
        &["config", "--null", "--name-only", "--list"],
    )
    .await?;
    for name in std::str::from_utf8(&config)
        .map_err(|_| "Git config name is not valid UTF-8")?
        .split('\0')
    {
        if name.starts_with("filter.") && (name.ends_with(".clean") || name.ends_with(".process")) {
            overrides.push(format!("{name}="));
        }
    }
    Ok(Some(Repository { root, overrides }))
}

pub async fn changes(agent: &str) -> Result<WorkspaceChanges, String> {
    let root = root(agent)?;
    let mut result = WorkspaceChanges {
        workspace: root.to_string_lossy().into_owned(),
        is_repo: false,
        files: vec![],
    };
    let Some(repo) = repository(&root).await? else {
        return Ok(result);
    };
    result.is_repo = true;
    let output = git(
        &root,
        &repo.overrides,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
            "--",
            ".",
        ],
    )
    .await?;
    for record in output.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        if record.len() < 4 {
            return Err("Git status format is incomplete".into());
        }
        let path = std::str::from_utf8(&record[3..]).map_err(|_| "Git file name is not valid UTF-8")?;
        let absolute = repo.root.join(path);
        let Ok(path) = relative(&root, &absolute) else {
            continue;
        };
        // 符号链接/子模块等不能通过差异入口绕过文件预览的边界。
        if resolve(&root, &path, true).is_err() {
            continue;
        }
        result.files.push(WorkspaceChange {
            path,
            status: String::from_utf8_lossy(&record[..2]).into_owned(),
        });
    }
    Ok(result)
}

pub async fn diff(agent: &str, input: &str) -> Result<WorkspaceDiff, String> {
    let root = root(agent)?;
    let path = resolve(&root, input, true)?;
    let relative_path = relative(&root, &path)?;
    if relative_path.is_empty() || path.is_dir() {
        return Err("Please select a file to view diff".into());
    }
    let repo = repository(&root)
        .await?
        .ok_or("Current workspace is not a Git repository")?;
    let repo_path = relative(&repo.root, &path)?;
    let common = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--submodule=short",
        "--ignore-submodules=dirty",
        "--no-color",
        "--no-indent-heuristic",
    ];
    let has_head = git(&root, &repo.overrides, &["rev-parse", "--verify", "HEAD"])
        .await
        .is_ok();
    let mut args = common.to_vec();
    if has_head {
        args.push("HEAD")
    } else {
        args.push("--cached")
    }
    args.extend(["--", &repo_path]);
    let output = git(&repo.root, &repo.overrides, &args).await?;
    let mut patch = String::from_utf8_lossy(&output).into_owned();
    if !has_head {
        let mut args = common.to_vec();
        args.extend(["--", &repo_path]);
        patch.push_str(&String::from_utf8_lossy(
            &git(&repo.root, &repo.overrides, &args).await?,
        ));
    }
    // 未跟踪文件没有 Git 基线：给出有界的新增文本视图，不能冒充某次会话的历史版本。
    let tracked = git(
        &repo.root,
        &repo.overrides,
        &["ls-files", "--error-unmatch", "--", &repo_path],
    )
    .await
    .is_ok();
    let mut truncated = false;
    let base = if !tracked && path.is_file() {
        let read_root = root.clone();
        let read_path = relative_path.clone();
        let file =
            tokio::task::spawn_blocking(move || super::files::preview_at(&read_root, &read_path))
                .await
                .map_err(|e| format!("Failed to read new file: {e}"))??;
        truncated = file.truncated;
        patch = match file.content {
            Some(text) => format!(
                "--- /dev/null\n+++ {relative_path}\n@@ new file @@\n{}",
                text.lines()
                    .map(|line| format!("+{line}\n"))
                    .collect::<String>()
            ),
            None => "Binary file: no displayable text diff".into(),
        };
        "untracked"
    } else if has_head {
        "HEAD"
    } else {
        "empty (staged + unstaged)"
    };
    Ok(WorkspaceDiff {
        path: relative_path,
        base: base.into(),
        patch,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        assert!(
            std::process::Command::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }

    #[tokio::test]
    async fn git_reads_never_execute_repository_filters_or_fsmonitor() {
        let _guard = pipi_test_support::HOME_LOCK.lock().unwrap();
        let dir = pipi_test_support::temp_dir("preview-git");
        run(&dir, &["init"]);
        run(&dir, &["config", "user.name", "test"]);
        run(&dir, &["config", "user.email", "test@example.invalid"]);
        std::fs::write(dir.join("a.txt"), "before\n").unwrap();
        run(&dir, &["add", "a.txt"]);
        run(&dir, &["commit", "-m", "initial"]);
        std::fs::write(dir.join(".gitattributes"), "*.txt filter=evil diff=evil\n").unwrap();
        run(
            &dir,
            &["config", "filter.evil.clean", "touch FILTER_EXECUTED; cat"],
        );
        run(
            &dir,
            &[
                "config",
                "filter.evil.process",
                "touch PROCESS_EXECUTED; cat",
            ],
        );
        run(
            &dir,
            &["config", "core.fsmonitor", "touch FSMONITOR_EXECUTED"],
        );
        std::fs::write(dir.join("a.txt"), "after\n").unwrap();
        let repo = repository(&dir).await.unwrap().unwrap();
        let result = git(
            &dir,
            &repo.overrides,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "HEAD",
                "--",
                "a.txt",
            ],
        )
        .await
        .unwrap();
        assert!(String::from_utf8(result).unwrap().contains("+after"));
        for marker in ["FILTER_EXECUTED", "PROCESS_EXECUTED", "FSMONITOR_EXECUTED"] {
            assert!(!dir.join(marker).exists());
        }
    }
}
