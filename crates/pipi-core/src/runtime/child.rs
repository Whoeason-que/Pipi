//! 子运行的能力装配、进度适配与结果转换。
use super::*;

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
        let (tool_context, resolved_env) =
            agents::build_tool_context(&definition, Some(session_id.clone()), abort)?;
        append_declared_env(&engine, &resolved_env)?;
        let resolved = resolve_model(&definition, None, Some(&resolved_env.vars))?;
        let tools = Arc::new(pipi_tools::builtin_registry(&tool_context));
        let system_prompt =
            agents::build_system_prompt_with_tools(&definition, &tools.wire_tools())?;
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
                        "子 Agent 正在调用工具 {tool_name}"
                    )));
                }
                AgentEvent::MessageEnd { message } if message.role() == "assistant" => {
                    let turn = turns.fetch_add(1, Ordering::AcqRel) + 1;
                    let _ = progress.send(pipi_protocol::tools::ToolOutput::text(format!(
                        "子 Agent 完成第 {turn} 轮回复"
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
            message: format!("子任务运行超时（{} 秒），已终止", timeout.as_secs()),
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
                bash: Default::default(),
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
            crate::tools::agent::read_agent_output("timeout-worker", Some(&result.session_id))
                .unwrap();
        assert_eq!(reread.status, AgentRunStatus::Failed);

        let _ = std::fs::remove_dir_all(home);
    }
}
