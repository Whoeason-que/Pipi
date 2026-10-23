use std::path::{Component, Path, PathBuf};

pub(super) fn root(agent_or_workspace: &str) -> Result<PathBuf, String> {
    if let Ok(def) = pipi_venv::agents::load_agent(agent_or_workspace) {
        if let Some(path) = def.resolve_workspace() {
            if let Ok(root) = path.canonicalize() {
                if root.is_dir() {
                    return Ok(root);
                }
            }
        }
    }
    let direct = PathBuf::from(agent_or_workspace);
    if let Ok(root) = direct.canonicalize() {
        if root.is_dir() {
            return Ok(root);
        }
    }
    Err("Cannot resolve workspace directory".into())
}

/// 相对路径与工作区内的绝对工具路径都接受；拒绝穿越、Git 内部文件及所有符号链接。
/// deleted 用于查看已删除文件的 Git 差异，允许末尾路径不存在。
pub(super) fn resolve(root: &Path, input: &str, deleted: bool) -> Result<PathBuf, String> {
    let input = Path::new(input);
    let relative = if input.is_absolute() {
        input
            .strip_prefix(root)
            .map_err(|_| "File is outside the workspace directory")?
    } else {
        input
    };
    let mut path = root.to_path_buf();
    for part in relative.components() {
        match part {
            Component::CurDir => continue,
            Component::Normal(name) if name != ".git" => path.push(name),
            _ => return Err("Path traversal or Git internal files are not allowed".into()),
        }
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => return Err("Preview does not follow symlinks".into()),
            Ok(_) => (),
            Err(e) if deleted && e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(format!("Cannot access file: {e}")),
        }
    }
    if path.exists()
        && !path
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(root)
    {
        return Err("File is outside the workspace directory".into());
    }
    Ok(path)
}

pub(super) fn relative(root: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .map(|s| s.replace(std::path::MAIN_SEPARATOR, "/"))
        .ok_or_else(|| "File name cannot be converted to UTF-8 workspace path".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escape_git_and_accepts_deleted_paths() {
        let root = pipi_test_support::temp_dir("preview-paths")
            .canonicalize()
            .unwrap();
        std::fs::write(root.join("中文.txt"), "ok").unwrap();
        assert!(resolve(&root, "../outside", true).is_err());
        assert!(resolve(&root, "/etc/passwd", false).is_err());
        assert!(resolve(&root, ".git/config", true).is_err());
        assert!(resolve(&root, "nested/.git/config", true).is_err());
        assert!(resolve(&root, "中文.txt", false).is_ok());
        assert!(resolve(&root, "deleted.txt", true).is_ok());
        assert!(resolve(&root, "deleted.txt", false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_file_and_directory_symlinks_even_inside_root() {
        let root = pipi_test_support::temp_dir("preview-links");
        std::fs::create_dir(root.join("dir")).unwrap();
        std::fs::write(root.join("dir/a"), "ok").unwrap();
        std::os::unix::fs::symlink(root.join("dir"), root.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("dir/a"), root.join("file")).unwrap();
        assert!(resolve(&root, "link/a", false).is_err());
        assert!(resolve(&root, "file", false).is_err());
    }
}
