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
) -> Result<(pipi_protocol::tools::ToolContext, crate::av::ResolvedEnv), String> {
    let workspace = def.resolve_workspace().ok_or("无法解析工作目录")?;
    fs::create_dir_all(&workspace).map_err(|e| format!("工作目录不可用: {e}"))?;
    let workspace =
        fs::canonicalize(&workspace).map_err(|e| format!("工作目录不可用：无法规范化路径: {e}"))?;
    let mut read_roots = match def.skills_dir().filter(|skills| skills.is_dir()) {
        Some(skills) => {
            let agent_dir = def.dir().ok_or("Agent 目录不可用：无法确定 skills 所属目录")?;
            let canonical_agent_dir = fs::canonicalize(&agent_dir)
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
        None => Vec::new(),
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
    let merged = crate::av::merge_layers(&layers)?;
    let resolved =
        crate::av::resolve_env(&layers, &crate::av::collect_process_env(), &runtime_vars)?;
    crate::av::check_requires(&merged.requires, &resolved.vars)?;

    // 声明启用的 store 技能：只读解析（不联网；缺失即 fail-closed），
    // 内容目录进受信任读取根 —— 模型用 read 工具按需加载 SKILL.md 与随附文件。
    for skill in load_declared_store_skills(&layers)? {
        if !read_roots.contains(&skill.dir) {
            read_roots.push(skill.dir);
        }
    }

    Ok((
        pipi_protocol::tools::ToolContext {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentDefinition, agent_dir, create_agent};
    use pipi_protocol::permissions::PermissionsConfig;
    use pipi_test_support::HOME_LOCK;

    #[test]
    fn tool_context_grants_read_access_to_memory_dir() {
        let _guard = HOME_LOCK.lock().unwrap();
        let name = format!("memory-read-{}", pipi_protocol::new_id());
        let def = create_agent(&name, "", None, None, None, None).unwrap();
        let memory_dir = def.memory_dir().unwrap();
        fs::write(memory_dir.join("note.md"), "hello").unwrap();

        let (ctx, _) = build_tool_context(&def, None, pipi_protocol::AbortSignal::new()).unwrap();
        let read_root = ctx
            .read_roots
            .iter()
            .find(|root| root.ends_with("memory"))
            .expect("memory dir should be a trusted read root");

        // read 工具路径解析能通过 memory 根
        let resolved = pipi_protocol::tools::resolve_read_path(
            &ctx.workspace,
            &ctx.read_roots,
            &memory_dir.join("note.md").display().to_string(),
        )
        .unwrap();
        assert!(resolved.starts_with(read_root));

        let _ = fs::remove_dir_all(agent_dir(&name).unwrap());
    }

    /// Agent 自带的技能目录搬进 harness/ 后仍是受信任读取根（与提示词策略无关：
    /// 即便策略只取工作区侧，模型也能用 read 加载技能正文）。
    #[test]
    fn tool_context_grants_read_access_to_harness_skills() {
        let _guard = HOME_LOCK.lock().unwrap();
        let name = format!("skills-read-{}", pipi_protocol::new_id());
        let def = create_agent(&name, "", None, None, None, None).unwrap();
        fs::write(def.skills_dir().unwrap().join("SKILL.md"), "body").unwrap();

        let (ctx, _) = build_tool_context(&def, None, pipi_protocol::AbortSignal::new()).unwrap();
        assert!(
            ctx.read_roots
                .iter()
                .any(|root| root.ends_with("harness/skills")),
            "harness/skills 应在 read_roots：{:?}",
            ctx.read_roots
        );

        let _ = fs::remove_dir_all(agent_dir(&name).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn build_tool_context_rejects_skills_symlink_outside_agent_dir() {
        use std::os::unix::fs::symlink;

        let _guard = HOME_LOCK.lock().unwrap();
        let home =
            std::env::temp_dir().join(format!("pipi-venv-link-home-{}", pipi_protocol::new_id()));
        let external = std::env::temp_dir().join(format!(
            "pipi-venv-link-external-{}",
            pipi_protocol::new_id()
        ));
        let agent_dir = home.join(".pipi/agents/skills-link-agent");
        std::fs::create_dir_all(agent_dir.join("harness")).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        symlink(&external, agent_dir.join("harness/skills")).unwrap();

        let previous_home = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);
        let def = AgentDefinition {
            name: "skills-link-agent".into(),
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

        let result = build_tool_context(&def, None, pipi_protocol::AbortSignal::new());
        pipi_test_support::set_env_var("HOME", previous_home);
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&external).unwrap();

        let error = match result {
            Ok(_) => panic!("skills symlink outside the agent must fail closed"),
            Err(error) => error,
        };
        assert!(error.contains("skills"), "unexpected error: {error}");
    }
}
