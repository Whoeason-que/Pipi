//! Agent 组合工具。
//!
//! 当前 Agent 可以创建普通 Agent、在全新 session 中同步等待它，或把它提交给
//! 应用层的 session-owned 后台托管器；被运行的 Agent 不再获得这组三个工具，
//! 因此不会递归委派。工具协议保持在 core，生命周期由宿主注入。

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{AgentTool, ToolContext, ToolOutput};
use crate::agents;
use crate::permissions::{BACKGROUND_TASK_TOOLS, DEFAULT_TOOLS};
use crate::session::{active_path, list_session_summaries, load_session, rebuild_messages};
use crate::tools::background::{
    BackgroundAgentSessionSink, BackgroundAgentTaskSpec, BackgroundTaskOwner, BackgroundTaskService,
};
use crate::types::{
    AbortSignal, BackgroundTaskInfo, ContentBlock, Message, Model, StopReason, ToolResultContent,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRunStatus {
    Pending,
    Completed,
    Failed,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunResult {
    pub agent_name: String,
    pub session_id: String,
    pub status: AgentRunStatus,
    pub response: String,
}

/// 子运行进度通道：`run_agent` 执行期间，子 Agent 的里程碑事件经它回流到
/// 父会话的工具更新流（ToolExecutionUpdate），让父会话能看到 child 在做什么。
pub type ChildProgressTx = tokio::sync::mpsc::UnboundedSender<ToolOutput>;

#[async_trait]
pub trait AgentRunner: Send + Sync {
    async fn run_once(
        &self,
        agent_name: &str,
        prompt: &str,
        abort: AbortSignal,
        progress: Option<ChildProgressTx>,
    ) -> Result<AgentRunResult, String>;
}

pub struct CreateAgentTool {
    model: Model,
}

impl CreateAgentTool {
    pub fn new(model: Model) -> Self {
        Self { model }
    }
}

#[async_trait]
impl AgentTool for CreateAgentTool {
    fn name(&self) -> &'static str {
        "create_agent"
    }

    fn description(&self) -> String {
        "Create a persistent Pipi Agent. The new Agent inherits the current model, workspace, sandbox, and ordinary tools; instructions become its AGENTS.md. Agent-composition tools are never inherited."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Stable Agent name using only letters, numbers, '-' and '_'"
                },
                "description": {
                    "type": "string",
                    "description": "Short description of the Agent's purpose"
                },
                "instructions": {
                    "type": "string",
                    "description": "Complete operating instructions written to AGENTS.md"
                },
                "tools": {
                    "type": "array",
                    "description": "Optional subset of the current Agent's ordinary tools",
                    "items": { "type": "string", "enum": DEFAULT_TOOLS }
                }
            },
            "required": ["name", "instructions"],
            "additionalProperties": false
        })
    }

    fn requires_sequential(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let name = args["name"].as_str().ok_or("缺少 name")?.trim();
        let description = args
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let instructions = args["instructions"]
            .as_str()
            .ok_or("缺少 instructions")?
            .trim();
        if instructions.is_empty() {
            return Err("instructions 不能为空".into());
        }

        // 只允许继承基础工具。后台任务工具也是显式高权限能力，不能因
        // 默认继承或伪造工具参数而从父 Agent 传给新 Agent。
        let ordinary_tools: Vec<String> = ctx
            .permissions
            .tools
            .iter()
            .filter(|tool| DEFAULT_TOOLS.contains(&tool.as_str()))
            .cloned()
            .collect();
        let allowed: HashSet<&str> = ordinary_tools.iter().map(String::as_str).collect();
        let selected_tools = match args.get("tools") {
            None | Some(Value::Null) => ordinary_tools,
            Some(Value::Array(values)) => {
                let mut selected = Vec::with_capacity(values.len());
                let mut seen = HashSet::new();
                for value in values {
                    let tool = value.as_str().ok_or("tools 必须是字符串数组")?;
                    if !allowed.contains(tool) {
                        return Err(format!(
                            "只能把当前 Agent 已启用的基础工具授予新 Agent: {tool}"
                        ));
                    }
                    if seen.insert(tool) {
                        selected.push(tool.to_string());
                    }
                }
                selected
            }
            Some(_) => return Err("tools 必须是字符串数组".into()),
        };
        if selected_tools.is_empty() {
            return Err("新 Agent 至少需要一个普通工具".into());
        }

        let mut permissions = (*ctx.permissions).clone();
        permissions.tools = selected_tools;
        let workspace = ctx.workspace.to_string_lossy().into_owned();
        let definition = agents::create_agent_with_instructions(
            name,
            description,
            Some(&workspace),
            Some(permissions),
            Some(&self.model.id),
            Some(self.model.clone()),
            Some(instructions),
            true,
        )?;
        let details = serde_json::to_value(&definition).map_err(|error| error.to_string())?;

        Ok(ToolOutput {
            content: vec![ToolResultContent::Text {
                text: format!(
                    "Created Agent {} at ~/.pipi/agents/{}/. It is persistent and can now be passed to run_agent.",
                    definition.name, definition.name
                ),
            }],
            details: Some(details),
            terminate: false,
        })
    }
}

pub struct RunAgentTool {
    caller_agent: String,
    runner: Arc<dyn AgentRunner>,
    background_owner: Option<BackgroundTaskOwner>,
    background_service: Option<Arc<dyn BackgroundTaskService>>,
    session_sink: Option<BackgroundAgentSessionSink>,
    default_background: bool,
}

impl RunAgentTool {
    pub fn new(caller_agent: String, runner: Arc<dyn AgentRunner>) -> Self {
        Self {
            caller_agent,
            runner,
            background_owner: None,
            background_service: None,
            session_sink: None,
            default_background: false,
        }
    }

    /// 应用层注入托管服务。`default_background` 只影响省略 `background` 参数时
    /// 的默认值；显式 `background:false` 始终保留旧的同步等待语义。
    pub fn new_managed(
        caller_agent: String,
        owner: BackgroundTaskOwner,
        runner: Arc<dyn AgentRunner>,
        service: Arc<dyn BackgroundTaskService>,
        default_background: bool,
    ) -> Self {
        Self {
            caller_agent,
            runner,
            background_owner: Some(owner),
            background_service: Some(service),
            session_sink: None,
            default_background,
        }
    }

    /// 注入宿主的 child-session 生命周期通知。该回调同时用于同步 child 与
    /// 后台 child，避免两条 `run_agent` 路径的 UI 可见性语义分叉。
    pub fn with_session_sink(mut self, sink: BackgroundAgentSessionSink) -> Self {
        self.session_sink = Some(sink);
        self
    }
}

#[async_trait]
impl AgentTool for RunAgentTool {
    fn name(&self) -> &'static str {
        "run_agent"
    }

    fn description(&self) -> String {
        if self.default_background {
            "Submit an existing Pipi Agent as a managed background task in a fresh independent session. Return immediately with a job ID; use query_background_tasks to wait/read progress and manage_background_task to terminate it. Pass background:false only when the final output is needed before continuing."
                .into()
        } else {
            "Run an existing Pipi Agent in a fresh independent session. By default wait for completion and return its final output plus session ID; pass background:true to submit a managed task and return immediately."
                .into()
        }
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of an existing Agent"
                },
                "prompt": {
                    "type": "string",
                    "description": "Self-contained task and context for the Agent"
                },
                "background": {
                    "type": "boolean",
                    "description": "Submit and return immediately as a managed task. Defaults to the host's configured mode; false waits for the final output."
                }
            },
            "required": ["name", "prompt"],
            "additionalProperties": false
        })
    }

    fn requires_sequential(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &Value,
        on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let name = args["name"].as_str().ok_or("缺少 name")?.trim();
        let prompt = args["prompt"].as_str().ok_or("缺少 prompt")?.trim();
        if name == self.caller_agent {
            return Err("第一阶段不允许 Agent 运行自身".into());
        }
        if prompt.is_empty() {
            return Err("prompt 不能为空".into());
        }

        let background = args
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(self.default_background);
        if background {
            if !ctx.permissions.tool_enabled(BACKGROUND_TASK_TOOLS[1])
                || !ctx.permissions.tool_enabled(BACKGROUND_TASK_TOOLS[2])
            {
                return Err(
                    "后台 run_agent 需要同时启用 query_background_tasks 和 manage_background_task，避免提交后无法观察或终止 child".into(),
                );
            }
            let owner = self
                .background_owner
                .clone()
                .ok_or("当前宿主没有配置后台 Agent 托管服务")?;
            let service = self
                .background_service
                .clone()
                .ok_or("当前宿主没有配置后台 Agent 托管服务")?;
            let info = service
                .submit_agent(BackgroundAgentTaskSpec {
                    owner,
                    target_agent: name.to_string(),
                    prompt: prompt.to_string(),
                    session_sink: self.session_sink.clone(),
                })
                .await?;
            on_update(ToolOutput::text(format!(
                "Agent {name} 已提交为托管后台任务 {}。",
                info.job_id
            )));
            return background_agent_to_tool_output(&info);
        }

        on_update(ToolOutput::text(format!("Running Agent {name}…")));
        // 进度转发：子运行与转发循环同任务并发（select 双分支互不取消），
        // 运行结束后排空通道里剩余的里程碑再返回，避免丢最后几条。
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let runner = self.runner.clone();
        let abort = ctx.abort.clone();
        let run_future = async move {
            runner
                .run_once(name, prompt, abort, Some(progress_tx.clone()))
                .await
        };
        tokio::pin!(run_future);
        let outcome = loop {
            tokio::select! {
                result = &mut run_future => break result,
                // 通道关闭且运行未结束：继续等结果（进度就此静默）
                update = progress_rx.recv() => if let Some(update) = update {
                    on_update(update);
                },
            }
        };
        while let Ok(update) = progress_rx.try_recv() {
            on_update(update);
        }
        let result = outcome?;
        result_to_tool_output(&result)
    }
}

fn background_agent_to_tool_output(info: &BackgroundTaskInfo) -> Result<ToolOutput, String> {
    let mut text = format!(
        "Agent {} 已提交后台任务 {}，状态为 {:?}。使用 query_background_tasks 查询状态和输出。",
        info.target_agent.as_deref().unwrap_or("(unknown)"),
        info.job_id,
        info.status
    );
    if let Some(session_id) = &info.child_session_id {
        text.push_str(&format!(" Child session: {session_id}."));
    }
    Ok(ToolOutput {
        content: vec![ToolResultContent::Text { text }],
        details: Some(serde_json::to_value(info).map_err(|error| error.to_string())?),
        terminate: false,
    })
}

pub struct ReadAgentTool;

#[async_trait]
impl AgentTool for ReadAgentTool {
    fn name(&self) -> &'static str {
        "read_agent"
    }

    fn description(&self) -> String {
        "Read an Agent's latest persisted output. Pass sessionId to read a specific session; omit it to read the most recently active session."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of an existing Agent"
                },
                "sessionId": {
                    "type": "string",
                    "description": "Optional session ID returned by run_agent"
                }
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let name = args["name"].as_str().ok_or("缺少 name")?.trim();
        let session_id = args
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty());
        let result = read_agent_output(name, session_id)?;
        result_to_tool_output(&result)
    }
}

fn result_to_tool_output(result: &AgentRunResult) -> Result<ToolOutput, String> {
    let details = serde_json::to_value(result).map_err(|error| error.to_string())?;
    Ok(ToolOutput {
        content: vec![ToolResultContent::Text {
            text: format!(
                "Agent: {}\nSession: {}\nStatus: {:?}\n\n{}",
                result.agent_name, result.session_id, result.status, result.response
            ),
        }],
        details: Some(details),
        terminate: false,
    })
}

pub fn result_from_messages(
    agent_name: &str,
    session_id: &str,
    messages: &[Message],
) -> AgentRunResult {
    let Some(message) = messages
        .iter()
        .rev()
        .find(|message| matches!(message, Message::Assistant { .. }))
    else {
        return AgentRunResult {
            agent_name: agent_name.to_string(),
            session_id: session_id.to_string(),
            status: AgentRunStatus::Pending,
            response: "(Agent 还没有输出)".into(),
        };
    };

    let Message::Assistant {
        content,
        stop_reason,
        error_message,
        ..
    } = message
    else {
        unreachable!();
    };
    let text = content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let status = match stop_reason {
        StopReason::Error => AgentRunStatus::Failed,
        StopReason::Aborted => AgentRunStatus::Aborted,
        StopReason::Pending | StopReason::ToolUse => AgentRunStatus::Pending,
        StopReason::Stop | StopReason::Length => AgentRunStatus::Completed,
    };
    let response = if text.trim().is_empty() {
        error_message
            .clone()
            .unwrap_or_else(|| "(Agent 未返回文本输出)".into())
    } else {
        text
    };

    AgentRunResult {
        agent_name: agent_name.to_string(),
        session_id: session_id.to_string(),
        status,
        response,
    }
}

pub fn read_agent_output(
    agent_name: &str,
    session_id: Option<&str>,
) -> Result<AgentRunResult, String> {
    let definition = agents::load_agent(agent_name)?;
    let sessions_dir = definition
        .sessions_dir()
        .ok_or_else(|| "无法解析会话目录".to_string())?;
    if !agents::ensure_real_directory(&sessions_dir, "sessions 目录")? {
        return Err(format!("Agent「{agent_name}」还没有会话"));
    }

    let session_id = match session_id {
        Some(id) => {
            validate_session_id(id)?;
            id.to_string()
        }
        None => list_session_summaries(&sessions_dir)
            .into_iter()
            .next()
            .map(|summary| summary.id)
            .ok_or_else(|| format!("Agent「{agent_name}」还没有会话"))?,
    };
    let path = sessions_dir.join(format!("{session_id}.jsonl"));
    if !agents::ensure_real_file(&path, "会话文件")? {
        return Err(format!("Agent「{agent_name}」没有会话 {session_id}"));
    }
    let entries = load_session(&path)?;
    let active = active_path(&entries);
    let messages = rebuild_messages(&active);
    Ok(result_from_messages(agent_name, &session_id, &messages))
}

fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("非法会话 ID".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{now_millis, Usage};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[test]
    fn extracts_latest_assistant_output_and_status() {
        let messages = vec![
            Message::assistant_text("first", "model"),
            Message::Assistant {
                content: vec![
                    ContentBlock::Text {
                        text: "part 1".into(),
                    },
                    ContentBlock::Thinking {
                        thinking: "hidden".into(),
                        thinking_signature: None,
                    },
                    ContentBlock::Text {
                        text: "part 2".into(),
                    },
                ],
                api: "test".into(),
                provider: "test".into(),
                model: "model".into(),
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: now_millis(),
                duration_ms: None,
            },
        ];

        let result = result_from_messages("reviewer", "session-1", &messages);
        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.response, "part 1\npart 2");
    }

    #[test]
    fn reports_persisted_agent_errors() {
        let message = Message::assistant_error("provider failed", "model", StopReason::Error);
        let result = result_from_messages("reviewer", "session-1", &[message]);
        assert_eq!(result.status, AgentRunStatus::Failed);
        assert_eq!(result.response, "provider failed");
    }

    #[test]
    fn rejects_unsafe_session_ids() {
        assert!(validate_session_id("1730000000000-deadbeef").is_ok());
        assert!(validate_session_id("../escape").is_err());
        assert!(validate_session_id("nested/session").is_err());
    }

    struct StubRunner;

    #[async_trait]
    impl AgentRunner for StubRunner {
        async fn run_once(
            &self,
            agent_name: &str,
            _prompt: &str,
            _abort: AbortSignal,
            progress: Option<ChildProgressTx>,
        ) -> Result<AgentRunResult, String> {
            if let Some(progress) = &progress {
                let _ = progress.send(ToolOutput::text("子 Agent 正在调用工具 read"));
            }
            Ok(AgentRunResult {
                agent_name: agent_name.into(),
                session_id: "session-1".into(),
                status: AgentRunStatus::Completed,
                response: "worker output".into(),
            })
        }
    }

    #[tokio::test]
    async fn run_tool_forwards_child_progress_updates() {
        let workspace = std::env::temp_dir();
        let context = ToolContext {
            workspace,
            memory_dir: None,
            read_roots: Vec::new(),
            permissions: Arc::new(Default::default()),
            sandbox: crate::permissions::SandboxMode::DangerFullAccess,
            resolved_env: Arc::new(BTreeMap::new()),
            abort: AbortSignal::new(),
            approver: None,
        };
        let updates = Arc::new(Mutex::new(Vec::new()));
        let updates_for_cb = updates.clone();
        let tool = RunAgentTool::new("parent".into(), Arc::new(StubRunner));
        let output = tool
            .execute(
                &context,
                &json!({"name": "worker", "prompt": "do work"}),
                &move |update| {
                    if let ToolResultContent::Text { text } = &update.content[0] {
                        updates_for_cb.lock().unwrap().push(text.clone());
                    }
                },
            )
            .await
            .unwrap();

        assert!(matches!(
            &output.content[0],
            ToolResultContent::Text { text } if text.contains("worker output")
        ));
        assert_eq!(output.details.unwrap()["response"], "worker output");
        let updates = updates.lock().unwrap();
        assert!(
            updates.iter().any(|text| text.contains("read")),
            "子运行里程碑应回流到父工具更新流: {updates:?}"
        );
    }
}
