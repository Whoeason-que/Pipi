//! av 单元测试共享夹具。
//!
//! 由 `lib.rs` 以 `#[cfg(test)] mod test_support;` 声明（默认路径 `src/test_support.rs`）
//! 声明；测试文件通过 `use crate::av::test_support::...;` 引用。
//! 临时目录统一走 `pipi_test_support::temp_dir(prefix)`，调用方自行清理。

use std::fs;
use std::path::{Path, PathBuf};

use crate::av::discovery::Layer;
use crate::av::schema::AgentToml;

/// 构造一个通过校验的契约层（label 同时作为 path）。
pub(crate) fn layer(label: &str, text: &str) -> Layer {
    let config: AgentToml = toml::from_str(text).unwrap();
    config.validate().unwrap();
    Layer {
        label: label.to_string(),
        path: PathBuf::from(label),
        config,
    }
}

/// 写一个带 frontmatter 的技能目录（`<dir>/<name>/SKILL.md`），返回技能目录。
pub(crate) fn write_skill(dir: &Path, name: &str, body: &str) -> PathBuf {
    let skill = dir.join(name);
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} skill\n---\n{body}"),
    )
    .unwrap();
    skill
}

/// 安装管线测试用的静默日志回调。
pub(crate) fn noop_log() -> impl FnMut(String) {
    |_| {}
}

/// 建一个本地 git 仓库（含 `skills/pdf`、tag `v1`），返回 (仓库路径, HEAD commit)。
///
/// 统一了 install 与 sources 两份夹具：环境变量取并集（author/committer +
/// `GIT_TERMINAL_PROMPT=0` + `GIT_LFS_SKIP_SMUDGE=1`），返回值取 sources 版
/// （调用方需要 HEAD 修订）。
pub(crate) fn make_repo(label: &str) -> (PathBuf, String) {
    let repo = pipi_test_support::temp_dir(&format!("av-{label}"));
    let run = |args: &[&str]| -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_LFS_SKIP_SMUDGE", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    run(&["init", "--quiet"]);
    write_skill(&repo.join("skills"), "pdf", "v1");
    run(&["add", "-A"]);
    run(&["commit", "--quiet", "-m", "init"]);
    run(&["tag", "v1"]);
    let head = run(&["rev-parse", "HEAD"]);
    (repo, head.trim().to_string())
}
