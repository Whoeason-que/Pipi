//! 文件系统原语：原子写入 + 受管数据树的路径守卫。
//!
//! - [`write_atomic`]：覆盖写先写同目录临时文件、fsync，再 rename —— 只对
//!   「已存在的文件」做原子替换（覆盖是最容易丢数据的一步），新建文件走普通
//!   写入（没有旧内容可丢，且权限遵循进程 umask）。
//! - [`ensure_real_file`] / [`ensure_real_directory`]：Agent 数据树
//!   （`~/.pipi/agents/**`）「文件即真相」的前提是路径本身可信 —— 这些目录对
//!   用户可写，任何一环被换成符号链接，读写就会逃出数据树。不存在返回
//!   `Ok(false)`，由调用方决定是创建还是报错（fail-closed 的时机在调用方）。

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

use std::fs;

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
mod tests;
