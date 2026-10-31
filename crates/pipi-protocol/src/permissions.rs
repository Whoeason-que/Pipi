//! 权限与沙箱的公共词汇：沙箱档位 / 能力清单、工具名清单，以及写入路径的
//! canonical containment 检查。
//!
//! 安全边界是 `pipi-sandbox` 的 OS 沙箱；这里不再有命令文本策略（白/黑名单、
//! 危险命令、审批）。`permissions.bash` 作为旧字段被忽略，保留读取兼容。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 沙箱策略。移植自 codex 的 `SandboxMode`。
///
/// 缺省值为 `WorkspaceWrite`：旧 agent.json 缺少 `sandbox` 时也落到受限档位，
/// 不再因为字段缺失而静默获得完全访问。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxMode {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "workspace-write")]
    #[default]
    WorkspaceWrite,
    #[serde(rename = "danger-full-access")]
    DangerFullAccess,
}

impl SandboxMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SandboxMode::ReadOnly => "read-only",
            SandboxMode::WorkspaceWrite => "workspace-write",
            SandboxMode::DangerFullAccess => "danger-full-access",
        }
    }

    /// 这一档是否需要真正的内核边界。`danger-full-access` 是显式的逃生门，
    /// 语义就是「不挂沙箱」。
    pub fn is_enforced(&self) -> bool {
        *self != SandboxMode::DangerFullAccess
    }
}

/// 传给 OS 沙箱 helper 的能力清单（唯一的安全边界）。
///
/// 这是 `SandboxMode` 的可执行展开：模式决定默认集合，`writable_roots` 与
/// `network_access` 是正交轴。helper 把它编译成 Landlock/Seatbelt 规则；
/// 用户态不再对命令文本做安全判定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxPolicy {
    pub mode: SandboxMode,
    /// 默认关闭。开启后 helper 不安装网络过滤器（或允许 Seatbelt 网络）。
    #[serde(default)]
    pub network_access: bool,
    /// `workspace-write` 下可写的目录（workspace 与临时目录由
    /// [`SandboxPolicy::for_workspace`] 填入）。`read-only` 下为空。
    #[serde(default)]
    pub writable_roots: Vec<PathBuf>,
}

impl SandboxPolicy {
    /// 由 Agent 的沙箱档位派生默认能力集合。
    ///
    /// `workspace-write` 同时放行进程的临时目录：大量工具链（编译器、包管理器、
    /// 测试框架）离开 TMPDIR 无法工作；这是可用性取舍，不是工作区外写入的默许。
    /// 需要更严格时在调用方把 `writable_roots` 收窄。
    pub fn for_workspace(mode: SandboxMode, workspace: &Path) -> Self {
        let mut writable_roots: Vec<PathBuf> = Vec::new();
        if mode == SandboxMode::WorkspaceWrite {
            writable_roots.push(workspace.to_path_buf());
            let temp = std::env::temp_dir();
            if temp != workspace {
                writable_roots.push(temp);
            }
            writable_roots.dedup();
        }
        SandboxPolicy {
            mode,
            network_access: false,
            writable_roots,
        }
    }

    pub fn is_enforced(&self) -> bool {
        self.mode.is_enforced()
    }
}

/// 普通 Agent 默认启用的基础工具。Agent 组合工具不在默认集合中，避免旧的
/// `agent.json`（缺少 `permissions.tools`）在升级后静默获得创建、运行或读取
/// 其他 Agent 的能力。
///
/// `glob` / `grep` 已退役（2026-10-31）：历史会话里合计只占 0.2% 的调用，
/// 而内置实现不读 `.gitignore`、跳过点路径、`include` 只匹配 basename，
/// 给出的「无匹配」经常是错的。搜索统一走 bash（`rg` 优先，`grep -rn` 兜底）。
pub const DEFAULT_TOOLS: [&str; 5] = ["read", "write", "edit", "bash", "memory"];

/// Agent 组合工具：必须在 `permissions.tools` 中显式启用。
pub const AGENT_TOOLS: [&str; 3] = ["create_agent", "run_agent", "read_agent"];

/// 后台任务托管工具：必须显式启用。它们允许进程跨越当前模型轮次继续运行，
/// 因此不随普通 bash 权限默认开放。
pub const BACKGROUND_TASK_TOOLS: [&str; 3] = [
    "submit_background_task",
    "query_background_tasks",
    "manage_background_task",
];

/// 论坛工具：必须显式启用。它们读写一个跨 Agent 的共享版面（默认
/// `~/.pipi/forum`，工作区里带 `.agents/forum` 则用本地的），属于组合能力，
/// 不随基础工具默认开放。
pub const FORUM_TOOLS: [&str; 4] = ["forum_list", "forum_read", "forum_post", "forum_reply"];

/// Pipi 已知内置工具名（基础工具 + 显式启用的 Agent 组合 / 后台任务 / 论坛工具）。
pub const KNOWN_TOOLS: [&str; 16] = [
    "read",
    "write",
    "edit",
    "bash",
    "memory",
    "create_agent",
    "run_agent",
    "read_agent",
    "submit_background_task",
    "query_background_tasks",
    "manage_background_task",
    "session_search",
    "forum_list",
    "forum_read",
    "forum_post",
    "forum_reply",
];

/// Agent 的权限配置（`agent.json` 的 `permissions` 段）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsConfig {
    /// 启用的工具名列表（`KNOWN_TOOLS` 的子集）。
    #[serde(default = "default_tools")]
    pub tools: Vec<String>,
    /// 沙箱策略（codex 的 SandboxMode）。缺省 WorkspaceWrite（fail-safe）。
    #[serde(default)]
    pub sandbox: SandboxMode,
}

fn default_tools() -> Vec<String> {
    DEFAULT_TOOLS.iter().map(|s| s.to_string()).collect()
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        PermissionsConfig {
            tools: default_tools(),
            sandbox: SandboxMode::default(),
        }
    }
}

impl PermissionsConfig {
    pub fn tool_enabled(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t == name)
    }
}

/// 解析 workspace-write 的目标，先 canonicalize 工作目录与最近的现有祖先，
/// 再拼回不存在的尾部；悬空 symlink 和指向外部的 symlink 都 fail-closed。
pub fn resolve_write_path(workspace: &Path, path: &str) -> Result<PathBuf, String> {
    let requested_path = Path::new(path);
    let requested = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        workspace.join(requested_path)
    };
    resolve_write_target(workspace, &requested)
}

/// `resolve_write_path` 的 Path 版本，供工具在已解析路径上复用。
pub(crate) fn resolve_write_target(workspace: &Path, requested: &Path) -> Result<PathBuf, String> {
    let canonical_workspace = fs::canonicalize(workspace)
        .map_err(|e| format!("无法验证工作目录 {}：{e}", workspace.display()))?;
    let canonical_target = canonicalize_existing_ancestor(requested)?;
    if canonical_target == canonical_workspace || canonical_target.starts_with(&canonical_workspace)
    {
        Ok(canonical_target)
    } else {
        Err(format!(
            "目标 {} 不在工作目录 {} 内",
            canonical_target.display(),
            canonical_workspace.display()
        ))
    }
}

fn canonicalize_existing_ancestor(path: &Path) -> Result<PathBuf, String> {
    let mut missing = Vec::new();
    let mut existing = path;
    loop {
        match fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = existing
                    .file_name()
                    .ok_or_else(|| format!("无法解析写入路径 {}", path.display()))?;
                missing.push(name.to_os_string());
                existing = existing
                    .parent()
                    .ok_or_else(|| format!("无法解析写入路径 {}", path.display()))?;
            }
            Err(error) => {
                return Err(format!("无法解析写入路径 {}：{error}", path.display()));
            }
        }
    }

    let mut canonical = fs::canonicalize(existing)
        .map_err(|e| format!("无法解析写入路径 {}：{e}", path.display()))?;
    for name in missing.iter().rev() {
        canonical.push(name);
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_permissions_enable_the_baseline_tools() {
        let config = PermissionsConfig::default();
        assert!(config.tool_enabled("read"));
        assert!(config.tool_enabled("bash"));
        // 组合工具必须显式启用
        assert!(!config.tool_enabled("create_agent"));
        assert_eq!(config.sandbox, SandboxMode::WorkspaceWrite);
    }

    #[test]
    fn legacy_bash_policy_fields_are_ignored() {
        // 旧 agent.json 里的 permissions.bash 已废弃；反序列化必须继续成功，
        // 但不参与任何行为。
        let raw = r#"{"tools":["read"],"bash":{"mode":"allowlist","commands":["git"]},"sandbox":"workspace-write"}"#;
        let config: PermissionsConfig = serde_json::from_str(raw).unwrap();
        assert!(config.tool_enabled("read"));
        assert!(!config.tool_enabled("bash"));
        assert_eq!(config.sandbox, SandboxMode::WorkspaceWrite);
        // 缺省段仍可反序列化（兼容旧 agent.json）
        let sparse: PermissionsConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(sparse.tools.len(), DEFAULT_TOOLS.len());
    }

    #[test]
    fn sandbox_policy_derives_writable_roots_from_mode() {
        let workspace = Path::new("/tmp/pipi-policy-workspace");
        let read_only = SandboxPolicy::for_workspace(SandboxMode::ReadOnly, workspace);
        assert!(read_only.writable_roots.is_empty());
        assert!(read_only.is_enforced());

        let workspace_write = SandboxPolicy::for_workspace(SandboxMode::WorkspaceWrite, workspace);
        assert!(
            workspace_write
                .writable_roots
                .iter()
                .any(|root| root == workspace)
        );
        assert_eq!(workspace_write.writable_roots.len(), 2);
        assert!(workspace_write.is_enforced());

        let full = SandboxPolicy::for_workspace(SandboxMode::DangerFullAccess, workspace);
        assert!(full.writable_roots.is_empty());
        assert!(!full.is_enforced());

        let json = serde_json::to_string(&workspace_write).unwrap();
        assert!(json.contains("writableRoots"));
        let back: SandboxPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, workspace_write);
    }
}
