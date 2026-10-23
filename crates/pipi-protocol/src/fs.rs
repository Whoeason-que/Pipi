//! 文件系统原语：原子写入 + 受管数据树的路径守卫。
//!
//! - [`write_atomic`]：覆盖写先写同目录临时文件、fsync，再 rename —— 只对
//!   「已存在的文件」做原子替换（覆盖是最容易丢数据的一步），新建文件走普通
//!   写入（没有旧内容可丢，且权限遵循进程 umask）。
//! - [`ensure_real_file`] / [`ensure_real_directory`]：Agent 数据树
//!   （`~/.pipi/agents/**`）「文件即真相」的前提是路径本身可信 —— 这些目录对
//!   用户可写，任何一环被换成符号链接，读写就会逃出数据树。不存在返回
//!   `Ok(false)`，由调用方决定是创建还是报错（fail-closed 的时机在调用方）。
//!
//! 放在契约层的原因：会话载体、Agent 注册表、设置、模型目录都要用它们，
//! 而这些 crate 之间不该互相依赖。

use std::fs;
use std::io::Write;
use std::path::Path;

/// 原子写入 `path`。父目录不存在时自动创建。
pub fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let contents = contents.as_ref();
    let parent = path.parent().filter(|dir| !dir.as_os_str().is_empty());
    if let Some(dir) = parent {
        std::fs::create_dir_all(dir)?;
    }
    if !path.exists() {
        return std::fs::write(path, contents);
    }

    let dir = parent.unwrap_or_else(|| Path::new("."));
    let existing = std::fs::metadata(path)?.permissions();
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(contents)?;
    tmp.as_file().sync_all()?;
    tmp.as_file().set_permissions(existing)?;
    tmp.persist(path)?;
    Ok(())
}

/// 验证路径是真实目录（不是符号链接）。不存在返回 `Ok(false)`。
pub fn ensure_real_directory(path: &Path, label: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("{label} cannot be a symbolic link: {}", path.display()))
        }
        Ok(metadata) if !metadata.is_dir() => Err(format!("{label} is not a directory: {}", path.display())),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Failed to check {label} {}: {error}", path.display())),
    }
}

/// 验证路径是普通文件（不是符号链接）。不存在返回 `Ok(false)`。
pub fn ensure_real_file(path: &Path, label: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("{label} cannot be a symbolic link: {}", path.display()))
        }
        Ok(metadata) if !metadata.is_file() => {
            Err(format!("{label} is not a regular file: {}", path.display()))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Failed to check {label} {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_file_atomically() {
        let dir = pipi_test_support::temp_dir("pipi-fs-replace");
        let path = dir.join("a.txt");
        std::fs::write(&path, "old").unwrap();
        write_atomic(&path, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        // 目录里不留临时文件
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "a.txt")
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件: {leftovers:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn creates_parent_directories_and_new_file() {
        let dir = pipi_test_support::temp_dir("pipi-fs-create");
        let path = dir.join("nested/deep/b.txt");
        write_atomic(&path, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn preserves_mode_of_existing_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = pipi_test_support::temp_dir("pipi-fs-mode");
        let path = dir.join("script.sh");
        std::fs::write(&path, "#!/bin/sh\necho old\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_atomic(&path, "#!/bin/sh\necho new\n").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "覆盖写不应改变原文件权限");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_paths_report_absent_instead_of_failing() {
        let dir = pipi_test_support::temp_dir("pipi-fs-guard-missing");
        assert!(!ensure_real_file(&dir.join("nope.jsonl"), "会话文件").unwrap());
        assert!(!ensure_real_directory(&dir.join("nope"), "目录").unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wrong_kind_is_rejected() {
        let dir = pipi_test_support::temp_dir("pipi-fs-guard-kind");
        let file = dir.join("file.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(ensure_real_file(&file, "文件").unwrap());
        assert!(ensure_real_directory(&file, "目录").is_err());
        assert!(ensure_real_file(&dir, "文件").is_err());
        assert!(ensure_real_directory(&dir, "目录").unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_rejected() {
        let dir = pipi_test_support::temp_dir("pipi-fs-guard-link");
        let target = dir.join("target");
        std::fs::create_dir(&target).unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(ensure_real_directory(&link, "目录").is_err());
        assert!(ensure_real_file(&link, "文件").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
