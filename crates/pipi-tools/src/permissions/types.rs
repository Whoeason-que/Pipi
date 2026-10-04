//! 权限与沙箱的公共词汇类型：沙箱模式、bash 权限配置、命令段、评估结论、
//! 审批通道与内置工具名清单。

use serde::{Deserialize, Serialize};

/// 沙箱策略。移植自 codex 的 `SandboxMode`。
///
/// 反序列化缺省值为 `DangerFullAccess`（兼容旧 agent.json 的行为）；
/// 新建 Agent 的 UI 默认选 `WorkspaceWrite`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxMode {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "workspace-write")]
    WorkspaceWrite,
    #[serde(rename = "danger-full-access")]
    #[default]
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

    pub fn label(&self) -> &'static str {
        match self {
            SandboxMode::ReadOnly => "只读",
            SandboxMode::WorkspaceWrite => "工作目录内可写",
            SandboxMode::DangerFullAccess => "完全访问",
        }
    }
}

/// 一个已切分的命令段：argv（供危险检测）与重组文本（供白名单匹配）。
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub argv: Vec<String>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BashMode {
    #[default]
    AllowAll,
    Allowlist,
    Denylist,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashPermissions {
    #[serde(default)]
    pub mode: BashMode,
    /// 白名单 / 黑名单条目。单词条目（如 "git"）匹配以该词开头的命令段；
    /// 带空格条目（如 "npm run"）按前缀匹配。
    #[serde(default)]
    pub commands: Vec<String>,
}

/// 普通 Agent 默认启用的基础工具。Agent 组合工具不在默认集合中，避免旧的
/// `agent.json`（缺少 `permissions.tools`）在升级后静默获得创建、运行或读取
/// 其他 Agent 的能力。
pub const DEFAULT_TOOLS: [&str; 7] = ["read", "write", "edit", "bash", "memory", "glob", "grep"];

/// Agent 组合工具：必须在 `permissions.tools` 中显式启用。
pub const AGENT_TOOLS: [&str; 3] = ["create_agent", "run_agent", "read_agent"];

/// 后台任务托管工具：必须显式启用。它们允许进程跨越当前模型轮次继续运行，
/// 因此不随普通 bash 权限默认开放。
pub const BACKGROUND_TASK_TOOLS: [&str; 3] = [
    "submit_background_task",
    "query_background_tasks",
    "manage_background_task",
];

/// Pipi 已知内置工具名（基础工具 + 显式启用的 Agent 组合 / 后台任务工具）。
pub const KNOWN_TOOLS: [&str; 13] = [
    "read",
    "write",
    "edit",
    "bash",
    "memory",
    "glob",
    "grep",
    "create_agent",
    "run_agent",
    "read_agent",
    "submit_background_task",
    "query_background_tasks",
    "manage_background_task",
];

/// bash 命令评估结论。
#[derive(Debug, Clone, PartialEq)]
pub enum BashAssessment {
    Allowed,
    /// 黑名单命中 / 危险命令 / 沙箱约束 / 解析失败 —— 不可审批。
    HardDenied(String),
    /// Allowlist 模式下白名单未命中（且非危险）。`missing` 是未命中的段落文本，
    /// 「总是允许」时按段写入白名单。
    NeedsApproval {
        missing: Vec<String>,
    },
}

/// 交互审批通道。宿主（桌面壳 / Web 服务）实现传输：把请求发给用户界面，
/// 等待用户决定。任何实现失败都必须落到 Err（fail-closed）。
#[async_trait::async_trait]
pub trait CommandApprover: Send + Sync {
    /// 请求批准执行整条命令。Ok(()) = 批准；Err = 拒绝 / 超时 / 已中止 / 通道失败。
    async fn approve(&self, command: &str) -> Result<(), String>;
}
