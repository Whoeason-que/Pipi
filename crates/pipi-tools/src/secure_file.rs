//! Handle-relative workspace writes. No component is followed after validation.

use std::path::PathBuf;

use pipi_protocol::permissions::{SandboxMode, resolve_write_path};
use pipi_protocol::tools::{ToolContext, resolve_path};

/// write / edit 的写入目标：`(绝对路径, 是否必须走 `SecureFile`)`。
///
/// 三档语义与 `ToolContext::ensure_writable` 一致：`workspace-write` 走
/// canonical containment；`read-only` 被拒绝；`danger-full-access` 直通。
pub(crate) fn mutation_target(ctx: &ToolContext, path: &str) -> Result<(PathBuf, bool), String> {
    if matches!(ctx.sandbox, SandboxMode::WorkspaceWrite) {
        resolve_write_path(&ctx.workspace, path).map(|abs| (abs, true))
    } else {
        let abs = resolve_path(&ctx.workspace, path);
        ctx.ensure_writable(&abs)?;
        Ok((abs, false))
    }
}

#[cfg(unix)]
mod unix {
    use rustix::fs::{self, Mode, OFlags};
    use std::ffi::{OsStr, OsString};
    use std::io::{Read, Write};
    use std::os::fd::OwnedFd;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Component, Path};

    pub fn open_read(root: &Path, target: &Path) -> Result<std::fs::File, String> {
        let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        let relative = target
            .strip_prefix(&root)
            .map_err(|_| "Path is outside read root".to_string())?;
        let mut dir = fs::open(
            &root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| e.to_string())?;
        let components = relative.components().collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            let Component::Normal(name) = component else {
                return Err("Invalid path component".into());
            };
            let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
            if index + 1 != components.len() {
                flags |= OFlags::DIRECTORY;
            }
            dir = fs::openat(&dir, *name, flags, Mode::empty()).map_err(|e| e.to_string())?;
        }
        Ok(std::fs::File::from(dir))
    }

    pub struct SecureFile {
        dir: OwnedFd,
        name: OsString,
    }
    impl SecureFile {
        pub fn open(root: &Path, target: &Path, create_parents: bool) -> Result<Self, String> {
            let root =
                std::fs::canonicalize(root).map_err(|e| format!("Cannot open workspace: {e}"))?;
            let relative = target
                .strip_prefix(&root)
                .map_err(|_| "Path is outside workspace".to_string())?;
            let mut components = relative.components().collect::<Vec<_>>();
            let Some(Component::Normal(name)) = components.pop() else {
                return Err("Expected a file path".into());
            };
            let mut dir = fs::open(
                &root,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| e.to_string())?;
            for component in components {
                let Component::Normal(name) = component else {
                    return Err("Invalid path component".into());
                };
                let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
                let next = match fs::openat(&dir, name, flags, Mode::empty()) {
                    Ok(fd) => fd,
                    Err(rustix::io::Errno::NOENT) if create_parents => {
                        match fs::mkdirat(&dir, name, Mode::from_raw_mode(0o777)) {
                            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                            Err(error) => return Err(error.to_string()),
                        }
                        fs::openat(&dir, name, flags, Mode::empty()).map_err(|e| e.to_string())?
                    }
                    Err(error) => return Err(error.to_string()),
                };
                dir = next;
            }
            Ok(Self {
                dir,
                name: name.to_os_string(),
            })
        }
        fn open_file(&self) -> Result<std::fs::File, String> {
            let fd = fs::openat(
                &self.dir,
                &self.name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| e.to_string())?;
            let file = std::fs::File::from(fd);
            if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                return Err("Path is not a regular file".into());
            }
            Ok(file)
        }
        pub fn read(&self, max_bytes: usize) -> Result<String, String> {
            let file = self.open_file()?;
            let mut bytes = Vec::new();
            file.take(max_bytes as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > max_bytes {
                return Err(format!("File exceeds {max_bytes} bytes"));
            }
            String::from_utf8(bytes).map_err(|e| e.to_string())
        }
        pub fn write(
            &self,
            content: &str,
            abort: &pipi_protocol::AbortSignal,
        ) -> Result<(), String> {
            let existing_mode = match self.open_file() {
                Ok(file) => Some(
                    file.metadata()
                        .map_err(|e| e.to_string())?
                        .permissions()
                        .mode(),
                ),
                Err(error) => {
                    match fs::statat(&self.dir, &self.name, fs::AtFlags::SYMLINK_NOFOLLOW) {
                        Err(rustix::io::Errno::NOENT) => None,
                        _ => return Err(error),
                    }
                }
            };
            let temp_name = format!(".pipi-{}", pipi_protocol::new_id());
            let temp = OsStr::new(&temp_name);
            let fd = fs::openat(
                &self.dir,
                temp,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(existing_mode.unwrap_or(0o666)),
            )
            .map_err(|e| e.to_string())?;
            let result = (|| {
                let mut file = std::fs::File::from(fd);
                file.write_all(content.as_bytes())
                    .map_err(|e| e.to_string())?;
                if let Some(mode) = existing_mode {
                    file.set_permissions(std::fs::Permissions::from_mode(mode))
                        .map_err(|e| e.to_string())?;
                }
                file.sync_all().map_err(|e| e.to_string())?;
                if abort.is_aborted() {
                    return Err("Operation aborted before writing file".into());
                }
                fs::renameat(&self.dir, temp, &self.dir, &self.name).map_err(|e| e.to_string())?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::unlinkat(&self.dir, temp, fs::AtFlags::empty());
            }
            result
        }
    }
}
#[cfg(unix)]
pub use unix::{SecureFile, open_read};

#[cfg(not(unix))]
pub fn open_read(_: &std::path::Path, _: &std::path::Path) -> Result<std::fs::File, String> {
    Err("Secure workspace file operations are unavailable on this platform".into())
}
#[cfg(not(unix))]
pub struct SecureFile;
#[cfg(not(unix))]
impl SecureFile {
    pub fn open(_: &std::path::Path, _: &std::path::Path, _: bool) -> Result<Self, String> {
        Err("Secure workspace file operations are unavailable on this platform".into())
    }
    pub fn read(&self, _: usize) -> Result<String, String> {
        Err("Secure workspace file operations are unavailable on this platform".into())
    }
    pub fn write(&self, _: &str, _: &pipi_protocol::AbortSignal) -> Result<(), String> {
        Err("Secure workspace file operations are unavailable on this platform".into())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn parent_swap_cannot_redirect_a_validated_write() {
        let root = pipi_test_support::temp_dir("secure-file-swap");
        let workspace = root.join("workspace");
        let inside = workspace.join("dir");
        let outside = root.join("outside");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(inside.join("file"), "inside").unwrap();
        std::fs::write(outside.join("file"), "outside").unwrap();
        let target =
            pipi_protocol::permissions::resolve_write_path(&workspace, "dir/file").unwrap();
        let file = SecureFile::open(&workspace, &target, false).unwrap();
        std::fs::rename(&inside, workspace.join("moved")).unwrap();
        symlink(&outside, &inside).unwrap();
        file.write("safe", &pipi_protocol::AbortSignal::new())
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(outside.join("file")).unwrap(),
            "outside"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("moved/file")).unwrap(),
            "safe"
        );
        assert!(SecureFile::open(&workspace, &target, false).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn aborted_write_preserves_original() {
        let root = pipi_test_support::temp_dir("secure-file-abort");
        let path = root.join("file");
        std::fs::write(&path, "original").unwrap();
        let file = SecureFile::open(&root, &path, false).unwrap();
        let abort = pipi_protocol::AbortSignal::new();
        abort.abort();
        assert!(file.write("changed", &abort).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
        std::fs::remove_dir_all(root).unwrap();
    }
}
