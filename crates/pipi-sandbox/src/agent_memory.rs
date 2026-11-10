//! One systemd user slice bounds the aggregate of Agent-controlled processes.
//! The desktop/Web host opts in; the sandbox helper still enforces OS policy.
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use pipi_protocol::permissions::SandboxPolicy;

const SLICE: &str = "pipi-agents.slice";
const LIMIT: &str = "MemoryMax=16G";
const SWAP: &str = "MemorySwapMax=0";
static ENABLED: AtomicBool = AtomicBool::new(false);
static INITIALIZED: Mutex<bool> = Mutex::new(false);

pub(super) fn enable() {
    ENABLED.store(true, Ordering::Release);
}

pub(super) fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

fn runtime_dir() -> Result<String, String> {
    let uid = std::fs::metadata("/proc/self")
        .map_err(|error| format!("Cannot identify user for Agent memory limit: {error}"))?
        .uid();
    Ok(format!("/run/user/{uid}"))
}

fn bus_environment(command: &mut Command, runtime: &str) {
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_RUNTIME_DIR", runtime)
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={runtime}/bus"),
        );
}

fn systemctl(runtime: &str, args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new("/usr/bin/systemctl");
    bus_environment(&mut cmd, runtime);
    let mut child = cmd
        .args(["--user", "--no-ask-password", "--no-pager"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Agent memory limit unavailable: {error}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Agent memory limit unavailable: systemd request timed out".into());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Agent memory limit unavailable: {error}"));
            }
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("Agent memory limit unavailable: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "Agent memory limit unavailable: {}",
        message.trim()
    ))
}

fn initialize(runtime: &str, slice: &str, limit: &str) -> Result<(), String> {
    systemctl(runtime, &["start", slice])?;
    systemctl(runtime, &["set-property", "--runtime", slice, limit, SWAP])
}

pub(super) fn command<I, S>(
    policy: &SandboxPolicy,
    program: &OsStr,
    args: I,
    env: &BTreeMap<String, String>,
) -> Result<tokio::process::Command, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let runtime = runtime_dir()?;
    {
        let mut initialized = INITIALIZED
            .lock()
            .map_err(|_| "Agent memory limit initialization lock poisoned")?;
        if !*initialized {
            initialize(&runtime, SLICE, LIMIT)?;
            *initialized = true;
        }
    }
    scoped_command(policy, program, args, env, &runtime, SLICE)
}

fn scoped_command<I, S>(
    policy: &SandboxPolicy,
    program: &OsStr,
    args: I,
    env: &BTreeMap<String, String>,
    runtime: &str,
    slice: &str,
) -> Result<tokio::process::Command, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    // The trusted helper restores the exact Agent environment after systemd-run
    // has joined the scope. Secrets never appear in argv or the systemd unit.
    let helper =
        super::binary().ok_or("Agent memory limit unavailable (pipi-sandbox helper not found)")?;
    let policy = super::effective_policy(policy);
    let encoded_env = serde_json::to_vec(env)
        .map_err(|error| format!("Cannot encode Agent environment: {error}"))?;
    let mut cmd = tokio::process::Command::new("/usr/bin/systemd-run");
    cmd.arg("--user")
        .arg("--scope")
        .arg("--quiet")
        .arg("--expand-environment=no")
        .arg(format!("--slice={slice}"))
        .arg("--property=OOMPolicy=kill")
        .arg("--")
        .arg(helper)
        .arg("--restore-agent-env")
        .arg("--policy")
        .arg(super::policy_json(&policy)?)
        .arg("--")
        .arg(program)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_RUNTIME_DIR", runtime)
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={runtime}/bus"),
        );
    // Linux limits each environment entry, not just the total. Keep each
    // transport chunk below that limit, then restore the exact map in helper.
    let chunks: Vec<_> = encoded_env.chunks(16 * 1024).collect();
    cmd.env(super::AGENT_ENV_TRANSFER, chunks.len().to_string());
    for (index, chunk) in chunks.iter().enumerate() {
        cmd.env(
            format!("{}{index}", super::AGENT_ENV_CHUNK_PREFIX),
            std::ffi::OsString::from_vec(chunk.to_vec()),
        );
    }
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::permissions::SandboxMode;
    use std::io::Write;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    #[test]
    fn unavailable_user_manager_fails_closed() {
        let error = initialize(
            "/nonexistent-pipi-runtime",
            "pipi-test.slice",
            "MemoryMax=96M",
        )
        .expect_err("an unavailable user bus must not silently skip the limit");
        assert!(error.contains("Agent memory limit unavailable"));
    }

    struct TestSlice {
        runtime: String,
        name: String,
    }

    impl Drop for TestSlice {
        fn drop(&mut self) {
            let _ = systemctl(&self.runtime, &["stop", &self.name]);
            let _ = systemctl(&self.runtime, &["revert", &self.name]);
        }
    }

    fn scoped(
        fixture: &TestSlice,
        code: &str,
        env: &BTreeMap<String, String>,
    ) -> std::process::Child {
        let policy = SandboxPolicy::for_workspace(
            SandboxMode::DangerFullAccess,
            std::path::Path::new("/tmp"),
        );
        let mut command = scoped_command(
            &policy,
            OsStr::new("/usr/bin/python3"),
            ["-c", code],
            env,
            &fixture.runtime,
            &fixture.name,
        )
        .unwrap();
        command
            .as_std_mut()
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    /// Opt-in: creates a temporary systemd user slice and intentionally OOMs
    /// one of two bounded processes. Run with PIPI_TEST_SYSTEMD_MEMORY=1.
    #[test]
    fn real_scopes_share_limit_and_preserve_agent_environment() {
        if std::env::var_os("PIPI_TEST_SYSTEMD_MEMORY").is_none() {
            return;
        }
        let fixture = TestSlice {
            runtime: runtime_dir().unwrap(),
            name: format!("pipimemorytest{}.slice", std::process::id()),
        };
        initialize(&fixture.runtime, &fixture.name, "MemoryMax=96M").unwrap();
        let mut env = BTreeMap::from([
            ("TOKEN".into(), "private-test-value".into()),
            ("LC_ALL".into(), "C.UTF-8".into()),
            ("DBUS_SESSION_BUS_ADDRESS".into(), "agent-bus".into()),
            ("XDG_RUNTIME_DIR".into(), "agent-runtime".into()),
            (
                super::super::AGENT_ENV_TRANSFER.into(),
                "agent-owned-value".into(),
            ),
            (
                format!("{}0", super::super::AGENT_ENV_CHUNK_PREFIX),
                "agent-owned-chunk".into(),
            ),
        ]);
        for index in 0..8 {
            env.insert(format!("LONG_{index}"), "数据".repeat(5_000));
        }
        let child = scoped(
            &fixture,
            "import json,os; print(json.dumps(dict(os.environ)))",
            &env,
        );
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let actual: BTreeMap<String, String> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(actual, env);

        let dir = pipi_test_support::temp_dir("pipi-shared-memory");
        // The helper must resolve executable names using the Agent's PATH,
        // not the limited environment used by systemd-run.
        use std::os::unix::fs::PermissionsExt;
        let local_tool = dir.join("agent-only-tool");
        std::fs::write(&local_tool, "#!/bin/sh\nprintf 'agent-path\\n'\n").unwrap();
        std::fs::set_permissions(&local_tool, std::fs::Permissions::from_mode(0o700)).unwrap();
        let agent_path = BTreeMap::from([("PATH".to_string(), dir.display().to_string())]);
        let full = SandboxPolicy::for_workspace(SandboxMode::DangerFullAccess, &dir);
        let mut path_command = scoped_command(
            &full,
            OsStr::new("agent-only-tool"),
            std::iter::empty::<&str>(),
            &agent_path,
            &fixture.runtime,
            &fixture.name,
        )
        .unwrap();
        let path_output = path_command.as_std_mut().output().unwrap();
        assert!(
            path_output.status.success(),
            "{}",
            String::from_utf8_lossy(&path_output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&path_output.stdout).trim(),
            "agent-path"
        );

        // The scope wrapper must preserve bidirectional stdio (MCP) and must
        // still apply the existing read-only sandbox before the command runs.
        let policy = SandboxPolicy::for_workspace(SandboxMode::ReadOnly, &dir);
        let forbidden = dir.join("must-not-be-created");
        let code = format!(
            "import sys,os\ntry:\n open({:?},'w')\nexcept PermissionError:\n print('denied:'+sys.stdin.readline().strip()+':'+str(os.getpgrp()),flush=True)",
            forbidden.to_string_lossy()
        );
        let mut command = scoped_command(
            &policy,
            OsStr::new("/usr/bin/python3"),
            ["-c", &code],
            &env,
            &fixture.runtime,
            &fixture.name,
        )
        .unwrap();
        let mut io_child = command
            .as_std_mut()
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let process_group = io_child.id();
        io_child
            .stdin
            .take()
            .unwrap()
            .write_all(b"hello\n")
            .unwrap();
        let output = io_child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            format!("denied:hello:{process_group}")
        );
        assert!(!forbidden.exists());
        let mut network = scoped_command(
            &policy,
            OsStr::new("/usr/bin/python3"),
            [
                "-c",
                "import socket; socket.socket(socket.AF_INET, socket.SOCK_STREAM)",
            ],
            &env,
            &fixture.runtime,
            &fixture.name,
        )
        .unwrap();
        let output = network.as_std_mut().output().unwrap();
        assert!(
            !output.status.success(),
            "scope must not bypass network sandbox"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("PermissionError"));

        let first_ready = dir.join("first-ready");
        let first_code = format!(
            "import time; x=bytearray(55*1024*1024); open({:?},'w').write('ok'); time.sleep(30)",
            first_ready.to_string_lossy()
        );
        let mut first = scoped(&fixture, &first_code, &BTreeMap::new());
        let deadline = Instant::now() + Duration::from_secs(10);
        while !first_ready.exists() && Instant::now() < deadline {
            assert!(
                first.try_wait().unwrap().is_none(),
                "first task exited before allocation"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(first_ready.exists(), "first task did not start");
        let second_code = "import time; x=bytearray(55*1024*1024); time.sleep(30)";
        let mut second = scoped(&fixture, second_code, &BTreeMap::new());
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut killed = false;
        while Instant::now() < deadline {
            if first
                .try_wait()
                .unwrap()
                .is_some_and(|status| !status.success())
                || second
                    .try_wait()
                    .unwrap()
                    .is_some_and(|status| !status.success())
            {
                killed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = first.kill();
        let _ = second.kill();
        let _ = first.wait();
        let _ = second.wait();
        std::fs::remove_dir_all(dir).ok();
        assert!(
            killed,
            "the aggregate memory limit did not kill either task"
        );
    }
}
