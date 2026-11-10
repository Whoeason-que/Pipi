//! Crash-recovery files. Complete entries keep the exact session JSONL format;
//! an optional assistant snapshot is separate because it is not yet an entry.
use super::exec::GitExec;
use pipi_protocol::Message;
use pipi_protocol::session::{SessionEntry, validate_session_id};
use std::io;
use std::path::PathBuf;

fn directory(exec: &GitExec, create: bool) -> io::Result<PathBuf> {
    let repo = exec.repo_dir();
    if !pipi_protocol::fs::ensure_real_directory(repo, "Session repository")
        .map_err(io::Error::other)?
    {
        return Err(io::Error::other("Session repository does not exist"));
    }
    let dir = repo.join("pipi-recovery");
    if create && !dir.exists() {
        std::fs::create_dir(&dir).or_else(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(e)
            }
        })?;
    }
    if create
        && !pipi_protocol::fs::ensure_real_directory(&dir, "Session recovery directory")
            .map_err(io::Error::other)?
    {
        return Err(io::Error::other(
            "Session recovery directory does not exist",
        ));
    }
    if !create
        && !pipi_protocol::fs::ensure_real_directory(&dir, "Session recovery directory")
            .map_err(io::Error::other)?
    {
        return Ok(dir);
    }
    Ok(dir)
}
fn path(exec: &GitExec, id: &str, suffix: &str, create: bool) -> io::Result<PathBuf> {
    validate_session_id(id).map_err(io::Error::other)?;
    Ok(directory(exec, create)?.join(format!("{id}.{suffix}")))
}
fn read(exec: &GitExec, id: &str, suffix: &str) -> io::Result<Option<Vec<u8>>> {
    let path = path(exec, id, suffix, false)?;
    if !pipi_protocol::fs::ensure_real_file(&path, "Session recovery file")
        .map_err(io::Error::other)?
    {
        return Ok(None);
    }
    std::fs::read(path).map(Some)
}
fn remove(exec: &GitExec, id: &str, suffix: &str) -> io::Result<()> {
    let path = path(exec, id, suffix, false)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
pub(super) fn write_entries(exec: &GitExec, id: &str, entries: &[SessionEntry]) -> io::Result<()> {
    if entries.is_empty() {
        return remove(exec, id, "jsonl");
    }
    let mut data = Vec::new();
    for entry in entries {
        serde_json::to_writer(&mut data, entry)?;
        data.push(b'\n');
    }
    let file = path(exec, id, "jsonl", true)?;
    pipi_protocol::fs::write_private_atomic(&file, data)?;
    sync_directory(&file)
}
pub(super) fn write_partial(
    exec: &GitExec,
    id: &str,
    parent: Option<&str>,
    message: &Message,
) -> io::Result<()> {
    let data = serde_json::to_vec(&serde_json::json!({"parentId": parent, "message": message}))?;
    let file = path(exec, id, "partial.json", true)?;
    pipi_protocol::fs::write_private_atomic(&file, data)?;
    sync_directory(&file)
}
fn sync_directory(file: &std::path::Path) -> io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(
        file.parent()
            .ok_or_else(|| io::Error::other("Missing recovery directory"))?,
    )?
    .sync_all()?;
    Ok(())
}
pub(super) fn has_files(exec: &GitExec, id: &str) -> io::Result<bool> {
    for suffix in ["jsonl", "partial.json"] {
        if pipi_protocol::fs::ensure_real_file(
            &path(exec, id, suffix, false)?,
            "Session recovery file",
        )
        .map_err(io::Error::other)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(super) fn clear_partial(exec: &GitExec, id: &str) -> io::Result<()> {
    remove(exec, id, "partial.json")
}
pub(super) fn clear(exec: &GitExec, id: &str) -> io::Result<()> {
    remove(exec, id, "jsonl")?;
    clear_partial(exec, id)
}
pub(super) fn load_partial(
    exec: &GitExec,
    id: &str,
    parent: Option<&str>,
) -> io::Result<Option<Message>> {
    let Some(bytes) = read(exec, id, "partial.json")? else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    if value.get("parentId").and_then(|v| v.as_str()) != parent {
        return Ok(None);
    }
    let message: Message = serde_json::from_value(
        value
            .get("message")
            .cloned()
            .ok_or_else(|| io::Error::other("Missing partial message"))?,
    )?;
    if !matches!(message, Message::Assistant { .. }) {
        return Err(io::Error::other("Invalid partial message"));
    }
    Ok(Some(message))
}
/// Verify overlap after a crash between commit and cleanup; never merge a stale
/// or divergent journal onto another branch tip.
pub(super) fn recover(
    exec: &GitExec,
    id: &str,
    committed: &[SessionEntry],
) -> Result<Vec<SessionEntry>, String> {
    let Some(bytes) = read(exec, id, "jsonl").map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    let mut pending = Vec::new();
    for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let entry: SessionEntry = serde_json::from_slice(line)
            .map_err(|e| format!("Invalid session recovery entry: {e}"))?;
        pending.push(entry);
    }
    let mut suffix = Vec::new();
    let mut previous = committed.last().map(|entry| entry.id.clone());
    let mut next_seq = committed.last().map_or(0, |entry| entry.seq + 1);
    for entry in pending {
        if entry.seq < next_seq {
            let existing = committed
                .iter()
                .find(|item| item.seq == entry.seq)
                .ok_or("Session recovery does not match committed history")?;
            if serde_json::to_vec(existing).ok() != serde_json::to_vec(&entry).ok() {
                return Err("Session recovery conflicts with committed history".into());
            }
            continue;
        }
        if entry.seq != next_seq || entry.parent_id != previous {
            return Err("Session recovery does not continue the current branch".into());
        }
        previous = Some(entry.id.clone());
        next_seq += 1;
        suffix.push(entry);
    }
    Ok(suffix)
}
