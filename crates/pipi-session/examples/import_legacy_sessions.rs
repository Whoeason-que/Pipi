//! 把所有 Agent 的旧 JSONL 会话导入 git 分支（无壳迁移，维护工具）。
//!
//! 与宿主命令 `import_sessions`（Agent 工作台「文件」页的「导入到 git」）走同一实现
//! （[`import_legacy_sessions`]），区别只是不需要起桌面壳：
//!
//! ```text
//! cargo run -p pipi-session --example import_legacy_sessions
//! cargo run -p pipi-session --example import_legacy_sessions -- ~/.pipi/agents/Q
//! ```
//!
//! 默认扫描 `$PIPI_HOME/agents`（缺省 `~/.pipi/agents`）下带 `agent.json` 的目录，
//! 含 `.archive/` 里的归档 Agent（环境变量口径与 `av::paths` 一致，但本 crate 不依赖
//! pipi-venv，这里自己解析）。逐条校验、幂等，原文件改名 `.jsonl.imported` 保留。
//! `--list` 经 git 载体列出会话（迁移前的旧会话用 `--dry-run` 看清单）。
//!
//! **先停掉应用再跑**：应用内打开的会话与导入命令会先脱离槽位，这个工具做不到。

use std::path::PathBuf;

use pipi_protocol::session::SessionRepo;
use pipi_session::{GitSessionRepo, import_legacy_sessions};

fn main() {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut dry_run = false;
    let mut list = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--list" => list = true,
            "--help" | "-h" => {
                println!("用法: import_legacy_sessions [--dry-run|--list] [<agent 目录> ...]");
                return;
            }
            other => dirs.push(PathBuf::from(other)),
        }
    }
    if dirs.is_empty() {
        dirs = default_agent_dirs();
    }
    if dirs.is_empty() {
        eprintln!("没有找到 Agent 目录（$PIPI_HOME/agents）");
        std::process::exit(1);
    }

    let mut failures = 0usize;
    for dir in &dirs {
        let label = dir
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("<unknown>");
        if list {
            let repo = GitSessionRepo::new(dir.join("sessions.git"));
            let archived = repo.list_archived().unwrap_or_default();
            let active = repo.list();
            println!(
                "{label}: 活跃 {} 条，归档 {} 条",
                active.len(),
                archived.len()
            );
            for summary in active.iter().chain(archived.iter()) {
                println!(
                    "  · {} [{}] {} 条消息  {}",
                    summary.id,
                    summary.model.as_deref().unwrap_or("-"),
                    summary.message_count,
                    summary.title
                );
            }
            continue;
        }
        if dry_run {
            let legacy = legacy_session_files(dir);
            println!("{label}: 待导入 {} 条", legacy.len());
            for path in legacy {
                println!("  ? {}", path.display());
            }
            continue;
        }
        let sessions_dir = dir.join("sessions");
        let report = import_legacy_sessions(
            &GitSessionRepo::new(dir.join("sessions.git")),
            &sessions_dir,
        );
        println!(
            "{label}: 导入 {} 条，跳过 {} 条，失败 {} 条",
            report.imported.len(),
            report.skipped.len(),
            report.failed.len()
        );
        for id in &report.imported {
            println!("  + {id}");
        }
        for failure in &report.failed {
            println!("  ! {}：{}", failure.session_id, failure.reason);
        }
        failures += report.failed.len();
    }
    if failures > 0 {
        std::process::exit(1);
    }
}

/// 该 Agent 的旧会话文件（`sessions/*.jsonl` 与 `sessions/.archive/*.jsonl`）。
fn legacy_session_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let sessions = dir.join("sessions");
    let mut files = Vec::new();
    for base in [sessions.clone(), sessions.join(".archive")] {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// `$PIPI_HOME`（缺省 `~/.pipi`）下带 `agent.json` 的 Agent 目录（含 `.archive/`）。
fn default_agent_dirs() -> Vec<PathBuf> {
    let Some(root) = std::env::var_os("PIPI_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".pipi")))
    else {
        return Vec::new();
    };
    let agents = root.join("agents");
    let mut dirs = Vec::new();
    for base in [agents.clone(), agents.join(".archive")] {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.join("agent.json").is_file() {
                dirs.push(path);
            }
        }
    }
    dirs.sort();
    dirs
}
