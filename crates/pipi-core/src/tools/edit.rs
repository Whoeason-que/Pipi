//! edit 工具。移植自 `packages/agent/src/harness/tools/edit.ts` 与
//! `edit-diff.ts` 的核心：唯一匹配的精确替换；保留 BOM 与行尾风格；
//! 多个 edit 都对原始内容匹配、按位置排序后应用并检查重叠。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{resolve_path, AgentTool, ToolContext, ToolOutput};
use crate::types::ToolResultContent;

pub struct EditTool;

#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
}

/// 把 LF 归一后的内容按 edits 应用替换。
/// 每个 oldText 必须在原文中恰好出现一次；edits 之间不得重叠。
pub fn apply_edits(content: &str, edits: &[Edit], path: &str) -> Result<String, String> {
    if edits.is_empty() {
        return Err("Edit tool input is invalid. edits must contain at least one replacement.".into());
    }
    let mut ranges: Vec<(usize, usize, &Edit)> = Vec::new();
    for edit in edits {
        if edit.old_text.is_empty() {
            return Err("oldText must not be empty.".into());
        }
        let positions: Vec<usize> = content
            .match_indices(edit.old_text.as_str())
            .map(|(i, _)| i)
            .collect();
        match positions.len() {
            0 => {
                let preview: String = edit.old_text.chars().take(40).collect();
                return Err(format!("oldText not found in {path}: \"{preview}\""));
            }
            1 => ranges.push((positions[0], positions[0] + edit.old_text.len(), edit)),
            n => {
                return Err(format!(
                    "oldText matches {n} locations in {path}; it must be unique. Add surrounding context to make it unique."
                ))
            }
        }
    }
    ranges.sort_by_key(|(start, _, _)| *start);
    for pair in ranges.windows(2) {
        if pair[1].0 < pair[0].1 {
            return Err(
                "edits overlap; if two changes touch nearby text, merge them into one edit.".into(),
            );
        }
    }

    let mut out = String::with_capacity(content.len());
    let mut pos = 0usize;
    for (start, end, edit) in ranges {
        out.push_str(&content[pos..start]);
        out.push_str(&edit.new_text);
        pos = end;
    }
    out.push_str(&content[pos..]);
    Ok(out)
}

pub fn strip_bom(s: &str) -> (bool, &str) {
    match s.strip_prefix('\u{FEFF}') {
        Some(rest) => (true, rest),
        None => (false, s),
    }
}

pub fn detect_line_ending(s: &str) -> &'static str {
    if s.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
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

/// 简单 unified diff（公共前缀/后缀之间的变更块）。
/// 返回 (diff 文本, 首个变更行号 1-indexed)。
pub fn diff_summary(base: &str, new: &str) -> (String, Option<usize>) {
    let a: Vec<&str> = base.split('\n').collect();
    let b: Vec<&str> = new.split('\n').collect();
    let mut prefix = 0usize;
    while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < a.len() - prefix && suffix < b.len() - prefix
        && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let removed = &a[prefix..a.len() - suffix];
    let added = &b[prefix..b.len() - suffix];
    if removed.is_empty() && added.is_empty() {
        return (String::new(), None);
    }
    let mut out = format!(
        "@@ -{},{} +{},{} @@",
        prefix + 1,
        removed.len(),
        prefix + 1,
        added.len()
    );
    for line in removed {
        out.push_str("\n-");
        out.push_str(line);
    }
    for line in added {
        out.push_str("\n+");
        out.push_str(line);
    }
    (out, Some(prefix + 1))
}

fn parse_edits(value: &Value) -> Result<Vec<Edit>, String> {
    let arr = value["edits"]
        .as_array()
        .ok_or("缺少 edits 数组")?;
    arr.iter()
        .map(|e| {
            Ok(Edit {
                old_text: e["oldText"]
                    .as_str()
                    .or_else(|| e["old_text"].as_str())
                    .ok_or("edits[] 缺少 oldText")?
                    .to_string(),
                new_text: e["newText"]
                    .as_str()
                    .or_else(|| e["new_text"].as_str())
                    .ok_or("edits[] 缺少 newText")?
                    .to_string(),
            })
        })
        .collect()
}

#[async_trait]
impl AgentTool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> String {
        "Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.".into()
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
        let path = args["path"].as_str().ok_or("缺少 path")?;
        let edits = parse_edits(args)?;

        let abs = resolve_path(&ctx.workspace, path)?;
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
        tokio::fs::write(&abs, final_content)
            .await
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

    #[test]
    fn applies_unique_replacement() {
        let out = apply_edits("fn main() {\n    println!(\"hi\");\n}\n", &[
            edit("println!(\"hi\")", "println!(\"hello\")")
        ], "m.rs")
        .unwrap();
        assert!(out.contains("println!(\"hello\")"));
        assert!(!out.contains("\"hi\""));
    }

    #[test]
    fn applies_multiple_sorted_edits() {
        let out = apply_edits("aaa bbb ccc", &[edit("bbb", "B"), edit("aaa", "A")], "f")
            .unwrap();
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
}
