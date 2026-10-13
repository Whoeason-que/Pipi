//! git 载体的读侧：`ls-tree` + 一次 `cat-file --batch` 取回条目。
//!
//! 读回顺序按路径里的零填充 `seq` 排序（不依赖 DAG 拓扑，与
//! `docs/git-session-store.md` §3 的口径一致）；坏 blob 跳过，语义与 JSONL
//! 的 `load_session` 相同（绝不让一条坏数据拖垮整个会话）。

use std::collections::BTreeSet;

use pipi_protocol::session::{EntryKind, SessionEntry, SessionSummary};

use super::exec::GitExec;
use super::{layout, refs};

/// tree 里的一个条目文件。
#[derive(Clone)]
struct TreePath {
    seq: u64,
    kind: String,
    sha: String,
}

/// 读回一条分支上的全部条目（按 seq 升序）。
pub fn read_entries(exec: &GitExec, ref_name: &str) -> Result<Vec<SessionEntry>, String> {
    let paths = tree_paths(exec, ref_name)?;
    let mut ordered = paths
        .iter()
        .map(|path| (path.seq, path.sha.clone()))
        .collect::<Vec<_>>();
    ordered.sort_by_key(|(seq, _)| *seq);

    let mut input = String::new();
    for (_, sha) in &ordered {
        input.push_str(sha);
        input.push('\n');
    }
    let output = exec.run_with_stdin(&["cat-file", "--batch"], input.into_bytes())?;
    let blobs = parse_batch(&output);

    let mut entries = Vec::new();
    for ((seq, _), blob) in ordered.iter().zip(blobs) {
        let Some(blob) = blob else {
            eprintln!("pipi: 会话 {ref_name} 的第 {seq} 条读取失败，已跳过");
            continue;
        };
        match serde_json::from_slice::<SessionEntry>(&blob) {
            Ok(entry) => entries.push(entry),
            Err(error) => eprintln!("pipi: 会话 {ref_name} 的第 {seq} 条损坏，已跳过: {error}"),
        }
    }
    Ok(entries)
}

/// 该分支上已提交的轮数（数带 `Turn:` trailer 的提交）。
///
/// 重开一个会话时用它恢复轮号计数，保证 `git log --grep='Turn: n'` 的轮号
/// 在会话的整个生命周期里唯一。
pub fn count_turns(exec: &GitExec, ref_name: &str) -> Result<u64, String> {
    let output = exec.run(&["log", "--format=%B%x00", ref_name])?;
    let text = String::from_utf8_lossy(&output);
    let count = text
        .split('\0')
        .filter(|message| {
            message
                .lines()
                .any(|line| line.trim_start().starts_with("Turn: "))
        })
        .count();
    Ok(count as u64)
}

/// 某个前缀下全部会话的摘要（活跃区 `session`、归档区 `archive`）。
///
/// 每分支两次 git 调用：`ls-tree` 拿路径（条数、消息条数、要读的 blob），
/// 一次 `cat-file --batch` 取标题/模型/最后活跃所需的少量 blob —— 不整读会话。
pub fn read_summaries(exec: &GitExec, prefix: &str) -> Result<Vec<SessionSummary>, String> {
    let mut out = Vec::new();
    for id in refs::list_ids(exec, prefix)? {
        let ref_name = format!("refs/heads/{prefix}/{id}");
        let paths = tree_paths(exec, &ref_name)?;
        let message_count = paths.iter().filter(|path| path.kind == "message").count();
        let started_at = id
            .split('-')
            .next()
            .and_then(|millis| millis.parse::<u64>().ok())
            .unwrap_or(0);

        let mut selected: Vec<TreePath> = Vec::new();
        let mut seen = BTreeSet::new();
        let mut messages = paths
            .iter()
            .filter(|path| path.kind == "message")
            .cloned()
            .collect::<Vec<_>>();
        messages.sort_by_key(|path| path.seq);
        // 标题取最早的几条消息；模型回退与最后活跃取最新的几条
        for path in messages.iter().take(8).chain(messages.iter().rev().take(3)) {
            select(path, &mut selected, &mut seen);
        }
        if let Some(path) = paths
            .iter()
            .filter(|path| path.kind == "model_change")
            .max_by_key(|path| path.seq)
        {
            select(path, &mut selected, &mut seen);
        }
        if let Some(path) = paths.iter().max_by_key(|path| path.seq) {
            select(path, &mut selected, &mut seen);
        }

        let mut input = String::new();
        for path in &selected {
            input.push_str(&path.sha);
            input.push('\n');
        }
        let output = exec.run_with_stdin(&["cat-file", "--batch"], input.into_bytes())?;
        let blobs = parse_batch(&output);
        let entries = blobs
            .iter()
            .map(|blob| {
                blob.as_ref()
                    .and_then(|bytes| serde_json::from_slice::<SessionEntry>(bytes).ok())
            })
            .collect::<Vec<_>>();
        let pairs = selected
            .iter()
            .zip(entries.iter())
            .filter_map(|(path, entry)| entry.as_ref().map(|entry| (path, entry)))
            .collect::<Vec<_>>();

        let mut message_pairs = pairs
            .iter()
            .filter(|(path, _)| path.kind == "message")
            .copied()
            .collect::<Vec<_>>();
        message_pairs.sort_by_key(|(path, _)| path.seq);
        let title = crate::title_from_entries(message_pairs.iter().map(|(_, entry)| *entry));

        let model = pairs
            .iter()
            .filter(|(path, _)| path.kind == "model_change")
            .max_by_key(|(path, _)| path.seq)
            .and_then(|(_, entry)| match &entry.kind {
                EntryKind::ModelChange { provider } => Some(provider.display_name().to_string()),
                _ => None,
            })
            .or_else(|| {
                // 回退：最近一条带模型名的 assistant 消息（与 JSONL 列表口径一致）
                crate::latest_assistant_model(message_pairs.iter().map(|(_, entry)| *entry))
            });

        let last_active = pairs
            .iter()
            .max_by_key(|(path, _)| path.seq)
            .map(|(_, entry)| entry.timestamp)
            .unwrap_or(0);

        out.push(SessionSummary {
            id,
            title,
            message_count,
            started_at,
            last_active,
            model,
        });
    }
    out.sort_by_key(|summary| std::cmp::Reverse(summary.last_active));
    Ok(out)
}

fn select(path: &TreePath, selected: &mut Vec<TreePath>, seen: &mut BTreeSet<u64>) {
    if seen.insert(path.seq) {
        selected.push(path.clone());
    }
}

/// `git ls-tree -r -z` 的输出 → 条目文件清单。
fn tree_paths(exec: &GitExec, ref_name: &str) -> Result<Vec<TreePath>, String> {
    let output = exec.run(&["ls-tree", "-r", "-z", "--full-tree", ref_name])?;
    let mut paths = Vec::new();
    for record in output.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let record = String::from_utf8_lossy(record);
        let Some((meta, path)) = record.split_once('\t') else {
            continue;
        };
        let mut parts = meta.split(' ');
        let _mode = parts.next();
        let _object_type = parts.next();
        let Some(sha) = parts.next() else {
            continue;
        };
        let Some((seq, kind)) = layout::parse_entry_path(path) else {
            continue;
        };
        paths.push(TreePath {
            seq,
            kind,
            sha: sha.to_string(),
        });
    }
    Ok(paths)
}

/// 解析 `git cat-file --batch` 的输出流（`<sha> <type> <size>\n<content>\n`）。
///
/// 返回与请求顺序对齐的内容列表；对象缺失（`<name> missing`）或不是 blob 时
/// 该项为 `None`。
fn parse_batch(output: &[u8]) -> Vec<Option<Vec<u8>>> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < output.len() {
        let Some(offset) = output[cursor..].iter().position(|byte| *byte == b'\n') else {
            break;
        };
        let header_end = cursor + offset;
        let header = String::from_utf8_lossy(&output[cursor..header_end]).to_string();
        cursor = header_end + 1;
        let mut parts = header.split(' ');
        let _name = parts.next();
        let object_type = parts.next();
        let size = parts.next().and_then(|value| value.parse::<usize>().ok());
        match (object_type, size) {
            (Some("blob"), Some(size)) if cursor + size <= output.len() => {
                out.push(Some(output[cursor..cursor + size].to_vec()));
                cursor += size;
                if output.get(cursor) == Some(&b'\n') {
                    cursor += 1;
                }
            }
            _ => out.push(None),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_batch_reads_contents_and_marks_missing() {
        let stream = b"aaaa blob 2\n{}\nbbbb missing\ncccc blob 5\n[1,2]\n";
        let parsed = parse_batch(stream);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].as_deref(), Some(&b"{}"[..]));
        assert_eq!(parsed[1], None);
        assert_eq!(parsed[2].as_deref(), Some(&b"[1,2]"[..]));
        // 截断的流：不 panic，读不出来就标 None
        let truncated = parse_batch(b"aaaa blob 9\n{}");
        assert_eq!(truncated.len(), 1);
        assert_eq!(truncated[0], None);
    }
}
