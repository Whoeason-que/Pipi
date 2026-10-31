//! 渲染进 prompt 的文本助手。
//!
//! 上下文预算（`pipi-context`）与提示词渲染（`pipi-venv::prompt`）共用同一套
//! 转义规则，所以它住在契约层，而不是任一侧。

/// 转义 XML 特殊字符（不改变路径分隔符或控制字符）。
pub fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// 将文件系统路径安全地渲染进 prompt：规范化分隔符、可见化控制字符并
/// 转义 XML 特殊字符，避免路径破坏上下文标签或注入额外行。
pub fn escape_path_for_prompt(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let mut sanitized = String::with_capacity(normalized.len());
    for character in normalized.chars() {
        match character {
            '\n' => sanitized.push_str("\\n"),
            '\r' => sanitized.push_str("\\r"),
            '\t' => sanitized.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write;
                write!(sanitized, "\\u{{{:x}}}", character as u32).unwrap();
            }
            character => sanitized.push(character),
        }
    }
    escape_xml_text(&sanitized)
}

/// 按字符数截断，超出时补省略号（标题、摘要与检索结果共用）。
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max_chars).collect();
    cut.push('…');
    cut
}

/// 在 UTF-8 字符边界上截断到不超过 `max_bytes` 字节。
pub fn truncate_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    if max_bytes >= text.len() {
        return text;
    }
    let mut index = max_bytes;
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    &text[..index]
}

/// 取文本尾部不超过 `max_bytes` 字节的完整字符片段（命令 stderr 摘要用）。
pub fn tail_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    if max_bytes >= text.len() {
        return text;
    }
    let mut index = text.len() - max_bytes;
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    &text[index..]
}

#[cfg(test)]
mod tests {
    use super::escape_path_for_prompt;

    #[test]
    fn path_escaping_preserves_prompt_structure() {
        assert_eq!(
            escape_path_for_prompt("C:\\work\n<unsafe>&\"'"),
            "C:/work\\n&lt;unsafe&gt;&amp;&quot;&apos;"
        );
    }
}
