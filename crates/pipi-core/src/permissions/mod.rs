//! bash 命令权限与沙箱策略。
//!
//! 权限是 Pipi 新增的（pi 靠扩展/审批机制，codex 靠 OS 级沙箱）。Pipi 的
//! 分层设计，按顺序评估，任何一层拒绝即拒绝（宁可拒绝不可放行）：
//!
//! 1. **切分**：引号感知的词法扫描 + shlex 分词，复合命令按 `&&` `||` `;`
//!    `|` 换行 `&` 拆段；引号内的连接符不拆段；无法安全解析 → 拒绝。
//! 2. **白/黑名单**：`denylist` 命中即拒；`allowlist` 必须逐段命中。
//! 3. **危险命令**（移植自 codex，见 [`safety`]）：`rm -f` 家族、
//!    sudo/env/trap/bash-c 包装器 —— 非 `danger-full-access` 下拒绝。
//! 4. **沙箱**：`workspace-write` 下重定向目标不得越出工作目录。
//!
//! 局限（诚实声明）：这是用户态的粗粒度闸门，拦不住所有逃逸路径（如
//! `tee`、工具进程自身写文件）。codex 的做法是 OS 级沙箱
//! （Landlock/Seatbelt），那是 Pipi 的后续工作；在此之前本模块提供
//! 「配置 + 启发式」两层防护。
//!
//! 沙箱模式移植自 codex `protocol/src/config_types.rs` 的 `SandboxMode`：
//! `read-only` / `workspace-write` / `danger-full-access`（kebab-case）。

pub mod safety;

pub use safety::{dangerous_command_match, DangerousCommandMatch};

use serde::{Deserialize, Serialize};

/// 沙箱策略。移植自 codex 的 `SandboxMode`。
///
/// 反序列化缺省值为 `DangerFullAccess`（兼容旧 agent.json 的行为）；
/// 新建 Agent 的 UI 默认选 `WorkspaceWrite`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxMode {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "workspace-write")]
    WorkspaceWrite,
    #[serde(rename = "danger-full-access")]
    #[default]
    DangerFullAccess,
}

impl SandboxMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SandboxMode::ReadOnly => "read-only",
            SandboxMode::WorkspaceWrite => "workspace-write",
            SandboxMode::DangerFullAccess => "danger-full-access",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            SandboxMode::ReadOnly => "只读",
            SandboxMode::WorkspaceWrite => "工作目录内可写",
            SandboxMode::DangerFullAccess => "完全访问",
        }
    }
}

/// 一个已切分的命令段：argv（供危险检测）与重组文本（供白名单匹配）。
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub argv: Vec<String>,
    pub text: String,
}

/// 引号感知的命令切分。
///
/// 先在原始文本上按引号状态机找连接符（引号内的 `;`、`&&` 不拆段，
/// `git commit -m "a; rm -rf /"` 是一个段），再用 shlex 对每段分词。
/// 无法安全解析（未闭合引号等）返回 Err，由调用方 fail-closed。
pub fn split_segments(command: &str) -> Result<Vec<Segment>, String> {
    let mut out = Vec::new();
    for raw in split_raw_on_connectors(command)? {
        let Some(argv) = shlex::split(raw.trim()) else {
            return Err(format!("命令片段「{raw}」包含未闭合的引号或转义"));
        };
        if argv.is_empty() {
            continue;
        }
        let text = shlex::try_join(argv.iter().map(String::as_str))
            .map_err(|_| "命令片段无法重组".to_string())?;
        out.push(Segment { argv, text });
    }
    if out.is_empty() {
        return Err("空命令".into());
    }
    Ok(out)
}

/// 在原始文本上按连接符切分：跟踪单双引号与反斜杠转义状态，
/// 只在引号外切分。单行命令支持 `&&` `||` `;` `|` `&` 和换行。
fn split_raw_on_connectors(s: &str) -> Result<Vec<String>, String> {
    let mut segments: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let push_segment = |cur: &mut String, segments: &mut Vec<String>| {
        if !cur.trim().is_empty() {
            segments.push(std::mem::take(cur));
        } else {
            cur.clear();
        }
    };

    while let Some(c) = chars.next() {
        if escaped {
            cur.push('\\');
            cur.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' if !in_single => {
                escaped = true;
            }
            '\'' if !in_double => {
                in_single = !in_single;
                cur.push(c);
            }
            '"' if !in_single => {
                in_double = !in_double;
                cur.push(c);
            }
            ';' if !in_single && !in_double => push_segment(&mut cur, &mut segments),
            '\n' if !in_single && !in_double => push_segment(&mut cur, &mut segments),
            '&' if !in_single && !in_double => {
                if chars.peek() == Some(&'&') {
                    chars.next();
                    push_segment(&mut cur, &mut segments);
                } else if chars
                    .peek()
                    .is_none_or(|next| next.is_whitespace())
                {
                    // 后台运行符 `sleep 5 & cmd`
                    push_segment(&mut cur, &mut segments);
                } else {
                    cur.push(c); // 粘在词里的 &（如文件名），当普通字符
                }
            }
            '|' if !in_single && !in_double => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                push_segment(&mut cur, &mut segments);
            }
            _ => cur.push(c),
        }
    }
    if in_single || in_double {
        return Err("命令包含未闭合的引号".into());
    }
    push_segment(&mut cur, &mut segments);
    Ok(segments)
}

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
    fn check(&self, segment: &Segment) -> Result<(), String> {
        match self.mode {
            BashMode::AllowAll => Ok(()),
            BashMode::Allowlist => {
                if self.commands.iter().any(|c| matches_entry(c, &segment.text)) {
                    Ok(())
                } else {
                    Err(format!(
                        "命令「{}」不在白名单中（允许：{}）",
                        segment.text,
                        self.commands.join(", ")
                    ))
                }
            }
            BashMode::Denylist => {
                if self.commands.iter().any(|c| matches_entry(c, &segment.text)) {
                    Err(format!("命令「{}」被黑名单禁止", segment.text))
                } else {
                    Ok(())
                }
            }
        }
    }
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
    /// 沙箱策略（codex 的 SandboxMode）。缺省 DangerFullAccess 以兼容旧文件。
    #[serde(default)]
    pub sandbox: SandboxMode,
}

fn default_tools() -> Vec<String> {
    KNOWN_TOOLS.iter().map(|s| s.to_string()).collect()
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        PermissionsConfig {
            tools: default_tools(),
            bash: BashPermissions::default(),
            sandbox: SandboxMode::default(),
        }
    }
}

impl PermissionsConfig {
    pub fn tool_enabled(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t == name)
    }

    /// bash 命令总闸：切分 → 白/黑名单 → 危险命令 → 沙箱重定向检查。
    pub fn assess_bash(&self, command: &str, workspace: &std::path::Path) -> Result<(), String> {
        if self.sandbox == SandboxMode::ReadOnly {
            return Err(
                "沙箱策略为 read-only：不允许执行命令（需要执行请调整 Agent 的沙箱设置）"
                    .into(),
            );
        }
        let segments = split_segments(command)?;
        for seg in &segments {
            self.bash.check(seg)?;
            if self.sandbox != SandboxMode::DangerFullAccess {
                if let Some(matched) = dangerous_command_match(&seg.argv) {
                    return Err(match matched {
                        DangerousCommandMatch::ForcedRm => format!(
                            "「{}」包含强制删除（rm -f 家族），在 {} 沙箱下被拒绝",
                            seg.text,
                            self.sandbox.as_str()
                        ),
                        DangerousCommandMatch::Other => format!(
                            "「{}」命中危险命令规则，在 {} 沙箱下被拒绝",
                            seg.text,
                            self.sandbox.as_str()
                        ),
                    });
                }
            }
            if self.sandbox == SandboxMode::WorkspaceWrite {
                check_redirect_targets(&seg.argv, workspace)?;
            }
        }
        Ok(())
    }
}

/// workspace-write 沙箱下：重定向目标（`>` `>>` `2>` 后面的路径）不得越出工作目录。
fn check_redirect_targets(argv: &[String], workspace: &std::path::Path) -> Result<(), String> {
    let mut i = 0;
    while i < argv.len() {
        let (op_len, has_inline_target) = redirect_kind(&argv[i]);
        if op_len > 0 {
            let target = if has_inline_target {
                argv[i][op_len..].to_string()
            } else {
                i += 1;
                argv.get(i)
                    .cloned()
                    .ok_or_else(|| format!("重定向「{}」缺少目标路径", argv[i - 1]))?
            };
            check_one_redirect(&target, workspace)?;
        }
        i += 1;
    }
    Ok(())
}

/// 识别重定向操作符：返回 (操作符占用的字符数, 目标是否粘连在同一 token)。
/// 支持 `>` `>>` `1>` `2>` 及其粘连形式（`>file`、`2>>log`）。
fn redirect_kind(token: &str) -> (usize, bool) {
    let bytes = token.as_bytes();
    let mut idx = 0;
    if bytes.first().is_some_and(|c| c.is_ascii_digit()) {
        idx = 1;
    }
    match bytes.get(idx) {
        Some(b'>') => {
            let op_len = if bytes.get(idx + 1) == Some(&b'>') { idx + 2 } else { idx + 1 };
            (op_len, token.len() > op_len)
        }
        _ => (0, false),
    }
}

fn check_one_redirect(target: &str, workspace: &std::path::Path) -> Result<(), String> {
    let path = std::path::Path::new(target);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    if is_within(workspace, &absolute) {
        Ok(())
    } else {
        Err(format!(
            "重定向目标 {target} 越出工作目录（沙箱策略 workspace-write）"
        ))
    }
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
    use std::path::Path;

    #[test]
    fn split_respects_quotes() {
        let segs = split_segments(r#"git commit -m "a; rm -rf /" && ls"#).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].argv, vec!["git", "commit", "-m", "a; rm -rf /"]);
        assert_eq!(segs[1].argv, vec!["ls"]);
    }

    #[test]
    fn glued_connectors_split_like_bash() {
        // bash 本来就把 a&&rm 解析为两个命令 —— 同样切分，交给后续层拦截
        let segs = split_segments("echo a&&rm -rf /").unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[1].argv, vec!["rm", "-rf", "/"]);
        assert!(split_segments("echo \"unclosed").is_err());
        assert!(split_segments("   ").is_err());
    }

    #[test]
    fn quoted_metacharacters_are_plain_text() {
        // 引号里的 & ; | 是普通字符：shlex 分词后仍在同一个参数里
        let segs = split_segments(r#"echo "a & b"; echo c'|'d"#).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].argv, vec!["echo", "a & b"]);
        assert_eq!(segs[1].argv, vec!["echo", "c|d"]);
    }

    #[test]
    fn background_ampersand_splits() {
        let segs = split_segments("sleep 5 & echo done").unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].argv, vec!["sleep", "5"]);
        assert_eq!(segs[1].argv, vec!["echo", "done"]);
    }

    #[test]
    fn allowlist_matches_word_prefix() {
        let p = BashPermissions {
            mode: BashMode::Allowlist,
            commands: vec!["git".into(), "npm run".into()],
        };
        let ok = |c: &str| p.check(&split_segments(c).unwrap().remove(0));
        assert!(ok("git status").is_ok());
        assert!(ok("git commit -m 'x'").is_ok());
        assert!(ok("npm run build").is_ok());
        assert!(ok("  ls -la").is_err());
    }

    #[test]
    fn allowlist_checks_compound_segments() {
        let p = PermissionsConfig {
            tools: default_tools(),
            bash: BashPermissions {
                mode: BashMode::Allowlist,
                commands: vec!["git".into()],
            },
            sandbox: SandboxMode::DangerFullAccess,
        };
        assert!(p.assess_bash("git add . && git commit", Path::new("/tmp")).is_ok());
        assert!(p.assess_bash("git add . && rm -rf /tmp/x", Path::new("/tmp")).is_err());
        assert!(p.assess_bash("echo hi; git status", Path::new("/tmp")).is_err());
    }

    #[test]
    fn denylist_blocks_matching_segment() {
        let p = PermissionsConfig {
            tools: default_tools(),
            bash: BashPermissions {
                mode: BashMode::Denylist,
                commands: vec!["rm".into(), "sudo".into()],
            },
            sandbox: SandboxMode::DangerFullAccess,
        };
        assert!(p.assess_bash("ls -la && rm -rf /tmp/x", Path::new("/tmp")).is_err());
        assert!(p.assess_bash("sudo apt install x", Path::new("/tmp")).is_err());
        assert!(p.assess_bash("ls -la && git status", Path::new("/tmp")).is_ok());
    }

    #[test]
    fn quotes_defeat_naive_denylist_but_not_ours() {
        let p = PermissionsConfig {
            tools: default_tools(),
            bash: BashPermissions {
                mode: BashMode::Denylist,
                commands: vec!["rm".into()],
            },
            sandbox: SandboxMode::DangerFullAccess,
        };
        // 引号里的 "分号" 不拆段：整段是一条 echo，不命中 rm
        assert!(p.assess_bash(r#"echo "a; rm -rf /""#, Path::new("/tmp")).is_ok());
        assert!(p.assess_bash("rm x", Path::new("/tmp")).is_err());
    }

    #[test]
    fn sandbox_blocks_forced_rm_unless_full_access() {
        let mut p = PermissionsConfig::default();
        // workspace-write：强制删除被拒，普通 rm 放行
        p.sandbox = SandboxMode::WorkspaceWrite;
        assert!(p.assess_bash("rm -rf build/", Path::new("/tmp")).is_err());
        assert!(p.assess_bash("rm -r build/", Path::new("/tmp")).is_ok());
        // danger-full-access：放行
        p.sandbox = SandboxMode::DangerFullAccess;
        assert!(p.assess_bash("rm -rf build/", Path::new("/tmp")).is_ok());
    }

    #[test]
    fn read_only_blocks_bash_entirely() {
        let p = PermissionsConfig {
            tools: default_tools(),
            bash: BashPermissions::default(),
            sandbox: SandboxMode::ReadOnly,
        };
        assert!(p.assess_bash("ls", Path::new("/tmp")).is_err());
    }

    #[test]
    fn workspace_write_blocks_outside_redirects() {
        let ws = Path::new("/home/u/project");
        let p = PermissionsConfig {
            tools: default_tools(),
            bash: BashPermissions::default(),
            sandbox: SandboxMode::WorkspaceWrite,
        };
        assert!(p.assess_bash("ls > out.txt", ws).is_ok());
        assert!(p.assess_bash("ls > build/out.txt", ws).is_ok());
        assert!(p.assess_bash("ls > ../outside.txt", ws).is_err());
        assert!(p.assess_bash("ls > /etc/passwd", ws).is_err());
        assert!(p.assess_bash("cat a >> /tmp/x", ws).is_err());
        assert!(p.assess_bash("ls > ../project/ok.txt", ws).is_ok());
        assert!(p.assess_bash("cargo build 2> build/log.txt", ws).is_ok());
        assert!(p.assess_bash("cargo build 2> /tmp/log.txt", ws).is_err());
        assert!(p.assess_bash("ls >out.txt", ws).is_ok());
    }

    #[test]
    fn serde_roundtrip_and_legacy_default() {
        let cfg = PermissionsConfig {
            tools: vec!["read".into(), "bash".into()],
            bash: BashPermissions {
                mode: BashMode::Allowlist,
                commands: vec!["git".into()],
            },
            sandbox: SandboxMode::WorkspaceWrite,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("workspace-write"));
        let back: PermissionsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);

        // 旧文件：没有 sandbox 字段 → DangerFullAccess（行为不变）
        let legacy = r#"{"tools":["read"],"bash":{"mode":"allowAll","commands":[]}}"#;
        let back: PermissionsConfig = serde_json::from_str(legacy).unwrap();
        assert_eq!(back.sandbox, SandboxMode::DangerFullAccess);
    }

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
