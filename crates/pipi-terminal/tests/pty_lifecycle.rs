//! Black-box PTY lifecycle: real shells, real process groups.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use pipi_terminal::{TerminalManager, TerminalRead, TerminalSpec};

fn temp_workspace(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("pipi-terminal-{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn spec(program: &str, args: &[&str]) -> TerminalSpec {
    TerminalSpec {
        program: program.to_string(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        env: vec![("TERM".into(), "xterm-256color".into())],
        cols: 80,
        rows: 24,
    }
}

async fn read_until(
    manager: &TerminalManager,
    workspace: &Path,
    terminal_id: &str,
    needle: &str,
    timeout: Duration,
) -> String {
    let deadline = Instant::now() + timeout;
    let mut cursor = 0u64;
    let mut text = String::new();
    while Instant::now() < deadline {
        let read = manager
            .read(workspace, terminal_id, cursor, 256 * 1024, 200)
            .await
            .unwrap();
        cursor = read.next_seq;
        for chunk in read.chunks {
            text.push_str(&String::from_utf8_lossy(&chunk.data));
        }
        if text.contains(needle) {
            return text;
        }
        if !read.running {
            break;
        }
    }
    panic!("terminal did not produce {needle:?}; collected: {text:?}");
}

async fn wait_exit(
    manager: &TerminalManager,
    workspace: &Path,
    terminal_id: &str,
    timeout: Duration,
) -> TerminalRead {
    let deadline = Instant::now() + timeout;
    let mut cursor = 0u64;
    while Instant::now() < deadline {
        let read = manager
            .read(workspace, terminal_id, cursor, 256 * 1024, 100)
            .await
            .unwrap();
        cursor = read.next_seq;
        if !read.running {
            return read;
        }
    }
    panic!("terminal did not exit within {timeout:?}");
}

fn process_state(pid: u32) -> String {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output();
    match output {
        Ok(output) => String::from_utf8_lossy(&output.stdout).trim().to_string(),
        Err(_) => String::new(),
    }
}

fn alive(pid: u32) -> bool {
    let state = process_state(pid);
    // A zombie has already exited; only a live (non-Z) process matters.
    !state.is_empty() && !state.starts_with('Z')
}

async fn wait_pid_file(path: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse::<u32>()
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "shell did not write its pid");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn shell_echoes_input_resizes_and_reports_exit() {
    let workspace = temp_workspace("echo");
    let manager = TerminalManager::new();
    let snapshot = manager.open(&workspace, spec("/bin/sh", &[])).unwrap();
    assert!(snapshot.running);
    assert_eq!(snapshot.workspace, workspace.to_string_lossy());

    manager
        .write(
            &workspace,
            &snapshot.terminal_id,
            b"echo marker-123\n".to_vec(),
        )
        .await
        .unwrap();
    let output = read_until(
        &manager,
        &workspace,
        &snapshot.terminal_id,
        "marker-123",
        Duration::from_secs(5),
    )
    .await;
    assert!(output.contains("marker-123"));

    manager
        .resize(&workspace, &snapshot.terminal_id, 100, 30)
        .unwrap();
    let listed = manager.list(&workspace);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].cols, 100);
    assert_eq!(listed[0].rows, 30);

    manager
        .write(&workspace, &snapshot.terminal_id, b"exit\n".to_vec())
        .await
        .unwrap();
    let exited = wait_exit(
        &manager,
        &workspace,
        &snapshot.terminal_id,
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(exited.exit_code, Some(0));

    manager.shutdown().await;
    std::fs::remove_dir_all(workspace).ok();
}

#[tokio::test]
async fn closing_reaches_a_foreground_job_in_its_own_process_group() {
    let workspace = temp_workspace("foreground");
    let pid_file = workspace.join("pid");
    let manager = TerminalManager::new();
    let snapshot = manager.open(&workspace, spec("/bin/sh", &[])).unwrap();
    manager
        .write(
            &workspace,
            &snapshot.terminal_id,
            format!("sh -c 'echo $$ > {}; sleep 30'\n", pid_file.display()).into_bytes(),
        )
        .await
        .unwrap();

    let pid = wait_pid_file(&pid_file).await;
    assert!(alive(pid), "foreground job should be running before close");

    manager.close(&workspace, &snapshot.terminal_id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(pid) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !alive(pid),
        "foreground job survived terminal close (process group not reached)"
    );

    manager.shutdown().await;
    std::fs::remove_dir_all(workspace).ok();
}

#[tokio::test]
async fn closing_a_terminal_kills_the_whole_process_group() {
    let workspace = temp_workspace("kill");
    let pid_file = workspace.join("pid");
    let command = format!("echo $$ > '{}'; sleep 30 & wait", pid_file.display());
    let manager = TerminalManager::new();
    let snapshot = manager
        .open(&workspace, spec("/bin/sh", &["-c", &command]))
        .unwrap();

    let pid = wait_pid_file(&pid_file).await;
    assert!(alive(pid), "shell should be running before close");

    manager.close(&workspace, &snapshot.terminal_id).unwrap();
    assert!(manager.list(&workspace).is_empty());

    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(pid) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!alive(pid), "process group survived terminal close");

    manager.shutdown().await;
    std::fs::remove_dir_all(workspace).ok();
}
