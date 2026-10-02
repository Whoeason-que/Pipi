//! Pipi 的应用服务层。
//!
//! `pipi-core` 保存可复用的领域逻辑；这里把它们组装为带会话槽、后台运行和
//! 宿主事件的应用服务。Tauri 与 Web server 共用这一层，因此不会各自复制
//! Agent 调度逻辑或改变 IPC event 的载荷。

pub mod approval;
pub mod background;
pub mod runtime;

/// `HOME` 是进程级状态。runtime 单元测试在自己的 crate 中改写它，必须串行。
#[cfg(test)]
pub(crate) static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 测试内改写进程环境。
///
/// edition 2024 起 `std::env::set_var` 是 unsafe：环境块不是线程安全的。
/// 安全性由调用方保证 —— 先持 [`HOME_LOCK`] 再做环境读写；新增用例照此办理。
#[cfg(test)]
pub(crate) fn set_env_var(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    // SAFETY: 调用方持 HOME_LOCK
    unsafe { std::env::set_var(key, value) };
}

/// 见 [`set_env_var`]：同样要求调用方持 [`HOME_LOCK`]。
#[cfg(test)]
pub(crate) fn remove_env_var(key: &str) {
    // SAFETY: 调用方持 HOME_LOCK
    unsafe { std::env::remove_var(key) };
}
