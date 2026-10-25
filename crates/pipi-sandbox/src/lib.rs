//! OS 级沙箱（唯一安全边界）。
//!
//! 这里把 [`SandboxPolicy`] 变成内核能理解的能力集合：
//!
//! - Linux：`pipi-sandbox` helper 先安装 Landlock 文件系统规则，再用
//!   seccomp-BPF 禁止 `AF_INET`/`AF_INET6` socket，然后 `exec` 真实命令；
//! - macOS：helper 生成 SBPL profile 并 `exec /usr/bin/sandbox-exec`；
//! - 其他平台：显式失败（fail-closed），不会静默退化成无沙箱。
//!
//! 库侧只负责「找到 helper 并构造 Command」。**不要在主进程里用
//! `pre_exec` 安装策略**：fork 之后不能分配、不能读文件，策略解析与规则构造
//! 都必须发生在 helper 自己的进程里。
//!
//! 开关：`PIPI_SANDBOX=off|auto|require`（默认 auto）。`auto` 找不到 helper
//! 时回退到旧权限层并打印一次警告；`require` 直接报错，适合测试与发布环境。

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::Once;

use pipi_protocol::permissions::SandboxPolicy;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;

/// 沙箱后端开关。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// 完全关闭：走旧的用户态权限层（迁移期回滚开关）。
    Off,
    /// 有 helper 就使用，没有就回退并警告一次。
    Auto,
    /// 必须使用 helper；任何不可用都 fail-closed。
    Require,
}

/// 从 `PIPI_SANDBOX` 读取后端开关。
pub fn backend() -> Backend {
    match std::env::var("PIPI_SANDBOX").as_deref() {
        Ok("off") | Ok("0") => Backend::Off,
        Ok("require") | Ok("1") => Backend::Require,
        _ => Backend::Auto,
    }
}

/// 一次 spawn 的沙箱判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enforcement {
    /// 使用给定 helper 二进制。
    Sandboxed(PathBuf),
    /// 不回退到内核沙箱（旧权限层或显式关闭）。
    Legacy,
}

static WARNED: Once = Once::new();

fn warn_legacy_once() {
    WARNED.call_once(|| {
        eprintln!(
            "pipi: pipi-sandbox helper not found; falling back to the legacy userspace permission layer (set PIPI_SANDBOX=require to fail instead)"
        );
    });
}

/// 查找 helper 二进制。
///
/// 顺序：`PIPI_SANDBOX_BIN` → 当前可执行文件附近（开发 target/、Tauri sidecar、
/// macOS .app 的 MacOS/Resources）→ `PATH`。
pub fn binary() -> Option<PathBuf> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        binary_on_supported_platform()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // 没有后端实现的平台不要假装能用：auto 回退旧权限层，require 报错。
        None
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn binary_on_supported_platform() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("PIPI_SANDBOX_BIN") {
        // 显式覆盖优先且不透传：指向不存在的 helper 时直接视为不可用，
        // 不再悄悄从 PATH 找另一个（require 模式据此 fail-closed）。
        let path = PathBuf::from(raw);
        return path.is_file().then_some(path);
    }

    let name = if cfg!(windows) {
        "pipi-sandbox.exe"
    } else {
        "pipi-sandbox"
    };

    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let mut candidates = vec![dir.join(name)];
        if let Some(parent) = dir.parent() {
            candidates.push(parent.join(name));
            if let Some(grandparent) = parent.parent() {
                candidates.push(grandparent.join(name));
            }
        }
        // Tauri macOS bundle：Contents/MacOS/<exe> 与 Contents/Resources/
        candidates.push(dir.join("../Resources").join(name));
        if let Some(found) = candidates.into_iter().find(|candidate| candidate.is_file()) {
            return Some(found);
        }
    }

    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 这次运行是否真的会挂内核沙箱（不产生警告）。
pub fn will_enforce(policy: &SandboxPolicy) -> bool {
    policy.is_enforced() && backend() != Backend::Off && binary().is_some()
}

/// 解析本次 spawn 的沙箱执行方式。
pub fn enforcement(policy: &SandboxPolicy) -> Result<Enforcement, String> {
    if !policy.is_enforced() || backend() == Backend::Off {
        return Ok(Enforcement::Legacy);
    }
    match binary() {
        Some(binary) => Ok(Enforcement::Sandboxed(binary)),
        None if backend() == Backend::Require => {
            Err("OS sandbox unavailable (pipi-sandbox helper not found); refusing to run. \
             Set PIPI_SANDBOX_BIN to the helper path, or PIPI_SANDBOX=off to disable the sandbox explicitly"
                .into())
        }
        None => {
            warn_legacy_once();
            Ok(Enforcement::Legacy)
        }
    }
}

fn policy_json(policy: &SandboxPolicy) -> Result<String, String> {
    serde_json::to_string(policy)
        .map_err(|error| format!("Failed to serialize sandbox policy: {error}"))
}

/// 宿主级逃生阀（迁移期）：
/// - `PIPI_SANDBOX_NETWORK=1`：保留文件系统沙箱、放行网络；
/// - `PIPI_SANDBOX_WRITABLE`：额外的可写根（`split_paths` 分隔，必须是绝对路径）。
///
/// 正式的按 Agent/按次授权（审批提升能力）是下一步；这里只解决「不关沙箱就
/// 无法用工具链缓存」的过渡问题。
fn effective_policy(policy: &SandboxPolicy) -> SandboxPolicy {
    let allow_network = std::env::var("PIPI_SANDBOX_NETWORK")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "on"));
    let mut effective = policy.clone();
    if allow_network {
        effective.network_access = true;
    }
    if let Some(raw) = std::env::var_os("PIPI_SANDBOX_WRITABLE") {
        for path in std::env::split_paths(&raw) {
            if path.is_absolute() && !effective.writable_roots.contains(&path) {
                effective.writable_roots.push(path);
            }
        }
    }
    effective
}

/// 构造一个 std [`std::process::Command`]，必要时经 helper 包装。
pub fn std_command<I, S>(
    policy: &SandboxPolicy,
    program: impl AsRef<OsStr>,
    args: I,
) -> Result<std::process::Command, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let policy = effective_policy(policy);
    match enforcement(&policy)? {
        Enforcement::Sandboxed(binary) => {
            let mut command = std::process::Command::new(binary);
            command
                .arg("--policy")
                .arg(policy_json(&policy)?)
                .arg("--")
                .arg(program);
            command.args(args);
            Ok(command)
        }
        Enforcement::Legacy => {
            let mut command = std::process::Command::new(program);
            command.args(args);
            Ok(command)
        }
    }
}

/// 构造一个 tokio [`tokio::process::Command`]，必要时经 helper 包装。
pub fn tokio_command<I, S>(
    policy: &SandboxPolicy,
    program: impl AsRef<OsStr>,
    args: I,
) -> Result<tokio::process::Command, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let policy = effective_policy(policy);
    match enforcement(&policy)? {
        Enforcement::Sandboxed(binary) => {
            let mut command = tokio::process::Command::new(binary);
            command
                .arg("--policy")
                .arg(policy_json(&policy)?)
                .arg("--")
                .arg(program);
            command.args(args);
            Ok(command)
        }
        Enforcement::Legacy => {
            let mut command = tokio::process::Command::new(program);
            command.args(args);
            Ok(command)
        }
    }
}

/// helper 侧的参数：`--policy <json> -- <program> [args...]`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxInvocation {
    pub policy: SandboxPolicy,
    pub program: OsString,
    pub args: Vec<OsString>,
}

/// 解析 helper 参数。供 `pipi-sandbox` 二进制与测试共用。
pub fn parse_invocation<I>(args: I) -> Result<SandboxInvocation, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let mut policy_json = None;
    let mut program = None;
    let mut rest = Vec::new();

    while let Some(arg) = args.next() {
        if arg == OsStr::new("--policy") {
            let value = args
                .next()
                .ok_or("--policy is missing its JSON argument")?
                .into_string()
                .map_err(|_| "--policy must be UTF-8 JSON")?;
            policy_json = Some(value);
        } else if arg == OsStr::new("--") {
            program = args.next();
            rest.extend(args);
            break;
        } else if arg == OsStr::new("--help") || arg == OsStr::new("-h") {
            return Err(USAGE.into());
        } else {
            return Err(format!("unknown argument: {}", arg.to_string_lossy()));
        }
    }

    let policy = policy_json.ok_or("missing --policy")?;
    let policy = serde_json::from_str(&policy)
        .map_err(|error| format!("invalid sandbox policy JSON: {error}"))?;
    let program = program.ok_or("missing `-- <program>`")?;
    Ok(SandboxInvocation {
        policy,
        program,
        args: rest,
    })
}

pub const USAGE: &str = "usage: pipi-sandbox --policy <json> -- <program> [args...]";

/// 在 helper 进程内安装沙箱（之后调用方立即 `exec`）。
pub fn apply(policy: &SandboxPolicy) -> Result<(), String> {
    if !policy.is_enforced() {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        linux::apply(policy)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("sandbox on this platform must be installed via the helper exec path".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::permissions::SandboxMode;
    use std::path::Path;

    #[test]
    fn parses_helper_invocation() {
        let policy =
            SandboxPolicy::for_workspace(SandboxMode::WorkspaceWrite, Path::new("/tmp/ws"));
        let json = serde_json::to_string(&policy).unwrap();
        let invocation = parse_invocation([
            OsString::from("--policy"),
            OsString::from(json),
            OsString::from("--"),
            OsString::from("bash"),
            OsString::from("-c"),
            OsString::from("echo hi"),
        ])
        .unwrap();
        assert_eq!(invocation.policy, policy);
        assert_eq!(invocation.program, OsString::from("bash"));
        assert_eq!(
            invocation.args,
            vec![OsString::from("-c"), OsString::from("echo hi")]
        );
    }

    #[test]
    fn parses_usage_errors() {
        assert!(parse_invocation([OsString::from("--policy")]).is_err());
        assert!(parse_invocation([OsString::from("--")]).is_err());
        assert!(parse_invocation([OsString::from("nope")]).is_err());
        let error = parse_invocation([OsString::from("--help")]).unwrap_err();
        assert!(error.contains("usage"));
    }

    #[test]
    fn dangerous_mode_never_enforces() {
        let policy =
            SandboxPolicy::for_workspace(SandboxMode::DangerFullAccess, Path::new("/tmp/ws"));
        assert!(!will_enforce(&policy));
        assert_eq!(enforcement(&policy).unwrap(), Enforcement::Legacy);
    }
}
