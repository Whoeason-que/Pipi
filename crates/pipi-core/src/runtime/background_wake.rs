//! 后台任务完成的推送式唤醒。
//!
//! 订阅 [`BackgroundTaskManager`] 的终态广播：**订阅开启**（默认）且没有被
//! 查询观测过的 `Completed` / `Failed` 任务，在一个短窗口内合并后写回会话 ——
//! 空闲会话追加 `background_result` 条目并直接起一轮，运行中的会话排进现有
//! 队列（当前轮结束后接着跑，不打断）。
//!
//! 刻意不唤醒的情况：`Terminated`（终止是当前会话自己的决定）、`Orphaned`
//! （重启恢复的旧记录）、订阅关闭（长驻进程）、已被 `query` 读到终态（避免
//! 模型刚等到结果又被推一条）、会话已归档/删除。
//!
//! `RuntimeState::new` 返回 `Self`，拿不到 `Arc`，所以宿主在 `Arc::new(..)`
//! 之后调用一次 [`RuntimeState::start_background_waker`]。

use std::collections::HashSet;
use std::sync::{Arc, Weak};
use std::time::Duration;

use pipi_protocol::{BackgroundTaskInfo, BackgroundTaskResult, BackgroundTaskStatus};
use tokio::sync::broadcast::error::RecvError;

use super::RuntimeState;

/// 合并窗口：窗口内完成的任务合成一轮，避免 N 个任务触发 N 轮。
const MERGE_WINDOW: Duration = Duration::from_millis(300);
/// 单条通知保留的输出尾部。
const OUTPUT_TAIL_BYTES: usize = 8 * 1024;

/// 一条待投递的通知：投递目标来自任务所有权，不属于通知本身。
struct PendingResult {
    agent_name: String,
    session_id: String,
    result: BackgroundTaskResult,
}

impl RuntimeState {
    /// 启动后台任务唤醒器。宿主在拿到 `Arc<RuntimeState>` 之后调用一次。
    pub fn start_background_waker(self: &Arc<Self>) {
        let mut receiver = self.background.subscribe();
        let weak = Arc::downgrade(self);
        self.spawn_run(async move {
            let mut delivered = HashSet::new();
            loop {
                let first = match receiver.recv().await {
                    Ok(info) => info,
                    Err(RecvError::Lagged(_)) => {
                        let mut missed = Vec::new();
                        reconcile(&weak, &delivered, &mut missed);
                        deliver(&weak, missed, &mut delivered);
                        continue;
                    }
                    Err(RecvError::Closed) => break,
                };
                let mut batch = Vec::new();
                collect(&weak, first, &delivered, &mut batch);
                // 固定截止时间：持续有事件也不能无限推迟投递。
                let deadline = tokio::time::Instant::now() + MERGE_WINDOW;
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => break,
                        event = receiver.recv() => match event {
                            Ok(info) => collect(&weak, info, &delivered, &mut batch),
                            Err(RecvError::Lagged(_)) => reconcile(&weak, &delivered, &mut batch),
                            Err(RecvError::Closed) => {
                                deliver(&weak, std::mem::take(&mut batch), &mut delivered);
                                return;
                            }
                        },
                    }
                }
                deliver(&weak, std::mem::take(&mut batch), &mut delivered);
            }
        });
    }
}

/// 哪些终态值得推送：订阅开启、失败或成功、且还没有被一次查询观测过。
/// `Terminated`（终止是当前会话自己的决定）与 `Orphaned`（重启恢复的旧记录）
/// 一律不推送。
fn is_wakeable(info: &BackgroundTaskInfo, observed: bool) -> bool {
    info.subscribe
        && !observed
        && matches!(
            info.status,
            BackgroundTaskStatus::Completed | BackgroundTaskStatus::Failed
        )
}

/// 过滤与排队：订阅、终态、未观测。
fn collect(
    weak: &Weak<RuntimeState>,
    info: BackgroundTaskInfo,
    delivered: &HashSet<String>,
    batch: &mut Vec<PendingResult>,
) {
    if delivered.contains(&info.job_id) || batch.iter().any(|p| p.result.job_id == info.job_id) {
        return;
    }
    let Some(runtime) = weak.upgrade() else {
        return;
    };
    // 模型可能正在（或刚刚）用 waitMs 查询并已看到终态：不再重复推。
    if !is_wakeable(&info, runtime.background.was_observed(&info.job_id)) {
        return;
    }
    let result = runtime
        .background
        .completion_result(&info, OUTPUT_TAIL_BYTES);
    batch.push(PendingResult {
        agent_name: info.agent_name,
        session_id: info.session_id,
        result,
    });
}

fn reconcile(
    weak: &Weak<RuntimeState>,
    delivered: &HashSet<String>,
    batch: &mut Vec<PendingResult>,
) {
    if let Some(runtime) = weak.upgrade() {
        for info in runtime.background.completion_candidates() {
            collect(weak, info, delivered, batch);
        }
    }
}

/// 按会话分组投递；每组只起一轮。
fn deliver(weak: &Weak<RuntimeState>, batch: Vec<PendingResult>, delivered: &mut HashSet<String>) {
    if batch.is_empty() {
        return;
    }
    let Some(runtime) = weak.upgrade() else {
        return;
    };
    // 窗口内可能刚好被一次查询读到终态，投递前再过滤一次。
    let batch: Vec<PendingResult> = batch
        .into_iter()
        .filter(|pending| !runtime.background.was_observed(&pending.result.job_id))
        .collect();
    let mut groups: Vec<((String, String), Vec<BackgroundTaskResult>)> = Vec::new();
    for pending in batch {
        let key = (pending.agent_name, pending.session_id);
        match groups.iter_mut().find(|(candidate, _)| *candidate == key) {
            Some((_, results)) => results.push(pending.result),
            None => groups.push((key, vec![pending.result])),
        }
    }
    for ((agent_name, session_id), results) in groups {
        let ids: Vec<String> = results.iter().map(|r| r.job_id.clone()).collect();
        // Reopen an idle released session if needed, then preserve its typed results
        // and pause state in the same queue as an already open session.
        match runtime.queue_background_results(&agent_name, &session_id, results) {
            Ok(()) => delivered.extend(ids),
            Err(error) => eprintln!(
                "pipi: failed to deliver background results to {agent_name}/{session_id}: {error}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::BackgroundTaskKind;

    fn info(status: BackgroundTaskStatus, subscribe: bool) -> BackgroundTaskInfo {
        BackgroundTaskInfo {
            job_id: "job-1".into(),
            kind: BackgroundTaskKind::Shell,
            agent_name: "agent".into(),
            session_id: "session".into(),
            run_id: 1,
            command: "cargo test".into(),
            cwd: "/tmp".into(),
            target_agent: None,
            child_session_id: None,
            status,
            started_at: 1,
            completed_at: Some(2),
            exit_code: Some(0),
            output_cursor: 0,
            output_truncated: false,
            subscribe,
            error: None,
            result: None,
        }
    }

    #[test]
    fn completion_window_keeps_more_than_eight_results() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let runtime = Arc::new(RuntimeState::new(rt.handle().clone()));
        let weak = Arc::downgrade(&runtime);
        let mut batch = Vec::new();
        let delivered = HashSet::new();
        for i in 0..20 {
            let mut task = info(BackgroundTaskStatus::Completed, true);
            task.job_id = format!("job-{i}");
            collect(&weak, task, &delivered, &mut batch);
        }
        assert_eq!(batch.len(), 20);
    }

    #[test]
    fn only_unobserved_subscribed_success_or_failure_wakes() {
        assert!(is_wakeable(
            &info(BackgroundTaskStatus::Completed, true),
            false
        ));
        assert!(is_wakeable(
            &info(BackgroundTaskStatus::Failed, true),
            false
        ));
        // 订阅关闭（长驻进程）
        assert!(!is_wakeable(
            &info(BackgroundTaskStatus::Completed, false),
            false
        ));
        // 已经被 waitMs 查询看到
        assert!(!is_wakeable(
            &info(BackgroundTaskStatus::Completed, true),
            true
        ));
        // 终止与重启孤儿都不唤醒
        assert!(!is_wakeable(
            &info(BackgroundTaskStatus::Terminated, true),
            false
        ));
        assert!(!is_wakeable(
            &info(BackgroundTaskStatus::Orphaned, true),
            false
        ));
        assert!(!is_wakeable(
            &info(BackgroundTaskStatus::Running, true),
            false
        ));
    }
}
