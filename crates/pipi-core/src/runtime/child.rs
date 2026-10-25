//! 子运行的能力装配、进度适配与结果转换。
use super::*;
use crate::tools::agent::AgentRunStatus;

pub(super) struct RuntimeAgentRunner {
    pub(super) runtime: RuntimeState,
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
        run_agent_once_with_sink(
            agent_name,
            prompt,
            abort,
            std::time::Duration::from_secs(CHILD_RUN_TIMEOUT_SECS),
            progress,
            self.session_sink.clone(),
        )
        .await
    }

    async fn run_in_session(
        &self,
        agent_name: &str,
        session_id: &str,
        prompt: &str,
        abort: AbortSignal,
        progress: Option<ChildProgressTx>,
    ) -> Result<AgentRunResult, String> {
        self.runtime
            .run_agent_in_session(
                agent_name,
                session_id,
                prompt,
                abort,
                std::time::Duration::from_secs(CHILD_RUN_TIMEOUT_SECS),
                progress,
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
        run_agent_once_with_sink(
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

pub async fn run_agent_once(
    agent_name: &str,
    prompt: &str,
    abort: AbortSignal,
) -> Result<AgentRunResult, String> {
    run_agent_once_with_sink(
        agent_name,
        prompt,
        abort,
        std::time::Duration::from_secs(CHILD_RUN_TIMEOUT_SECS),
        None,
        None,
    )
    .await
}

/// 宿主在已有会话上运行一轮并等待结束。
///
/// 复用 [`RuntimeState::send_prompt`] 的会话装载、模型解析、工具装配和排队语义，
/// 因而不会为同一条 git 分支创建第二个 writer/运行守卫。事件流同时用于：
/// - 把工具调用与轮次完成转发给父工具；
/// - 捕获 `AgentEnd`（提交与运行守卫释放之后）作为本次运行的终态；
/// - 在父运行中止时停止目标会话，避免 child 在后台继续跑。
impl RuntimeState {
    pub async fn run_agent_in_session(
        &self,
        agent_name: &str,
        session_id: &str,
        prompt: &str,
        abort: AbortSignal,
        timeout: std::time::Duration,
        progress: Option<ChildProgressTx>,
    ) -> Result<AgentRunResult, String> {
        validate_session_id(session_id)?;
        if prompt.trim().is_empty() {
            return Err("Message cannot be empty".into());
        }

        let current_id = Arc::new(Mutex::new(session_id.to_string()));
        let (completion_tx, mut completion_rx) = tokio::sync::mpsc::unbounded_channel();
        let agent = agent_name.to_string();
        let sink: EventEmitter = {
            let agent = agent.clone();
            let current_id = current_id.clone();
            let completion_tx = completion_tx.clone();
            let progress = progress.clone();
            let turns = Arc::new(AtomicUsize::new(0));
            Arc::new(move |event| match event {
                RuntimeEvent::AgentEvent(envelope) => {
                    let AgentEventEnvelope {
                        agent_name,
                        session_id,
                        event,
                        ..
                    } = envelope;
                    let matches_current = {
                        let current = current_id
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        agent_name == agent && session_id == *current
                    };
                    if !matches_current {
                        return;
                    }
                    if let Some(progress) = &progress {
                        match &event {
                            AgentEvent::ToolExecutionStart { tool_name, .. } => {
                                let _ = progress.send(pipi_protocol::tools::ToolOutput::text(
                                    format!("Subagent is calling tool {tool_name}"),
                                ));
                            }
                            AgentEvent::MessageEnd { message } if message.role() == "assistant" => {
                                let turn = turns.fetch_add(1, Ordering::AcqRel) + 1;
                                let _ = progress.send(pipi_protocol::tools::ToolOutput::text(
                                    format!("Subagent completed turn {turn}"),
                                ));
                            }
                            _ => {}
                        }
                    }
                    if let AgentEvent::AgentEnd { messages } = event {
                        let _ = completion_tx.send(FollowEvent::End {
                            session_id,
                            messages,
                        });
                    }
                }
                RuntimeEvent::SessionSwitched(envelope) => {
                    if envelope.agent_name == agent {
                        let mut current = current_id
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if envelope.session_id == *current {
                            *current = envelope.to_session_id;
                        }
                    }
                }
                RuntimeEvent::SessionError(envelope) => {
                    let current = current_id
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if envelope.agent_name == agent && envelope.session_id == *current {
                        let _ = completion_tx.send(FollowEvent::Error(envelope.message));
                    }
                }
                _ => {}
            })
        };

        let info = self.send_prompt(agent_name, Some(session_id), prompt, None, sink)?;
        *current_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = info.session_id.clone();

        // watcher 负责响应父运行的中止；guard 负责本 future 被直接丢弃/提前返回
        // 时也不要留下仍在跑的 child。
        let mut stop_guard = SessionStopGuard {
            runtime: self.clone(),
            agent: agent_name.to_string(),
            current_id: current_id.clone(),
            armed: true,
        };
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let watcher = {
            let runtime = self.clone();
            let agent = agent.clone();
            let current_id = current_id.clone();
            let abort = abort.clone();
            tokio::spawn(async move {
                tokio::select! {
                    biased;
                    _ = abort.wait_aborted() => {
                        let session_id = current_id
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone();
                        if let Err(error) = runtime.stop_run(&agent, &session_id) {
                            eprintln!(
                                "pipi: failed to stop child session {session_id} after parent abort: {error}"
                            );
                        }
                    }
                    _ = done_rx => {}
                }
            })
        };

        let waited = tokio::time::timeout(timeout, wait_for_agent_end(&mut completion_rx)).await;
        let timed_out = waited.is_err();
        let mut stopped_after_timeout = false;
        if timed_out {
            let session_id = current_id
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let stopped = self.stop_run(agent_name, &session_id).is_ok();
            if stopped {
                // 只有观察到 AgentEnd 才认为 child 真的结束；否则保留 guard，
                // 让 drop 再尝试一次，并把超时报告为「未确认停止」。
                stopped_after_timeout = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    wait_for_agent_end(&mut completion_rx),
                )
                .await
                .ok()
                .and_then(Result::ok)
                .is_some();
            }
        }
        let _ = done_tx.send(());
        watcher.abort();

        match waited {
            Ok(Ok((final_session_id, messages, error))) => {
                stop_guard.armed = false;
                if messages.is_empty()
                    && let Some(error) = error
                {
                    return Err(error);
                }
                let result = result_from_messages(agent_name, &final_session_id, &messages);
                if result.status == AgentRunStatus::Pending
                    && let Some(error) = error
                {
                    return Err(error);
                }
                Ok(result)
            }
            Ok(Err(error)) => Err(error),
            Err(_) if stopped_after_timeout => {
                stop_guard.armed = false;
                Ok(AgentRunResult {
                    agent_name: agent_name.to_string(),
                    session_id: current_id
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone(),
                    status: AgentRunStatus::Failed,
                    response: format!(
                        "Subtask run timed out ({} seconds), terminated",
                        timeout.as_secs()
                    ),
                })
            }
            Err(_) => Err(
                "Subtask run timed out and could not be confirmed stopped; it may still be running"
                    .into(),
            ),
        }
    }
}

enum FollowEvent {
    End {
        session_id: String,
        messages: Vec<Message>,
    },
    Error(String),
}

/// 本 future 被丢弃时停止目标会话，避免 child 在父运行已退出后继续写同一分支。
struct SessionStopGuard {
    runtime: RuntimeState,
    agent: String,
    current_id: Arc<Mutex<String>>,
    armed: bool,
}

impl Drop for SessionStopGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let session_id = self
            .current_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let _ = self.runtime.stop_run(&self.agent, &session_id);
    }
}

async fn wait_for_agent_end(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<FollowEvent>,
) -> Result<(String, Vec<Message>, Option<String>), String> {
    let mut last_error = None;
    while let Some(event) = rx.recv().await {
        match event {
            FollowEvent::Error(message) => last_error = Some(message),
            FollowEvent::End {
                session_id,
                messages,
            } => return Ok((session_id, messages, last_error)),
        }
    }
    Err(last_error.unwrap_or_else(|| "Agent run ended without a terminal event".into()))
}

/// 子 Agent 单次运行的时间上限。到时终止并落盘终态，避免父会话无限阻塞。
const CHILD_RUN_TIMEOUT_SECS: u64 = 600;

/// `run_agent_once` 的可注入版本：`timeout` 供测试收紧，`progress` 把子运行
/// 里程碑（工具调用、轮次完成）转发给父 Agent 的工具更新流。
pub(crate) async fn run_agent_once_with_sink(
    agent_name: &str,
    prompt: &str,
    abort: AbortSignal,
    timeout: std::time::Duration,
    progress: Option<ChildProgressTx>,
    session_sink: Option<BackgroundAgentSessionSink>,
) -> Result<AgentRunResult, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("Message cannot be empty".into());
    }
    let definition = agents::load_agent(agent_name)?;
    let repo = session_repo(&definition)?;
    let engine = AgentSession::create(
        SessionStorage {
            log: repo.create()?,
            repo: Some(repo),
        },
        definition.provider.clone(),
        None,
        abort.clone(),
    )?;
    let reservation = engine.reserve_run(AbortPolicy::Preserve)?;
    let session_id = reservation.session_id()?;
    let run_id = reservation.run_id();
    if let Some(sink) = session_sink {
        sink(definition.name.clone(), session_id.clone(), run_id);
    }
    let inputs = (|| {
        // 子运行同样一轮只准备一次：契约解析失败（含素材与 store 技能）与
        // 工具上下文、提示词组装共用同一份结果、同一套 fail-closed 口径。
        let prepared = agents::prepare_agent(&definition)?;
        let (tool_context, resolved_env) =
            agents::build_tool_context(&definition, &prepared, Some(session_id.clone()), abort)?;
        append_declared_env(&engine, &resolved_env)?;
        let resolved = resolve_model(&definition, None, Some(&resolved_env.vars))?;
        let tools = Arc::new(pipi_tools::builtin_registry(&tool_context));
        let system_prompt =
            agents::build_system_prompt_with_tools(&definition, &prepared, &tools.wire_tools())?;
        Ok::<_, String>((tool_context, tools, system_prompt, resolved))
    })();
    let (
        tool_context,
        tools,
        system_prompt,
        ResolvedModel {
            model,
            api_key,
            auto_protocol,
        },
    ) = match inputs {
        Ok(inputs) => inputs,
        Err(error) => {
            let name = definition
                .provider
                .as_ref()
                .map(|m| m.display_name().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| (!definition.model.is_empty()).then(|| definition.model.clone()))
                .unwrap_or_else(|| "unconfigured".into());
            let result = reservation.fail_start(prompt, &name, error)?;
            return Ok(result_from_messages(
                &definition.name,
                &result.session_id,
                &result.messages,
            ));
        }
    };
    let progress_sink: EventEmitter = {
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
                    let _ = progress.send(pipi_protocol::tools::ToolOutput::text(format!(
                        "Subagent is calling tool {tool_name}"
                    )));
                }
                AgentEvent::MessageEnd { message } if message.role() == "assistant" => {
                    let turn = turns.fetch_add(1, Ordering::AcqRel) + 1;
                    let _ = progress.send(pipi_protocol::tools::ToolOutput::text(format!(
                        "Subagent completed turn {turn}"
                    )));
                }
                _ => {}
            }
        })
    };
    let request = RunRequest {
        prompt: prompt.into(),
        system_prompt,
        model: model.clone(),
        provider: provider_for(),
        tools,
        tool_context,
        options: run_stream_options(api_key, &model, &session_id, auto_protocol),
        retry: load_settings().retry.policy(),
        tool_execution: ToolExecutionMode::Parallel,
        budget: (model.context_window > 0).then(|| {
            pipi_context::compaction::Budget::from_window(
                model.context_window,
                definition.compact_threshold_percent(),
            )
        }),
        compaction: None,
        timeout: Some(RunTimeout {
            duration: timeout,
            message: format!(
                "Subtask run timed out ({} seconds), terminated",
                timeout.as_secs()
            ),
        }),
    };
    let outcome = reservation
        .prepare(
            request,
            make_session_hooks(progress_sink, definition.name.clone(), None),
        )?
        .run()
        .await?;
    Ok(result_from_messages(
        &definition.name,
        &outcome.session_id,
        &outcome.messages,
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::await_holding_lock)] // HOME_LOCK 必须覆盖整个异步测试
    use super::*;
    use crate::tools::agent::AgentRunStatus;
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
        crate::settings::save_settings(&crate::settings::Settings {
            notifications: Default::default(),
            theme: crate::settings::Theme::Dark,
            providers: vec![crate::settings::ProviderConfig {
                id: "silent".into(),
                name: "Silent".into(),
                api: pipi_protocol::Api::OpenAICompletions,
                api_mode: crate::settings::ApiMode::Auto,
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
            Some(pipi_protocol::permissions::PermissionsConfig {
                tools: vec!["read".into()],
                sandbox: pipi_protocol::permissions::SandboxMode::WorkspaceWrite,
            }),
            None,
            Some(pipi_protocol::Model {
                id: "silent-model".into(),
                name: "Silent Model".into(),
                api: pipi_protocol::Api::OpenAICompletions,
                base_url,
                max_tokens: 64,
                context_window: 4096,
                reasoning_effort: None,
            }),
        )
        .unwrap();

        let result = run_agent_once_with_sink(
            "timeout-worker",
            "hang",
            AbortSignal::new(),
            std::time::Duration::from_millis(300),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.status, AgentRunStatus::Failed);
        assert!(
            result.response.contains("timed out"),
            "should report timeout: {}",
            result.response
        );

        // 终态已落盘：read_agent 读到 Failed 而不是 pending
        let reread =
            crate::tools::agent::read_agent_output("timeout-worker", Some(&result.session_id))
                .unwrap();
        assert_eq!(reread.status, AgentRunStatus::Failed);

        let _ = std::fs::remove_dir_all(home);
    }
}
