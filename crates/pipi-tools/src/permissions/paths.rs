//! 写入路径的 canonical containment：解析最近现有祖先、拒绝悬空 symlink 与
//! 指向工作区外的目标。

use std::fs;
use std::path::{Path, PathBuf};

/// 解析 workspace-write 的目标，先 canonicalize 工作目录与最近的现有祖先，
/// 再拼回不存在的尾部；悬空 symlink 和指向外部的 symlink 都 fail-closed。
pub fn resolve_write_path(workspace: &Path, path: &str) -> Result<PathBuf, String> {
    let requested_path = Path::new(path);
    let requested = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        workspace.join(requested_path)
    };
    resolve_write_target(workspace, &requested)
}

/// `resolve_write_path` 的 Path 版本，供工具在已解析路径上复用。
pub(crate) fn resolve_write_target(workspace: &Path, requested: &Path) -> Result<PathBuf, String> {
    let canonical_workspace = fs::canonicalize(workspace)
        .map_err(|e| format!("无法验证工作目录 {}：{e}", workspace.display()))?;
    let canonical_target = canonicalize_existing_ancestor(requested)?;
    if canonical_target == canonical_workspace || canonical_target.starts_with(&canonical_workspace)
    {
        Ok(canonical_target)
    } else {
        Err(format!(
            "目标 {} 不在工作目录 {} 内",
            canonical_target.display(),
            canonical_workspace.display()
        ))
    }
}

fn canonicalize_existing_ancestor(path: &Path) -> Result<PathBuf, String> {
    let mut missing = Vec::new();
    let mut existing = path;
    loop {
        match fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = existing
                    .file_name()
                    .ok_or_else(|| format!("无法解析写入路径 {}", path.display()))?;
                missing.push(name.to_os_string());
                existing = existing
                    .parent()
                    .ok_or_else(|| format!("无法解析写入路径 {}", path.display()))?;
            }
            Err(error) => {
                return Err(format!("无法解析写入路径 {}：{error}", path.display()));
            }
        }
    }

    let mut canonical = fs::canonicalize(existing)
        .map_err(|e| format!("无法解析写入路径 {}：{e}", path.display()))?;
    for name in missing.iter().rev() {
        canonical.push(name);
    }
    Ok(canonical)
}

/// 词法路径包含检查：解析 `.` / `..` 后判断前缀（不访问文件系统，
/// 目标文件可能还不存在）。
pub fn is_within(base: &std::path::Path, path: &std::path::Path) -> bool {
    fn components_vec(p: &std::path::Path) -> Vec<std::path::Component<'_>> {
        let mut stack: Vec<std::path::Component<'_>> = Vec::new();
        for c in p.components() {
            match c {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    stack.pop();
                }
                other => stack.push(other),
            }
        }
        stack
    }

    let base = components_vec(base);
    let path = components_vec(path);
    path.starts_with(&base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_containment() {
        let base = Path::new("/home/u/project");
        assert!(is_within(base, Path::new("/home/u/project/a/b.txt")));
        assert!(is_within(base, Path::new("/home/u/project/../project/x")));
        assert!(!is_within(base, Path::new("/home/u/other")));
        assert!(!is_within(base, Path::new("/home/u/project/../..")));
        assert!(!is_within(base, Path::new("/etc/passwd")));
    }
}
