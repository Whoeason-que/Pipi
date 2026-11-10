//! Agent 组合工具。
//!
//! 当前 Agent 可以创建普通 Agent、在全新 session 中同步等待它，或把它提交给
//! 应用层的 session-owned 后台托管器；被运行的 Agent 不再获得这组三个工具，
//! 因此不会递归委派。工具协议保持在 core，生命周期由宿主注入。

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::tools::background::{
    BackgroundAgentSessionSink, BackgroundAgentTaskSpec, BackgroundTaskOwner, BackgroundTaskService,
};
use pipi_protocol::permissions::{BACKGROUND_TASK_TOOLS, DEFAULT_TOOLS};
use pipi_protocol::session::{SessionRepo, active_path, rebuild_messages, validate_session_id};
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput};
use pipi_protocol::{
    AbortSignal, BackgroundTaskInfo, ContentBlock, Message, Model, StopReason, ToolResultContent,
};
use pipi_venv::agents;

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

/// 一次性子运行（`run_agent` inline 模式）的请求。
///
/// 与「已有 Agent 的一次运行」不同：不加载、也不创建 Agent 定义，而是在
/// 调用者 Agent 的会话库里开一条 child session。工作区 / 沙箱 / env 与基础
/// 工具继承调用者，`instructions` 就是这次运行的完整角色说明。
#[derive(Debug, Clone)]
pub struct EphemeralRunRequest {
    pub caller_agent: String,
    /// 父会话 id：写进 child session 的首条 `SubagentRun` 标记。
    pub parent_session: String,
    pub instructions: String,
    pub prompt: String,
    /// 调用者 Agent 已启用的基础工具子集（组合 / 后台托管工具不继承）。
    pub tools: Vec<String>,
    /// 调用者本轮实际使用的模型（会话级覆盖优先于 Agent 默认）。
    pub model: Model,
}

#[async_trait]
pub trait AgentRunner: Send + Sync {
    async fn run_once(
        &self,
        agent_name: &str,
        prompt: &str,
        abort: AbortSignal,
        progress: Option<ChildProgressTx>,
    ) -> Result<AgentRunResult, String>;

    /// 在目标 Agent 的已有会话上继续运行。宿主未实现时返回可读错误，
    /// 而不是静默新建会话。
    async fn run_in_session(
        &self,
        _agent_name: &str,
        _session_id: &str,
        _prompt: &str,
        _abort: AbortSignal,
        _progress: Option<ChildProgressTx>,
    ) -> Result<AgentRunResult, String> {
        Err("This host does not support resuming an existing session".into())
    }

    /// 跑一次一次性子运行（inline 模式）。宿主未实现时返回可读错误，
    /// 而不是静默退回「创建持久 Agent」的旧路径。
    async fn run_ephemeral(
        &self,
        _request: EphemeralRunRequest,
        _abort: AbortSignal,
        _progress: Option<ChildProgressTx>,
    ) -> Result<AgentRunResult, String> {
        Err("This host does not support one-off subagent runs".into())
    }
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
        "Create a persistent, reusable Pipi Agent that appears in the Subagent list. The new Agent inherits the current model, workspace, sandbox, and ordinary tools; instructions become its harness/AGENTS.md. Agent-composition tools are never inherited. Only create an Agent when the same role will be reused across sessions; for a one-off task call run_agent with instructions instead."
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
                    "description": "Complete operating instructions written to harness/AGENTS.md"
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
        let name = args["name"].as_str().ok_or("Missing name")?.trim();
        let description = args
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let instructions = args["instructions"]
            .as_str()
            .ok_or("Missing instructions")?
            .trim();
        if instructions.is_empty() {
            return Err("instructions cannot be empty".into());
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
                    let tool = value.as_str().ok_or("tools must be an array of strings")?;
                    if !allowed.contains(tool) {
                        return Err(format!(
                            "Only tools already enabled for current Agent can be granted to new Agent: {tool}"
                        ));
                    }
                    if seen.insert(tool) {
                        selected.push(tool.to_string());
                    }
                }
                selected
            }
            Some(_) => return Err("tools must be an array of strings".into()),
        };
        if selected_tools.is_empty() {
            return Err("New Agent requires at least one ordinary tool".into());
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
    /// 调用者当前会话：inline 模式用它写 child session 的父会话标记。
    caller_session: String,
    /// 调用者本轮实际使用的模型；inline 子运行继承它（含会话级覆盖）。
    caller_model: Option<Model>,
    runner: Arc<dyn AgentRunner>,
    background_owner: Option<BackgroundTaskOwner>,
    background_service: Option<Arc<dyn BackgroundTaskService>>,
    session_sink: Option<BackgroundAgentSessionSink>,
    default_background: bool,
}

impl RunAgentTool {
    #[cfg(test)]
    pub fn new(caller_agent: String, runner: Arc<dyn AgentRunner>) -> Self {
        Self {
            caller_agent,
            caller_session: String::new(),
            caller_model: None,
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
            caller_session: owner.session_id.clone(),
            caller_agent,
            caller_model: None,
            runner,
            background_owner: Some(owner),
            background_service: Some(service),
            session_sink: None,
            default_background,
        }
    }

    /// 注入调用者本轮实际使用的模型（inline 子运行继承它）。
    pub fn with_caller_model(mut self, model: Model) -> Self {
        self.caller_model = Some(model);
        self
    }

    /// 注入宿主的 child-session 生命周期通知。该回调同时用于同步 child 与
    /// 后台 child，避免两条 `run_agent` 路径的 UI 可见性语义分叉。
    pub fn with_session_sink(mut self, sink: BackgroundAgentSessionSink) -> Self {
        self.session_sink = Some(sink);
        self
    }

    /// inline 模式：在调用者自己的会话库里跑一次一次性子运行。
    async fn run_inline(
        &self,
        ctx: &ToolContext,
        args: &Value,
        prompt: &str,
        instructions: &str,
        on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        if args
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .is_some()
        {
            return Err(
                "One-off subagent runs cannot resume an existing session; pass name to run an existing Agent"
                    .into(),
            );
        }
        if args.get("background").and_then(Value::as_bool) == Some(true) {
            return Err("One-off subagent runs are foreground-only; omit background".into());
        }
        if self.caller_session.is_empty() {
            return Err(
                "Current host cannot run a one-off subagent (missing caller session)".into(),
            );
        }
        let model = self
            .caller_model
            .clone()
            .ok_or("Current host cannot run a one-off subagent (missing caller model)")?;

        // 只允许继承基础工具。组合 / 后台托管工具是显式高权限能力，
        // 不能因为继承或伪造参数泄漏到下一层（与 create_agent 同一口径）。
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
                    let tool = value.as_str().ok_or("tools must be an array of strings")?;
                    if !allowed.contains(tool) {
                        return Err(format!(
                            "Only tools already enabled for current Agent can be granted to a one-off subagent: {tool}"
                        ));
                    }
                    if seen.insert(tool) {
                        selected.push(tool.to_string());
                    }
                }
                selected
            }
            Some(_) => return Err("tools must be an array of strings".into()),
        };
        if selected_tools.is_empty() {
            return Err("One-off subagent requires at least one ordinary tool".into());
        }

        on_update(ToolOutput::text("Running one-off subagent…"));
        let (progress_tx, progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let runner = self.runner.clone();
        let abort = ctx.abort.clone();
        let request = EphemeralRunRequest {
            caller_agent: self.caller_agent.clone(),
            parent_session: self.caller_session.clone(),
            instructions: instructions.to_string(),
            prompt: prompt.to_string(),
            tools: selected_tools,
            model,
        };
        let result = forward_child_run(
            runner.run_ephemeral(request, abort, Some(progress_tx)),
            progress_rx,
            on_update,
        )
        .await?;
        result_to_tool_output(&result)
    }
}

#[async_trait]
impl AgentTool for RunAgentTool {
    fn name(&self) -> &'static str {
        "run_agent"
    }

    fn description(&self) -> String {
        let inline = "Pass instructions instead of name to run a one-off subagent in a child session of the current Agent: no persistent Agent is created, and it inherits the current workspace, sandbox, and ordinary tools (foreground only). Prefer this for one-off delegation; use create_agent only when the same role will be reused.";
        if self.default_background {
            format!(
                "Submit an existing Pipi Agent as a managed background task in a fresh independent session, or resume a specific session in the foreground by passing sessionId. Return immediately with a job ID for background runs. By default the session is woken with the result: end your turn instead of polling. Use query_background_tasks for intermediate progress and manage_background_task to terminate it. Pass background:false only when the final output is needed before continuing. {inline}"
            )
        } else {
            format!(
                "Run an existing Pipi Agent in a fresh independent session, or resume a specific session by passing sessionId. By default wait for completion and return its final output plus session ID; pass background:true to submit a managed task and return immediately. Background runs cannot resume an existing session. {inline}"
            )
        }
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of an existing Agent to run. Omit when passing instructions for a one-off subagent."
                },
                "instructions": {
                    "type": "string",
                    "description": "One-off subagent only: self-contained operating instructions. The run happens in a child session of the current Agent and leaves no Agent definition behind. Foreground only; cannot be combined with name or sessionId."
                },
                "prompt": {
                    "type": "string",
                    "description": "Self-contained task and context for the Agent"
                },
                "tools": {
                    "type": "array",
                    "description": "One-off subagent only: subset of the current Agent's ordinary tools (defaults to all of them)",
                    "items": { "type": "string", "enum": DEFAULT_TOOLS }
                },
                "sessionId": {
                    "type": "string",
                    "description": "Optional existing session ID to resume. Preserves history, model selection, and stats. Only supported for foreground runs of an existing Agent."
                },
                "background": {
                    "type": "boolean",
                    "description": "Submit and return immediately as a managed task. Defaults to the host's configured mode; false waits for the final output. Forced to foreground when sessionId is provided, and never available for a one-off subagent."
                },
                "subscribe": {
                    "type": "boolean",
                    "description": "Wake this session with the child Agent's result when the background task finishes (default true)."
                }
            },
            "required": ["prompt"],
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
        let prompt = args["prompt"].as_str().ok_or("Missing prompt")?.trim();
        if prompt.is_empty() {
            return Err("prompt cannot be empty".into());
        }
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let instructions = args
            .get("instructions")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty());
        let name = match (name, instructions) {
            (Some(_), Some(_)) => {
                return Err(
                    "Pass either name (existing Agent) or instructions (one-off subagent), not both"
                        .into(),
                );
            }
            (None, None) => return Err("Missing name or instructions".into()),
            (Some(name), None) => name,
            (None, Some(instructions)) => {
                return self
                    .run_inline(ctx, args, prompt, instructions, on_update)
                    .await;
            }
        };
        if name == self.caller_agent {
            return Err("Agent is not allowed to run itself".into());
        }

        let background = args
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(self.default_background);
        let session_id = args
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty());
        if let Some(id) = session_id {
            validate_session_id(id)?;
        }
        if session_id.is_some() && args.get("background").and_then(Value::as_bool) == Some(true) {
            return Err(
                "Background run_agent cannot resume an existing session; pass background:false or omit background"
                    .into(),
            );
        }
        // 省略 background 时，恢复已有会话始终走前台等待（后台恢复尚未实现）。
        let background = background && session_id.is_none();
        if background {
            if !ctx.permissions.tool_enabled(BACKGROUND_TASK_TOOLS[1])
                || !ctx.permissions.tool_enabled(BACKGROUND_TASK_TOOLS[2])
            {
                return Err(
                    "Background run_agent requires both query_background_tasks and manage_background_task enabled to observe or terminate the child".into(),
                );
            }
            let owner = self
                .background_owner
                .clone()
                .ok_or("Current host does not configure background Agent service")?;
            let service = self
                .background_service
                .clone()
                .ok_or("Current host does not configure background Agent service")?;
            let info = service
                .submit_agent(BackgroundAgentTaskSpec {
                    owner,
                    target_agent: name.to_string(),
                    prompt: prompt.to_string(),
                    session_sink: self.session_sink.clone(),
                    subscribe: args
                        .get("subscribe")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                })
                .await?;
            on_update(ToolOutput::text(format!(
                "Agent {name} submitted as managed background task {}.",
                info.job_id
            )));
            return background_agent_to_tool_output(&info);
        }

        on_update(ToolOutput::text(if let Some(id) = session_id {
            format!("Resuming Agent {name} session {id}…")
        } else {
            format!("Running Agent {name}…")
        }));
        // 进度转发：子运行与转发循环同任务并发（select 双分支互不取消），
        // 运行结束后排空通道里剩余的里程碑再返回，避免丢最后几条。
        let (progress_tx, progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let runner = self.runner.clone();
        let abort = ctx.abort.clone();
        let outcome = match session_id {
            Some(session_id) => {
                forward_child_run(
                    runner.run_in_session(name, session_id, prompt, abort, Some(progress_tx)),
                    progress_rx,
                    on_update,
                )
                .await
            }
            None => {
                forward_child_run(
                    runner.run_once(name, prompt, abort, Some(progress_tx)),
                    progress_rx,
                    on_update,
                )
                .await
            }
        };
        let result = outcome?;
        result_to_tool_output(&result)
    }
}

/// 转发子运行的里程碑更新，并等待子运行结束。
async fn forward_child_run<F>(
    future: F,
    mut progress_rx: tokio::sync::mpsc::UnboundedReceiver<ToolOutput>,
    on_update: &(dyn Fn(ToolOutput) + Send + Sync),
) -> Result<AgentRunResult, String>
where
    F: std::future::Future<Output = Result<AgentRunResult, String>>,
{
    tokio::pin!(future);
    let outcome = loop {
        tokio::select! {
            result = &mut future => break result,
            // 通道关闭且运行未结束：继续等结果（进度就此静默）
            update = progress_rx.recv() => if let Some(update) = update {
                on_update(update);
            },
        }
    };
    while let Ok(update) = progress_rx.try_recv() {
        on_update(update);
    }
    outcome
}

fn background_agent_to_tool_output(info: &BackgroundTaskInfo) -> Result<ToolOutput, String> {
    let mut text = format!(
        "Agent {} submitted background task {}, status is {:?}. {}",
        info.target_agent.as_deref().unwrap_or("(unknown)"),
        info.job_id,
        info.status,
        if info.subscribe {
            "You will be woken with the result when it finishes; end your turn instead of polling."
        } else {
            "Subscription is off: query_background_tasks can retrieve the result."
        }
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
        let name = args["name"].as_str().ok_or("Missing name")?.trim();
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

pub(crate) fn result_from_messages(
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
            response: "(Agent has no output yet)".into(),
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
            .unwrap_or_else(|| "(Agent returned no text output)".into())
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
    // 载体装配点与运行时同源（git 分支），不要在这里另起一份
    let repo = crate::runtime::agent_repo(&definition)?;

    let session_id = match session_id {
        Some(id) => {
            validate_session_id(id)?;
            id.to_string()
        }
        None => repo
            .list()
            .into_iter()
            .next()
            .map(|summary| summary.id)
            .ok_or_else(|| format!("Agent \"{agent_name}\" has no sessions yet"))?,
    };
    if !repo.exists(&session_id)? {
        return Err(format!(
            "Agent \"{agent_name}\" has no session {session_id}"
        ));
    }
    let entries = repo.entries(&session_id)?;
    let active = active_path(&entries);
    let messages = rebuild_messages(&active);
    Ok(result_from_messages(agent_name, &session_id, &messages))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::{Usage, now_millis};
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
                error_kind: None,
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
            notebook_path: None,
            read_roots: Vec::new(),
            permissions: Arc::new(Default::default()),
            sandbox: pipi_protocol::permissions::SandboxMode::DangerFullAccess,
            sandbox_policy_override: None,
            resolved_env: Arc::new(BTreeMap::new()),
            abort: AbortSignal::new(),
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

    struct ResumingRunner {
        seen: Mutex<Option<String>>,
    }

    #[async_trait]
    impl AgentRunner for ResumingRunner {
        async fn run_once(
            &self,
            _agent_name: &str,
            _prompt: &str,
            _abort: AbortSignal,
            _progress: Option<ChildProgressTx>,
        ) -> Result<AgentRunResult, String> {
            Err("run_once must not be called when sessionId is present".into())
        }

        async fn run_in_session(
            &self,
            agent_name: &str,
            session_id: &str,
            _prompt: &str,
            _abort: AbortSignal,
            progress: Option<ChildProgressTx>,
        ) -> Result<AgentRunResult, String> {
            *self.seen.lock().unwrap() = Some(session_id.to_string());
            if let Some(progress) = &progress {
                let _ = progress.send(ToolOutput::text("resumed milestone"));
            }
            Ok(AgentRunResult {
                agent_name: agent_name.into(),
                session_id: session_id.into(),
                status: AgentRunStatus::Completed,
                response: "resumed output".into(),
            })
        }
    }

    #[tokio::test]
    async fn run_tool_resumes_the_requested_session() {
        let context = ToolContext {
            workspace: std::env::temp_dir(),
            memory_dir: None,
            notebook_path: None,
            read_roots: Vec::new(),
            permissions: Arc::new(Default::default()),
            sandbox: pipi_protocol::permissions::SandboxMode::DangerFullAccess,
            sandbox_policy_override: None,
            resolved_env: Arc::new(BTreeMap::new()),
            abort: AbortSignal::new(),
        };
        let runner = Arc::new(ResumingRunner {
            seen: Mutex::new(None),
        });
        let tool = RunAgentTool::new("parent".into(), runner.clone());
        let output = tool
            .execute(
                &context,
                &json!({
                    "name": "worker",
                    "prompt": "continue",
                    "sessionId": "1730000000000-deadbeef"
                }),
                &|_| {},
            )
            .await
            .unwrap();

        assert_eq!(
            runner.seen.lock().unwrap().as_deref(),
            Some("1730000000000-deadbeef")
        );
        assert_eq!(
            output.details.as_ref().unwrap()["sessionId"],
            "1730000000000-deadbeef"
        );
        assert!(matches!(
            &output.content[0],
            ToolResultContent::Text { text } if text.contains("resumed output")
        ));
    }

    #[tokio::test]
    async fn run_tool_rejects_background_session_resume_and_unsafe_ids() {
        let context = ToolContext {
            workspace: std::env::temp_dir(),
            memory_dir: None,
            notebook_path: None,
            read_roots: Vec::new(),
            permissions: Arc::new(Default::default()),
            sandbox: pipi_protocol::permissions::SandboxMode::DangerFullAccess,
            sandbox_policy_override: None,
            resolved_env: Arc::new(BTreeMap::new()),
            abort: AbortSignal::new(),
        };
        let runner = Arc::new(ResumingRunner {
            seen: Mutex::new(None),
        });
        let tool = RunAgentTool::new("parent".into(), runner.clone());
        let error = tool
            .execute(
                &context,
                &json!({
                    "name": "worker",
                    "prompt": "continue",
                    "sessionId": "1730000000000-deadbeef",
                    "background": true
                }),
                &|_| {},
            )
            .await
            .expect_err("后台运行不得静默恢复会话");
        assert!(error.contains("Background run_agent cannot resume"));
        assert!(runner.seen.lock().unwrap().is_none());

        let error = tool
            .execute(
                &context,
                &json!({
                    "name": "worker",
                    "prompt": "continue",
                    "sessionId": "../escape"
                }),
                &|_| {},
            )
            .await
            .expect_err("非法会话 id 必须拒绝");
        assert!(!error.is_empty());
        assert!(runner.seen.lock().unwrap().is_none());
    }

    struct UnusedBackgroundService;

    #[async_trait]
    impl BackgroundTaskService for UnusedBackgroundService {
        async fn submit(
            &self,
            _: crate::tools::background::BackgroundTaskSpec,
        ) -> Result<pipi_protocol::BackgroundTaskInfo, String> {
            Err("background service must not be used for session resume".into())
        }

        async fn query(
            &self,
            _: &BackgroundTaskOwner,
            _: crate::tools::background::BackgroundTaskQuery,
        ) -> Result<Vec<pipi_protocol::BackgroundTaskSnapshot>, String> {
            Ok(Vec::new())
        }

        async fn manage(
            &self,
            _: &BackgroundTaskOwner,
            _: &str,
            _: crate::tools::background::BackgroundTaskCommand,
        ) -> Result<pipi_protocol::BackgroundTaskSnapshot, String> {
            Err("background service must not be used for session resume".into())
        }

        fn active_count(&self, _: &BackgroundTaskOwner) -> usize {
            0
        }
    }

    #[tokio::test]
    async fn run_tool_forces_foreground_when_default_background_but_session_is_provided() {
        let context = ToolContext {
            workspace: std::env::temp_dir(),
            memory_dir: None,
            notebook_path: None,
            read_roots: Vec::new(),
            permissions: Arc::new(Default::default()),
            sandbox: pipi_protocol::permissions::SandboxMode::DangerFullAccess,
            sandbox_policy_override: None,
            resolved_env: Arc::new(BTreeMap::new()),
            abort: AbortSignal::new(),
        };
        let runner = Arc::new(ResumingRunner {
            seen: Mutex::new(None),
        });
        let tool = RunAgentTool::new_managed(
            "parent".into(),
            BackgroundTaskOwner::new("parent", "session", 1),
            runner.clone(),
            Arc::new(UnusedBackgroundService),
            true,
        );
        tool.execute(
            &context,
            &json!({
                "name": "worker",
                "prompt": "continue",
                "sessionId": "1730000000000-deadbeef"
            }),
            &|_| {},
        )
        .await
        .expect("省略 background 时必须为已有会话走前台");
        assert_eq!(
            runner.seen.lock().unwrap().as_deref(),
            Some("1730000000000-deadbeef")
        );
    }

    struct EphemeralRunner {
        seen: Mutex<Option<EphemeralRunRequest>>,
    }

    #[async_trait]
    impl AgentRunner for EphemeralRunner {
        async fn run_once(
            &self,
            _agent_name: &str,
            _prompt: &str,
            _abort: AbortSignal,
            _progress: Option<ChildProgressTx>,
        ) -> Result<AgentRunResult, String> {
            Err("run_once must not be called for an inline subagent".into())
        }

        async fn run_ephemeral(
            &self,
            request: EphemeralRunRequest,
            _abort: AbortSignal,
            progress: Option<ChildProgressTx>,
        ) -> Result<AgentRunResult, String> {
            *self.seen.lock().unwrap() = Some(request);
            if let Some(progress) = &progress {
                let _ = progress.send(ToolOutput::text("inline milestone"));
            }
            Ok(AgentRunResult {
                agent_name: "parent".into(),
                session_id: "child-session".into(),
                status: AgentRunStatus::Completed,
                response: "inline output".into(),
            })
        }
    }

    fn inline_context(tools: &[&str]) -> ToolContext {
        ToolContext {
            workspace: std::env::temp_dir(),
            memory_dir: None,
            notebook_path: None,
            read_roots: Vec::new(),
            permissions: Arc::new(pipi_protocol::permissions::PermissionsConfig {
                tools: tools.iter().map(|tool| tool.to_string()).collect(),
                ..Default::default()
            }),
            sandbox: pipi_protocol::permissions::SandboxMode::DangerFullAccess,
            sandbox_policy_override: None,
            resolved_env: Arc::new(BTreeMap::new()),
            abort: AbortSignal::new(),
        }
    }

    fn inline_model() -> Model {
        Model {
            id: "caller-model".into(),
            name: "Caller Model".into(),
            api: pipi_protocol::Api::OpenAICompletions,
            base_url: "http://127.0.0.1:1/v1".into(),
            max_tokens: 4096,
            context_window: 128_000,
            reasoning_effort: None,
        }
    }

    fn inline_tool(runner: Arc<EphemeralRunner>) -> RunAgentTool {
        let mut tool = RunAgentTool::new("parent".into(), runner);
        tool.caller_session = "parent-session".into();
        tool.caller_model = Some(inline_model());
        tool
    }

    #[tokio::test]
    async fn run_tool_inline_mode_delegates_without_creating_an_agent() {
        let context = inline_context(&["read", "bash", "run_agent"]);
        let runner = Arc::new(EphemeralRunner {
            seen: Mutex::new(None),
        });
        let updates = Arc::new(Mutex::new(Vec::new()));
        let updates_for_cb = updates.clone();
        let output = inline_tool(runner.clone())
            .execute(
                &context,
                &json!({
                    "instructions": "You are a scout.",
                    "prompt": "find the entry point",
                    "tools": ["read"]
                }),
                &move |update| {
                    if let ToolResultContent::Text { text } = &update.content[0] {
                        updates_for_cb.lock().unwrap().push(text.clone());
                    }
                },
            )
            .await
            .unwrap();

        let seen = runner.seen.lock().unwrap().clone().expect("inline request");
        assert_eq!(seen.caller_agent, "parent");
        assert_eq!(seen.parent_session, "parent-session");
        assert_eq!(seen.instructions, "You are a scout.");
        assert_eq!(seen.prompt, "find the entry point");
        // 组合工具不继承，显式子集只保留 read
        assert_eq!(seen.tools, vec!["read"]);
        assert_eq!(seen.model.id, "caller-model");
        assert_eq!(
            output.details.as_ref().unwrap()["response"],
            "inline output"
        );
        assert!(
            updates
                .lock()
                .unwrap()
                .iter()
                .any(|text| text.contains("inline milestone")),
            "inline 进度应回流到父工具更新流"
        );
    }

    #[tokio::test]
    async fn run_tool_inline_mode_rejects_conflicting_or_unsafe_arguments() {
        let context = inline_context(&["read", "run_agent"]);
        let runner = Arc::new(EphemeralRunner {
            seen: Mutex::new(None),
        });
        let tool = inline_tool(runner.clone());

        for (args, expected) in [
            (
                json!({"name": "worker", "instructions": "scout", "prompt": "go"}),
                "not both",
            ),
            (
                json!({"instructions": "scout", "prompt": "go", "background": true}),
                "foreground-only",
            ),
            (
                json!({"instructions": "scout", "prompt": "go", "sessionId": "1730000000000-deadbeef"}),
                "cannot resume",
            ),
            (
                json!({"instructions": "scout", "prompt": "go", "tools": ["run_agent"]}),
                "Only tools already enabled",
            ),
        ] {
            let error = tool.execute(&context, &args, &|_| {}).await.unwrap_err();
            assert!(error.contains(expected), "{args}: {error}");
        }
        assert!(runner.seen.lock().unwrap().is_none());
    }
}
