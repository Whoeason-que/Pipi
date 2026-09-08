//! Agent 定义与注册表（Pipi 核心，pi 无对应物 —— pi 以 CLI 参数 + 全局
//! 配置承载这些信息，Pipi 把它升格为一等公民）。
//!
//! 一切皆文件：一个 Agent 就是 `~/.pipi/agents/<name>/` 下的一组文件。
//! 本模块只做两件事：从文件读取 Agent、把创建请求写成文件。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::permissions::KNOWN_TOOLS;
use crate::types::{Model, Tool};

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

/// 校验 Agent 名称必须是单一、稳定的目录名，禁止路径分隔符与 `..` 穿越。
pub fn validate_agent_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Agent 名称不能为空".into());
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err("名称只能包含字母、数字、- 和 _".into());
    }
    Ok(())
}

fn manifest_name_matches_directory(directory_name: &str, manifest_name: &str) -> bool {
    directory_name == manifest_name && validate_agent_name(manifest_name).is_ok()
}

fn ensure_real_directory(path: &Path, label: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("{label} 不能是符号链接: {}", path.display()))
        }
        Ok(metadata) if !metadata.is_dir() => Err(format!("{label} 不是目录: {}", path.display())),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("无法检查 {label} {}: {error}", path.display())),
    }
}

fn ensure_real_file(path: &Path, label: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("{label} 不能是符号链接: {}", path.display()))
        }
        Ok(metadata) if !metadata.is_file() => {
            Err(format!("{label} 不是普通文件: {}", path.display()))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("无法检查 {label} {}: {error}", path.display())),
    }
}

fn checked_agents_root() -> Result<PathBuf, String> {
    let agents = agents_dir().ok_or_else(|| "无法定位用户主目录".to_string())?;
    let pipi = agents
        .parent()
        .ok_or_else(|| "无法定位 .pipi 目录".to_string())?;
    ensure_real_directory(pipi, ".pipi")?;
    ensure_real_directory(&agents, "agents")?;
    Ok(agents)
}

fn checked_agent_dir(name: &str) -> Result<PathBuf, String> {
    validate_agent_name(name)?;
    let dir = checked_agents_root()?.join(name);
    ensure_real_directory(&dir, "Agent 目录")?;
    Ok(dir)
}

pub fn agent_dir(name: &str) -> Option<PathBuf> {
    checked_agent_dir(name).ok()
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
    let dir = checked_agents_root()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut agents = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                eprintln!("pipi: 跳过无法检查的 {}: {error}", entry.path().display());
                continue;
            }
        };
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let directory_name = entry.file_name().to_string_lossy().into_owned();
        let manifest = entry.path().join("agent.json");
        match ensure_real_file(&manifest, "agent.json") {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                eprintln!("pipi: 跳过不安全的 {}: {error}", manifest.display());
                continue;
            }
        }
        let text = match fs::read_to_string(&manifest) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("pipi: 跳过无法读取的 {}: {e}", manifest.display());
                continue;
            }
        };
        match serde_json::from_str::<AgentDefinition>(&text) {
            Ok(def) if manifest_name_matches_directory(&directory_name, &def.name) => {
                agents.push(def)
            }
            Ok(def) => eprintln!(
                "pipi: 跳过 Agent 名称/目录不匹配的 {}（{}）",
                def.name,
                manifest.display()
            ),
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
    validate_agent_name(&name)?;

    // 新建 Agent 默认使用受限沙箱；旧 agent.json 的 serde 默认仍保持兼容。
    let permissions = permissions.unwrap_or_else(|| PermissionsConfig {
        sandbox: crate::permissions::SandboxMode::WorkspaceWrite,
        ..Default::default()
    });
    // 工具名单校验：必须是已知工具的子集且至少启用一个
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
    let dir = checked_agent_dir(name)?;
    let path = dir.join("agent.json");
    if !ensure_real_file(&path, "agent.json")? {
        return Err(format!("Agent「{name}」的 agent.json 不存在"));
    }
    let text = fs::read_to_string(&path).map_err(|e| {
        format!(
            "无法读取 {}: {e}（agent 目录就是数据库，改文件即生效）",
            path.display()
        )
    })?;
    let def: AgentDefinition =
        serde_json::from_str(&text).map_err(|e| format!("agent.json 格式错误: {e}"))?;
    if !manifest_name_matches_directory(name, &def.name) {
        return Err(format!(
            "agent.json 名称 {} 与请求的 Agent 目录 {} 不匹配",
            def.name, name
        ));
    }
    Ok(def)
}

/// 保存 agent.json（UI 编辑入口；文件仍是唯一真相源）。
pub fn save_agent(def: &AgentDefinition) -> Result<(), String> {
    let dir = checked_agent_dir(&def.name)?;
    if !dir.is_dir() {
        return Err(format!("Agent「{}」不存在", def.name));
    }
    let manifest = serde_json::to_string_pretty(def).map_err(|e| e.to_string())?;
    let manifest_path = dir.join("agent.json");
    ensure_real_file(&manifest_path, "agent.json")?;
    fs::write(manifest_path, manifest + "\n").map_err(|e| e.to_string())
}

/// 组装工具执行上下文（loop 与工具共用）。
pub fn build_tool_context(
    def: &AgentDefinition,
    abort: crate::types::AbortSignal,
) -> Result<crate::tools::ToolContext, String> {
    let workspace = def.resolve_workspace().ok_or("无法解析工作目录")?;
    fs::create_dir_all(&workspace).map_err(|e| format!("工作目录不可用: {e}"))?;
    let workspace =
        fs::canonicalize(&workspace).map_err(|e| format!("工作目录不可用：无法规范化路径: {e}"))?;
    let read_roots = match def.dir().map(|dir| dir.join("skills")) {
        Some(skills) if skills.is_dir() => {
            let agent_dir = skills
                .parent()
                .ok_or("Agent 目录不可用：无法确定 skills 所属目录")?;
            let canonical_agent_dir = fs::canonicalize(agent_dir)
                .map_err(|e| format!("Agent 目录不可用：无法规范化路径: {e}"))?;
            let canonical_skills = fs::canonicalize(&skills)
                .map_err(|e| format!("skills 目录不可用：无法规范化路径: {e}"))?;
            if !canonical_skills.starts_with(&canonical_agent_dir) {
                return Err(format!(
                    "skills 目录不可用：{} 不在 Agent 目录 {} 内",
                    canonical_skills.display(),
                    canonical_agent_dir.display()
                ));
            }
            vec![canonical_skills]
        }
        _ => Vec::new(),
    };
    Ok(crate::tools::ToolContext {
        workspace,
        memory_dir: def.memory_dir(),
        read_roots,
        permissions: std::sync::Arc::new(def.permissions.clone()),
        sandbox: def.permissions.sandbox,
        abort,
    })
}

/// 组装系统提示：收集宿主资源，再交给 Pi-compatible harness 渲染。
pub fn build_system_prompt(def: &AgentDefinition) -> String {
    build_system_prompt_with_tools(def, &[])
}

/// 组装带 wire tool 描述的系统提示。
pub fn build_system_prompt_with_tools(def: &AgentDefinition, tools: &[Tool]) -> String {
    let dir = def.dir();
    let workspace = def.resolve_workspace();
    let mut context_files = Vec::new();
    let mut skills = Vec::new();

    let (cwd, append_system_prompt) = if let Some(workspace) = &workspace {
        context_files.extend(crate::harness::resources::load_project_context_files(
            dir.as_deref(),
            workspace,
        ));

        if let Some(dir) = &dir {
            skills =
                crate::skills::load_skill_metadata(Some(dir.as_path()), Some(workspace.as_path()))
                    .into_iter()
                    .map(|skill| crate::harness::SkillMetadata {
                        name: skill.name,
                        description: skill.description,
                        disable_model_invocation: skill.disable_model_invocation,
                        path: skill.path.display().to_string(),
                    })
                    .collect();
        }

        (
            workspace.display().to_string(),
            Some(crate::context::environment_context(
                workspace,
                &def.permissions.sandbox,
            )),
        )
    } else {
        context_files.extend(crate::harness::resources::load_agent_context_files(
            dir.as_deref(),
        ));
        (String::new(), None)
    };

    crate::harness::build_system_prompt(crate::harness::BuildSystemPromptOptions {
        selected_tools: Some(def.permissions.tools.clone()),
        tool_snippets: tools
            .iter()
            .map(|tool| (tool.name.clone(), tool.description.clone()))
            .collect(),
        cwd,
        context_files,
        skills,
        append_system_prompt,
        ..Default::default()
    })
}

/// 确保路径里的 agent 目录存在（从文件系统恢复定义时用）。
pub fn ensure_agent_dir(name: &str) -> Result<PathBuf, String> {
    let dir = checked_agent_dir(name)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{BashMode, BashPermissions};
    use std::sync::Mutex;

    #[test]
    fn agent_directory_rejects_path_traversal_and_separators() {
        assert!(agent_dir("safe-agent_1").is_some());
        for invalid in ["", ".", "..", "../escape", "nested/name", "nested\\name"] {
            assert!(
                agent_dir(invalid).is_none(),
                "accepted invalid agent name: {invalid}"
            );
        }
    }

    #[test]
    fn manifest_name_must_match_containing_agent_directory() {
        assert!(manifest_name_matches_directory("safe-agent", "safe-agent"));
        assert!(!manifest_name_matches_directory(
            "safe-agent",
            "other-agent"
        ));
        assert!(!manifest_name_matches_directory("../escape", "../escape"));
    }

    #[cfg(unix)]
    #[test]
    fn agent_directory_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;

        let _guard = HOME_LOCK.lock().unwrap();
        let home =
            std::env::temp_dir().join(format!("pipi-agent-link-home-{}", crate::session::new_id()));
        let external = std::env::temp_dir().join(format!(
            "pipi-agent-link-external-{}",
            crate::session::new_id()
        ));
        let linked = home.join(".pipi/agents/linked-agent");
        std::fs::create_dir_all(home.join(".pipi/agents")).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        symlink(&external, &linked).unwrap();

        let previous_home = std::env::var("HOME").unwrap();
        std::env::set_var("HOME", &home);
        let def = AgentDefinition {
            name: "linked-agent".into(),
            description: String::new(),
            model: String::new(),
            provider: None,
            workspace: None,
            permissions: PermissionsConfig::default(),
            mcp_servers: Vec::new(),
        };

        assert!(agent_dir("linked-agent").is_none());
        assert!(load_agent("linked-agent").is_err());
        assert!(save_agent(&def).is_err());
        assert!(ensure_agent_dir("linked-agent").is_err());
        assert!(list_agents().unwrap().is_empty());

        std::env::set_var("HOME", previous_home);
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&external).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn agent_root_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!(
            "pipi-agent-root-link-home-{}",
            crate::session::new_id()
        ));
        let external = std::env::temp_dir().join(format!(
            "pipi-agent-root-link-external-{}",
            crate::session::new_id()
        ));
        std::fs::create_dir_all(home.join(".pipi")).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        symlink(&external, home.join(".pipi/agents")).unwrap();

        let previous_home = std::env::var("HOME").unwrap();
        std::env::set_var("HOME", &home);
        assert!(list_agents().is_err());
        assert!(ensure_agent_dir("root-link-agent").is_err());
        std::env::set_var("HOME", previous_home);

        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&external).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn agent_manifest_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;

        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!(
            "pipi-agent-manifest-link-home-{}",
            crate::session::new_id()
        ));
        let external = std::env::temp_dir().join(format!(
            "pipi-agent-manifest-link-external-{}",
            crate::session::new_id()
        ));
        let agent = home.join(".pipi/agents/manifest-link-agent");
        let external_manifest = external.join("agent.json");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        let def = AgentDefinition {
            name: "manifest-link-agent".into(),
            description: "external".into(),
            model: String::new(),
            provider: None,
            workspace: None,
            permissions: PermissionsConfig::default(),
            mcp_servers: Vec::new(),
        };
        std::fs::write(
            &external_manifest,
            serde_json::to_string_pretty(&def).unwrap() + "\n",
        )
        .unwrap();
        symlink(&external_manifest, agent.join("agent.json")).unwrap();

        let previous_home = std::env::var("HOME").unwrap();
        std::env::set_var("HOME", &home);
        assert!(load_agent("manifest-link-agent").is_err());
        assert!(save_agent(&def).is_err());
        assert!(list_agents().unwrap().is_empty());
        std::env::set_var("HOME", previous_home);

        let external_contents = std::fs::read_to_string(&external_manifest).unwrap();
        assert!(external_contents.contains("external"));
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&external).unwrap();
    }

    #[test]
    fn create_defaults_to_workspace_write_sandbox() {
        let _guard = HOME_LOCK.lock().unwrap();
        let name = format!("pipi-default-sandbox-{}", crate::session::new_id());
        let def = create_agent(&name, "", None, None).unwrap();
        assert_eq!(
            def.permissions.sandbox,
            crate::permissions::SandboxMode::WorkspaceWrite
        );
        let _ = fs::remove_dir_all(agent_dir(&name).unwrap());
    }

    #[test]
    fn system_prompt_includes_wire_tool_descriptions() {
        let def = AgentDefinition {
            name: "__pipi_harness_wire_test__".into(),
            description: String::new(),
            model: String::new(),
            provider: None,
            workspace: Some(
                std::env::temp_dir()
                    .join("pipi-harness-wire-test-does-not-write")
                    .to_string_lossy()
                    .into_owned(),
            ),
            permissions: PermissionsConfig {
                tools: vec!["read".into()],
                ..Default::default()
            },
            mcp_servers: Vec::new(),
        };
        let tools = [crate::types::Tool {
            name: "read".into(),
            description: "Read files".into(),
            parameters: serde_json::json!({}),
        }];

        let prompt = build_system_prompt_with_tools(&def, &tools);

        assert!(prompt.contains("- read: Read files"));
    }

    #[cfg(unix)]
    #[test]
    fn build_tool_context_rejects_skills_symlink_outside_agent_dir() {
        use std::os::unix::fs::symlink;

        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!(
            "pipi-skills-link-home-{}",
            crate::session::new_id()
        ));
        let external = std::env::temp_dir().join(format!(
            "pipi-skills-link-external-{}",
            crate::session::new_id()
        ));
        let agent_dir = home.join(".pipi/agents/skills-link-agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        symlink(&external, agent_dir.join("skills")).unwrap();

        let previous_home = std::env::var("HOME").unwrap();
        std::env::set_var("HOME", &home);
        let def = AgentDefinition {
            name: "skills-link-agent".into(),
            description: String::new(),
            model: String::new(),
            provider: None,
            workspace: None,
            permissions: PermissionsConfig::default(),
            mcp_servers: Vec::new(),
        };

        let result = build_tool_context(&def, crate::types::AbortSignal::new());

        std::env::set_var("HOME", previous_home);
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&external).unwrap();

        let error = match result {
            Ok(_) => panic!("skills symlink outside the agent must fail closed"),
            Err(error) => error,
        };
        assert!(error.contains("skills"), "unexpected error: {error}");
    }

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
