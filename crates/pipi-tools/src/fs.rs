//! 受管数据树的文件系统守卫。
//!
//! Agent 数据树（`~/.pipi/agents/**`）「文件即真相」的前提是路径本身可信：这些
//! 目录对用户可写，任何一环被换成符号链接，读写就会逃出数据树。打开、归档、
//! 读取前先过这两道检查；路径不存在返回 `Ok(false)`，由调用方决定是创建还是
//! 报错（fail-closed 的时机在调用方）。
//!
//! 与 [`crate::fs_atomic`] 的分工：那边负责写入的原子性，这边负责路径的可信性。

use std::fs;
use std::path::Path;

/// 验证路径是真实目录（不是符号链接）。不存在返回 `Ok(false)`。
pub fn ensure_real_directory(path: &Path, label: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("{label} 不能是符号链接: {}", path.display()))
        }
        Ok(metadata) if !metadata.is_dir() => Err(format!("{label} 不是目录: {}", path.display())),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("无法检查 {label} {}: {error}", path.display())),
    }
}

/// 验证路径是普通文件（不是符号链接）。不存在返回 `Ok(false)`。
pub fn ensure_real_file(path: &Path, label: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("{label} 不能是符号链接: {}", path.display()))
        }
        Ok(metadata) if !metadata.is_file() => {
            Err(format!("{label} 不是普通文件: {}", path.display()))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("无法检查 {label} {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_paths_report_absent_instead_of_failing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!ensure_real_file(&dir.path().join("nope.jsonl"), "会话文件").unwrap());
        assert!(!ensure_real_directory(&dir.path().join("nope"), "目录").unwrap());
    }

    #[test]
    fn wrong_kind_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(ensure_real_file(&file, "文件").unwrap());
        assert!(ensure_real_directory(&file, "目录").is_err());
        assert!(ensure_real_file(dir.path(), "文件").is_err());
        assert!(ensure_real_directory(dir.path(), "目录").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(ensure_real_directory(&link, "目录").is_err());
        assert!(ensure_real_file(&link, "文件").is_err());
    }
}
