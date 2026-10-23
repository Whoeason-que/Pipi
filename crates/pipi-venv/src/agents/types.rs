use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use pipi_protocol::Model;
use pipi_protocol::permissions::{KNOWN_TOOLS, PermissionsConfig};

use super::paths::{expand_tilde, validate_agent_name};

/// Agent 清单，对应 `agent.json`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefinition {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 展示用的模型标签；真实模型配置在 `provider`。
    #[serde(default)]
    pub model: String,
    /// 模型与 API 配置（OpenAI / Anthropic 兼容协议）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Model>,
    /// Agent 可操作的目录；None 表示使用 Agent 目录内的 workspace/。
    #[serde(default)]
    pub workspace: Option<String>,
    /// 工具开关与命令权限。
    #[serde(default)]
    pub permissions: PermissionsConfig,
    /// Stdio MCP 服务器（M3 接入，这里先占位）。
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// 是否归入由 Agent 组合工具创建的 Subagent 分组。
    #[serde(default)]
    pub subagent: bool,
    /// 自动压缩阈值：上下文占用达到模型窗口的这个百分比时压缩（默认 75）。
    /// 越界值由 [`AgentDefinition::compact_threshold_percent`] 兜底成默认值。
    #[serde(default = "default_compact_threshold_percent")]
    pub compact_threshold_percent: u8,
    /// 可选的压缩后上下文目标比例；缺失时继续保留最多 20K 近期原文。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_target_percent: Option<u8>,
    /// 系统提示词的取材范围（默认两者都要）。
    #[serde(default)]
    pub system_prompt: PromptScope,
}

/// 系统提示词的取材策略：决定 Agent 侧（`harness/`）与工作区侧
/// （项目 AGENTS.md 链、`.pi/skills`、项目契约资源）各自是否参与组装。
///
/// 只影响提示词 —— env、权限、沙箱与受信任读取根一律与策略无关。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptScope {
    /// Agent 侧 + 工作区侧（新建 Agent 的默认）。
    #[default]
    #[serde(rename = "both")]
    Both,
    /// 只要 Agent 侧的 `harness/`（指令、记忆索引、技能索引）。
    #[serde(rename = "agent")]
    Agent,
    /// 只要工作区侧的项目上下文。
    #[serde(rename = "workspace")]
    Workspace,
    /// 完全不发 system 消息（对话历史与工具 schema 照发）。
    #[serde(rename = "none")]
    None,
}

impl PromptScope {
    /// 是否读取 Agent 侧的 `harness/`（含指令、记忆索引与技能索引）。
    pub fn uses_agent_side(self) -> bool {
        matches!(self, Self::Both | Self::Agent)
    }

    /// 是否读取工作区侧的项目上下文。
    pub fn uses_workspace_side(self) -> bool {
        matches!(self, Self::Both | Self::Workspace)
    }
}

pub(super) fn default_compact_threshold_percent() -> u8 {
    pipi_protocol::compaction::DEFAULT_THRESHOLD_PERCENT
}

impl AgentDefinition {
    /// 自动压缩阈值百分比：越界（0 / >100）视为「没设过」，用默认值 ——
    /// 手改过的 agent.json 不该让压缩失效。
    pub fn compact_threshold_percent(&self) -> u8 {
        pipi_protocol::compaction::normalize_threshold_percent(self.compact_threshold_percent)
    }

    /// 无效的手写比例退回未配置行为，避免把历史压到不可预期的目标。
    pub fn compact_target_percent(&self) -> Option<u8> {
        self.compact_target_percent
            .filter(|value| (1..=99).contains(value))
    }
}

/// Stdio MCP 服务器声明。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// 默认 Agent 的名字。首次启动（或该 Agent 缺失）时由核心播种，保证开箱即可用。
/// 它与用户自建 Agent 完全同构：就是 `~/.pipi/agents/<name>/` 下的一组普通文件。
pub const DEFAULT_AGENT_NAME: &str = "Pipi";

/// 播种默认 Agent 时写入的描述（同时作为它 AGENTS.md 的首段）。
pub(super) const DEFAULT_AGENT_DESCRIPTION: &str = "Pipi 自带的默认 Agent：开箱可用；工作目录、技能与记忆都在 ~/.pipi/agents/Pipi/ 下（提示词素材在 harness/），改文件即生效";

/// 校验 Agent 定义的合法性（create 与 save 共用；UI 全字段编辑也走 save）。
///
/// - 名称合法（字母数字 + `-` `_`）
/// - 工具名单：至少启用一个，且全部是已知内置工具
/// - 工作目录：显式指定时必须展开后为绝对路径（不自动创建 —— 创建目录
///   是 create_agent 的行为，save 保持只写 agent.json）
/// - MCP 服务器：名称非空且不重复（M3 占位，先挡住明显错误）
pub fn validate_definition(def: &AgentDefinition) -> Result<(), String> {
    validate_agent_name(&def.name)?;
    if def.permissions.tools.is_empty() {
        return Err("Enable at least one tool".into());
    }
    for tool in &def.permissions.tools {
        if !KNOWN_TOOLS.contains(&tool.as_str()) {
            return Err(format!(
                "Unknown tool: {tool} (available: {})",
                KNOWN_TOOLS.join(", ")
            ));
        }
    }
    if let Some(workspace) = def
        .workspace
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
    {
        let expanded = expand_tilde(workspace);
        if !PathBuf::from(&expanded).is_absolute() {
            return Err(format!("Workspace must be an absolute path: {workspace}"));
        }
    }
    let mut seen_servers = std::collections::HashSet::new();
    for server in &def.mcp_servers {
        if server.name.trim().is_empty() {
            return Err("MCP server name cannot be empty".into());
        }
        if !seen_servers.insert(server.name.trim().to_string()) {
            return Err(format!("Duplicate MCP server name: {}", server.name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_definition_rejects_bad_tools_workspace_and_mcp() {
        let base = || AgentDefinition {
            name: "ok-name".into(),
            description: String::new(),
            model: String::new(),
            provider: None,
            workspace: None,
            permissions: PermissionsConfig::default(),
            mcp_servers: Vec::new(),
            subagent: false,
            compact_threshold_percent: 75,
            compact_target_percent: None,
            system_prompt: Default::default(),
        };

        // 未知工具 / 空工具名单
        let mut def = base();
        def.permissions.tools = vec!["nuclear".into()];
        assert!(validate_definition(&def).is_err());
        def.permissions.tools = vec![];
        assert!(validate_definition(&def).is_err());
        // 相对工作目录拒绝，绝对路径通过
        def.permissions.tools = vec!["read".into()];
        def.workspace = Some("relative/path".into());
        assert!(validate_definition(&def).is_err());
        def.workspace = Some("/absolute/path".into());
        assert!(validate_definition(&def).is_ok());
        // 名称非法
        def.workspace = None;
        def.name = "bad name".into();
        assert!(validate_definition(&def).is_err());
        def.name = "ok-name".into();
        assert!(validate_definition(&def).is_ok());
        // MCP 名称重复拒绝
        let server = |name: &str| McpServerConfig {
            name: name.into(),
            command: "cmd".into(),
            args: Vec::new(),
            env: Default::default(),
            enabled: true,
        };
        def.mcp_servers = vec![server("a"), server("a")];
        assert!(validate_definition(&def).is_err());
        def.mcp_servers = vec![server("a"), server("b")];
        assert!(validate_definition(&def).is_ok());
    }

    /// systemPrompt 策略：缺字段 = both（旧 agent.json 直接可用），
    /// 显式值按名解析，非法值拒绝（宁可拒绝不可放行）。
    #[test]
    fn system_prompt_scope_defaults_to_both_and_rejects_unknown_values() {
        let legacy: AgentDefinition = serde_json::from_str(r#"{"name":"legacy"}"#).unwrap();
        assert_eq!(legacy.system_prompt, PromptScope::Both);
        assert_eq!(
            serde_json::to_value(&legacy).unwrap()["systemPrompt"],
            serde_json::json!("both")
        );

        for (raw, expected) in [
            ("both", PromptScope::Both),
            ("agent", PromptScope::Agent),
            ("workspace", PromptScope::Workspace),
            ("none", PromptScope::None),
        ] {
            let def: AgentDefinition =
                serde_json::from_str(&format!(r#"{{"name":"a","systemPrompt":"{raw}"}}"#)).unwrap();
            assert_eq!(def.system_prompt, expected, "{raw}");
        }

        assert!(
            serde_json::from_str::<AgentDefinition>(r#"{"name":"a","systemPrompt":"everything"}"#)
                .is_err()
        );
    }

    #[test]
    fn compact_threshold_defaults_and_bounds() {
        let minimal = r#"{"name":"t","permissions":{"tools":["read"]}}"#;
        let def: AgentDefinition = serde_json::from_str(minimal).unwrap();
        assert_eq!(def.compact_threshold_percent, 75);
        assert_eq!(def.compact_threshold_percent(), 75);

        // 显式值生效（含边界）
        let custom =
            r#"{"name":"t","permissions":{"tools":["read"]},"compactThresholdPercent":40}"#;
        let def: AgentDefinition = serde_json::from_str(custom).unwrap();
        assert_eq!(def.compact_threshold_percent(), 40);

        // 越界 → 退回默认（手改过的文件不该让压缩失效或不可预期）
        for value in [0u8, 101, 200] {
            let text = format!(
                r#"{{"name":"t","permissions":{{"tools":["read"]}},"compactThresholdPercent":{value}}}"#
            );
            let def: AgentDefinition = serde_json::from_str(&text).unwrap();
            assert_eq!(
                def.compact_threshold_percent(),
                75,
                "值 {value} 应兜底成默认"
            );
        }

        // 序列化键名与前端类型对齐（此刻 def 是上面那个 40 的）
        let json = serde_json::to_string(&def).unwrap();
        assert!(json.contains("\"compactThresholdPercent\":40"), "{json}");
    }

    #[test]
    fn compact_target_is_optional_and_uses_camel_case() {
        let minimal = r#"{"name":"t","permissions":{"tools":["read"]}}"#;
        let def: AgentDefinition = serde_json::from_str(minimal).unwrap();
        assert_eq!(def.compact_target_percent(), None);
        assert!(
            !serde_json::to_string(&def)
                .unwrap()
                .contains("compactTargetPercent")
        );

        let custom = r#"{"name":"t","permissions":{"tools":["read"]},"compactTargetPercent":20}"#;
        let def: AgentDefinition = serde_json::from_str(custom).unwrap();
        assert_eq!(def.compact_target_percent(), Some(20));
        assert!(
            serde_json::to_string(&def)
                .unwrap()
                .contains("\"compactTargetPercent\":20")
        );

        for value in [0, 100, 200] {
            let text = format!(r#"{{"name":"t","compactTargetPercent":{value}}}"#);
            let def: AgentDefinition = serde_json::from_str(&text).unwrap();
            assert_eq!(def.compact_target_percent(), None);
        }
    }
}
