use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::{io::AsyncReadExt, process::Command};

use super::paths::{relative, resolve, root};

const OUTPUT_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRepoInfo {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChange {
    pub path: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChanges {
    pub workspace: String,
    pub is_repo: bool,
    pub files: Vec<WorkspaceChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repositories: Vec<WorkspaceRepoInfo>,
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
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot run Git: {e}"))?;
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

#[derive(Clone)]
struct Repository {
    root: PathBuf,
    overrides: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PipiConfig {
    #[serde(default)]
    git: Option<PipiGitConfig>,
    #[serde(default)]
    repositories: Option<RepositoriesConfig>,
    #[serde(default)]
    repos: Option<RepositoriesConfig>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PipiGitConfig {
    #[serde(default)]
    repositories: Option<RepositoriesConfig>,
    #[serde(default)]
    repos: Option<RepositoriesConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RepositoriesConfig {
    List(Vec<RepoDeclaration>),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RepoDeclaration {
    Simple(String),
    Detailed {
        path: String,
        #[serde(default)]
        name: Option<String>,
    },
}

struct DeclaredRepo {
    path: String,
    name: Option<String>,
}

fn load_declared_repos(workspace: &Path) -> Vec<DeclaredRepo> {
    let toml_path = workspace.join("pipi.toml");
    let Ok(content) = std::fs::read_to_string(&toml_path) else {
        return Vec::new();
    };
    let Ok(config) = toml::from_str::<PipiConfig>(&content) else {
        return Vec::new();
    };
    let entries = config
        .git
        .as_ref()
        .and_then(|g| g.repositories.as_ref().or(g.repos.as_ref()))
        .or(config.repositories.as_ref())
        .or(config.repos.as_ref());

    match entries {
        Some(RepositoriesConfig::List(list)) => list
            .iter()
            .map(|item| match item {
                RepoDeclaration::Simple(p) => DeclaredRepo {
                    path: p.clone(),
                    name: None,
                },
                RepoDeclaration::Detailed { path, name } => DeclaredRepo {
                    path: path.clone(),
                    name: name.clone(),
                },
            })
            .collect(),
        None => Vec::new(),
    }
}

async fn branch_name(dir: &Path, overrides: &[String]) -> Option<String> {
    if let Ok(output) = git(dir, overrides, &["branch", "--show-current"]).await {
        let branch = String::from_utf8_lossy(&output).trim().to_string();
        if !branch.is_empty() {
            return Some(branch);
        }
    }
    if let Ok(output) = git(dir, overrides, &["rev-parse", "--short", "HEAD"]).await {
        let hash = String::from_utf8_lossy(&output).trim().to_string();
        if !hash.is_empty() {
            return Some(hash);
        }
    }
    None
}

struct DiscoveredRepo {
    info: WorkspaceRepoInfo,
    repo: Repository,
}

async fn discover_repositories(workspace: &Path) -> Result<Vec<DiscoveredRepo>, String> {
    let mut declared = load_declared_repos(workspace);
    if declared.is_empty() {
        declared.push(DeclaredRepo {
            path: ".".into(),
            name: None,
        });
    }

    let mut results = Vec::new();
    let mut seen_roots = std::collections::HashSet::new();

    for decl in declared {
        let decl_path = decl.path.trim();
        let target_dir = if decl_path.is_empty() || decl_path == "." {
            workspace.to_path_buf()
        } else {
            workspace.join(decl_path)
        };

        // 安全检查：目标目录必须存在且必须在工作区内，防止穿越 ../
        let Ok(canonical_target) = target_dir.canonicalize() else {
            continue;
        };
        if !canonical_target.starts_with(workspace) {
            continue;
        }

        let Some(repo) = repository(&canonical_target).await? else {
            continue;
        };

        // 避免重复包含同一个 Git 仓库
        if !seen_roots.insert(repo.root.clone()) {
            continue;
        }

        let rel_path = if repo.root == workspace || workspace.starts_with(&repo.root) {
            String::new()
        } else {
            match relative(workspace, &repo.root) {
                Ok(p) => p,
                Err(_) => continue,
            }
        };

        let default_name = if rel_path.is_empty() {
            repo.root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("root")
                .to_string()
        } else {
            rel_path.clone()
        };

        let name = decl.name.unwrap_or(default_name);
        let branch = branch_name(&repo.root, &repo.overrides).await;

        results.push(DiscoveredRepo {
            info: WorkspaceRepoInfo {
                name,
                path: rel_path,
                branch,
            },
            repo,
        });
    }

    Ok(results)
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
    if !workspace.starts_with(&root) && !root.starts_with(workspace) {
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
    let repos = discover_repositories(&root).await?;
    if repos.is_empty() {
        return Ok(WorkspaceChanges {
            workspace: root.to_string_lossy().into_owned(),
            is_repo: false,
            files: vec![],
            repositories: vec![],
        });
    }

    let mut result = WorkspaceChanges {
        workspace: root.to_string_lossy().into_owned(),
        is_repo: true,
        files: vec![],
        repositories: repos.iter().map(|d| d.info.clone()).collect(),
    };

    let mut seen_paths = std::collections::HashSet::new();

    for discovered in &repos {
        let status_dir = if discovered.repo.root.starts_with(&root) {
            &discovered.repo.root
        } else {
            &root
        };

        let output = git(
            status_dir,
            &discovered.repo.overrides,
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
            let path = std::str::from_utf8(&record[3..])
                .map_err(|_| "Git file name is not valid UTF-8")?;
            let absolute = discovered.repo.root.join(path);
            let Ok(workspace_rel_path) = relative(&root, &absolute) else {
                continue;
            };
            // 符号链接/子模块等不能通过差异入口绕过文件预览的边界。
            if resolve(&root, &workspace_rel_path, true).is_err() {
                continue;
            }
            if !seen_paths.insert(workspace_rel_path.clone()) {
                continue;
            }
            result.files.push(WorkspaceChange {
                path: workspace_rel_path,
                status: String::from_utf8_lossy(&record[..2]).into_owned(),
                repository: Some(discovered.info.name.clone()),
            });
        }
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

    let repos = discover_repositories(&root).await?;
    // 寻找包含此文件的最具体 Git 仓库（按 repo.root 最长前缀匹配）
    let matched_repo = repos
        .into_iter()
        .filter(|d| path.starts_with(&d.repo.root))
        .max_by_key(|d| d.repo.root.as_os_str().len())
        .map(|d| d.repo);

    let repo = match matched_repo {
        Some(repo) => repo,
        None => {
            let candidate_dir = path.parent().unwrap_or(&root);
            repository(candidate_dir)
                .await?
                .or(repository(&root).await?)
                .ok_or("Current workspace is not a Git repository")?
        }
    };

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
    let has_head = git(
        &repo.root,
        &repo.overrides,
        &["rev-parse", "--verify", "HEAD"],
    )
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
#[allow(clippy::await_holding_lock)]
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

    #[tokio::test]
    async fn pipi_toml_declares_sub_repositories_and_diffs_correctly() {
        let _guard = pipi_test_support::HOME_LOCK.lock().unwrap();
        let workspace = pipi_test_support::temp_dir("multi-repo-workspace");

        // 根目录不是 git 仓库，但有两个子仓库 sub1 和 sub2
        let sub1 = workspace.join("sub1");
        let sub2 = workspace.join("packages").join("sub2");
        std::fs::create_dir_all(&sub1).unwrap();
        std::fs::create_dir_all(&sub2).unwrap();

        run(&sub1, &["init", "-b", "main"]);
        run(&sub1, &["config", "user.name", "test"]);
        run(&sub1, &["config", "user.email", "test@example.invalid"]);
        std::fs::write(sub1.join("file1.txt"), "hello sub1\n").unwrap();
        run(&sub1, &["add", "file1.txt"]);
        run(&sub1, &["commit", "-m", "init sub1"]);
        std::fs::write(sub1.join("file1.txt"), "modified sub1\n").unwrap();

        run(&sub2, &["init", "-b", "feature-x"]);
        run(&sub2, &["config", "user.name", "test"]);
        run(&sub2, &["config", "user.email", "test@example.invalid"]);
        std::fs::write(sub2.join("file2.txt"), "hello sub2\n").unwrap();
        run(&sub2, &["add", "file2.txt"]);
        run(&sub2, &["commit", "-m", "init sub2"]);
        std::fs::write(sub2.join("new_file.txt"), "new file in sub2\n").unwrap();

        // 写 pipi.toml 声明这两个仓库
        let pipi_toml_content = r#"
[git]
repositories = [
    { path = "sub1", name = "SubRepo 1" },
    { path = "packages/sub2", name = "SubRepo 2" },
]
"#;
        std::fs::write(workspace.join("pipi.toml"), pipi_toml_content).unwrap();

        let changes_res = changes(workspace.to_str().unwrap()).await.unwrap();
        assert!(changes_res.is_repo);
        assert_eq!(changes_res.repositories.len(), 2);
        assert_eq!(changes_res.repositories[0].name, "SubRepo 1");
        assert_eq!(changes_res.repositories[0].path, "sub1");
        assert_eq!(changes_res.repositories[0].branch.as_deref(), Some("main"));

        assert_eq!(changes_res.repositories[1].name, "SubRepo 2");
        assert_eq!(changes_res.repositories[1].path, "packages/sub2");
        assert_eq!(
            changes_res.repositories[1].branch.as_deref(),
            Some("feature-x")
        );

        // 验证文件改动被正确归类并包含仓库信息
        let file1_change = changes_res
            .files
            .iter()
            .find(|f| f.path == "sub1/file1.txt")
            .expect("sub1/file1.txt");
        assert_eq!(file1_change.repository.as_deref(), Some("SubRepo 1"));

        let file2_change = changes_res
            .files
            .iter()
            .find(|f| f.path == "packages/sub2/new_file.txt")
            .expect("sub2 new file");
        assert_eq!(file2_change.repository.as_deref(), Some("SubRepo 2"));

        // 测试 diff 对两个子仓库分别正确工作
        let diff1 = diff(workspace.to_str().unwrap(), "sub1/file1.txt")
            .await
            .unwrap();
        assert_eq!(diff1.path, "sub1/file1.txt");
        assert!(diff1.patch.contains("-hello sub1"));
        assert!(diff1.patch.contains("+modified sub1"));

        let diff2 = diff(workspace.to_str().unwrap(), "packages/sub2/new_file.txt")
            .await
            .unwrap();
        assert_eq!(diff2.path, "packages/sub2/new_file.txt");
        assert_eq!(diff2.base, "untracked");
        assert!(diff2.patch.contains("+new file in sub2"));
    }

    #[tokio::test]
    async fn pipi_toml_rejects_escaping_repositories() {
        let _guard = pipi_test_support::HOME_LOCK.lock().unwrap();
        let workspace = pipi_test_support::temp_dir("safe-workspace");
        let outside = pipi_test_support::temp_dir("outside-repo");
        run(&outside, &["init"]);

        // pipi.toml 尝试穿越出工作区
        let pipi_toml_content = r#"
[git]
repositories = [
    "../outside-repo",
]
"#;
        std::fs::write(workspace.join("pipi.toml"), pipi_toml_content).unwrap();

        let changes_res = changes(workspace.to_str().unwrap()).await.unwrap();
        assert!(!changes_res.is_repo);
        assert!(changes_res.repositories.is_empty());
        assert!(changes_res.files.is_empty());
    }
}
