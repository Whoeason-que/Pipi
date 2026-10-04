use super::*;

#[test]
fn head_truncates_by_lines_bytes_and_first_line() {
    // 行数优先：3000 行被截到上限，尾部丢弃
    let lines: String = (0..3000).map(|i| format!("line {i}\n")).collect();
    let r = truncate_head(&lines, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
    assert!(r.truncated);
    assert_eq!(r.truncated_by, Some("lines"));
    assert_eq!(r.output_lines, DEFAULT_MAX_LINES);
    assert_eq!(r.total_lines, 3000);
    assert!(r.content.starts_with("line 0\n"));
    assert!(r.content.ends_with("line 1999"));

    // 字节上限先到：按字节截断
    let bytes: String = (0..2000).map(|_| "x".repeat(100) + "\n").collect();
    let r = truncate_head(&bytes, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
    assert!(r.truncated);
    assert_eq!(r.truncated_by, Some("bytes"));
    assert!(r.output_bytes <= DEFAULT_MAX_BYTES);

    // 单行就超上限：标记出来，让调用方知道截断点不可读
    let one_line = format!("{}\nshort\n", "y".repeat(60_000));
    let r = truncate_head(&one_line, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
    assert!(r.first_line_exceeds_limit);
    assert!(r.truncated);

    // 未截断时原样返回（保留结尾换行，与 pi 一致）
    let r = truncate_head("hello\nworld\n", DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
    assert!(!r.truncated);
    assert_eq!(r.total_lines, 2);
    assert_eq!(r.content, "hello\nworld\n");
}

#[test]
fn tail_truncates_by_lines_bytes_and_keeps_partial() {
    // 行数优先：保留最后 DEFAULT_MAX_LINES 行
    let lines: String = (0..3000).map(|i| format!("line {i}\n")).collect();
    let r = truncate_tail(&lines, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
    assert!(r.truncated);
    assert_eq!(r.truncated_by, Some("lines"));
    assert_eq!(r.output_lines, DEFAULT_MAX_LINES);
    assert!(r.content.starts_with("line 1000"));
    assert!(r.content.ends_with("line 2999"));

    // 字节上限先到：保留尾部字节
    let bytes: String = (0..2000).map(|_| "z".repeat(100) + "\n").collect();
    let r = truncate_tail(&bytes, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
    assert!(r.truncated);
    assert!(r.output_bytes <= DEFAULT_MAX_BYTES + 101);
    assert!(r.content.ends_with('z'));
    assert!(!r.content.is_empty());

    // 单行超上限：保留部分行并标记
    let one_line = format!("ok\n{}\n", "w".repeat(60_000));
    let r = truncate_tail(&one_line, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
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
