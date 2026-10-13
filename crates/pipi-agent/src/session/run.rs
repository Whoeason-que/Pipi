//! 同步预约、先落输入，再异步驱动单会话；宿主负责选择执行器。
use super::*;

pub struct RunReservation {
    // 未轮询即丢弃时也先释放账本引用，再释放占位。
    pub(super) session: SessionHandle,
    pub(super) guard: RunningGuard,
}

enum Operation {
    Run(Box<RunRequest>),
    Compact(Box<CompactionRequest>),
}

pub struct PreparedRun {
    session: SessionHandle,
    operation: Operation,
    hooks: SessionHooks,
    guard: RunningGuard,
}

impl RunReservation {
    pub fn run_id(&self) -> usize {
        self.guard.run_id()
    }
    pub fn session_id(&self) -> Result<String, String> {
        self.session.session_id()
    }

    pub fn prepare(
        self,
        mut request: RunRequest,
        hooks: SessionHooks,
    ) -> Result<PreparedRun, String> {
        request.prompt = request.prompt.trim().into();
        if request.prompt.is_empty() {
            return Err("空消息".into());
        }
        request.options.session_id = Some(self.session_id()?);
        request.tool_context.abort = self.session.abort.clone();
        let message = Message::user_text(request.prompt.clone());
        let mut history = self.session.messages.lock().map_err(|e| e.to_string())?;
        self.session
            .writer
            .lock()
            .map_err(|e| format!("无法锁定会话写入器: {e}"))?
            .append_message(&message)
            .map_err(|e| format!("无法写入用户消息: {e}"))?;
        history.push(message.clone());
        drop(history);
        Ok(PreparedRun {
            session: self.session,
            operation: Operation::Run(Box::new(request)),
            hooks,
            guard: self.guard,
        })
    }

    pub fn prepare_compaction(
        self,
        request: CompactionRequest,
        hooks: SessionHooks,
    ) -> PreparedRun {
        PreparedRun {
            session: self.session,
            operation: Operation::Compact(Box::new(request)),
            hooks,
            guard: self.guard,
        }
    }

    pub fn fail_start(
        self,
        prompt: &str,
        model_name: &str,
        error: String,
    ) -> Result<RunOutcome, String> {
        let failure = Message::assistant_error(error, model_name, StopReason::Error);
        let id = self.session_id()?;
        {
            let mut history = self.session.messages.lock().map_err(|e| e.to_string())?;
            let mut writer = self.session.writer.lock().map_err(|e| e.to_string())?;
            let input = Message::user_text(prompt);
            writer.append_message(&input).map_err(|e| e.to_string())?;
            history.push(input);
            writer.append_message(&failure).map_err(|e| e.to_string())?;
            history.push(failure.clone());
            drop(writer);
            drop(history);
            self.session
                .stats
                .lock()
                .map_err(|e| e.to_string())?
                .record(&failure);
        }
        // 提交点（启动失败）：这两条今天即时落盘，git 载体下不提交就会随进程消失
        if let Err(error) = commit::commit_shared(&self.session.writer) {
            eprintln!("pipi: 启动失败的记录未能提交: {error}");
        }
        Ok(RunOutcome {
            session_id: id,
            messages: vec![failure],
        })
    }
}

impl PreparedRun {
    pub async fn run(self) -> Result<RunOutcome, String> {
        // guard 先绑定，内部 future 的所有账本引用先析构，再清 running。
        let Self {
            guard,
            session,
            operation,
            hooks,
        } = self;
        let id = session.session_id()?;
        let run_id = guard.run_id();
        let outcome = match operation {
            Operation::Run(request) => drive_run(session, *request, hooks.clone(), run_id).await,
            Operation::Compact(request) => {
                hooks.agent(&id, run_id, AgentEvent::AgentStart);
                drive_compaction(session, *request, hooks.clone(), run_id).await
            }
        };
        drop(guard);
        let (id, messages) = match &outcome {
            Ok(outcome) => (&outcome.session_id, outcome.messages.clone()),
            Err(error) => {
                hooks.error(&id, run_id, error.clone());
                (&id, Vec::new())
            }
        };
        hooks.agent(id, run_id, AgentEvent::AgentEnd { messages });
        outcome
    }
}

async fn drive_compaction(
    session: SessionHandle,
    request: CompactionRequest,
    hooks: SessionHooks,
    run_id: usize,
) -> Result<RunOutcome, String> {
    let mut id = session.session_id()?;
    if let Err(error) =
        compaction::run_compaction(&session, &mut id, run_id, &request, &hooks, true).await
    {
        hooks.error(&id, run_id, format!("压缩未执行：{error}"));
    }
    Ok(RunOutcome {
        session_id: id,
        messages: Vec::new(),
    })
}

async fn drive_run(
    session: SessionHandle,
    request: RunRequest,
    hooks: SessionHooks,
    run_id: usize,
) -> Result<RunOutcome, String> {
    let mut id = session.session_id()?;
    let observed = Arc::new(Mutex::new(Vec::<Message>::new()));
    let ledger_emitter = events::make_emitter(
        hooks.clone(),
        session.writer.clone(),
        session.stats.clone(),
        id.clone(),
        run_id,
    );
    let emitter: Emitter = {
        let observed = observed.clone();
        Arc::new(move |event| {
            if let AgentEvent::MessageEnd { message } = &event {
                observed.lock().unwrap().push(message.clone());
            }
            ledger_emitter(event);
        })
    };
    let config = AgentLoopConfig {
        model: request.model.clone(),
        provider: request.provider.clone(),
        tools: request.tools,
        tool_context: request.tool_context,
        options: request.options.clone(),
        retry: request.retry,
        tool_execution: request.tool_execution,
        steering: session.steering.clone(),
        follow_up: MessageQueue::new(),
        before_tool_call: None,
        transform_context: request.budget.map(|budget| {
            Arc::new(move |messages| pipi_context::compaction::project(messages, budget))
                as crate::agent_loop::TransformContextHook
        }),
    };
    let drive = harvest(
        &session,
        request.system_prompt,
        config,
        emitter,
        observed.clone(),
    );
    let needs_abort_terminal = request.timeout.is_some();
    let mut messages = if let Some(timeout) = request.timeout {
        match tokio::time::timeout(timeout.duration, drive).await {
            Ok(messages) => messages,
            Err(_) => {
                session
                    .messages
                    .lock()
                    .expect("session history poisoned")
                    .extend(observed.lock().unwrap().clone());
                let terminal = ledger::append_terminal(
                    &session,
                    &request.model,
                    timeout.message,
                    StopReason::Error,
                )?;
                let result = terminal.last().cloned().into_iter().collect();
                session
                    .messages
                    .lock()
                    .expect("session history poisoned")
                    .extend(terminal);
                // 提交点（超时收尾）：必须在运行守卫释放之前
                if let Err(error) = commit::commit_log(&session).await {
                    eprintln!("pipi: 会话提交失败（超时收尾，条目留在缓冲）: {error}");
                }
                return Ok(RunOutcome {
                    session_id: id,
                    messages: result,
                });
            }
        }
    } else {
        drive.await
    };
    // 一次性运行在工具完成后中止时，loop 可能尚未生成 assistant 终态。
    if needs_abort_terminal
        && session.abort.is_aborted()
        && messages
            .iter()
            .rev()
            .find_map(|m| match m {
                Message::Assistant { stop_reason, .. } => Some(*stop_reason),
                _ => None,
            })
            .is_none_or(|reason| matches!(reason, StopReason::ToolUse | StopReason::Pending))
    {
        let terminal = ledger::append_terminal(
            &session,
            &request.model,
            "已中止".into(),
            StopReason::Aborted,
        )?;
        session
            .messages
            .lock()
            .expect("session history poisoned")
            .extend(terminal.clone());
        messages.extend(terminal);
    }
    // 提交点（O2，一轮一个 commit）：本轮全部条目在这里一次落盘。位置有三条
    // 约束 —— 在压缩之前（压缩分叉从分支 tip 起，未提交会丢整轮）、在运行守卫
    // 释放之前、在 AgentEnd 之前（否则「running 已释放」≠「历史已落盘」，前端
    // 收到 AgentEnd 后立刻发的新一轮会与提交竞争同一分支）。提交失败不失败本轮：
    // 条目留在缓冲里，随下一轮再提交（自愈）。
    if let Err(error) = commit::commit_log(&session).await {
        eprintln!("pipi: 会话提交失败（条目留在缓冲，随下一轮再提交）: {error}");
        hooks.error(&id, run_id, format!("会话提交失败：{error}"));
    }
    if let (Some(budget), Some(policy)) = (request.budget, request.compaction) {
        let compact = CompactionRequest {
            model: request.model,
            provider: request.provider,
            options: request.options,
            retry: request.retry,
            budget,
            policy,
        };
        if let Err(error) =
            compaction::run_compaction(&session, &mut id, run_id, &compact, &hooks, false).await
        {
            eprintln!("pipi: 上下文压缩失败（本轮跳过）: {error}");
        }
    }
    Ok(RunOutcome {
        session_id: id,
        messages,
    })
}

async fn harvest(
    session: &SessionHandle,
    system_prompt: String,
    config: AgentLoopConfig,
    emitter: Emitter,
    observed: Arc<Mutex<Vec<Message>>>,
) -> Vec<Message> {
    let mut prompts = session.steering.drain();
    let mut completion = Vec::new();
    loop {
        observed.lock().unwrap().clear();
        let context = AgentContext {
            system_prompt: system_prompt.clone(),
            messages: session
                .messages
                .lock()
                .expect("session history poisoned")
                .clone(),
        };
        let added = run_agent_loop(
            std::mem::take(&mut prompts),
            context,
            config.clone(),
            emitter.clone(),
            session.abort.clone(),
        )
        .await;
        completion.extend(added.clone());
        session
            .messages
            .lock()
            .expect("session history poisoned")
            .extend(added);
        let leftover = session.steering.drain();
        if leftover.is_empty() || session.abort.is_aborted() {
            break;
        }
        prompts = leftover;
    }
    completion
}
