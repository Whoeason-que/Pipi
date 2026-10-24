//! 引号感知的命令切分与词法读取：连接符切段、白/黑名单条目匹配，以及重定向
//! 扫描复用的 token 级助手。

use pipi_protocol::permissions::Segment;

/// 引号感知的命令切分。
///
/// 先在原始文本上按引号状态机找连接符（引号内的 `;`、`&&` 不拆段，
/// `git commit -m "a; rm -rf /"` 是一个段），再用 shlex 对每段分词。
/// 无法安全解析（未闭合引号等）返回 Err，由调用方 fail-closed。
pub fn split_segments(command: &str) -> Result<Vec<Segment>, String> {
    let mut out = Vec::new();
    for raw in split_raw_on_connectors(command)? {
        let Some(argv) = shlex::split(raw.trim()) else {
            return Err(format!(
                "Command segment \"{raw}\" contains unclosed quote or escape"
            ));
        };
        if argv.is_empty() {
            continue;
        }
        let text = shlex::try_join(argv.iter().map(String::as_str))
            .map_err(|_| "Command segments cannot be reconstructed".to_string())?;
        out.push(Segment { argv, text });
    }
    if out.is_empty() {
        return Err("Empty command".into());
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
        return Err("Command contains unclosed quote".into());
    }
    push_segment(&mut cur, &mut segments);
    Ok(segments)
}

/// 白/黑名单条目匹配。交互审批层需要用同一规则判断需要持久化的未命中段落。
pub fn matches_entry(entry: &str, segment: &str) -> bool {
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

/// 读取一个重定向目标词：跳过前导空白，按引号 / 反斜杠规则读取并去引号，
/// 在未加引号的元字符（空白、`;` `&` `|` `<` `>` `(` `)`）处结束。
pub(super) fn read_word(chars: &[char], start: usize) -> Result<(String, usize), String> {
    let mut i = start;
    while matches!(chars.get(i), Some(' ') | Some('\t')) {
        i += 1;
    }
    let mut word = String::new();
    while let Some(&c) = chars.get(i) {
        match c {
            ' ' | '\t' | '\n' | ';' | '&' | '|' | '<' | '>' | '(' | ')' => break,
            '\'' => {
                let end = skip_quoted(chars, i + 1, '\'')
                    .ok_or("Command contains unclosed single quote")?;
                word.extend(&chars[i + 1..end]);
                i = end + 1;
            }
            '"' => {
                let end = skip_quoted(chars, i + 1, '"')
                    .ok_or("Command contains unclosed double quote")?;
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
pub(super) fn skip_quoted(chars: &[char], mut i: usize, quote: char) -> Option<usize> {
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
pub(super) fn skip_balanced_parens(chars: &[char], open: usize) -> Option<usize> {
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
pub(super) fn skip_heredoc_body(
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
pub(super) fn is_word_start(chars: &[char], i: usize) -> bool {
    match i.checked_sub(1).and_then(|prev| chars.get(prev)) {
        None => true,
        Some(c) => matches!(c, ' ' | '\t' | '\n' | ';' | '&' | '|' | '(' | ')'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
