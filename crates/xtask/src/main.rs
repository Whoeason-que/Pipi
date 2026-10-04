//! 仓库维护任务。
//!
//! - `cargo xtask arch` —— 按根目录 `architecture.toml` 检查工作区的分层与
//!   依赖方向：内部依赖白名单、禁用依赖、无环、成员登记完整。违规时非零退出，
//!   可以放进提交前检查或 CI。
//!
//! 别名在 `.cargo/config.toml`（`xtask = "run --package xtask --"`）。

mod arch;

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("arch") => arch::run(),
        other => {
            eprintln!(
                "用法: cargo xtask arch\n（未知子命令: {}）",
                other.unwrap_or("无")
            );
            ExitCode::from(2)
        }
    }
}
