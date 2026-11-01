//! 交互运行的产品装配与宿主调度。
use super::*;

/// 一轮的用户侧输入：普通消息，或宿主注入的后台任务完成通知。
///
/// 两条路径共用同一套装配（Agent 定义、工具上下文、系统提示、压缩预算），
/// 差别只在“这一轮因为什么开始”和落账的条目类型。
pub(super) enum PromptInput {
    User {
        prompt: String,
        content: Option<pipi_protocol::UserContent>,
        model: Option<Model>,
    },
    BackgroundResults(Vec<pipi_protocol::BackgroundTaskResult>),
}

impl RuntimeState {
    /// 启动一轮 Agent；完成后的事件通过 event_sink 广播给宿主。
    ///
    /// `session_id`：
    /// - `Some(id)` —— 在这条会话上跑：已打开就复用，没打开就从文件载入；
    /// - `None` —— 新建一条会话（新文件）。
    ///
    /// 返回这条会话的 [`SessionInfo`]：调用方（前端）据此直接拿到权威身份
    /// （新会话的 id、run、模型都在里面），不必从事件里反推。事件与返回值走
    /// 不同通道、先后无保证，认领必须容忍两种顺序。
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
    ) -> Result<SessionInfo, String> {
        self.send_prompt_with_images(agent_name, session_id, prompt, model, event_sink, &[])
    }

    pub fn send_prompt_with_images(
        &self,
        agent_name: &str,
        session_id: Option<&str>,
        prompt: &str,
        model: Option<Model>,
        event_sink: EventEmitter,
        images: &[pipi_protocol::UserImage],
    ) -> Result<SessionInfo, String> {
        let content = attachments::user_content(prompt, images)?;
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        self.send_prompt_locked(
            &mut sessions,
            agent_name,
            session_id,
            PromptInput::User {
                prompt: prompt.to_string(),
                content,
                model,
            },
            event_sink,
        )
    }

    /// 后台任务唤醒：在已有会话上追加 `background_result` 条目并启动一轮。
    /// 会话不必须已经打开（空闲释放后会从载体重新载入），但绝不能新建会话。
    pub(super) fn send_background_results(
        &self,
        agent_name: &str,
        session_id: &str,
        results: Vec<pipi_protocol::BackgroundTaskResult>,
        event_sink: EventEmitter,
    ) -> Result<SessionInfo, String> {
        if results.is_empty() {
            return Err("No background results to deliver".into());
        }
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        self.send_prompt_locked(
            &mut sessions,
            agent_name,
            Some(session_id),
            PromptInput::BackgroundResults(results),
            event_sink,
        )
    }

    /// 排队取出与运行预约共享 sessions 锁，避免取消/停止与发送之间留下竞态。
    pub(super) fn send_prompt_locked(
        &self,
        sessions: &mut HashMap<SessionKey, HostSession>,
        agent_name: &str,
        session_id: Option<&str>,
        input: PromptInput,
        event_sink: EventEmitter,
    ) -> Result<SessionInfo, String> {
        let (prompt, content, model, results) = match input {
            PromptInput::User {
                prompt,
                content,
                model,
            } => (prompt.trim().to_string(), content, model, Vec::new()),
            PromptInput::BackgroundResults(results) => (String::new(), None, None, results),
        };
        if let Some(id) = session_id {
            validate_session_id(id)?;
        }
        if prompt.is_empty() && content.is_none() && results.is_empty() {
            return Err("Message cannot be empty".into());
        }
        if !results.is_empty() && session_id.is_none() {
            return Err("Background results require an existing session".into());
        }

        self.ensure_not_shutting_down()?;
        let def = agents::load_agent(agent_name)?;
        // 目标会话：显式指定且已打开 → 复用；否则要载入或新建
        let reuse_key = session_id
            .map(|id| SessionKey::new(agent_name, id))
            .filter(|key| sessions.contains_key(key));
        if let Some(key) = &reuse_key
            && sessions[key].engine.snapshot()?.running
        {
            return Err(format!(
                "Session \"{}\" is running, please wait for completion or stop it first",
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
                        return Err("Session does not exist".into());
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
            .as_mut()
            .map(|(_, session)| session)
            .or_else(|| reuse_key.as_ref().and_then(|key| sessions.get_mut(key)))
            .ok_or("Failed to create session")?;
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
        // MCP：按定义刷新服务器集合。连接在后台建立，本轮注册表只声明已知清单
        // （重启后的第一轮由 <Agent>/mcp-cache/ 的快照兜住）。
        self.mcp.prepare(&def, &resolved_env.vars);
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
        // 权威身份在起跑前就取好（纯内存快照）：此刻 session_id / run_id /
        // running / model 都已确定，失败也发生在新消息落盘之前。
        let info = self.session_info_of(session)?;
        let event_sink = crate::notifications::run_emitter(event_sink, session.temporary);

        // 论坛：仅在启用任一 forum_* 工具时才解析根目录（避免无代价地创建目录），
        // 并把根目录加入受信任读取根，让 read/grep 也能直接看论坛文件。
        // 仓库只开一次，供工具与 run 启动时的未读摘要共用。
        // **论坛不可用不应让主 run 失败**（根目录被换成符号链接、权限不足等）：
        // 当作「本轮没有论坛能力」降级处理。
        let forum = if crate::forum::enabled(&tool_context.permissions) {
            let root = crate::forum::resolve_root(&tool_context.workspace);
            tool_context.read_roots.push(root.clone());
            match pipi_forum::FileForumRepo::open(root.clone()) {
                Ok(repo) => Some((
                    root,
                    Arc::new(repo) as Arc<dyn pipi_protocol::forum::ForumRepo>,
                )),
                Err(_) => None,
            }
        } else {
            None
        };

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
                        runtime: self.clone(),
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
        if tool_context.permissions.tool_enabled("session_search") {
            registry.push(Arc::new(
                crate::tools::session_search::SessionSearchTool::new(
                    repo.clone(),
                    Some(session_id.clone()),
                ),
            ));
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
        if let Some((root, repo)) = &forum {
            crate::forum::register_tools(
                &mut registry,
                repo.clone(),
                root,
                &tool_context.permissions,
                &def.name,
                format!("run:{session_id}:{run_id}"),
            );
        }
        // MCP 工具：启用一台服务器就是它那批工具的授权（不经过 permissions.tools）。
        // 清单来自缓存，顺序稳定；服务器进程在后台连接，调用时按需重连。
        for spec in self.mcp.tools_for(&def.name) {
            registry.push(Arc::new(crate::mcp::McpTool::new(
                self.mcp.clone(),
                def.name.clone(),
                spec,
            )));
        }
        let registry = Arc::new(registry);
        let wire_tools = registry.wire_tools();
        // 压缩设置与重试策略在同步段取好，move 进任务（会话仓库在更早处已取）
        let runtime_settings = load_settings();
        let compaction_settings = runtime_settings.compaction;
        let retry_policy = runtime_settings.retry.policy();
        let mut system_prompt =
            agents::build_system_prompt_with_tools(&def, &prepared, &wire_tools)?;
        // 拉式通知：把该 Agent 的未读摘要拼进系统提示（不推进游标 —— 读取才推进）。
        // `systemPrompt: none` 的 Agent 不发 system，也就不拼摘要；摘要计算失败只跳过
        // （不因论坛而让整个 run 失败）。
        if let Some((root, repo)) = &forum
            && !matches!(def.system_prompt, pipi_venv::agents::PromptScope::None)
            && let Ok(Some(digest)) = crate::forum::digest(repo.as_ref(), root, &def.name)
        {
            if system_prompt.is_empty() {
                system_prompt = digest;
            } else {
                system_prompt.push_str("\n\n");
                system_prompt.push_str(&digest);
            }
        }

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
        let request = RunRequest {
            prompt,
            user_content: content,
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
        };
        let prepared = if results.is_empty() {
            reservation.prepare(request, hooks)?
        } else {
            // 后台通知：先落 background_result 条目，再启动同一轮。
            reservation.prepare_background_results(results, request, hooks)?
        };
        let wake = session.queue.wake.clone();
        session.queue.active_run = Some(run_id);
        let runtime = self.clone();
        self.spawn_run(async move {
            let result = prepared.run().await;
            if let Err(error) = &result {
                eprintln!("pipi: session run failed: {error}");
            }
            runtime.finish_queued_run(&wake, run_id, result);
        });

        if let Some((key, session)) = created {
            sessions.insert(key, session);
        }
        Ok(info)
    }
}
