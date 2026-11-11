//! 宿主侧的终端装配与 wire 编码。
//!
//! 终端本身是 `pipi-terminal` 的领域：这里只负责把 Agent / 工作区解析成
//! canonical 启动目录、挑选用户 shell、补环境，并把 PTY 字节编码成 base64
//! JSON 字段。**用户终端不是 Agent 可控执行**，所以不经过 `pipi_sandbox`；
//! 沙箱约束的是模型，不是坐在键盘前的人。

use std::path::{Path, PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD};
use pipi_terminal::{TerminalRead, TerminalSpec};
use pipi_venv::agents;
use serde::{Deserialize, Serialize};

pub const DEFAULT_COLS: u16 = 120;
pub const DEFAULT_ROWS: u16 = 32;
pub const DEFAULT_READ_BYTES: usize = 256 * 1024;
pub const MAX_READ_BYTES: usize = 512 * 1024;
pub const DEFAULT_WAIT_MS: u64 = 500;
pub const MAX_WAIT_MS: u64 = 5000;
pub const MAX_WRITE_BYTES: usize = 1024 * 1024;

/// Resolve the directory a terminal belongs to.
///
/// An explicit `workspace` (the navigation page's selector) wins over the
/// Agent's default; both are canonicalized so the same directory spelled two
/// ways shares one terminal bucket. This matches the Files/Git commands, which
/// also accept a direct workspace path.
pub fn resolve_workspace(agent_name: &str, workspace: Option<&str>) -> Result<PathBuf, String> {
    let definition = agents::load_agent(agent_name)?;
    let requested = workspace
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(agents::expand_tilde(value)))
        .or_else(|| definition.resolve_workspace())
        .ok_or_else(|| "Cannot resolve Agent working directory".to_string())?;
    let root = requested
        .canonicalize()
        .map_err(|error| format!("Terminal working directory is not available: {error}"))?;
    if !root.is_dir() {
        return Err("Terminal working directory is not a directory".into());
    }
    Ok(root)
}

/// Startup description for the user's shell. The host inherits its own
/// environment through portable-pty and layers these entries on top.
pub fn shell_spec(cols: u16, rows: u16) -> TerminalSpec {
    let (program, args) = shell_program();
    TerminalSpec {
        program,
        args,
        env: shell_env(),
        cols,
        rows,
    }
}

#[cfg(unix)]
fn shell_program() -> (String, Vec<String>) {
    let from_env = std::env::var("SHELL").ok().filter(|value| {
        let path = Path::new(value);
        path.is_absolute() && path.is_file()
    });
    let program = from_env.unwrap_or_else(|| {
        ["/bin/zsh", "/bin/bash", "/bin/sh"]
            .into_iter()
            .find(|path| Path::new(path).is_file())
            .unwrap_or("/bin/sh")
            .to_string()
    });
    // Login shells read the user's profile (PATH, toolchains). Known POSIX
    // shells accept -l; other programs get no arguments rather than a guess.
    let login = Path::new(&program)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "bash" | "zsh" | "sh" | "dash" | "ksh" | "fish"));
    let args = if login {
        vec!["-l".to_string()]
    } else {
        Vec::new()
    };
    (program, args)
}

#[cfg(not(unix))]
fn shell_program() -> (String, Vec<String>) {
    // ConPTY is handled by portable-pty; COMSPEC is the user's preferred
    // command interpreter on Windows.
    (
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()),
        Vec::new(),
    )
}

fn shell_env() -> Vec<(String, String)> {
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut env = vec![
        ("TERM".to_string(), "xterm-256color".to_string()),
        ("COLORTERM".to_string(), "truecolor".to_string()),
        ("PIPI_TERMINAL".to_string(), "1".to_string()),
    ];
    // A macOS GUI launch inherits launchd's minimal PATH, not the shell
    // profile's. Keep Homebrew/system locations reachable even before the
    // login shell has a chance to adjust PATH.
    #[cfg(target_os = "macos")]
    {
        let current = std::env::var("PATH").unwrap_or_default();
        let mut parts: Vec<String> = current
            .split(':')
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect();
        for path in [
            "/opt/homebrew/bin",
            "/opt/homebrew/sbin",
            "/usr/local/bin",
            "/usr/local/sbin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ] {
            if !parts.iter().any(|part| part == path) {
                parts.push(path.to_string());
            }
        }
        env.push(("PATH".to_string(), parts.join(":")));
    }
    env
}

/// One output chunk on the JSON wire. `data` is base64 because PTY bytes are
/// not necessarily valid UTF-8 at a chunk boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalChunkView {
    pub seq: u64,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalReadView {
    pub chunks: Vec<TerminalChunkView>,
    pub next_seq: u64,
    pub truncated: bool,
    pub running: bool,
    pub exit_code: Option<i32>,
}

impl From<TerminalRead> for TerminalReadView {
    fn from(read: TerminalRead) -> Self {
        Self {
            chunks: read
                .chunks
                .into_iter()
                .map(|chunk| TerminalChunkView {
                    seq: chunk.seq,
                    data: STANDARD.encode(chunk.data),
                })
                .collect(),
            next_seq: read.next_seq,
            truncated: read.truncated,
            running: read.running,
            exit_code: read.exit_code,
        }
    }
}

pub fn decode_input(data: &str) -> Result<Vec<u8>, String> {
    let bytes = STANDARD
        .decode(data)
        .map_err(|error| format!("Invalid terminal input: {error}"))?;
    if bytes.len() > MAX_WRITE_BYTES {
        return Err(format!(
            "Terminal input is too large ({} byte limit)",
            MAX_WRITE_BYTES
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_roundtrip_keeps_raw_bytes_and_rejects_garbage() {
        let bytes = b"\x1b[A\xe4\xbd\xa0\xe5\xa5\xbd\n";
        let encoded = STANDARD.encode(bytes);
        assert_eq!(decode_input(&encoded).unwrap(), bytes);
        assert!(decode_input("not base64!").is_err());
    }

    #[test]
    fn read_view_encodes_chunks_as_base64() {
        let read = TerminalRead {
            chunks: vec![pipi_terminal::TerminalChunk {
                seq: 4,
                data: b"ok".to_vec(),
            }],
            next_seq: 5,
            truncated: true,
            running: true,
            exit_code: None,
        };
        let view = TerminalReadView::from(read);
        assert_eq!(view.chunks[0].seq, 4);
        assert_eq!(STANDARD.decode(&view.chunks[0].data).unwrap(), b"ok");
        assert!(view.truncated);
    }
}
