//! 技能启用 / 禁用：编辑契约的 `use` 声明并维护解析锁。

use std::fs;

use crate::av::store::{self, SkillLock};

use crate::agents;

use crate::skills::agent_skills::{DeclaredContext, agent_skills_for, declared_context};
use crate::skills::model::AgentSkills;

/// 新建 agent.toml 时的骨架：与 `create_agent` 共用默认模板 ——
/// Agent 侧素材的路径必须显式声明，只写 `schema = 1` 拿不到可用的契约。
const NEW_CONTRACT_SKELETON: &str = crate::agents::DEFAULT_AGENT_CONTRACT;

/// 启用（写入 `use` + 解析锁）或禁用（移除声明与锁记录）一个 store 技能。
///
/// 编辑目标是当前生效的最高声明层；没有任何层声明 `use` 时写 Agent 层的
/// `agent.toml`（不存在则新建合法骨架）。启用要求技能已在 store 中安装；
/// 已有锁记录时不改写版本（锁是解析权威），缺记录才锁到 store 当前版本。
pub fn set_skill_enabled(
    agent_name: &str,
    name: &str,
    enabled: bool,
) -> Result<AgentSkills, String> {
    if !crate::av::skills::valid_skill_name(name) {
        return Err(format!("技能名 {name:?} 形状非法"));
    }
    let def = agents::load_agent(agent_name)?;
    let context = declared_context(&def)?;

    if enabled {
        // 先确认 store 里有内容，再动契约 —— 失败要早于任何写盘
        let index = store::read_index()?;
        let entry = store::latest_index_entry(&index, name)
            .ok_or_else(|| {
                format!("技能 {name:?} 尚未安装到全局 store；先安装再启用（装 ≠ 启用）")
            })?
            .clone();

        let text = contract_text_for_edit(&context)?;
        let (edited, _) =
            crate::av::contract_edit::edit_use_names(&text, &[name.to_string()], &[])?;
        store::write_text_atomic(&context.contract_path, &edited)?;

        let mut lock = SkillLock::read(&context.lock_path)?;
        lock.validate_unique_names()?;
        if lock.get(name).is_none() {
            lock.set_entry(entry);
        }
        lock.write(&context.lock_path)?;
    } else {
        if context.contract_path.is_file() {
            let text = fs::read_to_string(&context.contract_path)
                .map_err(|e| format!("无法读取 {}: {e}", context.contract_path.display()))?;
            let (edited, changed) =
                crate::av::contract_edit::edit_use_names(&text, &[], &[name.to_string()])?;
            if changed {
                store::write_text_atomic(&context.contract_path, &edited)?;
            }
        }
        if context.lock_path.is_file() {
            let mut lock = SkillLock::read(&context.lock_path)?;
            if lock.remove(name) {
                if lock.skills.is_empty() {
                    fs::remove_file(&context.lock_path)
                        .map_err(|e| format!("无法删除 {}: {e}", context.lock_path.display()))?;
                } else {
                    lock.write(&context.lock_path)?;
                }
            }
        }
    }
    agent_skills_for(&def)
}

/// 取编辑目标契约的文本；不存在且目标就是 Agent 层时用合法骨架（尚未落盘，
/// 编辑成功后才写）。
fn contract_text_for_edit(context: &DeclaredContext) -> Result<String, String> {
    if context.contract_path.is_file() {
        return fs::read_to_string(&context.contract_path)
            .map_err(|e| format!("无法读取 {}: {e}", context.contract_path.display()));
    }
    if context.contract_path != context.agent_contract_path {
        // 声明层文件刚被删掉之类的竞态：明确报错，不凭空重建项目文件
        return Err(format!(
            "契约文件不存在：{}（项目层文件不会被自动创建）",
            context.contract_path.display()
        ));
    }
    Ok(NEW_CONTRACT_SKELETON.to_string())
}
