//! 技能中心：全局技能 store（`~/.av/skills`）的浏览、安装、校验，以及每个
//! Agent 的 `[resources.skills].use` 启用状态。
//!
//! 与 av 的分工：安全语义（内容哈希、限额、锁语义、fail-closed 判定）全部由
//! av 拥有；这里只做 Pipi 侧的策略与视图组装，写操作与 `av skill
//! install/add/sync/remove` 共用同一份 av 实现（[`av::contract_edit`]、
//! [`av::install`]、[`av::store`]）。
//!
//! 不变量与 av 一致：**安装 ≠ 启用**；**会话启动只读、不联网**（`agents.rs`
//! 的解析路径不经过本模块）；这里的写操作只发生在用户显式点击时。
//!
//! 阻塞说明：[`install_skill`] 与 [`sync_agent_skills`] 可能联网（git 源），
//! 宿主必须放到阻塞线程池执行 —— Tauri 用 `spawn_blocking`，不要占用事件循环。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use av::store::{self, SKILL_LOCK_FILENAME, SkillLock, SkillLockEntry, SourceType, VerifyOutcome};
use serde::{Deserialize, Serialize};

use pipi_core::agents::{self, AgentDefinition};

/// 新建 agent.toml 时的最小骨架（编辑前先落一个合法契约）。
const NEW_CONTRACT_SKELETON: &str = "schema = 1\n";

// ============ 视图（前端 DTO，serde camelCase） ============

/// store 里的一条版本记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillVersion {
    pub content_hash: String,
    pub source: String,
    pub source_type: SourceType,
    pub reference: Option<String>,
    pub revision: Option<String>,
    pub skill_path: Option<String>,
    pub installed_at: Option<String>,
    pub updated_at: Option<String>,
    /// 内容目录是否存在（廉价检查；哈希漂移由 [`verify_store_skills`] 判定）。
    pub present: bool,
}

/// store 里的一个技能（同名多版本聚合）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreSkill {
    pub name: String,
    /// 当前版本 SKILL.md 的描述（内容缺失时为 null）。
    pub description: Option<String>,
    /// 当前版本（最近安装/更新）的哈希。
    pub current_hash: String,
    pub source: String,
    pub source_type: SourceType,
    pub skill_path: Option<String>,
    pub installed_at: Option<String>,
    /// 全部版本，新的在前。
    pub versions: Vec<SkillVersion>,
}

/// 全局 store 一览。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillLibrary {
    pub store_path: String,
    pub skills: Vec<StoreSkill>,
}

/// 已声明技能的解析状态（不解引用哈希；漂移由校验/同步判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSkillStatus {
    /// 锁记录 + store 内容 + SKILL.md 齐备，且未被 only/exclude 过滤。
    Ready,
    /// 有锁记录但 store 内容缺失（同步可修复）。
    Missing,
    /// 内容在但 SKILL.md 缺失或不可解析（会话启动会 fail-closed）。
    Broken,
    /// use 声明了，但同层 agent.lock 里没有解析记录。
    Unresolved,
    /// 解析齐备，但被 only / exclude 过滤，不会注入。
    Filtered,
}

/// `use` 里一个技能及其状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    pub name: String,
    pub description: Option<String>,
    pub status: AgentSkillStatus,
    pub content_hash: Option<String>,
    pub source: Option<String>,
    pub revision: Option<String>,
    /// 被 only / exclude 过滤时的原因（命中的模式）。
    pub filter: Option<String>,
}

/// store 里可启用（尚未声明）的技能。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableSkill {
    pub name: String,
    pub description: Option<String>,
    pub content_hash: String,
    pub present: bool,
    pub source: String,
    pub source_type: SourceType,
    pub version_count: usize,
}

/// 一个 Agent 的技能启用视图。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkills {
    pub agent_name: String,
    /// 启用/禁用会写入的契约文件（最高声明层；没有声明时是 Agent 层的 agent.toml）。
    pub contract_path: String,
    /// 与契约同目录的解析锁 `agent.lock`。
    pub lock_path: String,
    /// 当前生效的 `use` 由谁声明："agent" | "project" | "project-local"；没有声明时为 null。
    pub declared_by: Option<String>,
    pub declared_by_path: Option<String>,
    /// Agent 层 agent.toml 是否已存在（不存在时启用会新建）。
    pub agent_contract_exists: bool,
    /// 声明落在项目层（覆盖 Agent 层）时为 true —— 编辑会写项目文件。
    pub declares_in_project: bool,
    pub declared: Vec<AgentSkill>,
    pub available: Vec<AvailableSkill>,
    pub only: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub sources: Option<Vec<String>>,
    /// 锁不可读（损坏）或声明了 use 却缺锁时的可读提示；不阻塞视图渲染。
    pub lock_error: Option<String>,
}

/// store 内容哈希校验结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillVerifyStatus {
    Ok,
    Missing,
    Mismatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillVerify {
    pub name: String,
    pub content_hash: String,
    pub status: SkillVerifyStatus,
    /// 漂移时的实际哈希。
    pub actual_hash: Option<String>,
}

/// 一次安装里落进 store 的技能。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkillView {
    pub name: String,
    pub content_hash: String,
    pub source: String,
    pub source_type: SourceType,
    pub skill_path: Option<String>,
    pub revision: Option<String>,
    /// store 里已有同内容版本（幂等跳过）。
    pub unchanged: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedSkill {
    pub path: String,
    pub reason: String,
}

/// `skill_install` 的结果（含安装日志行）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallReport {
    pub installed: Vec<InstalledSkillView>,
    pub skipped: Vec<SkippedSkill>,
    pub log: Vec<String>,
    pub dry_run: bool,
}

/// `sync_agent_skills` 的结果（含同步日志行）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSyncReport {
    /// 本来就就绪的。
    pub ready: Vec<String>,
    /// 本次补装内容的。
    pub installed: Vec<String>,
    /// 本次重装漂移内容的。
    pub repaired: Vec<String>,
    /// 本次从 store 索引补上解析锁的。
    pub resolved: Vec<String>,
    pub log: Vec<String>,
}

/// 某技能在各个 Agent 上的启用情况（技能库详情用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUsage {
    pub agent_name: String,
    pub enabled: bool,
    pub status: Option<AgentSkillStatus>,
    pub filter: Option<String>,
    pub declared_by: Option<String>,
}

// ============ store 一览 ============

/// 全局 store 一览（索引 + 内容目录存在性 + 当前版本描述）。
pub fn store_skills() -> Result<SkillLibrary, String> {
    let index = store::read_index()?;
    let store_path = av::paths::skills_store()
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
            .and_then(|dir| av::skills::read_skill_meta(&dir))
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
fn content_dir(name: &str, content_hash: &str) -> Option<PathBuf> {
    store::entry_dir(name, content_hash)
        .ok()
        .filter(|dir| dir.is_dir())
}

// ============ 每个 Agent 的启用状态 ============

/// 契约层与 `use` 声明位置的解析结果。
struct DeclaredContext {
    layers: Vec<av::Layer>,
    /// 启用/禁用会写入的契约（最高声明层；无声明时是 Agent 层）。
    contract_path: PathBuf,
    lock_path: PathBuf,
    names: Vec<String>,
    declared_by: Option<String>,
    declared_by_path: Option<String>,
    declares_in_project: bool,
    agent_contract_path: PathBuf,
    agent_contract_exists: bool,
}

fn declared_context(def: &AgentDefinition) -> Result<DeclaredContext, String> {
    let workspace = def.resolve_workspace();
    let (agent_layer, project_layers) = agents::collect_contract_layers(def, workspace.as_deref())?;
    let dir = def
        .dir()
        .ok_or_else(|| format!("无法定位 Agent 目录（{}）", def.name))?;
    let canonical_dir = fs::canonicalize(&dir)
        .map_err(|e| format!("无法规范化 Agent 目录 {}: {e}", dir.display()))?;
    let agent_contract_path = dir.join(av::discovery::AGENT_TOML_FILENAME);
    let agent_contract_exists = agent_contract_path.is_file();

    let mut layers: Vec<av::Layer> = Vec::new();
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
    let winner = av::store::winning_use(&context.layers).map(|declared| {
        let kind = if declared.layer.path.parent() == Some(canonical_dir.as_path()) {
            "agent"
        } else if declared.layer.label == av::discovery::AGENT_LOCAL_TOML_FILENAME {
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
fn read_lock_lossy(path: &Path) -> (SkillLock, Option<String>) {
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
fn declared_status(
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
    if av::skills::read_skill_meta(&dir).is_none() {
        return Ok((AgentSkillStatus::Broken, Some(entry.clone()), Some(dir)));
    }
    if av::skills::skill_name_filter_reason(name, only, exclude)?.is_some() {
        return Ok((AgentSkillStatus::Filtered, Some(entry.clone()), Some(dir)));
    }
    Ok((AgentSkillStatus::Ready, Some(entry.clone()), Some(dir)))
}

/// store 索引里某技能当前版本的描述。
fn index_description(index: &SkillLock, name: &str) -> Option<String> {
    let entry = store::latest_index_entry(index, name)?;
    content_dir(name, &entry.content_hash)
        .and_then(|dir| av::skills::read_skill_meta(&dir))
        .and_then(|meta| meta.description)
}

/// 一个 Agent 的技能启用视图（声明 + 状态 + 可启用清单）。
pub fn agent_skills(agent_name: &str) -> Result<AgentSkills, String> {
    let def = agents::load_agent(agent_name)?;
    agent_skills_for(&def)
}

fn agent_skills_for(def: &AgentDefinition) -> Result<AgentSkills, String> {
    let context = declared_context(def)?;
    let merged = av::merge_layers(&context.layers)?;
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
            av::skills::skill_name_filter_reason(name, only.as_deref(), exclude.as_deref())?
        } else {
            None
        };
        let description = entry
            .as_ref()
            .and_then(|entry| content_dir(name, &entry.content_hash))
            .and_then(|dir| av::skills::read_skill_meta(&dir))
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

// ============ 启用 / 禁用 ============

/// 启用（写入 `use` + 解析锁）或禁用（移除声明与锁记录）一个 store 技能。
///
/// 编辑目标是当前生效的最高声明层；没有任何层声明 `use` 时写 Agent 层的
/// `agent.toml`（不存在则新建合法骨架）。启用要求技能已在 store 中安装；
/// 已有锁记录时不改写版本（锁是解析权威），缺记录才锁到 store 当前版本。
pub fn set_skill_enabled(
    agent_name: &str,
    name: &str,
    enabled: bool,
) -> Result<AgentSkills, String> {
    if !av::skills::valid_skill_name(name) {
        return Err(format!("技能名 {name:?} 形状非法"));
    }
    let def = agents::load_agent(agent_name)?;
    let context = declared_context(&def)?;

    if enabled {
        // 先确认 store 里有内容，再动契约 —— 失败要早于任何写盘
        let index = store::read_index()?;
        let entry = store::latest_index_entry(&index, name)
            .ok_or_else(|| {
                format!("技能 {name:?} 尚未安装到全局 store；先安装再启用（装 ≠ 启用）")
            })?
            .clone();

        let text = contract_text_for_edit(&context)?;
        let (edited, _) = av::contract_edit::edit_use_names(&text, &[name.to_string()], &[])?;
        store::write_text_atomic(&context.contract_path, &edited)?;

        let mut lock = SkillLock::read(&context.lock_path)?;
        lock.validate_unique_names()?;
        if lock.get(name).is_none() {
            lock.set_entry(entry);
        }
        lock.write(&context.lock_path)?;
    } else {
        if context.contract_path.is_file() {
            let text = fs::read_to_string(&context.contract_path)
                .map_err(|e| format!("无法读取 {}: {e}", context.contract_path.display()))?;
            let (edited, changed) =
                av::contract_edit::edit_use_names(&text, &[], &[name.to_string()])?;
            if changed {
                store::write_text_atomic(&context.contract_path, &edited)?;
            }
        }
        if context.lock_path.is_file() {
            let mut lock = SkillLock::read(&context.lock_path)?;
            if lock.remove(name) {
                if lock.skills.is_empty() {
                    fs::remove_file(&context.lock_path)
                        .map_err(|e| format!("无法删除 {}: {e}", context.lock_path.display()))?;
                } else {
                    lock.write(&context.lock_path)?;
                }
            }
        }
    }
    agent_skills_for(&def)
}

/// 取编辑目标契约的文本；不存在且目标就是 Agent 层时用合法骨架（尚未落盘，
/// 编辑成功后才写）。
fn contract_text_for_edit(context: &DeclaredContext) -> Result<String, String> {
    if context.contract_path.is_file() {
        return fs::read_to_string(&context.contract_path)
            .map_err(|e| format!("无法读取 {}: {e}", context.contract_path.display()));
    }
    if context.contract_path != context.agent_contract_path {
        // 声明层文件刚被删掉之类的竞态：明确报错，不凭空重建项目文件
        return Err(format!(
            "契约文件不存在：{}（项目层文件不会被自动创建）",
            context.contract_path.display()
        ));
    }
    Ok(NEW_CONTRACT_SKELETON.to_string())
}

// ============ 校验 ============

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

// ============ 安装 / 移除 ============

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

// ============ 同步 / 修复 ============

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

// ============ 技能 → Agent 影响面 ============

/// 某技能在各个 Agent 上的启用情况（含未启用的 Agent）。
pub fn skill_usage(name: &str) -> Result<Vec<SkillUsage>, String> {
    if !av::skills::valid_skill_name(name) {
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
        let merged = av::merge_layers(&context.layers)?;
        let filter = merged
            .resources
            .as_ref()
            .and_then(|resources| resources.skills.as_ref());
        let only = filter.and_then(|skills| skills.only.as_deref());
        let exclude = filter.and_then(|skills| skills.exclude.as_deref());
        let (lock, _lock_error) = read_lock_lossy(&context.lock_path);
        let (status, _entry, _dir) = declared_status(name, &lock, only, exclude)?;
        let filter_hit = if status == AgentSkillStatus::Filtered {
            av::skills::skill_name_filter_reason(name, only, exclude)?
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

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_test_support::HOME_LOCK;

    /// 隔离的 HOME：`~/.pipi`（Agent 数据）与 `~/.av`（技能 store）都落在临时目录。
    struct TestHome {
        root: PathBuf,
        previous_home: String,
        previous_av: Option<String>,
        previous_pipi: Option<String>,
    }

    impl TestHome {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "pipi-skill-center-{label}-{}",
                pipi_agent::session::new_id()
            ));
            fs::create_dir_all(&root).unwrap();
            let previous_home = std::env::var("HOME").unwrap();
            let previous_av = std::env::var("AV_HOME").ok();
            let previous_pipi = std::env::var("PIPI_HOME").ok();
            pipi_test_support::set_env_var("HOME", &root);
            pipi_test_support::remove_env_var("AV_HOME");
            pipi_test_support::remove_env_var("PIPI_HOME");
            Self {
                root,
                previous_home,
                previous_av,
                previous_pipi,
            }
        }

        fn root(&self) -> &Path {
            &self.root
        }

        fn agent_dir(&self, name: &str) -> PathBuf {
            self.root.join(".pipi").join("agents").join(name)
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            pipi_test_support::set_env_var("HOME", &self.previous_home);
            match &self.previous_av {
                Some(value) => pipi_test_support::set_env_var("AV_HOME", value),
                None => pipi_test_support::remove_env_var("AV_HOME"),
            }
            match &self.previous_pipi {
                Some(value) => pipi_test_support::set_env_var("PIPI_HOME", value),
                None => pipi_test_support::remove_env_var("PIPI_HOME"),
            }
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn write_skill_source(root: &Path) -> PathBuf {
        let source = root.join("source").join("pdf");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: pdf\ndescription: PDF-DESC\n---\nbody",
        )
        .unwrap();
        source
    }

    #[test]
    fn install_enable_disable_and_purge_roundtrip() {
        let _lock = HOME_LOCK.lock().unwrap();
        let home = TestHome::new("roundtrip");
        let source = write_skill_source(home.root());

        // 安装只进 store（装 ≠ 启用）
        let report = install_skill(source.to_str().unwrap(), None, true, false).unwrap();
        assert_eq!(report.installed.len(), 1);
        assert_eq!(report.installed[0].name, "pdf");
        assert!(!report.installed[0].unchanged);

        let library = store_skills().unwrap();
        assert_eq!(library.skills.len(), 1);
        assert_eq!(library.skills[0].description.as_deref(), Some("PDF-DESC"));
        assert!(library.skills[0].versions[0].present);

        let name = format!("skill-agent-{}", pipi_agent::session::new_id());
        let def = agents::create_agent(&name, "", None, None, None, None).unwrap();
        let view = agent_skills(&name).unwrap();
        assert!(view.declared.is_empty(), "安装不等于启用");
        assert_eq!(view.available.len(), 1);
        assert_eq!(view.available[0].name, "pdf");
        assert!(view.available[0].present);
        assert!(!view.agent_contract_exists);
        assert!(view.lock_error.is_none());

        // 启用：写 Agent 层 agent.toml + 同目录 agent.lock
        let view = set_skill_enabled(&name, "pdf", true).unwrap();
        assert_eq!(view.declared.len(), 1);
        assert_eq!(view.declared[0].status, AgentSkillStatus::Ready);
        assert_eq!(view.declared[0].description.as_deref(), Some("PDF-DESC"));
        assert_eq!(view.declared_by.as_deref(), Some("agent"));
        assert!(!view.declares_in_project);
        assert!(view.agent_contract_exists, "启用会新建 agent.toml");
        assert_eq!(view.available.len(), 0, "已声明的技能不再出现在可用列表");
        let contract = fs::read_to_string(home.agent_dir(&name).join("agent.toml")).unwrap();
        assert!(contract.contains("[resources.skills]"), "{contract}");
        assert!(contract.contains("\"pdf\""), "{contract}");
        assert!(home.agent_dir(&name).join("agent.lock").is_file());

        // 启用后的声明能通过会话启动路径（提示词里出现技能描述）
        let prompt = agents::build_system_prompt_with_tools(&def, &[]).unwrap();
        assert!(prompt.contains("PDF-DESC"), "{prompt}");

        // 重复启用幂等
        let again = set_skill_enabled(&name, "pdf", true).unwrap();
        assert_eq!(again.declared.len(), 1);
        assert_eq!(again.declared[0].status, AgentSkillStatus::Ready);

        // 影响面：这个 Agent 已启用
        let usage = skill_usage("pdf").unwrap();
        assert_eq!(usage.len(), 1);
        assert!(usage[0].enabled);
        assert_eq!(usage[0].status, Some(AgentSkillStatus::Ready));

        // 仍被声明的技能拒绝从 store 移除
        let error = remove_store_skill("pdf").unwrap_err();
        assert!(error.contains(&name), "{error}");

        // 禁用：声明与锁记录一起移除，清理空表
        let view = set_skill_enabled(&name, "pdf", false).unwrap();
        assert!(view.declared.is_empty());
        assert_eq!(view.available.len(), 1);
        assert!(!home.agent_dir(&name).join("agent.lock").exists());
        let contract = fs::read_to_string(home.agent_dir(&name).join("agent.toml")).unwrap();
        assert!(!contract.contains("use"), "{contract}");
        assert!(!contract.contains("[resources"), "空表应被清理：{contract}");
        assert!(
            agent_skills(&name).unwrap().lock_error.is_none(),
            "没有声明时缺锁不算错误态"
        );

        // 禁用后可移除
        assert_eq!(remove_store_skill("pdf").unwrap(), 1);
        assert!(store_skills().unwrap().skills.is_empty());
        let error = remove_store_skill("pdf").unwrap_err();
        assert!(error.contains("不在 store 索引"), "{error}");
    }

    #[test]
    fn project_layer_declaration_wins_and_filter_is_reported() {
        let _lock = HOME_LOCK.lock().unwrap();
        let home = TestHome::new("project-layer");
        let source = write_skill_source(home.root());
        install_skill(source.to_str().unwrap(), None, true, false).unwrap();

        let name = format!("skill-agent-{}", pipi_agent::session::new_id());
        // 项目层：随工作区走的 agent.toml + agent.lock
        let project = home.root().join("project");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::write(
            project.join("agent.toml"),
            "schema = 1\n\n[resources.skills]\nuse = [\"pdf\"]\n",
        )
        .unwrap();
        let index = store::read_index().unwrap();
        let entry = store::latest_index_entry(&index, "pdf").unwrap().clone();
        let mut lock = SkillLock::empty();
        lock.set_entry(entry);
        lock.write(&project.join("agent.lock")).unwrap();

        let project_path = project.to_string_lossy().into_owned();
        let def =
            agents::create_agent(&name, "", Some(project_path.as_str()), None, None, None).unwrap();
        let view = agent_skills(&name).unwrap();
        assert_eq!(view.declared_by.as_deref(), Some("project"));
        assert!(view.declares_in_project);
        assert_eq!(view.declared[0].status, AgentSkillStatus::Ready);
        assert!(
            view.declared_by_path
                .as_deref()
                .unwrap()
                .ends_with("agent.toml")
        );

        // only 过滤：解析齐备但不会注入
        fs::write(
            project.join("agent.toml"),
            "schema = 1\n\n[resources.skills]\nuse = [\"pdf\"]\nonly = [\"other-*\"]\n",
        )
        .unwrap();
        let view = agent_skills(&name).unwrap();
        assert_eq!(view.declared[0].status, AgentSkillStatus::Filtered);
        assert!(
            view.declared[0].filter.as_deref().unwrap().contains("only"),
            "{:?}",
            view.declared[0].filter
        );

        // 禁用写的是项目层文件（声明所在层），不新建 Agent 层契约
        let view = set_skill_enabled(&name, "pdf", false).unwrap();
        assert!(view.declared.is_empty());
        assert!(!view.agent_contract_exists);
        let project_text = fs::read_to_string(project.join("agent.toml")).unwrap();
        assert!(!project_text.contains("use"), "{project_text}");
        assert!(!home.agent_dir(&name).join("agent.toml").exists());

        // 声明清空后再启用：退回 Agent 层；项目层的 only 过滤仍生效（数组按层整体替换）
        let view = set_skill_enabled(&name, "pdf", true).unwrap();
        assert_eq!(view.declared_by.as_deref(), Some("agent"));
        assert!(home.agent_dir(&name).join("agent.toml").is_file());
        assert_eq!(view.declared[0].status, AgentSkillStatus::Filtered);
        let _ = def;
    }

    #[test]
    fn sync_restores_missing_content_resolves_lock_and_repairs_drift() {
        let _lock = HOME_LOCK.lock().unwrap();
        let home = TestHome::new("sync");
        let source = write_skill_source(home.root());
        install_skill(source.to_str().unwrap(), None, true, false).unwrap();
        let name = format!("skill-agent-{}", pipi_agent::session::new_id());
        agents::create_agent(&name, "", None, None, None, None).unwrap();
        set_skill_enabled(&name, "pdf", true).unwrap();
        let entry = store::read_index().unwrap().skills[0].clone();

        // 内容缺失 → Missing，同步按锁复现（本地源，不联网）
        store::remove_version("pdf", &entry.content_hash).unwrap();
        let view = agent_skills(&name).unwrap();
        assert_eq!(view.declared[0].status, AgentSkillStatus::Missing);
        let report = sync_agent_skills(&name, false).unwrap();
        assert_eq!(report.installed, vec!["pdf"]);
        assert!(report.repaired.is_empty());
        assert_eq!(
            agent_skills(&name).unwrap().declared[0].status,
            AgentSkillStatus::Ready
        );

        // 内容漂移：默认拒绝，repair 才重装
        let dir = store::entry_dir("pdf", &entry.content_hash).unwrap();
        fs::write(dir.join("SKILL.md"), "tampered").unwrap();
        let error = sync_agent_skills(&name, false).unwrap_err();
        assert!(error.contains("漂移"), "{error}");
        let report = sync_agent_skills(&name, true).unwrap();
        assert_eq!(report.repaired, vec!["pdf"]);
        assert!(
            verify_store_skills(None)
                .unwrap()
                .iter()
                .all(|row| row.status == SkillVerifyStatus::Ok)
        );

        // 缺锁 → Unresolved + 提示；同步从 store 索引补锁
        fs::remove_file(home.agent_dir(&name).join("agent.lock")).unwrap();
        let view = agent_skills(&name).unwrap();
        assert_eq!(view.declared[0].status, AgentSkillStatus::Unresolved);
        assert!(view.lock_error.as_deref().unwrap().contains("agent.lock"));
        let report = sync_agent_skills(&name, false).unwrap();
        assert_eq!(report.resolved, vec!["pdf"]);
        assert!(home.agent_dir(&name).join("agent.lock").is_file());
        assert_eq!(
            agent_skills(&name).unwrap().declared[0].status,
            AgentSkillStatus::Ready
        );
    }

    #[test]
    fn install_reports_git_and_name_validation_errors() {
        let _lock = HOME_LOCK.lock().unwrap();
        let _home = TestHome::new("errors");
        // 非法技能名在动网络之前拒绝
        let error =
            install_skill("owner/repo", Some(&["PDF".to_string()]), false, false).unwrap_err();
        assert!(error.contains("形状非法"), "{error}");
        // 选择冲突
        let error =
            install_skill("owner/repo", Some(&["pdf".to_string()]), true, false).unwrap_err();
        assert!(error.contains("不能同时"), "{error}");
        // 不在 store 里的技能无法启用
        let name = format!("skill-agent-{}", pipi_agent::session::new_id());
        agents::create_agent(&name, "", None, None, None, None).unwrap();
        let error = set_skill_enabled(&name, "pdf", true).unwrap_err();
        assert!(error.contains("尚未安装"), "{error}");
    }
}
