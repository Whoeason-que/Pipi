//! bash 命令权限（Pipi 新增，pi 中对应的能力由扩展/审批机制承担）。
//!
//! 设计：一切皆文件 —— 权限就是 `agent.json` 里的 `permissions.bash`：
//! - `allowAll`：不做限制（默认，个人工具的常见选择）
//! - `allowlist`：每一段命令都必须命中白名单
//! - `denylist`：命中黑名单的命令段被拒绝
//!
//! 复合命令（`&&` / `||` / `;` / `|` / 换行）会拆段逐个检查；朴素实现，
//! 不解析引号 —— 这是有意的（见 README「小核心」）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BashMode {
    #[default]
    AllowAll,
    Allowlist,
    Denylist,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashPermissions {
    #[serde(default)]
    pub mode: BashMode,
    /// 白名单 / 黑名单条目。单词条目（如 "git"）匹配以该词开头的命令段；
    /// 带空格条目（如 "npm run"）按前缀匹配。
    #[serde(default)]
    pub commands: Vec<String>,
}

impl BashPermissions {
    pub fn check(&self, command: &str) -> Result<(), String> {
        match self.mode {
            BashMode::AllowAll => Ok(()),
            BashMode::Allowlist => {
                for seg in split_command_segments(command) {
                    if !self.commands.iter().any(|c| matches_entry(c, &seg)) {
                        return Err(format!(
                            "命令「{seg}」不在白名单中（允许：{}）",
                            self.commands.join(", ")
                        ));
                    }
                }
                Ok(())
            }
            BashMode::Denylist => {
                for seg in split_command_segments(command) {
                    if self.commands.iter().any(|c| matches_entry(c, &seg)) {
                        return Err(format!("命令「{seg}」被黑名单禁止"));
                    }
                }
                Ok(())
            }
        }
    }
}

/// 拆分复合命令。按 && || ; | 换行切分；不解析引号（有意的朴素实现）。
pub fn split_command_segments(command: &str) -> Vec<String> {
    let mut segs: Vec<String> = vec![command.to_string()];
    for sep in ["&&", "||", ";", "|", "\n"] {
        segs = segs
            .iter()
            .flat_map(|s| s.split(sep))
            .map(str::to_string)
            .collect();
    }
    segs.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn matches_entry(entry: &str, segment: &str) -> bool {
    let entry = entry.trim();
    let seg = segment.trim_start();
    if entry.is_empty() || seg.is_empty() {
        return false;
    }
    if entry.contains(' ') {
        seg.starts_with(entry)
    } else {
        seg == entry
            || seg
                .strip_prefix(entry)
                .is_some_and(|rest| rest.starts_with(' '))
    }
}

/// Pipi 已知内置工具名。
pub const KNOWN_TOOLS: [&str; 5] = ["read", "write", "edit", "bash", "memory"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsConfig {
    /// 启用的工具名列表（`KNOWN_TOOLS` 的子集）。
    #[serde(default = "default_tools")]
    pub tools: Vec<String>,
    #[serde(default)]
    pub bash: BashPermissions,
}

fn default_tools() -> Vec<String> {
    KNOWN_TOOLS.iter().map(|s| s.to_string()).collect()
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        PermissionsConfig {
            tools: default_tools(),
            bash: BashPermissions::default(),
        }
    }
}

impl PermissionsConfig {
    pub fn tool_enabled(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_all_passes_everything() {
        let p = BashPermissions::default();
        assert!(p.check("rm -rf /").is_ok());
    }

    #[test]
    fn allowlist_matches_word_prefix() {
        let p = BashPermissions {
            mode: BashMode::Allowlist,
            commands: vec!["git".into(), "npm run".into()],
        };
        assert!(p.check("git status").is_ok());
        assert!(p.check("git commit -m 'x'").is_ok());
        assert!(p.check("npm run build").is_ok());
        assert!(p.check("  ls -la").is_err());
    }

    #[test]
    fn allowlist_checks_compound_segments() {
        let p = BashPermissions {
            mode: BashMode::Allowlist,
            commands: vec!["git".into()],
        };
        assert!(p.check("git add . && git commit").is_ok());
        assert!(p.check("git add . && rm -rf /").is_err());
        assert!(p.check("echo hi; git status").is_err());
    }

    #[test]
    fn denylist_blocks_matching_segment() {
        let p = BashPermissions {
            mode: BashMode::Denylist,
            commands: vec!["rm".into(), "sudo".into()],
        };
        assert!(p.check("ls -la && rm -rf /tmp/x").is_err());
        assert!(p.check("sudo apt install x").is_err());
        assert!(p.check("ls -la && git status").is_ok());
    }

    #[test]
    fn serde_roundtrip() {
        let cfg = PermissionsConfig {
            tools: vec!["read".into(), "bash".into()],
            bash: BashPermissions {
                mode: BashMode::Allowlist,
                commands: vec!["git".into()],
            },
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("allowlist"));
        assert!(json.contains("tools"));
        let back: PermissionsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
    }
}
