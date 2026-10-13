use std::fs;
use std::path::{Path, PathBuf};

use pipi_protocol::fs::{ensure_real_directory, ensure_real_file};

use super::paths::checked_agent_dir;

/// 可编辑素材视图（wire 形状见前端 `types.ts` 的同名类型）。
///
/// 指令文件与记忆文件分开列：路径前缀可能互相嵌套（比如二者都在 `notes/` 下），
/// 前端靠 `instructions[0]` 决定默认打开哪个文件，不做前缀猜测。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEditableFiles {
    /// 指令文件的相对路径（按契约声明顺序）。
    pub instructions: Vec<String>,
    /// 记忆文件的相对路径（`.md`，深度 ≤ 3）。
    pub memory: Vec<String>,
    /// 记忆目录的相对前缀（`/` 分隔、无尾斜杠），UI 用它拼新文件名。
    pub memory_prefix: String,
}

/// 解析 Agent 侧素材（编辑器与 memory 索引共用同一条契约路径）。
///
/// 返回 `(canonical Agent 目录, 契约声明的素材)`：缺 agent.toml 或漏声明
/// 即 fail-closed（与提示词、工具上下文同一套规则）。
fn agent_material_for(
    agent_name: &str,
) -> Result<(PathBuf, super::contract::AgentMaterial), String> {
    let dir = fs::canonicalize(checked_agent_dir(agent_name)?)
        .map_err(|e| format!("无法规范化 Agent 目录：{e}"))?;
    let def = super::registry::load_agent(agent_name)?;
    let (agent_layer, _) = super::contract::collect_contract_layers(&def, None)?;
    let material = super::contract::agent_material(agent_layer.as_ref(), &dir)?;
    Ok((dir, material))
}

/// memory 目录相对 Agent 目录的前缀（`/` 分隔、无尾斜杠）。
fn memory_prefix(dir: &Path, material: &super::contract::AgentMaterial) -> Result<String, String> {
    let relative = material
        .memory_dir
        .strip_prefix(dir)
        .map_err(|_| "记忆目录不在 Agent 目录内（fail-closed）".to_string())?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

/// 解析允许 UI 编辑的 Markdown 相对路径。
///
/// 放行两类，与契约一致：**声明过的指令文件**，以及 **memory 目录下的
/// `.md` 文件树**（相对 memory 目录深度 ≤ 3，对齐 memory 工具的
/// `collect_markdown`）。其余路径一律拒绝 —— UI 编辑器不是任意文件写入通道。
/// 任何组件都不允许 `..`、点开头或符号链接（fail-closed）。
fn resolve_agent_md_path(
    dir: &Path,
    material: &super::contract::AgentMaterial,
    rel_path: &str,
) -> Result<PathBuf, String> {
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
    let mut target = dir.to_path_buf();
    for component in &components {
        target.push(component);
    }
    let canonical_target = fs::canonicalize(&target).unwrap_or_else(|_| target.clone());

    let declared = material
        .instructions
        .iter()
        .any(|instruction| instruction.path == canonical_target);
    if !declared {
        if !canonical_target.starts_with(&material.memory_dir) {
            return Err(format!(
                "只允许编辑契约声明的指令文件与记忆目录（{}）下的 .md: {rel_path}",
                material.memory_dir.display()
            ));
        }
        let mut depth = components.len();
        if let Ok(relative_to_memory) = canonical_target.strip_prefix(&material.memory_dir) {
            depth = relative_to_memory.components().count();
        }
        if !(1..=3).contains(&depth) {
            return Err(format!("记忆文件路径深度须为 1-3 层: {rel_path}"));
        }
        let file_name = components.last().unwrap().to_string_lossy().into_owned();
        if !file_name.ends_with(".md") || file_name.len() <= 3 {
            return Err(format!("记忆目录下只能编辑 .md 文件: {rel_path}"));
        }
    }

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

/// 读取 Agent 目录内可编辑的 Markdown（契约声明的指令文件 / 记忆文件）。
pub fn read_agent_file(agent_name: &str, rel_path: &str) -> Result<String, String> {
    let (dir, material) = agent_material_for(agent_name)?;
    let target = resolve_agent_md_path(&dir, &material, rel_path)?;
    if !target.is_file() {
        return Err(format!("文件不存在: {rel_path}"));
    }
    fs::read_to_string(&target).map_err(|e| format!("无法读取 {rel_path}: {e}"))
}

/// 写入 Agent 目录内可编辑的 Markdown（契约声明的指令文件 / 记忆文件）。
/// 记忆下的新文件允许创建（父目录自动补齐）；指令文件必须已存在
/// （不该被删除后从 UI 重建）。
pub fn write_agent_file(agent_name: &str, rel_path: &str, content: &str) -> Result<(), String> {
    let (dir, material) = agent_material_for(agent_name)?;
    let target = resolve_agent_md_path(&dir, &material, rel_path)?;
    let canonical_target = fs::canonicalize(&target).unwrap_or_else(|_| target.clone());
    let declared = material
        .instructions
        .iter()
        .any(|instruction| instruction.path == canonical_target);
    if declared && !target.is_file() {
        return Err("指令文件不存在（不应删除 Agent 的系统指令文件）".into());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("无法创建目录 {}: {e}", parent.display()))?;
    }
    fs::write(&target, content).map_err(|e| format!("无法写入 {rel_path}: {e}"))
}

/// 列出 Agent 目录内可编辑的 Markdown 相对路径与 memory 前缀：
/// 先按声明顺序列出指令文件，再列出记忆目录下的 `.md`。
/// UI 文件编辑器与 memory 索引共用同一条契约路径。
pub fn list_agent_editable_files(agent_name: &str) -> Result<AgentEditableFiles, String> {
    let (dir, material) = agent_material_for(agent_name)?;
    let prefix = memory_prefix(&dir, &material)?;
    let instructions: Vec<String> = material
        .instructions
        .iter()
        .filter_map(|instruction| instruction.path.strip_prefix(&dir).ok())
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect();
    let mut rels = Vec::new();
    pipi_tools::memory::collect_markdown(&material.memory_dir, 0, &mut rels);
    rels.sort();
    let memory: Vec<String> = rels
        .into_iter()
        .map(|rel| format!("{prefix}/{rel}"))
        .collect();
    Ok(AgentEditableFiles {
        instructions,
        memory,
        memory_prefix: prefix,
    })
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
/// `dir` 是契约声明的记忆目录（绝对路径）：模型用 read 工具直接加载正文
/// （该目录在 build_tool_context 里加入了 read_roots）。
pub fn memory_index(dir: &Path) -> Vec<crate::prompt::MemoryFileMeta> {
    let mut rels = Vec::new();
    pipi_tools::memory::collect_markdown(dir, 0, &mut rels);
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

        // harness/AGENTS.md 可读写
        write_agent_file("editor", "harness/AGENTS.md", "# 新指令\n").unwrap();
        assert_eq!(
            read_agent_file("editor", "harness/AGENTS.md").unwrap(),
            "# 新指令\n"
        );

        // memory 新文件可创建并读写
        write_agent_file("editor", "harness/memory/user-prefs.md", "偏好： concise\n").unwrap();
        assert!(
            read_agent_file("editor", "harness/memory/user-prefs.md")
                .unwrap()
                .contains("偏好")
        );

        // 拒绝：绝对路径、逃逸、agent.json、其他扩展名、过深路径
        assert!(read_agent_file("editor", "/etc/passwd").is_err());
        assert!(read_agent_file("editor", "../escape.md").is_err());
        assert!(write_agent_file("editor", "agent.json", "{}").is_err());
        assert!(write_agent_file("editor", "harness/memory/notes.txt", "x").is_err());
        assert!(write_agent_file("editor", "harness/skills/tool/SKILL.md", "x").is_err());
        assert!(write_agent_file("editor", "harness/memory/a/b/c/d.md", "x").is_err());
        assert!(read_agent_file("editor", "harness/memory/missing.md").is_err());
        // 顶层 legacy 路径已收口：不再是可编辑位置
        assert!(write_agent_file("editor", "AGENTS.md", "x").is_err());
        assert!(write_agent_file("editor", "memory/user-prefs.md", "x").is_err());
        // AGENTS.md 不允许通过 write 新建（已被 create_agent 管理）
        std::fs::remove_file(home.join(".pipi/agents/editor/harness/AGENTS.md")).unwrap();
        assert!(write_agent_file("editor", "harness/AGENTS.md", "x").is_err());

        pipi_test_support::set_env_var("HOME", prev);
        std::fs::remove_dir_all(&home).unwrap();
    }
}
