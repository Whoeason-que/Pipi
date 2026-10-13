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

use crate::av::schema::AgentToml;

pub const AGENT_TOML_FILENAME: &str = "agent.toml";
pub const AGENT_LOCAL_TOML_FILENAME: &str = "agent.local.toml";

/// 契约层的种类：决定哪些段允许出现。
///
/// 项目层（含 `agent.local.toml`）是**别人的仓库**带来的文件：它不能声明
/// Agent 的记忆目录（`[resources].memory`），否则克隆来的仓库就能把 Agent
/// 的写入位置指到仓库里 —— 与「项目层不得声明身份与权限段」同一取向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    /// Agent 定义目录的 `agent.toml`（`~/.pipi/agents/<name>/`）。
    Agent,
    /// 项目根的 `agent.toml`（通常随仓库提交）。
    Project,
    /// 同目录的 `agent.local.toml`（本地覆盖层）。
    ProjectLocal,
}

/// 一个契约层：来源文件 + 解析结果。
#[derive(Debug, Clone)]
pub struct Layer {
    /// 来源种类（决定层专属规则）。
    pub kind: LayerKind,
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

/// 向上定位项目根：`.git` 标记。与 `crate::prompt::find_project_root` 同一语义；
/// av 模块保持自包含（可独立提取），故此处独立实现、不反向依赖 prompt 层。
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
    let mut layers = vec![load_layer(
        &main_path,
        AGENT_TOML_FILENAME,
        LayerKind::Project,
    )?];
    let local_path = dir.join(AGENT_LOCAL_TOML_FILENAME);
    if local_path.is_file() {
        layers.push(load_layer(
            &local_path,
            AGENT_LOCAL_TOML_FILENAME,
            LayerKind::ProjectLocal,
        )?);
    }
    let root = std::fs::canonicalize(&dir)
        .map_err(|e| format!("无法规范化契约目录 {}: {e}", dir.display()))?;
    Ok(Discovered { root, layers })
}

/// 显式加载单个契约文件（解析 + schema 校验；不含 symlink 包含检查）。
///
/// 供宿主直接读取已知路径的契约 —— 例如 Agent 定义目录里的 `agent.toml`
/// （该目录不属于 cwd 发现范围）。层专属规则（如项目层不得声明 memory）
/// 在合并时按 [`LayerKind`] 判定，[`merge_layers`](crate::av::merge_layers) 一次覆盖全部调用点。
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

fn load_layer(path: &Path, label: &str, kind: LayerKind) -> Result<Layer, String> {
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
        kind,
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
mod tests {
    use super::*;

    fn write_contract(dir: &Path, text: &str) {
        std::fs::write(dir.join(AGENT_TOML_FILENAME), text).unwrap();
    }

    #[test]
    fn no_contract_anywhere_yields_empty_layers() {
        let dir = pipi_test_support::temp_dir("av-discovery");
        let discovered = discover(&dir).unwrap();
        assert!(discovered.layers.is_empty());
        // 无项目根时 root 回退为 cwd
        let cwd = std::fs::canonicalize(&dir).unwrap();
        assert_eq!(discovered.root, cwd);
    }

    #[test]
    fn nearest_wins_from_cwd() {
        let root = pipi_test_support::temp_dir("av-discovery");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let deep = root.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        write_contract(&root, "schema = 1\n[env]\nset = { FROM = \"root\" }");
        write_contract(&deep, "schema = 1\n[env]\nset = { FROM = \"deep\" }");

        let discovered = discover(&deep).unwrap();
        assert_eq!(discovered.layers.len(), 1);
        assert_eq!(
            discovered.layers[0]
                .config
                .env
                .as_ref()
                .unwrap()
                .set
                .as_ref()
                .unwrap()["FROM"],
            "deep"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stops_search_at_project_root() {
        // 外层 repo 有 agent.toml，内层 repo（.git）没有：不得越界继承外层契约
        let outer = pipi_test_support::temp_dir("av-discovery");
        std::fs::create_dir_all(outer.join(".git")).unwrap();
        write_contract(&outer, "schema = 1\n[env]\nset = { FROM = \"outer\" }");
        let inner = outer.join("sub");
        std::fs::create_dir_all(inner.join(".git")).unwrap();

        let discovered = discover(&inner).unwrap();
        assert!(discovered.layers.is_empty());
        std::fs::remove_dir_all(outer).unwrap();
    }

    #[test]
    fn local_layer_loaded_with_higher_priority() {
        let dir = pipi_test_support::temp_dir("av-discovery");
        write_contract(&dir, "schema = 1\n[env]\nset = { K = \"main\" }");
        std::fs::write(
            dir.join(AGENT_LOCAL_TOML_FILENAME),
            "schema = 1\n[env]\nset = { K = \"local\" }",
        )
        .unwrap();

        let discovered = discover(&dir).unwrap();
        assert_eq!(discovered.layers.len(), 2);
        assert_eq!(discovered.layers[0].label, "agent.toml");
        assert_eq!(discovered.layers[1].label, "agent.local.toml");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_contract_is_fail_closed() {
        let dir = pipi_test_support::temp_dir("av-discovery");
        write_contract(&dir, "schema = 9");
        assert!(discover(&dir).is_err());

        let dir = pipi_test_support::temp_dir("av-discovery");
        write_contract(&dir, "schema = = 1");
        assert!(discover(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn symlinked_contract_outside_dir_rejected() {
        let dir = pipi_test_support::temp_dir("av-discovery");
        let external = pipi_test_support::temp_dir("av-discovery");
        write_contract(&external, "schema = 1");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            external.join(AGENT_TOML_FILENAME),
            dir.join(AGENT_TOML_FILENAME),
        )
        .unwrap();
        #[cfg(windows)]
        std::fs::copy(
            external.join(AGENT_TOML_FILENAME),
            dir.join(AGENT_TOML_FILENAME),
        )
        .unwrap();

        #[cfg(unix)]
        assert!(discover(&dir).is_err());
        #[cfg(windows)]
        assert!(discover(&dir).is_ok());

        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(external).unwrap();
    }

    #[test]
    fn explicit_contract_file_load() {
        let dir = pipi_test_support::temp_dir("av-discovery");
        let path = dir.join("agent.toml");
        std::fs::write(&path, "schema = 1\n[env]\nset = { K = \"v\" }").unwrap();

        let config = load_contract_file(&path).unwrap();
        assert!(config.env.is_some());
        // 未知段 fail-closed
        std::fs::write(&path, "schema = 1\n[unknown]\nx = 1").unwrap();
        assert!(load_contract_file(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tilde_and_relative_resolution() {
        let _guard = pipi_test_support::HOME_LOCK.lock().unwrap();
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/x"), home.join("x"));
        assert_eq!(expand_tilde("/abs/x"), PathBuf::from("/abs/x"));
        assert_eq!(expand_tilde("~user/x"), PathBuf::from("~user/x"));
        assert_eq!(
            resolve_path(Path::new("/base"), "rel"),
            PathBuf::from("/base/rel")
        );
        assert_eq!(resolve_path(Path::new("/base"), "~/x"), home.join("x"));
    }
}
