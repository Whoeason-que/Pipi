use std::fs;

use super::contract::collect_contract_layers;
use super::prompt::load_declared_store_skills;
use super::types::AgentDefinition;

/// 组装工具执行上下文（loop 与工具共用）。
///
/// `session_id` 注入 `AV_SESSION`（嵌套感知），无会话上下文时缺省。
/// 返回 `(ToolContext, ResolvedEnv)`：后者供环境记账与 provider key 解析复用。
pub fn build_tool_context(
    def: &AgentDefinition,
    session_id: Option<String>,
    abort: pipi_protocol::AbortSignal,
) -> Result<(pipi_tools::ToolContext, av::ResolvedEnv), String> {
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
    if let Some(memory_dir) = def.memory_dir()
        && memory_dir.is_dir()
        && let Ok(canonical_memory) = fs::canonicalize(&memory_dir)
    {
        read_roots.push(canonical_memory);
    }

    // av 环境契约：会话启动时解析一次，本会话所有工具子进程共用同一份
    // （inherit/ignore/set/path-prepend/secrets + AV_* 运行时注入）。
    // Agent 定义目录的 agent.toml 作为最低声明层参与合并。
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
    let (agent_layer, project_layers) = collect_contract_layers(def, Some(&workspace))?;
    let mut layers = Vec::new();
    if let Some(layer) = &agent_layer {
        layers.push(layer.clone());
    }
    layers.extend(project_layers);
    let merged = av::merge_layers(&layers)?;
    let resolved = av::resolve_env(&layers, &av::collect_process_env(), &runtime_vars)?;
    av::check_requires(&merged.requires, &resolved.vars)?;

    // 声明启用的 store 技能：只读解析（不联网；缺失即 fail-closed），
    // 内容目录进受信任读取根 —— 模型用 read 工具按需加载 SKILL.md 与随附文件。
    for skill in load_declared_store_skills(&layers)? {
        if !read_roots.contains(&skill.dir) {
            read_roots.push(skill.dir);
        }
    }

    Ok((
        pipi_tools::ToolContext {
            workspace,
            memory_dir: def.memory_dir(),
            read_roots,
            permissions: std::sync::Arc::new(def.permissions.clone()),
            sandbox: def.permissions.sandbox,
            resolved_env: std::sync::Arc::new(resolved.vars.clone()),
            abort,
            approver: None,
        },
        resolved,
    ))
}
