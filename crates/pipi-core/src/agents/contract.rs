use std::fs;
use std::path::{Path, PathBuf};

use super::types::AgentDefinition;

/// Agent 的契约层集合（av `agent.toml`）：Agent 定义目录的 agent.toml
/// （agent 层，可能缺席）+ 项目层发现（agent.toml → agent.local.toml）。
/// env 合并、`[resources]` 三态共用同一份层集合（技能中心也用它取 `use` 声明）。
pub fn collect_contract_layers(
    def: &AgentDefinition,
    workspace: Option<&Path>,
) -> Result<(Option<av::Layer>, Vec<av::Layer>), String> {
    let mut agent_layer = None;
    if let Some(dir) = def.dir() {
        let path = dir.join("agent.toml");
        if path.is_file() {
            let canonical =
                fs::canonicalize(&path).map_err(|e| format!("无法读取 agent.toml：{e}"))?;
            let config = av::load_contract_file(&canonical)?;
            agent_layer = Some(av::Layer {
                label: "agent/agent.toml".into(),
                path: canonical,
                config,
            });
        }
    }
    let project_layers = match workspace {
        Some(workspace) => av::discover(workspace)?.layers,
        None => Vec::new(),
    };
    Ok((agent_layer, project_layers))
}

pub(super) fn layer_path_dir(layer: &av::Layer) -> PathBuf {
    layer
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// 声明式技能源路径解析（相对基准 = 声明它的契约文件所在目录）。
pub(super) fn resolve_declared_sources(raw: &[String], base_dir: &Path) -> Vec<PathBuf> {
    raw.iter()
        .map(|raw| av::resolve_path(base_dir, raw))
        .collect()
}

/// 加载声明式上下文文件（`[resources].instructions` 的 Some(list) 分支）。
///
/// fail-closed：缺失、非常规文件、逃逸包含根、内容为空均报错；项目层预算
/// 沿用发现路径的同一语义（截断 + 预算耗尽即止）。
pub(super) fn load_declared_context_files(
    raw_paths: &[String],
    base_dir: &Path,
    containment_root: &Path,
    project_budget: Option<usize>,
) -> Result<Vec<pipi_harness::ContextFile>, String> {
    let mut files = Vec::new();
    let mut remaining = project_budget.unwrap_or(usize::MAX);
    for raw in raw_paths {
        if remaining == 0 {
            break;
        }
        let resolved = av::resolve_path(base_dir, raw);
        let canonical = fs::canonicalize(&resolved)
            .map_err(|e| format!("上下文文件不可用 {}: {e}", resolved.display()))?;
        if !canonical.is_file() {
            return Err(format!("上下文文件不是常规文件：{}", canonical.display()));
        }
        if !canonical.starts_with(containment_root) {
            return Err(format!(
                "上下文文件 {} 逃逸包含根 {}（fail-closed）",
                canonical.display(),
                containment_root.display()
            ));
        }
        let content = fs::read_to_string(&canonical)
            .map_err(|e| format!("无法读取 {}: {e}", canonical.display()))?;
        let content = content
            .strip_prefix('\u{feff}')
            .unwrap_or(&content)
            .to_string();
        if content.trim().is_empty() {
            return Err(format!("上下文文件为空：{}", canonical.display()));
        }
        let content = if project_budget.is_some() {
            let truncated = pipi_harness::resources::truncate_to_char_boundary(&content, remaining);
            remaining = remaining.saturating_sub(truncated.len());
            truncated
        } else {
            content
        };
        files.push(pipi_harness::ContextFile {
            path: resolved.display().to_string(),
            content,
        });
    }
    Ok(files)
}
