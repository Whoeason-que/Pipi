use std::fs;
use std::path::{Path, PathBuf};

use pipi_protocol::fs::{ensure_real_directory, ensure_real_file};

use super::paths::checked_agent_dir;
use super::types::AgentDefinition;

/// 解析 Agent 目录内允许 UI 编辑的 Markdown 相对路径。
///
/// 只放行两类：根目录的 `AGENTS.md`，以及 `memory/` 下的 `.md` 文件树
/// （深度 ≤ 3，对齐 memory 工具的 `collect_markdown`）。其余路径一律
/// 拒绝 —— UI 编辑器不是任意文件写入通道。任何组件都不允许 `..`、
/// 点开头或符号链接（fail-closed）。
fn resolve_agent_md_path(dir: &Path, rel_path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(rel_path);
    if relative.is_absolute() {
        return Err(format!("必须是相对路径: {rel_path}"));
    }
    let components: Vec<std::ffi::OsString> = relative
        .components()
        .map(|c| c.as_os_str().to_owned())
        .collect();
    if components.is_empty() {
        return Err("路径不能为空".into());
    }
    let dot_or_escape = components
        .iter()
        .any(|c| c == ".." || c.to_string_lossy().starts_with('.'));
    if dot_or_escape {
        return Err(format!("路径不合法: {rel_path}"));
    }

    let first = components[0].to_string_lossy().into_owned();
    let target = if first == "AGENTS.md" {
        if components.len() != 1 {
            return Err(format!("AGENTS.md 只能是 Agent 目录根下的文件: {rel_path}"));
        }
        dir.join("AGENTS.md")
    } else if first == "memory" {
        if components.len() < 2 || components.len() > 4 {
            return Err(format!("memory 路径深度须为 1-3 层: {rel_path}"));
        }
        let file_name = components.last().unwrap().to_string_lossy().into_owned();
        if !file_name.ends_with(".md") || file_name.len() <= 3 {
            return Err(format!("memory 下只能编辑 .md 文件: {rel_path}"));
        }
        let mut path = dir.to_path_buf();
        for component in &components {
            path.push(component);
        }
        path
    } else {
        return Err(format!("只允许编辑 AGENTS.md 与 memory/*.md: {rel_path}"));
    };

    // 中间目录与目标本身都不能是符号链接/非常规条目
    let mut current = dir.to_path_buf();
    for component in &components {
        current.push(component);
        if current == target {
            ensure_real_file(&current, "目标文件")?;
        } else {
            ensure_real_directory(&current, "路径目录")?;
        }
    }
    Ok(target)
}

/// 读取 Agent 目录内的可编辑 Markdown（AGENTS.md / memory/*.md）。
pub fn read_agent_file(agent_name: &str, rel_path: &str) -> Result<String, String> {
    let dir = checked_agent_dir(agent_name)?;
    let target = resolve_agent_md_path(&dir, rel_path)?;
    if !target.is_file() {
        return Err(format!("文件不存在: {rel_path}"));
    }
    fs::read_to_string(&target).map_err(|e| format!("无法读取 {rel_path}: {e}"))
}

/// 写入 Agent 目录内的可编辑 Markdown（AGENTS.md / memory/*.md）。
/// memory 下的新文件允许创建（父目录自动补齐）；AGENTS.md 必须已存在。
pub fn write_agent_file(agent_name: &str, rel_path: &str, content: &str) -> Result<(), String> {
    let dir = checked_agent_dir(agent_name)?;
    let target = resolve_agent_md_path(&dir, rel_path)?;
    if rel_path == "AGENTS.md" && !target.is_file() {
        return Err("AGENTS.md 不存在（不应删除 Agent 的系统指令文件）".into());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("无法创建目录 {}: {e}", parent.display()))?;
    }
    fs::write(&target, content).map_err(|e| format!("无法写入 {rel_path}: {e}"))
}

/// 列出 Agent 目录内可编辑的 Markdown 相对路径（`AGENTS.md` + `memory/*.md`）。
/// UI 文件编辑器与 memory 索引共用此扫描（复用 memory 工具的 collect_markdown）。
pub fn list_agent_md_files(agent_name: &str) -> Result<Vec<String>, String> {
    let dir = checked_agent_dir(agent_name)?;
    let mut out = Vec::new();
    if dir.join("AGENTS.md").is_file() {
        out.push("AGENTS.md".to_string());
    }
    let mut rels = Vec::new();
    pipi_tools::memory::collect_markdown(&dir.join("memory"), 0, &mut rels);
    rels.sort();
    for rel in rels {
        out.push(format!("memory/{rel}"));
    }
    Ok(out)
}

/// 摘要截断长度（字符）；超长以省略号收尾。
const MEMORY_SUMMARY_MAX_CHARS: usize = 80;

/// 从 memory 文件内容提取摘要：优先首个 `#` 标题，否则首行非空文本。
fn memory_summary(content: &str) -> String {
    let mut fallback = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let candidate = trimmed.trim_start_matches('#').trim();
        if line.trim_start().starts_with('#') && !candidate.is_empty() {
            return truncate_summary(candidate);
        }
        if fallback.is_empty() {
            fallback = candidate.to_string();
        }
    }
    truncate_summary(&fallback)
}

fn truncate_summary(value: &str) -> String {
    let char_count = value.chars().count();
    if char_count <= MEMORY_SUMMARY_MAX_CHARS {
        return value.to_string();
    }
    let head: String = value.chars().take(MEMORY_SUMMARY_MAX_CHARS).collect();
    format!("{head}…")
}

/// 生成 memory 渐进召回索引（空目录返回空 —— 零打扰）。
/// `path` 是绝对路径：模型用 read 工具直接加载正文（memory 目录在
/// build_tool_context 里加入了 read_roots）。
pub fn memory_index(def: &AgentDefinition) -> Vec<crate::prompt::MemoryFileMeta> {
    let Some(dir) = def.memory_dir() else {
        return Vec::new();
    };
    let mut rels = Vec::new();
    pipi_tools::memory::collect_markdown(&dir, 0, &mut rels);
    rels.sort();
    rels.into_iter()
        .map(|rel| {
            let summary = fs::read_to_string(dir.join(&rel))
                .map(|content| memory_summary(&content))
                .unwrap_or_default();
            crate::prompt::MemoryFileMeta {
                path: dir.join(&rel).display().to_string(),
                summary,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::create_agent;
    use pipi_test_support::HOME_LOCK;

    #[test]
    fn agent_file_editor_is_restricted_to_agents_md_and_memory() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("pipi-edit-{}", pipi_protocol::new_id()));
        std::fs::create_dir_all(&home).unwrap();
        let prev = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);
        create_agent("editor", "d", None, None, None, None).unwrap();

        // AGENTS.md 可读写
        write_agent_file("editor", "AGENTS.md", "# 新指令\n").unwrap();
        assert_eq!(
            read_agent_file("editor", "AGENTS.md").unwrap(),
            "# 新指令\n"
        );

        // memory 新文件可创建并读写
        write_agent_file("editor", "memory/user-prefs.md", "偏好： concise\n").unwrap();
        assert!(
            read_agent_file("editor", "memory/user-prefs.md")
                .unwrap()
                .contains("偏好")
        );

        // 拒绝：绝对路径、逃逸、agent.json、其他扩展名、过深路径
        assert!(read_agent_file("editor", "/etc/passwd").is_err());
        assert!(read_agent_file("editor", "../escape.md").is_err());
        assert!(write_agent_file("editor", "agent.json", "{}").is_err());
        assert!(write_agent_file("editor", "memory/notes.txt", "x").is_err());
        assert!(write_agent_file("editor", "skills/tool/SKILL.md", "x").is_err());
        assert!(write_agent_file("editor", "memory/a/b/c/d.md", "x").is_err());
        assert!(read_agent_file("editor", "memory/missing.md").is_err());
        // AGENTS.md 不允许通过 write 新建（已被 create_agent 管理）
        std::fs::remove_file(home.join(".pipi/agents/editor/AGENTS.md")).unwrap();
        assert!(write_agent_file("editor", "AGENTS.md", "x").is_err());

        pipi_test_support::set_env_var("HOME", prev);
        std::fs::remove_dir_all(&home).unwrap();
    }
}
