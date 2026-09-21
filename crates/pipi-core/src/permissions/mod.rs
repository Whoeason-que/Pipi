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
//!    `rm -f` 家族的唯一例外是「只作用于工作区内的字面量路径、且不是 git
//!    仓库根」的直接调用（见 `forced_rm_inside_workspace`），用于清理构建
//!    与缓存产物；包装调用、变量 / 通配符目标一律落回拒绝。
//! 4. **沙箱**：`workspace-write` 下拒绝写入文件系统的重定向（`>` `>>`
//!    `2>文件` `&>` `>|` `<>` `>&词`）与从文件读入的重定向（`<文件` `<(cmd)`）；
//!    文件写入请使用具备 canonical containment 的 `write` / `edit` 工具。
//!    丢弃 / 合并输出（`2>/dev/null`、`2>&1`、`>&-`）与 heredoc / herestring
//!    既不落盘也不读文件，一律放行 —— 见 `check_write_redirect`。
//!
//! 局限（诚实声明）：这是用户态的粗粒度闸门，拦不住所有逃逸路径（如
//! `tee`、`python -c "open(p,'w')"`、工具进程自身写文件）。bash 在
//! `workspace-write` 下拒绝写入重定向，是因为仅做执行前路径检查无法消除
//! 「命令先创建 symlink、再重定向写入」的 TOCTOU 窗口；codex 的做法是
//! OS 级沙箱（Landlock/Seatbelt），那是 Pipi 的后续工作。**因此这条规则的
//! 价值是「工作区写入必须走 write / edit 的 canonical containment」，不是
//! 防逃逸**——把不含写入的构造一并拦掉只会白付摩擦成本（实测：一个真实
//! 会话里 91 次拒绝中只有 1 次是真实文件写入）。
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
        if self.sandbox == SandboxMode::WorkspaceWrite {
            match check_write_redirect(command) {
                RedirectCheck::Ok => {}
                RedirectCheck::Write(token) => {
                    return BashAssessment::HardDenied(format!(
                        "沙箱策略为 workspace-write：拒绝写入重定向「{token}」；工作区内写入请用 write / edit 工具。\
                         丢弃或合并输出可以用 2>/dev/null、2>&1，管道与 heredoc 不受限制"
                    ));
                }
                RedirectCheck::Read(token) => {
                    return BashAssessment::HardDenied(format!(
                        "沙箱策略为 workspace-write：拒绝输入重定向「{token}」；读文件请用 read 工具\
                         （带工作区包含检查），把脚本喂给 stdin 可以用 heredoc（<<EOF）"
                    ));
                }
                RedirectCheck::Unparseable(reason) => {
                    return BashAssessment::HardDenied(format!(
                        "沙箱策略为 workspace-write：{reason}，无法静态分析命令中的重定向，已拒绝"
                    ));
                }
            }
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
                    // 工作区内的字面量 `rm -f`：清理构建 / 缓存产物的常规操作，
                    // 放行；其余强制删除（含包装调用、越界或不可解析的目标）照旧拒绝。
                    let workspace_contained = matches!(matched, DangerousCommandMatch::ForcedRm)
                        && forced_rm_inside_workspace(&seg.argv, _workspace, command);
                    if !workspace_contained {
                        return BashAssessment::HardDenied(match matched {
                            DangerousCommandMatch::ForcedRm => format!(
                                "「{}」包含强制删除（rm -f 家族），在 {} 沙箱下被拒绝（仅放行工作区内、\
                                 非仓库根的绝对路径字面量；含变量 / 通配符的目标，以及命令内有 cd / pushd \
                                 时的相对路径都不可审批）",
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

/// 重定向静态检查的结论。
enum RedirectCheck {
    /// 只有丢弃 / 合并输出与 heredoc，放行。
    Ok,
    /// 命中写入文件系统的重定向，附原文（用于拒绝文案）。
    Write(String),
    /// 命中从文件读入（stdin）的重定向，附原文。
    Read(String),
    /// 无法静态分析的形态 —— fail-closed 拒绝。
    Unparseable(String),
}

/// 单个重定向的判定结果。
enum RedirectStep {
    /// 放行（fd 复制 / 关闭、丢弃输出、heredoc）。
    Allowed,
    /// 命中写入，附原文。
    Write(String),
    /// 命中文件读入，附原文。
    Read(String),
}

/// 判断命令里的重定向是否可以放行。
///
/// 放行（不落盘、不读文件）：
///
/// - `>/dev/null`、`2>/dev/null`、`2>>/dev/null`：丢弃输出
/// - `2>&1`、`1>&2`、`>&-`、`2>&-`：fd 复制 / 关闭
/// - `<<EOF`、`<<-EOF`：heredoc（喂 stdin，正文按数据处理）
/// - `<<<词`：herestring
///
/// 拒绝：
///
/// - **写入文件系统**：`>` `>>` `2>文件` `&>` `&>>` `>|` `<>` `>&词`
/// - **从文件读入**：`<文件` `<&0` `<(cmd)` —— 与 `read` 工具的工作区包含检查
///   保持同一方向，不因为 bash 本来就能 `cat` 而刻意放宽
///
/// 为什么写侧只拦「真实文件写入」：这条规则保护的是「工作区写入必须走
/// `write` / `edit` 的 canonical containment」。`2>/dev/null` 这类构造既没有
/// 落盘能力、也不绕过 containment，拦下来只产生摩擦 —— 实测一个真实会话的
/// 91 次拒绝里只有 1 次是真实文件写入，其余全是丢弃/合并 stderr。
///
/// fail-closed：缺结束符的 heredoc、不配对的进程替换括号一律判为不可分析
/// 并拒绝。未闭合引号不在此处报错 —— 交给后续的段落切分统一拒绝。
///
/// heredoc 正文按「喂给 stdin 的数据」处理，其中的 `<` `>` 不算重定向。正文行
/// 仍会经过后面的段落切分，所以正文里出现危险命令或引号不配对时整条命令照样
/// 被拒绝 —— 保守方向，不构成放行。
fn check_write_redirect(command: &str) -> RedirectCheck {
    let chars: Vec<char> = command.chars().collect();
    let mut pending_heredocs: Vec<(String, bool)> = Vec::new();
    let mut i = 0usize;

    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            // 未闭合引号不在这里判死：段落切分会以同样的理由拒绝整条命令
            '\'' => match skip_quoted(&chars, i + 1, '\'') {
                Some(end) => i = end + 1,
                None => return RedirectCheck::Ok,
            },
            '"' => match skip_quoted(&chars, i + 1, '"') {
                Some(end) => i = end + 1,
                None => return RedirectCheck::Ok,
            },
            '\n' => {
                i += 1;
                for (delimiter, strip_tabs) in std::mem::take(&mut pending_heredocs) {
                    match skip_heredoc_body(&chars, i, &delimiter, strip_tabs) {
                        Some(next) => i = next,
                        None => {
                            return RedirectCheck::Unparseable(format!(
                                "heredoc 缺少结束符「{delimiter}」"
                            ))
                        }
                    }
                }
            }
            '#' if is_word_start(&chars, i) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '<' | '>' => match scan_redirect(&chars, i, &mut pending_heredocs) {
                Ok((RedirectStep::Allowed, next)) => i = next,
                Ok((RedirectStep::Write(token), _)) => return RedirectCheck::Write(token),
                Ok((RedirectStep::Read(token), _)) => return RedirectCheck::Read(token),
                Err(reason) => return RedirectCheck::Unparseable(reason),
            },
            _ => i += 1,
        }
    }

    if let Some((delimiter, _)) = pending_heredocs.first() {
        return RedirectCheck::Unparseable(format!("heredoc 缺少结束符「{delimiter}」"));
    }
    RedirectCheck::Ok
}

/// 解析 `start` 处的一个重定向，返回判定与下一个待扫描下标。
///
/// `Err` = 无法静态分析（进程替换括号不配对），由调用方 fail-closed 拒绝。
fn scan_redirect(
    chars: &[char],
    start: usize,
    heredocs: &mut Vec<(String, bool)>,
) -> Result<(RedirectStep, usize), String> {
    let op = chars[start];
    let next = chars.get(start + 1).copied();
    let prefix = redirect_prefix(chars, start);

    // 进程替换：`<(cmd)` 读、`>(cmd)` 写进管道 —— 都不落盘
    if next == Some('(') {
        let after =
            skip_balanced_parens(chars, start + 1).ok_or("进程替换的括号不配对".to_string())?;
        if op == '<' {
            return Ok((read_step(&redirect_snippet(chars, start, after)), after));
        }
        return Ok((RedirectStep::Allowed, after));
    }

    if next == Some(op) {
        if op == '<' {
            let mut j = start + 2;
            let strip_tabs = chars.get(j) == Some(&'-');
            if strip_tabs {
                j += 1;
            }
            // `<<<词`：herestring
            if chars.get(j) == Some(&'<') {
                let (_, after) = read_word(chars, j + 1)?;
                return Ok((RedirectStep::Allowed, after));
            }
            let (delimiter, after) = read_word(chars, j)?;
            if delimiter.is_empty() {
                return Err("heredoc 缺少结束符".into());
            }
            heredocs.push((delimiter, strip_tabs));
            return Ok((RedirectStep::Allowed, after));
        }
        // `>>文件`：追加写
        let (target, after) = read_word(chars, start + 2)?;
        return Ok((write_step(&format!("{prefix}>>{target}"), &target), after));
    }

    // `<>文件`：读写打开（可写即算写入）
    if op == '<' && next == Some('>') {
        let (target, after) = read_word(chars, start + 2)?;
        return Ok((write_step(&format!("{prefix}<>{target}"), &target), after));
    }

    // `>|文件`：覆盖 noclobber
    if op == '>' && next == Some('|') {
        let (target, after) = read_word(chars, start + 2)?;
        return Ok((write_step(&format!("{prefix}>|{target}"), &target), after));
    }

    // `2>&1`：fd 复制 / 关闭；`<&0` 是文件读入；`>&词` 在 bash 里等价于 `&>词`，是写文件
    if next == Some('&') {
        let mut j = start + 2;
        // `>&-` / `2>&-`：关闭 fd
        if chars.get(j) == Some(&'-') {
            return Ok((RedirectStep::Allowed, j + 1));
        }
        // `2>&1` / `>&2`：fd 复制
        if chars.get(j).is_some_and(|c| c.is_ascii_digit()) {
            while chars.get(j).is_some_and(|c| c.is_ascii_digit()) {
                j += 1;
            }
            if op == '>' {
                return Ok((RedirectStep::Allowed, j));
            }
            let token: String = chars[start..j].iter().collect();
            return Ok((read_step(&token), j));
        }
        let (target, after) = read_word(chars, j)?;
        if op == '<' {
            return Ok((read_step(&format!("{prefix}<&{target}")), after));
        }
        return Ok((write_step(&format!("{prefix}>&{target}"), &target), after));
    }

    // `<文件`：stdin（从文件读入，同样不放行）
    if op == '<' {
        let (target, after) = read_word(chars, start + 1)?;
        return Ok((read_step(&format!("{prefix}<{target}")), after));
    }

    // 单个 `>`：写入
    let (target, after) = read_word(chars, start + 1)?;
    Ok((write_step(&format!("{prefix}>{target}"), &target), after))
}

/// 写入重定向的判定：目标是 `/dev/null` 即丢弃输出，放行。
fn write_step(token: &str, target: &str) -> RedirectStep {
    if target == "/dev/null" {
        RedirectStep::Allowed
    } else {
        RedirectStep::Write(token.to_string())
    }
}

/// 读入重定向的判定（目标是 `/dev/null` 时读不到东西，按丢弃处理）。
fn read_step(token: &str) -> RedirectStep {
    if token.ends_with("/dev/null") {
        RedirectStep::Allowed
    } else {
        RedirectStep::Read(token.to_string())
    }
}

/// 重定向operator 前缀：字首的文件描述符数字（`2>`）或 `&`（`&>`）。
///
/// 只用于让拒绝文案复述原文，不参与判定。
fn redirect_prefix(chars: &[char], start: usize) -> String {
    let mut digits = start;
    while digits > 0 && chars[digits - 1].is_ascii_digit() {
        digits -= 1;
    }
    if digits < start && is_word_start(chars, digits) {
        return chars[digits..start].iter().collect();
    }
    // `cmd &> file`：`&` 紧贴操作符时算作操作符的一部分（`&&` 不算）
    if start >= 1 && chars[start - 1] == '&' && (start < 2 || chars[start - 2] != '&') {
        return "&".into();
    }
    String::new()
}

/// 复述一段原文作为拒绝文案，过长时截断。
fn redirect_snippet(chars: &[char], start: usize, end: usize) -> String {
    let raw: String = chars[start..end.min(chars.len())].iter().collect();
    if raw.chars().count() > 40 {
        format!("{}…", raw.chars().take(40).collect::<String>())
    } else {
        raw
    }
}

/// 读取一个重定向目标词：跳过前导空白，按引号 / 反斜杠规则读取并去引号，
/// 在未加引号的元字符（空白、`;` `&` `|` `<` `>` `(` `)`）处结束。
fn read_word(chars: &[char], start: usize) -> Result<(String, usize), String> {
    let mut i = start;
    while matches!(chars.get(i), Some(' ') | Some('\t')) {
        i += 1;
    }
    let mut word = String::new();
    while let Some(&c) = chars.get(i) {
        match c {
            ' ' | '\t' | '\n' | ';' | '&' | '|' | '<' | '>' | '(' | ')' => break,
            '\'' => {
                let end = skip_quoted(chars, i + 1, '\'').ok_or("命令含未闭合的单引号")?;
                word.extend(&chars[i + 1..end]);
                i = end + 1;
            }
            '"' => {
                let end = skip_quoted(chars, i + 1, '"').ok_or("命令含未闭合的双引号")?;
                word.extend(&chars[i + 1..end]);
                i = end + 1;
            }
            '\\' => match chars.get(i + 1) {
                Some(&escaped) => {
                    word.push(escaped);
                    i += 2;
                }
                None => i += 1,
            },
            _ => {
                word.push(c);
                i += 1;
            }
        }
    }
    Ok((word, i))
}

/// 返回闭合引号的下标；`start` 是引号之后的第一个字符。
fn skip_quoted(chars: &[char], mut i: usize, quote: char) -> Option<usize> {
    while let Some(&c) = chars.get(i) {
        // 单引号内反斜杠是字面量；双引号内可转义
        if c == '\\' && quote == '"' {
            i += 2;
            continue;
        }
        if c == quote {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 跳过 `open` 处的括号与其配对内容，返回配对 `)` 之后的下标。
fn skip_balanced_parens(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while let Some(&c) = chars.get(i) {
        match c {
            '\'' => i = skip_quoted(chars, i + 1, '\'')? + 1,
            '"' => i = skip_quoted(chars, i + 1, '"')? + 1,
            '\\' => i += 2,
            '(' => {
                depth += 1;
                i += 1;
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// 消费 heredoc 正文，返回结束符之后的下标；正文里跑完也没见到结束符 → `None`。
fn skip_heredoc_body(
    chars: &[char],
    mut i: usize,
    delimiter: &str,
    strip_tabs: bool,
) -> Option<usize> {
    loop {
        let line_start = i;
        let mut line_end = i;
        while line_end < chars.len() && chars[line_end] != '\n' {
            line_end += 1;
        }
        let line: String = chars[line_start..line_end].iter().collect();
        let candidate = if strip_tabs {
            line.trim_start_matches('\t')
        } else {
            line.as_str()
        };
        if candidate == delimiter {
            return Some((line_end + 1).min(chars.len()));
        }
        if line_end >= chars.len() {
            return None;
        }
        i = line_end + 1;
    }
}

/// `#` 只有在词首才是注释，`a#b` 是普通词的一部分。
fn is_word_start(chars: &[char], i: usize) -> bool {
    match i.checked_sub(1).and_then(|prev| chars.get(prev)) {
        None => true,
        Some(c) => matches!(c, ' ' | '\t' | '\n' | ';' | '&' | '|' | '(' | ')'),
    }
}

/// `rm` 的 force 变体是否只作用于工作区内的字面量路径（清理构建 / 缓存产物的
/// 常规操作）。
///
/// 放行需同时满足：
/// - 直接调用 `rm`：`sudo` / `env` / `trap` / `bash -c` 包装一律不放行；
/// - 旗标只含 `-r` / `-R` / `-f` 的组合或 `--recursive` / `--force`；
/// - 至少一个目标，且每个目标都是字面量路径（不含 `~` `$` 反引号反斜杠与
///   通配符）—— 变量和 glob 无法静态判定删除范围；
/// - 每个目标经 canonical containment 解析后落在工作区内，且既不是工作区根、
///   也不是 git 仓库根（一次删掉整个仓库不允许）；
/// - 相对路径仅在命令内没有任何 `cd` / `pushd` / `popd` 时可放行：否则工具的
///   cwd 与解析基准不一致，只能用绝对路径表达。
///
/// 任一条件不满足返回 `false`，由调用方按「强制删除」拒绝（不可审批）。
fn forced_rm_inside_workspace(argv: &[String], workspace: &Path, command: &str) -> bool {
    let Some(program) = argv.first().and_then(|arg| arg.rsplit('/').next()) else {
        return false;
    };
    if program != "rm" {
        return false;
    }
    let Ok(canonical_workspace) = fs::canonicalize(workspace) else {
        return false;
    };
    let relative_allowed = !command_changes_directory(command);

    let mut after_separator = false;
    let mut targets: Vec<&str> = Vec::new();
    for arg in &argv[1..] {
        if !after_separator && arg == "--" {
            after_separator = true;
            continue;
        }
        if !after_separator && arg.starts_with('-') {
            if !rm_flag_is_supported(arg) {
                return false;
            }
            continue;
        }
        targets.push(arg);
    }

    !targets.is_empty()
        && targets.iter().all(|target| {
            if !is_literal_path(target) {
                return false;
            }
            if !Path::new(target).is_absolute() && !relative_allowed {
                return false;
            }
            match resolve_write_path(workspace, target) {
                Ok(resolved) => resolved != canonical_workspace && !resolved.join(".git").exists(),
                Err(_) => false,
            }
        })
}

/// 命令内是否可能出现目录切换（`cd` / `pushd` / `popd`）。
///
/// 切段失败时按「可能切换」处理（fail-closed）；本条评估之前，无法解析的命令
/// 已经被拒绝，这里只是不引入更宽的假设。
fn command_changes_directory(command: &str) -> bool {
    match split_segments(command) {
        Ok(segments) => segments.iter().any(|seg| {
            matches!(
                seg.argv.first().map(String::as_str),
                Some("cd") | Some("pushd") | Some("popd")
            )
        }),
        Err(_) => true,
    }
}

/// `rm` 旗标白名单：只接受不改变「删什么」语义的旗标。
fn rm_flag_is_supported(flag: &str) -> bool {
    match flag {
        "--recursive" | "--force" => true,
        other => other.strip_prefix('-').is_some_and(|chars| {
            !chars.is_empty()
                && !chars.starts_with('-')
                && chars.chars().all(|c| matches!(c, 'r' | 'R' | 'f'))
        }),
    }
}

/// 字面量路径：含变量、通配符或 `~` 的路径无法静态判定，一律不放行。
fn is_literal_path(raw: &str) -> bool {
    !raw.is_empty()
        && !raw.starts_with('~')
        && !raw.contains(['*', '?', '[', ']', '{', '}', '$', '`', '\\'])
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
    use std::fs;
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
        // workspace-write：工作区外的强制删除被拒，普通 rm 放行
        let mut p = PermissionsConfig {
            sandbox: SandboxMode::WorkspaceWrite,
            ..Default::default()
        };
        assert!(p
            .assess_bash("rm -rf /var/tmp/pipi-outside", Path::new("/tmp"))
            .is_err());
        assert!(p.assess_bash("rm -r build/", Path::new("/tmp")).is_ok());
        // 工作区内的字面量强制删除放行 —— 边界与反例见
        // forced_rm_is_allowed_only_for_workspace_literals
        assert!(p.assess_bash("rm -rf build/", Path::new("/tmp")).is_ok());
        // danger-full-access：放行
        p.sandbox = SandboxMode::DangerFullAccess;
        assert!(p
            .assess_bash("rm -rf /var/tmp/pipi-outside", Path::new("/tmp"))
            .is_ok());
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

    // ---------- 沙箱重定向：只有写入文件系统的重定向才算命中 ----------

    #[test]
    fn write_redirect_allows_non_writing_forms() {
        for command in [
            // 丢弃 / 合并输出
            "duckdb -c 'select 1' 2>/dev/null",
            "timeout 900 cargo test -p x --lib 2>&1 | tail -40",
            "make >>/dev/null",
            "make > /dev/null",
            "cmd 2>&1",
            "cmd 1>&2",
            "cmd >&2",
            "cmd 2>&-",
            "cmd 2>>/dev/null",
            "cmd 2>> \"/dev/null\"",
            "cmd 2>&2 >>/dev/null",
            // heredoc / herestring：喂 stdin，正文按数据处理
            "cmd <<<word",
            "python3 - <<'PY'\nprint(1 > 0)\nPY",
            "python3 - <<'PY'\nrows = [x for x in y if x->z]\nPY",
            "python3 - <<-'PY'\n\tprint(\"a\")\n\tPY",
            "put <<A <<B\nbody A\nA\nbody B\nB",
            "cat <<EOF\n> out.txt && rm -f /tmp/whatever\nEOF",
            // 引号 / 注释 / 转义里的操作符
            "echo \"a > b\"",
            "grep '>' file.txt",
            "echo hi # > not-a-redirect",
            "printf \\>",
            "cmd | grep x",
        ] {
            assert!(
                matches!(check_write_redirect(command), RedirectCheck::Ok),
                "应放行：{command}"
            );
        }
    }

    #[test]
    fn write_redirect_flags_filesystem_writes() {
        for (command, token) in [
            ("echo hi > out.txt", ">out.txt"),
            ("make >> build.log", ">>build.log"),
            ("cmd 2> err.log", "2>err.log"),
            ("cmd &> all.log", "&>all.log"),
            ("cmd >| force.txt", ">|force.txt"),
            ("cmd <> rw.txt", "<>rw.txt"),
            ("cmd >& file.txt", ">&file.txt"),
            ("exec > trace.log", ">trace.log"),
            ("cmd 2>&1 > out.txt", ">out.txt"),
            ("cmd > $OUT", ">$OUT"),
            ("cmd > \"quoted path.txt\"", ">quoted path.txt"),
            ("cmd >", ">"),
            ("echo 'a > b' > c", ">c"),
            ("ls foo>out.txt", ">out.txt"),
            ("duckdb -c 'select 1' > /tmp/x 2>/dev/null", ">/tmp/x"),
            ("ln -s /outside link && printf x > link", ">link"),
        ] {
            match check_write_redirect(command) {
                RedirectCheck::Write(found) => assert_eq!(found, token, "{command}"),
                RedirectCheck::Ok => panic!("应拒绝：{command}"),
                RedirectCheck::Read(found) => panic!("{command} 应为写入而非读入：{found}"),
                RedirectCheck::Unparseable(reason) => panic!("{command} 不应不可分析：{reason}"),
            }
        }
    }

    /// 输入侧重定向保持拒绝：与 read 工具的工作区包含检查同向，不放宽。
    #[test]
    fn write_redirect_flags_file_reads() {
        for (command, token) in [
            ("cat < /etc/passwd", "</etc/passwd"),
            ("wc -l < file.txt", "<file.txt"),
            ("cmd <&0", "<&0"),
            ("diff <(ls a) <(ls b)", "<(ls a)"),
            ("cmd < /dev/zero", "</dev/zero"),
        ] {
            match check_write_redirect(command) {
                RedirectCheck::Read(found) => assert_eq!(found, token, "{command}"),
                RedirectCheck::Ok => panic!("应拒绝：{command}"),
                RedirectCheck::Write(found) => panic!("{command} 应为读入而非写入：{found}"),
                RedirectCheck::Unparseable(reason) => panic!("{command} 不应不可分析：{reason}"),
            }
        }
        // 读 /dev/null 等于读不到东西，按丢弃处理
        assert!(matches!(
            check_write_redirect("cmd < /dev/null"),
            RedirectCheck::Ok
        ));
    }

    #[test]
    fn write_redirect_fails_closed_on_unanalyzable_forms() {
        for command in [
            "cmd <<EOF",         // heredoc 没有正文 / 结束符
            "cmd <<EOF\nbody\n", // 正文跑完也没见到结束符
            "cmd <(a",           // 进程替换括号不配对
        ] {
            assert!(
                matches!(check_write_redirect(command), RedirectCheck::Unparseable(_)),
                "应判为不可分析：{command}"
            );
        }
        // 未闭合引号不在这里判死：段落切分会以同样的理由拒绝整条命令
        assert!(matches!(
            check_write_redirect("echo \"unclosed > out"),
            RedirectCheck::Ok
        ));
        assert!(split_segments("echo \"unclosed > out").is_err());
    }

    #[test]
    fn assessment_reports_the_offending_redirect() {
        let cfg = workspace_write_config();
        assert_eq!(
            cfg.assess_bash_classified("timeout 900 cargo test 2>&1 | tail -40", Path::new("/tmp")),
            BashAssessment::Allowed
        );
        let denied = cfg.assess_bash_classified("cargo test > out.txt", Path::new("/tmp"));
        let BashAssessment::HardDenied(message) = denied else {
            panic!("写入重定向必须硬拒");
        };
        assert!(message.contains("「>out.txt」"), "{message}");
        // 报错要教模型怎么改：点明允许的构造
        assert!(message.contains("2>/dev/null"), "{message}");
    }

    fn workspace_write_config() -> PermissionsConfig {
        PermissionsConfig {
            tools: vec!["bash".into()],
            bash: BashPermissions {
                mode: BashMode::Denylist,
                commands: Vec::new(),
            },
            sandbox: SandboxMode::WorkspaceWrite,
        }
    }

    // ---------- 强制删除：工作区内的字面量目标 ----------

    #[test]
    fn forced_rm_is_allowed_only_for_workspace_literals() {
        let root = std::env::temp_dir().join(format!("pipi-perm-{}", crate::session::new_id()));
        let workspace = root.join("ws");
        fs::create_dir_all(workspace.join(".cache/bench3/tmp")).unwrap();
        fs::create_dir_all(workspace.join("Polymarket/.git")).unwrap();
        fs::write(workspace.join("stale.txt"), "x").unwrap();
        let cfg = workspace_write_config();
        let cache_tmp = workspace.join(".cache/bench3/tmp");

        let allowed = [
            format!("rm -rf {}", cache_tmp.display()),
            format!("rm -f {}", workspace.join("stale.txt").display()),
            format!("rm -rf -- {}", cache_tmp.display()),
            format!("rm -fr {}", cache_tmp.display()),
            "rm -rf .cache/bench3/tmp".to_string(),
        ];
        for command in allowed {
            assert_eq!(
                cfg.assess_bash_classified(&command, &workspace),
                BashAssessment::Allowed,
                "应放行：{command}"
            );
        }

        let denied = [
            "rm -rf /".to_string(),
            "rm -rf /tmp".to_string(),
            format!("rm -rf {}/..", workspace.display()),
            format!("rm -rf {}", workspace.display()), // 工作区根自身
            format!("rm -rf {}", workspace.join("Polymarket").display()), // git 仓库根
            format!("rm -rf {}/.cache/bench3/*", workspace.display()),
            "rm -rf $HOME/x".to_string(),
            "rm -rf ~/x".to_string(),
            "rm -rf $DIR/bench3/tmp".to_string(), // 变量
            "cd /tmp && rm -rf .cache/bench3/tmp".to_string(), // 含 cd 的相对路径
            format!("sudo rm -rf {}", cache_tmp.display()),
            format!("bash -c 'rm -rf {}'", cache_tmp.display()),
            format!("rm -rf {} --one-file-system", cache_tmp.display()),
        ];
        for command in denied {
            assert!(
                matches!(
                    cfg.assess_bash_classified(&command, &workspace),
                    BashAssessment::HardDenied(_)
                ),
                "应拒绝：{command}"
            );
        }

        fs::remove_dir_all(&root).ok();
    }
}
