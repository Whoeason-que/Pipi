//! 提交点：把缓冲的条目固化到载体（git 分支一轮一个 commit）。
//!
//! 时机由内核决定，规则见 `docs/git-session-store.md` §4：
//!
//! - **轮末**（`harvest` 返回后、压缩之前、运行守卫释放之前）—— 提交失败不
//!   失败本轮：条目留在缓冲里，随下一轮一起提交；
//! - **轮外**（会话创建、env 记账、切模型、异常终态尾部修复）—— 一次性路径，
//!   在 `state` / `run` 里直接提交（几毫秒，与同路径上的同步文件 IO 同量级）；
//! - **压缩条目**（分叉的新载体、原地追加）—— 见 `compaction`，必须在换账本
//!   之前完成。
//!
//! 跑 git 是阻塞调用（spawn 进程），所以轮末这类热路径在阻塞线程池上跑；
//! 一次性路径直接调用。

use super::*;

/// 提交当前会话的账本（阻塞线程池版；轮末与超时收尾用）。
pub(super) async fn commit_log(session: &SessionHandle) -> Result<(), String> {
    let writer = session.writer.clone();
    tokio::task::spawn_blocking(move || commit_shared(&writer))
        .await
        .map_err(|error| format!("Failed to commit session: {error}"))?
}

/// 提交一份账本（同步版；一次性路径直接调用）。
pub(super) fn commit_shared(writer: &SharedLog) -> Result<(), String> {
    writer
        .lock()
        .map_err(|error| format!("Cannot lock session writer: {error}"))?
        .commit_point()
        .map_err(|error| format!("Failed to commit session: {error}"))
}
