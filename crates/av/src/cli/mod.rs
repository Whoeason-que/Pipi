//! av CLI：参数分发与共享工具。
//!
//! 约定与旧版一致：`av <command> [目录] [--json]`，目录缺省当前目录，
//! 错误以 `av: <message>` 写 stderr，退出码 0/1。

pub mod contract;
pub mod skill;

use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub const USAGE: &str = "\
av —— Agent 运行时环境契约（agent.toml）与技能包管理（~/.av）

用法：
  av check   [目录]                   静态校验契约（schema/白名单/requires/声明技能）
  av env     [目录] [--json]          打印最终子进程环境（秘密值脱敏）
  av doctor  [目录]                   实测 requires：存在性 + 版本探测

  av skill search <关键词> [--limit N] [--json]
                                      在 skills.sh 搜索技能（结果为 owner/repo）
  av skill install <来源> [--skill 名称]... [--all] [--dry-run]
                                      只装进全局 store（不声明启用）
  av skill add <来源> [--skill 名称]... [--all] [--contract 文件] [--replace] [--dry-run]
                                      安装 + 在 agent.toml 声明 use + 写 agent.lock
  av skill sync [目录] [--dry-run] [--repair] [--yes]
                                      按 use + agent.lock 物化 store（可复现）
  av skill list [--declared] [--json] 列出 store 已装版本 / 契约声明
  av skill verify [名称]... [--declared] [--json]
                                      重算内容哈希，检查漂移
  av skill update [名称]... [--contract 文件] [--dry-run]
                                      重解析来源到新修订
  av skill remove <名称> [--purge] [--contract 文件] [--yes]
                                      移除声明与锁（--purge 同时清 store）

来源：`owner/repo[@ref][#子目录]`、`gitlab:owner/repo`、git URL（https/ssh/git@）、
本地目录（./、../、/、~/）；也接受 GitHub/GitLab 的 tree 网页 URL。
";

pub fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("av: {message}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(args: &[String]) -> Result<(), String> {
    let Some(command) = args.first() else {
        return Err(format!("缺少子命令\n\n{USAGE}"));
    };
    match command.as_str() {
        "check" | "env" | "doctor" => contract::run(command, &args[1..]),
        "skill" => skill::run(&args[1..]),
        other => Err(format!(
            "未知子命令 {other:?}（可用：check / env / doctor / skill）\n\n{USAGE}"
        )),
    }
}

/// 解析后的命令行：位置参数 + 值型 `--key value`（可重复）+ 开关型 `--flag`。
#[derive(Debug, Default)]
pub struct ParsedArgs {
    pub positional: Vec<String>,
    pub values: HashMap<String, Vec<String>>,
    pub flags: HashSet<String>,
}

impl ParsedArgs {
    /// 值型参数的最后一个取值（`--limit 10`）。
    pub fn value(&self, key: &str) -> Option<&str> {
        self.values
            .get(key)
            .and_then(|list| list.last())
            .map(String::as_str)
    }

    /// 值型参数的全部取值（可重复的 `--skill a --skill b`）。
    pub fn values_of(&self, key: &str) -> Vec<String> {
        self.values.get(key).cloned().unwrap_or_default()
    }

    pub fn flag(&self, key: &str) -> bool {
        self.flags.contains(key)
    }

    pub fn first_positional(&self) -> Option<&str> {
        self.positional.first().map(String::as_str)
    }

    /// 值型参数解析为非负整数（`--limit`）。
    pub fn usize_value(&self, key: &str) -> Result<Option<usize>, String> {
        match self.value(key) {
            None => Ok(None),
            Some(raw) => raw
                .parse::<usize>()
                .map(Some)
                .map_err(|_| format!("参数 --{key} 需要非负整数（得到 {raw:?}）")),
        }
    }
}

/// 严格解析：未声明的 `--x` 一律报错（fail-closed，避免拼错静默生效）。
pub fn parse_args(
    args: &[String],
    value_flags: &[&str],
    bool_flags: &[&str],
) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some(key) = arg.strip_prefix("--") {
            if key.is_empty() {
                return Err("未知参数：--".into());
            }
            if value_flags.contains(&key) {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(format!("参数 --{key} 缺少取值"));
                };
                parsed
                    .values
                    .entry(key.to_string())
                    .or_default()
                    .push(value.clone());
            } else if bool_flags.contains(&key) {
                parsed.flags.insert(key.to_string());
            } else {
                return Err(format!("未知参数：{arg}"));
            }
        } else {
            parsed.positional.push(arg.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

/// 确认（联网/破坏性操作）：`--yes` 直接通过；TTY 交互确认；非 TTY 拒绝。
pub fn confirm(assume_yes: bool, prompt: &str) -> Result<(), String> {
    if assume_yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return Err(format!("{prompt}（非交互环境需要 --yes 明确确认）"));
    }
    use std::io::Write;
    print!("{prompt} [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("读取确认失败：{e}"))?;
    let answer = line.trim().to_ascii_lowercase();
    if answer == "y" || answer == "yes" {
        Ok(())
    } else {
        Err("已取消".into())
    }
}

/// 与契约同目录的解析锁路径。
pub fn lock_path_for(contract: &Path) -> PathBuf {
    contract
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(av::store::SKILL_LOCK_FILENAME)
}

/// 选定契约文件：显式 `--contract` > cwd 向上发现的最近 `agent.toml`。
pub fn choose_contract(explicit: Option<&str>) -> Result<PathBuf, String> {
    if let Some(raw) = explicit {
        let path = av::discovery::expand_tilde(raw);
        let canonical = std::fs::canonicalize(&path)
            .map_err(|e| format!("契约文件不可用 {}: {e}", path.display()))?;
        if !canonical.is_file() {
            return Err(format!("契约文件不是常规文件：{}", canonical.display()));
        }
        av::load_contract_file(&canonical)?;
        return Ok(canonical);
    }
    let discovered = av::discover(Path::new("."))?;
    discovered
        .layers
        .first()
        .map(|layer| layer.path.clone())
        .ok_or_else(|| {
            "当前目录（向上到项目根）没有 agent.toml；先创建它（内容至少 `schema = 1`）\
             或用 --contract <文件> 指定"
                .to_string()
        })
}

/// 短哈希（内容哈希的 12 位十六进制）。
pub fn short_hash(content_hash: &str) -> &str {
    let hex = content_hash.strip_prefix("sha256:").unwrap_or(content_hash);
    &hex[..hex.len().min(12)]
}

/// 短修订（commit 前 12 位）。
pub fn short_revision(revision: &str) -> &str {
    &revision[..revision.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arg_parsing_is_strict() {
        let parsed = parse_args(
            &args(&[
                "spec",
                "--skill",
                "pdf",
                "--skill",
                "writer",
                "--all",
                "--dry-run",
            ]),
            &["skill"],
            &["all", "dry-run"],
        )
        .unwrap();
        assert_eq!(parsed.positional, ["spec".to_string()]);
        assert_eq!(parsed.values_of("skill"), ["pdf", "writer"]);
        assert!(parsed.flag("all") && parsed.flag("dry-run"));

        // 未声明参数 / 缺少取值
        assert!(parse_args(&args(&["--nope"]), &[], &[]).is_err());
        assert!(parse_args(&args(&["--limit"]), &["limit"], &[]).is_err());
        assert_eq!(
            parse_args(&args(&["--limit", "7"]), &["limit"], &[])
                .unwrap()
                .usize_value("limit")
                .unwrap(),
            Some(7)
        );
        assert!(parse_args(&args(&["--limit", "x"]), &["limit"], &[])
            .unwrap()
            .usize_value("limit")
            .is_err());
    }
}
