//! agent.toml 的发现与加载。
//!
//! 发现规则（uv/pyproject 的 nearest-wins 心智）：
//! - 有项目根（向上找 `.git` 定位）时，从 cwd 向上逐目录找最近的
//!   `agent.toml`，检查范围到项目根为止（含根）；无项目根时只看 cwd。
//! - 命中目录里的 `agent.local.toml`（本地覆盖层，通常不入库）作为
//!   更高优先级层加载。
//! - 相对路径以契约文件所在目录为基准；`~` 展开。
//! - 加载与 schema 校验 fail-closed：任何错误直接报错，不静默降级。

use std::path::{Path, PathBuf};

use crate::schema::AgentToml;

pub const AGENT_TOML_FILENAME: &str = "agent.toml";
pub const AGENT_LOCAL_TOML_FILENAME: &str = "agent.local.toml";

/// 一个契约层：来源文件 + 解析结果。
#[derive(Debug, Clone)]
pub struct Layer {
    /// 来源记账标签（"agent.toml" / "agent.local.toml"）。
    pub label: String,
    pub path: PathBuf,
    pub config: AgentToml,
}

/// 发现结果：`layers` 自低到高优先级排列。
#[derive(Debug, Clone)]
pub struct Discovered {
    /// 契约文件所在目录（canonicalize 后），相对路径的解析基准。
    pub root: PathBuf,
    pub layers: Vec<Layer>,
}

/// 向上定位项目根：`.git` 标记。与 `pipi-harness` 的 `find_project_root`
/// 同一语义（av 不反向依赖宿主 crate，故此处独立实现）。
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cursor = start.to_path_buf();
    loop {
        if cursor.join(".git").exists() {
            return Some(cursor);
        }
        cursor = cursor.parent()?.to_path_buf();
    }
}

/// 从 `cwd` 向上发现契约文件。未找到时返回空 `layers`（环境 = 进程环境 + 运行时注入）。
pub fn discover(cwd: &Path) -> Result<Discovered, String> {
    let project_root = find_project_root(cwd);
    let layer_dir = match &project_root {
        Some(root) => {
            let mut cursor = cwd.to_path_buf();
            loop {
                if cursor.join(AGENT_TOML_FILENAME).is_file() {
                    break Some(cursor);
                }
                if cursor == *root {
                    break None;
                }
                match cursor.parent() {
                    Some(parent) => cursor = parent.to_path_buf(),
                    None => break None,
                }
            }
        }
        None => cwd
            .join(AGENT_TOML_FILENAME)
            .is_file()
            .then(|| cwd.to_path_buf()),
    };

    let Some(dir) = layer_dir else {
        return Ok(Discovered {
            root: cwd.to_path_buf(),
            layers: Vec::new(),
        });
    };

    let main_path = dir.join(AGENT_TOML_FILENAME);
    let mut layers = vec![load_layer(&main_path, AGENT_TOML_FILENAME)?];
    let local_path = dir.join(AGENT_LOCAL_TOML_FILENAME);
    if local_path.is_file() {
        layers.push(load_layer(&local_path, AGENT_LOCAL_TOML_FILENAME)?);
    }
    let root = std::fs::canonicalize(&dir)
        .map_err(|e| format!("无法规范化契约目录 {}: {e}", dir.display()))?;
    Ok(Discovered { root, layers })
}

/// 显式加载单个契约文件（解析 + schema 校验；不含 symlink 包含检查）。
///
/// 供宿主直接读取已知路径的契约 —— 例如 Agent 定义目录里的 `agent.toml`
/// （该目录不属于 cwd 发现范围）。
pub fn load_contract_file(path: &Path) -> Result<AgentToml, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("无法读取 {}: {e}", path.display()))?;
    let config: AgentToml =
        toml::from_str(&text).map_err(|e| format!("解析 {} 失败：{e}", path.display()))?;
    config
        .validate()
        .map_err(|e| format!("{} 校验失败：{e}", path.display()))?;
    Ok(config)
}

fn load_layer(path: &Path, label: &str) -> Result<Layer, String> {
    // symlink 逃逸 fail-closed：契约文件必须真实存在于所在目录内
    let canonical =
        std::fs::canonicalize(path).map_err(|e| format!("无法读取 {}: {e}", path.display()))?;
    if let Some(parent) = path.parent() {
        let parent = std::fs::canonicalize(parent)
            .map_err(|e| format!("无法规范化 {}: {e}", parent.display()))?;
        if !canonical.starts_with(&parent) {
            return Err(format!(
                "{label} 是指向目录外的符号链接，已拒绝（fail-closed）"
            ));
        }
    }
    let config = load_contract_file(&canonical)?;
    Ok(Layer {
        label: label.to_string(),
        path: canonical,
        config,
    })
}

/// 展开 `~`（及其后跟 `/` 的形式）；`~user` 不展开，按字面处理。
pub fn expand_tilde(raw: &str) -> PathBuf {
    if raw == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(raw)
}

/// 相对路径以基准目录解析；绝对路径原样。
pub fn resolve_path(base: &Path, raw: &str) -> PathBuf {
    let expanded = expand_tilde(raw);
    if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    }
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
