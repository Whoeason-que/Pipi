//! Pipi 的应用服务层。
//!
//! `pipi-core` 保存可复用的领域逻辑；这里把它们组装为带会话槽、后台运行和
//! 宿主事件的应用服务。Tauri 与 Web server 共用这一层，因此不会各自复制
//! Agent 调度逻辑或改变 IPC event 的载荷。

pub use pipi_core::*;

pub mod approval;
pub mod background;
pub mod runtime;

/// `HOME` 是进程级状态。runtime 单元测试在自己的 crate 中改写它，必须串行。
#[cfg(test)]
pub(crate) static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
