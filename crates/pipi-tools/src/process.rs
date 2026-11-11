//! 子进程的进程组收尾：spawn 时用 `process_group(0)` 单独成组，终止时给整组
//! 发信号。只杀直接子进程会留下孙进程 —— 它们继续持有 stdout/stderr 管道，
//! 读取任务永远等不到 EOF（表现为工具「卡住不返回」），也不受超时约束。

/// 兜底：工具 future 在跑到收尾之前被 drop（任务取消、运行时关闭）时，
/// 整组仍在跑的孙进程会失去约束，这里补一次组终止。正常路径必须 `disarm()`，
/// 否则已回收的 PID 可能被系统复用，误杀无关进程组。
pub(crate) struct GroupGuard(Option<u32>);

impl GroupGuard {
    pub(crate) fn new(pid: Option<u32>) -> Self {
        Self(pid)
    }

    pub(crate) fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = kill_process_group(pid);
        }
    }
}

/// 终止整个进程组（`SIGKILL`）。
///
/// 目标进程组已经不存在（`ESRCH`）时视为成功：这是正常的收尾竞态，调用方
/// 随后仍要 `wait()` 回收直接子进程。
#[cfg(unix)]
pub fn kill_process_group(pid: u32) -> Result<(), String> {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    match killpg(Pid::from_raw(pid as i32), Signal::SIGKILL) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(error) => Err(format!("Failed to terminate process group {pid}: {error}")),
    }
}

/// 给整个进程组发 `SIGTERM`（先礼后兵的第一步）。
#[cfg(unix)]
pub fn terminate_process_group(pid: u32) -> Result<(), String> {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    match killpg(Pid::from_raw(pid as i32), Signal::SIGTERM) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(error) => Err(format!("Failed to terminate process group {pid}: {error}")),
    }
}

/// 非 unix 平台没有进程组语义：中止 / 超时时杀掉直接子进程（与旧行为一致）。
#[cfg(not(unix))]
pub fn kill_process_group(_pid: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(not(unix))]
pub fn terminate_process_group(_pid: u32) -> Result<(), String> {
    Ok(())
}
