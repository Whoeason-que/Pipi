use super::*;

#[test]
fn version_compare_semantics() {
    assert!(version_satisfies("node v20.11.1", ">=20").unwrap());
    assert!(version_satisfies("node v20.11.1", ">=19.9").unwrap());
    assert!(!version_satisfies("node v20.11.1", ">=21").unwrap());
    assert!(version_satisfies("git version 2.39.5", ">=2.39").unwrap());
    assert!(version_satisfies("git version 2.39.5", ">=2.38.999").unwrap());
    assert!(!version_satisfies("git version 2.39.5", ">=2.40").unwrap());
    // 精确比较：20 == 20.0.0
    assert!(version_satisfies("node v20", "20").unwrap());
    assert!(version_satisfies("node v20.0.0", "=20").unwrap());
    assert!(!version_satisfies("node v20.1", "20").unwrap());
    // 提取：取输出里的首个数字段
    assert!(version_satisfies("Python 3.12.1", ">=3.12").unwrap());
    assert!(version_satisfies("rustc 1.75.0 (abc)", ">=1.74").unwrap());
    // 无数字 / 空断言
    assert!(version_satisfies("no version here", ">=1").is_err());
    assert!(version_satisfies("node v20", "").is_err());
    assert!(version_satisfies("node v20", ">=abc").is_err());
}

#[test]
fn lookup_finds_executable_in_path() {
    let dir = pipi_test_support::temp_dir("av-requires");

    // 不存在：None
    assert!(lookup_command("av-test-cmd", Some("/nonexistent")).is_none());
    assert!(lookup_command("av-test-cmd", None).is_none());

    // 存在但不可执行（unix）：跳过
    let plain = dir.join("av-test-cmd");
    std::fs::write(&plain, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        assert!(lookup_command("av-test-cmd", Some(&dir.to_string_lossy())).is_none());
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(windows)]
    assert!(
        lookup_command("av-test-cmd", Some(&dir.to_string_lossy())).is_some(),
        "windows 下存在即可"
    );

    #[cfg(unix)]
    assert_eq!(
        lookup_command("av-test-cmd", Some(&dir.to_string_lossy())),
        Some(plain)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn check_requires_fails_closed_on_missing_command() {
    let env = BTreeMap::from([("PATH".to_string(), "/nonexistent-av".to_string())]);
    let requires = vec![RequiresEntry {
        command: "av-definitely-missing".into(),
        version: None,
    }];
    let err = check_requires(&requires, &env).unwrap_err();
    assert!(err.contains("不在 PATH 中"), "{err}");
}

#[test]
fn path_like_command_rejected() {
    let env = BTreeMap::new();
    let requires = vec![RequiresEntry {
        command: "/usr/bin/git".into(),
        version: None,
    }];
    let err = check_requires(&requires, &env).unwrap_err();
    assert!(err.contains("不支持路径写法"), "{err}");
}
