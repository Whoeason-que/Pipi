//! 工具端口：执行结果、执行上下文、工具 trait 与注册表容器。
//!
//! 内置工具的**实现**在 `pipi-tools`（bash / read / write / edit / glob /
//! grep / memory），组合工具在宿主（`pipi-core::tools`）—— 两侧都实现这里的
//! [`AgentTool`]，引擎只认这个 trait 与 [`ToolRegistry`] 容器。
//! 「按权限装配哪些内置工具」是实现的策略，见 `pipi_tools::builtin_registry`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::permissions::{PermissionsConfig, SandboxMode};
use crate::{AbortSignal, Tool, ToolResultContent};

/// 工具执行结果。对应 pi 的 `AgentToolResult`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutput {
    pub content: Vec<ToolResultContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub terminate: bool,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> ToolOutput {
        ToolOutput {
            content: vec![ToolResultContent::Text { text: text.into() }],
            details: None,
            terminate: false,
        }
    }
}

/// 工具执行上下文：工作目录、memory 目录、受信任读取根、权限、沙箱、中止信号。
///
/// `resolved_env` 是 av 契约解析出的**完整**子进程环境（会话启动时解析一次），
/// bash 等 spawn 点以它整体重建环境变量。`approver` 是交互审批通道（bash 白名单
/// 未命中时救回用）；无宿主交互能力的运行（如子 Agent）必须为 None —— 缺席即
/// fail-closed 拒绝。
#[derive(Clone)]
pub struct ToolContext {
    pub workspace: PathBuf,
    pub memory_dir: Option<PathBuf>,
    /// 除工作目录外允许 `read` 访问的受信任目录（例如 Agent 的 skills/）。
    pub read_roots: Vec<PathBuf>,
    pub permissions: Arc<PermissionsConfig>,
    pub sandbox: SandboxMode,
    pub resolved_env: Arc<BTreeMap<String, String>>,
    pub abort: AbortSignal,
    pub approver: Option<Arc<dyn crate::permissions::CommandApprover>>,
}

impl ToolContext {
    /// 沙箱下的写入约束（移植自 codex 的 SandboxMode 语义）：
    /// - `danger-full-access`：不限制
    /// - `workspace-write`：只能写工作目录内
    /// - `read-only`：禁止写文件（memory 除外 —— 那是 Agent 自己的脑子）
    pub fn ensure_writable(&self, path: &Path) -> Result<(), String> {
        match self.sandbox {
            SandboxMode::DangerFullAccess => Ok(()),
            SandboxMode::ReadOnly => {
                Err("Sandbox policy is read-only: file writing is prohibited".into())
            }
            SandboxMode::WorkspaceWrite => {
                crate::permissions::resolve_write_target(&self.workspace, path)
                    .map(|_| ())
                    .map_err(|_| {
                        format!(
                            "Sandbox policy is workspace-write: {} is outside workspace {}",
                            path.display(),
                            self.workspace.display()
                        )
                    })
            }
        }
    }
}

/// 工具 trait。对应 pi 的 `AgentTool`（typebox schema 换成 JSON Schema）。
#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> String;
    fn parameters(&self) -> Value;
    /// 有副作用、不适合与其他调用并发的工具（如 bash）返回 true。
    fn requires_sequential(&self) -> bool {
        false
    }
    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &Value,
        on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String>;
}

/// 把相对路径解析到工作目录下（绝对路径原样通过）。同 pi 的 path-utils。
pub fn resolve_path(workspace: &Path, path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    if p.is_absolute() {
        Ok(p.to_path_buf())
    } else {
        Ok(workspace.join(p))
    }
}

/// 解析并校验 `read` 的目标，只允许工作目录或受信任读取根内的现有文件。
///
/// 目标和每个根都会先 canonicalize；因此 `..` 与指向外部的符号链接都按
/// 真实路径判断。返回 canonical 目标，供调用方直接读取，避免再次跟随原始链接。
pub fn resolve_read_path(
    workspace: &Path,
    read_roots: &[PathBuf],
    path: &str,
) -> Result<PathBuf, String> {
    let requested = resolve_path(workspace, path)?;
    let target = std::fs::canonicalize(&requested)
        .map_err(|e| format!("Cannot read file {path}: failed to resolve path (file does not exist or link is invalid): {e}"))?;
    std::fs::metadata(&target).map_err(|e| {
        format!("Cannot read file {path}: failed to verify target file status: {e}")
    })?;

    let mut roots = Vec::with_capacity(read_roots.len() + 1);
    roots.push(workspace.to_path_buf());
    roots.extend(read_roots.iter().cloned());
    for root in roots {
        let canonical_root = std::fs::canonicalize(&root).map_err(|e| {
            format!(
                "Cannot read file {path}: failed to verify trusted read directory {}: {e}",
                root.display()
            )
        })?;
        let metadata = std::fs::metadata(&canonical_root).map_err(|e| {
            format!(
                "Cannot read file {path}: failed to verify trusted read directory {}: {e}",
                canonical_root.display()
            )
        })?;
        if !metadata.is_dir() {
            return Err(format!(
                "Cannot read file {path}: read root {} is not a directory",
                canonical_root.display()
            ));
        }
        if target == canonical_root || target.starts_with(&canonical_root) {
            return Ok(target);
        }
    }

    Err(format!(
        "Reading {path} is rejected: path is not within workspace or trusted skill directories"
    ))
}

/// 按工具名查 schema 里的 required 字段做轻量校验（完整 JSON Schema 校验
/// 是后续项，见 lib.rs「尚未移植」）。
pub fn validate_args(parameters: &Value, args: &Value) -> Result<(), String> {
    if !args.is_object() {
        return Err("Tool arguments must be a JSON object".into());
    }
    if let Some(required) = parameters["required"].as_array() {
        for name in required {
            if let Some(name) = name.as_str()
                && args.get(name).is_none()
            {
                return Err(format!("Missing required parameter: {name}"));
            }
        }
    }
    Ok(())
}

/// 一次运行可用的工具集合。宿主往里注入自己的工具（Agent 组合、后台任务），
/// 内置工具由 `pipi_tools::builtin_registry` 按权限装配。
pub struct ToolRegistry {
    tools: Vec<Arc<dyn AgentTool>>,
}

impl ToolRegistry {
    pub fn new(tools: Vec<Arc<dyn AgentTool>>) -> ToolRegistry {
        ToolRegistry { tools }
    }

    /// 由运行时注入需要额外执行能力的工具（例如 Agent 组合工具）。基础文件
    /// 工具仍由 `pipi_tools::builtin_registry` 按权限自动构造。
    pub fn push(&mut self, tool: Arc<dyn AgentTool>) {
        self.tools.push(tool);
    }

    pub fn wire_tools(&self) -> Vec<Tool> {
        self.tools
            .iter()
            .map(|t| Tool {
                name: t.name().to_string(),
                description: t.description(),
                parameters: t.parameters(),
            })
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }
}
