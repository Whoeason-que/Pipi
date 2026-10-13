//! 会话载体装配点。
//!
//! 运行时只依赖 `SessionLog` / `SessionRepo` 两个接口（见 `pipi_protocol::session`），
//! 具体载体在这里选：新建会话写 git 分支（`sessions.git`），旧 `sessions/*.jsonl`
//! 会话按 id 兼容（同一个会话只有一个载体），列表合并两区 —— 设计见
//! `docs/git-session-store.md`。**只改本文件**，调用点不动。

use super::*;
use pipi_session::AgentSessionRepo;

/// 该 Agent 的会话仓库。
pub(super) fn session_repo(def: &AgentDefinition) -> Result<Arc<dyn SessionRepo>, String> {
    Ok(Arc::new(agent_repo(def)?))
}

/// 同上，返回具体类型（导入等只属于组合载体的操作用）。
pub(crate) fn agent_repo(def: &AgentDefinition) -> Result<AgentSessionRepo, String> {
    let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
    let agent_dir = dir.parent().ok_or("无法解析 Agent 目录")?;
    Ok(AgentSessionRepo::new(agent_dir))
}
