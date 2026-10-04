//! 技能（Skills）—— SKILL.md 的发现、解析与过滤（av 标准的一部分）。
//!
//! 渐进式披露（移植自 pi 的最小技能系统）：只有技能的**名称与描述**常驻
//! 上下文，模型需要时用 read 工具按需读取 SKILL.md 全文 —— 索引 + 按需
//! 读取就是全部机制，不需要专用工具。
//!
//! 技能来源（由宿主决定）：
//! - 约定目录（Agent 目录 `skills/`、项目 `.pi/skills`）；
//! - av 契约 `[resources.skills].sources` 的声明式路径；
//! - 全局 store（`~/.av/skills/<name>/<hash16>/`，经 `[resources.skills].use`
//!   声明启用，见 [`crate::store`]）。
//!
//! 本模块只负责**发现与解析**；索引渲染在宿主的 system prompt 组装层。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// 技能元信息（SKILL.md frontmatter）。
#[derive(Debug, Clone, PartialEq)]
pub struct SkillMeta {
    pub name: String,
    pub description: Option<String>,
    /// `true` 时仅允许显式调用，不注入模型可见技能索引。
    pub disable_model_invocation: bool,
    /// SKILL.md 的绝对路径（模型用 read 工具按需加载全文）。
    pub path: PathBuf,
}

/// 扫描一个技能目录：containment_root 之外的路径一律拒绝（canonical 之后判定），
/// 跨调用按路径与技能名去重（先发现者赢）。约定目录与 av 契约 sources 共用。
fn load_bounded_skill_dir(
    skills_dir: &Path,
    containment_root: &Path,
    seen_paths: &mut HashSet<PathBuf>,
    seen_names: &mut HashSet<String>,
    skills: &mut Vec<SkillMeta>,
) {
    let Ok(canonical_root) = fs::canonicalize(containment_root) else {
        return;
    };
    let Ok(canonical_skills_dir) = fs::canonicalize(skills_dir) else {
        return;
    };
    if !canonical_skills_dir.starts_with(&canonical_root) {
        return;
    }

    let mut files = Vec::new();
    let mut visited_dirs = HashSet::new();
    collect_skill_files(
        &canonical_skills_dir,
        &canonical_root,
        &mut visited_dirs,
        &mut files,
    );
    files.sort();

    for path in files {
        let Ok(canonical_path) = fs::canonicalize(&path) else {
            continue;
        };
        if !canonical_path.starts_with(&canonical_root)
            || !seen_paths.insert(canonical_path.clone())
        {
            continue;
        }
        let Some(skill) = parse_skill_metadata(&canonical_path) else {
            continue;
        };
        if seen_names.insert(skill.name.clone()) {
            skills.push(skill);
        }
    }
}

fn collect_skill_files(
    directory: &Path,
    containment_root: &Path,
    visited_dirs: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
) {
    let Ok(directory) = fs::canonicalize(directory) else {
        return;
    };
    if !directory.starts_with(containment_root) || !visited_dirs.insert(directory.clone()) {
        return;
    }

    let declared_skill = directory.join("SKILL.md");
    if declared_skill.exists() {
        if let Ok(path) = fs::canonicalize(&declared_skill)
            && path.starts_with(containment_root)
            && path.is_file()
        {
            files.push(path);
        }
        return;
    }

    let mut entries = fs::read_dir(&directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
        let Some(name) = entry.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let Ok(path) = fs::canonicalize(&entry) else {
            continue;
        };
        if !path.starts_with(containment_root) || !path.is_dir() {
            continue;
        }
        collect_skill_files(&path, containment_root, visited_dirs, files);
    }
}

fn parse_skill_metadata(path: &Path) -> Option<SkillMeta> {
    let content = fs::read_to_string(path).ok()?;
    let (frontmatter, _) = parse_frontmatter(&content);
    let parent_name = path.parent()?.file_name()?.to_str()?.to_string();
    let name = frontmatter
        .iter()
        .find(|(key, _)| key == "name")
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or(parent_name);
    let description = frontmatter
        .iter()
        .find(|(key, _)| key == "description")
        .map(|(_, value)| value.clone())
        .filter(|value| !value.trim().is_empty())?;
    if !valid_skill_name(&name) || description.len() > 1024 {
        return None;
    }
    let disable_model_invocation = frontmatter
        .iter()
        .any(|(key, value)| key == "disable-model-invocation" && value == "true");
    Some(SkillMeta {
        name,
        description: Some(description),
        disable_model_invocation,
        path: path.to_path_buf(),
    })
}

/// 技能名形状：小写字母、数字、`-`，≤64 字符，不以 `-` 开头/结尾、无 `--`。
pub fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

/// 解析 frontmatter：开头 `---` 块内的简单 `key: value` 行。
/// 返回 (键值对, 正文)。不引入 YAML 依赖 —— pi 的技能 frontmatter
/// 实际只用平铺的字符串字段。
pub fn parse_frontmatter(content: &str) -> (Vec<(String, String)>, String) {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return (Vec::new(), normalized.trim().to_string());
    }
    let Some(end) = normalized[3..].find("\n---").map(|i| i + 3) else {
        return (Vec::new(), normalized.trim().to_string());
    };
    let yaml = &normalized[3..end];
    let body = normalized[end + 4..].trim().to_string();
    let mut pairs = Vec::new();
    for line in yaml.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            pairs.push((key.trim().to_string(), value.to_string()));
        }
    }
    (pairs, body)
}

/// 按声明的 sources 扫描技能（av 契约 `[resources.skills].sources`）。
///
/// 每个 source 独立做包含检查（canonical 路径必须落在 containment_root 内），
/// 跨源按 canonical 路径与技能名去重（先发现者赢，与约定目录加载一致）。
pub fn load_skill_sources(sources: &[PathBuf], containment_root: &Path) -> Vec<SkillMeta> {
    let mut skills = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut seen_names = HashSet::new();
    for source in sources {
        load_bounded_skill_dir(
            source,
            containment_root,
            &mut seen_paths,
            &mut seen_names,
            &mut skills,
        );
    }
    skills
}

/// 读取单个技能目录（`<dir>/SKILL.md`）的元信息；不可解析时返回 `None`。
/// 与目录扫描同一套 frontmatter 规则（名称/描述/长度校验）。
pub fn read_skill_meta(dir: &Path) -> Option<SkillMeta> {
    let canonical = fs::canonicalize(dir.join("SKILL.md")).ok()?;
    if !canonical.is_file() {
        return None;
    }
    parse_skill_metadata(&canonical)
}

/// only / exclude 按技能名 glob 过滤（exclude 优先；`only` 缺席 = 全部保留）。
///
/// 模式非法即 fail-closed 报错 —— 宁可拒绝不可放行。
pub fn filter_skills_by_name(
    skills: Vec<SkillMeta>,
    only: Option<&[String]>,
    exclude: Option<&[String]>,
) -> Result<Vec<SkillMeta>, String> {
    let exclude_patterns = compile_name_patterns(exclude)?;
    let only_patterns = compile_name_patterns(only)?;
    Ok(skills
        .into_iter()
        .filter(|skill| {
            name_filter_reason(&skill.name, &exclude_patterns, &only_patterns).is_none()
        })
        .collect())
}

/// 名称被 only / exclude 过滤的原因；未被过滤返回 `None`。
///
/// 与 [`filter_skills_by_name`] 共用同一套 glob 判定 —— 宿主用它向用户解释
/// 「技能声明了但不会注入」，避免在别处重写一遍匹配规则。
pub fn skill_name_filter_reason(
    name: &str,
    only: Option<&[String]>,
    exclude: Option<&[String]>,
) -> Result<Option<String>, String> {
    let exclude_patterns = compile_name_patterns(exclude)?;
    let only_patterns = compile_name_patterns(only)?;
    Ok(name_filter_reason(name, &exclude_patterns, &only_patterns))
}

fn compile_name_patterns(patterns: Option<&[String]>) -> Result<Vec<glob::Pattern>, String> {
    patterns
        .unwrap_or(&[])
        .iter()
        .map(|pattern| {
            glob::Pattern::new(pattern).map_err(|e| format!("技能过滤模式 {pattern:?} 无效：{e}"))
        })
        .collect()
}

fn name_filter_reason(
    name: &str,
    exclude_patterns: &[glob::Pattern],
    only_patterns: &[glob::Pattern],
) -> Option<String> {
    if let Some(pattern) = exclude_patterns
        .iter()
        .find(|pattern| pattern.matches(name))
    {
        return Some(format!("命中 exclude 模式 {:?}", pattern.as_str()));
    }
    if !only_patterns.is_empty() && !only_patterns.iter().any(|pattern| pattern.matches(name)) {
        let patterns = only_patterns
            .iter()
            .map(|pattern| pattern.as_str())
            .collect::<Vec<_>>()
            .join("、");
        return Some(format!("不在 only 模式内（{patterns}）"));
    }
    None
}

#[cfg(test)]
mod tests;
