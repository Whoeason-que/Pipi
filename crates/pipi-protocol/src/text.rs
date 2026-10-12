//! 渲染进 prompt 的文本助手。
//!
//! 上下文预算（`pipi-context`）与提示词渲染（`pipi-venv::prompt`）共用同一套
//! 转义规则，所以它住在契约层，而不是任一侧。

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
    sanitized
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
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
