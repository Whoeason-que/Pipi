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
    agent_skills: Vec<av::skills::SkillMeta>,
    project_skills: Vec<av::skills::SkillMeta>,
) -> Vec<av::skills::SkillMeta> {
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
    layers: &[av::Layer],
) -> Result<Vec<av::store::ResolvedSkill>, String> {
    match av::store::winning_use(layers) {
        Some(declared) => av::store::resolve_declared_skills(declared.names, &declared.lock_path)
            .map_err(|e| format!("声明技能不可用（{}）：{e}", declared.layer.label)),
        None => Ok(Vec::new()),
    }
}

/// 把声明的 store 技能追加在约定目录之后（同名先发现者赢）。
fn merge_store_skills(
    mut skills: Vec<av::skills::SkillMeta>,
    store_skills: Vec<av::skills::SkillMeta>,
) -> Vec<av::skills::SkillMeta> {
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
fn harness_skill_metadata(skill: av::skills::SkillMeta) -> pipi_harness::SkillMetadata {
    pipi_harness::SkillMetadata {
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
        let merged = av::merge_layers(&layers)?;

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
            None => context_files.extend(pipi_harness::resources::load_agent_context_files(
                dir.as_deref(),
            )),
        }

        // —— instructions：项目层三态（最高声明层）——
        let project_budget = merged
            .resources
            .as_ref()
            .and_then(|resources| resources.max_bytes)
            .unwrap_or(pipi_harness::DEFAULT_PROJECT_DOC_MAX_BYTES);
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
                    av::find_project_root(workspace).unwrap_or_else(|| workspace.to_path_buf()),
                )
                .map_err(|e| format!("无法规范化项目根：{e}"))?;
                context_files.extend(load_declared_context_files(
                    list,
                    &base,
                    &containment,
                    Some(project_budget),
                )?);
            }
            None => context_files.extend(pipi_harness::resources::load_project_scope_files(
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
                        av::skills::load_skill_sources(&resolved, &containment)
                    }
                    None => av::skills::load_skill_sources(
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
                    av::find_project_root(workspace).unwrap_or_else(|| workspace.to_path_buf()),
                )
                .map_err(|e| format!("无法规范化项目根：{e}"))?;
                av::skills::load_skill_sources(&resolved, &containment)
            }
            None => {
                let skills_dir = workspace.join(".pi").join("skills");
                fs::canonicalize(workspace)
                    .map(|canonical| av::skills::load_skill_sources(&[skills_dir], &canonical))
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
        let filtered = av::skills::filter_skills_by_name(
            merge_store_skills(
                merge_skill_scopes(agent_skills, project_skills),
                declared_store.into_iter().map(|skill| skill.meta).collect(),
            ),
            skills_filter.as_ref().and_then(|(only, _)| *only),
            skills_filter.as_ref().and_then(|(_, exclude)| *exclude),
        )?;
        let skills: Vec<pipi_harness::SkillMetadata> =
            filtered.into_iter().map(harness_skill_metadata).collect();

        (
            workspace.display().to_string(),
            Some(pipi_agent::context::environment_context(
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
            None => context_files.extend(pipi_harness::resources::load_agent_context_files(
                dir.as_deref(),
            )),
        }
        // 无 workspace：仍可声明 agent 层契约里的 store 技能
        let agent_only_layers: Vec<av::Layer> = agent_layer.iter().cloned().collect();
        let skills: Vec<pipi_harness::SkillMetadata> =
            load_declared_store_skills(&agent_only_layers)?
                .into_iter()
                .map(|skill| harness_skill_metadata(skill.meta))
                .collect();
        (String::new(), None, skills)
    };

    Ok(pipi_harness::build_system_prompt(
        pipi_harness::BuildSystemPromptOptions {
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
