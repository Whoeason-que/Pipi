//! One PTY-backed shell.
//!
//! Reads happen on a dedicated blocking thread (portable-pty readers are
//! blocking), so the Tokio runtime never parks on the PTY. A second thread
//! waits for the child and publishes the exit status. The master, writer and
//! killer are kept for resize, input and process-group teardown.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize};

use crate::buffer::{OutputBuffer, READ_CHUNK_BYTES};
use crate::types::{TerminalRead, TerminalSnapshot, TerminalSpec, now_millis};

struct Status {
    running: bool,
    exit_code: Option<i32>,
}

pub struct Terminal {
    id: String,
    workspace: PathBuf,
    shell: String,
    created_at: u64,
    /// PTY master; resize is the only operation we need from it.
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// `setsid()` makes the shell a session leader, so its pid is also the
    /// process-group id used for whole-group teardown.
    pid: Option<u32>,
    size: Mutex<(u16, u16)>,
    buffer: Arc<OutputBuffer>,
    status: Arc<Mutex<Status>>,
}

impl Terminal {
    pub fn spawn(
        id: String,
        workspace: PathBuf,
        spec: TerminalSpec,
        max_buffer_bytes: usize,
    ) -> Result<Self, String> {
        let pair = portable_pty::native_pty_system()
            .openpty(PtySize {
                rows: spec.rows.max(1),
                cols: spec.cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("Cannot allocate terminal: {error}"))?;
        let portable_pty::PtyPair { master, slave } = pair;

        let mut command = CommandBuilder::new(&spec.program);
        command.args(spec.args.iter());
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        command.cwd(&workspace);
        // The child becomes a session leader with this PTY as its controlling
        // terminal; portable-pty performs setsid() in the spawn helper.
        command.set_controlling_tty(true);

        let mut child = slave
            .spawn_command(command)
            .map_err(|error| format!("Cannot start terminal shell: {error}"))?;
        drop(slave);

        let reader = master
            .try_clone_reader()
            .map_err(|error| format!("Cannot read from terminal: {error}"))?;
        let writer = master
            .take_writer()
            .map_err(|error| format!("Cannot write to terminal: {error}"))?;
        let killer = child.clone_killer();
        let pid = child.process_id();
        let buffer = Arc::new(OutputBuffer::new(max_buffer_bytes));
        let status = Arc::new(Mutex::new(Status {
            running: true,
            exit_code: None,
        }));

        let read_buffer = buffer.clone();
        let read_thread = std::thread::Builder::new()
            .name(format!("pipi-terminal-read-{id}"))
            .spawn(move || {
                let mut reader = reader;
                let mut chunk = vec![0u8; READ_CHUNK_BYTES];
                loop {
                    match reader.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(read) => read_buffer.push(chunk[..read].to_vec()),
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        // Linux and macOS report EIO instead of EOF when the
                        // slave side closes; any read error ends the stream.
                        Err(_) => break,
                    }
                }
                read_buffer.wake();
            });
        if let Err(error) = read_thread {
            let mut killer = killer;
            let _ = killer.kill();
            return Err(format!("Cannot start terminal reader: {error}"));
        }

        let wait_status = status.clone();
        let wait_buffer = buffer.clone();
        let wait_thread = std::thread::Builder::new()
            .name(format!("pipi-terminal-wait-{id}"))
            .spawn(move || {
                let exit = child.wait();
                let exit_code = match exit {
                    Ok(status) => Some(status.exit_code() as i32),
                    Err(_) => None,
                };
                if let Ok(mut status) = wait_status.lock() {
                    status.running = false;
                    status.exit_code = exit_code;
                }
                wait_buffer.wake();
            });
        if let Err(error) = wait_thread {
            let mut killer = killer;
            let _ = killer.kill();
            return Err(format!("Cannot start terminal waiter: {error}"));
        }

        Ok(Self {
            id,
            workspace,
            shell: spec.program,
            created_at: now_millis(),
            master: Mutex::new(master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            pid,
            size: Mutex::new((spec.cols.max(1), spec.rows.max(1))),
            buffer,
            status,
        })
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        let (cols, rows) = *self.size.lock().unwrap_or_else(|error| error.into_inner());
        let status = self
            .status
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        TerminalSnapshot {
            terminal_id: self.id.clone(),
            workspace: self.workspace.to_string_lossy().into_owned(),
            shell: self.shell.clone(),
            cols,
            rows,
            running: status.running,
            exit_code: status.exit_code,
            created_at: self.created_at,
        }
    }

    pub fn running(&self) -> bool {
        self.status
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .running
    }

    pub async fn read(&self, after_seq: u64, max_bytes: usize, wait_ms: u64) -> TerminalRead {
        // A finished terminal never has more output; return the cursor read
        // immediately instead of making every poll wait for a timeout.
        let wait_ms = if self.running() { wait_ms } else { 0 };
        let slice = self.buffer.read_async(after_seq, max_bytes, wait_ms).await;
        let status = self
            .status
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        TerminalRead {
            chunks: slice.chunks,
            next_seq: slice.next_seq,
            truncated: slice.truncated,
            running: status.running,
            exit_code: status.exit_code,
        }
    }

    pub fn write(&self, data: &[u8]) -> Result<(), String> {
        if data.is_empty() {
            return Ok(());
        }
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        writer
            .write_all(data)
            .and_then(|()| writer.flush())
            .map_err(|error| format!("Cannot write to terminal: {error}"))
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        let cols = cols.clamp(1, 500);
        let rows = rows.clamp(1, 500);
        self.master
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("Cannot resize terminal: {error}"))?;
        *self.size.lock().unwrap_or_else(|error| error.into_inner()) = (cols, rows);
        Ok(())
    }

    /// Ask the shell to exit; the manager escalates to TERM/KILL if needed.
    pub(crate) fn hangup(&self) {
        #[cfg(unix)]
        self.signal_group(nix::sys::signal::Signal::SIGHUP);
        #[cfg(not(unix))]
        self.kill();
    }

    pub(crate) fn terminate(&self) {
        #[cfg(unix)]
        self.signal_group(nix::sys::signal::Signal::SIGTERM);
        #[cfg(not(unix))]
        self.kill();
    }

    pub(crate) fn kill(&self) {
        #[cfg(unix)]
        self.signal_group(nix::sys::signal::Signal::SIGKILL);
        // Keep the portable-pty killer as a fallback for hosts where the pid
        // is unavailable (or on Windows, where ConPTY has no process groups).
        let mut killer = self
            .killer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _ = killer.kill();
    }

    #[cfg(unix)]
    fn signal_group(&self, signal: nix::sys::signal::Signal) {
        let mut groups = Vec::new();
        if let Some(pid) = self.pid {
            groups.push(pid as i32);
        }
        // Interactive shells put foreground jobs (vim, htop, ...) in their own
        // process group. `tcgetpgrp` names that group, so closing a tab reaches
        // the job as well as the shell; background jobs are expected to be
        // reaped by the shell's own SIGHUP handling.
        if let Ok(master) = self.master.lock()
            && let Some(leader) = master.process_group_leader()
            && !groups.contains(&leader)
        {
            groups.push(leader);
        }
        for group in groups {
            let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(group), signal);
        }
    }
}

/// Give a batch of terminals a short grace period, then escalate. Used by both
/// explicit tab close and host shutdown so a foreground job cannot keep a
/// session alive after its tab disappears.
pub(crate) fn terminate_batch(terminals: Vec<Arc<Terminal>>) {
    if terminals.is_empty() {
        return;
    }
    for terminal in &terminals {
        terminal.hangup();
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(250));
        for terminal in &terminals {
            if terminal.running() {
                terminal.terminate();
            }
        }
        std::thread::sleep(Duration::from_millis(750));
        for terminal in &terminals {
            if terminal.running() {
                terminal.kill();
            }
        }
    });
}

/// Async variant for host shutdown (no blocking sleep on the runtime).
pub(crate) async fn terminate_batch_async(terminals: Vec<Arc<Terminal>>) {
    if terminals.is_empty() {
        return;
    }
    for terminal in &terminals {
        terminal.hangup();
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    for terminal in &terminals {
        if terminal.running() {
            terminal.terminate();
        }
    }
    tokio::time::sleep(Duration::from_millis(750)).await;
    for terminal in &terminals {
        if terminal.running() {
            terminal.kill();
        }
    }
    // Give the waiter threads a moment to publish final status before the
    // manager drops its last references.
    tokio::time::sleep(Duration::from_millis(50)).await;
}
