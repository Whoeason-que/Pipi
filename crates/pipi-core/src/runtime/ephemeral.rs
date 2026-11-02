//! 一次性子运行（`run_agent` 的 inline 模式）。
//!
//! 与 [`super::child`] 的区别是**不创建 Agent 定义**：子运行落在调用者
//! Agent 的会话库里（一条 child session），首条条目写 `SubagentRun` 标记。
//! 工作区、沙箱、env 与基础工具继承调用者；`instructions` 是这次运行的
//! 完整角色说明。这样「委派一次任务」不再沉淀成永久 Agent，SUBS 只留真正
//! 可复用的角色。
//!
//! 组合工具（create_agent / run_agent / read_agent）与后台托管工具一律不
//! 继承 —— 与 `create_agent` 创建的子 Agent 同一口径，子运行不能递归委派。

use super::*;
use crate::tools::agent::{EphemeralRunRequest, result_from_messages};
use pipi_protocol::permissions::DEFAULT_TOOLS;

/// 跑一次 inline 子运行并等待终态。`timeout` 供测试收紧；`progress` 把
/// 子运行的里程碑（工具调用、轮次完成）回流到父工具更新流。
pub(super) async fn run_ephemeral_with_sink(
    request: EphemeralRunRequest,
    abort: AbortSignal,
    timeout: std::time::Duration,
    progress: Option<ChildProgressTx>,
    session_sink: Option<BackgroundAgentSessionSink>,
) -> Result<AgentRunResult, String> {
    let prompt = request.prompt.trim();
    if prompt.is_empty() {
        return Err("Message cannot be empty".into());
    }
    if request.instructions.trim().is_empty() {
        return Err("instructions cannot be empty".into());
    }
    validate_session_id(&request.parent_session)?;

    let mut definition = agents::load_agent(&request.caller_agent)?;
    // 只保留基础工具：调用方可能在参数里塞了任意名字，这里按 Agent 定义
    // 再过滤一次（工具侧已经校验过，这里是载体侧的兜底）。
    let allowed: Vec<String> = definition
        .permissions
        .tools
        .iter()
        .filter(|tool| DEFAULT_TOOLS.contains(&tool.as_str()))
        .cloned()
        .collect();
    let selected: Vec<String> = request
        .tools
        .iter()
        .filter(|tool| allowed.contains(tool))
        .cloned()
        .collect();
    if selected.is_empty() {
        return Err("One-off subagent requires at least one ordinary tool".into());
    }
    definition.permissions.tools = selected;

    let repo = session_repo(&definition)?;
    let mut log = repo.create()?;
    log.append_subagent_run(&request.caller_agent, &request.parent_session)
        .map_err(|error| format!("Cannot mark one-off subagent session: {error}"))?;
    let engine = AgentSession::create(
        SessionStorage {
            log,
            repo: Some(repo),
        },
        Some(request.model.clone()),
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
        let prepared = agents::prepare_agent(&definition)?;
        let (tool_context, resolved_env) =
            agents::build_tool_context(&definition, &prepared, Some(session_id.clone()), abort)?;
        append_declared_env(&engine, &resolved_env)?;
        // 继承调用者本轮实际使用的模型（会话级覆盖优先于 Agent 默认）。
        let resolved = resolve_model(&definition, Some(&request.model), Some(&resolved_env.vars))?;
        let tools = Arc::new(pipi_tools::builtin_registry(&tool_context));
        let system_prompt = agents::build_subagent_system_prompt(
            &definition,
            &tools.wire_tools(),
            &request.instructions,
        );
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
        user_content: None,
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
    use pipi_protocol::permissions::PermissionsConfig;

    /// inline 子运行不落 Agent 定义：只在调用者的会话库里开一条带标记的
    /// child session；模型解析失败也要把终态写进这条会话（可读、可续跑）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn inline_run_marks_a_child_session_and_creates_no_agent() {
        let _guard = pipi_test_support::HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = pipi_test_support::temp_dir("pipi-ephemeral-home");
        let previous_home = std::env::var("HOME").unwrap_or_default();
        pipi_test_support::set_env_var("HOME", &home);

        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let caller = agents::create_agent(
            "caller",
            "",
            Some(workspace.to_str().unwrap()),
            Some(PermissionsConfig {
                tools: vec!["read".into(), "run_agent".into()],
                ..Default::default()
            }),
            None,
            None,
        )
        .unwrap();

        let model = Model {
            id: "caller-model".into(),
            name: "Caller Model".into(),
            api: pipi_protocol::Api::OpenAICompletions,
            base_url: "http://127.0.0.1:1/v1".into(),
            max_tokens: 4096,
            context_window: 128_000,
            reasoning_effort: None,
        };
        let result = run_ephemeral_with_sink(
            EphemeralRunRequest {
                caller_agent: "caller".into(),
                parent_session: "1730000000000-parent".into(),
                instructions: "You are a scout.".into(),
                prompt: "find the entry point".into(),
                tools: vec!["read".into()],
                model,
            },
            AbortSignal::new(),
            std::time::Duration::from_secs(5),
            None,
            None,
        )
        .await
        .unwrap();

        // 没有配置 provider：本轮以可读错误收尾，但会话与标记都已落盘。
        assert_eq!(result.status, AgentRunStatus::Failed);
        assert!(!result.response.trim().is_empty());
        assert_eq!(
            agents::list_agents().unwrap().len(),
            1,
            "inline 子运行不得创建新的 Agent 目录"
        );

        let repo = crate::runtime::agent_repo(&caller).unwrap();
        let summaries = repo.list();
        let child = summaries
            .iter()
            .find(|summary| summary.id == result.session_id)
            .expect("child session should be listed");
        assert_eq!(
            child.parent_session_id.as_deref(),
            Some("1730000000000-parent")
        );
        let entries = repo.entries(&result.session_id).unwrap();
        assert!(matches!(
            entries.first().map(|entry| &entry.kind),
            Some(pipi_protocol::session::EntryKind::SubagentRun { parent_agent, parent_session })
                if parent_agent == "caller" && parent_session == "1730000000000-parent"
        ));

        pipi_test_support::set_env_var("HOME", previous_home);
        let _ = std::fs::remove_dir_all(home);
    }
}
