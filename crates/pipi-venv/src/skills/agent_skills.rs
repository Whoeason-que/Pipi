//! 单个 Agent 的技能启用视图：契约层与 `use` 声明解析、解析锁读取、声明状态
//! 判定与可启用清单。

use std::fs;
use std::path::{Path, PathBuf};

use crate::av::store::{self, SKILL_LOCK_FILENAME, SkillLock, SkillLockEntry};

use crate::agents::{self, AgentDefinition};

use crate::skills::library::content_dir;
use crate::skills::model::{AgentSkill, AgentSkillStatus, AgentSkills, AvailableSkill};

/// 契约层与 `use` 声明位置的解析结果。
pub(super) struct DeclaredContext {
    pub(super) layers: Vec<crate::av::Layer>,
    /// 启用/禁用会写入的契约（最高声明层；无声明时是 Agent 层）。
    pub(super) contract_path: PathBuf,
    pub(super) lock_path: PathBuf,
    pub(super) names: Vec<String>,
    pub(super) declared_by: Option<String>,
    declared_by_path: Option<String>,
    declares_in_project: bool,
    pub(super) agent_contract_path: PathBuf,
    agent_contract_exists: bool,
}

pub(super) fn declared_context(def: &AgentDefinition) -> Result<DeclaredContext, String> {
    let workspace = def.resolve_workspace();
    let (agent_layer, project_layers) = agents::collect_contract_layers(def, workspace.as_deref())?;
    let dir = def
        .dir()
        .ok_or_else(|| format!("无法定位 Agent 目录（{}）", def.name))?;
    let canonical_dir = fs::canonicalize(&dir)
        .map_err(|e| format!("无法规范化 Agent 目录 {}: {e}", dir.display()))?;
    let agent_contract_path = dir.join(crate::av::discovery::AGENT_TOML_FILENAME);
    let agent_contract_exists = agent_contract_path.is_file();

    let mut layers: Vec<crate::av::Layer> = Vec::new();
    if let Some(layer) = agent_layer {
        layers.push(layer);
    }
    layers.extend(project_layers);

    let mut context = DeclaredContext {
        contract_path: agent_contract_path.clone(),
        lock_path: dir.join(SKILL_LOCK_FILENAME),
        names: Vec::new(),
        declared_by: None,
        declared_by_path: None,
        declares_in_project: false,
        agent_contract_path,
        agent_contract_exists,
        layers,
    };
    // winning_use 借用 layers；先把结果取成 owned 值再改 context
    let winner = crate::av::store::winning_use(&context.layers).map(|declared| {
        let kind = if declared.layer.path.parent() == Some(canonical_dir.as_path()) {
            "agent"
        } else if declared.layer.label == crate::av::discovery::AGENT_LOCAL_TOML_FILENAME {
            "project-local"
        } else {
            "project"
        };
        (
            declared.layer.path.clone(),
            declared.lock_path.clone(),
            declared.names.to_vec(),
            kind.to_string(),
            declared.layer.path.display().to_string(),
        )
    });
    if let Some((path, lock_path, names, kind, path_text)) = winner {
        context.contract_path = path;
        context.lock_path = lock_path;
        context.names = names;
        context.declares_in_project = kind != "agent";
        context.declared_by = Some(kind);
        context.declared_by_path = Some(path_text);
    }
    Ok(context)
}

/// 读取解析锁：缺失 → 空锁（调用方按 `names` 判断是否算错误态）；
/// 损坏 → 空锁 + 错误文本，视图照常渲染，写操作另行 fail-closed。
pub(super) fn read_lock_lossy(path: &Path) -> (SkillLock, Option<String>) {
    if !path.is_file() {
        return (SkillLock::empty(), None);
    }
    match SkillLock::read(path) {
        Ok(lock) => (lock, None),
        Err(error) => (SkillLock::empty(), Some(error)),
    }
}

/// 已声明技能的状态判定（与会话启动同一套判据，只做廉价检查）。
///
/// 顺序：锁记录 → store 内容 → SKILL.md → only/exclude 过滤。
pub(super) fn declared_status(
    name: &str,
    lock: &SkillLock,
    only: Option<&[String]>,
    exclude: Option<&[String]>,
) -> Result<(AgentSkillStatus, Option<SkillLockEntry>, Option<PathBuf>), String> {
    let Some(entry) = lock.get(name) else {
        return Ok((AgentSkillStatus::Unresolved, None, None));
    };
    let Some(dir) = content_dir(name, &entry.content_hash) else {
        return Ok((AgentSkillStatus::Missing, Some(entry.clone()), None));
    };
    if crate::av::skills::read_skill_meta(&dir).is_none() {
        return Ok((AgentSkillStatus::Broken, Some(entry.clone()), Some(dir)));
    }
    if crate::av::skills::skill_name_filter_reason(name, only, exclude)?.is_some() {
        return Ok((AgentSkillStatus::Filtered, Some(entry.clone()), Some(dir)));
    }
    Ok((AgentSkillStatus::Ready, Some(entry.clone()), Some(dir)))
}

/// store 索引里某技能当前版本的描述。
fn index_description(index: &SkillLock, name: &str) -> Option<String> {
    let entry = store::latest_index_entry(index, name)?;
    content_dir(name, &entry.content_hash)
        .and_then(|dir| crate::av::skills::read_skill_meta(&dir))
        .and_then(|meta| meta.description)
}

/// 一个 Agent 的技能启用视图（声明 + 状态 + 可启用清单）。
pub fn agent_skills(agent_name: &str) -> Result<AgentSkills, String> {
    let def = agents::load_agent(agent_name)?;
    agent_skills_for(&def)
}

pub(super) fn agent_skills_for(def: &AgentDefinition) -> Result<AgentSkills, String> {
    let context = declared_context(def)?;
    let merged = crate::av::merge_layers(&context.layers)?;
    let filter = merged
        .resources
        .as_ref()
        .and_then(|resources| resources.skills.as_ref());
    let only = filter.and_then(|skills| skills.only.clone());
    let exclude = filter.and_then(|skills| skills.exclude.clone());
    let sources = filter.and_then(|skills| skills.sources.clone());

    let (lock, mut lock_error) = read_lock_lossy(&context.lock_path);
    if lock_error.is_none() && !context.names.is_empty() && !context.lock_path.is_file() {
        lock_error = Some(format!(
            "契约声明了 use，但缺少解析锁 {}：会话启动会失败；用「同步」补齐或重新启用技能",
            context.lock_path.display()
        ));
    }
    let index = store::read_index()?;

    let mut declared = Vec::new();
    for name in &context.names {
        let (status, entry, _dir) =
            declared_status(name, &lock, only.as_deref(), exclude.as_deref())?;
        let filter = if status == AgentSkillStatus::Filtered {
            crate::av::skills::skill_name_filter_reason(name, only.as_deref(), exclude.as_deref())?
        } else {
            None
        };
        let description = entry
            .as_ref()
            .and_then(|entry| content_dir(name, &entry.content_hash))
            .and_then(|dir| crate::av::skills::read_skill_meta(&dir))
            .and_then(|meta| meta.description)
            .or_else(|| index_description(&index, name));
        declared.push(AgentSkill {
            name: name.clone(),
            description,
            status,
            content_hash: entry.as_ref().map(|entry| entry.content_hash.clone()),
            source: entry.as_ref().map(|entry| entry.source.clone()),
            revision: entry.as_ref().and_then(|entry| entry.revision.clone()),
            filter,
        });
    }

    let declared_names: std::collections::HashSet<&str> =
        context.names.iter().map(String::as_str).collect();
    let mut available = Vec::new();
    for entry in &index.skills {
        if declared_names.contains(entry.name.as_str()) {
            continue;
        }
        if available
            .iter()
            .any(|skill: &AvailableSkill| skill.name == entry.name)
        {
            continue;
        }
        let Some(current) = store::latest_index_entry(&index, &entry.name) else {
            continue;
        };
        available.push(AvailableSkill {
            name: entry.name.clone(),
            description: index_description(&index, &entry.name),
            content_hash: current.content_hash.clone(),
            present: content_dir(&entry.name, &current.content_hash).is_some(),
            source: current.source.clone(),
            source_type: current.source_type,
            version_count: index
                .skills
                .iter()
                .filter(|candidate| candidate.name == entry.name)
                .count(),
        });
    }
    available.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(AgentSkills {
        agent_name: def.name.clone(),
        contract_path: context.contract_path.display().to_string(),
        lock_path: context.lock_path.display().to_string(),
        declared_by: context.declared_by,
        declared_by_path: context.declared_by_path,
        agent_contract_exists: context.agent_contract_exists,
        declares_in_project: context.declares_in_project,
        declared,
        available,
        only,
        exclude,
        sources,
        lock_error,
    })
}
