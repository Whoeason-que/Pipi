//! `pipi-tools` 的 edit 工具。移植自 `packages/agent/src/harness/tools/edit.ts` 与
//! `edit-diff.ts`：先精确匹配，失败再退到「行尾空白 + 智能引号/破折号/空格」
//! 归一化后的模糊匹配；保留 BOM 与行尾风格；多个 edit 都对原始内容匹配、按
//! 位置排序后应用并检查重叠。模糊匹配只用于定位，未被改动的行保留原始字节。

use async_trait::async_trait;
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

use pipi_protocol::ToolResultContent;
use pipi_protocol::permissions::resolve_write_path;
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput, resolve_path};

pub struct EditTool;

#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
}

/// 一次匹配：`index/len` 是匹配空间里的字节偏移（精确匹配时是原文，模糊匹配
/// 时是归一化后的文本）。
struct MatchSpan {
    index: usize,
    len: usize,
}

/// 模糊匹配用的归一化（对齐 pi 的 `normalizeForFuzzyMatch`）：NFKC、逐行去掉
/// 行尾空白、智能引号/各种破折号/特殊空格转 ASCII。只用于定位，不写回文件。
fn normalize_for_fuzzy_match(text: &str) -> String {
    text.nfkc()
        .collect::<String>()
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .map(|ch| match ch {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

/// 按行切分并保留行尾（最后一行可能没有换行）。
fn split_lines_with_endings(content: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            lines.push(&content[start..=index]);
            start = index + 1;
        }
    }
    if start < content.len() {
        lines.push(&content[start..]);
    }
    lines
}

/// 每行的字节范围 `[start, end)`。
fn line_spans(content: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            spans.push((start, index + 1));
            start = index + 1;
        }
    }
    if start < content.len() {
        spans.push((start, content.len()));
    }
    spans
}

fn fuzzy_find(content: &str, old_text: &str) -> Option<(MatchSpan, bool)> {
    if let Some(index) = content.find(old_text) {
        return Some((
            MatchSpan {
                index,
                len: old_text.len(),
            },
            false,
        ));
    }
    let normalized_content = normalize_for_fuzzy_match(content);
    let normalized_old = normalize_for_fuzzy_match(old_text);
    if normalized_old.is_empty() {
        return None;
    }
    normalized_content
        .find(&normalized_old)
        .map(|index| (MatchSpan { index, len: normalized_old.len() }, true))
}

fn count_occurrences(content: &str, old_text: &str) -> usize {
    let normalized_content = normalize_for_fuzzy_match(content);
    let normalized_old = normalize_for_fuzzy_match(old_text);
    if normalized_old.is_empty() {
        return 0;
    }
    normalized_content.matches(&normalized_old).count()
}

/// 一次替换：`(匹配起点, 匹配长度, 替换文本)`。
type Replacement<'a> = (usize, usize, &'a str);
/// 一组落在相邻行上的替换：`(起始行, 结束行（不含）, 替换列表)`。
type ReplacementGroup<'a> = (usize, usize, Vec<Replacement<'a>>);

/// 在 `content` 上按偏移倒序应用替换（偏移相对 `base_offset`）。
fn apply_replacements(
    content: &str,
    replacements: &[Replacement<'_>],
    base_offset: usize,
) -> String {
    let mut out = content.to_string();
    for &(index, len, new_text) in replacements.iter().rev() {
        let start = index - base_offset;
        out.replace_range(start..start + len, new_text);
    }
    out
}

/// 模糊匹配后把改动叠加回原文：只有被替换命中的行从归一化文本重写，其余行
/// 保留原始字节（行尾空白、引号样式不被顺手改掉）。
fn apply_preserving_unchanged_lines(
    original: &str,
    base: &str,
    replacements: &[Replacement<'_>],
) -> Result<String, String> {
    let original_lines = split_lines_with_endings(original);
    let spans = line_spans(base);
    if original_lines.len() != spans.len() {
        return Err(
            "Cannot preserve unchanged lines: fuzzy-normalized content has a different line count."
                .into(),
        );
    }

    let mut sorted = replacements.to_vec();
    sorted.sort_by_key(|(index, _, _)| *index);
    let mut groups: Vec<ReplacementGroup> = Vec::new();
    for (index, len, new_text) in sorted {
        let range = replacement_line_range(&spans, index, len)?;
        if let Some(last) = groups.last_mut()
            && range.0 < last.1
        {
            last.1 = last.1.max(range.1);
            last.2.push((index, len, new_text));
            continue;
        }
        groups.push((range.0, range.1, vec![(index, len, new_text)]));
    }

    let mut out = String::with_capacity(original.len());
    let mut original_index = 0;
    for (start_line, end_line, group) in groups {
        for line in &original_lines[original_index..start_line] {
            out.push_str(line);
        }
        let group_start = spans[start_line].0;
        let group_end = spans[end_line - 1].1;
        out.push_str(&apply_replacements(
            &base[group_start..group_end],
            &group,
            group_start,
        ));
        original_index = end_line;
    }
    for line in &original_lines[original_index..] {
        out.push_str(line);
    }
    Ok(out)
}

fn replacement_line_range(
    spans: &[(usize, usize)],
    index: usize,
    len: usize,
) -> Result<(usize, usize), String> {
    let end = index + len;
    let start_line = spans
        .iter()
        .position(|(start, line_end)| index >= *start && index < *line_end)
        .ok_or_else(|| "Replacement range is outside the base content.".to_string())?;
    let mut end_line = start_line;
    while end_line < spans.len() && spans[end_line].1 < end {
        end_line += 1;
    }
    if end_line >= spans.len() {
        return Err("Replacement range is outside the base content.".into());
    }
    Ok((start_line, end_line + 1))
}

fn empty_old_text_error(path: &str, index: usize, total: usize) -> String {
    if total == 1 {
        format!("oldText must not be empty in {path}.")
    } else {
        format!("edits[{index}].oldText must not be empty in {path}.")
    }
}

fn not_found_error(path: &str, index: usize, total: usize) -> String {
    if total == 1 {
        format!(
            "Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines."
        )
    } else {
        format!(
            "Could not find edits[{index}] in {path}. The oldText must match exactly including all whitespace and newlines."
        )
    }
}

fn duplicate_error(path: &str, index: usize, total: usize, occurrences: usize) -> String {
    if total == 1 {
        format!(
            "Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
        )
    } else {
        format!(
            "Found {occurrences} occurrences of edits[{index}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
        )
    }
}

fn no_change_error(path: &str, total: usize) -> String {
    if total == 1 {
        format!(
            "No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
        )
    } else {
        format!("No changes made to {path}. The replacements produced identical content.")
    }
}

/// 把 LF 归一后的内容按 edits 应用替换。
/// 每个 oldText 必须恰好出现一次（先精确、再模糊）；edits 之间不得重叠。
pub fn apply_edits(content: &str, edits: &[Edit], path: &str) -> Result<String, String> {
    if edits.is_empty() {
        return Err(
            "Edit tool input is invalid. edits must contain at least one replacement.".into(),
        );
    }
    let total = edits.len();
    let normalized: Vec<Edit> = edits
        .iter()
        .map(|edit| Edit {
            old_text: normalize_to_lf(&edit.old_text),
            new_text: normalize_to_lf(&edit.new_text),
        })
        .collect();
    for (index, edit) in normalized.iter().enumerate() {
        if edit.old_text.is_empty() {
            return Err(empty_old_text_error(path, index, total));
        }
    }

    // 任一 edit 需要模糊匹配，就整体切到归一化空间匹配（与 pi 一致），这样
    // 一个 edit 的空白差异不会让同批的另一个 edit 落在不同的偏移空间里。
    let used_fuzzy = normalized
        .iter()
        .any(|edit| matches!(fuzzy_find(content, &edit.old_text), Some((_, true))));
    let base = if used_fuzzy {
        normalize_for_fuzzy_match(content)
    } else {
        content.to_string()
    };

    let mut matched: Vec<(usize, usize, usize, &str)> = Vec::new();
    for (edit_index, edit) in normalized.iter().enumerate() {
        let Some((span, _)) = fuzzy_find(&base, &edit.old_text) else {
            return Err(not_found_error(path, edit_index, total));
        };
        let occurrences = count_occurrences(&base, &edit.old_text);
        if occurrences > 1 {
            return Err(duplicate_error(path, edit_index, total, occurrences));
        }
        matched.push((span.index, span.len, edit_index, edit.new_text.as_str()));
    }
    matched.sort_by_key(|(index, _, _, _)| *index);
    for pair in matched.windows(2) {
        if pair[0].0 + pair[0].1 > pair[1].0 {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
                pair[0].2, pair[1].2
            ));
        }
    }

    let replacements: Vec<Replacement<'_>> = matched
        .iter()
        .map(|(index, len, _, new_text)| (*index, *len, *new_text))
        .collect();
    let new_content = if used_fuzzy {
        apply_preserving_unchanged_lines(content, &base, &replacements)?
    } else {
        apply_replacements(&base, &replacements, 0)
    };
    if new_content == content {
        return Err(no_change_error(path, total));
    }
    Ok(new_content)
}

pub fn strip_bom(s: &str) -> (bool, &str) {
    match s.strip_prefix('\u{FEFF}') {
        Some(rest) => (true, rest),
        None => (false, s),
    }
}

pub fn detect_line_ending(s: &str) -> &'static str {
    if s.contains("\r\n") { "\r\n" } else { "\n" }
}

pub fn normalize_to_lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

pub fn restore_line_endings(s: String, ending: &str) -> String {
    if ending == "\n" {
        s
    } else {
        s.replace('\n', ending)
    }
}

/// 基于 similar 生成的 unified diff。
/// 返回 (diff 文本, 首个变更行号 1-indexed)。
pub fn diff_summary(base: &str, new: &str) -> (String, Option<usize>) {
    if base == new {
        return (String::new(), None);
    }
    let diff = similar::TextDiff::from_lines(base, new);
    let mut first_changed = None;
    for change in diff.iter_all_changes() {
        if change.tag() == similar::ChangeTag::Delete {
            if let Some(idx) = change.old_index() {
                first_changed = Some(idx + 1);
                break;
            }
        } else if change.tag() == similar::ChangeTag::Insert
            && let Some(idx) = change.new_index()
        {
            first_changed = Some(idx + 1);
            break;
        }
    }
    let unified = diff.unified_diff().context_radius(2).to_string();
    (unified, first_changed)
}

fn edit_from(index: usize, item: &Value) -> Result<Edit, String> {
    let old_text = item["oldText"]
        .as_str()
        .or_else(|| item["old_text"].as_str())
        .ok_or_else(|| format!("edits[{index}].oldText is required"))?;
    let new_text = item["newText"]
        .as_str()
        .or_else(|| item["new_text"].as_str())
        .ok_or_else(|| format!("edits[{index}].newText is required"))?;
    Ok(Edit {
        old_text: old_text.to_string(),
        new_text: new_text.to_string(),
    })
}

/// 接受模型常见的几种形状（数组、JSON 字符串、单个对象、顶层 oldText/newText），
/// 与 pi 的 `prepareEditArguments` 同一取向：能修的参数先修，别让一次可恢复的
/// 调用变成一轮重试。
fn parse_edits(value: &Value) -> Result<Vec<Edit>, String> {
    let items = match value.get("edits") {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::String(text)) => {
            let parsed: Value = serde_json::from_str(text).map_err(|e| {
                format!("edits must be a JSON array of {{oldText, newText}} objects: {e}")
            })?;
            match parsed {
                Value::Array(items) => items,
                other if other.get("oldText").is_some() || other.get("old_text").is_some() => {
                    vec![other]
                }
                _ => {
                    return Err(
                        "edits must contain at least one replacement.".to_string()
                    );
                }
            }
        }
        Some(Value::Object(_)) => vec![value["edits"].clone()],
        Some(Value::Null) | None => {
            if value.get("oldText").is_some() || value.get("old_text").is_some() {
                vec![value.clone()]
            } else {
                return Err("Missing edits array".into());
            }
        }
        Some(_) => return Err("edits must be an array of {oldText, newText} objects".into()),
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| edit_from(index, item))
        .collect()
}

#[async_trait]
impl AgentTool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> String {
        "Edit a single file using text replacement. Every edits[].oldText is matched against the original file, trying an exact match first and then a match that ignores trailing whitespace and smart quotes/dashes/spaces. It must identify a unique, non-overlapping region. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file to edit (relative or absolute)" },
                "edits": {
                    "type": "array",
                    "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": { "type": "string", "description": "Exact text for one targeted replacement. It must be unique in the original file." },
                            "newText": { "type": "string", "description": "Replacement text for this targeted edit." }
                        },
                        "required": ["oldText", "newText"]
                    }
                }
            },
            "required": ["path", "edits"]
        })
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        if ctx.abort.is_aborted() {
            return Err("Operation aborted".into());
        }
        let path = args["path"].as_str().ok_or("Missing path")?;
        let edits = parse_edits(args)?;

        let abs = if matches!(
            ctx.sandbox,
            pipi_protocol::permissions::SandboxMode::WorkspaceWrite
        ) {
            resolve_write_path(&ctx.workspace, path)?
        } else {
            let abs = resolve_path(&ctx.workspace, path)?;
            ctx.ensure_writable(&abs)?;
            abs
        };
        // 同一文件的两个 edit/write 在同一批里是并行执行的：从这里到写入完成
        // 必须持有文件锁，否则后写的那次会静默覆盖先写的。
        let _file_lock = crate::file_queue::lock(&abs).await;
        let meta = tokio::fs::metadata(&abs)
            .await
            .map_err(|e| format!("Could not edit file: {path}. Error: {e}"))?;
        if !meta.is_file() {
            return Err(format!("Could not edit file: {path}. Path is not a file."));
        }
        let raw = tokio::fs::read_to_string(&abs)
            .await
            .map_err(|e| format!("Could not edit file: {path}. Error: {e}"))?;
        if ctx.abort.is_aborted() {
            return Err("Operation aborted".into());
        }

        let (has_bom, text) = strip_bom(&raw);
        let original_ending = detect_line_ending(text);
        let normalized = normalize_to_lf(text);
        let new_content = apply_edits(&normalized, &edits, path)?;
        let final_content = format!(
            "{}{}",
            if has_bom { "\u{FEFF}" } else { "" },
            restore_line_endings(new_content.clone(), original_ending)
        );
        // 原子替换（同步 fs 调用走 blocking 线程池，别占住运行时 worker）
        let target = abs.clone();
        tokio::task::spawn_blocking(move || {
            pipi_protocol::fs::write_atomic(&target, final_content)
        })
        .await
        .map_err(|e| format!("Could not edit file: {path}. Error: {e}"))?
        .map_err(|e| format!("Could not edit file: {path}. Error: {e}"))?;
        if ctx.abort.is_aborted() {
            return Err("Operation aborted".into());
        }

        let (diff, first_changed_line) = diff_summary(&normalized, &new_content);
        Ok(ToolOutput {
            content: vec![ToolResultContent::Text {
                text: format!("Successfully replaced {} block(s) in {path}.", edits.len()),
            }],
            details: Some(json!({
                "diff": diff,
                "firstChangedLine": first_changed_line,
            })),
            terminate: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(old: &str, new: &str) -> Edit {
        Edit {
            old_text: old.into(),
            new_text: new.into(),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn workspace_edit_blocks_symlink_to_outside_file() {
        let root =
            std::env::temp_dir().join(format!("pipi-edit-link-{}", pipi_test_support::new_id()));
        let workspace = root.join("workspace");
        let outside = root.join("outside.txt");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::write(&outside, "old").await.unwrap();
        std::os::unix::fs::symlink(&outside, workspace.join("link.txt")).unwrap();

        let ctx = ToolContext {
            workspace,
            memory_dir: None,
            notebook_path: None,
            read_roots: Vec::new(),
            permissions: std::sync::Arc::new(
                pipi_protocol::permissions::PermissionsConfig::default(),
            ),
            sandbox: pipi_protocol::permissions::SandboxMode::WorkspaceWrite,
            resolved_env: std::sync::Arc::new(std::collections::BTreeMap::new()),
            abort: pipi_protocol::AbortSignal::new(),
        };
        let result = EditTool
            .execute(
                &ctx,
                &json!({
                    "path": "link.txt",
                    "edits": [{"oldText": "old", "newText": "changed"}]
                }),
                &|_| {},
            )
            .await;

        assert!(result.is_err());
        assert_eq!(tokio::fs::read_to_string(&outside).await.unwrap(), "old");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn applies_unique_replacement() {
        let out = apply_edits(
            "fn main() {\n    println!(\"hi\");\n}\n",
            &[edit("println!(\"hi\")", "println!(\"hello\")")],
            "m.rs",
        )
        .unwrap();
        assert!(out.contains("println!(\"hello\")"));
        assert!(!out.contains("\"hi\""));
    }

    #[test]
    fn applies_multiple_sorted_edits() {
        let out = apply_edits("aaa bbb ccc", &[edit("bbb", "B"), edit("aaa", "A")], "f").unwrap();
        assert_eq!(out, "A B ccc");
    }

    #[test]
    fn rejects_missing_and_ambiguous() {
        assert!(apply_edits("abc", &[edit("xyz", "1")], "f").is_err());
        assert!(apply_edits("abab", &[edit("ab", "1")], "f").is_err());
    }

    #[test]
    fn rejects_overlapping_edits() {
        // 两个 edit 的匹配位置分别为 0..3 和 2..5，重叠
        let err = apply_edits("abcde", &[edit("abc", "X"), edit("cde", "Y")], "f");
        assert!(err.is_err());
    }

    #[test]
    fn line_ending_and_bom_helpers() {
        assert_eq!(detect_line_ending("a\r\nb"), "\r\n");
        assert_eq!(detect_line_ending("a\nb"), "\n");
        assert_eq!(normalize_to_lf("a\r\nb"), "a\nb");
        assert_eq!(restore_line_endings("a\nb".into(), "\r\n"), "a\r\nb");
        let (bom, rest) = strip_bom("\u{FEFF}hi");
        assert!(bom);
        assert_eq!(rest, "hi");
    }

    #[test]
    fn diff_summary_reports_first_changed_line() {
        let (diff, first) = diff_summary("a\nb\nc", "a\nB\nc");
        assert_eq!(first, Some(2));
        assert!(diff.contains("-b"));
        assert!(diff.contains("+B"));
    }

    #[test]
    fn fuzzy_match_tolerates_trailing_whitespace_and_keeps_untouched_lines() {
        let content = "line one   \nlet x = 1;   \nline three\n";
        let out = apply_edits(
            content,
            &[edit("let x = 1;\n", "let x = 2;\n")],
            "m.rs",
        )
        .unwrap();
        // 被替换的行按归一化文本重写；没碰到的行保留原始行尾空白。
        assert_eq!(out, "line one   \nlet x = 2;\nline three\n");
    }

    #[test]
    fn fuzzy_match_normalizes_smart_quotes_and_dashes() {
        let content = "say \u{201C}hi\u{201D} \u{2013} done\n";
        let out = apply_edits(
            content,
            &[edit("say \"hi\" - done", "say \"hello\" - done")],
            "m.rs",
        )
        .unwrap();
        assert_eq!(out, "say \"hello\" - done\n");
    }

    #[test]
    fn fuzzy_match_handles_crlf_old_text() {
        let out = apply_edits(
            "a\nb\nc\n",
            &[edit("a\r\nb\r\n", "a\nB\n")],
            "m.rs",
        )
        .unwrap();
        assert_eq!(out, "a\nB\nc\n");
    }

    #[test]
    fn mixed_exact_and_fuzzy_edits_apply_together() {
        let content = "alpha\nbeta   \ngamma\ndelta\n";
        let out = apply_edits(
            content,
            &[edit("alpha", "ALPHA"), edit("beta\n", "BETA\n")],
            "m.rs",
        )
        .unwrap();
        assert_eq!(out, "ALPHA\nBETA\ngamma\ndelta\n");
    }

    #[test]
    fn ambiguous_match_is_rejected_even_across_whitespace_variants() {
        let err = apply_edits("a  \na\n", &[edit("a", "x")], "f").unwrap_err();
        assert!(err.contains("2 occurrences"), "{err}");
    }

    #[test]
    fn empty_replacement_is_reported_as_a_no_change() {
        let err = apply_edits("abc", &[edit("abc", "abc")], "f").unwrap_err();
        assert!(err.contains("No changes made"), "{err}");
    }

    #[test]
    fn errors_name_the_failing_edit_index() {
        let err = apply_edits(
            "abc",
            &[edit("abc", "x"), edit("missing", "y")],
            "f",
        )
        .unwrap_err();
        assert!(err.contains("edits[1]"), "{err}");

        let overlap = apply_edits("abcde", &[edit("abc", "X"), edit("cde", "Y")], "f").unwrap_err();
        assert!(overlap.contains("edits[0]") && overlap.contains("edits[1]"), "{overlap}");
    }

    #[test]
    fn parse_edits_repairs_common_shapes() {
        let array = parse_edits(&json!({"edits": [{"oldText": "a", "newText": "b"}]})).unwrap();
        assert_eq!(array, vec![edit("a", "b")]);

        let string = parse_edits(&json!({"edits": "[{\"oldText\":\"a\",\"newText\":\"b\"}]"})).unwrap();
        assert_eq!(string, vec![edit("a", "b")]);

        let single = parse_edits(&json!({"edits": {"oldText": "a", "newText": "b"}})).unwrap();
        assert_eq!(single, vec![edit("a", "b")]);

        let top_level = parse_edits(&json!({"oldText": "a", "newText": "b"})).unwrap();
        assert_eq!(top_level, vec![edit("a", "b")]);

        let snake = parse_edits(&json!({"edits": [{"old_text": "a", "new_text": "b"}]})).unwrap();
        assert_eq!(snake, vec![edit("a", "b")]);
    }
}
