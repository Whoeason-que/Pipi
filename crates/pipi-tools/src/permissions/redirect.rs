//! `workspace-write` 沙箱下的重定向静态检查：区分「写入文件系统」与
//! 「从文件读入」，丢弃 / 合并输出与 heredoc 放行。

use super::parse::{
    is_word_start, read_word, skip_balanced_parens, skip_heredoc_body, skip_quoted,
};

/// 重定向静态检查的结论。
pub(super) enum RedirectCheck {
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
pub(super) enum RedirectStep {
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
pub(super) fn check_write_redirect(command: &str) -> RedirectCheck {
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
                                "heredoc missing delimiter \"{delimiter}\""
                            ));
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
        return RedirectCheck::Unparseable(format!("heredoc missing delimiter \"{delimiter}\""));
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
            skip_balanced_parens(chars, start + 1).ok_or("Unmatched parentheses in process substitution".to_string())?;
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
                return Err("heredoc missing delimiter".into());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::parse::split_segments;

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
}
