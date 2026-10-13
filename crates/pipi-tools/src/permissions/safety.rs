//! 危险命令检测 —— 移植自 openai/codex（Apache-2.0）：
//! `codex-rs/shell-command/src/command_safety/is_dangerous_command.rs`。
//!
//! 保留了 argv 层的核心语义：`rm` 的 force 变体识别、`sudo` / `env` / `trap`
//! 包装器解包、包装深度上限（fail-closed）。
//!
//! 与上游的差异（按需裁剪）：
//! - 上游用 tree-sitter 解析 `bash -c '<script>'` 里的字面命令；这里改用
//!   shlex 切分脚本的段（见 [`super::split_segments`]），脚本无法安全解析时
//!   视为危险（fail-closed），不引入 tree-sitter 依赖。
//! - Windows / PowerShell 规则未移植（Pipi 当前只面向 POSIX bash）。

/// 被 `dangerous_command_match` 命中的规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DangerousCommandMatch {
    /// `rm` 调用带 force 选项。
    ForcedRm,
    /// 其他危险规则。
    Other,
}

const MAX_DANGEROUS_COMMAND_WRAPPER_DEPTH: usize = 8;

/// 返回已参数化的命令命中的危险规则（POSIX 语义）。
pub fn dangerous_command_match(command: &[String]) -> Option<DangerousCommandMatch> {
    dangerous_command_match_with_depth(command, 0)
}

fn dangerous_command_match_with_depth(
    command: &[String],
    wrapper_depth: usize,
) -> Option<DangerousCommandMatch> {
    if wrapper_depth > MAX_DANGEROUS_COMMAND_WRAPPER_DEPTH {
        // 与上游一致：包装过深时宁可误报也不放行
        return Some(DangerousCommandMatch::Other);
    }

    if let Some(matched) = dangerous_command_match_for_exec(command, wrapper_depth) {
        return Some(matched);
    }

    // `bash -c '<script>'`：用 shlex 切分脚本的段，逐段递归检查
    if let Some(matched) = unwrap_shell_lc(command, wrapper_depth) {
        return Some(matched);
    }

    None
}

/// 可执行名：取路径最后一段（空段视作无）。`rm` 的判据（[`rm_args_include_force_option`]
/// 与 `super::rm::forced_rm_inside_workspace`）共用，保证 `/bin/rm`、`./rm` 与 `rm` 一致。
pub(super) fn executable_name(raw: &str) -> Option<&str> {
    raw.rsplit('/').next().filter(|name| !name.is_empty())
}

fn dangerous_command_match_for_exec(
    command: &[String],
    wrapper_depth: usize,
) -> Option<DangerousCommandMatch> {
    let cmd0 = command.first().and_then(|c| executable_name(c));

    match cmd0 {
        // rm 带 force 选项（含组合短旗标与 --force）
        Some("rm") if rm_args_include_force_option(&command[1..]) => {
            Some(DangerousCommandMatch::ForcedRm)
        }
        // sudo <cmd>：直接检查 <cmd>
        Some("sudo") => dangerous_command_match_with_depth(&command[1..], wrapper_depth + 1),
        // 跳过 env 的环境变量赋值再检查真正的命令
        Some("env") => dangerous_command_match_for_env(command, wrapper_depth),
        // trap 的第一个操作数是 shell 源码
        Some("trap") => dangerous_command_match_for_trap(command, wrapper_depth),
        _ => None,
    }
}

fn dangerous_command_match_for_env(
    command: &[String],
    wrapper_depth: usize,
) -> Option<DangerousCommandMatch> {
    let mut command_index = 1;
    while let Some(argument) = command.get(command_index) {
        if argument == "--" {
            command_index += 1;
            break;
        }
        if matches!(argument.as_str(), "-i" | "--ignore-environment")
            || argument
                .split_once('=')
                .is_some_and(|(name, _)| !name.is_empty() && !name.starts_with('-'))
        {
            command_index += 1;
            continue;
        }
        break;
    }
    dangerous_command_match_with_depth(&command[command_index..], wrapper_depth + 1)
}

fn dangerous_command_match_for_trap(
    command: &[String],
    wrapper_depth: usize,
) -> Option<DangerousCommandMatch> {
    let mut action_index = 1;
    if command.get(action_index).is_some_and(|a| a == "--") {
        action_index += 1;
    }
    let action = command
        .get(action_index)
        .filter(|action| !action.starts_with('-'))?;
    let shell_command = vec!["sh".to_string(), "-c".to_string(), action.clone()];
    dangerous_command_match_with_depth(&shell_command, wrapper_depth + 1)
}

/// `bash`/`sh`/`zsh` 的 `-c`/`-lc` 脚本：切段后逐段递归。
///
/// 上游用 tree-sitter 解析完整 shell 语法（控制流、子 shell 里的字面命令）；
/// 这里不引入 tree-sitter，改为：脚本里出现语法关键字或命令替换时视为
/// 无法静态分析 → 按危险处理（fail-closed）。纯命令序列（含管道/重定向）
/// 仍可正常切段递归。
fn unwrap_shell_lc(command: &[String], wrapper_depth: usize) -> Option<DangerousCommandMatch> {
    let cmd0 = command.first().and_then(|c| executable_name(c));
    if !matches!(cmd0, Some("bash") | Some("sh") | Some("zsh")) {
        return None;
    }
    let mut script: Option<&String> = None;
    let mut args = command[1..].iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "-lc" | "-cl" => {
                script = args.next();
                break;
            }
            _ => continue,
        }
    }
    let script = script?;
    if script_has_grammar(script) {
        // 无法证明安全 —— 宁可拒绝不可放行
        return Some(DangerousCommandMatch::Other);
    }
    match super::split_segments(script) {
        Ok(segments) => segments
            .iter()
            .find_map(|seg| dangerous_command_match_with_depth(&seg.argv, wrapper_depth + 1)),
        // 解析不了就当危险处理
        Err(_) => Some(DangerousCommandMatch::Other),
    }
}

/// shell 语法关键字与命令替换标记：出现即认为脚本无法静态分析。
const SHELL_GRAMMAR_KEYWORDS: [&str; 16] = [
    "if", "then", "elif", "else", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "function", "select", "time", "{",
];

fn script_has_grammar(script: &str) -> bool {
    if script.contains("$(") || script.contains('`') {
        return true;
    }
    shlex::split(script)
        .map(|tokens| {
            tokens
                .iter()
                .any(|t| SHELL_GRAMMAR_KEYWORDS.contains(&t.as_str()))
        })
        .unwrap_or(true) // 分词失败同样无法分析
}

/// `rm` 旗标参数的共享解码（危险命令识别与工作区放行判据共用，见 `super::rm`）。
///
/// 把 `argv[1..]` 变成有序事件：`--` 之前 `-` / `--` 开头的参数按旗标解码，
/// `--` 之后的参数（即使以 `-` 开头）一律是操作数。两处判据只解读各自关心的位：
/// - [`rm_args_include_force_option`] 只要 `force` —— **任何**短簇里出现 `f`
///   即命中（含白名单外的 `-xfoo`），长旗标不逐字符扫描（`--foo` 不算）；
/// - `forced_rm_inside_workspace` 要求旗标全在白名单内（`unknown == false`）。
pub(super) fn decode_rm_args(args: &[String]) -> impl Iterator<Item = RmArg<'_>> {
    let mut after_separator = false;
    args.iter().map(move |arg| {
        if after_separator {
            return RmArg::Operand(arg);
        }
        match arg.as_str() {
            "--" => {
                after_separator = true;
                RmArg::Separator
            }
            raw if raw.starts_with("--") => RmArg::Flag(decode_long_flag(raw)),
            raw if raw.starts_with('-') => RmArg::Flag(decode_short_flags(&raw[1..])),
            _ => RmArg::Operand(arg),
        }
    })
}

/// 长旗标：只有 `--recursive` / `--force` 在白名单内；其余整体判为白名单外，
/// 不逐字符扫描（`--foo` 里的 `f` 不算 force —— 既有判据如此，保持不放宽）。
fn decode_long_flag(raw: &str) -> RmFlags {
    match raw {
        "--recursive" => RmFlags {
            force: false,
            unknown: false,
        },
        "--force" => RmFlags {
            force: true,
            unknown: false,
        },
        _ => RmFlags {
            force: false,
            unknown: true,
        },
    }
}

/// 短旗标簇：逐字符解码。`r` / `R` 是白名单内，`f` 同时是白名单内与 force，
/// 其余字符标记 `unknown`（`-xfoo` 因此 force 与 unknown 同时为真）。
fn decode_short_flags(chars: &str) -> RmFlags {
    let mut flags = RmFlags::default();
    if chars.is_empty() {
        // 孤立的 `-`：不是白名单旗标
        flags.unknown = true;
        return flags;
    }
    for c in chars.chars() {
        match c {
            'f' => flags.force = true,
            'r' | 'R' => {}
            _ => flags.unknown = true,
        }
    }
    flags
}

/// 一个旗标参数的解码结果（两处判据需要的全部信息）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct RmFlags {
    /// 出现 `f`：短簇任意位置，或 `--force`。
    pub force: bool,
    /// 含白名单（`-r` / `-R` / `-f` 的组合、`--recursive` / `--force`）之外的内容。
    pub unknown: bool,
}

/// `rm` 参数解码后的事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RmArg<'a> {
    /// 旗标参数（`-` / `--` 开头），携带逐字符解码结果。
    Flag(RmFlags),
    /// `--` 分隔符：之后参数一律是操作数。
    Separator,
    /// 操作数（`--` 之后即使以 `-` 开头也在此）。
    Operand(&'a str),
}

/// `rm` 参数里是否出现 force 选项：`--force` 或任意短旗标簇中的 `f`（`--` 之后不算）。
///
/// 这是**危险命令识别**的宽判据：白名单外的簇含 `f`（如 `-xfoo`）同样命中；
/// 工作区放行例外另按 `unknown` 标记整体拒绝，两处语义保持各自原样。
fn rm_args_include_force_option(args: &[String]) -> bool {
    decode_rm_args(args).any(|arg| matches!(arg, RmArg::Flag(flags) if flags.force))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec_str(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn rm_rf_is_dangerous() {
        assert_eq!(
            dangerous_command_match(&vec_str(&["rm", "-rf", "/"])),
            Some(DangerousCommandMatch::ForcedRm)
        );
        assert_eq!(
            dangerous_command_match(&vec_str(&["rm", "-f", "/"])),
            Some(DangerousCommandMatch::ForcedRm)
        );
    }

    #[test]
    fn forced_rm_variants_are_dangerous() {
        for command in [
            vec_str(&["/bin/rm", "-fr", "/tmp/example"]),
            vec_str(&["rm", "-r", "-f", "/tmp/example"]),
            vec_str(&["rm", "--force", "/tmp/example"]),
            vec_str(&["rm", "/tmp/example", "-f"]),
            vec_str(&["sudo", "rm", "-rf", "/tmp/example"]),
            vec_str(&["env", "TARGET=/tmp/example", "rm", "-rf", "/tmp/example"]),
            vec_str(&["trap", "rm -rf /tmp/example", "EXIT"]),
        ] {
            assert_eq!(
                dangerous_command_match(&command),
                Some(DangerousCommandMatch::ForcedRm),
                "{command:?}"
            );
        }
    }

    #[test]
    fn deeply_nested_command_wrappers_fail_closed() {
        for (depth, expected) in [
            (
                MAX_DANGEROUS_COMMAND_WRAPPER_DEPTH,
                DangerousCommandMatch::ForcedRm,
            ),
            (
                MAX_DANGEROUS_COMMAND_WRAPPER_DEPTH + 1,
                DangerousCommandMatch::Other,
            ),
        ] {
            let command = std::iter::repeat_n("env", depth)
                .chain(["rm", "-rf", "/tmp/example"])
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            assert_eq!(dangerous_command_match(&command), Some(expected));
        }
    }

    #[test]
    fn forced_rm_in_shell_scripts_is_dangerous() {
        for script in [
            "echo x && rm -rf /tmp/example",
            "echo x; rm --force /tmp/example",
            "printf x | rm -rf /tmp/example",
            "bash -c 'rm -rf /tmp/example'",
        ] {
            let command = vec_str(&["bash", "-lc", script]);
            assert_eq!(
                dangerous_command_match(&command),
                Some(DangerousCommandMatch::ForcedRm),
                "{script}"
            );
        }
    }

    #[test]
    fn unparseable_script_fails_closed() {
        // 引号未闭合 / 控制流粘连接符 —— 无法证明安全，按危险处理
        for script in [
            "if test -d /tmp/x; then rm -rf /tmp/x; fi",
            "echo \"$(rm x)",
        ] {
            let command = vec_str(&["bash", "-lc", script]);
            assert_eq!(
                dangerous_command_match(&command),
                Some(DangerousCommandMatch::Other),
                "{script}"
            );
        }
    }

    #[test]
    fn non_forced_or_non_literal_rm_is_not_dangerous() {
        for command in [
            vec_str(&["rm", "-r", "/tmp/example"]),
            vec_str(&["rm", "--", "-f"]),
            vec_str(&["rm", "/tmp/file"]),
            vec_str(&["env", "TARGET=/tmp/x", "rm", "-r", "/tmp/example"]),
            vec_str(&["bash", "-lc", "rm -r /tmp/example"]),
        ] {
            assert_eq!(dangerous_command_match(&command), None, "{command:?}");
        }
    }

    /// 解码事件的测试摘要：`Force` 表示 `force == true` 的旗标。
    #[derive(Debug, PartialEq, Eq)]
    enum Event {
        Force { unknown: bool },
        Flag { unknown: bool },
        Separator,
        Operand(String),
    }

    fn events(args: &[&str]) -> Vec<Event> {
        // 借用不跨语句：Vec 在函数内被完整消费成拥有所有权的摘要
        let args = vec_str(args);
        decode_rm_args(&args)
            .map(|arg| match arg {
                RmArg::Flag(flags) if flags.force => Event::Force {
                    unknown: flags.unknown,
                },
                RmArg::Flag(flags) => Event::Flag {
                    unknown: flags.unknown,
                },
                RmArg::Separator => Event::Separator,
                RmArg::Operand(operand) => Event::Operand(operand.to_string()),
            })
            .collect()
    }

    #[test]
    fn shared_flag_decoder_handles_clusters_and_separator() {
        use Event::{Flag, Force, Operand, Separator};
        assert_eq!(events(&["-rf"]), vec![Force { unknown: false }]);
        assert_eq!(events(&["-fr"]), vec![Force { unknown: false }]);
        assert_eq!(events(&["-r"]), vec![Flag { unknown: false }]);
        assert_eq!(events(&["-f"]), vec![Force { unknown: false }]);
        assert_eq!(events(&["--force"]), vec![Force { unknown: false }]);
        assert_eq!(events(&["--recursive"]), vec![Flag { unknown: false }]);
        assert_eq!(events(&["--"]), vec![Separator]);
        // 只含 `f` 的非白名单簇：force 与 unknown 同时为真
        assert_eq!(events(&["-xfoo"]), vec![Force { unknown: true }]);
        // 白名单外的长旗标不逐字符扫描：`--foo` 里的 `f` 不算 force
        assert_eq!(events(&["--foo"]), vec![Flag { unknown: true }]);
        // 孤立的 `-` 不是白名单旗标
        assert_eq!(events(&["-"]), vec![Flag { unknown: true }]);
        // `--` 之后以 `-` 开头的是操作数，不参与旗标解码
        assert_eq!(
            events(&["--", "-f"]),
            vec![Separator, Operand("-f".to_string())]
        );
        assert_eq!(
            events(&["--", "--force"]),
            vec![Separator, Operand("--force".to_string())]
        );
    }

    #[test]
    fn shared_decoder_preserves_force_predicate_edges() {
        assert!(rm_args_include_force_option(&vec_str(&["-rf", "/tmp/x"])));
        assert!(rm_args_include_force_option(&vec_str(&["/tmp/x", "-f"])));
        // 白名单外的簇含 f 仍命中（危险识别的宽判据，历史语义）
        assert!(rm_args_include_force_option(&vec_str(&["-xfoo"])));
        // `--` 之后的 `-f` 不算
        assert!(!rm_args_include_force_option(&vec_str(&["--", "-f"])));
        assert!(!rm_args_include_force_option(&vec_str(&["--foo"])));
        assert!(!rm_args_include_force_option(&vec_str(&["-r"])));
    }
}
