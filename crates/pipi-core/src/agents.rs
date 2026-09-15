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

/// 归档区目录名（agents 根与各 Agent 的 sessions 目录共用）：
/// `~/.pipi/agents/.archive/` 与 `<agent>/sessions/.archive/`。以点开头，
/// 列表扫描天然跳过，文件即真相 —— 归档就是一次目录/文件移动。
pub const ARCHIVE_DIR: &str = ".archive";

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

pub(crate) fn ensure_real_directory(path: &Path, label: &str) -> Result<bool, String> {
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

pub(crate) fn ensure_real_file(path: &Path, label: &str) -> Result<bool, String> {
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

/// 扫描一个目录下的所有合法 Agent（活跃区与归档区共用）。
/// 缺 agent.json 或解析失败的目录直接跳过 —— 文件即真相，坏文件不拖垮列表。
/// 以 `.` 开头的目录（归档区等）一律跳过。
fn scan_agents(dir: &Path) -> Vec<AgentDefinition> {
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

// ============ 归档 / 恢复 / 删除 ============
// 归档 = 移动到 `agents/.archive/<name>/`；删除 = 彻底删目录。
// 都是纯文件操作：移动失败、目录不存在都直接报中文错误。
// 调用方（命令层）负责先做「会话占用」检查 —— 打开中的 Agent 目录不能挪走。

fn checked_archive_dir() -> Result<PathBuf, String> {
    Ok(checked_agents_root()?.join(ARCHIVE_DIR))
}

fn checked_archived_agent_dir(name: &str) -> Result<PathBuf, String> {
    validate_agent_name(name)?;
    let dir = checked_archive_dir()?.join(name);
    ensure_real_directory(&dir, "归档 Agent 目录")?;
    Ok(dir)
}

/// 列出已归档 Agent（`~/.pipi/agents/.archive/` 下）。
pub fn list_archived_agents() -> Result<Vec<AgentDefinition>, String> {
    let dir = checked_archive_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    Ok(scan_agents(&dir))
}

/// 归档 Agent：把 `agents/<name>/` 整体移入 `agents/.archive/<name>/`。
pub fn archive_agent(name: &str) -> Result<(), String> {
    let src = checked_agent_dir(name)?;
    let archive_dir = checked_archive_dir()?;
    fs::create_dir_all(&archive_dir).map_err(|e| format!("无法创建归档目录: {e}"))?;
    let dst = archive_dir.join(name);
    if dst.exists() {
        return Err(format!("归档区已存在同名 Agent「{name}」"));
    }
    fs::rename(&src, &dst).map_err(|e| format!("归档 Agent「{name}」失败: {e}"))
}

/// 恢复归档 Agent：移回 `agents/<name>/`。
pub fn restore_agent(name: &str) -> Result<(), String> {
    let src = checked_archived_agent_dir(name)?;
    let root = checked_agents_root()?;
    let dst = root.join(name);
    if dst.exists() {
        return Err(format!("活跃区已存在同名 Agent「{name}」"));
    }
    fs::rename(&src, &dst).map_err(|e| format!("恢复 Agent「{name}」失败: {e}"))
}

/// 彻底删除 Agent（含其目录下的 session、workspace、skills、memory）。
/// 不可恢复；调用方应先在 UI 层做确认。
pub fn delete_agent(name: &str) -> Result<(), String> {
    let dir = checked_agent_dir(name)?;
    fs::remove_dir_all(&dir).map_err(|e| format!("删除 Agent「{name}」失败: {e}"))
}

/// 彻底删除已归档 Agent（`agents/.archive/<name>/`）。不可恢复。
pub fn delete_archived_agent(name: &str) -> Result<(), String> {
    let dir = checked_archived_agent_dir(name)?;
    fs::remove_dir_all(&dir).map_err(|e| format!("删除归档 Agent「{name}」失败: {e}"))
}

/// 默认 Agent 的名字。首次启动（或该 Agent 缺失）时由核心播种，保证开箱即可用。
/// 它与用户自建 Agent 完全同构：就是 `~/.pipi/agents/<name>/` 下的一组普通文件。
pub const DEFAULT_AGENT_NAME: &str = "Pipi";

/// 播种默认 Agent 时写入的描述（同时作为它 AGENTS.md 的首段）。
const DEFAULT_AGENT_DESCRIPTION: &str =
    "Pipi 自带的默认 Agent：开箱可用；工作目录、技能与记忆都在 ~/.pipi/agents/Pipi/ 下，改文件即生效";

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
    // 权限沿用「新建 Agent」表单的默认：全部已知工具 + 受限沙箱（workspace-write）。
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
            if dir.exists() {
                Ok(None)
            } else {
                Err(error)
            }
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
        return Err("至少启用一个工具".into());
    }
    for tool in &def.permissions.tools {
        if !KNOWN_TOOLS.contains(&tool.as_str()) {
            return Err(format!(
                "未知工具: {tool}（可用：{}）",
                KNOWN_TOOLS.join(", ")
            ));
        }
    }
    if let Some(workspace) = def.workspace.as_deref().map(str::trim).filter(|w| !w.is_empty()) {
        let expanded = expand_tilde(workspace);
        if !PathBuf::from(&expanded).is_absolute() {
            return Err(format!("工作目录必须是绝对路径: {workspace}"));
        }
    }
    let mut seen_servers = std::collections::HashSet::new();
    for server in &def.mcp_servers {
        if server.name.trim().is_empty() {
            return Err("MCP 服务器名称不能为空".into());
        }
        if !seen_servers.insert(server.name.trim().to_string()) {
            return Err(format!("MCP 服务器名称重复: {}", server.name));
        }
    }
    Ok(())
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
    let name = name.trim().to_string();
    validate_agent_name(&name)?;

    // 新建 Agent 默认使用受限沙箱；旧 agent.json 的 serde 默认仍保持兼容。
    let permissions = permissions.unwrap_or_else(|| PermissionsConfig {
        sandbox: crate::permissions::SandboxMode::WorkspaceWrite,
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
    };
    // 与 save_agent 同一套校验（工具名单等）
    validate_definition(&def)?;
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

/// 解析 Agent 目录内允许 UI 编辑的 Markdown 相对路径。
///
/// 只放行两类：根目录的 `AGENTS.md`，以及 `memory/` 下的 `.md` 文件树
/// （深度 ≤ 3，对齐 memory 工具的 `collect_markdown`）。其余路径一律
/// 拒绝 —— UI 编辑器不是任意文件写入通道。任何组件都不允许 `..`、
/// 点开头或符号链接（fail-closed）。
fn resolve_agent_md_path(dir: &Path, rel_path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(rel_path);
    if relative.is_absolute() {
        return Err(format!("必须是相对路径: {rel_path}"));
    }
    let components: Vec<std::ffi::OsString> = relative
        .components()
        .map(|c| c.as_os_str().to_owned())
        .collect();
    if components.is_empty() {
        return Err("路径不能为空".into());
    }
    let dot_or_escape = components
        .iter()
        .any(|c| c == ".." || c.to_string_lossy().starts_with('.'));
    if dot_or_escape {
        return Err(format!("路径不合法: {rel_path}"));
    }

    let first = components[0].to_string_lossy().into_owned();
    let target = if first == "AGENTS.md" {
        if components.len() != 1 {
            return Err(format!("AGENTS.md 只能是 Agent 目录根下的文件: {rel_path}"));
        }
        dir.join("AGENTS.md")
    } else if first == "memory" {
        if components.len() < 2 || components.len() > 4 {
            return Err(format!("memory 路径深度须为 1-3 层: {rel_path}"));
        }
        let file_name = components.last().unwrap().to_string_lossy().into_owned();
        if !file_name.ends_with(".md") || file_name.len() <= 3 {
            return Err(format!("memory 下只能编辑 .md 文件: {rel_path}"));
        }
        let mut path = dir.to_path_buf();
        for component in &components {
            path.push(component);
        }
        path
    } else {
        return Err(format!("只允许编辑 AGENTS.md 与 memory/*.md: {rel_path}"));
    };

    // 中间目录与目标本身都不能是符号链接/非常规条目
    let mut current = dir.to_path_buf();
    for component in &components {
        current.push(component);
        if current == target {
            ensure_real_file(&current, "目标文件")?;
        } else {
            ensure_real_directory(&current, "路径目录")?;
        }
    }
    Ok(target)
}

/// 读取 Agent 目录内的可编辑 Markdown（AGENTS.md / memory/*.md）。
pub fn read_agent_file(agent_name: &str, rel_path: &str) -> Result<String, String> {
    let dir = checked_agent_dir(agent_name)?;
    let target = resolve_agent_md_path(&dir, rel_path)?;
    if !target.is_file() {
        return Err(format!("文件不存在: {rel_path}"));
    }
    fs::read_to_string(&target).map_err(|e| format!("无法读取 {rel_path}: {e}"))
}

/// 写入 Agent 目录内的可编辑 Markdown（AGENTS.md / memory/*.md）。
/// memory 下的新文件允许创建（父目录自动补齐）；AGENTS.md 必须已存在。
pub fn write_agent_file(agent_name: &str, rel_path: &str, content: &str) -> Result<(), String> {
    let dir = checked_agent_dir(agent_name)?;
    let target = resolve_agent_md_path(&dir, rel_path)?;
    if rel_path == "AGENTS.md" && !target.is_file() {
        return Err("AGENTS.md 不存在（不应删除 Agent 的系统指令文件）".into());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("无法创建目录 {}: {e}", parent.display()))?;
    }
    fs::write(&target, content).map_err(|e| format!("无法写入 {rel_path}: {e}"))
}

/// 列出 Agent 目录内可编辑的 Markdown 相对路径（`AGENTS.md` + `memory/*.md`）。
/// UI 文件编辑器与 memory 索引共用此扫描（复用 memory 工具的 collect_markdown）。
pub fn list_agent_md_files(agent_name: &str) -> Result<Vec<String>, String> {
    let dir = checked_agent_dir(agent_name)?;
    let mut out = Vec::new();
    if dir.join("AGENTS.md").is_file() {
        out.push("AGENTS.md".to_string());
    }
    let mut rels = Vec::new();
    crate::tools::memory::collect_markdown(&dir.join("memory"), 0, &mut rels);
    rels.sort();
    for rel in rels {
        out.push(format!("memory/{rel}"));
    }
    Ok(out)
}

/// memory 渐进召回的单条索引：相对 memory 目录的路径 + 一句话摘要。
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryFileMeta {
    /// 相对 memory 目录的路径（`/` 分隔）。
    pub path: String,
    /// 首个标题或首行非空文本（截断）。
    pub summary: String,
}

/// 摘要截断长度（字符）；超长以省略号收尾。
const MEMORY_SUMMARY_MAX_CHARS: usize = 80;

/// 从 memory 文件内容提取摘要：优先首个 `#` 标题，否则首行非空文本。
fn memory_summary(content: &str) -> String {
    let mut fallback = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let candidate = trimmed.trim_start_matches('#').trim();
        if line.trim_start().starts_with('#') && !candidate.is_empty() {
            return truncate_summary(candidate);
        }
        if fallback.is_empty() {
            fallback = candidate.to_string();
        }
    }
    truncate_summary(&fallback)
}

fn truncate_summary(value: &str) -> String {
    let char_count = value.chars().count();
    if char_count <= MEMORY_SUMMARY_MAX_CHARS {
        return value.to_string();
    }
    let head: String = value.chars().take(MEMORY_SUMMARY_MAX_CHARS).collect();
    format!("{head}…")
}

/// 生成 memory 渐进召回索引（空目录返回空 —— 零打扰）。
/// `path` 是绝对路径：模型用 read 工具直接加载正文（memory 目录在
/// build_tool_context 里加入了 read_roots）。
pub fn memory_index(def: &AgentDefinition) -> Vec<crate::harness::MemoryFileMeta> {
    let Some(dir) = def.memory_dir() else {
        return Vec::new();
    };
    let mut rels = Vec::new();
    crate::tools::memory::collect_markdown(&dir, 0, &mut rels);
    rels.sort();
    rels
        .into_iter()
        .map(|rel| {
            let summary = fs::read_to_string(dir.join(&rel))
                .map(|content| memory_summary(&content))
                .unwrap_or_default();
            crate::harness::MemoryFileMeta {
                path: dir.join(&rel).display().to_string(),
                summary,
            }
        })
        .collect()
}

/// 组装工具执行上下文（loop 与工具共用）。
///
/// `session_id` 注入 `AV_SESSION`（嵌套感知），无会话上下文时缺省。
pub fn build_tool_context(
    def: &AgentDefinition,
    session_id: Option<String>,
    abort: crate::types::AbortSignal,
) -> Result<crate::tools::ToolContext, String> {
    let workspace = def.resolve_workspace().ok_or("无法解析工作目录")?;
    fs::create_dir_all(&workspace).map_err(|e| format!("工作目录不可用: {e}"))?;
    let workspace =
        fs::canonicalize(&workspace).map_err(|e| format!("工作目录不可用：无法规范化路径: {e}"))?;
    let mut read_roots = match def.dir().map(|dir| dir.join("skills")) {
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
    // memory 渐进召回：正文由模型用 read 按需加载，memory 目录须在受信任
    // 读取根内（memory 工具本身仍走自己的 memory_dir 约束）。
    if let Some(memory_dir) = def.memory_dir() {
        if memory_dir.is_dir() {
            if let Ok(canonical_memory) = fs::canonicalize(&memory_dir) {
                read_roots.push(canonical_memory);
            }
        }
    }

    // av 环境契约：会话启动时解析一次，本会话所有工具子进程共用同一份
    // （inherit/ignore/set/path-prepend/secrets + AV_* 运行时注入）。
    let mut runtime_vars = std::collections::BTreeMap::new();
    runtime_vars.insert("AV".to_string(), "1".to_string());
    runtime_vars.insert("AV_AGENT".to_string(), def.name.clone());
    runtime_vars.insert("AV_WORKSPACE".to_string(), workspace.display().to_string());
    runtime_vars.insert(
        "AV_SANDBOX".to_string(),
        def.permissions.sandbox.as_str().to_string(),
    );
    if let Some(session_id) = &session_id {
        runtime_vars.insert("AV_SESSION".to_string(), session_id.clone());
    }
    let discovered = av::discover(&workspace)?;
    let merged = av::merge_layers(&discovered.layers)?;
    let resolved = av::resolve_env(&discovered.layers, &av::collect_process_env(), &runtime_vars)?;
    av::check_requires(&merged.requires, &resolved.vars)?;

    Ok(crate::tools::ToolContext {
        workspace,
        memory_dir: def.memory_dir(),
        read_roots,
        permissions: std::sync::Arc::new(def.permissions.clone()),
        sandbox: def.permissions.sandbox,
        resolved_env: std::sync::Arc::new(resolved.vars),
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
        memory_files: memory_index(def),
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
        let def = create_agent(&name, "", None, None, None, None).unwrap();
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

    #[test]
    fn system_prompt_injects_memory_index_with_summaries() {
        let _guard = HOME_LOCK.lock().unwrap();
        let name = format!("memory-index-{}", crate::session::new_id());
        let def = create_agent(&name, "", None, None, None, None).unwrap();
        let memory_dir = def.memory_dir().unwrap();
        fs::write(
            memory_dir.join("user-prefs.md"),
            "# 用户偏好\n\n简洁回答，中文优先。\n",
        )
        .unwrap();

        let prompt = build_system_prompt_with_tools(&def, &[]);
        assert!(prompt.contains("<persistent_memory>"));
        assert!(prompt.contains("<path>"));
        assert!(prompt
            .contains(&memory_dir.join("user-prefs.md").display().to_string()));
        assert!(prompt.contains("用户偏好"));
        // 渐进披露：只注入摘要，不注入正文
        assert!(prompt.contains("简洁回答") || !prompt.contains("中文优先"));

        // 空 memory 时零打扰
        fs::remove_file(memory_dir.join("user-prefs.md")).unwrap();
        let prompt = build_system_prompt_with_tools(&def, &[]);
        assert!(!prompt.contains("<persistent_memory>"));

        let _ = fs::remove_dir_all(agent_dir(&name).unwrap());
    }

    #[test]
    fn tool_context_grants_read_access_to_memory_dir() {
        let _guard = HOME_LOCK.lock().unwrap();
        let name = format!("memory-read-{}", crate::session::new_id());
        let def = create_agent(&name, "", None, None, None, None).unwrap();
        let memory_dir = def.memory_dir().unwrap();
        fs::write(memory_dir.join("note.md"), "hello").unwrap();

        let ctx = build_tool_context(&def, None, crate::types::AbortSignal::new()).unwrap();
        let read_root = ctx
            .read_roots
            .iter()
            .find(|root| root.ends_with("memory"))
            .expect("memory dir should be a trusted read root");

        // read 工具路径解析能通过 memory 根
        let resolved = crate::tools::resolve_read_path(
            &ctx.workspace,
            &ctx.read_roots,
            &memory_dir.join("note.md").display().to_string(),
        )
        .unwrap();
        assert!(resolved.starts_with(read_root));

        let _ = fs::remove_dir_all(agent_dir(&name).unwrap());
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

        let result = build_tool_context(&def, None, crate::types::AbortSignal::new());

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
    fn agent_file_editor_is_restricted_to_agents_md_and_memory() {        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("pipi-edit-{}", crate::session::new_id()));
        std::fs::create_dir_all(&home).unwrap();
        let prev = std::env::var("HOME").unwrap();
        std::env::set_var("HOME", &home);
        create_agent("editor", "d", None, None, None, None).unwrap();

        // AGENTS.md 可读写
        write_agent_file("editor", "AGENTS.md", "# 新指令\n").unwrap();
        assert_eq!(read_agent_file("editor", "AGENTS.md").unwrap(), "# 新指令\n");

        // memory 新文件可创建并读写
        write_agent_file("editor", "memory/user-prefs.md", "偏好： concise\n").unwrap();
        assert!(read_agent_file("editor", "memory/user-prefs.md")
            .unwrap()
            .contains("偏好"));

        // 拒绝：绝对路径、逃逸、agent.json、其他扩展名、过深路径
        assert!(read_agent_file("editor", "/etc/passwd").is_err());
        assert!(read_agent_file("editor", "../escape.md").is_err());
        assert!(write_agent_file("editor", "agent.json", "{}").is_err());
        assert!(write_agent_file("editor", "memory/notes.txt", "x").is_err());
        assert!(write_agent_file("editor", "skills/tool/SKILL.md", "x").is_err());
        assert!(write_agent_file("editor", "memory/a/b/c/d.md", "x").is_err());
        assert!(read_agent_file("editor", "memory/missing.md").is_err());
        // AGENTS.md 不允许通过 write 新建（已被 create_agent 管理）
        std::fs::remove_file(home.join(".pipi/agents/editor/AGENTS.md")).unwrap();
        assert!(write_agent_file("editor", "AGENTS.md", "x").is_err());

        std::env::set_var("HOME", prev);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn create_rejects_bad_names_and_tools() {
        assert!(create_agent("", "d", None, None, None, None).is_err());
        assert!(create_agent("bad name", "d", None, None, None, None).is_err());
        assert!(create_agent("../escape", "d", None, None, None, None).is_err());
        assert!(create_agent(
            "x",
            "d",
            None,
            Some(PermissionsConfig {
                tools: vec!["nuclear".into()],
                bash: Default::default(),
                sandbox: Default::default(),
            }),
            None,
            None,
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
            }),
            None,
            None,
        )
        .is_err());
    }

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
            None,
            None,
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
        assert!(create_agent("tester", "", None, None, None, None).is_err());
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
        let result = create_agent("ws-agent", "", Some(ws.to_str().unwrap()), None, None, None);
        std::env::set_var("HOME", &prev);
        let def = result.unwrap();
        assert_eq!(def.workspace.as_deref(), Some(ws.to_str().unwrap()));
        assert!(!home.join(".pipi/agents/ws-agent/workspace").exists());
        assert_eq!(def.resolve_workspace().unwrap(), ws);
    }

    #[test]
    fn create_with_default_model() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("pipi-home-{}", crate::session::new_id()));
        std::fs::create_dir_all(&home).unwrap();
        let prev = std::env::var("HOME").unwrap();
        std::env::set_var("HOME", &home);
        let provider = Model {
            id: "claude-3-7-sonnet-20250219".into(),
            name: "Claude 3.7 Sonnet".into(),
            api: crate::types::Api::AnthropicMessages,
            base_url: "https://api.anthropic.com".into(),
            max_tokens: 8192,
            context_window: 200000,
        };
        let result = create_agent(
            "model-agent",
            "测试默认模型",
            None,
            None,
            Some("claude-3-7-sonnet-20250219"),
            Some(provider.clone()),
        );
        let def = result.unwrap();
        assert_eq!(def.model, "claude-3-7-sonnet-20250219");
        assert_eq!(def.provider, Some(provider));

        let loaded = load_agent("model-agent");
        std::env::set_var("HOME", &prev);
        let loaded = loaded.unwrap();
        assert_eq!(loaded.model, "claude-3-7-sonnet-20250219");
        assert!(loaded.provider.is_some());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn seeds_default_agent_when_missing_and_never_overwrites() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home =
            std::env::temp_dir().join(format!("pipi-home-seed-{}", crate::session::new_id()));
        std::fs::create_dir_all(&home).unwrap();
        let prev = std::env::var("HOME").unwrap();
        std::env::set_var("HOME", &home);

        // 目录缺失 → 播种；骨架与 README「Agent 的组成」一致
        let created = ensure_default_agent()
            .unwrap()
            .expect("首次调用应播种默认 Agent");
        assert_eq!(created.name, DEFAULT_AGENT_NAME);
        let dir = home.join(".pipi").join("agents").join(DEFAULT_AGENT_NAME);
        for entry in [
            "agent.json",
            "AGENTS.md",
            "skills",
            "memory",
            "sessions",
            "workspace",
        ] {
            assert!(dir.join(entry).exists(), "{entry} 应存在");
        }
        // 默认权限：全部工具 + 受限沙箱（PermissionsConfig::default 的沙箱是 DangerFullAccess，不能被沿用）
        assert_eq!(
            created.permissions.sandbox,
            crate::permissions::SandboxMode::WorkspaceWrite
        );
        assert!(created.permissions.tools.iter().any(|tool| tool == "bash"));
        assert!(
            created.provider.is_none(),
            "播种的默认 Agent 不应凭空绑定模型"
        );

        // 已存在 → 不再创建，且不覆盖用户改动
        std::fs::write(dir.join("AGENTS.md"), "# Pipi\n\n用户改过的说明\n").unwrap();
        assert!(ensure_default_agent().unwrap().is_none());
        let agents = list_agents_bootstrapped().unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, DEFAULT_AGENT_NAME);
        assert!(
            std::fs::read_to_string(dir.join("AGENTS.md"))
                .unwrap()
                .contains("用户改过的说明"),
            "播种不得覆盖用户对 AGENTS.md 的改动"
        );

        std::env::set_var("HOME", &prev);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn bootstrap_keeps_existing_agents_and_backfills_default() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home =
            std::env::temp_dir().join(format!("pipi-home-fill-{}", crate::session::new_id()));
        std::fs::create_dir_all(&home).unwrap();
        let prev = std::env::var("HOME").unwrap();
        std::env::set_var("HOME", &home);

        create_agent("helper", "已有的 Agent", None, None, None, None).unwrap();
        assert_eq!(
            list_agents().unwrap().len(),
            1,
            "list_agents 本身不应有副作用"
        );

        // 有其它 Agent 但没有默认 Agent → 保留原样并补上默认 Agent
        let agents = list_agents_bootstrapped().unwrap();
        let names: Vec<String> = agents.iter().map(|agent| agent.name.clone()).collect();
        assert!(names.contains(&"helper".to_string()));
        assert!(names.contains(&DEFAULT_AGENT_NAME.to_string()));
        assert_eq!(names.len(), 2);
        // 幂等
        assert!(ensure_default_agent().unwrap().is_none());
        assert_eq!(list_agents().unwrap().len(), 2);

        std::env::set_var("HOME", &prev);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn agent_archive_restore_delete_lifecycle() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home =
            std::env::temp_dir().join(format!("pipi-home-arch-{}", crate::session::new_id()));
        std::fs::create_dir_all(&home).unwrap();
        let prev = std::env::var("HOME").unwrap();
        std::env::set_var("HOME", &home);

        let name = "arch-lifecycle";
        create_agent(name, "归档生命周期", None, None, None, None).unwrap();
        assert!(list_agents().unwrap().iter().any(|a| a.name == name));
        assert!(list_archived_agents().unwrap().is_empty());

        // 归档：活跃列表消失、归档列表出现、目录真的移动了
        archive_agent(name).unwrap();
        assert!(!list_agents().unwrap().iter().any(|a| a.name == name));
        assert!(list_archived_agents().unwrap().iter().any(|a| a.name == name));
        let dir = agents_dir().unwrap();
        assert!(dir.join(ARCHIVE_DIR).join(name).is_dir());
        assert!(!dir.join(name).exists());

        // 重复归档同一名字的 Agent → 该名字现在已不在活跃区 → 报错
        assert!(archive_agent(name).is_err());

        // 恢复：回到活跃区
        restore_agent(name).unwrap();
        assert!(list_agents().unwrap().iter().any(|a| a.name == name));
        assert!(!list_archived_agents().unwrap().iter().any(|a| a.name == name));

        // 恢复一个不存在的归档 Agent → 报错
        assert!(restore_agent("no-such-archived-agent").is_err());

        // 删除（活跃区）
        delete_agent(name).unwrap();
        assert!(!list_agents().unwrap().iter().any(|a| a.name == name));

        // 归档区删除
        archive_agent("arch-lifecycle-2").unwrap_or_else(|_| {
            create_agent("arch-lifecycle-2", "归档删除", None, None, None, None).unwrap();
            archive_agent("arch-lifecycle-2").unwrap();
        });
        // 上一步用 create+archive 保证该名字存在
        delete_archived_agent("arch-lifecycle-2").unwrap();
        assert!(!list_archived_agents().unwrap().iter().any(|a| a.name == "arch-lifecycle-2"));

        // 删除不存在的 Agent → 报错
        assert!(delete_agent("no-such-agent").is_err());

        std::env::set_var("HOME", &prev);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn archive_scan_skips_dot_dir_with_manifest() {
        // 归档区里即使意外出现 agent.json，活跃列表也不得扫到（点目录硬跳过）
        let _guard = HOME_LOCK.lock().unwrap();
        let home =
            std::env::temp_dir().join(format!("pipi-home-dot-{}", crate::session::new_id()));
        std::fs::create_dir_all(&home).unwrap();
        let prev = std::env::var("HOME").unwrap();
        std::env::set_var("HOME", &home);

        let name = "dot-dir-agent";
        create_agent(name, "点目录防漏", None, None, None, None).unwrap();
        archive_agent(name).unwrap();
        // 往归档区里塞一个「坏掉」的 agent.json（不该影响活跃列表）
        let archive_agent_dir = agents_dir().unwrap().join(ARCHIVE_DIR).join(name);
        std::fs::write(archive_agent_dir.join("agent.json"), "{ broken json").unwrap();
        assert!(!list_agents().unwrap().iter().any(|a| a.name == name));

        std::env::set_var("HOME", &prev);
        let _ = std::fs::remove_dir_all(&home);
    }
}
