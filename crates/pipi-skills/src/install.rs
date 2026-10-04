//! 技能安装 / 移除与声明同步：只写 store 与解析锁，不隐式启用。

use av::store::{self, SkillLockEntry, VerifyOutcome};

use pipi_core::agents;

use crate::agent_skills::{declared_context, read_lock_lossy};
use crate::library::content_dir;
use crate::model::{InstalledSkillView, SkillInstallReport, SkillSyncReport, SkippedSkill};

/// 从来源安装技能到全局 store（**只装不启用**）。`skills` 指定要装的技能名，
/// `all` 全装，两者都不给 = 来源里恰好一个才装（与 `av skill install` 一致）。
///
/// 阻塞且可能联网（git 源）：宿主必须放阻塞线程池执行。
pub fn install_skill(
    source: &str,
    skills: Option<&[String]>,
    all: bool,
    dry_run: bool,
) -> Result<SkillInstallReport, String> {
    let spec = av::sources::parse_spec(source)?;
    let selection = match (skills, all) {
        (Some(_), true) => return Err("不能同时指定技能列表与「全部安装」".into()),
        (Some(names), false) if !names.is_empty() => {
            for name in names {
                if !av::skills::valid_skill_name(name) {
                    return Err(format!("技能名 {name:?} 形状非法"));
                }
            }
            av::install::Selection::Only(names.to_vec())
        }
        _ if all => av::install::Selection::All,
        _ => av::install::Selection::Auto,
    };

    let mut log = Vec::new();
    let report =
        av::install::install_source(&spec, &selection, dry_run, &mut |line| log.push(line))?;
    Ok(SkillInstallReport {
        installed: report
            .installed
            .into_iter()
            .map(|skill| InstalledSkillView {
                name: skill.entry.name,
                content_hash: skill.entry.content_hash,
                source: skill.entry.source,
                source_type: skill.entry.source_type,
                skill_path: skill.entry.skill_path,
                revision: skill.entry.revision,
                unchanged: skill.unchanged,
            })
            .collect(),
        skipped: report
            .skipped
            .into_iter()
            .map(|(path, reason)| SkippedSkill { path, reason })
            .collect(),
        log,
        dry_run,
    })
}

/// 从 store 移除一个技能的全部版本（`av skill remove --purge` 的 store 侧）。
///
/// **拒绝移除仍被任何 Agent 声明启用的技能** —— 先在各 Agent 的技能页禁用，
/// 否则会话启动会 fail-closed。返回移除的版本数。
pub fn remove_store_skill(name: &str) -> Result<usize, String> {
    if !av::skills::valid_skill_name(name) {
        return Err(format!("技能名 {name:?} 形状非法"));
    }
    let declaring = agents_declaring(name)?;
    if !declaring.is_empty() {
        return Err(format!(
            "技能 {name:?} 仍被这些 Agent 声明启用：{}；先在技能页禁用后再移除",
            declaring.join("、")
        ));
    }
    let mut index = store::read_index()?;
    let hashes: Vec<String> = index
        .skills
        .iter()
        .filter(|entry| entry.name == name)
        .map(|entry| entry.content_hash.clone())
        .collect();
    if hashes.is_empty() {
        return Err(format!("技能 {name:?} 不在 store 索引中"));
    }
    for hash in &hashes {
        store::remove_version(name, hash)?;
    }
    index.remove(name);
    store::write_index(&index)?;
    Ok(hashes.len())
}

/// 声明启用某技能的全部 Agent（活跃 + 归档，按名字去重）。
fn agents_declaring(name: &str) -> Result<Vec<String>, String> {
    let mut defs = agents::list_agents()?;
    defs.extend(agents::list_archived_agents()?);
    let mut found = Vec::new();
    for def in defs {
        if found.iter().any(|existing| existing == &def.name) {
            continue;
        }
        let context = declared_context(&def)?;
        if context.names.iter().any(|declared| declared == name) {
            found.push(def.name);
        }
    }
    Ok(found)
}

/// 按 `use` + `agent.lock` 把声明技能物化到 store：
/// 缺锁记录从 store 索引补锁、缺内容复现、漂移内容重装（仅 `repair = true`）。
///
/// 复现出的内容哈希必须等于锁记录，否则拒绝（fail-closed 由
/// [`av::install::materialize_entry`] 保证）。阻塞且可能联网（git 源）。
pub fn sync_agent_skills(agent_name: &str, repair: bool) -> Result<SkillSyncReport, String> {
    let def = agents::load_agent(agent_name)?;
    let context = declared_context(&def)?;
    let mut report = SkillSyncReport {
        ready: Vec::new(),
        installed: Vec::new(),
        repaired: Vec::new(),
        resolved: Vec::new(),
        log: Vec::new(),
    };
    if context.names.is_empty() {
        return Ok(report);
    }
    let (mut lock, lock_error) = read_lock_lossy(&context.lock_path);
    if let Some(error) = lock_error {
        return Err(error);
    }
    let index = store::read_index()?;

    let mut to_install: Vec<SkillLockEntry> = Vec::new();
    let mut to_repair: Vec<SkillLockEntry> = Vec::new();
    let mut to_lock: Vec<SkillLockEntry> = Vec::new();
    for name in &context.names {
        match lock.get(name) {
            Some(entry) => {
                let entry = entry.clone();
                match store::verify_entry(&entry)? {
                    VerifyOutcome::Ok => report.ready.push(name.clone()),
                    VerifyOutcome::Missing => {
                        report
                            .log
                            .push(format!("↓ {name}：store 内容缺失，将按锁复现"));
                        to_install.push(entry);
                    }
                    VerifyOutcome::Mismatch { actual } => {
                        if repair {
                            report.log.push(format!(
                                "! {name}：store 内容漂移（{} ≠ {}），将重装",
                                short_hash(&actual),
                                short_hash(&entry.content_hash)
                            ));
                            to_repair.push(entry);
                        } else {
                            return Err(format!(
                                "技能 {name:?} 的 store 内容与锁不一致（{} ≠ {}）；\
                                 确认来源可信后勾选「修复漂移」重装",
                                short_hash(&actual),
                                short_hash(&entry.content_hash)
                            ));
                        }
                    }
                }
            }
            None => {
                // 锁里没有：从 store 索引补（离线，等同于 av skill add 的锁定步）
                let entry = store::latest_index_entry(&index, name)
                    .ok_or_else(|| {
                        format!("技能 {name:?} 未在锁中解析，也不在 store 中；先安装再同步")
                    })?
                    .clone();
                report.log.push(format!(
                    "＋ {name}：补解析锁 → {}",
                    short_hash(&entry.content_hash)
                ));
                if content_dir(name, &entry.content_hash).is_none() {
                    to_install.push(entry.clone());
                }
                report.resolved.push(name.clone());
                to_lock.push(entry);
            }
        }
    }
    if report.ready.len() == context.names.len() {
        return Ok(report);
    }

    {
        let log = &mut report.log;
        let mut sink = |line: String| log.push(line);
        for entry in &to_repair {
            av::install::remove_drifted(entry)?;
        }
        for entry in to_install.iter().chain(to_repair.iter()) {
            av::install::materialize_entry(entry, &mut sink)?;
        }
    }
    for entry in &to_install {
        if !report.resolved.contains(&entry.name) {
            report.installed.push(entry.name.clone());
        }
    }
    for entry in &to_repair {
        report.repaired.push(entry.name.clone());
    }
    if !to_lock.is_empty() {
        for entry in to_lock {
            lock.set_entry(entry);
        }
        lock.write(&context.lock_path)?;
    }
    Ok(report)
}

/// 内容哈希的短标识（日志与提示用；哈希已由 av 校验过形状）。
fn short_hash(hash: &str) -> String {
    hash.strip_prefix("sha256:")
        .unwrap_or(hash)
        .chars()
        .take(8)
        .collect()
}
