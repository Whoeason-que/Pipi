//! Terminal-domain values shared by the manager and the host bridge.
//!
//! Bytes are kept as raw `Vec<u8>` here; the host encodes output chunks as
//! base64 on the JSON wire (see `pipi-core::terminal::TerminalReadView`).

use serde::{Deserialize, Serialize};

/// How to start one user shell.
#[derive(Debug, Clone)]
pub struct TerminalSpec {
    /// Absolute shell program (the host resolves `$SHELL`).
    pub program: String,
    pub args: Vec<String>,
    /// Extra environment entries layered over the inherited process env.
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

/// Wire snapshot of one terminal. `workspace` is the canonical directory the
/// terminal was opened for; the shell may `cd` away afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSnapshot {
    pub terminal_id: String,
    pub workspace: String,
    pub shell: String,
    pub cols: u16,
    pub rows: u16,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub created_at: u64,
}

/// One ordered block of PTY output. `seq` is a chunk sequence number, never a
/// byte offset: the client stores the returned `next_seq` and replays from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalChunk {
    pub seq: u64,
    pub data: Vec<u8>,
}

/// Cursor read result. `next_seq` stays equal to the requested cursor when no
/// output is available, so an idle long poll does not skip anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRead {
    pub chunks: Vec<TerminalChunk>,
    pub next_seq: u64,
    pub truncated: bool,
    pub running: bool,
    pub exit_code: Option<i32>,
}

pub(crate) fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
