//! 一次会话启动的 Agent 契约准备：agent.toml 契约层、Agent 侧素材与声明的
//! store 技能**只解析一次**，工具上下文与系统提示词共用同一份结果。
//!
//! 以前 `build_tool_context` 与 `build_system_prompt_with_tools` 各自解析
//! 一遍（agent.toml + 项目层、每条声明的指令文件、agent.lock 与技能内容都被
//! 读两次），两次解析之间文件变化还会让同一轮 run 的两处口径互相矛盾
//! （TOCTOU）。现在三件事收敛到 [`prepare_agent`] 一处，两个组装函数只消费
//! [`PreparedAgent`]；`PreparedAgent` 是一次性快照，定稿后盘上文件再变也不
//! 影响本轮已备好的结果。

use std::fs;

use super::contract::{AgentMaterial, agent_material, collect_contract_layers};
use super::types::AgentDefinition;

/// 一次会话启动所需契约解析结果（[`prepare_agent`] 的产物）。
///
/// 工具上下文（env / 权限 / 沙箱 / 受信任读取根）与系统提示词都从这里取材；
/// 提示词策略（[`crate::agents::PromptScope`]）只决定**消费多少**，不再决定
/// **解析多少** —— 解析口径与工具上下文一致，一律 fail-closed。
#[derive(Debug, Clone)]
pub struct PreparedAgent {
    /// 契约层集合：agent 层（可能缺席）在前，项目层自低到高在后。
    layers: Vec<crate::av::Layer>,
    /// `layers` 中项目层的起点（agent 层缺席时为 0；存在时为 1）。
    project_start: usize,
    /// 契约层合并结果。
    merged: crate::av::Merged,
    /// Agent 侧素材（指令正文、记忆目录、技能来源目录）。
    material: AgentMaterial,
    /// 声明启用的 store 技能（`use` + 同层 `agent.lock` 的解析结果）。
    store_skills: Vec<crate::av::store::ResolvedSkill>,
}

impl PreparedAgent {
    /// 全部契约层（agent 层在前，项目层在后）—— env 合并与 `[resources]` 共用。
    pub fn layers(&self) -> &[crate::av::Layer] {
        &self.layers
    }

    /// 项目层（不含 agent 层）—— 项目侧声明（instructions / skills.sources）用。
    pub fn project_layers(&self) -> &[crate::av::Layer] {
        &self.layers[self.project_start..]
    }

    /// 契约层合并结果（准备阶段已预先合并并校验）。
    pub fn merged(&self) -> &crate::av::Merged {
        &self.merged
    }

    /// Agent 侧素材（指令正文、记忆目录、技能来源目录）。
    pub fn material(&self) -> &AgentMaterial {
        &self.material
    }

    /// 声明启用的 store 技能（含 store 内容目录与元信息）。
    pub fn store_skills(&self) -> &[crate::av::store::ResolvedSkill] {
        &self.store_skills
    }
}

/// 解析 Agent 的契约素材：契约层（agent.toml + 项目层发现）、Agent 侧素材
/// （`[resources]` 三项声明）、声明的 store 技能 —— 每次会话启动只解析一次。
///
/// fail-closed 与消费方原口径一致：缺契约 / 缺段 / 缺项、声明路径逃逸
/// Agent 目录、`agent.lock` 或 store 内容缺失，都在这里报错（后两者带
/// `av skill sync` 提示）。工具上下文本来就不受提示词策略影响，因此本函数
/// 不区分 `systemPrompt` 策略；策略只决定提示词消费哪些已备好的素材。
pub fn prepare_agent(def: &AgentDefinition) -> Result<PreparedAgent, String> {
    // 发现基准与 build_tool_context 同口径：工作目录存在时用规范路径（工具
    // 上下文始终以 canonical workspace 做发现）；尚未创建时退回原路径 ——
    // 向上发现只看已存在的目录，结果一致。
    let workspace = def
        .resolve_workspace()
        .map(|workspace| fs::canonicalize(&workspace).unwrap_or(workspace));
    let (agent_layer, project_layers) = collect_contract_layers(def, workspace.as_deref())?;

    let agent_dir = def.dir().ok_or("无法定位 Agent 目录")?;
    let material = agent_material(agent_layer.as_ref(), &agent_dir)?;

    let mut layers = Vec::with_capacity(project_layers.len() + 1);
    if let Some(layer) = agent_layer {
        layers.push(layer);
    }
    let project_start = layers.len();
    layers.extend(project_layers);

    let store_skills = load_declared_store_skills(&layers)?;
    let merged = crate::av::merge_layers(&layers)?;

    Ok(PreparedAgent {
        layers,
        project_start,
        merged,
        material,
        store_skills,
    })
}

/// 解析声明启用的 store 技能（av `[resources.skills].use` + 同层 `agent.lock`）。
///
/// 会话启动只读、不联网；缺锁/缺条目/缺内容一律 fail-closed，错误里带
/// `av skill sync` 提示（与 `av check`/`av doctor` 同一口径）。
fn load_declared_store_skills(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{
        DEFAULT_HARNESS_DIR, agent_dir, build_system_prompt_with_tools, build_tool_context,
        create_agent,
    };
    use pipi_test_support::HOME_LOCK;

    /// 一次准备、两处消费，且是同一份快照：解析完成后改盘上的指令文件，
    /// 本轮的工具上下文与系统提示词仍然用解析时的正文（不会各读各的）。
    #[test]
    fn one_preparation_serves_both_consumers_from_one_snapshot() {
        let _guard = HOME_LOCK.lock().unwrap();
        let name = format!("prepare-{}", pipi_protocol::new_id());
        let def = create_agent(&name, "", None, None, None, None).unwrap();
        let dir = agent_dir(&name).unwrap();
        let declared_instructions = dir.join(DEFAULT_HARNESS_DIR).join("AGENTS.md");

        let prepared = prepare_agent(&def).unwrap();
        fs::write(&declared_instructions, "SNAPSHOT-EDITED-AFTER-PREPARE\n").unwrap();

        let prompt = build_system_prompt_with_tools(&def, &prepared, &[]).unwrap();
        assert!(prompt.contains(&name), "已备好的指令正文仍在：{prompt}");
        assert!(
            !prompt.contains("SNAPSHOT-EDITED-AFTER-PREPARE"),
            "提示词消费的是准备时的快照，不该重读盘上文件"
        );

        // 工具上下文与提示词吃的是同一份准备结果：即使盘上契约已被改写，
        // 同一次准备仍能完成装配（口径一致）。
        let (ctx, resolved) =
            build_tool_context(&def, &prepared, None, pipi_protocol::AbortSignal::new()).unwrap();
        assert!(ctx.memory_dir.is_some());
        assert_eq!(
            resolved.vars.get("AV_AGENT").map(String::as_str),
            Some(name.as_str())
        );

        let _ = fs::remove_dir_all(dir);
    }

    /// 契约层顺序与字段口径：agent 层在前、项目层在后；`project_layers()`
    /// 只给项目层（提示词的项目侧声明按它取最高层）。
    #[test]
    fn layers_keep_agent_first_and_project_layers_separate() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = pipi_test_support::temp_dir("pipi-prepare-layers");
        let previous_home = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);

        let root = pipi_test_support::temp_dir("pipi-prepare-project");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("agent.toml"), "schema = 1\n").unwrap();

        let name = format!("prepare-layers-{}", pipi_protocol::new_id());
        let def = create_agent(&name, "", Some(root.to_str().unwrap()), None, None, None).unwrap();
        let prepared = prepare_agent(&def).unwrap();

        assert_eq!(prepared.layers().len(), 2);
        assert_eq!(
            prepared.layers()[0].kind,
            crate::av::LayerKind::Agent,
            "agent 层恒在最前"
        );
        assert_eq!(prepared.project_layers().len(), 1);
        assert_eq!(
            prepared.project_layers()[0].kind,
            crate::av::LayerKind::Project
        );
        assert_eq!(
            prepared.material().skill_sources.len(),
            1,
            "默认模板声明一个技能来源目录"
        );
        assert!(prepared.store_skills().is_empty(), "默认契约没有 use 声明");

        pipi_test_support::set_env_var("HOME", previous_home);
        let _ = fs::remove_dir_all(&home);
        let _ = fs::remove_dir_all(&root);
    }
}
