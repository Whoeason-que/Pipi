//! 技能（Skills）索引 —— pi 的渐进式披露，最小实现。
//!
//! 移植自 `packages/agent/src/harness/skills.ts` 的 frontmatter 解析与
//! 目录扫描。pi 的做法（也是 pi 拒绝做复杂技能系统的原因）：只有技能的
//! 名称与描述常驻上下文，模型需要时用 read 工具读 SKILL.md 全文 ——
//! 索引 + 按需读取就是全部机制，不需要专用工具。

use std::fs;
use std::path::{Path, PathBuf};

/// 技能元信息（SKILL.md frontmatter）。
#[derive(Debug, Clone, PartialEq)]
pub struct SkillMeta {
    pub name: String,
    pub description: Option<String>,
    /// SKILL.md 的绝对路径（模型用 read 工具按需加载全文）。
    pub path: PathBuf,
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
        skills.push(SkillMeta {
            name,
            description,
            path: skill_file,
        });
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// 渲染技能索引段（注入系统提示；全文由模型按需 read）。
pub fn render_skill_index(skills: &[SkillMeta]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n## Skills\n\n以下技能可用。需要时用 read 工具读取对应 SKILL.md 的完整内容再按其行事：\n");
    for skill in skills {
        match &skill.description {
            Some(desc) => out.push_str(&format!("\n- **{}** — {desc}\n  ({})", skill.name, skill.path.display())),
            None => out.push_str(&format!("\n- **{}**\n  ({})", skill.name, skill.path.display())),
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
        let (fm, body) = parse_frontmatter("---\nname: git-safety\ndescription: \"安全用 git\"\n---\n\n# 正文\n");
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
        make_skill(&dir, "git-safety", "---\nname: git-safety\ndescription: 安全用 git\n---\n正文");
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
    fn empty_dir_renders_nothing() {
        assert_eq!(render_skill_index(&[]), "");
        assert!(scan_skills(Path::new("/nonexistent-pipi-skills")).is_empty());
    }
}
