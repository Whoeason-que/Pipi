use std::path::{Component, Path, PathBuf};

pub(super) fn root(agent: &str) -> Result<PathBuf, String> {
    let def = pipi_venv::agents::load_agent(agent)?;
    let path = def.resolve_workspace().ok_or("无法解析工作目录")?;
    let root = path
        .canonicalize()
        .map_err(|e| format!("无法打开工作目录：{e}"))?;
    if !root.is_dir() {
        return Err("工作目录不是目录".into());
    }
    Ok(root)
}

/// 相对路径与工作区内的绝对工具路径都接受；拒绝穿越、Git 内部文件及所有符号链接。
/// deleted 用于查看已删除文件的 Git 差异，允许末尾路径不存在。
pub(super) fn resolve(root: &Path, input: &str, deleted: bool) -> Result<PathBuf, String> {
    let input = Path::new(input);
    let relative = if input.is_absolute() {
        input
            .strip_prefix(root)
            .map_err(|_| "文件不在此 Agent 的工作目录内")?
    } else {
        input
    };
    let mut path = root.to_path_buf();
    for part in relative.components() {
        match part {
            Component::CurDir => continue,
            Component::Normal(name) if name != ".git" => path.push(name),
            _ => return Err("不允许路径穿越或访问 Git 内部文件".into()),
        }
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => return Err("预览不跟随符号链接".into()),
            Ok(_) => (),
            Err(e) if deleted && e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(format!("无法访问文件：{e}")),
        }
    }
    if path.exists()
        && !path
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(root)
    {
        return Err("文件不在此 Agent 的工作目录内".into());
    }
    Ok(path)
}

pub(super) fn relative(root: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .map(|s| s.replace(std::path::MAIN_SEPARATOR, "/"))
        .ok_or_else(|| "文件名无法转换为 UTF-8 工作区路径".into())
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
