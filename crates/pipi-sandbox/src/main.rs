//! `pipi-sandbox` helper：`--policy <json> -- <program> [args...]`。
//!
//! 进程模型：helper 自己就是普通进程，策略解析、规则构造、日志都不受
//! `pre_exec` 的 async-signal-safe 限制；装好内核限制后直接 `exec` 目标命令。
//! 任何失败都以非零退出（fail-closed），不会退化成无沙箱执行。

#[cfg(target_os = "linux")]
use std::ffi::OsString;
use std::io::Write;

use pipi_sandbox::{USAGE, parse_invocation};

fn main() {
    if let Err(error) = run() {
        let _ = writeln!(std::io::stderr(), "pipi-sandbox: {error}");
        let _ = writeln!(std::io::stderr(), "{USAGE}");
        std::process::exit(125);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if args
        .first()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        println!("{USAGE}");
        return Ok(());
    }
    let invocation = parse_invocation(args)?;

    #[cfg(target_os = "macos")]
    {
        // exec 成功则永不返回。
        pipi_sandbox::macos::exec(&invocation.policy, &invocation.program, &invocation.args)
    }

    #[cfg(target_os = "linux")]
    {
        pipi_sandbox::apply(&invocation.policy)?;
        exec(invocation.program, invocation.args)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = invocation;
        Err("pipi-sandbox currently supports only Linux (Landlock) and macOS (Seatbelt)".into())
    }
}

#[cfg(target_os = "linux")]
fn exec(program: OsString, args: Vec<OsString>) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let error = std::process::Command::new(&program).args(&args).exec();
    Err(format!(
        "failed to exec {}: {error}",
        program.to_string_lossy()
    ))
}
