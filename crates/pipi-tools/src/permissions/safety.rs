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

fn executable_name(raw: &str) -> Option<&str> {
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

fn rm_args_include_force_option(args: &[String]) -> bool {
    args.iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| {
            arg == "--force"
                || arg
                    .strip_prefix('-')
                    .is_some_and(|flags| !flags.starts_with('-') && flags.contains('f'))
        })
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
}
