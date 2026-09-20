//! bash 命令权限与沙箱策略。
//!
//! 权限是 Pipi 新增的（pi 靠扩展/审批机制，codex 靠 OS 级沙箱）。Pipi 的
//! 分层设计，按顺序评估，任何一层拒绝即拒绝（宁可拒绝不可放行）：
//!
//! 1. **切分**：引号感知的词法扫描 + shlex 分词，复合命令按 `&&` `||` `;`
//!    `|` 换行 `&` 拆段；引号内的连接符不拆段；无法安全解析 → 拒绝。
//! 2. **白/黑名单**：`denylist` 命中即拒；`allowlist` 必须逐段命中。
//!    Allowlist 未命中的**非危险**段落是唯一可交互审批的类型（见
//!    [`crate::approval`]）：用户拒绝 / 超时 / 中止 / 通道缺失一律拒绝。
//! 3. **危险命令**（移植自 codex，见 [`safety`]）：`rm -f` 家族、
//!    sudo/env/trap/bash-c 包装器 —— 非 `danger-full-access` 下拒绝。
//! 4. **沙箱**：`workspace-write` 下拒绝 shell 重定向；文件写入请使用
//!    具备 canonical containment 的 `write` / `edit` 工具。
//!
//! 局限（诚实声明）：这是用户态的粗粒度闸门，拦不住所有逃逸路径（如
//! `tee`、工具进程自身写文件）。bash 在 `workspace-write` 下拒绝 shell 重定向，
//! 是因为仅做执行前路径检查无法消除「命令先创建 symlink、再重定向写入」的
//! TOCTOU 窗口；codex 的做法是 OS 级沙箱（Landlock/Seatbelt），那是 Pipi
//! 的后续工作。
//!
//! 沙箱模式移植自 codex `protocol/src/config_types.rs` 的 `SandboxMode`：
//! `read-only` / `workspace-write` / `danger-full-access`（kebab-case）。

pub mod safety;

pub use safety::{dangerous_command_match, DangerousCommandMatch};

use std::fs;
use std::path::{Path, PathBuf};

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
                } else if chars.peek().is_none_or(|next| next.is_whitespace()) {
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

pub(crate) fn matches_entry(entry: &str, segment: &str) -> bool {
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

/// 普通 Agent 默认启用的基础工具。Agent 组合工具不在默认集合中，避免旧的
/// `agent.json`（缺少 `permissions.tools`）在升级后静默获得创建、运行或读取
/// 其他 Agent 的能力。
pub const DEFAULT_TOOLS: [&str; 7] = ["read", "write", "edit", "bash", "memory", "glob", "grep"];

/// Agent 组合工具：必须在 `permissions.tools` 中显式启用。
pub const AGENT_TOOLS: [&str; 3] = ["create_agent", "run_agent", "read_agent"];

/// Pipi 已知内置工具名（基础工具 + 显式启用的 Agent 组合工具）。
pub const KNOWN_TOOLS: [&str; 10] = [
    "read",
    "write",
    "edit",
    "bash",
    "memory",
    "glob",
    "grep",
    "create_agent",
    "run_agent",
    "read_agent",
];

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
    DEFAULT_TOOLS.iter().map(|s| s.to_string()).collect()
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

    /// bash 命令总闸：切分 → 白/黑名单 → 危险命令 → 沙箱策略。
    pub fn assess_bash(&self, command: &str, _workspace: &std::path::Path) -> Result<(), String> {
        match self.assess_bash_classified(command, _workspace) {
            BashAssessment::Allowed => Ok(()),
            BashAssessment::HardDenied(message) => Err(message),
            BashAssessment::NeedsApproval { missing } => Err(format!(
                "命令「{}」不在白名单中（允许：{}）",
                missing.join(" && "),
                self.bash.commands.join(", ")
            )),
        }
    }

    /// bash 命令分类评估。只有「Allowlist 模式下白名单未命中、且不命中危险
    /// 规则」的段落可以交给交互审批救回；黑名单命中、危险命令、沙箱约束
    /// 一律硬拒 —— 宁可拒绝不可放行。
    pub fn assess_bash_classified(
        &self,
        command: &str,
        _workspace: &std::path::Path,
    ) -> BashAssessment {
        if self.sandbox == SandboxMode::ReadOnly {
            return BashAssessment::HardDenied(
                "沙箱策略为 read-only：不允许执行命令（需要执行请调整 Agent 的沙箱设置）".into(),
            );
        }
        if self.sandbox == SandboxMode::WorkspaceWrite && has_unquoted_shell_redirection(command) {
            return BashAssessment::HardDenied(
                "沙箱策略为 workspace-write：拒绝 shell 重定向；请使用 write 或 edit 工具写入文件"
                    .into(),
            );
        }

        let segments = match split_segments(command) {
            Ok(segments) => segments,
            Err(error) => return BashAssessment::HardDenied(error),
        };

        let mut missing: Vec<String> = Vec::new();
        for seg in &segments {
            match self.bash.mode {
                BashMode::AllowAll => {}
                BashMode::Allowlist => {
                    if !self.bash.commands.iter().any(|c| matches_entry(c, &seg.text)) {
                        // 危险段落即使等用户批准也不放行
                        if self.sandbox != SandboxMode::DangerFullAccess
                            && dangerous_command_match(&seg.argv).is_some()
                        {
                            return BashAssessment::HardDenied(format!(
                                "「{}」命中危险命令规则，在 {} 沙箱下被拒绝（不可审批）",
                                seg.text,
                                self.sandbox.as_str()
                            ));
                        }
                        missing.push(seg.text.clone());
                        continue;
                    }
                }
                BashMode::Denylist => {
                    if self.bash.commands.iter().any(|c| matches_entry(c, &seg.text)) {
                        return BashAssessment::HardDenied(format!(
                            "命令「{}」被黑名单禁止",
                            seg.text
                        ));
                    }
                }
            }
            if self.sandbox != SandboxMode::DangerFullAccess {
                if let Some(matched) = dangerous_command_match(&seg.argv) {
                    return BashAssessment::HardDenied(match matched {
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
        }

        if missing.is_empty() {
            BashAssessment::Allowed
        } else {
            BashAssessment::NeedsApproval { missing }
        }
    }
}

/// bash 命令评估结论。
#[derive(Debug, Clone, PartialEq)]
pub enum BashAssessment {
    Allowed,
    /// 黑名单命中 / 危险命令 / 沙箱约束 / 解析失败 —— 不可审批。
    HardDenied(String),
    /// Allowlist 模式下白名单未命中（且非危险）。`missing` 是未命中的段落文本，
    /// 「总是允许」时按段写入白名单。
    NeedsApproval { missing: Vec<String> },
}

/// 交互审批通道。宿主（桌面壳 / Web 服务）实现传输：把请求发给用户界面，
/// 等待用户决定。任何实现失败都必须落到 Err（fail-closed）。
#[async_trait::async_trait]
pub trait CommandApprover: Send + Sync {
    /// 请求批准执行整条命令。Ok(()) = 批准；Err = 拒绝 / 超时 / 已中止 / 通道失败。
    async fn approve(&self, command: &str) -> Result<(), String>;
}

/// 判断命令中是否存在未被引号或反斜杠保护的 shell 重定向字符。
///
/// 这里故意只做 fail-closed 检测，不尝试重写或执行原始 shell 命令：
/// 执行前检查目标路径无法阻止命令先创建 symlink 再重定向写入。
fn has_unquoted_shell_redirection(command: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for c in command.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && !in_single {
            escaped = true;
            continue;
        }
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '<' | '>' if !in_single && !in_double => return true,
            _ => {}
        }
    }
    false
}

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
        let p = PermissionsConfig {
            tools: default_tools(),
            bash: BashPermissions {
                mode: BashMode::Allowlist,
                commands: vec!["git".into(), "npm run".into()],
            },
            sandbox: SandboxMode::DangerFullAccess,
        };
        let allowed = |c: &str| {
            matches!(
                p.assess_bash_classified(c, Path::new("/tmp")),
                BashAssessment::Allowed
            )
        };
        let askable = |c: &str| {
            matches!(
                p.assess_bash_classified(c, Path::new("/tmp")),
                BashAssessment::NeedsApproval { .. }
            )
        };
        assert!(allowed("git status"));
        assert!(allowed("git commit -m 'x'"));
        assert!(allowed("npm run build"));
        assert!(askable("  ls -la"));
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
        assert!(p
            .assess_bash("git add . && git commit", Path::new("/tmp"))
            .is_ok());
        assert!(p
            .assess_bash("git add . && rm -rf /tmp/x", Path::new("/tmp"))
            .is_err());
        assert!(p
            .assess_bash("echo hi; git status", Path::new("/tmp"))
            .is_err());
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
        assert!(p
            .assess_bash("ls -la && rm -rf /tmp/x", Path::new("/tmp"))
            .is_err());
        assert!(p
            .assess_bash("sudo apt install x", Path::new("/tmp"))
            .is_err());
        assert!(p
            .assess_bash("ls -la && git status", Path::new("/tmp"))
            .is_ok());
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
        assert!(p
            .assess_bash(r#"echo "a; rm -rf /""#, Path::new("/tmp"))
            .is_ok());
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
    fn workspace_write_rejects_shell_redirects_before_execution() {
        let root =
            std::env::temp_dir().join(format!("pipi-redirect-policy-{}", crate::session::new_id()));
        let ws = root.join("workspace");
        std::fs::create_dir_all(&ws).unwrap();
        let p = PermissionsConfig {
            tools: default_tools(),
            bash: BashPermissions::default(),
            sandbox: SandboxMode::WorkspaceWrite,
        };

        for command in [
            "ls > out.txt",
            "ls >| out.txt",
            "ls &> out.txt",
            "ls foo>out.txt",
            "cat < /etc/passwd",
            "ln -s /outside link && printf x > link",
        ] {
            assert!(
                p.assess_bash(command, &ws).is_err(),
                "redirect must be rejected: {command}"
            );
        }
        assert!(p.assess_bash("printf '>'", &ws).is_ok());
        assert!(p.assess_bash(r#"printf ">""#, &ws).is_ok());
        assert!(p.assess_bash(r"printf \>", &ws).is_ok());

        let _ = std::fs::remove_dir_all(root);
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

    #[test]
    fn agent_composition_tools_are_known_but_never_defaulted() {
        let defaults = PermissionsConfig::default();
        for tool in AGENT_TOOLS {
            assert!(KNOWN_TOOLS.contains(&tool));
            assert!(!defaults.tool_enabled(tool));
        }

        let legacy: PermissionsConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.tools, DEFAULT_TOOLS);
    }

    fn allowlist_config(commands: &[&str], sandbox: SandboxMode) -> PermissionsConfig {
        PermissionsConfig {
            tools: vec!["bash".into()],
            bash: BashPermissions {
                mode: BashMode::Allowlist,
                commands: commands.iter().map(|c| c.to_string()).collect(),
            },
            sandbox,
        }
    }

    #[test]
    fn classification_allowlist_miss_is_askable() {
        let cfg = allowlist_config(&["git"], SandboxMode::WorkspaceWrite);
        assert_eq!(
            cfg.assess_bash_classified("git status", Path::new("/tmp")),
            BashAssessment::Allowed
        );
        assert_eq!(
            cfg.assess_bash_classified("npm install", Path::new("/tmp")),
            BashAssessment::NeedsApproval {
                missing: vec!["npm install".into()]
            }
        );
        // 复合命令：只收集未命中的段落
        assert_eq!(
            cfg.assess_bash_classified("git status && npm install", Path::new("/tmp")),
            BashAssessment::NeedsApproval {
                missing: vec!["npm install".into()]
            }
        );
    }

    #[test]
    fn classification_dangerous_and_denied_are_never_askable() {
        let cfg = allowlist_config(&["git"], SandboxMode::WorkspaceWrite);
        // 白名单未命中 + 危险命令 → 硬拒
        match cfg.assess_bash_classified("rm -rf /tmp/x", Path::new("/tmp")) {
            BashAssessment::HardDenied(message) => assert!(message.contains("拒绝")),
            other => panic!("expected HardDenied, got {other:?}"),
        }
        // denylist 命中 → 硬拒
        let deny = PermissionsConfig {
            tools: vec!["bash".into()],
            bash: BashPermissions {
                mode: BashMode::Denylist,
                commands: vec!["curl".into()],
            },
            sandbox: SandboxMode::DangerFullAccess,
        };
        assert!(matches!(
            deny.assess_bash_classified("curl example.com", Path::new("/tmp")),
            BashAssessment::HardDenied(_)
        ));
        // 沙箱重定向 → 硬拒
        assert!(matches!(
            cfg.assess_bash_classified("git status > out.txt", Path::new("/tmp")),
            BashAssessment::HardDenied(_)
        ));
        // read-only → 一律硬拒
        let ro = allowlist_config(&["git"], SandboxMode::ReadOnly);
        assert!(matches!(
            ro.assess_bash_classified("git status", Path::new("/tmp")),
            BashAssessment::HardDenied(_)
        ));
    }

    #[test]
    fn classification_assess_bash_maps_miss_to_error() {
        let cfg = allowlist_config(&["git"], SandboxMode::WorkspaceWrite);
        let error = cfg.assess_bash("npm install", Path::new("/tmp")).unwrap_err();
        assert!(error.contains("不在白名单中"));
    }
}
