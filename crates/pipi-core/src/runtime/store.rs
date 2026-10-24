//! 会话载体装配点。
//!
//! 运行时只依赖 `SessionLog` / `SessionRepo` 两个接口（见 `pipi_protocol::session`），
//! 具体载体在这里构造：git 分支（Agent 目录下的 `sessions.git` 影子仓库，
//! 一轮一个 commit）—— 设计见 `docs/git-session-store.md`。**只改本文件**，
//! 调用点不动。

use super::*;
use pipi_session::GitSessionRepo;

/// 该 Agent 的会话仓库。
pub(super) fn session_repo(def: &AgentDefinition) -> Result<Arc<dyn SessionRepo>, String> {
    Ok(Arc::new(agent_repo(def)?))
}

/// 同上，返回具体类型（导入等直接操作载体的调用点用）。
pub(crate) fn agent_repo(def: &AgentDefinition) -> Result<GitSessionRepo, String> {
    let dir = def
        .sessions_dir()
        .ok_or("Cannot resolve session directory")?;
    let agent_dir = dir.parent().ok_or("Cannot resolve agent directory")?;
    Ok(GitSessionRepo::new(agent_dir.join("sessions.git")))
}
