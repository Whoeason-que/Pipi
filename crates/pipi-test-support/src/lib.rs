//! 测试脚手架：进程级环境变量（`HOME` / `AV_HOME` / `PIPI_HOME`）的串行化改写。
//!
//! 为什么是独立 crate：拆分后改写环境变量的用例分散在多个 crate（pipi-core、
//! pipi-skills、pipi-app…），而 `pub(crate)` 助手不跨 crate。每个测试二进制各自
//! 链接一份本 crate，[`HOME_LOCK`] 因此是「每个测试进程一把锁」—— 同一进程内的
//! 用例持锁串行，不同测试二进制本来就是不同进程，互不影响。
//!
//! 用法：先 `let _guard = pipi_test_support::HOME_LOCK.lock().unwrap();`，再调用
//! [`set_env_var`] / [`remove_env_var`]，离开作用域恢复。**顺序不能反**：edition
//! 2024 起 `std::env::set_var` 是 unsafe，安全性正来自这把锁。

use std::ffi::OsStr;
use std::sync::Mutex;

/// 临时唯一 ID：只用于临时目录命名与测试 fixture，**不是**任何持久化身份
/// （Agent/session 的 JSONL id 由 `pipi_agent::session::new_id` 负责）。
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 环境改写互斥锁。所有改写 `HOME` / `AV_HOME` / `PIPI_HOME` 的用例必须先持有它。
pub static HOME_LOCK: Mutex<()> = Mutex::new(());

/// 测试内改写进程环境。调用方必须先持有 [`HOME_LOCK`]。
pub fn set_env_var(key: &str, value: impl AsRef<OsStr>) {
    // SAFETY: 调用方持 HOME_LOCK，同一进程内不再有并发的环境读写
    unsafe { std::env::set_var(key, value) };
}

/// 见 [`set_env_var`]：同样要求调用方持 [`HOME_LOCK`]。
pub fn remove_env_var(key: &str) {
    // SAFETY: 调用方持 HOME_LOCK
    unsafe { std::env::remove_var(key) };
}
