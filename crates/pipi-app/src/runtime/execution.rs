//! 主轮与单次 child 执行。child 的工具和审批边界保持独立。

use super::*;

fn append_declared_env(
    writer: &SharedLog,
    resolved_env: &av::resolve::ResolvedEnv,
) -> Result<(), String> {
    let declared = resolved_env
        .provenance
        .iter()
        .filter(|(_, source)| source.as_str() != av::resolve::PROCESS_SOURCE)
        .map(|(key, source)| pipi_protocol::session::EnvDeclared {
            key: key.clone(),
            source: source.clone(),
        })
        .collect();
    writer
        .lock()
        .map_err(|error| format!("无法锁定会话写入器: {error}"))?
        .append_env(declared)
        .map(|_| ())
        .map_err(|error| format!("无法写入环境记账: {error}"))
}

fn run_stream_options(api_key: String, model: &Model, session_id: &str) -> StreamOptions {
    StreamOptions {
        api_key: Some(api_key),
        temperature: None,
        max_tokens: Some(model.max_tokens),
        timeout_secs: 300,
        session_id: Some(session_id.to_string()),
    }
}

pub(super) fn resolve_model(
    def: &AgentDefinition,
    session_model: Option<&Model>,
    env: Option<&std::collections::BTreeMap<String, String>>,
) -> Result<(Model, String), String> {
    let target = session_model
        .cloned()
        .or_else(|| def.provider.clone())
        .ok_or_else(|| {
            "该 Agent 还未配置默认模型，且当前会话未选择模型（请在 Agent 详情页绑定，或在会话顶部选择模型）".to_string()
        })?;
    let settings = load_settings();
    let api_key = settings
        .providers
        .iter()
        .find(|p| p.api == target.api && p.base_url == target.base_url)
        .and_then(|p| match env {
            // 与 bash 子进程消费同一份 resolved env；无契约上下文时回退进程环境
            Some(env) => p.resolve_api_key_in(env),
            None => p.resolve_api_key(),
        })
        .ok_or_else(|| {
            format!(
                "提供商 {} 未配置 API 密钥（设置 → 模型提供商）",
                target.base_url
            )
        })?;
    Ok((
        Model {
            id: target.id.clone(),
            name: target.name.clone(),
            api: target.api,
            base_url: target.base_url.clone(),
            max_tokens: if target.max_tokens == 0 {
                8192
            } else {
                target.max_tokens
            },
            context_window: target.context_window,
        },
        api_key,
    ))
}

pub(super) fn writer_session_id(writer: &SharedLog) -> Result<String, String> {
    let writer = writer
        .lock()
        .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
    Ok(writer.session_id().to_string())
}

/// 载入自愈：会话尾部若留下未被回答的工具调用（上一次运行被进程中断 ——
/// 应用重启、强杀，结果没来得及落盘），追加合成的失败结果条目。
///
/// 只处理**尾部**：悬挂的 assistant 消息此时就是文件 tip，追加的 tool 结果
/// 成为它的子节点，消息顺序天然正确，会话记录被修复成合法状态（append-only，
/// 不重写任何已有条目）。历史中段的悬挂无法原地修复（树的顺序不可变），
/// 由发送前的 [`pipi_agent::context::repair_tool_pairing`] 兜底 —— 对齐 pi 的
/// post-tools / recovery 在回合结束时结算「orphaned / aborted」调用的做法。
pub(super) fn settle_unanswered_tail(
    writer: &mut dyn SessionLog,
    messages: &[Message],
) -> Result<Vec<Message>, String> {
    let Some(last) = messages.last() else {
        return Ok(Vec::new());
    };
    let calls: Vec<(String, String)> = last
        .tool_calls()
        .iter()
        .filter_map(|call| match call {
            pipi_protocol::ContentBlock::ToolCall { id, name, .. } => {
                Some((id.clone(), name.clone()))
            }
            _ => None,
        })
        .collect();
    if calls.is_empty() {
        return Ok(Vec::new());
    }
    let mut appended = Vec::with_capacity(calls.len());
    for (id, name) in calls {
        let result = pipi_agent::context::missing_tool_result(&id, &name);
        writer
            .append_message(&result)
            .map_err(|error| error.to_string())?;
        appended.push(result);
    }
    Ok(appended)
}

/// 在向持久化会话追加终态之前，先结算当前 tip 上尚未回答的工具调用。
///
/// 正常 loop 会自行写入每条 `toolResult`；但 child run 被超时取消时，future 会
/// 在工具执行中被丢弃。此时最后一条已落盘的 assistant 工具调用必须先补齐，
/// 再写 timeout / aborted assistant，才能保持工具配对不变量。
fn settle_unanswered_persistent_tail(
    repo: &dyn SessionRepo,
    session_id: &str,
    writer: &mut dyn SessionLog,
) -> Result<Vec<Message>, String> {
    let entries = repo.entries(session_id)?;
    let messages = rebuild_messages(&active_path(&entries));
    settle_unanswered_tail(writer, &messages)
}

/// child run 到达总时限后的统一收尾。返回的 assistant 终态只会排在合成的
/// `toolResult` 之后，避免 `read_agent` 重开会话时遇到中段的孤立工具调用。
pub(super) fn append_child_timeout_terminal(
    repo: &dyn SessionRepo,
    writer: &SharedLog,
    model: &Model,
    timeout: std::time::Duration,
) -> Result<Message, String> {
    let timeout_error = Message::assistant_error(
        format!("子任务运行超时（{} 秒），已终止", timeout.as_secs()),
        model.display_name(),
        StopReason::Error,
    );
    let mut writer = writer
        .lock()
        .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
    let session_id = writer.session_id().to_string();
    settle_unanswered_persistent_tail(repo, &session_id, &mut **writer)
        .map_err(|error| format!("无法修复超时前的工具调用: {error}"))?;
    writer
        .append_message(&timeout_error)
        .map_err(|error| format!("无法写入超时状态: {error}"))?;
    Ok(timeout_error)
}

/// 投影式压缩流水线的 transform 钩子（组装请求时应用，不落盘）。
fn projection_transform(
    budget: pipi_agent::compaction::Budget,
) -> pipi_agent::agent_loop::TransformContextHook {
    Arc::new(move |messages: Vec<Message>| pipi_agent::compaction::project(messages, budget))
}

/// 不占用 [`RuntimeState`] 当前会话槽的一次性 Agent 执行器。
///
/// `run_agent` 的同步兼容路径在父循环的工具调用内 await 本执行器；异步模式
/// 由 `BackgroundTaskManager::submit_agent` 使用独立的 child 生命周期调用同一
/// 内核。目标 Agent 只注册基础工具，所以不会递归调用其他 Agent。
pub(super) struct RuntimeAgentRunner {
    pub(super) session_sink: Option<BackgroundAgentSessionSink>,
}

#[async_trait::async_trait]
impl AgentRunner for RuntimeAgentRunner {
    async fn run_once(
        &self,
        agent_name: &str,
        prompt: &str,
        abort: AbortSignal,
        progress: Option<ChildProgressTx>,
    ) -> Result<AgentRunResult, String> {
        run_agent_once_inner_with_sink(
            agent_name,
            prompt,
            abort,
            std::time::Duration::from_secs(CHILD_RUN_TIMEOUT_SECS),
            progress,
            self.session_sink.clone(),
        )
        .await
    }
}

/// 把 child 运行能力注入后台任务管理器（依赖倒置：`background` 不依赖
/// `runtime`，由这里实现它声明的端口）。
pub(crate) struct RuntimeChildRunner;

#[async_trait::async_trait]
impl crate::background::ChildAgentRunner for RuntimeChildRunner {
    async fn run_child(
        &self,
        target_agent: &str,
        prompt: &str,
        abort: AbortSignal,
        progress: Option<ChildProgressTx>,
        session_sink: Option<BackgroundAgentSessionSink>,
    ) -> Result<AgentRunResult, String> {
        run_agent_once_inner_with_sink(
            target_agent,
            prompt,
            abort,
            std::time::Duration::from_secs(CHILD_RUN_TIMEOUT_SECS),
            progress,
            session_sink,
        )
        .await
    }
}

/// child session 一旦建立，后续的配置/环境错误也要成为可读取的运行输出，
/// 不能只给父 Agent 返回一个瞬时错误并留下空 JSONL。
fn persist_agent_start_failure(
    writer: &SharedLog,
    definition: &AgentDefinition,
    session_id: &str,
    prompt: &str,
    error: String,
) -> Result<AgentRunResult, String> {
    let user_message = Message::user_text(prompt);
    let model_name = definition
        .provider
        .as_ref()
        .map(|model| model.display_name().to_string())
        .filter(|name| !name.is_empty())
        .or_else(|| (!definition.model.is_empty()).then(|| definition.model.clone()))
        .unwrap_or_else(|| "unconfigured".into());
    let failure = Message::assistant_error(error, &model_name, StopReason::Error);
    {
        let mut writer = writer
            .lock()
            .map_err(|error| format!("无法锁定会话写入器: {error}"))?;
        writer
            .append_message(&user_message)
            .map_err(|error| format!("无法写入用户消息: {error}"))?;
        writer
            .append_message(&failure)
            .map_err(|error| format!("无法写入 Agent 启动错误: {error}"))?;
    }
    Ok(result_from_messages(
        &definition.name,
        session_id,
        &[failure],
    ))
}

/// 在目标 Agent 下创建一个独立 session 并同步运行到结束。
/// 不占用 UI 当前会话槽，也不向 child 注入 Agent 组合工具。
pub async fn run_agent_once(
    agent_name: &str,
    prompt: &str,
    abort: AbortSignal,
) -> Result<AgentRunResult, String> {
    run_agent_once_inner(
        agent_name,
        prompt,
        abort,
        std::time::Duration::from_secs(CHILD_RUN_TIMEOUT_SECS),
        None,
    )
    .await
}

/// 子 Agent 单次运行的时间上限。到时终止并落盘终态，避免父会话无限阻塞。
pub const CHILD_RUN_TIMEOUT_SECS: u64 = 600;

/// `run_agent_once` 的可注入版本：`timeout` 供测试收紧，`progress` 把子运行
/// 里程碑（工具调用、轮次完成）转发给父 Agent 的工具更新流。
pub(crate) async fn run_agent_once_inner(
    agent_name: &str,
    prompt: &str,
    abort: AbortSignal,
    timeout: std::time::Duration,
    progress: Option<ChildProgressTx>,
) -> Result<AgentRunResult, String> {
    run_agent_once_inner_with_sink(agent_name, prompt, abort, timeout, progress, None).await
}

/// 与 [`run_agent_once_inner`] 相同，但额外通知宿主 child session 已建立。
/// `session_sink` 在文件创建成功后立即调用，因此即使后续配置或 provider
/// 初始化失败，用户仍能在会话列表里打开这条失败记录。
pub(crate) async fn run_agent_once_inner_with_sink(
    agent_name: &str,
    prompt: &str,
    abort: AbortSignal,
    timeout: std::time::Duration,
    progress: Option<ChildProgressTx>,
    session_sink: Option<BackgroundAgentSessionSink>,
) -> Result<AgentRunResult, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("空消息".into());
    }

    let definition = agents::load_agent(agent_name)?;
    let repo = session_repo(&definition)?;
    let writer: SharedLog = Arc::new(Mutex::new(repo.create()?));
    let session_id = writer_session_id(&writer)?;
    let run_id = NEXT_RUN_ID.fetch_add(1, Ordering::AcqRel);
    if let Some(session_sink) = session_sink.as_ref() {
        session_sink(definition.name.clone(), session_id.clone(), run_id);
    }

    let (tool_context, resolved_env) =
        match agents::build_tool_context(&definition, Some(session_id.clone()), abort.clone()) {
            Ok(context) => context,
            Err(error) => {
                return persist_agent_start_failure(
                    &writer,
                    &definition,
                    &session_id,
                    prompt,
                    error,
                );
            }
        };
    append_declared_env(&writer, &resolved_env)?;

    let (model, api_key) = match resolve_model(&definition, None, Some(&resolved_env.vars)) {
        Ok(resolved) => resolved,
        Err(error) => {
            return persist_agent_start_failure(&writer, &definition, &session_id, prompt, error);
        }
    };
    // 有意只构造基础工具。即使目标 agent.json 显式启用了 Agent 组合工具，
    // 它作为 child 运行时也不会拿到这些工具，从而把首版嵌套深度固定为 1。
    let registry = Arc::new(ToolRegistry::for_context(&tool_context));
    let wire_tools = registry.wire_tools();
    let system_prompt = match agents::build_system_prompt_with_tools(&definition, &wire_tools) {
        Ok(system_prompt) => system_prompt,
        Err(error) => {
            return persist_agent_start_failure(&writer, &definition, &session_id, prompt, error);
        }
    };
    let user_message = Message::user_text(prompt);
    writer
        .lock()
        .map_err(|error| format!("无法锁定会话写入器: {error}"))?
        .append_message(&user_message)
        .map_err(|error| format!("无法写入用户消息: {error}"))?;

    let context = AgentContext {
        system_prompt,
        messages: vec![user_message],
    };
    let context_window = model.context_window;
    let stats = Arc::new(Mutex::new(SessionStatsTracker::new(
        (context_window > 0).then_some(context_window),
    )));
    let result_writer = writer.clone();
    // 子运行进度：把里程碑转发给父工具的 on_update（父会话里能看到 child
    // 在做什么）；无观察者时事件照旧丢弃。
    let progress_sink: EventEmitter = {
        let progress = progress.clone();
        let turns = Arc::new(AtomicUsize::new(0));
        Arc::new(move |event| {
            let RuntimeEvent::AgentEvent(envelope) = event else {
                return;
            };
            let Some(progress) = &progress else {
                return;
            };
            match envelope.event {
                AgentEvent::ToolExecutionStart { tool_name, .. } => {
                    let _ = progress.send(pipi_tools::ToolOutput::text(format!(
                        "子 Agent 正在调用工具 {tool_name}"
                    )));
                }
                AgentEvent::MessageEnd { message } if message.role() == "assistant" => {
                    let turn = turns.fetch_add(1, Ordering::AcqRel) + 1;
                    let _ = progress.send(pipi_tools::ToolOutput::text(format!(
                        "子 Agent 完成第 {turn} 轮回复"
                    )));
                }
                _ => {}
            }
        })
    };
    let emitter = make_emitter(
        progress_sink,
        writer,
        stats,
        definition.name.clone(),
        session_id.clone(),
        run_id,
    );
    let config = AgentLoopConfig {
        model: model.clone(),
        provider: provider_for(model.api),
        tools: registry,
        tool_context,
        options: run_stream_options(api_key, &model, &session_id),
        retry: load_settings().retry.policy(),
        tool_execution: ToolExecutionMode::Parallel,
        steering: MessageQueue::new(),
        follow_up: MessageQueue::new(),
        before_tool_call: None,
        after_tool_call: None,
        // 子 Agent 运行同样走投影式压缩流水线（清旧工具输出 → 硬裁），
        // 阈值用子 Agent 自己的 agent.json 配置
        transform_context: (context_window > 0).then(|| {
            projection_transform(pipi_agent::compaction::Budget::from_window(
                context_window,
                definition.compact_threshold_percent(),
            ))
        }),
    };
    let abort_for_result = abort.clone();
    // 超时取消整个循环 future（bash 子进程有 kill_on_drop 兜底）。在追加
    // timeout assistant 前必须补齐已经落盘的工具调用，避免中段留下 orphan。
    let new_messages = match tokio::time::timeout(
        timeout,
        run_agent_loop(Vec::new(), context, config, emitter, abort),
    )
    .await
    {
        Ok(new_messages) => new_messages,
        Err(_) => {
            let timeout_error =
                append_child_timeout_terminal(repo.as_ref(), &result_writer, &model, timeout)?;
            return Ok(result_from_messages(
                &definition.name,
                &session_id,
                &[timeout_error],
            ));
        }
    };
    let mut result = result_from_messages(&definition.name, &session_id, &new_messages);
    // 如果取消恰好发生在一次工具调用完成之后，loop 没有机会再生成一条
    // assistant aborted 消息。补写终态，避免 read_agent 永远把已结束任务报成 pending。
    if abort_for_result.is_aborted() && result.status == AgentRunStatus::Pending {
        let aborted = Message::assistant_error("已中止", model.display_name(), StopReason::Aborted);
        result_writer
            .lock()
            .map_err(|error| format!("无法锁定会话写入器: {error}"))?
            .append_message(&aborted)
            .map_err(|error| format!("无法写入 Agent 中止状态: {error}"))?;
        result = result_from_messages(&definition.name, &session_id, &[aborted]);
    }
    Ok(result)
}

impl RuntimeState {
    /// 启动一轮 Agent；完成后的事件通过 event_sink 广播给宿主。
    ///
    /// `session_id`：
    /// - `Some(id)` —— 在这条会话上跑：已打开就复用，没打开就从文件载入；
    /// - `None` —— 新建一条会话（新文件）。
    ///
    /// 并发口径：守卫是**按会话**的 —— 同一条会话同时只能跑一轮（硬不变量）；
    /// 同一个 Agent 的不同会话、不同 Agent 的会话都各自独立，可以同时跑。
    pub fn send_prompt(
        &self,
        agent_name: &str,
        session_id: Option<&str>,
        prompt: &str,
        model: Option<Model>,
        event_sink: EventEmitter,
    ) -> Result<(), String> {
        if let Some(id) = session_id {
            validate_session_id(id)?;
        }
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return Err("空消息".into());
        }

        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let def = agents::load_agent(agent_name)?;
        // 目标会话：显式指定且已打开 → 复用；否则要载入或新建
        let reuse_key = session_id
            .map(|id| SessionKey::new(agent_name, id))
            .filter(|key| sessions.contains_key(key));
        if let Some(key) = &reuse_key
            && sessions[key].running.is_running()
        {
            return Err(format!(
                "会话「{}」正在运行，请等待完成或先停止",
                key.session_id
            ));
        }
        // 载入 / 新建都在 map 之外完成，全部前置步骤成功后才插进 map ——
        // 中途失败必须保持 map 不变，否则半成品会被下一次 send_prompt 当成可复用的。
        let repo = session_repo(&def)?;
        let mut created: Option<(SessionKey, Session)> = None;
        if reuse_key.is_none() {
            let (key, session) = match session_id {
                Some(id) => {
                    if !repo.exists(id)? {
                        return Err("会话不存在".into());
                    }
                    (
                        SessionKey::new(agent_name, id),
                        Self::load_session_object(&def, repo.as_ref(), id)?,
                    )
                }
                None => {
                    let (session, new_id) =
                        Self::create_session_object(&def, repo.as_ref(), model.as_ref())?;
                    (SessionKey::new(agent_name, &new_id), session)
                }
            };
            created = Some((key, session));
        }
        let session = created
            .as_ref()
            .map(|(_, session)| session)
            .or_else(|| reuse_key.as_ref().map(|key| &sessions[key]))
            .ok_or("无法创建会话")?;
        // 若复用已有会话且传入了明确的模型变更请求
        if reuse_key.is_some()
            && let Some(target) = &model
        {
            let mut current_model_slot = session.model.lock().map_err(|e| e.to_string())?;
            if current_model_slot.as_ref() != Some(target) {
                let mut writer = session.writer.lock().map_err(|e| e.to_string())?;
                writer
                    .append_model_change(target)
                    .map_err(|e| e.to_string())?;
                if let Ok(mut tracker) = session.stats.lock() {
                    let context_max = if target.context_window > 0 {
                        Some(target.context_window)
                    } else {
                        None
                    };
                    tracker.set_context_max(context_max);
                }
                *current_model_slot = Some(target.clone());
            }
        }

        let def = session.agent.clone();
        let current_session_model = session.model.lock().ok().and_then(|m| m.clone());

        let user_message = Message::user_text(prompt);
        let messages = session.messages.clone();
        let writer = session.writer.clone();
        let stats = session.stats.clone();
        let running = session.running.clone();
        let abort = session.abort.clone();
        let steering = session.steering.clone();
        // 收割上一轮结束后仍滞留在 steering 队列里的插话：随本轮一起送入模型
        //（由 emitter 的 MessageEnd 落盘，不会丢）。
        let queued = steering.drain();

        // av 环境契约：会话启动时解析一次（requires fail-closed + AV_* 注入）
        let (mut tool_context, resolved_env) =
            agents::build_tool_context(&def, writer_session_id(&writer).ok(), abort.clone())?;
        // 环境记账：每会话至多一条（writer 幂等去重）；只记声明键（非 process 来源）
        append_declared_env(&writer, &resolved_env)?;
        // provider key 与工具子进程消费同一份 resolved env
        let (model, api_key) = resolve_model(
            &def,
            current_session_model.as_ref(),
            Some(&resolved_env.vars),
        )?;

        abort.reset();
        let running_guard = RunningGuard::new(running.clone());
        let run_token = running_guard.token;
        let run_id = running.current_run_id();
        // 可变：压缩换会话后要改成新 id，随后的 CompactionEnd / AgentEnd 才能
        // 被前端按新身份收下（见 SessionSwitched 事件）。
        let mut session_id = writer_session_id(&writer)?;

        let mut registry = ToolRegistry::for_context(&tool_context);
        let background_owner =
            BackgroundTaskOwner::new(def.name.clone(), session_id.clone(), run_id);
        if tool_context.permissions.tool_enabled("create_agent") {
            registry.push(Arc::new(CreateAgentTool::new(model.clone())));
        }
        if tool_context.permissions.tool_enabled("run_agent") {
            let default_background = tool_context
                .permissions
                .tool_enabled("query_background_tasks")
                && tool_context
                    .permissions
                    .tool_enabled("manage_background_task");
            registry.push(Arc::new(
                RunAgentTool::new_managed(
                    def.name.clone(),
                    background_owner.clone(),
                    Arc::new(RuntimeAgentRunner {
                        session_sink: Some(make_session_change_sink(event_sink.clone())),
                    }),
                    Arc::new(self.background.clone()),
                    default_background,
                )
                .with_session_sink(make_session_change_sink(event_sink.clone())),
            ));
        }
        if tool_context.permissions.tool_enabled("read_agent") {
            registry.push(Arc::new(ReadAgentTool));
        }
        if tool_context
            .permissions
            .tool_enabled("submit_background_task")
        {
            registry.push(Arc::new(SubmitBackgroundTaskTool::new(
                Arc::new(self.background.clone()),
                background_owner.clone(),
            )));
        }
        if tool_context
            .permissions
            .tool_enabled("query_background_tasks")
        {
            registry.push(Arc::new(QueryBackgroundTasksTool::new(
                Arc::new(self.background.clone()),
                background_owner.clone(),
            )));
        }
        if tool_context
            .permissions
            .tool_enabled("manage_background_task")
        {
            registry.push(Arc::new(ManageBackgroundTaskTool::new(
                Arc::new(self.background.clone()),
                background_owner,
            )));
        }
        let registry = Arc::new(registry);
        let wire_tools = registry.wire_tools();
        // api_key 随 config 被移走；压缩摘要调用还要用一份
        let compaction_api_key = api_key.clone();
        // 压缩设置与重试策略在同步段取好，move 进任务（会话仓库在更早处已取）
        let runtime_settings = load_settings();
        let compaction_settings = runtime_settings.compaction;
        let retry_policy = runtime_settings.retry.policy();
        let system_prompt = agents::build_system_prompt_with_tools(&def, &wire_tools)?;

        // 交互审批通道：桌面 / Web 的交互运行才接入；子 Agent 运行不注入，
        // 白名单未命中一律拒绝（fail-closed）。
        tool_context.approver = Some(Arc::new(InteractiveApprover::new(
            self.approval.clone(),
            event_sink.clone(),
            def.name.clone(),
            session_id.clone(),
            run_id,
            abort.clone(),
        )));

        // 压缩预算集中一处（窗口 × Agent 的阈值百分比）：投影式与替换式共用
        let budget = pipi_agent::compaction::Budget::from_window(
            model.context_window,
            def.compact_threshold_percent(),
        )
        .with_target_percent(def.compact_target_percent());
        let config = AgentLoopConfig {
            model: model.clone(),
            provider: provider_for(model.api),
            tools: registry,
            tool_context,
            options: run_stream_options(api_key, &model, &session_id),
            retry: retry_policy,
            tool_execution: ToolExecutionMode::Parallel,
            steering: steering.clone(),
            follow_up: MessageQueue::new(),
            before_tool_call: None,
            after_tool_call: None,
            // 请求前的投影式压缩流水线（pi 的 transformContext 位置）：先清旧
            // 工具输出，仍超预算再硬裁剪 —— 便宜的先用尽，摘要留到 turn 边界。
            // 投影是确定性的、不落盘，回放时重算即可，所以这里改历史不会造成
            // 「live 与重开不一致」。
            transform_context: (model.context_window > 0).then(|| projection_transform(budget)),
        };

        // 首条消息先落盘（崩溃也会留下用户输入）；写入失败必须阻止启动本轮。
        {
            let mut writer = writer
                .lock()
                .map_err(|e| format!("无法锁定会话写入器: {e}"))?;
            writer
                .append_message(&user_message)
                .map_err(|e| format!("无法写入用户消息: {e}"))?;
        }

        let completion_sink = event_sink.clone();
        // make_emitter 会按值取走 writer；压缩阶段还要用它，先克隆
        let compaction_writer = writer.clone();
        // 摘要调用的用量要进会话账本；make_emitter 会按值取走 stats，先克隆
        let ledger_stats = stats.clone();
        let emitter = make_emitter(
            event_sink,
            writer,
            stats,
            def.name.clone(),
            session_id.clone(),
            run_id,
        );
        // 压缩分叉换 id 后把 map 条目搬到新键
        let rekey = make_rekey(self.sessions.clone(), self.background.clone());
        // 显式 spawn 到注入的运行时上：调用线程可能根本不在运行时里
        // （桌面壳的 Tauri command 跑在 GTK 主线程），此时 `tokio::spawn` 会 panic。
        self.spawn_run(async move {
            let _running_guard = running_guard;
            messages.lock().await.push(user_message);

            // 收割循环：循环结束后迟到的 steering（用户在收尾流式期间插话）
            // 不会丢 —— 当作下一轮 prompt 自动续跑，直到队列排空或已中止。
            let mut prompts = queued;
            let mut completion_messages: Vec<Message> = Vec::new();
            loop {
                let context = AgentContext {
                    system_prompt: system_prompt.clone(),
                    messages: messages.lock().await.clone(),
                };
                let new_messages = run_agent_loop(
                    std::mem::take(&mut prompts), // 首轮为空：用户消息已并入 context
                    context,
                    config.clone(),
                    emitter.clone(),
                    abort.clone(),
                )
                .await;

                completion_messages.extend(new_messages.clone());
                messages.lock().await.extend(new_messages);

                let leftover = steering.drain();
                if leftover.is_empty() || abort.is_aborted() {
                    break;
                }
                prompts = leftover;
            }

            // turn 边界的摘要压缩（自动）：历史达到触发线时把旧轮次压成摘要并
            // 持久化。失败不阻塞会话 —— 请求前的投影流水线（清旧工具输出 + 硬裁）
            // 仍在；压缩完成前会话保持 running（见下方 finish），避免压缩期间
            // 新请求读到未压缩历史。
            let outcome = run_compaction(CompactionContext {
                agent_name: &def.name,
                session_id: &mut session_id,
                run_id,
                model: &model,
                api_key: &compaction_api_key,
                budget,
                messages: &messages,
                writer: &compaction_writer,
                stats: &ledger_stats,
                abort: abort.clone(),
                repo: Some(repo.as_ref()),
                settings: compaction_settings,
                retry: retry_policy,
                sink: &completion_sink,
                trigger: CompactionTrigger::Auto,
                rekey: Some(rekey),
            })
            .await;
            if let Err(error) = outcome {
                eprintln!("pipi: 上下文压缩失败（本轮跳过）: {error}");
            }

            running.finish(run_token);

            completion_sink(RuntimeEvent::AgentEvent(AgentEventEnvelope {
                agent_name: def.name,
                session_id,
                run_id,
                event: AgentEvent::AgentEnd {
                    messages: completion_messages,
                },
            }));
        });

        if let Some((key, session)) = created {
            sessions.insert(key, session);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn resolve_model_prefers_session_model_over_agent_default() {
        // 该测试依赖「HOME 下没有 settings.json → 默认 provider 表」，
        // 且 HOME 是进程级状态：持全局锁 + 指向空目录，保证确定性。
        let _guard = pipi_test_support::HOME_LOCK.lock().unwrap();
        let previous_home = std::env::var("HOME").unwrap_or_default();
        let empty_home = std::env::temp_dir().join(format!(
            "pipi-resolve-model-home-{}-{}",
            std::process::id(),
            pipi_protocol::new_id()
        ));
        std::fs::create_dir_all(empty_home.join(".pipi")).unwrap();
        pipi_test_support::set_env_var("HOME", &empty_home);

        let default_model = Model {
            id: "gpt-4o".into(),
            name: "GPT-4o".into(),
            api: pipi_protocol::Api::OpenAICompletions,
            base_url: "https://api.openai.com/v1".into(),
            max_tokens: 4096,
            context_window: 128000,
        };
        let session_model = Model {
            id: "claude-sonnet-4-5".into(),
            name: "Claude Sonnet".into(),
            api: pipi_protocol::Api::AnthropicMessages,
            base_url: "https://api.anthropic.com".into(),
            max_tokens: 8192,
            context_window: 200000,
        };

        let def = AgentDefinition {
            name: "test-agent".into(),
            description: String::new(),
            model: "gpt-4o".into(),
            provider: Some(default_model.clone()),
            workspace: None,
            permissions: Default::default(),
            mcp_servers: Vec::new(),
            subagent: false,
            compact_threshold_percent: 75,
            compact_target_percent: None,
        };

        // 未配置 key 时的校验
        pipi_test_support::remove_env_var("ANTHROPIC_API_KEY");
        pipi_test_support::remove_env_var("OPENAI_API_KEY");

        // 回退默认模型
        let err = super::resolve_model(&def, None, None).unwrap_err();
        assert!(err.contains("未配置 API 密钥"));

        // 优先会话覆盖模型
        let err = super::resolve_model(&def, Some(&session_model), None).unwrap_err();
        assert!(err.contains("api.anthropic.com"));

        // 配上 key 后成功解析
        pipi_test_support::set_env_var("ANTHROPIC_API_KEY", "test-key");
        let (resolved, key) = super::resolve_model(&def, Some(&session_model), None).unwrap();
        assert_eq!(resolved.id, "claude-sonnet-4-5");
        assert_eq!(key, "test-key");
        pipi_test_support::remove_env_var("ANTHROPIC_API_KEY");
        pipi_test_support::set_env_var("HOME", &previous_home);
        let _ = std::fs::remove_dir_all(&empty_home);
    }

    #[test]
    fn timeout_terminal_repairs_a_persisted_tool_call_before_appending_error() {
        let sessions = std::env::temp_dir().join(format!(
            "pipi-timeout-pairing-{}-{}",
            std::process::id(),
            pipi_protocol::new_id()
        ));
        std::fs::create_dir_all(&sessions).unwrap();
        let repo = JsonlSessionRepo::new(sessions.clone());
        let writer: SharedLog = Arc::new(Mutex::new(repo.create().unwrap()));
        let path = sessions.join(format!("{}.jsonl", writer.lock().unwrap().session_id()));

        let user = Message::user_text("run a tool");
        let tool_call = Message::Assistant {
            content: vec![pipi_protocol::ContentBlock::ToolCall {
                id: "call-timeout".into(),
                name: "bash".into(),
                arguments: serde_json::json!({ "command": "sleep 30" }),
            }],
            api: "openai-completions".into(),
            provider: "test".into(),
            model: "test-model".into(),
            usage: Default::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: pipi_protocol::now_millis(),
            duration_ms: None,
        };
        {
            let mut guard = writer.lock().unwrap();
            guard.append_message(&user).unwrap();
            guard.append_message(&tool_call).unwrap();
        }

        let model = Model {
            id: "test-model".into(),
            name: "Test model".into(),
            api: pipi_protocol::Api::OpenAICompletions,
            base_url: String::new(),
            max_tokens: 1,
            context_window: 0,
        };
        append_child_timeout_terminal(&repo, &writer, &model, std::time::Duration::from_secs(1))
            .unwrap();

        let entries = pipi_agent::session::load_session(&path).unwrap();
        let messages = pipi_protocol::session::rebuild_messages(
            &pipi_protocol::session::active_path(&entries),
        );
        assert_eq!(messages.len(), 4);
        assert!(matches!(
            &messages[2],
            Message::ToolResult {
                tool_call_id,
                tool_name,
                is_error: true,
                ..
            } if tool_call_id == "call-timeout" && tool_name == "bash"
        ));
        assert!(matches!(
            &messages[3],
            Message::Assistant {
                stop_reason: StopReason::Error,
                error_message: Some(message),
                ..
            } if message.contains("超时")
        ));

        drop(writer);
        std::fs::remove_dir_all(sessions).unwrap();
    }

    /// 子 Agent 挂死（端点接受连接但不回包）时，运行时限必须终止本轮并把
    /// 超时终态落盘 —— read_agent 读到 Failed 而不是永远的 pending。
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn child_run_times_out_and_persists_terminal_state() {
        let _guard = pipi_test_support::HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            // 故意持有每条连接：请求悬挂，直到 tokio 超时触发
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                held.push(stream);
            }
        });

        let home = std::env::temp_dir().join(format!(
            "pipi-child-timeout-{}-{}",
            std::process::id(),
            pipi_protocol::new_id()
        ));
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        pipi_test_support::set_env_var("HOME", &home);

        let base_url = format!("http://{addr}/v1");
        pipi_core::settings::save_settings(&pipi_core::settings::Settings {
            theme: pipi_core::settings::Theme::Dark,
            providers: vec![pipi_core::settings::ProviderConfig {
                id: "silent".into(),
                name: "Silent".into(),
                api: pipi_protocol::Api::OpenAICompletions,
                base_url: base_url.clone(),
                env_key: None,
                api_key: Some("test-key".into()),
            }],
            default_provider_id: None,
            compaction: Default::default(),
            retry: Default::default(),
        })
        .unwrap();

        agents::create_agent(
            "timeout-worker",
            "A worker whose endpoint never responds",
            Some(workspace.to_str().unwrap()),
            Some(pipi_tools::permissions::PermissionsConfig {
                tools: vec!["read".into()],
                bash: Default::default(),
                sandbox: pipi_tools::permissions::SandboxMode::WorkspaceWrite,
            }),
            None,
            Some(pipi_protocol::Model {
                id: "silent-model".into(),
                name: "Silent Model".into(),
                api: pipi_protocol::Api::OpenAICompletions,
                base_url,
                max_tokens: 64,
                context_window: 4096,
            }),
        )
        .unwrap();

        let result = run_agent_once_inner(
            "timeout-worker",
            "hang",
            AbortSignal::new(),
            std::time::Duration::from_millis(300),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.status, AgentRunStatus::Failed);
        assert!(
            result.response.contains("超时"),
            "应报超时: {}",
            result.response
        );

        // 终态已落盘：read_agent 读到 Failed 而不是 pending
        let reread =
            pipi_core::tools::agent::read_agent_output("timeout-worker", Some(&result.session_id))
                .unwrap();
        assert_eq!(reread.status, AgentRunStatus::Failed);

        let _ = std::fs::remove_dir_all(home);
    }
}
