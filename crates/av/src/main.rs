//! av CLI：`av check` / `av env` / `av doctor`。
//!
//! - `check`：加载契约并静态校验（schema/白名单/保留命名空间/requires 存在性）；
//! - `env`：打印解析后的最终子进程环境（秘密值脱敏；`--json` 机器可读）；
//! - `doctor`：逐条实测 requires（存在性 + 版本探测）。
//!
//! 约定：`av <command> [目录] [--json]`，目录缺省为当前目录。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use av::{
    collect_process_env, discover, lookup_command, merge_layers, probe_version, resolve_env,
    version_satisfies, Merged, ResolvedEnv,
};

const USAGE: &str = "\
av —— Agent 运行时环境契约（agent.toml）

用法：
  av check   [目录]           静态校验契约（schema/白名单/requires 存在性）
  av env     [目录] [--json]  打印最终子进程环境（秘密值脱敏）
  av doctor  [目录]           实测 requires：存在性 + 版本探测
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("av: {message}");
            ExitCode::FAILURE
        }
    }
}

fn runtime_vars() -> BTreeMap<String, String> {
    BTreeMap::from([("AV".to_string(), "1".to_string())])
}

struct Resolved {
    layers: usize,
    merged: Merged,
    env: ResolvedEnv,
}

fn resolve(path: &std::path::Path) -> Result<Resolved, String> {
    let discovered = discover(path)?;
    let merged = merge_layers(&discovered.layers)?;
    let resolved = resolve_env(&discovered.layers, &collect_process_env(), &runtime_vars())?;
    Ok(Resolved {
        layers: discovered.layers.len(),
        merged,
        env: resolved,
    })
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(command) = args.first() else {
        return Err(format!("缺少子命令\n\n{USAGE}"));
    };
    if !matches!(command.as_str(), "check" | "env" | "doctor") {
        return Err(format!(
            "未知子命令 {command:?}（可用：check / env / doctor）\n\n{USAGE}"
        ));
    }
    let mut path = PathBuf::from(".");
    let mut json = false;
    for arg in &args[1..] {
        if arg == "--json" {
            json = true;
        } else if arg.starts_with("--") {
            return Err(format!("未知参数：{arg}"));
        } else {
            path = PathBuf::from(arg);
        }
    }

    match command.as_str() {
        "check" => {
            let resolved = resolve(&path)?;
            let mut missing = 0;
            for entry in &resolved.merged.requires {
                if lookup_command(
                    &entry.command,
                    resolved.env.vars.get("PATH").map(String::as_str),
                )
                .is_none()
                {
                    eprintln!("✗ {}：不在 PATH 中", entry.command);
                    missing += 1;
                }
            }
            if missing > 0 {
                return Err(format!("requires 存在性校验失败：{missing} 条命令缺失"));
            }
            println!(
                "OK（{} 层）：requires {} 条全部存在，set {} 项，secrets {} 项",
                resolved.layers,
                resolved.merged.requires.len(),
                resolved.merged.env.set.len(),
                resolved.merged.env.secrets.len(),
            );
        }
        "env" => {
            let resolved = resolve(&path)?;
            if json {
                let output = serde_json::to_string_pretty(&resolved.env.redacted())
                    .map_err(|e| format!("JSON 序列化失败：{e}"))?;
                println!("{output}");
            } else {
                for (key, value) in &resolved.env.redacted() {
                    println!("{key}={value}");
                }
            }
        }
        "doctor" => {
            let resolved = resolve(&path)?;
            let path_value = resolved.env.vars.get("PATH").map(String::as_str);
            let mut all_ok = true;
            for entry in &resolved.merged.requires {
                if lookup_command(&entry.command, path_value).is_none() {
                    println!("✗ {}: 不在 PATH 中", entry.command);
                    all_ok = false;
                    continue;
                }
                match &entry.version {
                    None => println!("✓ {}（存在）", entry.command),
                    Some(required) => {
                        let outcome =
                            probe_version(&entry.command, &resolved.env.vars).and_then(|line| {
                                version_satisfies(&line, required).map(|ok| (ok, line))
                            });
                        match outcome {
                            Ok((true, line)) => {
                                println!("✓ {} {required}（{line}）", entry.command)
                            }
                            Ok((false, line)) => {
                                println!("✗ {} 需要 {required}，实际：{line}", entry.command);
                                all_ok = false;
                            }
                            Err(e) => {
                                println!("✗ {}：{e}", entry.command);
                                all_ok = false;
                            }
                        }
                    }
                }
            }
            if !all_ok {
                return Err("doctor 校验未全部通过".into());
            }
        }
        _ => unreachable!("子命令已在上方校验"),
    }
    Ok(())
}
