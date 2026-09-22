//! Agent 可调用的后台任务托管工具。
//!
//! 工具只定义能力边界和协议，不持有进程、不知道 Agent 目录如何落盘。具体的
//! 生命周期、进程组和重启恢复由 `pipi-app` 注入 [`BackgroundTaskService`]。
//! 这样 core 可以在 Tauri 与 Web 两个宿主中复用同一套工具语义。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{AgentTool, ToolContext, ToolOutput};
use crate::types::{BackgroundTaskInfo, BackgroundTaskSnapshot, ToolResultContent};

const MAX_COMMAND_BYTES: usize = 16 * 1024;
const MAX_WAIT_MS: u64 = 60_000;
const DEFAULT_LIST_LIMIT: usize = 20;
const MAX_LIST_LIMIT: usize = 100;

/// 工具提交任务时携带的稳定所有权身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundTaskOwner {
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
}

impl BackgroundTaskOwner {
    pub fn new(
        agent_name: impl Into<String>,
        session_id: impl Into<String>,
        run_id: usize,
    ) -> Self {
        Self {
            agent_name: agent_name.into(),
            session_id: session_id.into(),
            run_id,
        }
    }
}

/// 应用层启动一个后台进程所需的最小输入。环境变量在当前会话启动时已经
/// 由 av 解析完成，不能在真正 spawn 时重新读取宿主进程环境。
#[derive(Debug, Clone)]
pub struct BackgroundTaskSpec {
    pub owner: BackgroundTaskOwner,
    pub command: String,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
}

/// 应用层启动一个 child Agent 所需的输入。child 自己拥有独立 session，任务的
/// 所有权仍归提交它的父 Agent session，因此父轮结束后仍可查询/终止。
///
/// `session_sink` 是宿主可选的生命周期通知回调。它只报告 child session 已经
/// 创建，具体如何广播由应用层决定；这样后台任务不会因为父轮已经结束而失去
/// UI 刷新机会。
pub type BackgroundAgentSessionSink = Arc<dyn Fn(String, String, usize) + Send + Sync>;

#[derive(Clone)]
pub struct BackgroundAgentTaskSpec {
    pub owner: BackgroundTaskOwner,
    pub target_agent: String,
    pub prompt: String,
    pub session_sink: Option<BackgroundAgentSessionSink>,
}

#[derive(Debug, Clone, Default)]
pub struct BackgroundTaskQuery {
    pub job_id: Option<String>,
    pub after_seq: u64,
    pub wait_ms: u64,
    pub include_completed: bool,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub enum BackgroundTaskCommand {
    Terminate,
    WriteStdin(String),
}

/// 后台任务的应用层能力端口。实现必须再次校验 owner，不能把一个 session
/// 拿到另一个 session 的任务或控制权。
#[async_trait]
pub trait BackgroundTaskService: Send + Sync {
    async fn submit(&self, spec: BackgroundTaskSpec) -> Result<BackgroundTaskInfo, String>;

    /// 提交托管 child Agent。默认实现让只有 shell 能力的宿主显式失败；应用层
    /// 若支持 Agent 后台任务则覆盖它。二者共用 query/manage/active_count。
    async fn submit_agent(
        &self,
        _spec: BackgroundAgentTaskSpec,
    ) -> Result<BackgroundTaskInfo, String> {
        Err("当前宿主不支持后台 Agent 任务".into())
    }

    async fn query(
        &self,
        owner: &BackgroundTaskOwner,
        query: BackgroundTaskQuery,
    ) -> Result<Vec<BackgroundTaskSnapshot>, String>;

    async fn manage(
        &self,
        owner: &BackgroundTaskOwner,
        job_id: &str,
        command: BackgroundTaskCommand,
    ) -> Result<BackgroundTaskSnapshot, String>;

    /// 同步快照，用于会话槽的占用检查；只统计尚未进入终态的任务。
    fn active_count(&self, owner: &BackgroundTaskOwner) -> usize;
}

/// 提交后台任务：明确使用托管 API，而不是让模型在 bash 中自行 `&` / `nohup`。
pub struct SubmitBackgroundTaskTool {
    service: Arc<dyn BackgroundTaskService>,
    owner: BackgroundTaskOwner,
}

impl SubmitBackgroundTaskTool {
    pub fn new(service: Arc<dyn BackgroundTaskService>, owner: BackgroundTaskOwner) -> Self {
        Self { service, owner }
    }
}

#[async_trait]
impl AgentTool for SubmitBackgroundTaskTool {
    fn name(&self) -> &'static str {
        "submit_background_task"
    }

    fn description(&self) -> String {
        "Submit a managed background bash task owned by the current Agent session. The task survives the current model turn, has bounded live output, and can later be queried or terminated. Do not use shell '&', nohup, disown, or setsid; use this tool instead.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Command to run under bash. It must be a foreground command; Pipi owns the background lifecycle."
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional directory relative to the current workspace; it must remain inside the workspace."
                }
            },
            "required": ["command"]
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
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .ok_or("缺少 command")?
            .trim();
        validate_managed_command(command)?;

        match ctx.permissions.assess_bash_classified(command, &ctx.workspace) {
            crate::permissions::BashAssessment::Allowed => {}
            crate::permissions::BashAssessment::HardDenied(message) => return Err(message),
            crate::permissions::BashAssessment::NeedsApproval { .. } => match &ctx.approver {
                Some(approver) => approver.approve(command).await?,
                None => {
                    return Err(
                        "后台命令不在白名单中，且当前会话没有可用的交互审批通道（请把命令加入 bash 白名单）"
                            .into(),
                    )
                }
            },
        }

        let cwd = resolve_workspace_cwd(ctx, args.get("cwd").and_then(Value::as_str))?;
        let info = self
            .service
            .submit(BackgroundTaskSpec {
                owner: self.owner.clone(),
                command: command.to_string(),
                cwd,
                env: (*ctx.resolved_env).clone(),
            })
            .await?;

        Ok(ToolOutput {
            content: vec![ToolResultContent::Text {
                text: format!(
                    "后台任务已提交：{}，状态为 {:?}。使用 query_background_tasks 查询输出或状态。",
                    info.job_id, info.status
                ),
            }],
            details: Some(json!(info)),
            terminate: false,
        })
    }
}

/// 查询当前 session 自己提交的后台任务。省略 jobId 时返回任务列表；提供
/// waitMs 时由应用层异步等待状态/输出变化，不需要模型自行 sleep 轮询。
pub struct QueryBackgroundTasksTool {
    service: Arc<dyn BackgroundTaskService>,
    owner: BackgroundTaskOwner,
}

impl QueryBackgroundTasksTool {
    pub fn new(service: Arc<dyn BackgroundTaskService>, owner: BackgroundTaskOwner) -> Self {
        Self { service, owner }
    }
}

#[async_trait]
impl AgentTool for QueryBackgroundTasksTool {
    fn name(&self) -> &'static str {
        "query_background_tasks"
    }

    fn description(&self) -> String {
        "Query managed background tasks owned by the current Agent session. Pass jobId for one task, afterSeq to fetch only new output, and waitMs (up to 60000) to wait server-side for progress or completion instead of sleeping in bash.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "jobId": { "type": "string", "description": "Optional task id; omit to list this session's tasks." },
                "afterSeq": { "type": "integer", "minimum": 0, "description": "Return output chunks after this cursor." },
                "waitMs": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT_MS, "description": "Server-side wait timeout; zero means return immediately." },
                "includeCompleted": { "type": "boolean", "description": "Include completed historical tasks when listing; defaults to true." },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIST_LIMIT, "description": "Maximum tasks when listing; defaults to 20." }
            }
        })
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let wait_ms = parse_u64(args, "waitMs")?.unwrap_or(0);
        if wait_ms > MAX_WAIT_MS {
            return Err(format!("waitMs 不能超过 {MAX_WAIT_MS}"));
        }
        let limit = parse_usize(args, "limit")?.unwrap_or(DEFAULT_LIST_LIMIT);
        if !(1..=MAX_LIST_LIMIT).contains(&limit) {
            return Err(format!("limit 必须在 1 到 {MAX_LIST_LIMIT} 之间"));
        }
        let snapshots = self
            .service
            .query(
                &self.owner,
                BackgroundTaskQuery {
                    job_id: optional_string(args, "jobId"),
                    after_seq: parse_u64(args, "afterSeq")?.unwrap_or(0),
                    wait_ms,
                    include_completed: args
                        .get("includeCompleted")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                    limit,
                },
            )
            .await?;

        let text = format_query_result(&snapshots);
        Ok(ToolOutput {
            content: vec![ToolResultContent::Text { text }],
            details: Some(json!(snapshots)),
            terminate: false,
        })
    }
}

/// 管理当前 session 自己的任务：优雅终止（应用层负责超时升级）或写入 stdin。
pub struct ManageBackgroundTaskTool {
    service: Arc<dyn BackgroundTaskService>,
    owner: BackgroundTaskOwner,
}

impl ManageBackgroundTaskTool {
    pub fn new(service: Arc<dyn BackgroundTaskService>, owner: BackgroundTaskOwner) -> Self {
        Self { service, owner }
    }
}

#[async_trait]
impl AgentTool for ManageBackgroundTaskTool {
    fn name(&self) -> &'static str {
        "manage_background_task"
    }

    fn description(&self) -> String {
        "Manage one task owned by the current Agent session. Use action=terminate to stop the whole managed process group, or action=writeStdin with data for an interactive command.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "jobId": { "type": "string" },
                "action": { "type": "string", "enum": ["terminate", "writeStdin"] },
                "data": { "type": "string", "description": "Bytes/text to write to stdin for writeStdin." }
            },
            "required": ["jobId", "action"]
        })
    }

    fn requires_sequential(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: &Value,
        _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let job_id = args
            .get("jobId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or("缺少 jobId")?;
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .ok_or("缺少 action")?;
        let command = match action {
            "terminate" => BackgroundTaskCommand::Terminate,
            "writeStdin" => BackgroundTaskCommand::WriteStdin(
                args.get("data")
                    .and_then(Value::as_str)
                    .ok_or("writeStdin 需要 data")?
                    .to_string(),
            ),
            _ => return Err("action 必须是 terminate 或 writeStdin".into()),
        };
        let snapshot = self.service.manage(&self.owner, job_id, command).await?;
        Ok(ToolOutput {
            content: vec![ToolResultContent::Text {
                text: format_snapshot(&snapshot),
            }],
            details: Some(json!(snapshot)),
            terminate: false,
        })
    }
}

fn optional_string(args: &Value, name: &str) -> Option<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_u64(args: &Value, name: &str) -> Result<Option<u64>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("参数 {name} 必须是非负整数")),
    }
}

fn parse_usize(args: &Value, name: &str) -> Result<Option<usize>, String> {
    parse_u64(args, name)?
        .map(|value| usize::try_from(value).map_err(|_| format!("参数 {name} 超出范围")))
        .transpose()
}

fn resolve_workspace_cwd(ctx: &ToolContext, requested: Option<&str>) -> Result<PathBuf, String> {
    let path = requested
        .map(|value| crate::tools::resolve_path(&ctx.workspace, value))
        .transpose()?
        .unwrap_or_else(|| ctx.workspace.clone());
    let canonical =
        std::fs::canonicalize(&path).map_err(|error| format!("后台任务 cwd 不可用: {error}"))?;
    if !canonical.starts_with(&ctx.workspace) || !canonical.is_dir() {
        return Err(format!(
            "后台任务 cwd 必须是工作目录 {} 内的现有目录",
            ctx.workspace.display()
        ));
    }
    Ok(canonical)
}

fn validate_managed_command(command: &str) -> Result<(), String> {
    if command.is_empty() {
        return Err("command 不能为空".into());
    }
    if command.len() > MAX_COMMAND_BYTES {
        return Err(format!("command 不能超过 {MAX_COMMAND_BYTES} 字节"));
    }
    if has_unquoted_background_operator(command) {
        return Err("后台任务不接受 shell 的单独 '&'；请让 Pipi 托管任务生命周期".into());
    }
    let segments = crate::permissions::split_segments(command)?;
    if segments.iter().any(|segment| {
        segment
            .argv
            .first()
            .is_some_and(|argv| matches!(argv.as_str(), "nohup" | "setsid" | "disown" | "daemon"))
    }) {
        return Err("后台任务不接受 nohup、setsid、disown 或 daemon 脱离托管".into());
    }
    Ok(())
}

fn has_unquoted_background_operator(command: &str) -> bool {
    let chars: Vec<char> = command.chars().collect();
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        if ch == '\\' && !single {
            escaped = true;
            index += 1;
            continue;
        }
        match ch {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '&' if !single && !double => {
                if chars.get(index + 1) == Some(&'&') {
                    index += 2;
                    continue;
                }
                return true;
            }
            _ => {}
        }
        index += 1;
    }
    false
}

fn format_query_result(snapshots: &[BackgroundTaskSnapshot]) -> String {
    if snapshots.is_empty() {
        return "当前 session 没有可见的后台任务。".into();
    }
    snapshots
        .iter()
        .map(format_snapshot)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_snapshot(snapshot: &BackgroundTaskSnapshot) -> String {
    let task = &snapshot.task;
    let mut text = format!(
        "任务 {}（{:?}）：{:?}，输出游标 {}，命令 `{}`",
        task.job_id, task.kind, task.status, task.output_cursor, task.command
    );
    if let Some(target) = &task.target_agent {
        text.push_str(&format!("；目标 Agent：{target}"));
    }
    if let Some(session_id) = &task.child_session_id {
        text.push_str(&format!("；child session：{session_id}"));
    }
    if let Some(error) = &task.error {
        text.push_str(&format!("；错误：{error}"));
    }
    if let Some(result) = &task.result {
        text.push_str(&format!("\n[agent result] {result}"));
    }
    for chunk in &snapshot.output {
        text.push_str(&format!(
            "\n[{} #{}] {}",
            chunk.stream, chunk.seq, chunk.text
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BackgroundTaskStatus;

    #[test]
    fn rejects_shell_detachment_but_allows_quoted_ampersand_and_and() {
        assert!(validate_managed_command("cargo test && echo ok").is_ok());
        assert!(validate_managed_command("echo '&'").is_ok());
        assert!(validate_managed_command("sleep 10 &").is_err());
        assert!(validate_managed_command("nohup cargo test").is_err());
    }

    #[test]
    fn parses_limits() {
        let args = json!({"waitMs": 12, "limit": 3});
        assert_eq!(parse_u64(&args, "waitMs").unwrap(), Some(12));
        assert_eq!(parse_usize(&args, "limit").unwrap(), Some(3));
    }

    #[test]
    fn status_active_only_covers_live_states() {
        assert!(BackgroundTaskStatus::Queued.is_active());
        assert!(BackgroundTaskStatus::Running.is_active());
        assert!(!BackgroundTaskStatus::Completed.is_active());
        assert!(!BackgroundTaskStatus::Orphaned.is_active());
    }
}
