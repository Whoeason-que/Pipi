//! Agent 定义与注册表（Pipi 核心，pi 无对应物 —— pi 以 CLI 参数 + 全局
//! 配置承载这些信息，Pipi 把它升格为一等公民）。
//!
//! 一切皆文件：一个 Agent 就是 `~/.pipi/agents/<name>/` 下的一组文件。
//! 本模块只做两件事：从文件读取 Agent、把创建请求写成文件。

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::permissions::KNOWN_TOOLS;
use crate::types::Model;

// 供 Tauri 命令层与上层应用统一从 agents 引用
pub use crate::permissions::PermissionsConfig;

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

/// 所有 Agent 数据的根：`~/.pipi/agents`
pub fn agents_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".pipi").join("agents"))
}

pub fn agent_dir(name: &str) -> Option<PathBuf> {
    agents_dir().map(|dir| dir.join(name))
}

/// 展开 `~` 前缀（仅支持 `~` 与 `~/...`）。
pub fn expand_tilde(path: &str) -> String {
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().into_owned();
        }
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

impl AgentDefinition {
    /// Agent 目录（列表扫描时无法得知 name → 以 def.name 为准）。
    pub fn dir(&self) -> Option<PathBuf> {
        agent_dir(&self.name)
    }

    /// 解析工作目录：显式配置 > Agent 目录内的 workspace/。
    pub fn resolve_workspace(&self) -> Option<PathBuf> {
        match &self.workspace {
            Some(w) => {
                let expanded = expand_tilde(w);
                Some(PathBuf::from(expanded))
            }
            None => self.dir().map(|dir| dir.join("workspace")),
        }
    }

    /// memory 目录（Agent 目录内，固定）。
    pub fn memory_dir(&self) -> Option<PathBuf> {
        self.dir().map(|dir| dir.join("memory"))
    }

    pub fn sessions_dir(&self) -> Option<PathBuf> {
        self.dir().map(|dir| dir.join("sessions"))
    }
}

/// 扫描 ~/.pipi/agents，返回所有合法 Agent。
/// 缺 agent.json 或解析失败的目录直接跳过 —— 文件即真相，坏文件不拖垮列表。
pub fn list_agents() -> Result<Vec<AgentDefinition>, String> {
    let dir = agents_dir().ok_or_else(|| "无法定位用户主目录".to_string())?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut agents = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let manifest = entry.path().join("agent.json");
        if !manifest.is_file() {
            continue;
        }
        let text = match fs::read_to_string(&manifest) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("pipi: 跳过无法读取的 {}: {e}", manifest.display());
                continue;
            }
        };
        match serde_json::from_str::<AgentDefinition>(&text) {
            Ok(def) => agents.push(def),
            Err(e) => eprintln!("pipi: 跳过非法的 {}: {e}", manifest.display()),
        }
    }
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(agents)
}

/// 创建 Agent：生成目录骨架并写入 agent.json / AGENTS.md。
/// 不做任何额外存储 —— Agent 从诞生起就是一组普通文件。
pub fn create_agent(
    name: &str,
    description: &str,
    workspace: Option<&str>,
    permissions: Option<PermissionsConfig>,
) -> Result<AgentDefinition, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Agent 名称不能为空".into());
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err("名称只能包含字母、数字、- 和 _".into());
    }

    // 工具名单校验：必须是已知工具的子集且至少启用一个
    let permissions = permissions.unwrap_or_default();
    if permissions.tools.is_empty() {
        return Err("至少启用一个工具".into());
    }
    for tool in &permissions.tools {
        if !KNOWN_TOOLS.contains(&tool.as_str()) {
            return Err(format!(
                "未知工具: {tool}（可用：{}）",
                KNOWN_TOOLS.join(", ")
            ));
        }
    }

    // 工作目录校验：显式指定时展开 ~ 并确保是绝对路径，不存在则创建
    let workspace = match workspace.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => {
            let expanded = expand_tilde(w);
            let path = PathBuf::from(&expanded);
            if !path.is_absolute() {
                return Err(format!("工作目录必须是绝对路径: {w}"));
            }
            fs::create_dir_all(&path).map_err(|e| format!("无法创建工作目录 {expanded}: {e}"))?;
            Some(expanded)
        }
        None => None,
    };

    let dir = agent_dir(&name).ok_or_else(|| "无法定位用户主目录".to_string())?;
    if dir.exists() {
        return Err(format!("Agent「{name}」已存在"));
    }

    // 目录骨架（与 README「Agent 的组成」一一对应）
    for sub in ["skills", "memory", "sessions"] {
        fs::create_dir_all(dir.join(sub)).map_err(|e| e.to_string())?;
    }
    if workspace.is_none() {
        fs::create_dir_all(dir.join("workspace")).map_err(|e| e.to_string())?;
    }

    let def = AgentDefinition {
        name: name.clone(),
        description: description.trim().to_string(),
        model: String::new(),
        provider: None,
        workspace,
        permissions,
        mcp_servers: Vec::new(),
    };
    let manifest = serde_json::to_string_pretty(&def).map_err(|e| e.to_string())?;
    fs::write(dir.join("agent.json"), manifest + "\n").map_err(|e| e.to_string())?;
    let intro = if def.description.is_empty() {
        "（一句话介绍这个 Agent）"
    } else {
        &def.description
    };
    fs::write(
        dir.join("AGENTS.md"),
        format!("# {name}\n\n{intro}\n\n## 指令\n\n- \n"),
    )
    .map_err(|e| e.to_string())?;

    Ok(def)
}

/// 读取单个 agent.json（编辑器改完文件后 UI 刷新用）。
pub fn load_agent(name: &str) -> Result<AgentDefinition, String> {
    let dir = agent_dir(name).ok_or_else(|| "无法定位用户主目录".to_string())?;
    let path = dir.join("agent.json");
    let text = fs::read_to_string(&path).map_err(|e| {
        format!(
            "无法读取 {}: {e}（agent 目录就是数据库，改文件即生效）",
            path.display()
        )
    })?;
    serde_json::from_str(&text).map_err(|e| format!("agent.json 格式错误: {e}"))
}

/// 保存 agent.json（UI 编辑入口；文件仍是唯一真相源）。
pub fn save_agent(def: &AgentDefinition) -> Result<(), String> {
    let dir = agent_dir(&def.name).ok_or_else(|| "无法定位用户主目录".to_string())?;
    if !dir.is_dir() {
        return Err(format!("Agent「{}」不存在", def.name));
    }
    let manifest = serde_json::to_string_pretty(def).map_err(|e| e.to_string())?;
    fs::write(dir.join("agent.json"), manifest + "\n").map_err(|e| e.to_string())
}

/// 组装工具执行上下文（loop 与工具共用）。
pub fn build_tool_context(
    def: &AgentDefinition,
    abort: crate::types::AbortSignal,
) -> Result<crate::tools::ToolContext, String> {
    let workspace = def.resolve_workspace().ok_or("无法解析工作目录")?;
    fs::create_dir_all(&workspace).map_err(|e| format!("工作目录不可用: {e}"))?;
    // 规范化，供沙箱路径约束做前缀比较
    let workspace = fs::canonicalize(&workspace).unwrap_or(workspace);
    Ok(crate::tools::ToolContext {
        workspace,
        memory_dir: def.memory_dir(),
        permissions: std::sync::Arc::new(def.permissions.clone()),
        sandbox: def.permissions.sandbox,
        abort,
    })
}

/// 组装系统提示（M1 上下文四件套，来源均为 pi / codex 的实践）：
/// 1. Agent 自己的 AGENTS.md（pi：系统级指令）
/// 2. 工作目录的项目文档 AGENTS.md，根→近（codex：project_doc）
/// 3. 技能索引：名称 + 描述 + 路径，全文由模型按需 read（pi：渐进式披露）
/// 4. 环境上下文：工作目录、沙箱、平台、日期（codex：environment_context）
pub fn build_system_prompt(def: &AgentDefinition) -> String {
    let mut parts: Vec<String> = Vec::new();
    let dir = def.dir();

    if let Some(dir) = &dir {
        if let Ok(text) = fs::read_to_string(dir.join("AGENTS.md")) {
            if !text.trim().is_empty() {
                parts.push(text.trim().to_string());
            }
        }
    }

    if let Some(workspace) = def.resolve_workspace() {
        let docs = crate::project_doc::collect_project_docs(
            &workspace,
            crate::project_doc::DEFAULT_PROJECT_DOC_MAX_BYTES,
        );
        for (path, text) in docs {
            parts.push(format!(
                "# 项目文档（{}）\n\n{}",
                path.display(),
                text.trim()
            ));
        }

        let skills = dir
            .as_ref()
            .map(|d| crate::skills::scan_skills(&d.join("skills")))
            .unwrap_or_default();
        let index = crate::skills::render_skill_index(&skills);
        if !index.is_empty() {
            parts.push(index.trim_start().to_string());
        }

        parts.push(crate::context::environment_context(
            &workspace,
            &def.permissions.sandbox,
        ));
    }

    parts.join("\n\n")
}

/// 确保路径里的 agent 目录存在（从文件系统恢复定义时用）。
pub fn ensure_agent_dir(name: &str) -> Result<PathBuf, String> {
    let dir = agent_dir(name).ok_or_else(|| "无法定位用户主目录".to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{BashMode, BashPermissions};
    use std::sync::Mutex;

    #[test]
    fn tilde_expansion() {
        // 读取 HOME 前先拿锁，避免与其他测试的 set_var 竞争
        let _guard = HOME_LOCK.lock().unwrap();
        let home = dirs::home_dir().expect("home dir");
        assert_eq!(expand_tilde("~/foo"), home.join("foo").to_string_lossy());
        assert_eq!(expand_tilde("~"), home.to_string_lossy());
        assert_eq!(expand_tilde("/abs/path"), "/abs/path");
        assert_eq!(expand_tilde("rel/path"), "rel/path");
    }

    #[test]
    fn create_rejects_bad_names_and_tools() {
        assert!(create_agent("", "d", None, None).is_err());
        assert!(create_agent("bad name", "d", None, None).is_err());
        assert!(create_agent("../escape", "d", None, None).is_err());
        assert!(create_agent(
            "x",
            "d",
            None,
            Some(PermissionsConfig {
                tools: vec!["nuclear".into()],
                bash: Default::default(),
                sandbox: Default::default(),
            })
        )
        .is_err());
        assert!(create_agent(
            "x",
            "d",
            None,
            Some(PermissionsConfig {
                tools: vec![],
                bash: Default::default(),
                sandbox: Default::default(),
            })
        )
        .is_err());
    }

    /// HOME 环境变量是进程级的：涉及 ~/.pipi 的测试互斥串行。
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn create_writes_file_skeleton() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("pipi-home-{}", crate::session::new_id()));
        std::fs::create_dir_all(&home).unwrap();
        let prev = std::env::var("HOME").unwrap();
        std::env::set_var("HOME", &home);
        let result = create_agent(
            "tester",
            "测试用",
            None,
            Some(PermissionsConfig {
                tools: vec!["read".into(), "bash".into()],
                bash: crate::permissions::BashPermissions {
                    mode: crate::permissions::BashMode::Allowlist,
                    commands: vec!["git".into()],
                },
                sandbox: Default::default(),
            }),
        );
        // HOME 尚未恢复：此时解析 workspace 才指向临时 HOME
        let def = result.unwrap();
        let resolved_workspace = def.resolve_workspace().unwrap();
        std::env::set_var("HOME", &prev);

        let dir = home.join(".pipi/agents/tester");
        assert!(dir.join("agent.json").is_file());
        assert!(dir.join("AGENTS.md").is_file());
        assert!(dir.join("skills").is_dir());
        assert!(dir.join("memory").is_dir());
        assert!(dir.join("sessions").is_dir());
        assert!(dir.join("workspace").is_dir()); // 未指定外部目录 → 创建默认

        let text = std::fs::read_to_string(dir.join("agent.json")).unwrap();
        assert!(text.contains("\"allowlist\""));
        assert!(text.contains("\"tools\": ["));
        assert_eq!(resolved_workspace, dir.join("workspace"));

        // 重复创建被拒
        std::env::set_var("HOME", &home);
        assert!(create_agent("tester", "", None, None).is_err());
        std::env::set_var("HOME", &prev);
    }

    #[test]
    fn create_with_explicit_workspace() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("pipi-home-{}", crate::session::new_id()));
        std::fs::create_dir_all(&home).unwrap();
        let ws = std::env::temp_dir().join(format!("pipi-ws-{}", crate::session::new_id()));
        let prev = std::env::var("HOME").unwrap();
        std::env::set_var("HOME", &home);
        let result = create_agent("ws-agent", "", Some(ws.to_str().unwrap()), None);
        std::env::set_var("HOME", &prev);
        let def = result.unwrap();
        assert_eq!(def.workspace.as_deref(), Some(ws.to_str().unwrap()));
        assert!(!home.join(".pipi/agents/ws-agent/workspace").exists());
        assert_eq!(def.resolve_workspace().unwrap(), ws);
    }
}
