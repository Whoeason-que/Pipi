//! 交互运行的产品装配与宿主调度。
use super::*;

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
            && sessions[key].engine.snapshot()?.running
        {
            return Err(format!(
                "会话「{}」正在运行，请等待完成或先停止",
                key.session_id
            ));
        }
        // 载入 / 新建都在 map 之外完成，全部前置步骤成功后才插进 map ——
        // 中途失败必须保持 map 不变，否则半成品会被下一次 send_prompt 当成可复用的。
        let repo = session_repo(&def)?;
        let mut created: Option<(SessionKey, HostSession)> = None;
        if reuse_key.is_none() {
            let (key, session) = match session_id {
                Some(id) => {
                    if !repo.exists(id)? {
                        return Err("会话不存在".into());
                    }
                    (
                        SessionKey::new(agent_name, id),
                        Self::load_session_object(&def, repo.clone(), id)?,
                    )
                }
                None => {
                    let (session, new_id) =
                        Self::create_session_object(&def, repo.clone(), model.as_ref())?;
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
            session
                .engine
                .set_model(Some(target.clone()), Some(target.clone()))?;
        }
        let def = session.agent.clone();
        let current_session_model = session.engine.snapshot()?.selected_model;
        let abort = session.engine.abort_signal();
        let session_id = session.engine.snapshot()?.session_id;

        // Agent 契约（agent.toml + 项目层、`[resources]` 素材、声明的 store
        // 技能）每轮只解析一次：工具上下文与系统提示共用同一份，避免两次
        // 解析之间文件变化导致口径矛盾。
        let prepared = agents::prepare_agent(&def)?;
        // av 环境契约：会话启动时解析一次（requires fail-closed + AV_* 注入）
        let (mut tool_context, resolved_env) =
            agents::build_tool_context(&def, &prepared, Some(session_id.clone()), abort.clone())?;
        // 环境记账：每会话至多一条（writer 幂等去重）；只记声明键（非 process 来源）
        append_declared_env(&session.engine, &resolved_env)?;
        // provider key 与工具子进程消费同一份 resolved env
        let ResolvedModel {
            model,
            api_key,
            auto_protocol,
        } = resolve_model(
            &def,
            current_session_model.as_ref(),
            Some(&resolved_env.vars),
        )?;

        let reservation = session.engine.reserve_run(AbortPolicy::Reset)?;
        let run_id = reservation.run_id();

        let mut registry = pipi_tools::builtin_registry(&tool_context);
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
        // 压缩设置与重试策略在同步段取好，move 进任务（会话仓库在更早处已取）
        let runtime_settings = load_settings();
        let compaction_settings = runtime_settings.compaction;
        let retry_policy = runtime_settings.retry.policy();
        let system_prompt = agents::build_system_prompt_with_tools(&def, &prepared, &wire_tools)?;

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
        let budget = pipi_context::compaction::Budget::from_window(
            model.context_window,
            def.compact_threshold_percent(),
        )
        .with_target_percent(def.compact_target_percent());
        let hooks = make_session_hooks(
            event_sink,
            def.name.clone(),
            Some(make_rekey(self.sessions.clone(), self.background.clone())),
        );
        let prepared = reservation.prepare(
            RunRequest {
                prompt,
                system_prompt,
                model: model.clone(),
                provider: provider_for(),
                tools: registry,
                tool_context,
                options: run_stream_options(api_key, &model, &session_id, auto_protocol),
                retry: retry_policy,
                tool_execution: ToolExecutionMode::Parallel,
                budget: (model.context_window > 0).then_some(budget),
                compaction: Some(CompactionPolicy {
                    fork_before_compact: compaction_settings.fork_before_compact,
                    archive_original: compaction_settings.archive_original,
                }),
                timeout: None,
            },
            hooks,
        )?;
        self.spawn_run(async move {
            if let Err(error) = prepared.run().await {
                eprintln!("pipi: 会话运行失败: {error}");
            }
        });

        if let Some((key, session)) = created {
            sessions.insert(key, session);
        }
        Ok(())
    }
}
