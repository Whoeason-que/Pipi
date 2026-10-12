//! 全局 store 一览与内容校验：读取索引、聚合版本、重算内容哈希，以及技能在
//! 各 Agent 的启用情况。

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::av::store::{self, SkillLockEntry, VerifyOutcome};

use crate::agents;

use crate::skills::agent_skills::{declared_context, declared_status, read_lock_lossy};
use crate::skills::model::{
    AgentSkillStatus, SkillLibrary, SkillUsage, SkillVerify, SkillVerifyStatus, SkillVersion,
    StoreSkill,
};

/// 全局 store 一览（索引 + 内容目录存在性 + 当前版本描述）。
pub fn store_skills() -> Result<SkillLibrary, String> {
    let index = store::read_index()?;
    let store_path = crate::av::paths::skills_store()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "~/.av/skills".to_string());

    let mut by_name: BTreeMap<&str, Vec<&SkillLockEntry>> = BTreeMap::new();
    for entry in &index.skills {
        by_name.entry(entry.name.as_str()).or_default().push(entry);
    }

    let mut skills = Vec::new();
    for (name, entries) in by_name {
        let Some(current) = store::latest_index_entry(&index, name) else {
            continue;
        };
        let view = |entry: &SkillLockEntry| SkillVersion {
            content_hash: entry.content_hash.clone(),
            source: entry.source.clone(),
            source_type: entry.source_type,
            reference: entry.reference.clone(),
            revision: entry.revision.clone(),
            skill_path: entry.skill_path.clone(),
            installed_at: entry.installed_at.clone(),
            updated_at: entry.updated_at.clone(),
            present: content_dir(name, &entry.content_hash).is_some(),
        };
        let mut versions: Vec<SkillVersion> = entries.iter().map(|entry| view(entry)).collect();
        // 新的在前（索引时间戳是定长 RFC3339，字典序即时间序）
        versions.sort_by(|a, b| {
            let stamp = |v: &SkillVersion| {
                (
                    v.updated_at.clone().unwrap_or_default(),
                    v.installed_at.clone().unwrap_or_default(),
                )
            };
            stamp(b).cmp(&stamp(a))
        });
        let description = content_dir(name, &current.content_hash)
            .and_then(|dir| crate::av::skills::read_skill_meta(&dir))
            .and_then(|meta| meta.description);
        skills.push(StoreSkill {
            name: name.to_string(),
            description,
            current_hash: current.content_hash.clone(),
            source: current.source.clone(),
            source_type: current.source_type,
            skill_path: current.skill_path.clone(),
            installed_at: current.installed_at.clone(),
            versions,
        });
    }
    Ok(SkillLibrary { store_path, skills })
}

/// store 内容目录（存在时返回）。
pub(super) fn content_dir(name: &str, content_hash: &str) -> Option<PathBuf> {
    store::entry_dir(name, content_hash)
        .ok()
        .filter(|dir| dir.is_dir())
}

/// 重算 store 内容哈希（`names` 为空 = 全部）。阻塞：内容多时较慢，宿主放阻塞线程池。
pub fn verify_store_skills(names: Option<&[String]>) -> Result<Vec<SkillVerify>, String> {
    let index = store::read_index()?;
    let mut rows = Vec::new();
    for entry in &index.skills {
        if let Some(names) = names
            && !names.is_empty()
            && !names.iter().any(|name| name == &entry.name)
        {
            continue;
        }
        let (status, actual_hash) = match store::verify_entry(entry)? {
            VerifyOutcome::Ok => (SkillVerifyStatus::Ok, None),
            VerifyOutcome::Missing => (SkillVerifyStatus::Missing, None),
            VerifyOutcome::Mismatch { actual } => (SkillVerifyStatus::Mismatch, Some(actual)),
        };
        rows.push(SkillVerify {
            name: entry.name.clone(),
            content_hash: entry.content_hash.clone(),
            status,
            actual_hash,
        });
    }
    Ok(rows)
}

/// 某技能在各个 Agent 上的启用情况（含未启用的 Agent）。
pub fn skill_usage(name: &str) -> Result<Vec<SkillUsage>, String> {
    if !crate::av::skills::valid_skill_name(name) {
        return Err(format!("技能名 {name:?} 形状非法"));
    }
    let mut usage = Vec::new();
    for def in agents::list_agents()? {
        let context = declared_context(&def)?;
        let enabled = context.names.iter().any(|declared| declared == name);
        if !enabled {
            usage.push(SkillUsage {
                agent_name: def.name.clone(),
                enabled: false,
                status: None,
                filter: None,
                declared_by: None,
            });
            continue;
        }
        let merged = crate::av::merge_layers(&context.layers)?;
        let filter = merged
            .resources
            .as_ref()
            .and_then(|resources| resources.skills.as_ref());
        let only = filter.and_then(|skills| skills.only.as_deref());
        let exclude = filter.and_then(|skills| skills.exclude.as_deref());
        let (lock, _lock_error) = read_lock_lossy(&context.lock_path);
        let (status, _entry, _dir) = declared_status(name, &lock, only, exclude)?;
        let filter_hit = if status == AgentSkillStatus::Filtered {
            crate::av::skills::skill_name_filter_reason(name, only, exclude)?
        } else {
            None
        };
        usage.push(SkillUsage {
            agent_name: def.name.clone(),
            enabled: true,
            status: Some(status),
            filter: filter_hit,
            declared_by: context.declared_by,
        });
    }
    Ok(usage)
}
