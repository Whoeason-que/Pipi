use std::fs;

use pipi_protocol::Tool;

use super::contract::{
    collect_contract_layers, layer_path_dir, load_declared_context_files, resolve_declared_sources,
};
use super::files::memory_index;
use super::types::AgentDefinition;

/// 组装系统提示：收集宿主资源，再交给 Pi-compatible harness 渲染。
pub fn build_system_prompt(def: &AgentDefinition) -> Result<String, String> {
    build_system_prompt_with_tools(def, &[])
}

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

/// 解析声明启用的 store 技能（av `[resources.skills].use` + 同层 `agent.lock`）。
///
/// 会话启动只读、不联网；缺锁/缺条目/缺内容一律 fail-closed，错误里带
/// `av skill sync` 提示（与 `av check`/`av doctor` 同一口径）。
pub(super) fn load_declared_store_skills(
    layers: &[crate::av::Layer],
) -> Result<Vec<crate::av::store::ResolvedSkill>, String> {
    match crate::av::store::winning_use(layers) {
        Some(declared) => {
            crate::av::store::resolve_declared_skills(declared.names, &declared.lock_path)
                .map_err(|e| format!("声明技能不可用（{}）：{e}", declared.layer.label))
        }
        None => Ok(Vec::new()),
    }
}

/// 把声明的 store 技能追加在约定目录之后（同名先发现者赢）。
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
/// 资源三态（av 契约 `[resources]`，缺席 = 约定发现）：
/// - instructions：Some(list) 完全替换该范围的发现并按声明顺序拼接，
///   Some([]) 显式禁用；注入顺序 agent 层在前、项目层在后；
/// - skills：sources 按层替换约定目录；only/exclude 按技能名 glob 过滤
///   合并集（exclude 优先）；frontmatter `disable-model-invocation`
///   仍是调用方式的唯一权威。
pub fn build_system_prompt_with_tools(
    def: &AgentDefinition,
    tools: &[Tool],
) -> Result<String, String> {
    let dir = def.dir();
    let workspace = def.resolve_workspace();
    let mut context_files = Vec::new();

    let (cwd, append_system_prompt, skills) = if let Some(workspace) = &workspace {
        let (agent_layer, project_layers) = collect_contract_layers(def, Some(workspace))?;
        let mut layers = Vec::new();
        if let Some(layer) = &agent_layer {
            layers.push(layer.clone());
        }
        layers.extend(project_layers.iter().cloned());
        let merged = crate::av::merge_layers(&layers)?;

        // —— instructions：agent 层三态 ——
        let agent_instructions = agent_layer.as_ref().and_then(|layer| {
            layer
                .config
                .resources
                .as_ref()
                .and_then(|resources| resources.instructions.clone())
        });
        match &agent_instructions {
            Some(list) => {
                let layer = agent_layer.as_ref().expect("instructions 存在");
                let base = layer_path_dir(layer);
                let containment =
                    fs::canonicalize(layer.path.parent().expect("契约文件必有父目录"))
                        .map_err(|e| format!("无法规范化 Agent 目录：{e}"))?;
                context_files.extend(load_declared_context_files(
                    list,
                    &base,
                    &containment,
                    None,
                )?);
            }
            None => context_files.extend(crate::prompt::resources::load_agent_context_files(
                dir.as_deref(),
            )),
        }

        // —— instructions：项目层三态（最高声明层）——
        let project_budget = merged
            .resources
            .as_ref()
            .and_then(|resources| resources.max_bytes)
            .unwrap_or(crate::prompt::DEFAULT_PROJECT_DOC_MAX_BYTES);
        match project_layers.iter().rev().find_map(|layer| {
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

        // —— skills：sources 三态 + only/exclude 过滤 ——
        let agent_skills = match &agent_layer {
            Some(layer) => {
                let declared_sources = layer
                    .config
                    .resources
                    .as_ref()
                    .and_then(|resources| resources.skills.as_ref())
                    .and_then(|skills| skills.sources.clone());
                let base = layer_path_dir(layer);
                let containment =
                    fs::canonicalize(layer.path.parent().expect("契约文件必有父目录"))
                        .map_err(|e| format!("无法规范化 Agent 目录：{e}"))?;
                match &declared_sources {
                    Some(sources) => {
                        let resolved = resolve_declared_sources(sources, &base);
                        crate::av::skills::load_skill_sources(&resolved, &containment)
                    }
                    None => crate::av::skills::load_skill_sources(
                        &[dir.as_ref().expect("agent 层存在则目录存在").join("skills")],
                        &containment,
                    ),
                }
            }
            None => Vec::new(),
        };
        let project_skills = match project_layers.iter().rev().find_map(|layer| {
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
        };
        let skills_filter = merged
            .resources
            .as_ref()
            .and_then(|resources| resources.skills.as_ref())
            .map(|skills| (skills.only.as_deref(), skills.exclude.as_deref()));
        // 声明的 store 技能（`use`）追加在约定目录之后，同名先发现者赢
        let declared_store = load_declared_store_skills(&layers)?;
        let filtered = crate::av::skills::filter_skills_by_name(
            merge_store_skills(
                merge_skill_scopes(agent_skills, project_skills),
                declared_store.into_iter().map(|skill| skill.meta).collect(),
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
        // 无 workspace：agent 层 instructions 三态仍生效（无项目层，技能沿用现状）
        let (agent_layer, _) = collect_contract_layers(def, None)?;
        match agent_layer.as_ref().and_then(|layer| {
            layer
                .config
                .resources
                .as_ref()
                .and_then(|resources| resources.instructions.clone())
        }) {
            Some(list) => {
                let layer = agent_layer.as_ref().expect("instructions 存在");
                let base = layer_path_dir(layer);
                let containment =
                    fs::canonicalize(layer.path.parent().expect("契约文件必有父目录"))
                        .map_err(|e| format!("无法规范化 Agent 目录：{e}"))?;
                context_files.extend(load_declared_context_files(
                    &list,
                    &base,
                    &containment,
                    None,
                )?);
            }
            None => context_files.extend(crate::prompt::resources::load_agent_context_files(
                dir.as_deref(),
            )),
        }
        // 无 workspace：仍可声明 agent 层契约里的 store 技能
        let agent_only_layers: Vec<crate::av::Layer> = agent_layer.iter().cloned().collect();
        let skills: Vec<crate::prompt::SkillMetadata> =
            load_declared_store_skills(&agent_only_layers)?
                .into_iter()
                .map(|skill| harness_skill_metadata(skill.meta))
                .collect();
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
            memory_files: memory_index(def),
            append_system_prompt,
            ..Default::default()
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentDefinition, agent_dir, build_tool_context, create_agent};
    use pipi_protocol::permissions::PermissionsConfig;
    use pipi_test_support::HOME_LOCK;

    #[test]
    fn system_prompt_includes_wire_tool_descriptions() {
        let def = AgentDefinition {
            name: "__pipi_harness_wire_test__".into(),
            description: String::new(),
            model: String::new(),
            provider: None,
            workspace: Some(
                std::env::temp_dir()
                    .join("pipi-venv-wire-test-does-not-write")
                    .to_string_lossy()
                    .into_owned(),
            ),
            permissions: PermissionsConfig {
                tools: vec!["read".into()],
                ..Default::default()
            },
            mcp_servers: Vec::new(),
            subagent: false,
            compact_threshold_percent: 75,
            compact_target_percent: None,
        };
        let tools = [pipi_protocol::Tool {
            name: "read".into(),
            description: "Read files".into(),
            parameters: serde_json::json!({}),
        }];

        let prompt = build_system_prompt_with_tools(&def, &tools).unwrap();

        assert!(prompt.contains("- read: Read files"));
    }

    /// av 契约端到端：项目层 agent.toml 的声明替换约定发现（instructions）、
    /// skills 按名过滤、env 解析注入 AV_*，以及包含约束 fail-closed。
    #[test]
    fn agent_contract_drives_instructions_skills_and_env() {
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

        let def = AgentDefinition {
            name: "__pipi_contract_e2e__".into(),
            description: String::new(),
            model: String::new(),
            provider: None,
            workspace: Some(root.to_string_lossy().into_owned()),
            permissions: PermissionsConfig {
                tools: vec!["read".into(), "bash".into()],
                ..Default::default()
            },
            mcp_servers: Vec::new(),
            subagent: false,
            compact_threshold_percent: 75,
            compact_target_percent: None,
        };

        // 声明替换约定发现；skills 按名过滤
        let prompt = build_system_prompt_with_tools(&def, &[]).unwrap();
        assert!(prompt.contains("DECLARED-PROMPT-BODY"));
        assert!(!prompt.contains("CONVENTION-AGENTS-ROOT-RULES"));
        assert!(prompt.contains("<name>wanted</name>"));
        assert!(!prompt.contains("noisy"));

        // env：声明 set + AV_* 注入 + 默认继承 all
        let (ctx, _) = build_tool_context(
            &def,
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
            Some("__pipi_contract_e2e__")
        );
        assert_eq!(
            ctx.resolved_env.get("AV_SESSION").map(String::as_str),
            Some("sess-1")
        );
        assert!(ctx.resolved_env.contains_key("PATH"));

        // 逃逸包含根 fail-closed
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
        let err = build_system_prompt_with_tools(&def, &[]).unwrap_err();
        assert!(err.contains("fail-closed"), "{err}");

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(outside);
    }

    /// 项目层契约可以用 `[resources.skills] sources` 指向仓库内的技能包目录
    /// （多仓库工作区用 `.agents/skills` 这种既有约定），不必改造成 `.pi/skills`。
    #[test]
    fn project_contract_skills_sources_point_into_the_repo() {
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

        let def = AgentDefinition {
            name: "__pipi_skills_sources__".into(),
            description: String::new(),
            model: String::new(),
            provider: None,
            workspace: Some(root.to_string_lossy().into_owned()),
            permissions: PermissionsConfig {
                tools: vec!["read".into(), "bash".into()],
                ..Default::default()
            },
            mcp_servers: Vec::new(),
            subagent: false,
            compact_threshold_percent: 75,
            compact_target_percent: None,
        };

        let prompt = build_system_prompt_with_tools(&def, &[]).unwrap();
        assert!(prompt.contains("<name>workspace-guide</name>"), "{prompt}");
        assert!(prompt.contains("repo skill pack"), "{prompt}");

        let _ = fs::remove_dir_all(root);
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

        let def = AgentDefinition {
            name: "__pipi_store_skills__".into(),
            description: String::new(),
            model: String::new(),
            provider: None,
            workspace: Some(root.to_string_lossy().into_owned()),
            permissions: PermissionsConfig {
                tools: vec!["read".into(), "bash".into()],
                ..Default::default()
            },
            mcp_servers: Vec::new(),
            subagent: false,
            compact_threshold_percent: 75,
            compact_target_percent: None,
        };

        // 3) 提示词注入 + 内容目录进受信任读取根
        let prompt = build_system_prompt_with_tools(&def, &[]).unwrap();
        assert!(prompt.contains("<name>pdf</name>"), "{prompt}");
        assert!(prompt.contains("STORE-DECLARED-PDF"), "{prompt}");
        let (ctx, _) = build_tool_context(&def, None, pipi_protocol::AbortSignal::new()).unwrap();
        let content_dir_name = crate::av::store::content_dir_name(&entry.content_hash).unwrap();
        assert!(
            ctx.read_roots.iter().any(|path| {
                path.file_name().and_then(|name| name.to_str()) == Some(content_dir_name)
            }),
            "store 技能目录应在 read_roots：{:?}",
            ctx.read_roots
        );

        // 4) store 内容缺失：两处路径都应 fail-closed 且带 av skill sync 提示
        crate::av::store::remove_version("pdf", &entry.content_hash).unwrap();
        let error = build_system_prompt_with_tools(&def, &[]).unwrap_err();
        assert!(error.contains("av skill sync"), "{error}");
        let error = build_tool_context(&def, None, pipi_protocol::AbortSignal::new())
            .err()
            .expect("内容缺失应 fail-closed");
        assert!(error.contains("av skill sync"), "{error}");

        // 5) 缺锁文件：fail-closed
        fs::remove_file(&lock_path).unwrap();
        let error = build_system_prompt_with_tools(&def, &[]).unwrap_err();
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
        let memory_dir = def.memory_dir().unwrap();
        fs::write(
            memory_dir.join("user-prefs.md"),
            "# 用户偏好\n\n简洁回答，中文优先。\n",
        )
        .unwrap();

        let prompt = build_system_prompt_with_tools(&def, &[]).unwrap();
        assert!(prompt.contains("<persistent_memory>"));
        assert!(prompt.contains("<path>"));
        assert!(prompt.contains(&memory_dir.join("user-prefs.md").display().to_string()));
        assert!(prompt.contains("用户偏好"));
        // 渐进披露：只注入摘要，不注入正文
        assert!(prompt.contains("简洁回答") || !prompt.contains("中文优先"));

        // 空 memory 时零打扰
        fs::remove_file(memory_dir.join("user-prefs.md")).unwrap();
        let prompt = build_system_prompt_with_tools(&def, &[]).unwrap();
        assert!(!prompt.contains("<persistent_memory>"));

        let _ = fs::remove_dir_all(agent_dir(&name).unwrap());
    }
}
