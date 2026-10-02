//! 子进程的进程组收尾：spawn 时用 `process_group(0)` 单独成组，终止时给整组
//! 发信号。只杀直接子进程会留下孙进程 —— 它们继续持有 stdout/stderr 管道，
//! 读取任务永远等不到 EOF（表现为工具「卡住不返回」），也不受超时约束。

/// 终止整个进程组（`SIGKILL`）。
///
/// 目标进程组已经不存在（`ESRCH`）时视为成功：这是正常的收尾竞态，调用方
/// 随后仍要 `wait()` 回收直接子进程。
#[cfg(unix)]
pub fn kill_process_group(pid: u32) -> Result<(), String> {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    match killpg(Pid::from_raw(pid as i32), Signal::SIGKILL) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(error) => Err(format!("终止进程组 {pid} 失败: {error}")),
    }
}

/// 给整个进程组发 `SIGTERM`（先礼后兵的第一步）。
#[cfg(unix)]
pub fn terminate_process_group(pid: u32) -> Result<(), String> {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    match killpg(Pid::from_raw(pid as i32), Signal::SIGTERM) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(error) => Err(format!("终止进程组 {pid} 失败: {error}")),
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
