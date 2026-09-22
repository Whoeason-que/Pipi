//! Agent 工具协议、权限策略与内置工具集。移植自 `packages/agent/src/harness/tools/`，外加 Pipi 新增的
//! memory 工具。工具按 `permissions.tools` 注册；`ToolContext` 携带工作目录、
//! 权限与中止信号。

pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod memory;
pub mod permissions;
pub mod read;
pub mod truncate;
pub mod write;

/// 旧工具实现的协议类型路径。保留这个小模块能让工具实现与 core 解耦，同时
/// 不把 Pipi 的持久化 session 模块重新引入工具 crate。
pub mod types {
    pub use pipi_protocol::{AbortSignal, Tool, ToolResultContent};
}

/// 工具自身需要的临时唯一 ID（bash spill 与测试 fixture）。它不是 Agent
/// session 的 JSONL 身份，不能依赖 `pipi-core::session`。
pub mod session {
    pub fn new_id() -> String {
        uuid::Uuid::new_v4().simple().to_string()
    }
}

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::permissions::{PermissionsConfig, SandboxMode};
use pipi_protocol::{AbortSignal, Tool, ToolResultContent};

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
            SandboxMode::ReadOnly => Err("沙箱策略为 read-only：禁止写入文件".into()),
            SandboxMode::WorkspaceWrite => {
                crate::permissions::resolve_write_target(&self.workspace, path)
                    .map(|_| ())
                    .map_err(|_| {
                        format!(
                            "沙箱策略为 workspace-write：{} 在工作目录 {} 之外",
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

/// 解析 workspace-write 的目标，委托给权限模块的 canonical resolver。
/// 保留该入口兼容已有 tools 调用方。
pub fn resolve_write_path(workspace: &Path, path: &str) -> Result<PathBuf, String> {
    crate::permissions::resolve_write_path(workspace, path)
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
        .map_err(|e| format!("无法读取文件 {path}：无法解析路径（文件不存在或链接无效）：{e}"))?;
    std::fs::metadata(&target)
        .map_err(|e| format!("无法读取文件 {path}：无法确认目标文件状态：{e}"))?;

    let mut roots = Vec::with_capacity(read_roots.len() + 1);
    roots.push(workspace.to_path_buf());
    roots.extend(read_roots.iter().cloned());
    for root in roots {
        let canonical_root = std::fs::canonicalize(&root).map_err(|e| {
            format!(
                "无法读取文件 {path}：无法验证受信任读取目录 {}：{e}",
                root.display()
            )
        })?;
        let metadata = std::fs::metadata(&canonical_root).map_err(|e| {
            format!(
                "无法读取文件 {path}：无法确认受信任读取目录 {}：{e}",
                canonical_root.display()
            )
        })?;
        if !metadata.is_dir() {
            return Err(format!(
                "无法读取文件 {path}：读取根 {} 不是目录",
                canonical_root.display()
            ));
        }
        if target == canonical_root || target.starts_with(&canonical_root) {
            return Ok(target);
        }
    }

    Err(format!(
        "拒绝读取 {path}：路径不在工作目录或受信任 skill 目录内"
    ))
}

/// 按工具名查 schema 里的 required 字段做轻量校验（完整 JSON Schema 校验
/// 是后续项，见 lib.rs「尚未移植」）。
pub fn validate_args(parameters: &Value, args: &Value) -> Result<(), String> {
    if !args.is_object() {
        return Err("工具参数必须是 JSON 对象".into());
    }
    if let Some(required) = parameters["required"].as_array() {
        for name in required {
            if let Some(name) = name.as_str() {
                if args.get(name).is_none() {
                    return Err(format!("缺少必需参数: {name}"));
                }
            }
        }
    }
    Ok(())
}

pub struct ToolRegistry {
    tools: Vec<Arc<dyn AgentTool>>,
}

impl ToolRegistry {
    pub fn new(tools: Vec<Arc<dyn AgentTool>>) -> ToolRegistry {
        ToolRegistry { tools }
    }

    /// 由运行时注入需要额外执行能力的工具（例如 Agent 组合工具）。基础文件
    /// 工具仍由 [`ToolRegistry::for_context`] 按权限自动构造。
    pub fn push(&mut self, tool: Arc<dyn AgentTool>) {
        self.tools.push(tool);
    }

    /// 按 Agent 的权限配置注册内置工具。
    pub fn for_context(ctx: &ToolContext) -> ToolRegistry {
        let mut tools: Vec<Arc<dyn AgentTool>> = Vec::new();
        if ctx.permissions.tool_enabled("read") {
            tools.push(Arc::new(read::ReadTool));
        }
        if ctx.permissions.tool_enabled("glob") {
            tools.push(Arc::new(glob::GlobTool));
        }
        if ctx.permissions.tool_enabled("grep") {
            tools.push(Arc::new(grep::GrepTool));
        }
        if ctx.permissions.tool_enabled("write") {
            tools.push(Arc::new(write::WriteTool));
        }
        if ctx.permissions.tool_enabled("edit") {
            tools.push(Arc::new(edit::EditTool));
        }
        if ctx.permissions.tool_enabled("bash") {
            tools.push(Arc::new(bash::BashTool));
        }
        if ctx.permissions.tool_enabled("memory") {
            if let Some(dir) = &ctx.memory_dir {
                tools.push(Arc::new(memory::MemoryTool {
                    memory_dir: dir.clone(),
                }));
            }
        }
        ToolRegistry { tools }
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

    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|t| t.name()).collect()
    }
}
