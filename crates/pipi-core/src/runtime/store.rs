//! 会话载体装配点。
//!
//! 运行时只依赖 `SessionLog` / `SessionRepo` 两个接口（见 `pipi_protocol::session`），
//! 具体载体在这里选。今天只有 JSONL 文件；接入第二种载体（如 git 分支）时
//! **只改本文件**，调用点不动 —— 设计见 `docs/git-session-store.md`。

use super::*;
use pipi_session::JsonlSessionRepo;

/// 该 Agent 的会话仓库。
pub(super) fn session_repo(def: &AgentDefinition) -> Result<Arc<dyn SessionRepo>, String> {
    let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
    Ok(Arc::new(JsonlSessionRepo::new(dir)))
}
