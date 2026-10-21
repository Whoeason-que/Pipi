use std::{fs::File, io::Read, path::Path};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::paths::{relative, resolve, root};

const TEXT_LIMIT: usize = 256 * 1024;
const MEDIA_LIMIT: usize = 8 * 1024 * 1024;
const ENTRY_LIMIT: usize = 2000;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEntry {
    pub path: String,
    pub name: String,
    /// directory / file / symlink / other；后两类不能打开。
    pub kind: String,
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListing {
    pub workspace: String,
    pub path: String,
    pub entries: Vec<WorkspaceEntry>,
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFile {
    pub workspace: String,
    pub path: String,
    pub size: u64,
    /// 完整读取内容的 SHA-256；截断时为空，不能当成完整文件版本引用。
    pub version: String,
    pub kind: String,
    pub mime: String,
    pub content: Option<String>,
    pub base64: Option<String>,
    pub truncated: bool,
}

pub fn list_files(agent: &str, input: &str) -> Result<WorkspaceListing, String> {
    list_at(&root(agent)?, input)
}

fn list_at(root: &Path, input: &str) -> Result<WorkspaceListing, String> {
    let dir = resolve(root, input, false)?;
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("无法读取目录：{e}"))? {
        let entry = entry.map_err(|e| format!("无法读取目录项：{e}"))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name == ".git" {
            continue;
        }
        if entries.len() == ENTRY_LIMIT {
            truncated = true;
            break;
        }
        let meta = std::fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
        let kind = if meta.file_type().is_symlink() {
            "symlink"
        } else if meta.is_dir() {
            "directory"
        } else if meta.is_file() {
            "file"
        } else {
            "other"
        };
        entries.push(WorkspaceEntry {
            path: relative(root, &entry.path())?,
            name,
            kind: kind.into(),
            size: meta.len(),
        });
    }
    entries.sort_by(|a, b| {
        (a.kind != "directory", a.name.to_lowercase())
            .cmp(&(b.kind != "directory", b.name.to_lowercase()))
    });
    Ok(WorkspaceListing {
        workspace: root.to_string_lossy().into_owned(),
        path: relative(root, &dir)?,
        entries,
        truncated,
    })
}

pub fn preview_file(agent: &str, input: &str) -> Result<WorkspaceFile, String> {
    preview_at(&root(agent)?, input)
}

pub(super) fn preview_at(root: &Path, input: &str) -> Result<WorkspaceFile, String> {
    let path = resolve(root, input, false)?;
    // 只读取普通文件，避免 FIFO / 设备阻塞线程。打开后再次验证边界和文件身份。
    if !std::fs::symlink_metadata(&path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("只能预览普通文件".into());
    }
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // 检查与打开之间被替换成链接/FIFO 时也必须拒绝，不能挂住阻塞线程。
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(&path)
        .map_err(|e| format!("无法打开文件：{e}"))?;
    let before = file.metadata().map_err(|e| e.to_string())?;
    if !before.is_file() {
        return Err("只能预览普通文件".into());
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let opened = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .map_err(|e| e.to_string())?;
        if !opened.starts_with(root) {
            return Err("打开的文件已逃出工作目录".into());
        }
    }
    let mut bytes = Vec::new();
    // 先嗅探格式，再决定预算。媒体只允许惰性、安全的内嵌展示，不执行 HTML/SVG。
    let mut reader = file.take((MEDIA_LIMIT + 1) as u64);
    let mut head = [0u8; 512];
    let n = reader.read(&mut head).map_err(|e| e.to_string())?;
    bytes.extend_from_slice(&head[..n]);
    let (kind, mime) = media_type(&path, &bytes);
    let limit = if kind.is_some() {
        MEDIA_LIMIT
    } else {
        TEXT_LIMIT
    };
    reader
        .take((limit + 1).saturating_sub(bytes.len()) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    resolve(root, input, false)?;
    let after = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return Err("文件在读取时被替换，请刷新预览".into());
        }
    }
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err("文件在读取时发生变化，请刷新预览".into());
    }
    let truncated = bytes.len() > limit || before.len() > limit as u64;
    bytes.truncate(limit);
    let version = if truncated {
        String::new()
    } else {
        Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let mut preview = WorkspaceFile {
        workspace: root.to_string_lossy().into_owned(),
        path: relative(root, &path)?,
        size: before.len(),
        version,
        kind: kind.unwrap_or("binary").into(),
        mime: mime.into(),
        content: None,
        base64: None,
        truncated,
    };
    if kind.is_some() {
        if !truncated {
            preview.base64 = Some(STANDARD.encode(bytes))
        }
    } else if !bytes.contains(&0) {
        let valid = match std::str::from_utf8(&bytes) {
            Ok(s) => Some(s),
            Err(e) if truncated && e.error_len().is_none() => {
                std::str::from_utf8(&bytes[..e.valid_up_to()]).ok()
            }
            Err(_) => None,
        };
        if let Some(text) = valid {
            preview.kind = if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("md" | "mdx" | "markdown")
            ) {
                "markdown"
            } else {
                "text"
            }
            .into();
            preview.mime = "text/plain".into();
            preview.content = Some(text.to_owned());
        }
    }
    Ok(preview)
}

fn media_type(path: &Path, bytes: &[u8]) -> (Option<&'static str>, &'static str) {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return (Some("image"), "image/png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return (Some("image"), "image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return (Some("image"), "image/gif");
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return (Some("image"), "image/webp");
    }
    if bytes.starts_with(b"%PDF-") {
        return (Some("pdf"), "application/pdf");
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "mp3" if bytes.starts_with(b"ID3") || bytes.starts_with(b"\xff\xfb") => {
            (Some("audio"), "audio/mpeg")
        }
        "wav" if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") => {
            (Some("audio"), "audio/wav")
        }
        "ogg" if bytes.starts_with(b"OggS") => (Some("audio"), "audio/ogg"),
        "mp4" if bytes.get(4..8) == Some(b"ftyp") => (Some("video"), "video/mp4"),
        "webm" if bytes.starts_with(b"\x1a\x45\xdf\xa3") => (Some("video"), "video/webm"),
        _ => (None, "application/octet-stream"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_limits_preserve_utf8_and_complete_files_have_stable_versions() {
        let root = pipi_test_support::temp_dir("preview-text");
        let text = "中文\n".repeat(TEXT_LIMIT);
        std::fs::write(root.join("large.md"), text).unwrap();
        let large = preview_at(&root, "large.md").unwrap();
        assert!(large.truncated);
        assert!(large.version.is_empty());
        assert!(!large.content.unwrap().contains('\u{fffd}'));
        std::fs::write(root.join("small.txt"), "original").unwrap();
        let one = preview_at(&root, "small.txt").unwrap();
        assert_eq!(one.version, preview_at(&root, "small.txt").unwrap().version);
        std::fs::write(root.join("small.txt"), "changed").unwrap();
        assert_ne!(one.version, preview_at(&root, "small.txt").unwrap().version);
    }

    #[test]
    fn binary_and_html_do_not_become_executable_documents() {
        let root = pipi_test_support::temp_dir("preview-formats");
        std::fs::write(root.join("fake.png"), "<script>alert(1)</script>").unwrap();
        assert_eq!(preview_at(&root, "fake.png").unwrap().kind, "text");
        std::fs::write(root.join("binary"), [0, 1, 2]).unwrap();
        assert_eq!(preview_at(&root, "binary").unwrap().kind, "binary");
        std::fs::write(root.join("picture"), b"\x89PNG\r\n\x1a\nfixture").unwrap();
        let image = preview_at(&root, "picture").unwrap();
        assert_eq!(image.kind, "image");
        assert!(image.content.is_none());
        assert!(image.base64.is_some());
        std::fs::create_dir(root.join(".git")).unwrap();
        assert!(
            list_at(&root, "")
                .unwrap()
                .entries
                .iter()
                .all(|e| e.name != ".git")
        );
    }

    #[test]
    fn oversize_media_is_not_transferred_as_a_partial_blob() {
        use std::io::Write;
        let root = pipi_test_support::temp_dir("preview-media-limit");
        let mut file = File::create(root.join("large.png")).unwrap();
        file.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
        file.set_len((MEDIA_LIMIT + 1) as u64).unwrap();
        drop(file);
        let preview = preview_at(&root, "large.png").unwrap();
        assert!(preview.truncated);
        assert!(preview.base64.is_none());
        assert!(preview.version.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn special_files_are_rejected_without_reading_them() {
        let root = pipi_test_support::temp_dir("preview-special");
        let socket = std::os::unix::net::UnixListener::bind(root.join("socket")).unwrap();
        assert!(
            preview_at(&root, "socket")
                .unwrap_err()
                .contains("普通文件")
        );
        drop(socket);
        std::fs::remove_dir_all(root).unwrap();
    }
}
