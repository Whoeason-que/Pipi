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
