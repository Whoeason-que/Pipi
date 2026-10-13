//! `av skill` 子命令：搜索、安装、声明、同步、校验、更新、移除。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::av::install::{self, InstallReport, Selection, SyncAction};
use crate::av::sources::{self, SourceSpec};
use crate::av::store::{self, SkillLock, SkillLockEntry, SourceType, VerifyOutcome};

use super::{choose_contract, confirm, lock_path_for, parse_args, short_hash, short_revision};

const USAGE: &str = "\
av skill —— 全局技能 store（~/.av/skills）管理

用法：
  av skill search <关键词> [--limit N] [--json]
  av skill install <来源> [--skill 名称]... [--all] [--dry-run]
  av skill add <来源> [--skill 名称]... [--all] [--contract 文件] [--replace] [--dry-run]
  av skill sync [目录] [--dry-run] [--repair] [--yes]
  av skill list [--declared] [--json]
  av skill verify [名称]... [--declared] [--json]
  av skill update [名称]... [--contract 文件] [--dry-run]
  av skill remove <名称> [--purge] [--contract 文件] [--yes]

说明：
  install 只装进 store（装 ≠ 启用）；add 同时写契约 use 与同层 agent.lock；
  sync 按 use + agent.lock 物化（可复现，缺内容时联网取 git）；
  update 重解析来源到新修订；remove 改契约与锁，--purge 才动 store。
";

pub fn run(args: &[String]) -> Result<(), String> {
    let Some(sub) = args.first() else {
        return Err(format!("缺少 skill 子命令\n\n{USAGE}"));
    };
    let rest = &args[1..];
    match sub.as_str() {
        "search" => cmd_search(rest),
        "install" => cmd_install(rest),
        "add" => cmd_add(rest),
        "sync" => cmd_sync(rest),
        "list" => cmd_list(rest),
        "verify" => cmd_verify(rest),
        "update" => cmd_update(rest),
        "remove" => cmd_remove(rest),
        other => Err(format!("未知 skill 子命令 {other:?}\n\n{USAGE}")),
    }
}

// ============ install / add ============

fn spec_from(positional: &[String], usage: &str) -> Result<SourceSpec, String> {
    if positional.len() != 1 {
        return Err(format!("用法：{usage}"));
    }
    sources::parse_spec(&positional[0])
}

fn selection_from(parsed: &super::ParsedArgs) -> Result<Selection, String> {
    let names = parsed.values_of("skill");
    if !names.is_empty() && parsed.flag("all") {
        return Err("--skill 与 --all 不能同时使用".into());
    }
    if !names.is_empty() {
        for name in &names {
            if !crate::av::skills::valid_skill_name(name) {
                return Err(format!("技能名 {name:?} 形状非法"));
            }
        }
        return Ok(Selection::Only(names));
    }
    if parsed.flag("all") {
        return Ok(Selection::All);
    }
    Ok(Selection::Auto)
}

fn cmd_install(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &["skill"], &["all", "dry-run"])?;
    let spec = spec_from(
        &parsed.positional,
        "av skill install <来源> [--skill 名称]... [--all] [--dry-run]",
    )?;
    let selection = selection_from(&parsed)?;
    let dry_run = parsed.flag("dry-run");
    let mut log = |line: String| println!("{line}");
    let report = install::install_source(&spec, &selection, dry_run, &mut log)?;
    print_report(&report, dry_run);
    if !dry_run {
        println!(
            "\n已装入 store（装 ≠ 启用）。在 agent.toml 写 `[resources.skills] use = [...]` 后生效；\
             或直接 `av skill add <来源>` 一步完成安装 + 声明。"
        );
    }
    Ok(())
}

fn cmd_add(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &["skill", "contract"], &["all", "dry-run", "replace"])?;
    let spec = spec_from(
        &parsed.positional,
        "av skill add <来源> [--skill 名称]... [--all] [--contract 文件] [--replace] [--dry-run]",
    )?;
    let selection = selection_from(&parsed)?;
    let dry_run = parsed.flag("dry-run");

    // 先定契约（失败要早于任何网络/写盘）
    let contract = choose_contract(parsed.value("contract"))?;
    let lock_path = lock_path_for(&contract);

    let mut log = |line: String| println!("{line}");
    let report = install::install_source(&spec, &selection, dry_run, &mut log)?;
    print_report(&report, dry_run);

    // 同名不同来源：默认拒绝，--replace 显式替换
    let mut lock = SkillLock::read(&lock_path)?;
    lock.validate_unique_names()?;
    let mut conflicts = Vec::new();
    for skill in &report.installed {
        if let Some(existing) = lock.get(&skill.entry.name)
            && existing.source != skill.entry.source
        {
            conflicts.push(format!(
                "{}（{} → {}）",
                skill.entry.name, existing.source, skill.entry.source
            ));
        }
    }
    if !conflicts.is_empty() && !parsed.flag("replace") {
        return Err(format!(
            "同名技能已锁定到不同来源：{}；确认更换来源请加 --replace",
            conflicts.join("、")
        ));
    }

    if dry_run {
        println!(
            "\ndry-run：将在 {} 声明 use，并写入 {}",
            contract.display(),
            lock_path.display()
        );
        return Ok(());
    }

    let names: Vec<String> = report
        .installed
        .iter()
        .map(|skill| skill.entry.name.clone())
        .collect();
    let text = fs::read_to_string(&contract)
        .map_err(|e| format!("无法读取 {}: {e}", contract.display()))?;
    // 编辑结果先过 schema 校验再落盘（fail-closed）；与桌面端技能面板共用同一实现
    let (edited, _) = crate::av::contract_edit::edit_use_names(&text, &names, &[])?;
    store::write_text_atomic(&contract, &edited)?;

    for skill in &report.installed {
        lock.set_entry(skill.entry.clone());
    }
    lock.write(&lock_path)?;

    println!(
        "\n已声明 use（{}）：{}；解析锁：{}",
        names.join(", "),
        contract.display(),
        lock_path.display()
    );
    Ok(())
}

fn print_report(report: &InstallReport, dry_run: bool) {
    for skill in &report.installed {
        let status = if skill.unchanged {
            "已是最新"
        } else if dry_run {
            "将安装"
        } else {
            "已安装"
        };
        println!(
            "{status} {}（{} · {}）",
            skill.entry.name,
            source_of(&skill.entry),
            short_hash(&skill.entry.content_hash)
        );
    }
    for (relative, reason) in &report.skipped {
        println!("跳过 {relative}：{reason}");
    }
}

fn source_of(entry: &SkillLockEntry) -> String {
    match entry.source_type {
        SourceType::Git => format!(
            "{}@{}",
            entry.source,
            entry.revision.as_deref().map(short_revision).unwrap_or("?")
        ),
        SourceType::Local => format!("本地 {}", entry.source),
    }
}

// ============ sync ============

fn cmd_sync(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &[], &["dry-run", "repair", "yes"])?;
    if parsed.positional.len() > 1 {
        return Err("sync 最多接受一个目录参数".into());
    }
    let dir = parsed
        .positional
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let discovered = crate::av::discover(&dir)?;
    let Some(declared) = store::winning_use(&discovered.layers) else {
        println!("未声明 [resources.skills].use，无事可做");
        return Ok(());
    };
    if declared.names.is_empty() {
        println!("use = []（显式禁用），无事可做");
        return Ok(());
    }
    if !declared.lock_path.is_file() {
        return Err(format!(
            "缺少解析锁 {}；用 `av skill add <来源>` 生成",
            declared.lock_path.display()
        ));
    }
    let lock = SkillLock::read(&declared.lock_path)?;
    lock.validate_unique_names()?;

    // 锁的严格契约（CLI 专属）：use 里每个名字都必须在锁里有解析记录；
    // 技能面板路径则从 store 索引补锁（见 skills::install::sync_agent_skills）
    let repair = parsed.flag("repair");
    let mut plan = install::SyncPlan::default();
    for name in declared.names {
        let entry = lock
            .get(name)
            .ok_or_else(|| {
                format!(
                    "技能 {name:?} 未在 {} 中解析；用 `av skill add` 补上",
                    declared.lock_path.display()
                )
            })?
            .clone();
        let action = install::classify_sync_entry(&entry, repair)?;
        match &action {
            SyncAction::Ready => println!("✓ {}（已就绪）", name),
            SyncAction::Materialize => {
                println!("↓ {}（将安装 {}）", name, short_hash(&entry.content_hash));
            }
            SyncAction::Repair { .. } => println!("! {}（store 内容漂移，将重装）", name),
            SyncAction::Drift { actual } => {
                return Err(format!(
                    "技能 {name:?} 的 store 内容与锁不一致（{} ≠ {}）；\
                     确认来源可信后加 --repair 重装",
                    actual, entry.content_hash
                ));
            }
        }
        plan.record(entry, action);
    }
    let total = plan.total();
    if total == 0 {
        println!("全部就绪");
        return Ok(());
    }
    if parsed.flag("dry-run") {
        println!("dry-run：将安装 {total} 个技能（未执行）");
        return Ok(());
    }
    confirm(
        parsed.flag("yes"),
        &format!("将从来源安装 {total} 个技能（git 源会联网），继续？"),
    )?;

    let mut log = |line: String| println!("{line}");
    plan.apply(&mut log)?;
    println!("同步完成（{total} 个）");
    Ok(())
}

// ============ list ============

fn cmd_list(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &[], &["declared", "json"])?;
    if parsed.flag("declared") {
        return list_declared(parsed.flag("json"));
    }
    let index = store::read_index()?;
    if parsed.flag("json") {
        let output = serde_json::to_string_pretty(&index.skills)
            .map_err(|e| format!("JSON 序列化失败：{e}"))?;
        println!("{output}");
        return Ok(());
    }
    if index.skills.is_empty() {
        println!("store 为空（{}）", store_label());
        return Ok(());
    }
    for entry in &index.skills {
        println!(
            "{}  {}  {}  {}{}",
            entry.name,
            short_hash(&entry.content_hash),
            source_of(entry),
            entry.installed_at.as_deref().unwrap_or("-"),
            if entry.skill_path.as_deref() == Some(".") {
                String::new()
            } else {
                format!("  [{}]", entry.skill_path.as_deref().unwrap_or("-"))
            }
        );
    }
    Ok(())
}

fn list_declared(json: bool) -> Result<(), String> {
    let discovered = crate::av::discover(Path::new("."))?;
    let Some(declared) = store::winning_use(&discovered.layers) else {
        if json {
            println!("[]");
        } else {
            println!("未声明 [resources.skills].use");
        }
        return Ok(());
    };
    let lock = SkillLock::read(&declared.lock_path)?;
    let mut rows = Vec::new();
    for name in declared.names {
        let (status, hash) = match lock.get(name) {
            None => ("未解析", None),
            Some(entry) => match store::verify_entry(entry)? {
                VerifyOutcome::Ok => ("已就绪", Some(short_hash(&entry.content_hash).to_string())),
                VerifyOutcome::Missing => (
                    "内容缺失",
                    Some(short_hash(&entry.content_hash).to_string()),
                ),
                VerifyOutcome::Mismatch { .. } => (
                    "内容漂移",
                    Some(short_hash(&entry.content_hash).to_string()),
                ),
            },
        };
        rows.push(serde_json::json!({
            "name": name,
            "status": status,
            "hash": hash,
        }));
        if !json {
            println!("{name}  {status}  {}", hash.unwrap_or_default());
        }
    }
    if json {
        let output =
            serde_json::to_string_pretty(&rows).map_err(|e| format!("JSON 序列化失败：{e}"))?;
        println!("{output}");
    }
    Ok(())
}

// ============ verify ============

fn cmd_verify(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &[], &["declared", "json"])?;
    let names: HashSet<&str> = parsed.positional.iter().map(String::as_str).collect();
    let json = parsed.flag("json");

    let entries: Vec<SkillLockEntry> = if parsed.flag("declared") {
        let discovered = crate::av::discover(Path::new("."))?;
        let declared = store::winning_use(&discovered.layers)
            .ok_or("未声明 [resources.skills].use（--declared 需要契约）")?;
        let resolved = store::resolve_declared_skills(declared.names, &declared.lock_path)?;
        resolved.into_iter().map(|skill| skill.entry).collect()
    } else {
        let index = store::read_index()?;
        index
            .skills
            .into_iter()
            .filter(|entry| names.is_empty() || names.contains(entry.name.as_str()))
            .collect()
    };

    if entries.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("没有可校验的技能");
        }
        return Ok(());
    }

    let mut failures = 0;
    let mut rows = Vec::new();
    for entry in &entries {
        let outcome = store::verify_entry(entry)?;
        let (status, actual) = match &outcome {
            VerifyOutcome::Ok => ("ok", None),
            VerifyOutcome::Missing => ("missing", None),
            VerifyOutcome::Mismatch { actual } => ("mismatch", Some(actual.clone())),
        };
        if !matches!(outcome, VerifyOutcome::Ok) {
            failures += 1;
        }
        rows.push(serde_json::json!({
            "name": entry.name,
            "contentHash": entry.content_hash,
            "status": status,
            "actual": actual,
        }));
        if !json {
            match outcome {
                VerifyOutcome::Ok => {
                    println!("✓ {}（{}）", entry.name, short_hash(&entry.content_hash))
                }
                VerifyOutcome::Missing => println!("✗ {}：store 内容缺失", entry.name),
                VerifyOutcome::Mismatch { actual } => println!(
                    "✗ {}：内容漂移（{} ≠ {}）",
                    entry.name, actual, entry.content_hash
                ),
            }
        }
    }
    if json {
        let output =
            serde_json::to_string_pretty(&rows).map_err(|e| format!("JSON 序列化失败：{e}"))?;
        println!("{output}");
    }
    if failures > 0 {
        return Err(format!("verify 失败：{failures} 个技能与记录不一致"));
    }
    Ok(())
}

// ============ update ============

fn cmd_update(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &["contract"], &["dry-run"])?;
    let contract = choose_contract(parsed.value("contract"))?;
    let lock_path = lock_path_for(&contract);
    if !lock_path.is_file() {
        return Err(format!(
            "缺少解析锁 {}；update 只更新已声明的技能（先 `av skill add`）",
            lock_path.display()
        ));
    }
    let mut lock = SkillLock::read(&lock_path)?;
    lock.validate_unique_names()?;
    let names: Vec<String> = if parsed.positional.is_empty() {
        lock.skills.iter().map(|entry| entry.name.clone()).collect()
    } else {
        parsed.positional.clone()
    };
    if names.is_empty() {
        println!("{} 中没有技能", lock_path.display());
        return Ok(());
    }
    let dry_run = parsed.flag("dry-run");
    let mut log = |line: String| println!("{line}");
    let mut changed = 0;
    for name in &names {
        let entry = lock
            .get(name)
            .cloned()
            .ok_or_else(|| format!("技能 {name:?} 不在 {}", lock_path.display()))?;
        match install::resolve_refresh(&entry, &mut log)? {
            None => {}
            Some(refreshed) => {
                if dry_run {
                    println!(
                        "将更新 {}：{} → {}（{}）",
                        name,
                        short_hash(&entry.content_hash),
                        short_hash(&refreshed.entry.content_hash),
                        refreshed
                            .entry
                            .revision
                            .as_deref()
                            .map(short_revision)
                            .unwrap_or("本地来源")
                    );
                } else {
                    install::apply_refresh(&refreshed, &mut log)?;
                    lock.set_entry(refreshed.entry);
                    changed += 1;
                }
            }
        }
    }
    if !dry_run && changed > 0 {
        lock.write(&lock_path)?;
        println!("已更新 {changed} 个技能（{}）", lock_path.display());
    }
    Ok(())
}

// ============ remove ============

fn cmd_remove(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &["contract"], &["purge", "yes"])?;
    let Some(name) = parsed.first_positional() else {
        return Err("用法：av skill remove <名称> [--purge] [--contract 文件] [--yes]".into());
    };
    if !crate::av::skills::valid_skill_name(name) {
        return Err(format!("技能名 {name:?} 形状非法"));
    }
    let contract = choose_contract(parsed.value("contract"))?;
    let lock_path = lock_path_for(&contract);

    let text = fs::read_to_string(&contract)
        .map_err(|e| format!("无法读取 {}: {e}", contract.display()))?;
    let (edited, declared) =
        crate::av::contract_edit::edit_use_names(&text, &[], &[name.to_string()])?;
    let mut lock = SkillLock::read(&lock_path)?;
    let locked = lock.get(name).is_some();
    if !declared && !locked {
        return Err(format!(
            "技能 {name:?} 不在 {} 的 use 或 {} 中",
            contract.display(),
            lock_path.display()
        ));
    }

    if parsed.flag("purge") {
        confirm(
            parsed.flag("yes"),
            &format!("将删除 store 中所有 {name:?} 版本（不可恢复），继续？"),
        )?;
    }

    if declared {
        store::write_text_atomic(&contract, &edited)?;
        println!("已从 {} 移除声明", contract.display());
    }
    if locked {
        lock.remove(name);
        if lock.skills.is_empty() {
            let _ = fs::remove_file(&lock_path);
            println!("已删除 {}", lock_path.display());
        } else {
            lock.write(&lock_path)?;
            println!("已更新 {}", lock_path.display());
        }
    }
    if parsed.flag("purge") {
        let mut index = store::read_index()?;
        let hashes: Vec<String> = index
            .skills
            .iter()
            .filter(|entry| entry.name == name)
            .map(|entry| entry.content_hash.clone())
            .collect();
        for hash in &hashes {
            store::remove_version(name, hash)?;
        }
        index.remove(name);
        store::write_index(&index)?;
        println!("已从 store 清除 {} 个版本", hashes.len());
    }
    Ok(())
}

// ============ search ============

fn cmd_search(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &["limit"], &["json"])?;
    let query = parsed.positional.join(" ");
    if query.trim().is_empty() {
        return Err("用法：av skill search <关键词> [--limit N] [--json]".into());
    }
    let limit = parsed.usize_value("limit")?.unwrap_or(20) as u32;
    run_search(&query, limit, parsed.flag("json"))
}

#[cfg(feature = "search")]
fn run_search(query: &str, limit: u32, json: bool) -> Result<(), String> {
    let results = crate::av::net::search_skills(query, limit)?;
    if json {
        let output =
            serde_json::to_string_pretty(&results).map_err(|e| format!("JSON 序列化失败：{e}"))?;
        println!("{output}");
        return Ok(());
    }
    if results.is_empty() {
        println!("没有结果");
        return Ok(());
    }
    for result in &results {
        println!(
            "{}  （{} 次安装）  {}  {}",
            result.name, result.installs, result.source, result.url
        );
    }
    println!("\n安装：av skill add <owner/repo> --skill <名称>");
    Ok(())
}

#[cfg(not(feature = "search"))]
fn run_search(_query: &str, _limit: u32, _json: bool) -> Result<(), String> {
    Err("该 av 构建未启用 search feature（skills.sh 搜索不可用）".into())
}

fn store_label() -> String {
    crate::av::paths::skills_store()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "~/.av/skills".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::av::install::{self, Selection};
    use crate::av::sources::SourceSpec;
    use crate::av::test_support::{noop_log, setup_av_home, write_skill};
    use std::collections::BTreeMap;

    /// 走 `av skill sync` 的真实分发路径（`cli::run` 把 Err 映射为退出码 1）。
    fn dispatch_sync(project: &Path, extra: &[&str]) -> Result<(), String> {
        let mut argv: Vec<String> = vec!["skill".into(), "sync".into()];
        argv.push(project.to_string_lossy().into_owned());
        argv.extend(extra.iter().map(|flag| (*flag).to_string()));
        super::super::dispatch(&argv)
    }

    /// 递归快照目录树（相对路径 → 文件内容），用于断言「一字未动」。
    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else {
                    out.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        fs::read(&path).unwrap(),
                    );
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    /// 写一个声明项目：`.git`（项目根标记）+ agent.toml（use）+ 同层 agent.lock。
    fn write_project(home: &Path, names: &[&str], lock_entries: &[SkillLockEntry]) -> PathBuf {
        let project = home.join("project");
        fs::create_dir_all(project.join(".git")).unwrap();
        let use_list = names
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        fs::write(
            project.join("agent.toml"),
            format!("schema = 1\n\n[resources.skills]\nuse = [{use_list}]\n"),
        )
        .unwrap();
        let mut lock = SkillLock::empty();
        for entry in lock_entries {
            lock.set_entry(entry.clone());
        }
        lock.write(&project.join(store::SKILL_LOCK_FILENAME))
            .unwrap();
        project
    }

    #[test]
    fn sync_clean_rematerialize_repair_and_strict_lock() {
        let (home, _guard) = setup_av_home("cli-sync");

        // 先装进 store（装 ≠ 启用），再用索引记录写 agent.lock
        let source = home.join("source");
        write_skill(&source, "pdf", "body");
        let spec = SourceSpec::Local {
            path: fs::canonicalize(&source).unwrap(),
        };
        let report =
            install::install_source(&spec, &Selection::All, false, &mut noop_log()).unwrap();
        let entry = report.installed[0].entry.clone();
        let content_dir = report.installed[0].dir.clone();
        let project = write_project(&home, &["pdf"], std::slice::from_ref(&entry));

        // (a) 干净同步：全部就绪 = no-op，退出成功且 store / 锁一字未动
        let before = snapshot(&home);
        assert!(dispatch_sync(&project, &[]).is_ok());
        assert_eq!(snapshot(&home), before, "干净同步不得写任何文件");

        // (b) 内容缺失 → 按锁复现（退出成功，内容哈希回到一致）
        assert!(store::remove_version("pdf", &entry.content_hash).unwrap());
        assert!(dispatch_sync(&project, &["--yes"]).is_ok());
        assert_eq!(store::verify_entry(&entry).unwrap(), VerifyOutcome::Ok);
        assert!(content_dir.join("SKILL.md").is_file());

        // (c) 内容漂移：默认拒绝（非零退出）且不动 store
        fs::write(content_dir.join("SKILL.md"), "tampered").unwrap();
        let before = snapshot(&home);
        let error = dispatch_sync(&project, &[]).unwrap_err();
        assert!(error.contains("--repair"), "{error}");
        assert_eq!(snapshot(&home), before, "拒绝漂移时不得写 store");
        // --dry-run --repair 只预告、不落盘
        assert!(dispatch_sync(&project, &["--dry-run", "--repair"]).is_ok());
        assert_eq!(snapshot(&home), before, "dry-run 不得写 store");
        // --repair --yes：先删漂移目录再复现
        assert!(dispatch_sync(&project, &["--repair", "--yes"]).is_ok());
        assert_eq!(store::verify_entry(&entry).unwrap(), VerifyOutcome::Ok);

        // (d) use 里的名字不在锁里：非零退出，store 不动（CLI 的严格锁契约）
        let project = write_project(&home, &["pdf", "writer"], std::slice::from_ref(&entry));
        let before = snapshot(&home);
        let error = dispatch_sync(&project, &[]).unwrap_err();
        assert!(
            error.contains("writer") && error.contains("未在"),
            "{error}"
        );
        assert_eq!(snapshot(&home), before, "锁不齐时不得动 store");

        // (e) 锁文件缺失：CLI 硬错误（面板路径才允许从索引补锁）
        fs::remove_file(project.join(store::SKILL_LOCK_FILENAME)).unwrap();
        let error = dispatch_sync(&project, &[]).unwrap_err();
        assert!(error.contains("缺少解析锁"), "{error}");

        let _ = fs::remove_dir_all(home);
    }
}
