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
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    #[cfg(target_os = "linux")]
    let restored_env = if args.first().is_some_and(|arg| arg == "--restore-agent-env") {
        args.remove(0);
        use std::os::unix::ffi::OsStringExt;
        let count = std::env::var(pipi_sandbox::AGENT_ENV_TRANSFER)
            .map_err(|_| "missing Agent environment for memory scope")?;
        let count = count
            .parse::<usize>()
            .map_err(|_| "invalid Agent environment chunk count")?;
        if count == 0 || count > 1024 {
            return Err("invalid Agent environment chunk count".into());
        }
        let mut raw = Vec::new();
        for index in 0..count {
            let chunk =
                std::env::var_os(format!("{}{index}", pipi_sandbox::AGENT_ENV_CHUNK_PREFIX))
                    .ok_or("missing Agent environment chunk")?;
            raw.extend(chunk.into_vec());
        }
        Some(
            serde_json::from_slice::<std::collections::BTreeMap<String, String>>(&raw)
                .map_err(|error| format!("invalid Agent environment for memory scope: {error}"))?,
        )
    } else {
        None
    };
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
        exec(invocation.program, invocation.args, restored_env)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = invocation;
        Err("pipi-sandbox currently supports only Linux (Landlock) and macOS (Seatbelt)".into())
    }
}

#[cfg(target_os = "linux")]
fn exec(
    program: OsString,
    args: Vec<OsString>,
    env: Option<std::collections::BTreeMap<String, String>>,
) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new(&program);
    command.args(&args);
    if let Some(env) = env {
        command.env_clear().envs(env);
    }
    let error = command.exec();
    Err(format!(
        "failed to exec {}: {error}",
        program.to_string_lossy()
    ))
}
