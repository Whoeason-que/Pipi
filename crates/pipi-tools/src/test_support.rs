//! 单元测试共享 fixture。
//!
//! 只放跨多个工具测试模块重复出现的构造器；单文件专用的助手留在各自测试
//! 模块里。需要非默认字段时用 `with_*` 小 builder，不要再拼全量字面量。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use pipi_protocol::AbortSignal;
use pipi_protocol::permissions::{PermissionsConfig, SandboxMode};
use pipi_protocol::tools::ToolContext;

/// 最小可用 `ToolContext`：`danger-full-access`、无记忆 / 笔记本 / 受信任根、
/// 默认权限、空环境、新中止信号。
pub(crate) fn test_context(workspace: impl Into<PathBuf>) -> ToolContext {
    ToolContext {
        workspace: workspace.into(),
        memory_dir: None,
        notebook_path: None,
        read_roots: Vec::new(),
        permissions: Arc::new(PermissionsConfig::default()),
        sandbox: SandboxMode::DangerFullAccess,
        sandbox_policy_override: None,
        resolved_env: Arc::new(BTreeMap::new()),
        abort: AbortSignal::new(),
    }
}

pub(crate) fn with_sandbox(mut ctx: ToolContext, sandbox: SandboxMode) -> ToolContext {
    ctx.sandbox = sandbox;
    ctx
}

pub(crate) fn with_memory_dir(mut ctx: ToolContext, dir: impl Into<PathBuf>) -> ToolContext {
    ctx.memory_dir = Some(dir.into());
    ctx
}

pub(crate) fn with_read_roots(mut ctx: ToolContext, roots: Vec<PathBuf>) -> ToolContext {
    ctx.read_roots = roots;
    ctx
}

pub(crate) fn with_resolved_env(
    mut ctx: ToolContext,
    env: BTreeMap<String, String>,
) -> ToolContext {
    ctx.resolved_env = Arc::new(env);
    ctx
}

pub(crate) fn with_abort(mut ctx: ToolContext, abort: AbortSignal) -> ToolContext {
    ctx.abort = abort;
    ctx
}
