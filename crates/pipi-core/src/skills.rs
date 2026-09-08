//! 技能（Skills）索引 —— pi 的渐进式披露，最小实现。
//!
//! 移植自 `packages/agent/src/harness/skills.ts` 的 frontmatter 解析与
//! 目录扫描。pi 的做法（也是 pi 拒绝做复杂技能系统的原因）：只有技能的
//! 名称与描述常驻上下文，模型需要时用 read 工具读 SKILL.md 全文 ——
//! 索引 + 按需读取就是全部机制，不需要专用工具。

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

/// 加载 Agent 全局技能与当前工作区的项目技能元数据。
///
/// 全局 `agent/skills` 先于项目 `<cwd>/.pi/skills`，同名技能保留先发现者。
/// 只返回名称、描述和 canonical 路径，SKILL.md 正文仍由模型按需读取。
pub fn load_skill_metadata(agent_dir: Option<&Path>, cwd: Option<&Path>) -> Vec<SkillMeta> {
    let mut skills = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut seen_names = HashSet::new();

    if let Some(agent_dir) = agent_dir {
        load_bounded_skill_dir(
            &agent_dir.join("skills"),
            agent_dir,
            &mut seen_paths,
            &mut seen_names,
            &mut skills,
        );
    }
    if let Some(cwd) = cwd {
        load_bounded_skill_dir(
            &cwd.join(".pi").join("skills"),
            cwd,
            &mut seen_paths,
            &mut seen_names,
            &mut skills,
        );
    }

    skills
}

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
        if let Ok(path) = fs::canonicalize(&declared_skill) {
            if path.starts_with(containment_root) && path.is_file() {
                files.push(path);
            }
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

fn valid_skill_name(name: &str) -> bool {
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

/// 扫描技能目录：`<dir>/<skill>/SKILL.md`。
/// name 取 frontmatter 的 name，缺省用目录名（pi 的行为）。
pub fn scan_skills(skills_dir: &Path) -> Vec<SkillMeta> {
    let mut skills = Vec::new();
    let Ok(entries) = fs::read_dir(skills_dir) else {
        return skills;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let skill_file = dir.join("SKILL.md");
        if !skill_file.is_file() {
            continue;
        }
        let Ok(content) = fs::read_to_string(&skill_file) else {
            continue;
        };
        let (frontmatter, _body) = parse_frontmatter(&content);
        let dir_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let name = frontmatter
            .iter()
            .find(|(k, _)| k == "name")
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or(dir_name);
        let description = frontmatter
            .iter()
            .find(|(k, _)| k == "description")
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty());
        let disable_model_invocation = frontmatter
            .iter()
            .any(|(key, value)| key == "disable-model-invocation" && value == "true");
        skills.push(SkillMeta {
            name,
            description,
            disable_model_invocation,
            path: skill_file,
        });
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// 渲染技能索引段（注入系统提示；全文由模型按需 read）。
pub fn render_skill_index(skills: &[SkillMeta]) -> String {
    let visible_skills = skills
        .iter()
        .filter(|skill| !skill.disable_model_invocation)
        .collect::<Vec<_>>();
    if visible_skills.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n## Skills\n\n以下技能可用。需要时用 read 工具读取对应 SKILL.md 的完整内容再按其行事：\n");
    for skill in visible_skills {
        match &skill.description {
            Some(desc) => out.push_str(&format!(
                "\n- **{}** — {desc}\n  ({})",
                skill.name,
                skill.path.display()
            )),
            None => out.push_str(&format!(
                "\n- **{}**\n  ({})",
                skill.name,
                skill.path.display()
            )),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_skill(dir: &Path, name: &str, content: &str) {
        let d = dir.join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("SKILL.md"), content).unwrap();
    }

    #[test]
    fn frontmatter_parsing() {
        let (fm, body) = parse_frontmatter(
            "---\nname: git-safety\ndescription: \"安全用 git\"\n---\n\n# 正文\n",
        );
        assert_eq!(fm.len(), 2);
        assert_eq!(fm[0], ("name".into(), "git-safety".into()));
        assert_eq!(fm[1], ("description".into(), "安全用 git".into()));
        assert!(body.starts_with("# 正文"));
        // 无 frontmatter
        let (fm, body) = parse_frontmatter("# 普通文档\n");
        assert!(fm.is_empty());
        assert_eq!(body, "# 普通文档");
        // 未闭合
        let (fm, _) = parse_frontmatter("---\nname: x\n");
        assert!(fm.is_empty());
    }

    #[test]
    fn scan_and_index() {
        let dir = std::env::temp_dir().join(format!("pipi-skills-{}", crate::session::new_id()));
        make_skill(
            &dir,
            "git-safety",
            "---\nname: git-safety\ndescription: 安全用 git\n---\n正文",
        );
        make_skill(&dir, "deploy", "# 无 frontmatter\n");
        let skills = scan_skills(&dir);
        assert_eq!(skills.len(), 2);
        // name 排序：deploy < git-safety
        assert_eq!(skills[0].name, "deploy");
        assert_eq!(skills[0].description, None);
        assert_eq!(skills[1].name, "git-safety");
        assert_eq!(skills[1].description.as_deref(), Some("安全用 git"));

        let index = render_skill_index(&skills);
        assert!(index.contains("## Skills"));
        assert!(index.contains("**git-safety** — 安全用 git"));
        assert!(index.contains("SKILL.md"));
    }

    #[test]
    fn loads_global_then_recursive_project_skills() {
        let root =
            std::env::temp_dir().join(format!("pipi-skill-loader-{}", crate::session::new_id()));
        let agent = root.join("agent");
        let global = agent.join("skills");
        let workspace = root.join("workspace");
        let project = workspace.join(".pi/skills/team/nested");
        fs::create_dir_all(&global).unwrap();
        fs::create_dir_all(&project).unwrap();
        make_skill(
            &global,
            "global",
            "---\nname: global\ndescription: global skill\n---\nbody",
        );
        fs::write(
            project.join("SKILL.md"),
            "---\nname: nested\ndescription: nested project skill\n---\nbody",
        )
        .unwrap();

        let skills = load_skill_metadata(Some(&agent), Some(&workspace));

        assert_eq!(
            skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["global", "nested"]
        );
        assert!(skills[1].path.ends_with("SKILL.md"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn declared_skill_directory_stops_nested_discovery() {
        let root =
            std::env::temp_dir().join(format!("pipi-skill-root-stop-{}", crate::session::new_id()));
        let workspace = root.join("workspace");
        let skill = workspace.join(".pi/skills/root-skill");
        let nested = skill.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: root-skill\ndescription: root skill\n---\nbody",
        )
        .unwrap();
        fs::write(
            nested.join("SKILL.md"),
            "---\nname: nested-skill\ndescription: nested skill\n---\nbody",
        )
        .unwrap();

        let skills = load_skill_metadata(None, Some(&workspace));

        assert_eq!(
            skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["root-skill"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_disable_model_invocation_metadata() {
        let root =
            std::env::temp_dir().join(format!("pipi-skill-disabled-{}", crate::session::new_id()));
        let workspace = root.join("workspace");
        let project = workspace.join(".pi/skills");
        fs::create_dir_all(&project).unwrap();
        make_skill(
            &project,
            "disabled",
            "---\nname: disabled\ndescription: explicit only\ndisable-model-invocation: true\n---\nbody",
        );

        let skills = load_skill_metadata(None, Some(&workspace));

        assert_eq!(skills.len(), 1);
        assert!(skills[0].disable_model_invocation);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn global_skill_wins_over_project_duplicate_name() {
        let root =
            std::env::temp_dir().join(format!("pipi-skill-duplicate-{}", crate::session::new_id()));
        let agent = root.join("agent");
        let global = agent.join("skills");
        let workspace = root.join("workspace");
        let project = workspace.join(".pi/skills");
        fs::create_dir_all(&global).unwrap();
        fs::create_dir_all(&project).unwrap();
        make_skill(
            &global,
            "global-shared",
            "---\nname: shared\ndescription: global\n---\nbody",
        );
        make_skill(
            &project,
            "project-shared",
            "---\nname: shared\ndescription: project\n---\nbody",
        );

        let skills = load_skill_metadata(Some(&agent), Some(&workspace));

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description.as_deref(), Some("global"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_skill_metadata_is_omitted() {
        let root =
            std::env::temp_dir().join(format!("pipi-skill-invalid-{}", crate::session::new_id()));
        let workspace = root.join("workspace");
        let project = workspace.join(".pi/skills");
        fs::create_dir_all(&project).unwrap();
        make_skill(
            &project,
            "missing-description",
            "---\nname: missing-description\n---\nbody",
        );
        make_skill(
            &project,
            "invalid-name",
            "---\nname: Invalid_Name\ndescription: invalid\n---\nbody",
        );
        make_skill(
            &project,
            "valid-name",
            "---\nname: valid-name\ndescription: valid\n---\nbody",
        );

        let skills = load_skill_metadata(None, Some(&workspace));

        assert_eq!(
            skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["valid-name"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn rejects_skill_roots_outside_containment_root() {
        let root =
            std::env::temp_dir().join(format!("pipi-skill-external-{}", crate::session::new_id()));
        let agent = root.join("agent");
        let workspace = root.join("workspace");
        let external = root.join("external");
        fs::create_dir_all(&agent).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&external).unwrap();
        make_skill(
            &external,
            "outside",
            "---\nname: outside\ndescription: outside\n---\nbody",
        );

        #[cfg(unix)]
        std::os::unix::fs::symlink(&external, agent.join("skills")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&external, agent.join("skills")).unwrap();

        let skills = load_skill_metadata(Some(&agent), Some(&workspace));

        assert!(skills.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_dir_renders_nothing() {
        assert_eq!(render_skill_index(&[]), "");
        assert!(scan_skills(Path::new("/nonexistent-pipi-skills")).is_empty());
    }
}
