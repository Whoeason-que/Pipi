//! Linux/内核沙箱的黑盒回归：只经 `pipi-sandbox` helper 的公开入口，
//! 断言「写工作区外、read-only 写入、无网络」在内核层失败。
//!
//! 这些断言在 macOS 上不适用（Seatbelt 由 `sandbox-exec` 提供），因此整个
//! 文件只在 Linux 编译。

#![cfg(target_os = "linux")]

use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};

fn run(policy: &str, program: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pipi-sandbox"))
        .arg("--policy")
        .arg(policy)
        .arg("--")
        .arg(program)
        .args(args)
        .output()
        .expect("helper should be runnable")
}

fn policy(mode: &str, network: bool, roots: &[&Path]) -> String {
    let roots: Vec<String> = roots
        .iter()
        .map(|root| format!("\"{}\"", root.display()))
        .collect();
    format!(
        r#"{{"mode":"{mode}","networkAccess":{network},"writableRoots":[{}]}}"#,
        roots.join(",")
    )
}

fn python3() -> Option<&'static str> {
    Command::new("python3")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|_| "python3")
}

#[test]
fn workspace_write_confines_filesystem_writes() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let outside = root.path().join("outside.txt");
    let policy = policy("workspace-write", false, &[&workspace]);

    let inside = workspace.join("inside.txt");
    let output = run(
        &policy,
        "bash",
        &["-c", &format!("printf ok > '{}'", inside.display())],
    );
    assert!(
        output.status.success(),
        "workspace write should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_to_string(&inside).unwrap(), "ok");

    let output = run(
        &policy,
        "bash",
        &["-c", &format!("printf bad > '{}'", outside.display())],
    );
    assert!(!output.status.success(), "outside write must fail");
    assert!(!outside.exists(), "outside file must not be created");
}

#[test]
fn read_only_allows_reads_but_not_writes() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let policy = policy("read-only", false, &[]);

    let output = run(&policy, "bash", &["-c", "cat /etc/hosts >/dev/null"]);
    assert!(output.status.success(), "read-only should allow reads");

    let output = run(
        &policy,
        "bash",
        &["-c", &format!("printf bad > '{}/x'", workspace.display())],
    );
    assert!(!output.status.success(), "read-only write must fail");
}

#[test]
fn network_is_blocked_by_default() {
    let Some(python) = python3() else {
        eprintln!("skip: python3 not available");
        return;
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let root = tempfile::tempdir().unwrap();
    let blocked = self::policy("workspace-write", false, &[root.path()]);
    let script = format!(
        "import socket\nsocket.socket(socket.AF_INET, socket.SOCK_STREAM).connect(('127.0.0.1', {port}))"
    );
    let output = run(&blocked, python, &["-c", &script]);
    assert!(!output.status.success(), "network must be blocked");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("PermissionError") || stderr.contains("Operation not permitted"),
        "unexpected error: {stderr}"
    );

    // networkAccess=true 时同一策略应能连接（listener 仍在监听）。
    let allowed = self::policy("workspace-write", true, &[root.path()]);
    let output = run(&allowed, python, &["-c", &script]);
    assert!(
        output.status.success(),
        "network should be allowed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn helper_fails_closed_on_bad_invocation() {
    let output = Command::new(env!("CARGO_BIN_EXE_pipi-sandbox"))
        .arg("--policy")
        .arg("{}")
        .arg("--")
        .arg("/bin/true")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("pipi-sandbox"), "{stderr}");
}
