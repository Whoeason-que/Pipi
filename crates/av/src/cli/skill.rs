//! `av skill` 子命令：搜索、安装、声明、同步、校验、更新、移除。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use av::install::{self, InstallReport, Selection};
use av::sources::{self, SourceSpec};
use av::store::{self, SkillLock, SkillLockEntry, SourceType, VerifyOutcome};

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
            if !av::skills::valid_skill_name(name) {
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
        if let Some(existing) = lock.get(&skill.entry.name) {
            if existing.source != skill.entry.source {
                conflicts.push(format!(
                    "{}（{} → {}）",
                    skill.entry.name, existing.source, skill.entry.source
                ));
            }
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
    let edited = add_use_names(&text, &names)?;
    // 编辑结果先过 schema 校验再落盘（fail-closed）
    let config: av::AgentToml =
        toml::from_str(&edited).map_err(|e| format!("编辑后的契约无法解析：{e}"))?;
    config.validate()?;
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
    let discovered = av::discover(&dir)?;
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

    let mut to_install: Vec<&SkillLockEntry> = Vec::new();
    let mut to_repair: Vec<&SkillLockEntry> = Vec::new();
    for name in declared.names {
        let entry = lock.get(name).ok_or_else(|| {
            format!(
                "技能 {name:?} 未在 {} 中解析；用 `av skill add` 补上",
                declared.lock_path.display()
            )
        })?;
        match store::verify_entry(entry)? {
            VerifyOutcome::Ok => println!("✓ {}（已就绪）", name),
            VerifyOutcome::Missing => {
                println!("↓ {}（将安装 {}）", name, short_hash(&entry.content_hash));
                to_install.push(entry);
            }
            VerifyOutcome::Mismatch { actual } => {
                if parsed.flag("repair") {
                    println!("! {}（store 内容漂移，将重装）", name);
                    to_repair.push(entry);
                } else {
                    return Err(format!(
                        "技能 {name:?} 的 store 内容与锁不一致（{} ≠ {}）；\
                         确认来源可信后加 --repair 重装",
                        actual, entry.content_hash
                    ));
                }
            }
        }
    }
    let total = to_install.len() + to_repair.len();
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
    for entry in &to_repair {
        install::remove_drifted(entry)?;
    }
    for entry in to_install.iter().chain(to_repair.iter()) {
        install::materialize_entry(entry, &mut log)?;
    }
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
    let discovered = av::discover(Path::new("."))?;
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
        let discovered = av::discover(Path::new("."))?;
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
    if !av::skills::valid_skill_name(name) {
        return Err(format!("技能名 {name:?} 形状非法"));
    }
    let contract = choose_contract(parsed.value("contract"))?;
    let lock_path = lock_path_for(&contract);

    let text = fs::read_to_string(&contract)
        .map_err(|e| format!("无法读取 {}: {e}", contract.display()))?;
    let (edited, declared) = remove_use_name(&text, name)?;
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
        let config: av::AgentToml =
            toml::from_str(&edited).map_err(|e| format!("编辑后的契约无法解析：{e}"))?;
        config.validate()?;
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
    let results = av::net::search_skills(query, limit)?;
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

// ============ agent.toml 编辑（保格式） ============

/// 向 `[resources.skills].use` 追加技能名（去重；字段自带注释与格式保留）。
pub fn add_use_names(text: &str, names: &[String]) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("解析契约失败：{e}"))?;
    let resources = ensure_standard_table(doc.as_table_mut(), "resources")?;
    let skills = ensure_standard_table(resources, "skills")?;
    let array = ensure_use_array(skills)?;
    for name in names {
        if !array
            .iter()
            .any(|value| value.as_str() == Some(name.as_str()))
        {
            array.push(name.as_str());
        }
    }
    Ok(doc.to_string())
}

/// 从 `[resources.skills].use` 移除技能名；数组清空时移除 `use` 键。
/// 返回 (新文本, 是否找到并移除)。
pub fn remove_use_name(text: &str, name: &str) -> Result<(String, bool), String> {
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("解析契约失败：{e}"))?;
    let Some(resources) = doc
        .as_table_mut()
        .get_mut("resources")
        .and_then(toml_edit::Item::as_table_mut)
    else {
        return Ok((text.to_string(), false));
    };
    let Some(skills) = resources
        .get_mut("skills")
        .and_then(toml_edit::Item::as_table_mut)
    else {
        return Ok((text.to_string(), false));
    };
    let Some(item) = skills.get_mut("use") else {
        return Ok((text.to_string(), false));
    };
    let array = item
        .as_array_mut()
        .ok_or("resources.skills.use 不是数组（契约非法）")?;
    let mut found = false;
    let mut index = 0;
    while index < array.len() {
        if array.get(index).and_then(toml_edit::Value::as_str) == Some(name) {
            array.remove(index);
            found = true;
        } else {
            index += 1;
        }
    }
    if !found {
        return Ok((text.to_string(), false));
    }
    if array.is_empty() {
        skills.remove("use");
    }
    // 清理因移除而变空的表（合同保持干净；含其它键的表不动）
    let skills_empty = skills.is_empty();
    if skills_empty {
        resources.remove("skills");
    }
    if resources.is_empty() {
        doc.as_table_mut().remove("resources");
    }
    Ok((doc.to_string(), true))
}

fn ensure_standard_table<'a>(
    table: &'a mut toml_edit::Table,
    key: &str,
) -> Result<&'a mut toml_edit::Table, String> {
    if table.get(key).is_none() {
        table.insert(key, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    table
        .get_mut(key)
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| {
            format!("契约里的 {key} 不是标准表（内联表/数组形式不支持自动编辑，请手工展开）")
        })
}

fn ensure_use_array(table: &mut toml_edit::Table) -> Result<&mut toml_edit::Array, String> {
    if table.get("use").is_none() {
        table.insert(
            "use",
            toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::new())),
        );
    }
    table
        .get_mut("use")
        .and_then(toml_edit::Item::as_array_mut)
        .ok_or_else(|| "resources.skills.use 不是数组（契约非法）".to_string())
}

fn store_label() -> String {
    av::paths::skills_store()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "~/.av/skills".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_remove_use_names_preserve_format() {
        let text = "# 契约\nschema = 1\n\n[env]\nset = { A = \"1\" }\n";
        let edited = add_use_names(text, &["pdf".into(), "writer".into()]).unwrap();
        assert!(edited.contains("# 契约"), "注释保留：{edited}");
        assert!(
            edited.contains("set = { A = \"1\" }"),
            "既有内容保留：{edited}"
        );
        let config: av::AgentToml = toml::from_str(&edited).unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.resources.unwrap().skills.unwrap().use_.as_deref(),
            Some(&["pdf".to_string(), "writer".to_string()][..])
        );

        // 幂等：重复添加不产生重复项
        let edited = add_use_names(&edited, &["pdf".into()]).unwrap();
        let config: av::AgentToml = toml::from_str(&edited).unwrap();
        assert_eq!(
            config
                .resources
                .unwrap()
                .skills
                .unwrap()
                .use_
                .unwrap()
                .len(),
            2
        );

        // 移除一个
        let (edited, found) = remove_use_name(&edited, "pdf").unwrap();
        assert!(found);
        let config: av::AgentToml = toml::from_str(&edited).unwrap();
        assert_eq!(
            config.resources.unwrap().skills.unwrap().use_.as_deref(),
            Some(&["writer".to_string()][..])
        );

        // 移除最后一个 → use 键消失，且空表被清理
        let (edited, found) = remove_use_name(&edited, "writer").unwrap();
        assert!(found);
        assert!(!edited.contains("use"), "{edited}");
        assert!(!edited.contains("[resources"), "空表应被清理：{edited}");
        let config: av::AgentToml = toml::from_str(&edited).unwrap();
        assert!(config.resources.is_none());

        // 移除不存在的名字 → 原样返回
        let (edited, found) = remove_use_name(&edited, "nope").unwrap();
        assert!(!found);
        assert!(edited.contains("schema = 1"));
    }

    #[test]
    fn existing_use_array_is_extended_in_place() {
        let text = "schema = 1\n[resources.skills]\nuse = [\"a\"]  # 手动维护\n";
        let edited = add_use_names(text, &["b".into()]).unwrap();
        assert!(edited.contains("# 手动维护"), "{edited}");
        let config: av::AgentToml = toml::from_str(&edited).unwrap();
        assert_eq!(
            config.resources.unwrap().skills.unwrap().use_.as_deref(),
            Some(&["a".to_string(), "b".to_string()][..])
        );
    }

    #[test]
    fn inline_table_is_rejected_with_hint() {
        let text = "schema = 1\nresources = { max-bytes = 100 }\n";
        let error = add_use_names(text, &["pdf".into()]).unwrap_err();
        assert!(error.contains("标准表"), "{error}");
    }
}
