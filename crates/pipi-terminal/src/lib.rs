//! User terminal domain: PTY lifecycle, bounded output replay and process-group
//! cleanup.
//!
//! This crate deliberately does not know about Agents, permissions, sandboxes
//! or the workspace layout: the host resolves the working directory, shell and
//! environment and passes a [`TerminalSpec`]. That mirrors `pipi-mcp`: the
//! protocol/OS domain lives here, assembly lives in `pipi-core`.
//!
//! A user terminal is **not** Agent-controlled execution. It is the user's own
//! shell, so it does not go through `pipi_sandbox`; the sandbox exists to
//! contain the model, not the person at the keyboard.

mod buffer;
mod manager;
mod session;
mod types;

pub use manager::TerminalManager;
pub use types::{TerminalChunk, TerminalRead, TerminalSnapshot, TerminalSpec};
