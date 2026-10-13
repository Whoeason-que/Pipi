//! 后台任务测试夹具：两类 JobCore 的最小可观察构造。
//!
//! `shell` / `state` 两个测试模块共用。相关类型都是 `pub(super)`，夹具只能留在
//! background 模块内部，crate 根的 `test_support.rs` 看不到它们。

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::tools::background::BackgroundTaskOwner;
use pipi_protocol::{AbortSignal, BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskStatus};

use super::agent::AgentJobEntry;
use super::shell::JobEntry;
use super::state::JobCore;

pub(super) fn owner() -> BackgroundTaskOwner {
    BackgroundTaskOwner::new("agent", "session", 1)
}

fn info(status: BackgroundTaskStatus, kind: BackgroundTaskKind) -> BackgroundTaskInfo {
    BackgroundTaskInfo {
        job_id: "job-1".into(),
        kind,
        agent_name: "agent".into(),
        session_id: "session".into(),
        run_id: 1,
        command: "echo".into(),
        cwd: "/tmp".into(),
        target_agent: None,
        child_session_id: None,
        status,
        started_at: 1,
        completed_at: None,
        exit_code: None,
        output_cursor: 0,
        output_truncated: false,
        error: None,
        result: None,
    }
}

pub(super) fn entry(status: BackgroundTaskStatus) -> JobEntry {
    let (control, _rx) = mpsc::unbounded_channel();
    JobEntry {
        core: JobCore::new(
            BackgroundTaskKind::Shell,
            owner(),
            info(status, BackgroundTaskKind::Shell),
            PathBuf::new(),
        ),
        pid: std::sync::atomic::AtomicU32::new(0),
        control,
        log_path: PathBuf::new(),
    }
}

pub(super) fn agent_entry(status: BackgroundTaskStatus) -> AgentJobEntry {
    AgentJobEntry {
        core: JobCore::new(
            BackgroundTaskKind::Agent,
            owner(),
            info(status, BackgroundTaskKind::Agent),
            PathBuf::new(),
        ),
        abort: AbortSignal::new(),
    }
}
