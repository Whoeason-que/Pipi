use std::fs;
use std::path::{Path, PathBuf};

use pipi_protocol::Model;
use pipi_tools::fs::ensure_real_file;
use pipi_tools::permissions::PermissionsConfig;

use super::paths::{
    agent_dir, checked_agent_dir, checked_agents_root, expand_tilde,
    manifest_name_matches_directory, validate_agent_name,
};
use super::types::{
    AgentDefinition, DEFAULT_AGENT_DESCRIPTION, DEFAULT_AGENT_NAME,
    default_compact_threshold_percent, validate_definition,
};

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

/// 扫描一个目录下的所有合法 Agent（活跃区与归档区共用）。
/// 缺 agent.json 或解析失败的目录直接跳过 —— 文件即真相，坏文件不拖垮列表。
/// 以 `.` 开头的目录（归档区等）一律跳过。
pub(super) fn scan_agents(dir: &Path) -> Vec<AgentDefinition> {
    let mut agents = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return agents;
    };
    for entry in entries.flatten() {
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
        if directory_name.starts_with('.') {
            continue;
        }
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
    agents
}

/// 扫描 ~/.pipi/agents，返回所有合法 Agent。
pub fn list_agents() -> Result<Vec<AgentDefinition>, String> {
    let dir = checked_agents_root()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    Ok(scan_agents(&dir))
}

/// 确保默认 Agent 存在；**目录已存在时一律不动**（哪怕 manifest 被改坏，也不覆盖用户数据）。
///
/// 只在目录缺失时创建。因此删掉 `~/.pipi/agents/Pipi/` 后下次启动会重新播种；
/// 想彻底移除它，请改名而不是删除。
/// 返回 `Some(def)` 表示本次确实新建了。
pub fn ensure_default_agent() -> Result<Option<AgentDefinition>, String> {
    let dir = checked_agent_dir(DEFAULT_AGENT_NAME)?;
    if dir.exists() {
        return Ok(None);
    }
    // 权限沿用「新建 Agent」表单的默认：全部基础工具 + 受限沙箱（workspace-write）。
    // 注意不要直接传 PermissionsConfig::default()，其沙箱默认是 DangerFullAccess。
    match create_agent(
        DEFAULT_AGENT_NAME,
        DEFAULT_AGENT_DESCRIPTION,
        None,
        None,
        None,
        None,
    ) {
        Ok(def) => Ok(Some(def)),
        Err(error) => {
            // 并发播种（桌面端与 Web 端同时启动）：另一边先建好了，不算失败。
            if dir.exists() { Ok(None) } else { Err(error) }
        }
    }
}

/// 列表入口：先确保默认 Agent 已播种，再返回列表。
/// 播种失败不阻断列表（用户仍可手动创建 Agent），错误只记 stderr。
pub fn list_agents_bootstrapped() -> Result<Vec<AgentDefinition>, String> {
    if let Err(error) = ensure_default_agent() {
        eprintln!("pipi: 播种默认 Agent 失败：{error}");
    }
    list_agents()
}

/// 创建 Agent：生成目录骨架并写入 agent.json / AGENTS.md。
/// 不做任何额外存储 —— Agent 从诞生起就是一组普通文件。
pub fn create_agent(
    name: &str,
    description: &str,
    workspace: Option<&str>,
    permissions: Option<PermissionsConfig>,
    model: Option<&str>,
    provider: Option<Model>,
) -> Result<AgentDefinition, String> {
    create_agent_with_instructions(
        name,
        description,
        workspace,
        permissions,
        model,
        provider,
        None,
        false,
    )
}

/// 创建 Agent，并允许调用方在创建时直接写入 `AGENTS.md` 的指令正文及分组标记。
/// UI 的既有创建入口继续调用 [`create_agent`]（普通 Agent）；Agent 组合工具走本入口
/// 并传入 `subagent = true`，两者共享名称、路径、权限和清单校验。
pub fn create_agent_with_instructions(
    name: &str,
    description: &str,
    workspace: Option<&str>,
    permissions: Option<PermissionsConfig>,
    model: Option<&str>,
    provider: Option<Model>,
    instructions: Option<&str>,
    subagent: bool,
) -> Result<AgentDefinition, String> {
    let name = name.trim().to_string();
    validate_agent_name(&name)?;

    // 新建 Agent 默认使用受限沙箱；旧 agent.json 的 serde 默认仍保持兼容。
    let permissions = permissions.unwrap_or_else(|| PermissionsConfig {
        sandbox: pipi_tools::permissions::SandboxMode::WorkspaceWrite,
        ..Default::default()
    });

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

    let model_label = match (model.map(str::trim).filter(|m| !m.is_empty()), &provider) {
        (Some(m), _) => m.to_string(),
        (None, Some(p)) => p.display_name().to_string(),
        (None, None) => String::new(),
    };

    let def = AgentDefinition {
        name: name.clone(),
        description: description.trim().to_string(),
        model: model_label,
        provider,
        workspace: workspace.clone(),
        permissions,
        mcp_servers: Vec::new(),
        subagent,
        compact_threshold_percent: default_compact_threshold_percent(),
        compact_target_percent: None,
    };
    // 与 save_agent 同一套校验（工具名单等）
    validate_definition(&def)?;
    let dir = agent_dir(&name).ok_or_else(|| "无法定位用户主目录".to_string())?;
    if dir.exists() {
        return Err(format!("Agent「{name}」已存在"));
    }

    // 目录骨架（与 README「Agent 的组成」一一对应）
    for sub in ["skills", "memory", "sessions", "jobs"] {
        fs::create_dir_all(dir.join(sub)).map_err(|e| e.to_string())?;
    }
    if workspace.is_none() {
        fs::create_dir_all(dir.join("workspace")).map_err(|e| e.to_string())?;
    }

    let manifest = serde_json::to_string_pretty(&def).map_err(|e| e.to_string())?;
    fs::write(dir.join("agent.json"), manifest + "\n").map_err(|e| e.to_string())?;
    let intro = if def.description.is_empty() {
        "（一句话介绍这个 Agent）"
    } else {
        &def.description
    };
    let instructions = instructions
        .map(str::trim)
        .filter(|instructions| !instructions.is_empty())
        .unwrap_or("- ");
    fs::write(
        dir.join("AGENTS.md"),
        format!("# {name}\n\n{intro}\n\n## 指令\n\n{instructions}\n"),
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
/// 与 create 走同一套校验 —— UI 全字段编辑无法绕过工具/路径约束。
pub fn save_agent(def: &AgentDefinition) -> Result<(), String> {
    validate_definition(def)?;
    let dir = checked_agent_dir(&def.name)?;
    if !dir.is_dir() {
        return Err(format!("Agent「{}」不存在", def.name));
    }
    let manifest = serde_json::to_string_pretty(def).map_err(|e| e.to_string())?;
    let manifest_path = dir.join("agent.json");
    ensure_real_file(&manifest_path, "agent.json")?;
    fs::write(manifest_path, manifest + "\n").map_err(|e| e.to_string())
}

/// 交互审批「总是允许」：把命令段落追加进 Agent 的 bash 白名单并落盘。
/// 读改写之间不做跨进程互斥（与 save_agent 同一信任级别），条目去重。
pub fn add_bash_allowlist_entries(name: &str, entries: &[String]) -> Result<(), String> {
    let mut def = load_agent(name)?;
    for entry in entries {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if !def
            .permissions
            .bash
            .commands
            .iter()
            .any(|existing| existing == entry)
        {
            def.permissions.bash.commands.push(entry.to_string());
        }
    }
    save_agent(&def)
}
