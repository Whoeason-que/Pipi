use std::fs;

use super::preparation::PreparedAgent;
use super::types::AgentDefinition;

/// 组装工具执行上下文（loop 与工具共用）。
///
/// `session_id` 注入 `AV_SESSION`（嵌套感知），无会话上下文时缺省。
/// 返回 `(ToolContext, ResolvedEnv)`：后者供环境记账与 provider key 解析复用。
///
/// 契约层、Agent 侧素材与声明的 store 技能都不在这里解析 —— 它们由调用方
/// 先经 [`super::preparation::prepare_agent`] 备好（`prepared`），本函数只做
/// 消费与组装。Agent 侧的技能目录与记忆目录取契约声明（见 `agent_material`）：
/// 缺契约或漏声明在准备阶段即 fail-closed，两处都进受信任读取根。
pub fn build_tool_context(
    def: &AgentDefinition,
    prepared: &PreparedAgent,
    session_id: Option<String>,
    abort: pipi_protocol::AbortSignal,
) -> Result<(pipi_protocol::tools::ToolContext, crate::av::ResolvedEnv), String> {
    let workspace = def.resolve_workspace().ok_or("无法解析工作目录")?;
    fs::create_dir_all(&workspace).map_err(|e| format!("工作目录不可用: {e}"))?;
    let workspace =
        fs::canonicalize(&workspace).map_err(|e| format!("工作目录不可用：无法规范化路径: {e}"))?;

    // av 环境契约：会话启动时解析一次，本会话所有工具子进程共用同一份
    // （inherit/ignore/set/path-prepend/secrets + AV_* 运行时注入）。
    // 契约层来自 prepared（agent 定义目录的 agent.toml 已作为最低声明层参与）。
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
    let layers = prepared.layers();
    let merged = prepared.merged();
    let resolved =
        crate::av::resolve_env(layers, &crate::av::collect_process_env(), &runtime_vars)?;
    crate::av::check_requires(&merged.requires, &resolved.vars)?;

    // Agent 侧素材路径已由契约编排并在准备阶段校验过包含根（符号链接逃逸
    // 会被拒）；这里只取用，不再重读。
    let material = prepared.material();
    let mut read_roots = material.skill_sources.clone();
    // memory 渐进召回：正文由模型用 read 按需加载，memory 目录须在受信任
    // 读取根内（memory 工具本身仍走自己的 memory_dir 约束）。
    if material.memory_dir.is_dir() {
        read_roots.push(material.memory_dir.clone());
    }

    // 声明启用的 store 技能（准备阶段已只读解析、不联网；缺失即 fail-closed），
    // 内容目录进受信任读取根 —— 模型用 read 工具按需加载 SKILL.md 与随附文件。
    for skill in prepared.store_skills() {
        if !read_roots.contains(&skill.dir) {
            read_roots.push(skill.dir.clone());
        }
    }

    Ok((
        pipi_protocol::tools::ToolContext {
            workspace,
            memory_dir: Some(material.memory_dir.clone()),
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
    use crate::agents::{
        AgentDefinition, DEFAULT_HARNESS_DIR, agent_dir, create_agent, prepare_agent,
    };

    /// 默认模板声明的素材目录（`<Agent 目录>/harness`）。
    fn default_harness(agent_dir: &std::path::Path) -> std::path::PathBuf {
        agent_dir.join(DEFAULT_HARNESS_DIR)
    }
    use pipi_protocol::permissions::PermissionsConfig;
    use pipi_test_support::HOME_LOCK;

    #[test]
    fn tool_context_grants_read_access_to_memory_dir() {
        let _guard = HOME_LOCK.lock().unwrap();
        let name = format!("memory-read-{}", pipi_protocol::new_id());
        let def = create_agent(&name, "", None, None, None, None).unwrap();
        let memory_dir = default_harness(&agent_dir(&name).unwrap()).join("memory");
        fs::write(memory_dir.join("note.md"), "hello").unwrap();

        let prepared = prepare_agent(&def).unwrap();
        let (ctx, _) =
            build_tool_context(&def, &prepared, None, pipi_protocol::AbortSignal::new()).unwrap();
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

    /// 契约声明的技能来源目录进受信任读取根（与提示词策略无关：即便策略只取
    /// 工作区侧，模型也能用 read 加载技能正文）。
    #[test]
    fn tool_context_grants_read_access_to_declared_skills() {
        let _guard = HOME_LOCK.lock().unwrap();
        let name = format!("skills-read-{}", pipi_protocol::new_id());
        let def = create_agent(&name, "", None, None, None, None).unwrap();
        let skills = default_harness(&agent_dir(&name).unwrap()).join("skills");
        fs::write(skills.join("SKILL.md"), "body").unwrap();

        let prepared = prepare_agent(&def).unwrap();
        let (ctx, _) =
            build_tool_context(&def, &prepared, None, pipi_protocol::AbortSignal::new()).unwrap();
        assert!(
            ctx.read_roots
                .iter()
                .any(|root| root.ends_with("harness/skills")),
            "harness/skills 应在 read_roots：{:?}",
            ctx.read_roots
        );

        let _ = fs::remove_dir_all(agent_dir(&name).unwrap());
    }

    /// 改契约声明即改位置：提示词、记忆工具根、读取根与编辑器视图一起跟着走，
    /// 未声明的旧目录（harness/）不再参与。
    #[test]
    fn declared_locations_drive_prompt_read_roots_and_editor() {
        let _guard = HOME_LOCK.lock().unwrap();
        let name = format!("declared-{}", pipi_protocol::new_id());
        let def = create_agent(&name, "", None, None, None, None).unwrap();
        let dir = agent_dir(&name).unwrap();

        fs::create_dir_all(dir.join("notes").join("mem")).unwrap();
        fs::write(dir.join("notes").join("PROMPT.md"), "custom instructions").unwrap();
        fs::write(dir.join("notes").join("mem").join("a.md"), "# A\n\nbody\n").unwrap();
        fs::write(
            dir.join("agent.toml"),
            "schema = 1\n[resources]\ninstructions = [\"notes/PROMPT.md\"]\nmemory = \"notes/mem\"\n[resources.skills]\nsources = []\n",
        )
        .unwrap();

        let prepared = prepare_agent(&def).unwrap();
        let (ctx, _) =
            build_tool_context(&def, &prepared, None, pipi_protocol::AbortSignal::new()).unwrap();
        let memory_dir = ctx.memory_dir.clone().expect("memory 目录来自契约声明");
        assert!(memory_dir.ends_with("notes/mem"), "{memory_dir:?}");
        assert!(
            ctx.read_roots
                .iter()
                .any(|root| root.ends_with("notes/mem")),
            "记忆目录应在 read_roots：{:?}",
            ctx.read_roots
        );
        assert!(
            !ctx.read_roots
                .iter()
                .any(|root| root.ends_with("harness/skills")),
            "未声明的目录不该进读取根：{:?}",
            ctx.read_roots
        );

        let prompt = crate::agents::build_system_prompt_with_tools(&def, &prepared, &[]).unwrap();
        assert!(prompt.contains("custom instructions"), "{prompt}");
        assert!(
            !prompt.contains(&format!("# {name}")),
            "create_agent 写的 harness/AGENTS.md 未被声明，不该注入"
        );
        assert!(prompt.contains("<persistent_memory>"));

        let view = crate::agents::list_agent_editable_files(&name).unwrap();
        assert_eq!(view.instructions, vec!["notes/PROMPT.md".to_string()]);
        assert_eq!(view.memory, vec!["notes/mem/a.md".to_string()]);
        assert_eq!(view.memory_prefix, "notes/mem");

        // 未声明的路径不再是可编辑位置
        assert!(crate::agents::write_agent_file(&name, "harness/AGENTS.md", "x").is_err());
        // 声明过的记忆文件可以写
        crate::agents::write_agent_file(&name, "notes/mem/b.md", "hello").unwrap();
        assert!(
            dir.join("notes").join("mem").join("b.md").is_file(),
            "新记忆文件应落在契约声明的目录里"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[cfg(unix)]
    #[test]
    fn prepare_agent_rejects_skills_symlink_outside_agent_dir() {
        use std::os::unix::fs::symlink;

        let _guard = HOME_LOCK.lock().unwrap();
        let home =
            std::env::temp_dir().join(format!("pipi-venv-link-home-{}", pipi_protocol::new_id()));
        let external = std::env::temp_dir().join(format!(
            "pipi-venv-link-external-{}",
            pipi_protocol::new_id()
        ));
        let agent_dir = home.join(".pipi/agents/skills-link-agent");
        std::fs::create_dir_all(agent_dir.join(DEFAULT_HARNESS_DIR)).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        symlink(
            &external,
            agent_dir.join(DEFAULT_HARNESS_DIR).join("skills"),
        )
        .unwrap();
        std::fs::write(
            agent_dir.join("agent.toml"),
            "schema = 1\n[resources]\ninstructions = []\nmemory = \"harness/memory\"\n[resources.skills]\nsources = [\"harness/skills\"]\n",
        )
        .unwrap();

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

        let result = prepare_agent(&def);
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
