//! Terminal registry, keyed by canonical workspace directory.
//!
//! Terminals are workspace-owned rather than session-owned: every session of
//! every Agent that resolves to the same directory sees the same shells, and
//! switching sessions or remounting the UI never kills a process.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::buffer::DEFAULT_MAX_BUFFER_BYTES;
use crate::session::{Terminal, terminate_batch, terminate_batch_async};
use crate::types::{TerminalRead, TerminalSnapshot, TerminalSpec};

pub(crate) const MAX_TERMINALS_PER_WORKSPACE: usize = 8;

#[derive(Clone)]
pub struct TerminalManager {
    inner: Arc<Inner>,
}

struct Inner {
    workspaces: Mutex<HashMap<std::path::PathBuf, Vec<Arc<Terminal>>>>,
    accepting: AtomicBool,
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                workspaces: Mutex::new(HashMap::new()),
                accepting: AtomicBool::new(true),
            }),
        }
    }

    /// Start one shell in `workspace`. The caller is responsible for
    /// canonicalizing the directory so different spellings share the bucket.
    /// This performs blocking PTY setup; hosts call it on a blocking thread.
    pub fn open(&self, workspace: &Path, spec: TerminalSpec) -> Result<TerminalSnapshot, String> {
        if !self.inner.accepting.load(Ordering::Acquire) {
            return Err("Application is shutting down; terminals cannot be started".into());
        }
        let mut workspaces = self
            .inner
            .workspaces
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let terminals = workspaces.entry(workspace.to_path_buf()).or_default();
        if terminals.len() >= MAX_TERMINALS_PER_WORKSPACE {
            // Reclaim a finished tab before rejecting the request; users often
            // restart a shell instead of closing it first.
            if let Some(index) = terminals.iter().position(|terminal| !terminal.running()) {
                terminals.remove(index);
            } else {
                return Err(format!(
                    "This workspace already has {MAX_TERMINALS_PER_WORKSPACE} open terminals; close one first"
                ));
            }
        }
        let id = format!("term-{}", uuid::Uuid::new_v4());
        let terminal = Arc::new(Terminal::spawn(
            id,
            workspace.to_path_buf(),
            spec,
            DEFAULT_MAX_BUFFER_BYTES,
        )?);
        let snapshot = terminal.snapshot();
        terminals.push(terminal);
        Ok(snapshot)
    }

    pub fn list(&self, workspace: &Path) -> Vec<TerminalSnapshot> {
        let workspaces = self
            .inner
            .workspaces
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        workspaces
            .get(workspace)
            .map(|terminals| {
                terminals
                    .iter()
                    .map(|terminal| terminal.snapshot())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn get(&self, workspace: &Path, terminal_id: &str) -> Result<Arc<Terminal>, String> {
        let workspaces = self
            .inner
            .workspaces
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        workspaces
            .get(workspace)
            .and_then(|terminals| {
                terminals
                    .iter()
                    .find(|terminal| terminal.id() == terminal_id)
            })
            .cloned()
            .ok_or_else(|| "Terminal does not exist in this workspace".to_string())
    }

    pub async fn read(
        &self,
        workspace: &Path,
        terminal_id: &str,
        after_seq: u64,
        max_bytes: usize,
        wait_ms: u64,
    ) -> Result<TerminalRead, String> {
        let terminal = self.get(workspace, terminal_id)?;
        Ok(terminal.read(after_seq, max_bytes, wait_ms).await)
    }

    pub async fn write(
        &self,
        workspace: &Path,
        terminal_id: &str,
        data: Vec<u8>,
    ) -> Result<(), String> {
        let terminal = self.get(workspace, terminal_id)?;
        tokio::task::spawn_blocking(move || terminal.write(&data))
            .await
            .map_err(|error| format!("Terminal write task failed: {error}"))?
    }

    pub fn resize(
        &self,
        workspace: &Path,
        terminal_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        self.get(workspace, terminal_id)?.resize(cols, rows)
    }

    pub fn close(&self, workspace: &Path, terminal_id: &str) -> Result<(), String> {
        let removed = {
            let mut workspaces = self
                .inner
                .workspaces
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(terminals) = workspaces.get_mut(workspace) else {
                return Err("Terminal does not exist in this workspace".into());
            };
            let Some(index) = terminals
                .iter()
                .position(|terminal| terminal.id() == terminal_id)
            else {
                return Err("Terminal does not exist in this workspace".into());
            };
            let removed = terminals.remove(index);
            if terminals.is_empty() {
                workspaces.remove(workspace);
            }
            removed
        };
        terminate_batch(vec![removed]);
        Ok(())
    }

    /// Close every terminal of a workspace after the last Agent using it goes
    /// away (archive/delete), so no shell outlives its working directory.
    pub fn close_workspace(&self, workspace: &Path) {
        let removed = {
            let mut workspaces = self
                .inner
                .workspaces
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            workspaces.remove(workspace).unwrap_or_default()
        };
        terminate_batch(removed);
    }

    /// Stop accepting terminals, signal every process group, then escalate.
    pub async fn shutdown(&self) {
        self.inner.accepting.store(false, Ordering::Release);
        let all = {
            let mut workspaces = self
                .inner
                .workspaces
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            workspaces
                .drain()
                .flat_map(|(_, terminals)| terminals)
                .collect::<Vec<_>>()
        };
        terminate_batch_async(all).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn terminal_ids_are_scoped_to_their_workspace() {
        let manager = TerminalManager::new();
        let workspace = std::env::temp_dir();
        let other = std::env::temp_dir().join("pipi-terminal-other");
        assert!(
            manager
                .read(&workspace, "term-missing", 0, 1024, 0)
                .await
                .is_err()
        );
        assert!(
            manager
                .write(&other, "term-missing", Vec::new())
                .await
                .is_err()
        );
        assert!(manager.list(&workspace).is_empty());
    }
}
