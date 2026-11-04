//! 压缩编排与持久化；算法、预算和验证仍由 context 拥有。
use super::*;

/// 把「保留区间下标」换算成条目 id（compaction 条目的 `keep_from_entry`）。
///
/// 内存历史与落盘条目必须一一对应（见 `session::replay` 的 id 语义）。两边
/// 长度不一致说明二者脱节 —— 此时退回空 id：宁可只留摘要，也不让回放去猜
/// 一个错误的位置（猜错会让保留段整体错位）。
fn keep_from_entry_id(writer: &SharedLog, history: &[Message], keep_from: Option<usize>) -> String {
    let Some(index) = keep_from else {
        return String::new();
    };
    let Ok(writer) = writer.lock() else {
        return String::new();
    };
    let ids = writer.message_ids();
    if ids.len() != history.len() {
        eprintln!(
            "pipi: 压缩时内存历史（{} 条）与落盘条目（{} 条）不一致，本轮只保留摘要",
            history.len(),
            ids.len()
        );
        return String::new();
    }
    ids.get(index).cloned().unwrap_or_default()
}

/// 编辑式压缩必须精确换算每一个下标：长度不一致时 fail-closed，不猜位置。
fn entry_ids_for_edit(writer: &SharedLog, history: &[Message]) -> Result<Vec<String>, String> {
    let writer = writer
        .lock()
        .map_err(|error| format!("Cannot lock session writer: {error}"))?;
    let ids = writer.message_ids();
    if ids.len() != history.len() {
        return Err(format!(
            "Cannot compact: in-memory history ({}) and persisted entries ({}) disagree",
            history.len(),
            ids.len()
        ));
    }
    Ok(ids.to_vec())
}

/// 一条替换式压缩的落盘内容：摘要或 Agent 编辑。
enum CompactionEntry<'a> {
    Summary(CompactionRecord<'a>),
    Edit(ContextEditRecord<'a>),
}

impl CompactionEntry<'_> {
    fn append(&self, log: &mut dyn SessionLog) -> std::io::Result<String> {
        match self {
            Self::Summary(record) => log.append_compaction(record),
            Self::Edit(record) => log.append_context_edit(record),
        }
    }

    /// fork 溯源标记（回放中性，见 `EntryKind::Custom`）。
    fn fork_marker(&self, source_id: &str) -> String {
        match self {
            Self::Summary(_) => format!("compaction-fork:{source_id}"),
            Self::Edit(_) => format!("context-edit-fork:{source_id}"),
        }
    }

    fn noun(&self) -> &'static str {
        match self {
            Self::Summary(_) => "compaction entry",
            Self::Edit(_) => "context edit entry",
        }
    }
}

pub(super) async fn run_compaction(
    session: &SessionHandle,
    id: &mut String,
    run_id: usize,
    request: &CompactionRequest,
    hooks: &SessionHooks,
    manual: bool,
) -> Result<bool, String> {
    if session.abort.is_aborted() {
        return Ok(false);
    }
    // 压缩（尤其 fork）只能从已提交的 tip 出发。自动路径刚在轮末提交过；
    // 手动路径可能还留着上一轮提交失败的缓冲 —— 先统一提交一次，失败就
    // 拒绝压缩，避免分叉丢缓冲或回放出 live 从未应用过的历史。
    if let Err(error) = super::commit::commit_log(session).await {
        return Err(format!(
            "Cannot compact with uncommitted session entries: {error}"
        ));
    }
    let history = session
        .messages
        .lock()
        .expect("session history poisoned")
        .clone();
    if !manual && !pipi_context::compaction::needs_compaction(&history, request.budget) {
        return Ok(false);
    }
    hooks.agent(id, run_id, AgentEvent::CompactionStart);
    let result: Result<bool, String> = async {
        let tokens_before = pipi_context::context::context_tokens(&history);
        let mut options = request.options.clone();
        options.session_id = Some(id.clone());
        options.max_tokens = None;
        options.timeout_secs = 300;
        let env = pipi_context::compaction::StrategyEnv {
            provider: request.provider.as_ref(),
            model: &request.model,
            options: &options,
            abort: session.abort.clone(),
            retry: request.retry,
        };
        let compacted =
            pipi_context::compaction::compact(&history, request.budget, &env, request.mode).await?;
        let source_tip = session
            .writer
            .lock()
            .ok()
            .and_then(|w| w.tip_id().map(str::to_string))
            .unwrap_or_default();
        // 事件文案：摘要用摘要正文，编辑用模型 note（策略保证非空）。
        let (summary, replaced, switched) = if let Some(ops) = compacted.edit.as_ref() {
            let ids = entry_ids_for_edit(&session.writer, &history)?;
            let cleared = ops
                .cleared
                .iter()
                .map(|op| {
                    ids.get(op.index).map(|entry_id| ClearedToolResult {
                        entry_id: entry_id.clone(),
                        text: op.text.clone(),
                    })
                })
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| "Cannot compact: cleared index is out of range".to_string())?;
            let removed = ops
                .removed
                .iter()
                .map(|index| ids.get(*index).cloned())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| "Cannot compact: removed index is out of range".to_string())?;
            let thinking_removed = ops
                .thinking_removed
                .iter()
                .map(|index| ids.get(*index).cloned())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| "Cannot compact: thinking index is out of range".to_string())?;
            let record = ContextEditRecord {
                cleared: &cleared,
                removed: &removed,
                thinking_removed: &thinking_removed,
                note: &ops.note,
                strategy: compacted.strategy,
                source_tip: &source_tip,
                usage: compacted.usage,
            };
            let switched = {
                let entry = CompactionEntry::Edit(record);
                persist_compaction(
                    &session.writer,
                    session.repo.as_deref(),
                    &entry,
                    request.policy,
                )?
            };
            let count = ops.removed.len() + ops.cleared.len() + ops.thinking_removed.len();
            (ops.note.clone(), count, switched)
        } else {
            let summary = compacted
                .messages
                .first()
                .and_then(pipi_protocol::session::summary_text)
                .unwrap_or_default()
                .to_string();
            let keep_from_entry =
                keep_from_entry_id(&session.writer, &history, compacted.keep_from);
            let record = CompactionRecord {
                summary: &summary,
                strategy: compacted.strategy,
                keep_from_entry: &keep_from_entry,
                source_tip: &source_tip,
                usage: compacted.usage,
            };
            let switched = {
                let entry = CompactionEntry::Summary(record);
                persist_compaction(
                    &session.writer,
                    session.repo.as_deref(),
                    &entry,
                    request.policy,
                )?
            };
            (summary, compacted.replaced, switched)
        };
        *session.messages.lock().expect("session history poisoned") = compacted.messages;
        if let Some(usage) = compacted.usage
            && let Ok(mut tracker) = session.stats.lock()
        {
            tracker.record_ledger(&usage);
        }
        // 压缩后没有新的助手 usage，锚点失效，这里是估算值（与下一轮真实用量衔接）。
        let tokens_after = pipi_context::context::context_tokens(
            &session.messages.lock().expect("session history poisoned"),
        );
        if let Some(new_id) = switched {
            if let Some(rekey) = &hooks.rekey {
                rekey(id, &new_id);
            }
            (hooks.emit)(SessionEvent {
                session_id: id.clone(),
                run_id,
                kind: SessionEventKind::Switched {
                    to_session_id: new_id.clone(),
                    archived: request.policy.archive_original,
                },
            });
            *id = new_id;
        }
        hooks.agent(
            id,
            run_id,
            AgentEvent::CompactionEnd {
                summary,
                replaced: replaced as u64,
                strategy: compacted.strategy.to_string(),
                tokens_before,
                tokens_after,
            },
        );
        Ok(true)
    }
    .await;
    if let Err(error) = &result {
        hooks.agent(
            id,
            run_id,
            AgentEvent::CompactionFailed {
                message: error.clone(),
            },
        );
    }
    result
}

/// 把压缩/编辑结果落盘，返回 `Some(new_session_id)` 表示已换到新会话。
///
/// 两条路径：
/// - **分叉**（`fork_before_compact`）：新会话 = 原会话活跃路径的完整拷贝 +
///   压缩/编辑条目；随后把账本换过去（旧载体在这时关闭），最后按设置归档原会话。
/// - **原地**（关闭分叉，或分叉失败时的回退）：在当前会话上追加条目。
///
/// 分叉失败不是致命错误（回退原地）；原地追加失败才是 `Err`，此时调用方不得
/// 替换内存历史 —— live 与落盘必须一致。
fn persist_compaction(
    writer: &SharedLog,
    repo: Option<&dyn SessionRepo>,
    entry: &CompactionEntry<'_>,
    settings: CompactionPolicy,
) -> Result<Option<String>, String> {
    // 临时测试上下文需要替换历史，但绝不能分叉或写入 sessions 目录。内存账本
    // 仍追加条目，以保持 message id 映射与正式会话一致。
    {
        let mut guard = writer
            .lock()
            .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
        if guard.is_temporary() {
            entry
                .append(&mut **guard)
                .map_err(|error| format!("无法更新临时压缩账本: {error}"))?;
            return Ok(None);
        }
    }

    let mut fork_error: Option<String> = None;
    if settings.fork_before_compact {
        let outcome = match repo {
            Some(repo) => fork_and_write_compaction(writer, repo, entry, settings.archive_original),
            None => Err("Cannot resolve session repository".to_string()),
        };
        match outcome {
            Ok(new_id) => return Ok(Some(new_id)),
            Err(error) => fork_error = Some(error),
        }
    }
    let mut guard = writer
        .lock()
        .map_err(|error| format!("Cannot lock session writer: {error}"))?;
    entry
        .append(&mut **guard)
        .map_err(|error| format!("Cannot write {}: {error}", entry.noun()))?;
    // 提交点（原地压缩）：自动路径之后没有别的提交机会。失败必须回滚刚追加的
    // 条目 —— 否则 live 内存仍是未压缩历史，而下一次成功提交会把条目
    // 一起写进去，重启后的回放与 live 分叉。
    if let Err(error) = guard.commit_point() {
        let rolled_back = guard.rollback_last_entry().unwrap_or(false);
        return Err(if rolled_back {
            format!(
                "Cannot commit {}: {error} (entry rolled back)",
                entry.noun()
            )
        } else {
            format!(
                "Cannot commit {}: {error} (entry remains buffered)",
                entry.noun()
            )
        });
    }
    if let Some(error) = fork_error {
        eprintln!("pipi: compaction fork failed, fell back to in-place: {error}");
    }
    Ok(None)
}

/// 分叉出新会话并把条目写进去，然后归档原会话（可关）。
///
/// 顺序很关键：**先把新会话完整落盘，再换账本**（换的那一刻旧账本被丢弃），
/// 最后才归档原会话 —— 顺序反了的话，旧账本缓冲里的条目会在归档之后提交到
/// `session/<旧 id>`，把刚归档的分支重新建回活跃区。
fn fork_and_write_compaction(
    writer: &SharedLog,
    repo: &dyn SessionRepo,
    entry: &CompactionEntry<'_>,
    archive_original: bool,
) -> Result<String, String> {
    let source_id = {
        let guard = writer
            .lock()
            .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
        guard.session_id().to_string()
    };

    let mut forked = repo.fork(&source_id, None)?;
    let new_id = forked.session_id().to_string();
    if let Err(error) = entry.append(forked.as_mut()) {
        // 写失败就删掉半成品，别在会话列表里留下空壳
        drop(forked);
        let _ = repo.delete(&new_id);
        return Err(format!(
            "Failed to write {} to new session: {error}",
            entry.noun()
        ));
    }
    // 溯源标记：回答「这个会话从哪来」（回放中性，见 EntryKind::Custom）
    let _ = forked.append_custom(&entry.fork_marker(&source_id));
    // 提交点（压缩分叉）：新分支的第一个提交必须在换账本之前落定 —— 换掉之后
    // 旧句柄被 drop，新载体只经会话可达。
    if let Err(error) = forked.commit_point() {
        drop(forked);
        let _ = repo.delete(&new_id);
        return Err(format!(
            "Cannot commit {} of new session: {error}",
            entry.noun()
        ));
    }

    {
        let mut guard = writer
            .lock()
            .map_err(|error| format!("Cannot lock session writer: {error}"))?;
        let previous = std::mem::replace(&mut *guard, forked);
        drop(previous); // 关闭旧载体句柄 —— 归档前必须做到
    }

    if archive_original && let Err(error) = repo.archive(&source_id) {
        // 归档失败不回滚：新会话已经可用，原会话留在活跃列表里即可
        eprintln!("pipi: failed to archive original session (remains in active list): {error}");
    }
    Ok(new_id)
}
