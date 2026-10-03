//! 自动与手动压缩、writer 替换和会话重键。

use super::*;

/// 把「保留区间下标」换算成条目 id（compaction 条目的 `keep_from_entry`）。
///
/// 内存历史与落盘条目必须一一对应（见 `session::replay` 的 id 语义）。两边
/// 长度不一致说明二者脱节 —— 此时退回空 id：宁可只留摘要，也不让回放去猜
/// 一个错误的位置（猜错会让保留段整体错位）。
fn keep_from_entry_id(
    writer: &Arc<Mutex<SessionWriter>>,
    history: &[Message],
    keep_from: Option<usize>,
) -> String {
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

/// 压缩的触发方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompactionTrigger {
    /// turn 边界：历史达到触发线才压。
    Auto,
    /// 用户手动点：不管阈值都压（历史太短时返回可读错误）。
    Manual,
}

/// 跑一次压缩所需的全部素材 —— 自动（turn 边界）与手动（UI 按钮）共用，
/// 保证「摘要 → 分叉/归档 → 换会话 → 落事件」只有一条实现。
pub(super) struct CompactionContext<'a> {
    pub(super) agent_name: &'a str,
    /// 换会话后就地改成新 id，后续事件才会带新身份。
    pub(super) session_id: &'a mut String,
    pub(super) run_id: usize,
    pub(super) model: &'a Model,
    pub(super) api_key: &'a str,
    pub(super) budget: pipi_core::compaction::Budget,
    pub(super) messages: &'a Arc<tokio::sync::Mutex<Vec<Message>>>,
    pub(super) writer: &'a Arc<Mutex<SessionWriter>>,
    pub(super) stats: &'a Arc<Mutex<SessionStatsTracker>>,
    pub(super) abort: AbortSignal,
    pub(super) sessions_dir: Option<&'a std::path::Path>,
    pub(super) settings: pipi_core::settings::CompactionSettings,
    /// 摘要调用失败重发策略（与对话共用，见 `retry` 模块）。
    pub(super) retry: pipi_core::retry::RetryPolicy,
    pub(super) sink: &'a EventEmitter,
    pub(super) trigger: CompactionTrigger,
    /// 压缩分叉换了会话 id 后，把 map 里的条目搬到新键（None = 不需要，如子 Agent）。
    pub(super) rekey: Option<SessionRekey>,
}

/// 构造「把条目从旧键搬到新键」的回调（同一把锁、同一个 Session 对象）。
pub(super) fn make_rekey(
    sessions: Arc<Mutex<HashMap<SessionKey, Session>>>,
    background: BackgroundTaskManager,
) -> SessionRekey {
    Arc::new(move |old: &SessionKey, new_session_id: &str| {
        background.rekey_session(&old.agent_name, &old.session_id, new_session_id);
        let Ok(mut map) = sessions.lock() else {
            return;
        };
        if let Some(session) = map.remove(old) {
            map.insert(SessionKey::new(&old.agent_name, new_session_id), session);
        }
    })
}

pub(super) fn compaction_stream_options(api_key: &str, session_id: &str) -> StreamOptions {
    StreamOptions {
        api_key: Some(api_key.to_string()),
        temperature: None,
        max_tokens: None,
        timeout_secs: 300,
        // 摘要虽是一次性提示词，也必须属于当前会话。OpenCode Go 等供应商
        // 依赖这个 ID 注入 x-opencode-session，并据此路由请求。
        session_id: Some(session_id.to_string()),
    }
}

/// 压缩本体。`Ok(false)` 表示按触发方式判断「无需压缩」（自动且未达触发线）——
/// 调用方不应把它当失败；`Err` 才是真的没压成（手动时的「历史还不用压」也走这里）。
pub(super) async fn run_compaction(ctx: CompactionContext<'_>) -> Result<bool, String> {
    let history = ctx.messages.lock().await.clone();
    if ctx.trigger == CompactionTrigger::Auto
        && !pipi_core::compaction::needs_compaction(&history, ctx.budget)
    {
        return Ok(false);
    }
    // session_id 显式传入：换会话后就地改写它，之后的 envelope 才会带新身份
    let agent_name = ctx.agent_name.to_string();
    let run_id = ctx.run_id;
    let envelope = |session_id: &str, event: AgentEvent| {
        RuntimeEvent::AgentEvent(AgentEventEnvelope {
            agent_name: agent_name.clone(),
            session_id: session_id.to_string(),
            run_id,
            event,
        })
    };
    (ctx.sink)(envelope(ctx.session_id, AgentEvent::CompactionStart));
    let tokens_before = pipi_core::context::estimate_context_tokens(&history);

    // 摘要用量另计入会话账本；请求仍携带当前会话 ID，满足供应商的会话路由要求。
    let summary_options = compaction_stream_options(ctx.api_key, ctx.session_id);
    // provider 实例要活到 compact 调用结束（provider_for 返回 Arc）
    let summarizer = provider_for(ctx.model.api);
    let strategy_env = pipi_core::compaction::StrategyEnv {
        provider: summarizer.as_ref(),
        model: ctx.model,
        options: &summary_options,
        abort: ctx.abort.clone(),
        retry: ctx.retry,
    };
    let compacted = pipi_core::compaction::compact(&history, ctx.budget, &strategy_env).await?;

    // 落盘的摘要正文是**未包裹**的原文（回放时统一包裹，见 session::summary_message）。
    let summary = compacted
        .messages
        .first()
        .and_then(pipi_core::session::summary_text)
        .unwrap_or_default()
        .to_string();
    // 保留区间的起点条目 id：回放据此留住这段原文（缺了它就退回旧行为）。
    let keep_from_entry = keep_from_entry_id(ctx.writer, &history, compacted.keep_from);
    let source_tip = ctx
        .writer
        .lock()
        .ok()
        .and_then(|writer| writer.tip_id().map(str::to_string))
        .unwrap_or_default();
    let record = pipi_core::session::CompactionRecord {
        summary: &summary,
        strategy: compacted.strategy,
        keep_from_entry: &keep_from_entry,
        source_tip: &source_tip,
        usage: compacted.usage,
    };
    // 落盘（分叉 / 原地由设置决定）。失败则内存历史不动 —— live 与落盘必须
    // 一致，否则重开会话会退回未压缩。
    let switched = persist_compaction(ctx.writer, ctx.sessions_dir, &record, ctx.settings)?;
    *ctx.messages.lock().await = compacted.messages;
    if let Some(usage) = compacted.usage
        && let Ok(mut tracker) = ctx.stats.lock()
    {
        tracker.record_ledger(&usage);
    }
    let tokens_after = pipi_core::context::estimate_context_tokens(&ctx.messages.lock().await);
    // 换会话：先发 SessionSwitched（envelope 用旧 id，前端此刻身份还是旧的），
    // 把身份切到新 id，再发后续事件 —— 顺序反了会导致前端收不到切换、running 卡住。
    if let Some(new_id) = &switched {
        // 先把 map 里的条目搬到新键（同一条会话、新 id），再让前端与后续事件跟上 ——
        // 否则键会指向旧 id：归档/删除的占用检查与 session_infos 会各说各话。
        if let Some(rekey) = &ctx.rekey {
            rekey(&SessionKey::new(ctx.agent_name, ctx.session_id), new_id);
        }
        (ctx.sink)(RuntimeEvent::SessionSwitched(SessionSwitchedEnvelope {
            agent_name: ctx.agent_name.to_string(),
            session_id: ctx.session_id.clone(),
            run_id: ctx.run_id,
            to_session_id: new_id.clone(),
            archived: ctx.settings.archive_original,
        }));
        *ctx.session_id = new_id.clone();
    }
    (ctx.sink)(envelope(
        ctx.session_id,
        AgentEvent::CompactionEnd {
            summary,
            replaced: compacted.replaced as u64,
            strategy: compacted.strategy.to_string(),
            tokens_before,
            tokens_after,
        },
    ));
    Ok(true)
}

/// 把压缩结果落盘，返回 `Some(new_session_id)` 表示已换到新会话。
///
/// 两条路径：
/// - **分叉**（`fork_before_compact`）：新会话文件 = 原文件活跃路径的完整拷贝 +
///   压缩条目；随后把 writer 换过去（旧文件在这时关闭），最后按设置归档原文件。
/// - **原地**（关闭分叉，或分叉失败时的回退）：在当前文件上追加压缩条目。
///
/// 分叉失败不是致命错误（回退原地）；原地追加失败才是 `Err`，此时调用方不得
/// 替换内存历史 —— live 与落盘必须一致。
pub(super) fn persist_compaction(
    writer: &Arc<Mutex<SessionWriter>>,
    sessions_dir: Option<&std::path::Path>,
    record: &pipi_core::session::CompactionRecord<'_>,
    settings: pipi_core::settings::CompactionSettings,
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
        match fork_and_write_compaction(writer, sessions_dir, record, settings.archive_original) {
            Ok(new_id) => return Ok(Some(new_id)),
            Err(error) => fork_error = Some(error),
        }
    }
    let mut guard = writer
        .lock()
        .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
    guard
        .append_compaction(record)
        .map_err(|error| format!("无法写入压缩条目: {error}"))?;
    if let Some(error) = fork_error {
        eprintln!("pipi: 压缩分叉失败，已改为原地压缩: {error}");
    }
    Ok(None)
}

/// 分叉出新会话并把压缩条目写进去，然后归档原会话（可关）。
///
/// 顺序很关键：**先在内存外写好新文件，再换 writer**（换的那一刻旧文件句柄关闭），
/// 最后才 rename 归档 —— 否则 Linux 上仍打开的 fd 会继续往被移动的文件追加。
fn fork_and_write_compaction(
    writer: &Arc<Mutex<SessionWriter>>,
    sessions_dir: Option<&std::path::Path>,
    record: &pipi_core::session::CompactionRecord<'_>,
    archive_original: bool,
) -> Result<String, String> {
    let sessions_dir = sessions_dir.ok_or_else(|| "无法解析会话目录".to_string())?;
    let (source_path, source_id) = {
        let guard = writer
            .lock()
            .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
        let path = guard
            .persistent_path()
            .ok_or_else(|| "临时测试会话不能分叉".to_string())?
            .to_path_buf();
        let id = guard.session_id().to_string();
        (path, id)
    };

    let mut forked = pipi_core::session::fork_session(&source_path, sessions_dir, None)?;
    let new_id = forked
        .path()
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| "无法解析新会话 ID".to_string())?
        .to_string();
    if let Err(error) = forked.append_compaction(record) {
        // 写失败就删掉半成品，别在会话列表里留下空壳
        let path = forked.path().to_path_buf();
        drop(forked);
        let _ = std::fs::remove_file(&path);
        return Err(format!("无法把压缩条目写入新会话: {error}"));
    }
    // 溯源标记：回答「这个会话从哪来」（回放中性，见 EntryKind::Custom）
    let _ = forked.append_custom(&format!("compaction-fork:{source_id}"));

    {
        let mut guard = writer
            .lock()
            .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
        let previous = std::mem::replace(&mut *guard, forked);
        drop(previous); // 关闭原文件句柄 —— 归档前必须做到
    }

    if archive_original
        && let Err(error) = pipi_core::session::archive_session_file(sessions_dir, &source_id)
    {
        // 归档失败不回滚：新会话已经可用，原会话留在活跃列表里即可
        eprintln!("pipi: 原会话归档失败（保留在活跃列表）: {error}");
    }
    Ok(new_id)
}

impl RuntimeState {
    /// 手动压缩某条会话（UI「立即压缩」）：跳过阈值预检，其余与自动压缩完全同一条
    /// 路径（摘要 → 分叉/归档 → 换会话）。
    ///
    /// 工作交给注入的运行时异步执行，结果通过事件回报 —— 前端据
    /// `compaction_start/end`、`session-switched`、`session-error` 更新界面。
    /// 这里会占住 running 位：与正在跑的一轮互斥，且 `stop_run` 能中止摘要调用。
    pub fn compact_now(
        &self,
        agent_name: &str,
        session_id: &str,
        event_sink: EventEmitter,
    ) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let key = SessionKey::new(agent_name, session_id);
        let Some(session) = sessions.get(&key) else {
            return Err(format!("会话「{session_id}」没有打开"));
        };
        if session.running.is_running() {
            return Err("会话正在运行，请先停止再压缩".into());
        }
        let def = session.agent.clone();
        let current_model = session.model.lock().ok().and_then(|model| model.clone());
        // 与发送消息同一套解析：会话模型优先，回退 Agent 默认
        let (model, api_key) = resolve_model(&def, current_model.as_ref(), None)?;
        let budget = pipi_core::compaction::Budget::from_window(
            model.context_window,
            def.compact_threshold_percent(),
        )
        .with_target_percent(def.compact_target_percent());
        let sessions_dir = def.sessions_dir();
        let runtime_settings = load_settings();
        let settings = runtime_settings.compaction;
        let retry = runtime_settings.retry.policy();
        let messages = session.messages.clone();
        let writer = session.writer.clone();
        let stats = session.stats.clone();
        let abort = session.abort.clone();
        let running = session.running.clone();
        abort.reset();
        // 锁内占位：避免释放锁后与新一轮 send_prompt 抢跑
        let running_guard = RunningGuard::new(running.clone());
        let run_token = running_guard.token;
        let run_id = running.current_run_id();
        let sink = event_sink.clone();
        let sessions_map = self.sessions.clone();
        let rekey = make_rekey(sessions_map, self.background.clone());

        self.spawn_run(async move {
            let _running_guard = running_guard;
            let mut session_id = writer_session_id(&writer).unwrap_or_default();
            // 发一对 Agent 事件：前端在 compaction_start 会把 running 置 true，
            // 而只有 agent_end 会清 —— 手动压缩没有 agent loop，缺了它界面会
            // 永久停在「运行中」（停止按钮、模型切换、新会话全被禁）。
            sink(RuntimeEvent::AgentEvent(AgentEventEnvelope {
                agent_name: def.name.clone(),
                session_id: session_id.clone(),
                run_id,
                event: AgentEvent::AgentStart,
            }));
            let outcome = run_compaction(CompactionContext {
                agent_name: &def.name,
                session_id: &mut session_id,
                run_id,
                model: &model,
                api_key: &api_key,
                budget,
                messages: &messages,
                writer: &writer,
                stats: &stats,
                abort: abort.clone(),
                sessions_dir: sessions_dir.as_deref(),
                settings,
                retry,
                sink: &sink,
                trigger: CompactionTrigger::Manual,
                rekey: Some(rekey),
            })
            .await;
            if let Err(error) = outcome {
                // 手动路径要让人看到原因（例如「历史还不用压」），走会话错误通道
                sink(RuntimeEvent::SessionError(SessionErrorEnvelope {
                    agent_name: def.name.clone(),
                    session_id: session_id.clone(),
                    run_id,
                    message: format!("压缩未执行：{error}"),
                }));
            }
            running.finish(run_token);
            sink(RuntimeEvent::AgentEvent(AgentEventEnvelope {
                agent_name: def.name.clone(),
                session_id,
                run_id,
                event: AgentEvent::AgentEnd {
                    messages: Vec::new(),
                },
            }));
        });
        Ok(())
    }
}
