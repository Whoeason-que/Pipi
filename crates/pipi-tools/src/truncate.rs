//! Pipi 工具输出截断规则。
//!
//! 移植自 `packages/agent/src/harness/utils/truncate.ts`：行数（2000）与字节
//! （50KB）两个独立上限，先到先赢；绝不返回半行 —— 唯一例外是 tail 截断时
//! 单行自身超过字节上限，此时只保留该行的字节尾部并标记 `last_line_partial`。

pub const DEFAULT_MAX_LINES: usize = 2000;
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;

#[derive(Debug, Clone)]
pub struct TruncationResult {
    pub content: String,
    pub truncated: bool,
    /// "lines" | "bytes" | None
    pub truncated_by: Option<&'static str>,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub output_lines: usize,
    pub output_bytes: usize,
    /// 仅 tail 截断的边界情况：最后一行被字节上限截断。
    pub last_line_partial: bool,
    /// 仅 head 截断：首行自身超过字节上限。
    pub first_line_exceeds_limit: bool,
}

fn untruncated(content: String) -> TruncationResult {
    let total_lines = count_lines(&content);
    TruncationResult {
        output_lines: total_lines,
        total_bytes: content.len(),
        output_bytes: content.len(),
        content,
        truncated: false,
        truncated_by: None,
        total_lines,
        last_line_partial: false,
        first_line_exceeds_limit: false,
    }
}

/// 统计逻辑行数（与 pi 的 splitLinesForCounting 一致：结尾换行不算新行）。
pub fn count_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else if content.ends_with('\n') {
        content.split('\n').count() - 1
    } else {
        content.split('\n').count()
    }
}

/// 从头部保留（read 工具用）。
pub fn truncate_head(content: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
    let total_bytes = content.len();
    if total_bytes <= max_bytes && count_lines(content) <= max_lines {
        return TruncationResult {
            total_bytes,
            ..untruncated(content.to_string())
        };
    }

    let lines: Vec<&str> = content.split('\n').collect();
    let total_lines = count_lines(content);
    let first_line_exceeds_limit = lines[0].len() > max_bytes;

    let mut taken: Vec<&str> = Vec::new();
    let mut bytes = 0usize;
    let mut truncated_by = None;
    for (i, line) in lines.iter().enumerate() {
        if i >= max_lines {
            truncated_by = Some("lines");
            break;
        }
        if bytes + line.len() + 1 > max_bytes && !taken.is_empty() {
            truncated_by = Some("bytes");
            break;
        }
        bytes += line.len() + 1;
        taken.push(line);
        if i == 0 && line.len() > max_bytes {
            // 首行单独超限：收进来，交由调用方按 firstLineExceedsLimit 处理
            truncated_by = Some("bytes");
            break;
        }
    }

    let output = taken.join("\n");
    TruncationResult {
        output_lines: taken.len(),
        output_bytes: output.len(),
        content: output,
        truncated: true,
        truncated_by,
        total_lines,
        total_bytes,
        last_line_partial: false,
        first_line_exceeds_limit,
    }
}

/// 从尾部保留（bash 工具用，retain: "tail"）。
///
/// 与 pi 的细微差异：pi 在字节边界上会切出半行；这里保持整行语义 ——
/// 边界行整个丢弃；仅当单行自身超过字节上限时才保留其字节尾部并标记
/// `last_line_partial`。
pub fn truncate_tail(content: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
    let total_bytes = content.len();
    let total_lines = count_lines(content);
    if total_bytes <= max_bytes && total_lines <= max_lines {
        return TruncationResult {
            total_bytes,
            ..untruncated(content.to_string())
        };
    }

    let mut lines: Vec<&str> = content.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let total_lines = lines.len();
    let mut truncated_by = Some("lines");

    if lines.len() > max_lines {
        lines = lines[lines.len() - max_lines..].to_vec();
    }

    // 从尾部往前按字节保留整行
    let mut keep_start = 0usize;
    let mut acc = 0usize;
    for i in (0..lines.len()).rev() {
        if acc + lines[i].len() + 1 > max_bytes {
            // 单行自身超限：只保留该行的字节尾部
            if lines[i].len() > max_bytes {
                let mut cut = String::new();
                let mut b = 0usize;
                for ch in lines[i].chars().rev() {
                    if b + ch.len_utf8() > max_bytes {
                        break;
                    }
                    b += ch.len_utf8();
                    cut.insert(0, ch);
                }
                let output_bytes = cut.len();
                return TruncationResult {
                    content: cut,
                    truncated: true,
                    truncated_by: Some("bytes"),
                    total_lines,
                    total_bytes,
                    output_lines: 1,
                    output_bytes,
                    last_line_partial: true,
                    first_line_exceeds_limit: false,
                };
            }
            if i + 1 >= lines.len() {
                // 边界行是最后一行且尚未保留任何行：整行保留（不超过上限）
                keep_start = i;
            } else {
                keep_start = i + 1;
            }
            truncated_by = Some("bytes");
            break;
        }
        acc += lines[i].len() + 1;
        keep_start = i;
    }

    let output = lines[keep_start..].join("\n");
    TruncationResult {
        output_lines: lines.len() - keep_start,
        output_bytes: output.len(),
        content: output,
        truncated: true,
        truncated_by,
        total_lines,
        total_bytes,
        last_line_partial: false,
        first_line_exceeds_limit: false,
    }
}

/// pi 的 formatSize：B / KB（一位小数）。
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_truncates_by_lines() {
        let content: String = (0..3000).map(|i| format!("line {i}\n")).collect();
        let r = truncate_head(&content, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert!(r.truncated);
        assert_eq!(r.truncated_by, Some("lines"));
        assert_eq!(r.output_lines, DEFAULT_MAX_LINES);
        assert_eq!(r.total_lines, 3000);
        assert!(r.content.starts_with("line 0\n"));
        assert!(r.content.ends_with("line 1999"));
    }

    #[test]
    fn head_truncates_by_bytes() {
        let content: String = (0..2000).map(|_| "x".repeat(100) + "\n").collect();
        let r = truncate_head(&content, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert!(r.truncated);
        assert_eq!(r.truncated_by, Some("bytes"));
        assert!(r.output_bytes <= DEFAULT_MAX_BYTES);
    }

    #[test]
    fn head_first_line_exceeds_limit() {
        let content = format!("{}\nshort\n", "y".repeat(60_000));
        let r = truncate_head(&content, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert!(r.first_line_exceeds_limit);
        assert!(r.truncated);
    }

    #[test]
    fn head_no_truncation() {
        let r = truncate_head("hello\nworld\n", DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert!(!r.truncated);
        assert_eq!(r.total_lines, 2);
        // 未截断时原样返回（保留结尾换行，与 pi 一致）
        assert_eq!(r.content, "hello\nworld\n");
    }

    #[test]
    fn tail_keeps_last_lines() {
        let content: String = (0..3000).map(|i| format!("line {i}\n")).collect();
        let r = truncate_tail(&content, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert!(r.truncated);
        assert_eq!(r.truncated_by, Some("lines"));
        assert_eq!(r.output_lines, DEFAULT_MAX_LINES);
        assert!(r.content.starts_with("line 1000"));
        assert!(r.content.ends_with("line 2999"));
    }

    #[test]
    fn tail_keeps_last_bytes() {
        let content: String = (0..2000).map(|_| "z".repeat(100) + "\n").collect();
        let r = truncate_tail(&content, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert!(r.truncated);
        assert!(r.output_bytes <= DEFAULT_MAX_BYTES + 101);
        assert!(r.content.ends_with('z'));
        assert!(!r.content.is_empty());
    }

    #[test]
    fn tail_single_line_over_bytes_keeps_partial() {
        let content = format!("ok\n{}\n", "w".repeat(60_000));
        let r = truncate_tail(&content, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        assert!(r.last_line_partial);
        assert_eq!(r.truncated_by, Some("bytes"));
        assert!(r.output_bytes <= DEFAULT_MAX_BYTES);
        assert!(r.content.ends_with("w"));
    }

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(50 * 1024), "50.0KB");
    }
}
