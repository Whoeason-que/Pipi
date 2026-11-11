//! End-to-end user terminal commands: workspace resolution, cursor reads,
//! resize and whole-tab close. The test rewrites HOME, so it holds HOME_LOCK
//! for the whole async body (the repository-wide lock).
#![allow(clippy::await_holding_lock)]
#![cfg(unix)]

use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};

use base64::{Engine, engine::general_purpose::STANDARD};
use pipi_core::commands::{
    self, CloseTerminalArgs, ListTerminalsArgs, OpenTerminalArgs, ReadTerminalArgs,
    ResizeTerminalArgs, WriteTerminalArgs,
};
use pipi_core::runtime::RuntimeState;
use pipi_test_support::{HOME_LOCK, remove_env_var, set_env_var, temp_dir};

struct Environment(Vec<(&'static str, Option<OsString>)>);

impl Environment {
    fn isolated(home: &Path) -> Self {
        let saved = ["HOME", "PIPI_HOME", "AV_HOME"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        set_env_var("HOME", home);
        set_env_var("PIPI_HOME", home.join(".pipi"));
        set_env_var("AV_HOME", home.join(".av"));
        Self(saved)
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            match value {
                Some(value) => set_env_var(key, value),
                None => remove_env_var(key),
            }
        }
    }
}

fn list(runtime: &RuntimeState) -> Vec<pipi_terminal::TerminalSnapshot> {
    commands::list_terminals(
        runtime,
        ListTerminalsArgs {
            agent_name: "terminal-test".into(),
            workspace: None,
        },
    )
    .unwrap()
}

#[tokio::test]
async fn terminal_commands_stream_cursor_reads_and_resolve_the_workspace() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = temp_dir("terminal-runtime-home");
    let _env = Environment::isolated(&home);
    let root = home.join("project");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    pipi_venv::agents::create_agent(
        "terminal-test",
        "test",
        Some(root.to_str().unwrap()),
        None,
        None,
        None,
    )
    .unwrap();
    let runtime = RuntimeState::new(tokio::runtime::Handle::current());

    assert!(list(&runtime).is_empty());

    let opened = commands::open_terminal(
        &runtime,
        OpenTerminalArgs {
            agent_name: "terminal-test".into(),
            workspace: None,
            cols: Some(90),
            rows: Some(28),
        },
    )
    .await
    .unwrap();
    assert!(opened.running);
    assert_eq!(opened.workspace, root.to_string_lossy());
    assert_eq!((opened.cols, opened.rows), (90, 28));

    // The navigation page passes an explicit workspace; it must land in the
    // same canonical bucket as the Agent default.
    let same = commands::list_terminals(
        &runtime,
        ListTerminalsArgs {
            agent_name: "terminal-test".into(),
            workspace: Some(root.to_string_lossy().into_owned()),
        },
    )
    .unwrap();
    assert_eq!(same.len(), 1);
    assert_eq!(same[0].terminal_id, opened.terminal_id);

    let input = STANDARD.encode(b"echo pipi-terminal-marker\n");
    commands::write_terminal(
        &runtime,
        WriteTerminalArgs {
            agent_name: "terminal-test".into(),
            workspace: None,
            terminal_id: opened.terminal_id.clone(),
            data: input,
        },
    )
    .await
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut cursor = 0u64;
    let mut output = String::new();
    while Instant::now() < deadline && !output.contains("pipi-terminal-marker") {
        let read = commands::read_terminal(
            &runtime,
            ReadTerminalArgs {
                agent_name: "terminal-test".into(),
                workspace: None,
                terminal_id: opened.terminal_id.clone(),
                after_seq: cursor,
                max_bytes: None,
                wait_ms: Some(200),
            },
        )
        .await
        .unwrap();
        cursor = read.next_seq;
        for chunk in read.chunks {
            output.push_str(&String::from_utf8_lossy(
                &STANDARD.decode(chunk.data).unwrap(),
            ));
        }
    }
    assert!(
        output.contains("pipi-terminal-marker"),
        "shell did not echo the marker: {output:?}"
    );

    commands::resize_terminal(
        &runtime,
        ResizeTerminalArgs {
            agent_name: "terminal-test".into(),
            workspace: None,
            terminal_id: opened.terminal_id.clone(),
            cols: 100,
            rows: 30,
        },
    )
    .unwrap();
    let resized = list(&runtime);
    assert_eq!((resized[0].cols, resized[0].rows), (100, 30));

    commands::close_terminal(
        &runtime,
        CloseTerminalArgs {
            agent_name: "terminal-test".into(),
            workspace: None,
            terminal_id: opened.terminal_id,
        },
    )
    .unwrap();
    assert!(list(&runtime).is_empty());

    assert!(
        commands::open_terminal(
            &runtime,
            OpenTerminalArgs {
                agent_name: "missing-agent".into(),
                workspace: None,
                cols: None,
                rows: None,
            },
        )
        .await
        .is_err()
    );

    runtime.shutdown().await.unwrap();
}
