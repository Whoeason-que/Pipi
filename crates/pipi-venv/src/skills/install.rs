//! 技能安装 / 移除与声明同步：只写 store 与解析锁，不隐式启用。

use crate::av::install::{SyncAction, SyncPlan, classify_sync_entry};
use crate::av::store::{self, SkillLockEntry};

use crate::agents;

use crate::skills::agent_skills::{declared_context, read_lock_lossy};
use crate::skills::library::content_dir;
use crate::skills::model::{InstalledSkillView, SkillInstallReport, SkillSyncReport, SkippedSkill};

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
    let spec = crate::av::sources::parse_spec(source)?;
    let selection = match (skills, all) {
        (Some(_), true) => return Err("Cannot specify both skills list and 'all'".into()),
        (Some(names), false) if !names.is_empty() => {
            for name in names {
                if !crate::av::skills::valid_skill_name(name) {
                    return Err(format!("Invalid skill name {name:?}"));
                }
            }
            crate::av::install::Selection::Only(names.to_vec())
        }
        _ if all => crate::av::install::Selection::All,
        _ => crate::av::install::Selection::Auto,
    };

    let mut log = Vec::new();
    let report =
        crate::av::install::install_source(&spec, &selection, dry_run, &mut |line| log.push(line))?;
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
    if !crate::av::skills::valid_skill_name(name) {
        return Err(format!("Invalid skill name {name:?}"));
    }
    let declaring = agents_declaring(name)?;
    if !declaring.is_empty() {
        return Err(format!(
            "Skill {name:?} is still enabled by these Agent(s): {}; disable it in the skills page before removing",
            declaring.join(", ")
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
        return Err(format!("Skill {name:?} not found in store index"));
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
/// [`crate::av::install::materialize_entry`] 保证）。阻塞且可能联网（git 源）。
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

    // 面板路径的锁策略比 CLI 宽松：锁里没有的名字从 store 索引补解析；
    // 锁里有的走与 `av skill sync` 共用的 verify → classify（只读）。
    let mut plan = SyncPlan::default();
    let mut to_lock: Vec<SkillLockEntry> = Vec::new();
    for name in &context.names {
        if let Some(entry) = lock.get(name) {
            let entry = entry.clone();
            let action = classify_sync_entry(&entry, repair)?;
            match &action {
                SyncAction::Ready => report.ready.push(name.clone()),
                SyncAction::Materialize => report
                    .log
                    .push(format!("↓ {name}: store content missing, reproducing from lock")),
                SyncAction::Repair { actual } => report.log.push(format!(
                    "! {name}: store content drift ({} != {}), reinstalling",
                    short_hash(actual),
                    short_hash(&entry.content_hash)
                )),
                SyncAction::Drift { actual } => {
                    return Err(format!(
                        "Skill {name:?} store content does not match lock ({} != {}); \
                         verify the source is trusted and check 'repair drift' to reinstall",
                        short_hash(actual),
                        short_hash(&entry.content_hash)
                    ));
                }
            }
            plan.record(entry, action);
            continue;
        }
        // 锁里没有：从 store 索引补（离线，等同于 av skill add 的锁定步）
        let entry = store::latest_index_entry(&index, name)
            .ok_or_else(|| format!("Skill {name:?} is neither resolved in lock nor in store; please install before syncing"))?
            .clone();
        report.log.push(format!(
            "+ {name}: added lock entry -> {}",
            short_hash(&entry.content_hash)
        ));
        if content_dir(name, &entry.content_hash).is_none() {
            plan.to_install.push(entry.clone());
        }
        report.resolved.push(name.clone());
        to_lock.push(entry);
    }
    if report.ready.len() == context.names.len() {
        return Ok(report);
    }

    {
        let log = &mut report.log;
        let mut sink = |line: String| log.push(line);
        plan.apply(&mut sink)?;
    }
    for entry in &plan.to_install {
        if !report.resolved.contains(&entry.name) {
            report.installed.push(entry.name.clone());
        }
    }
    for entry in &plan.to_repair {
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
