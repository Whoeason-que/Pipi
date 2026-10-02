//! 原子文件写入：先写同目录临时文件、fsync，再 rename 覆盖。
//!
//! 只对「已存在的文件」做原子替换 —— 覆盖是最容易丢数据的一步（崩溃/断电会留下
//! 半截内容，把好文件也一起毁掉）。新建文件走普通写入：没有旧内容可丢，且权限
//! 遵循进程 umask（`tempfile` 建出来的临时文件是 0600，直接 persist 会让 Agent
//! 写出的源码文件带上 0600，不是我们要的语义）。

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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pipi-fs-{name}-{}", crate::session::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn replaces_existing_file_atomically() {
        let dir = temp_dir("replace");
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
        let dir = temp_dir("create");
        let path = dir.join("nested/deep/b.txt");
        write_atomic(&path, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn preserves_mode_of_existing_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("mode");
        let path = dir.join("script.sh");
        std::fs::write(&path, "#!/bin/sh\necho old\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_atomic(&path, "#!/bin/sh\necho new\n").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "覆盖写不应改变原文件权限");
        std::fs::remove_dir_all(&dir).ok();
    }
}
