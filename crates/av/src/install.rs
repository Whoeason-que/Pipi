//! 安装管线：把来源中的技能发布进全局 store，并维护 store 索引。
//!
//! - [`install_source`]：`av skill install/add` 的引擎（解析 → 发现 → 发布）；
//! - [`materialize_entry`]：`av skill sync` 的引擎 —— 按锁记录复现精确内容，
//!   内容哈希不一致即拒绝（fail-closed）；
//! - [`refresh_entry`]：`av skill update` 的引擎 —— 重解析来源到新修订。
//!
//! 所有写操作只发生在显式的 CLI 命令里；会话启动只读（见 `store` 模块）。

use std::path::{Path, PathBuf};

use crate::skills::SkillMeta;
use crate::sources::{self, SourceSkill, SourceSpec};
use crate::store::{self, SkillLockEntry, SourceType, VerifyOutcome};

/// 选择要安装哪些技能。
#[derive(Debug, Clone)]
pub enum Selection {
    /// 全部发现的合法技能。
    All,
    /// 指定技能名（必须存在，否则 fail-closed）。
    Only(Vec<String>),
    /// 恰好一个才装；多个时报错并列出可选项（CLI 的默认）。
    Auto,
}

/// 一个已安装/待安装的技能。
#[derive(Debug, Clone)]
pub struct InstalledSkill {
    pub entry: SkillLockEntry,
    /// store 内容目录（dry-run 时为预期路径）。
    pub dir: PathBuf,
    pub meta: SkillMeta,
    /// store 里已有同内容版本（幂等跳过复制）。
    pub unchanged: bool,
}

#[derive(Debug, Default)]
pub struct InstallReport {
    pub installed: Vec<InstalledSkill>,
    /// 发现但被跳过的目录：(相对路径, 原因)。
    pub skipped: Vec<(String, String)>,
}

/// 安装一个来源；`log` 用于向 CLI 输出进展。
pub fn install_source(
    spec: &SourceSpec,
    selection: &Selection,
    dry_run: bool,
    log: &mut dyn FnMut(String),
) -> Result<InstallReport, String> {
    match spec {
        SourceSpec::Local { path } => {
            install_from_root(path, None, spec, selection, dry_run, log, None, None)
        }
        SourceSpec::Git {
            url,
            reference,
            skill_path,
        } => {
            let revision = sources::resolve_revision(url, reference.as_deref())?;
            log(format!(
                "已解析 {} → {}",
                reference.as_deref().unwrap_or("HEAD"),
                short_revision(&revision)
            ));
            let fetch = sources::fetch_at(url, reference.as_deref(), &revision)?;
            install_from_root(
                &fetch.worktree,
                skill_path.as_deref(),
                spec,
                selection,
                dry_run,
                log,
                Some(revision),
                reference.clone(),
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn install_from_root(
    root: &Path,
    subpath: Option<&str>,
    spec: &SourceSpec,
    selection: &Selection,
    dry_run: bool,
    log: &mut dyn FnMut(String),
    revision: Option<String>,
    reference: Option<String>,
) -> Result<InstallReport, String> {
    let discovery = sources::discover_source_skills(root, subpath)?;
    for (relative, reason) in &discovery.skipped {
        log(format!("跳过 {relative}：{reason}"));
    }
    if discovery.skills.is_empty() {
        return Err(format!(
            "来源 {} 中未发现任何技能（需要含有效 SKILL.md 的目录）",
            spec.source_label()
        ));
    }
    let selected = select_skills(&discovery.skills, selection)?;

    let mut report = InstallReport {
        installed: Vec::new(),
        skipped: discovery.skipped,
    };
    let index = store::read_index()?;
    let mut entries = Vec::new();
    for skill in selected {
        let content = store::scan_skill_content(&skill.dir)?;
        let content_hash = store::hash_skill_content(&content)?;
        let unchanged = index.get_version(&skill.name, &content_hash).is_some();
        let dir = if dry_run {
            store::entry_dir(&skill.name, &content_hash)?
        } else {
            store::publish_skill(&content, &skill.name, &content_hash)?
        };
        if !unchanged {
            log(format!(
                "{} {}（{} 个文件）",
                if dry_run { "将安装" } else { "已安装" },
                skill.name,
                content.file_count()
            ));
        }
        let entry = SkillLockEntry {
            name: skill.name.clone(),
            source: spec.source_label(),
            source_type: spec.source_type(),
            reference: reference.clone(),
            revision: revision.clone(),
            skill_path: Some(skill.relative.clone()),
            content_hash,
            installed_at: None,
            updated_at: None,
        };
        entries.push(entry.clone());
        report.installed.push(InstalledSkill {
            entry,
            dir,
            meta: skill.meta.clone(),
            unchanged,
        });
    }
    if !dry_run {
        store::record_index_entries(&entries)?;
    }
    Ok(report)
}

/// 按选择过滤发现的技能；显式选择缺失即 fail-closed（并列出可选项）。
pub fn select_skills<'a>(
    discovered: &'a [SourceSkill],
    selection: &Selection,
) -> Result<Vec<&'a SourceSkill>, String> {
    match selection {
        Selection::All => Ok(discovered.iter().collect()),
        Selection::Only(names) => {
            let mut selected = Vec::new();
            for name in names {
                match discovered.iter().find(|skill| skill.name == *name) {
                    Some(skill) => selected.push(skill),
                    None => {
                        let available = discovered
                            .iter()
                            .map(|skill| skill.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        return Err(format!("来源中没有技能 {name:?}（可用：{available}）"));
                    }
                }
            }
            Ok(selected)
        }
        Selection::Auto => match discovered {
            [single] => Ok(vec![single]),
            _ => {
                let available = discovered
                    .iter()
                    .map(|skill| skill.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(format!(
                    "来源包含 {} 个技能（{available}）：用 --skill <名称>（可重复）选择，或 --all 全部安装",
                    discovered.len()
                ))
            }
        },
    }
}

/// 按锁记录把技能复现到 store（`av skill sync`）。
///
/// 复现出的内容哈希必须等于锁记录的哈希，否则拒绝安装（fail-closed）——
/// 来源被改写、ref 被移动都会在这里被拦下。
pub fn materialize_entry(
    entry: &SkillLockEntry,
    log: &mut dyn FnMut(String),
) -> Result<PathBuf, String> {
    match entry.source_type {
        SourceType::Git => {
            let revision = entry
                .revision
                .as_deref()
                .ok_or_else(|| format!("技能 {:?} 缺少 revision", entry.name))?;
            let fetch = sources::fetch_at(&entry.source, entry.reference.as_deref(), revision)?;
            verify_and_publish(&fetch.worktree, entry, log)
        }
        SourceType::Local => {
            let root = PathBuf::from(&entry.source);
            if !root.is_dir() {
                return Err(format!(
                    "技能 {:?} 的本地来源不存在（{}）：需要来源机器上的同一路径",
                    entry.name,
                    root.display()
                ));
            }
            verify_and_publish(&root, entry, log)
        }
    }
}

fn verify_and_publish(
    root: &Path,
    entry: &SkillLockEntry,
    log: &mut dyn FnMut(String),
) -> Result<PathBuf, String> {
    let skill = locate_skill(root, entry)?;
    let content = store::scan_skill_content(&skill.dir)?;
    let content_hash = store::hash_skill_content(&content)?;
    if content_hash != entry.content_hash {
        return Err(format!(
            "技能 {:?} 的来源内容与锁不一致（期望 {}，实际 {}），拒绝安装（fail-closed）",
            entry.name, entry.content_hash, content_hash
        ));
    }
    let dir = store::publish_skill(&content, &entry.name, &content_hash)?;
    log(format!(
        "已同步 {}（{}）",
        entry.name,
        short_hash(&content_hash)
    ));
    store::record_index_entries(std::slice::from_ref(entry))?;
    Ok(dir)
}

/// 在来源根里定位锁记录指向的技能：优先 `skill_path`，失败则按名发现。
fn locate_skill(root: &Path, entry: &SkillLockEntry) -> Result<SourceSkill, String> {
    if let Some(skill_path) = entry.skill_path.as_deref()
        && skill_path != "."
    {
        let dir = fs_canonical(root.join(skill_path));
        if let Ok(dir) = dir
            && dir.starts_with(root)
            && let Some(meta) = crate::skills::read_skill_meta(&dir)
            && meta.name == entry.name
        {
            return Ok(SourceSkill {
                name: meta.name.clone(),
                dir,
                relative: skill_path.to_string(),
                meta,
            });
        }
    }
    let discovery = sources::discover_source_skills(root, None)?;
    discovery
        .skills
        .into_iter()
        .find(|skill| skill.name == entry.name)
        .ok_or_else(|| {
            format!(
                "来源中找不到技能 {:?}（skill_path={:?}）",
                entry.name, entry.skill_path
            )
        })
}

fn fs_canonical(path: PathBuf) -> Result<PathBuf, String> {
    std::fs::canonicalize(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// 一次 update 的解析结果（尚未落盘，便于 `--dry-run`）。
#[derive(Debug)]
pub struct Refreshed {
    /// 新解析记录（revision / content-hash / skill-path 已更新）。
    pub entry: SkillLockEntry,
    /// 新内容（用于落盘；内容未变时也保留，便于统一处理）。
    pub content: store::SkillContent,
    /// 内容哈希未变（仅修订变化，如 tag 移动）。
    pub content_unchanged: bool,
    /// git 来源的工作树守卫：`content.files` 指向工作树内的文件，
    /// 必须活到 [`apply_refresh`] 复制完成之后（Drop 即清理暂存）。
    pub worktree: Option<sources::GitFetch>,
}

/// 重解析锁条目到新修订（`av skill update` 的解析步；不写盘）。
///
/// - git 源：`ls-remote` 重解析 ref；修订未变 → `Ok(None)`；
/// - local 源：重新扫描目录；内容未变 → `Ok(None)`。
pub fn resolve_refresh(
    entry: &SkillLockEntry,
    log: &mut dyn FnMut(String),
) -> Result<Option<Refreshed>, String> {
    match entry.source_type {
        SourceType::Git => {
            let reference = entry.reference.as_deref();
            let revision = sources::resolve_revision(&entry.source, reference)?;
            if Some(revision.as_str()) == entry.revision.as_deref() {
                log(format!(
                    "{}：已是最新（{}）",
                    entry.name,
                    short_revision(&revision)
                ));
                return Ok(None);
            }
            let fetch = sources::fetch_at(&entry.source, reference, &revision)?;
            let skill = locate_skill(&fetch.worktree, entry)?;
            let content = store::scan_skill_content(&skill.dir)?;
            let content_hash = store::hash_skill_content(&content)?;
            let content_unchanged = content_hash == entry.content_hash;
            let mut updated = entry.clone();
            updated.revision = Some(revision.clone());
            updated.skill_path = Some(skill.relative.clone());
            updated.content_hash = content_hash;
            updated.updated_at = Some(store::now_iso8601());
            Ok(Some(Refreshed {
                entry: updated,
                content,
                content_unchanged,
                worktree: Some(fetch),
            }))
        }
        SourceType::Local => {
            let root = PathBuf::from(&entry.source);
            if !root.is_dir() {
                return Err(format!(
                    "技能 {:?} 的本地来源不存在：{}",
                    entry.name, entry.source
                ));
            }
            let skill = locate_skill(&root, entry)?;
            let content = store::scan_skill_content(&skill.dir)?;
            let content_hash = store::hash_skill_content(&content)?;
            if content_hash == entry.content_hash {
                log(format!("{}：已是最新", entry.name));
                return Ok(None);
            }
            let mut updated = entry.clone();
            updated.content_hash = content_hash;
            updated.skill_path = Some(skill.relative.clone());
            updated.updated_at = Some(store::now_iso8601());
            Ok(Some(Refreshed {
                entry: updated,
                content,
                content_unchanged: false,
                worktree: None,
            }))
        }
    }
}

/// 落盘：发布新内容到 store（内容未变则跳过）并记入 store 索引。
pub fn apply_refresh(
    refreshed: &Refreshed,
    log: &mut dyn FnMut(String),
) -> Result<PathBuf, String> {
    let dir = if refreshed.content_unchanged {
        store::entry_dir(&refreshed.entry.name, &refreshed.entry.content_hash)?
    } else {
        let dir = store::publish_skill(
            &refreshed.content,
            &refreshed.entry.name,
            &refreshed.entry.content_hash,
        )?;
        log(format!(
            "{}：已更新（{}）",
            refreshed.entry.name,
            short_hash(&refreshed.entry.content_hash)
        ));
        dir
    };
    store::record_index_entries(std::slice::from_ref(&refreshed.entry))?;
    Ok(dir)
}

/// 解析 + 落盘（便捷路径，等价于 [`resolve_refresh`] + [`apply_refresh`]）。
pub fn refresh_entry(
    entry: &SkillLockEntry,
    log: &mut dyn FnMut(String),
) -> Result<Option<SkillLockEntry>, String> {
    match resolve_refresh(entry, log)? {
        Some(refreshed) => {
            apply_refresh(&refreshed, log)?;
            Ok(Some(refreshed.entry))
        }
        None => Ok(None),
    }
}

/// `sync --repair`：删除漂移的内容目录（索引记录保留，随后重建）。
pub fn remove_drifted(entry: &SkillLockEntry) -> Result<(), String> {
    let outcome = store::verify_entry(entry)?;
    if let VerifyOutcome::Mismatch { .. } = outcome {
        store::remove_version(&entry.name, &entry.content_hash)?;
    }
    Ok(())
}

fn short_revision(revision: &str) -> &str {
    &revision[..revision.len().min(12)]
}

fn short_hash(content_hash: &str) -> &str {
    let hex = content_hash.strip_prefix("sha256:").unwrap_or(content_hash);
    &hex[..hex.len().min(12)]
}

#[cfg(test)]
mod tests;
