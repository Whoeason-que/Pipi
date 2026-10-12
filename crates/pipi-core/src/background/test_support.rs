//! 后台任务测试夹具：两类 JobEntry 的最小可观察构造。
//!
//! `shell` / `state` 两个测试模块共用。相关类型都是 `pub(super)`，夹具只能留在
//! background 模块内部，crate 根的 `test_support.rs` 看不到它们。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;

use tokio::sync::{Notify, mpsc};

use crate::tools::background::BackgroundTaskOwner;
use pipi_protocol::{AbortSignal, BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskStatus};

use super::agent::AgentJobEntry;
use super::shell::JobEntry;
use super::state::JobState;

pub(super) fn owner() -> BackgroundTaskOwner {
    BackgroundTaskOwner::new("agent", "session", 1)
}

pub(super) fn entry(status: BackgroundTaskStatus) -> JobEntry {
    let (control, _rx) = mpsc::unbounded_channel();
    JobEntry {
        owner: Mutex::new(owner()),
        pid: AtomicU32::new(0),
        state: Mutex::new(JobState {
            info: BackgroundTaskInfo {
                job_id: "job-1".into(),
                kind: BackgroundTaskKind::Shell,
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
            },
            output: VecDeque::new(),
            output_bytes: 0,
            next_seq: 0,
        }),
        control,
        notify: Notify::new(),
        metadata_path: PathBuf::new(),
        log_path: PathBuf::new(),
    }
}

pub(super) fn agent_entry(status: BackgroundTaskStatus) -> AgentJobEntry {
    let mut state = JobState {
        info: entry(status).info(),
        output: VecDeque::new(),
        output_bytes: 0,
        next_seq: 0,
    };
    state.info.kind = BackgroundTaskKind::Agent;
    AgentJobEntry {
        owner: Mutex::new(owner()),
        abort: AbortSignal::new(),
        state: Mutex::new(state),
        notify: Notify::new(),
        metadata_path: PathBuf::new(),
    }
}
