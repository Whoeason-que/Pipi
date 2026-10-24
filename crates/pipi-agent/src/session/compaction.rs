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
    let history = session
        .messages
        .lock()
        .expect("session history poisoned")
        .clone();
    if !manual && !pipi_context::compaction::needs_compaction(&history, request.budget) {
        return Ok(false);
    }
    hooks.agent(id, run_id, AgentEvent::CompactionStart);
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
    let compacted = pipi_context::compaction::compact(&history, request.budget, &env).await?;
    let summary = compacted
        .messages
        .first()
        .and_then(pipi_protocol::session::summary_text)
        .unwrap_or_default()
        .to_string();
    let keep_from_entry = keep_from_entry_id(&session.writer, &history, compacted.keep_from);
    let source_tip = session
        .writer
        .lock()
        .ok()
        .and_then(|w| w.tip_id().map(str::to_string))
        .unwrap_or_default();
    let record = CompactionRecord {
        summary: &summary,
        strategy: compacted.strategy,
        keep_from_entry: &keep_from_entry,
        source_tip: &source_tip,
        usage: compacted.usage,
    };
    let switched = persist_compaction(
        &session.writer,
        session.repo.as_deref(),
        &record,
        request.policy,
    )?;
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
            replaced: compacted.replaced as u64,
            strategy: compacted.strategy.to_string(),
            tokens_before,
            tokens_after,
        },
    );
    Ok(true)
}

/// 把压缩结果落盘，返回 `Some(new_session_id)` 表示已换到新会话。
///
/// 两条路径：
/// - **分叉**（`fork_before_compact`）：新会话 = 原会话活跃路径的完整拷贝 +
///   压缩条目；随后把账本换过去（旧载体在这时关闭），最后按设置归档原会话。
/// - **原地**（关闭分叉，或分叉失败时的回退）：在当前会话上追加压缩条目。
///
/// 分叉失败不是致命错误（回退原地）；原地追加失败才是 `Err`，此时调用方不得
/// 替换内存历史 —— live 与落盘必须一致。
pub(super) fn persist_compaction(
    writer: &SharedLog,
    repo: Option<&dyn SessionRepo>,
    record: &pipi_protocol::session::CompactionRecord<'_>,
    settings: CompactionPolicy,
) -> Result<Option<String>, String> {
    // 临时测试上下文需要摘要替换，但绝不能分叉或写入 sessions 目录。内存账本
    // 仍追加 compaction entry，以保持 message id 映射与正式会话一致。
    {
        let mut guard = writer
            .lock()
            .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
        if guard.is_temporary() {
            guard
                .append_compaction(record)
                .map_err(|error| format!("无法更新临时压缩账本: {error}"))?;
            return Ok(None);
        }
    }

    let mut fork_error: Option<String> = None;
    if settings.fork_before_compact {
        let outcome = match repo {
            Some(repo) => {
                fork_and_write_compaction(writer, repo, record, settings.archive_original)
            }
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
    guard
        .append_compaction(record)
        .map_err(|error| format!("Cannot write compaction entry: {error}"))?;
    // 提交点（原地压缩）：自动路径之后没有别的提交机会
    guard
        .commit_point()
        .map_err(|error| format!("Cannot commit compaction entry: {error}"))?;
    if let Some(error) = fork_error {
        eprintln!("pipi: compaction fork failed, fell back to in-place: {error}");
    }
    Ok(None)
}

/// 分叉出新会话并把压缩条目写进去，然后归档原会话（可关）。
///
/// 顺序很关键：**先把新会话完整落盘，再换账本**（换的那一刻旧账本被丢弃），
/// 最后才归档原会话 —— 顺序反了的话，旧账本缓冲里的条目会在归档之后提交到
/// `session/<旧 id>`，把刚归档的分支重新建回活跃区。
fn fork_and_write_compaction(
    writer: &SharedLog,
    repo: &dyn SessionRepo,
    record: &pipi_protocol::session::CompactionRecord<'_>,
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
    if let Err(error) = forked.append_compaction(record) {
        // 写失败就删掉半成品，别在会话列表里留下空壳
        drop(forked);
        let _ = repo.delete(&new_id);
        return Err(format!(
            "Failed to write compaction entry to new session: {error}"
        ));
    }
    // 溯源标记：回答「这个会话从哪来」（回放中性，见 EntryKind::Custom）
    let _ = forked.append_custom(&format!("compaction-fork:{source_id}"));
    // 提交点（压缩分叉）：新分支的第一个提交必须在换账本之前落定 —— 换掉之后
    // 旧句柄被 drop，新载体只经会话可达。
    if let Err(error) = forked.commit_point() {
        drop(forked);
        let _ = repo.delete(&new_id);
        return Err(format!(
            "Cannot commit compaction entries of new session: {error}"
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
