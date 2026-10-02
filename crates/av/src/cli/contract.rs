//! `av check` / `av env` / `av doctor`：契约静态校验、环境打印、requires 实测。

use std::collections::BTreeMap;
use std::path::PathBuf;

use av::{
    collect_process_env, discover, lookup_command, merge_layers, probe_version, resolve_env,
    version_satisfies, Discovered, Merged, ResolvedEnv,
};

use super::{parse_args, USAGE};

pub fn run(command: &str, args: &[String]) -> Result<(), String> {
    if !matches!(command, "check" | "env" | "doctor") {
        return Err(format!(
            "未知子命令 {command:?}（可用：check / env / doctor）\n\n{USAGE}"
        ));
    }
    let parsed = parse_args(args, &[], &["json"])?;
    if parsed.positional.len() > 1 {
        return Err(format!("{command} 最多接受一个目录参数"));
    }
    let path = parsed
        .first_positional()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    match command {
        "check" => check(&path),
        "env" => env(&path, parsed.flag("json")),
        "doctor" => doctor(&path),
        _ => unreachable!("子命令已在上方校验"),
    }
}

fn runtime_vars() -> BTreeMap<String, String> {
    BTreeMap::from([("AV".to_string(), "1".to_string())])
}

struct Resolved {
    discovered: Discovered,
    merged: Merged,
    env: ResolvedEnv,
}

fn resolve(path: &std::path::Path) -> Result<Resolved, String> {
    let discovered = discover(path)?;
    let merged = merge_layers(&discovered.layers)?;
    let resolved = resolve_env(&discovered.layers, &collect_process_env(), &runtime_vars())?;
    Ok(Resolved {
        discovered,
        merged,
        env: resolved,
    })
}

fn check(path: &std::path::Path) -> Result<(), String> {
    let resolved = resolve(path)?;

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

    // 声明启用的 store 技能：锁与内容必须就绪（与宿主会话启动同一口径）
    let mut declared_skills = 0;
    if let Some(declared) = av::store::winning_use(&resolved.discovered.layers) {
        let skills = av::store::resolve_declared_skills(declared.names, &declared.lock_path)
            .map_err(|e| format!("声明技能校验失败（{}）：{e}", declared.layer.label))?;
        declared_skills = skills.len();
    }

    if missing > 0 {
        return Err(format!("requires 存在性校验失败：{missing} 条命令缺失"));
    }
    println!(
        "OK（{} 层）：requires {} 条全部存在，set {} 项，secrets {} 项，skills.use {} 项已就绪",
        resolved.discovered.layers.len(),
        resolved.merged.requires.len(),
        resolved.merged.env.set.len(),
        resolved.merged.env.secrets.len(),
        declared_skills,
    );
    Ok(())
}

fn env(path: &std::path::Path, json: bool) -> Result<(), String> {
    let resolved = resolve(path)?;
    if json {
        let output = serde_json::to_string_pretty(&resolved.env.redacted())
            .map_err(|e| format!("JSON 序列化失败：{e}"))?;
        println!("{output}");
    } else {
        for (key, value) in &resolved.env.redacted() {
            println!("{key}={value}");
        }
    }
    Ok(())
}

fn doctor(path: &std::path::Path) -> Result<(), String> {
    let resolved = resolve(path)?;
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
                let outcome = probe_version(&entry.command, &resolved.env.vars)
                    .and_then(|line| version_satisfies(&line, required).map(|ok| (ok, line)));
                match outcome {
                    Ok((true, line)) => println!("✓ {} {required}（{line}）", entry.command),
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

    // 声明技能的内容哈希深检（相当于 `av skill verify --declared`）
    if let Some(declared) = av::store::winning_use(&resolved.discovered.layers) {
        if !declared.names.is_empty() {
            let skills = av::store::resolve_declared_skills(declared.names, &declared.lock_path)?;
            for skill in skills {
                match av::store::verify_entry(&skill.entry)? {
                    av::store::VerifyOutcome::Ok => {
                        println!("✓ skill {}（内容哈希一致）", skill.entry.name)
                    }
                    av::store::VerifyOutcome::Missing => {
                        println!("✗ skill {}：store 内容缺失", skill.entry.name);
                        all_ok = false;
                    }
                    av::store::VerifyOutcome::Mismatch { actual } => {
                        println!(
                            "✗ skill {}：内容漂移（{} ≠ {}）",
                            skill.entry.name, actual, skill.entry.content_hash
                        );
                        all_ok = false;
                    }
                }
            }
        }
    }

    if !all_ok {
        return Err("doctor 校验未全部通过".into());
    }
    Ok(())
}
