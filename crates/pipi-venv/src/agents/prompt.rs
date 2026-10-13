use std::fs;

use pipi_protocol::Tool;

use super::contract::{layer_path_dir, load_declared_context_files, resolve_declared_sources};
use super::files::memory_index;
use super::preparation::PreparedAgent;
use super::types::{AgentDefinition, PromptScope};

/// 跨层合并技能（agent 层在前，同名先发现者赢）。
fn merge_skill_scopes(
    agent_skills: Vec<crate::av::skills::SkillMeta>,
    project_skills: Vec<crate::av::skills::SkillMeta>,
) -> Vec<crate::av::skills::SkillMeta> {
    let mut seen_names = std::collections::HashSet::new();
    agent_skills
        .into_iter()
        .chain(project_skills)
        .filter(|skill| seen_names.insert(skill.name.clone()))
        .collect()
}

/// 把声明的 store 技能追加在约定目录之后（同名先发现者赢）。
///
/// 声明技能的解析（`use` + `agent.lock`，只读、不联网、缺失即 fail-closed）
/// 已收敛到 `preparation::prepare_agent` 一处（工具上下文与提示词共用），
/// 这里只保留提示词侧的合并与过滤。
fn merge_store_skills(
    mut skills: Vec<crate::av::skills::SkillMeta>,
    store_skills: Vec<crate::av::skills::SkillMeta>,
) -> Vec<crate::av::skills::SkillMeta> {
    let mut seen_names: std::collections::HashSet<String> =
        skills.iter().map(|skill| skill.name.clone()).collect();
    for skill in store_skills {
        if seen_names.insert(skill.name.clone()) {
            skills.push(skill);
        }
    }
    skills
}

/// pipi-core 技能元信息 → harness 渲染所需的元信息。
fn harness_skill_metadata(skill: crate::av::skills::SkillMeta) -> crate::prompt::SkillMetadata {
    crate::prompt::SkillMetadata {
        name: skill.name,
        description: skill.description,
        disable_model_invocation: skill.disable_model_invocation,
        path: skill.path.display().to_string(),
    }
}

/// 组装带 wire tool 描述的系统提示。
///
/// 契约层、Agent 侧素材与声明的 store 技能由调用方先经
/// [`super::preparation::prepare_agent`] 备好（`prepared`，每轮 run 只解析
/// 一次，与工具上下文共用）；本函数只按策略消费，不再碰盘上的契约文件。
///
/// 取材范围由 `agent.json` 的 `systemPrompt` 策略决定（[`PromptScope`]）：
/// `both` 取 Agent 侧 + 工作区侧，`agent` 只取 `harness/`，`workspace` 只取
/// 项目层，`none` 直接返回空串（调用方不发 system 消息）。策略**只影响提示词**：
/// env、权限、沙箱与受信任读取根由 `build_tool_context` 独立解析，与它无关，
/// 所以 `workspace` 策略的 Agent 照样能用 memory 工具读写 `harness/memory`。
///
/// `harness/` 缺席不报错（约定发现没有文件就是没有上下文）；只有 av 契约里
/// **显式声明**的路径缺失才 fail-closed（在准备阶段报错，见 `prepare_agent`）。
///
/// 资源三态（av 契约 `[resources]`，缺席 = 约定发现）：
/// - instructions：Some(list) 完全替换该范围的发现并按声明顺序拼接，
///   Some([]) 显式禁用；注入顺序 agent 层在前、项目层在后；
/// - skills：sources 按层替换约定目录；only/exclude 按技能名 glob 过滤
///   合并集（exclude 优先）；frontmatter `disable-model-invocation`
///   仍是调用方式的唯一权威。
pub fn build_system_prompt_with_tools(
    def: &AgentDefinition,
    prepared: &PreparedAgent,
    tools: &[Tool],
) -> Result<String, String> {
    let scope = def.system_prompt;
    if scope == PromptScope::None {
        return Ok(String::new());
    }
    let dir = def.dir();
    let workspace = def.resolve_workspace();
    // 契约层、Agent 侧素材与声明的 store 技能都来自 prepare_agent 的同一份
    // 准备结果（错/缺在准备阶段已 fail-closed，这里只按策略取用）。
    let merged = crate::av::merge_layers(prepared.layers())?;

    // Agent 侧素材只有策略含 agent 侧时才消费；准备阶段不区分策略。
    let material = if scope.uses_agent_side() {
        Some(prepared.material())
    } else {
        None
    };
    let mut context_files = Vec::new();
    if let Some(material) = &material {
        context_files.extend(material.instructions.iter().map(|instruction| {
            crate::prompt::ContextFile {
                path: instruction.path.display().to_string(),
                content: instruction.content.clone(),
            }
        }));
    }

    let (cwd, append_system_prompt, skills) = if let Some(workspace) = &workspace {
        // —— instructions：项目层三态（最高声明层，仅工作区侧参与时）——

        // —— instructions：项目层三态（最高声明层，仅工作区侧参与时）——
        let project_budget = merged
            .resources
            .as_ref()
            .and_then(|resources| resources.max_bytes)
            .unwrap_or(crate::prompt::DEFAULT_PROJECT_DOC_MAX_BYTES);
        if scope.uses_workspace_side() {
            match prepared.project_layers().iter().rev().find_map(|layer| {
                layer
                    .config
                    .resources
                    .as_ref()
                    .and_then(|resources| resources.instructions.as_ref())
                    .map(|list| (list, layer))
            }) {
                Some((list, layer)) => {
                    let base = layer_path_dir(layer);
                    let containment = fs::canonicalize(
                        crate::av::find_project_root(workspace)
                            .unwrap_or_else(|| workspace.to_path_buf()),
                    )
                    .map_err(|e| format!("无法规范化项目根：{e}"))?;
                    context_files.extend(load_declared_context_files(
                        list,
                        &base,
                        &containment,
                        Some(project_budget),
                    )?);
                }
                None => context_files.extend(crate::prompt::resources::load_project_scope_files(
                    workspace,
                    project_budget,
                )),
            }
        }

        // —— skills：来源由契约声明 + only/exclude 过滤 ——
        let agent_skills = match &material {
            Some(material) => {
                let agent_dir = dir.as_deref().ok_or("无法定位 Agent 目录")?;
                let containment = fs::canonicalize(agent_dir)
                    .map_err(|e| format!("无法规范化 Agent 目录：{e}"))?;
                crate::av::skills::load_skill_sources(&material.skill_sources, &containment)
            }
            None => Vec::new(),
        };
        let project_skills = if !scope.uses_workspace_side() {
            Vec::new()
        } else {
            match prepared.project_layers().iter().rev().find_map(|layer| {
                layer
                    .config
                    .resources
                    .as_ref()
                    .and_then(|resources| resources.skills.as_ref())
                    .and_then(|skills| skills.sources.as_ref())
                    .map(|sources| (sources, layer))
            }) {
                Some((sources, layer)) => {
                    let base = layer_path_dir(layer);
                    let resolved = resolve_declared_sources(sources, &base);
                    let containment = fs::canonicalize(
                        crate::av::find_project_root(workspace)
                            .unwrap_or_else(|| workspace.to_path_buf()),
                    )
                    .map_err(|e| format!("无法规范化项目根：{e}"))?;
                    crate::av::skills::load_skill_sources(&resolved, &containment)
                }
                None => {
                    let skills_dir = workspace.join(".pi").join("skills");
                    fs::canonicalize(workspace)
                        .map(|canonical| {
                            crate::av::skills::load_skill_sources(&[skills_dir], &canonical)
                        })
                        .unwrap_or_default()
                }
            }
        };
        let skills_filter = merged
            .resources
            .as_ref()
            .and_then(|resources| resources.skills.as_ref())
            .map(|skills| (skills.only.as_deref(), skills.exclude.as_deref()));
        // 声明的 store 技能（`use`）追加在约定目录之后，同名先发现者赢
        let declared_store = if scope.uses_agent_side() {
            prepared.store_skills()
        } else {
            &[]
        };
        let filtered = crate::av::skills::filter_skills_by_name(
            merge_store_skills(
                merge_skill_scopes(agent_skills, project_skills),
                declared_store
                    .iter()
                    .map(|skill| skill.meta.clone())
                    .collect(),
            ),
            skills_filter.as_ref().and_then(|(only, _)| *only),
            skills_filter.as_ref().and_then(|(_, exclude)| *exclude),
        )?;
        let skills: Vec<crate::prompt::SkillMetadata> =
            filtered.into_iter().map(harness_skill_metadata).collect();

        (
            workspace.display().to_string(),
            Some(crate::prompt::environment_context(
                workspace,
                &def.permissions.sandbox,
            )),
            skills,
        )
    } else {
        // 无 workspace：只有 agent 侧成立（项目层不存在）。指令已在上面按契约
        // 注入，这里只剩声明的 store 技能索引（准备阶段按同一份层集合解析）。
        let mut skills = Vec::new();
        if scope.uses_agent_side() {
            skills = prepared
                .store_skills()
                .iter()
                .map(|skill| harness_skill_metadata(skill.meta.clone()))
                .collect();
        }
        (String::new(), None, skills)
    };

    Ok(crate::prompt::build_system_prompt(
        crate::prompt::BuildSystemPromptOptions {
            selected_tools: Some(def.permissions.tools.clone()),
            tool_snippets: tools
                .iter()
                .map(|tool| (tool.name.clone(), tool.description.clone()))
                .collect(),
            cwd,
            context_files,
            skills,
            // 记忆索引属于 Agent 侧素材：策略不含 agent 侧时不注入
            memory_files: match &material {
                Some(material) => memory_index(&material.memory_dir),
                None => Vec::new(),
            },
            append_system_prompt,
            ..Default::default()
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{
        AgentDefinition, DEFAULT_HARNESS_DIR, agent_dir, build_tool_context, create_agent,
        prepare_agent,
    };

    /// 默认模板声明的素材目录（`<Agent 目录>/harness`）。
    fn default_harness(agent_dir: &std::path::Path) -> std::path::PathBuf {
        agent_dir.join(DEFAULT_HARNESS_DIR)
    }

    /// 建一个真实 Agent（`create_agent` 会写默认契约与 harness 骨架）。
    /// 调用方必须先持有 HOME_LOCK 并把 HOME 指到临时目录。
    fn make_agent(workspace: Option<&str>) -> AgentDefinition {
        create_agent(
            &format!("prompt-{}", pipi_protocol::new_id()),
            "",
            workspace,
            None,
            None,
            None,
        )
        .unwrap()
    }
    use pipi_protocol::permissions::PermissionsConfig;
    use pipi_test_support::HOME_LOCK;

    #[test]
    fn system_prompt_includes_wire_tool_descriptions() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("pipi-wire-{}", pipi_protocol::new_id()));
        fs::create_dir_all(&home).unwrap();
        let previous_home = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);

        let def = make_agent(None);
        let tools = [pipi_protocol::Tool {
            name: "read".into(),
            description: "Read files".into(),
            parameters: serde_json::json!({}),
        }];

        let prepared = prepare_agent(&def).unwrap();
        let prompt = build_system_prompt_with_tools(&def, &prepared, &tools).unwrap();

        assert!(prompt.contains("- read: Read files"));

        pipi_test_support::set_env_var("HOME", previous_home);
        let _ = fs::remove_dir_all(&home);
    }

    /// av 契约端到端：项目层 agent.toml 的声明替换约定发现（instructions）、
    /// skills 按名过滤、env 解析注入 AV_*，以及包含约束 fail-closed。
    #[test]
    fn agent_contract_drives_instructions_skills_and_env() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("pipi-e2e-home-{}", pipi_protocol::new_id()));
        fs::create_dir_all(&home).unwrap();
        let previous_home = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);

        let root =
            std::env::temp_dir().join(format!("pipi-contract-e2e-{}", pipi_protocol::new_id()));
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("AGENTS.md"), "CONVENTION-AGENTS-ROOT-RULES").unwrap();
        fs::write(root.join("PROMPT.md"), "DECLARED-PROMPT-BODY").unwrap();

        let skills_dir = root.join(".pi").join("skills");
        fs::create_dir_all(skills_dir.join("wanted")).unwrap();
        fs::write(
            skills_dir.join("wanted").join("SKILL.md"),
            "---\nname: wanted\ndescription: wanted skill\n---\nbody",
        )
        .unwrap();
        fs::create_dir_all(skills_dir.join("noisy")).unwrap();
        fs::write(
            skills_dir.join("noisy").join("SKILL.md"),
            "---\nname: noisy\ndescription: noisy skill\n---\nbody",
        )
        .unwrap();

        fs::write(
            root.join("agent.toml"),
            r#"
    schema = 1

    [env]
    set = { CONTRACT_E2E_MARKER = "on" }

    [resources]
    instructions = ["PROMPT.md"]

    [resources.skills]
    only = ["wanted"]
    "#,
        )
        .unwrap();

        let name = format!("contract-e2e-{}", pipi_protocol::new_id());
        let def = create_agent(
            &name,
            "",
            Some(root.to_str().unwrap()),
            Some(PermissionsConfig {
                tools: vec!["read".into(), "bash".into()],
                ..Default::default()
            }),
            None,
            None,
        )
        .unwrap();

        // 声明替换约定发现；skills 按名过滤
        let prepared = prepare_agent(&def).unwrap();
        let prompt = build_system_prompt_with_tools(&def, &prepared, &[]).unwrap();
        assert!(prompt.contains("DECLARED-PROMPT-BODY"));
        assert!(!prompt.contains("CONVENTION-AGENTS-ROOT-RULES"));
        assert!(prompt.contains("<name>wanted</name>"));
        assert!(!prompt.contains("noisy"));

        // env：声明 set + AV_* 注入 + 默认继承 all
        let (ctx, _) = build_tool_context(
            &def,
            &prepared,
            Some("sess-1".into()),
            pipi_protocol::AbortSignal::new(),
        )
        .unwrap();
        assert_eq!(
            ctx.resolved_env
                .get("CONTRACT_E2E_MARKER")
                .map(String::as_str),
            Some("on")
        );
        assert_eq!(
            ctx.resolved_env.get("AV_AGENT").map(String::as_str),
            Some(name.as_str())
        );
        assert_eq!(
            ctx.resolved_env.get("AV_SESSION").map(String::as_str),
            Some("sess-1")
        );
        assert!(ctx.resolved_env.contains_key("PATH"));

        // 逃逸包含根 fail-closed（项目层声明的指令文件在组装提示词时加载，
        // 与 Agent 侧素材一样 fail-closed）
        let outside = std::env::temp_dir().join(format!(
            "pipi-contract-e2e-outside-{}.md",
            pipi_protocol::new_id()
        ));
        fs::write(&outside, "outside").unwrap();
        fs::write(
            root.join("agent.toml"),
            format!(
                "schema = 1\n\n[resources]\ninstructions = [\"../{}\"]\n",
                outside.file_name().unwrap().to_string_lossy()
            ),
        )
        .unwrap();
        let prepared = prepare_agent(&def).unwrap();
        let err = build_system_prompt_with_tools(&def, &prepared, &[]).unwrap_err();
        assert!(err.contains("fail-closed"), "{err}");

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(outside);
        pipi_test_support::set_env_var("HOME", previous_home);
        let _ = fs::remove_dir_all(&home);
    }

    /// 项目层契约可以用 `[resources.skills] sources` 指向仓库内的技能包目录
    /// （多仓库工作区用 `.agents/skills` 这种既有约定），不必改造成 `.pi/skills`。
    #[test]
    fn project_contract_skills_sources_point_into_the_repo() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home =
            std::env::temp_dir().join(format!("pipi-sources-home-{}", pipi_protocol::new_id()));
        fs::create_dir_all(&home).unwrap();
        let previous_home = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);

        let root =
            std::env::temp_dir().join(format!("pipi-venv-sources-{}", pipi_protocol::new_id()));
        fs::create_dir_all(root.join(".git")).unwrap();
        let pack = root.join(".agents").join("skills").join("workspace-guide");
        fs::create_dir_all(&pack).unwrap();
        fs::write(
            pack.join("SKILL.md"),
            "---\nname: workspace-guide\ndescription: repo skill pack\n---\nbody",
        )
        .unwrap();
        fs::write(
            root.join("agent.toml"),
            "schema = 1\n\n[resources.skills]\nsources = [\".agents/skills\"]\n",
        )
        .unwrap();

        let name = format!("skill-sources-{}", pipi_protocol::new_id());
        let def = create_agent(
            &name,
            "",
            Some(root.to_str().unwrap()),
            Some(PermissionsConfig {
                tools: vec!["read".into(), "bash".into()],
                ..Default::default()
            }),
            None,
            None,
        )
        .unwrap();

        let prepared = prepare_agent(&def).unwrap();
        let prompt = build_system_prompt_with_tools(&def, &prepared, &[]).unwrap();
        assert!(prompt.contains("<name>workspace-guide</name>"), "{prompt}");
        assert!(prompt.contains("repo skill pack"), "{prompt}");

        let _ = fs::remove_dir_all(root);
        pipi_test_support::set_env_var("HOME", previous_home);
        let _ = fs::remove_dir_all(&home);
    }

    /// 声明启用的 store 技能（av `[resources.skills].use` + `agent.lock`）：
    /// 安装后进入系统提示与 read_roots；锁/内容缺失即 fail-closed 并提示 `av skill sync`。
    #[test]
    fn declared_store_skills_are_injected_and_missing_fails_closed() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!(
            "pipi-store-skills-home-{}",
            pipi_protocol::new_id()
        ));
        fs::create_dir_all(&home).unwrap();
        let previous_home = std::env::var("HOME").unwrap();
        let previous_av = std::env::var("AV_HOME").ok();
        pipi_test_support::set_env_var("HOME", &home);
        // 避免外部覆盖影响 store 位置（av 家目录 = $AV_HOME 或 ~/.av）
        pipi_test_support::remove_env_var("AV_HOME");

        // 1) 真实安装管线装进 store（与 `av skill install` 同一实现）
        let source = home.join("source").join("pdf");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: pdf\ndescription: STORE-DECLARED-PDF\n---\nbody",
        )
        .unwrap();
        let spec = crate::av::sources::SourceSpec::Local {
            path: fs::canonicalize(&source).unwrap(),
        };
        let report = crate::av::install::install_source(
            &spec,
            &crate::av::install::Selection::All,
            false,
            &mut |_| {},
        )
        .unwrap();
        let entry = report.installed[0].entry.clone();

        // 2) 项目契约：声明 use + 同层 agent.lock
        let root = home.join("project");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(
            root.join("agent.toml"),
            "schema = 1\n\n[resources.skills]\nuse = [\"pdf\"]\n",
        )
        .unwrap();
        let mut lock = crate::av::store::SkillLock::empty();
        lock.set_entry(entry.clone());
        let lock_path = root.join("agent.lock");
        lock.write(&lock_path).unwrap();

        let name = format!("store-skills-{}", pipi_protocol::new_id());
        let def = create_agent(
            &name,
            "",
            Some(root.to_str().unwrap()),
            Some(PermissionsConfig {
                tools: vec!["read".into(), "bash".into()],
                ..Default::default()
            }),
            None,
            None,
        )
        .unwrap();

        // 3) 提示词注入 + 内容目录进受信任读取根（同一次准备喂两个消费方）
        let prepared = prepare_agent(&def).unwrap();
        let prompt = build_system_prompt_with_tools(&def, &prepared, &[]).unwrap();
        assert!(prompt.contains("<name>pdf</name>"), "{prompt}");
        assert!(prompt.contains("STORE-DECLARED-PDF"), "{prompt}");
        let (ctx, _) =
            build_tool_context(&def, &prepared, None, pipi_protocol::AbortSignal::new()).unwrap();
        let content_dir_name = crate::av::store::content_dir_name(&entry.content_hash).unwrap();
        assert!(
            ctx.read_roots.iter().any(|path| {
                path.file_name().and_then(|name| name.to_str()) == Some(content_dir_name)
            }),
            "store 技能目录应在 read_roots：{:?}",
            ctx.read_roots
        );

        // 4) store 内容缺失：准备阶段即 fail-closed（工具上下文与提示词共用
        //    这一处解析，任一消费方都拿不到缺失的内容），带 av skill sync 提示
        crate::av::store::remove_version("pdf", &entry.content_hash).unwrap();
        let error = prepare_agent(&def).unwrap_err();
        assert!(error.contains("av skill sync"), "{error}");

        // 5) 缺锁文件：fail-closed
        fs::remove_file(&lock_path).unwrap();
        let error = prepare_agent(&def).unwrap_err();
        assert!(error.contains("agent.lock"), "{error}");

        pipi_test_support::set_env_var("HOME", previous_home);
        match previous_av {
            Some(value) => pipi_test_support::set_env_var("AV_HOME", value),
            None => pipi_test_support::remove_env_var("AV_HOME"),
        }
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn system_prompt_injects_memory_index_with_summaries() {
        let _guard = HOME_LOCK.lock().unwrap();
        let name = format!("memory-index-{}", pipi_protocol::new_id());
        let def = create_agent(&name, "", None, None, None, None).unwrap();
        let memory_dir = default_harness(&agent_dir(&name).unwrap()).join("memory");
        fs::write(
            memory_dir.join("user-prefs.md"),
            "# 用户偏好\n\n简洁回答，中文优先。\n",
        )
        .unwrap();

        let prepared = prepare_agent(&def).unwrap();
        let prompt = build_system_prompt_with_tools(&def, &prepared, &[]).unwrap();
        assert!(prompt.contains("<persistent_memory>"));
        assert!(prompt.contains("<path>"));
        assert!(prompt.contains(&memory_dir.join("user-prefs.md").display().to_string()));
        assert!(prompt.contains("用户偏好"));
        // 渐进披露：只注入摘要，不注入正文
        assert!(prompt.contains("简洁回答") || !prompt.contains("中文优先"));

        // 空 memory 时零打扰
        fs::remove_file(memory_dir.join("user-prefs.md")).unwrap();
        let prepared = prepare_agent(&def).unwrap();
        let prompt = build_system_prompt_with_tools(&def, &prepared, &[]).unwrap();
        assert!(!prompt.contains("<persistent_memory>"));

        let _ = fs::remove_dir_all(agent_dir(&name).unwrap());
    }

    /// 策略矩阵：`systemPrompt` 决定 Agent 侧（harness/）与工作区侧各自是否进提示词。
    /// 环境块只看有没有工作区，与策略无关；`none` 一律空串（调用方不发 system 消息）。
    #[test]
    fn prompt_scope_gates_agent_and_workspace_sides() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("pipi-scope-{}", pipi_protocol::new_id()));
        fs::create_dir_all(&home).unwrap();
        let previous_home = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);

        let project = home.join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("AGENTS.md"), "WORKSPACE-INSTRUCTIONS").unwrap();

        let name = format!("scope-{}", pipi_protocol::new_id());
        let def =
            create_agent(&name, "", Some(project.to_str().unwrap()), None, None, None).unwrap();
        let harness = default_harness(&agent_dir(&name).unwrap());
        fs::write(harness.join("AGENTS.md"), "HARNESS-INSTRUCTIONS").unwrap();
        fs::create_dir_all(harness.join("skills/demo")).unwrap();
        fs::write(
            harness.join("skills/demo/SKILL.md"),
            "---\nname: demo\ndescription: harness skill\n---\nbody",
        )
        .unwrap();
        fs::write(harness.join("memory/notes.md"), "# 记忆标题\n\n正文\n").unwrap();

        for (scope, agent_side, workspace_side) in [
            (PromptScope::Both, true, true),
            (PromptScope::Agent, true, false),
            (PromptScope::Workspace, false, true),
        ] {
            let mut scoped = def.clone();
            scoped.system_prompt = scope;
            let prepared = prepare_agent(&scoped).unwrap();
            let prompt = build_system_prompt_with_tools(&scoped, &prepared, &[]).unwrap();
            assert_eq!(
                prompt.contains("HARNESS-INSTRUCTIONS"),
                agent_side,
                "{scope:?}"
            );
            assert_eq!(
                prompt.contains("WORKSPACE-INSTRUCTIONS"),
                workspace_side,
                "{scope:?}"
            );
            assert_eq!(
                prompt.contains("<available_skills>"),
                agent_side,
                "{scope:?}"
            );
            assert_eq!(
                prompt.contains("<persistent_memory>"),
                agent_side,
                "{scope:?}"
            );
            // 两侧各贡献一个上下文文件（渲染层用同一个 <project_instructions> 标签）
            assert_eq!(
                prompt.matches("<project_instructions").count(),
                usize::from(agent_side) + usize::from(workspace_side),
                "{scope:?}"
            );
            assert!(prompt.contains("<environment_context>"), "{scope:?}");
            assert!(!prompt.is_empty(), "{scope:?}");
        }

        let mut none = def.clone();
        none.system_prompt = PromptScope::None;
        let prepared = prepare_agent(&none).unwrap();
        assert!(
            build_system_prompt_with_tools(&none, &prepared, &[])
                .unwrap()
                .is_empty()
        );

        let _ = fs::remove_dir_all(&home);
        pipi_test_support::set_env_var("HOME", previous_home);
    }

    /// 声明的素材文件缺失即 fail-closed，且不因 `systemPrompt` 策略放行：
    /// 解析口径与工具上下文一致（env、权限、沙箱与读取根本来就不受策略影响），
    /// 策略只决定提示词消费哪些已备好的素材。
    #[test]
    fn missing_declared_material_fails_closed_for_every_scope() {
        let _guard = HOME_LOCK.lock().unwrap();
        let name = format!("no-harness-{}", pipi_protocol::new_id());
        let def = create_agent(&name, "", None, None, None, None).unwrap();
        let dir = agent_dir(&name).unwrap();
        fs::remove_file(default_harness(&dir).join("AGENTS.md")).unwrap();

        for scope in [
            PromptScope::Both,
            PromptScope::Agent,
            PromptScope::Workspace,
            PromptScope::None,
        ] {
            let mut scoped = def.clone();
            scoped.system_prompt = scope;
            let error = prepare_agent(&scoped).expect_err("声明的指令文件缺失应 fail-closed");
            assert!(error.contains("上下文文件不可用"), "{scope:?}: {error}");
        }

        let _ = fs::remove_dir_all(dir);
    }
}
