//! 工具链断言（`[[requires]]`）：只校验、不安装。
//!
//! - 存在性：在给定 PATH 中逐目录查找可执行文件；
//! - 版本：`command --version` 实测，取输出中的首个数字段与断言比较
//!   （`>=20` 下限 / `=20` 或 `20` 精确）；版本不可判定时 fail-closed 报错。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::schema::RequiresEntry;

/// 在给定 PATH 值中查找命令（纯查找，不 spawn）。
pub fn lookup_command(name: &str, path_value: Option<&str>) -> Option<PathBuf> {
    let path_value = path_value?;
    let separator = if cfg!(windows) { ';' } else { ':' };
    for dir in path_value.split(separator) {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        path.is_file()
    }
}

/// 实测版本：用给定环境执行 `command --version`，返回首行。
pub fn probe_version(command: &str, env: &BTreeMap<String, String>) -> Result<String, String> {
    let output = std::process::Command::new(command)
        .arg("--version")
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("无法执行 {command} --version：{e}"))?;
    let line = {
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines().next().unwrap_or("").trim().to_string()
    };
    if line.is_empty() {
        return Err(format!("{command} --version 未产生输出"));
    }
    Ok(line.to_string())
}

/// 校验 requires：命令必须在 PATH 中存在；声明了 version 时实测比较。
/// fail-closed：查不到或版本不可判定都报错。
pub fn check_requires(
    requires: &[RequiresEntry],
    env: &BTreeMap<String, String>,
) -> Result<(), String> {
    let path_value = env.get("PATH").map(String::as_str);
    for entry in requires {
        if entry.command.contains('/') || entry.command.contains('\\') {
            return Err(format!(
                "requires.command 不支持路径写法：{:?}（只接受 PATH 中的命令名）",
                entry.command
            ));
        }
        if lookup_command(&entry.command, path_value).is_none() {
            return Err(format!(
                "requires 校验失败：命令 {:?} 不在 PATH 中",
                entry.command
            ));
        }
        if let Some(required) = &entry.version {
            let line = probe_version(&entry.command, env)?;
            if !version_satisfies(&line, required)? {
                return Err(format!(
                    "requires 校验失败：{} 需要 {required}，实际：{line}",
                    entry.command
                ));
            }
        }
    }
    Ok(())
}

/// 版本断言比较：actual_line 是 `--version` 的首行输出。
pub fn version_satisfies(actual_line: &str, required: &str) -> Result<bool, String> {
    enum Op {
        AtLeast,
        Exact,
    }
    let (op, spec) = if let Some(rest) = required.strip_prefix(">=") {
        (Op::AtLeast, rest)
    } else if let Some(rest) = required.strip_prefix('=') {
        (Op::Exact, rest)
    } else {
        (Op::Exact, required)
    };
    let spec = spec.trim();
    if spec.is_empty() || spec.contains('=') {
        return Err(format!("版本断言无效：{required:?}"));
    }
    let spec = extract_version(spec).ok_or_else(|| format!("版本断言无效：{required:?}"))?;
    let actual = extract_version(actual_line)
        .ok_or_else(|| format!("无法从输出解析版本：{actual_line:?}"))?;

    let length = spec.len().max(actual.len());
    let pad = |mut segments: Vec<u64>| {
        segments.resize(length, 0);
        segments
    };
    let (actual, spec) = (pad(actual), pad(spec));
    Ok(match op {
        Op::AtLeast => actual >= spec,
        Op::Exact => actual == spec,
    })
}

/// 从文本中提取首个 `数字[.数字]*` 段为版本段。
fn extract_version(text: &str) -> Option<Vec<u64>> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let mut end = start;
    for (index, c) in text[start..].char_indices() {
        if c.is_ascii_digit() || c == '.' {
            end = start + index + c.len_utf8();
        } else {
            break;
        }
    }
    let segment = &text[start..end];
    let segment = segment.trim_end_matches('.');
    if segment.is_empty() {
        return None;
    }
    let segments = segment
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    (!segments.is_empty()).then_some(segments)
}

#[cfg(test)]
mod tests {
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
        let dir = std::env::temp_dir().join(format!(
            "av-lookup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

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
}
