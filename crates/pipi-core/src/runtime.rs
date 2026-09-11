//! Agent 会话运行时。
//!
//! 这里承载 Tauri 桌面端与 Web 远程服务共同使用的会话状态、Agent loop
//! 编排、消息落盘和事件协议。宿主只需要把 RuntimeEvent 转发到自己的
//! 事件系统即可，不应重复实现 Agent 执行逻辑。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::agent_loop::Emitter as LoopEmitter;
use crate::agent_loop::{
    run_agent_loop, AgentContext, AgentEvent, AgentLoopConfig, MessageQueue, ToolExecutionMode,
};
use crate::agents::{self, AgentDefinition};
use crate::provider::provider_for;
pub use crate::session::SessionSummary;
use crate::session::{list_session_summaries, load_session, SessionWriter};
use crate::settings::load_settings;
use crate::stats::SessionStatsTracker;
use crate::tools::ToolRegistry;
use crate::types::{AbortSignal, Message, Model, StreamOptions};

static NEXT_RUN_ID: AtomicUsize = AtomicUsize::new(1);

/// 发给宿主的 Agent 事件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventEnvelope {
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
    pub event: AgentEvent,
}

/// 发给宿主的统计快照。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsEventEnvelope {
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
    pub stats: crate::stats::SessionStats,
}

/// 发给宿主的会话错误。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionErrorEnvelope {
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
    pub message: String,
}

/// 宿主只需将这个协议映射为 Tauri event 或 WebSocket frame。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "payload")]
pub enum RuntimeEvent {
    #[serde(rename = "agent-event")]
    AgentEvent(AgentEventEnvelope),
    #[serde(rename = "session-stats")]
    SessionStats(StatsEventEnvelope),
    #[serde(rename = "session-error")]
    SessionError(SessionErrorEnvelope),
}

/// 运行时事件接收器。
pub type EventEmitter = Arc<dyn Fn(RuntimeEvent) + Send + Sync>;

/// 一个（可能跨多轮的）会话的运行状态。
///
/// 低位是 running 标志，其余位是代际；结束操作只允许清除自己开始的代际。
struct RunState {
    value: AtomicUsize,
    run_id: AtomicUsize,
}

impl RunState {
    const RUNNING_BIT: usize = 1;

    fn new() -> Self {
        Self {
            value: AtomicUsize::new(0),
            run_id: AtomicUsize::new(NEXT_RUN_ID.load(Ordering::Acquire).saturating_sub(1)),
        }
    }

    fn begin(&self) -> usize {
        let mut current = self.value.load(Ordering::Acquire);
        loop {
            let generation = current >> 1;
            let next_generation = generation.wrapping_add(1);
            let next = (next_generation << 1) | Self::RUNNING_BIT;
            match self
                .value
                .compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    let run_id = NEXT_RUN_ID.fetch_add(1, Ordering::AcqRel);
                    self.run_id.store(run_id, Ordering::Release);
                    return next;
                }
                Err(actual) => current = actual,
            }
        }
    }

    fn finish(&self, token: usize) {
        let idle = token & !Self::RUNNING_BIT;
        let _ = self
            .value
            .compare_exchange(token, idle, Ordering::AcqRel, Ordering::Acquire);
    }

    fn is_running(&self) -> bool {
        self.value.load(Ordering::Acquire) & Self::RUNNING_BIT != 0
    }

    fn current_run_id(&self) -> usize {
        self.run_id.load(Ordering::Acquire)
    }
}

/// 一个运行代际的守卫；无论正常返回、取消还是 panic 都会清理状态。
struct RunningGuard {
    state: Arc<RunState>,
    token: usize,
}

impl RunningGuard {
    fn new(state: Arc<RunState>) -> Self {
        let token = state.begin();
        Self { state, token }
    }
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.state.finish(self.token);
    }
}

struct Session {
    agent: AgentDefinition,
    messages: Arc<tokio::sync::Mutex<Vec<Message>>>,
    writer: Arc<Mutex<SessionWriter>>,
    stats: Arc<Mutex<SessionStatsTracker>>,
    abort: AbortSignal,
    running: Arc<RunState>,
    model: Arc<Mutex<Option<Model>>>,
}

/// 共享会话状态。Tauri 与 Web 服务各自持有一个实例。
#[derive(Default)]
pub struct RuntimeState {
    session: Mutex<Option<Session>>,
}

/// 暂存 slot 原值，前置步骤失败时自动恢复，成功后才提交新值。
struct SlotTransaction<'a, T> {
    slot: &'a mut Option<T>,
    staged: Option<T>,
    committed: bool,
}

impl<'a, T> SlotTransaction<'a, T> {
    fn new(slot: &'a mut Option<T>) -> Self {
        Self {
            staged: slot.take(),
            slot,
            committed: false,
        }
    }

    fn current(&self) -> Option<&T> {
        self.staged.as_ref()
    }

    fn commit(mut self, value: T) {
        *self.slot = Some(value);
        self.committed = true;
    }

    fn commit_current(mut self) {
        *self.slot = self.staged.take();
        self.committed = true;
    }
}

impl<T> Drop for SlotTransaction<'_, T> {
    fn drop(&mut self) {
        if !self.committed {
            *self.slot = self.staged.take();
        }
    }
}

fn resolve_model(
    def: &AgentDefinition,
    session_model: Option<&Model>,
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
        .and_then(|p| p.resolve_api_key())
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

fn writer_session_id(writer: &Arc<Mutex<SessionWriter>>) -> Result<String, String> {
    writer
        .lock()
        .map_err(|error| format!("无法锁定会话写入器: {error}"))?
        .path()
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| "无法解析会话 ID".to_string())
}

fn make_emitter(
    sink: EventEmitter,
    writer: Arc<Mutex<SessionWriter>>,
    stats: Arc<Mutex<SessionStatsTracker>>,
    agent_name: String,
    session_id: String,
    run_id: usize,
) -> LoopEmitter {
    Arc::new(move |event: AgentEvent| {
        // AgentEnd 延迟到 run loop 回写完整历史后统一发送。
        if matches!(&event, AgentEvent::AgentEnd { .. }) {
            return;
        }
        sink(RuntimeEvent::AgentEvent(AgentEventEnvelope {
            agent_name: agent_name.clone(),
            session_id: session_id.clone(),
            run_id,
            event: event.clone(),
        }));
        if let AgentEvent::MessageEnd { message } = &event {
            match writer.lock() {
                Ok(mut writer) => {
                    if let Err(error) = writer.append_message(message) {
                        sink(RuntimeEvent::SessionError(SessionErrorEnvelope {
                            agent_name: agent_name.clone(),
                            session_id: session_id.clone(),
                            run_id,
                            message: format!("会话消息落盘失败: {error}"),
                        }));
                    }
                }
                Err(error) => {
                    sink(RuntimeEvent::SessionError(SessionErrorEnvelope {
                        agent_name: agent_name.clone(),
                        session_id: session_id.clone(),
                        run_id,
                        message: format!("无法锁定会话写入器: {error}"),
                    }));
                }
            }
            if message.role() == "assistant" {
                match stats.lock() {
                    Ok(mut tracker) => {
                        tracker.record(message);
                        sink(RuntimeEvent::SessionStats(StatsEventEnvelope {
                            agent_name: agent_name.clone(),
                            session_id: session_id.clone(),
                            run_id,
                            stats: tracker.snapshot(),
                        }));
                    }
                    Err(error) => {
                        sink(RuntimeEvent::SessionError(SessionErrorEnvelope {
                            agent_name: agent_name.clone(),
                            session_id: session_id.clone(),
                            run_id,
                            message: format!("无法更新会话统计: {error}"),
                        }));
                    }
                }
            }
        }
    })
}

/// 列出指定 Agent 的会话摘要。
pub fn list_sessions(agent_name: &str) -> Result<Vec<SessionSummary>, String> {
    let def = agents::load_agent(agent_name)?;
    let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
    Ok(list_session_summaries(&dir))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub agent_name: String,
    pub session_id: String,
    pub running: bool,
    pub run_id: usize,
    pub model: Option<Model>,
    pub is_custom_model: bool,
}

impl RuntimeState {
    /// 打开（续写）一个已有会话。
    pub fn open_session(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        if session_id.is_empty()
            || !session_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err("非法会话 ID".into());
        }
        let def = agents::load_agent(agent_name)?;
        let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
        let path = dir.join(format!("{session_id}.jsonl"));
        if !path.is_file() {
            return Err("会话不存在".into());
        }

        let mut slot = self.session.lock().map_err(|e| e.to_string())?;
        if let Some(session) = slot.as_ref() {
            if session.running.is_running() {
                return Err("当前会话仍在运行，请先停止".into());
            }
        }

        let entries = load_session(&path).map_err(|e| e.to_string())?;
        let active = crate::session::active_path(&entries);
        let messages: Vec<Message> = active
            .into_iter()
            .filter_map(|entry| match &entry.kind {
                crate::session::EntryKind::Message { message } => Some(message.clone()),
                _ => None,
            })
            .collect();
        let active_model = crate::session::active_model_from_entries(&entries);
        let effective_model = active_model.as_ref().or(def.provider.as_ref());
        let context_max = effective_model
            .map(|provider| provider.context_window)
            .filter(|window| *window > 0);
        let mut tracker = SessionStatsTracker::new(context_max);
        for message in &messages {
            tracker.record(message);
        }

        *slot = Some(Session {
            agent: def,
            messages: Arc::new(tokio::sync::Mutex::new(messages)),
            writer: Arc::new(Mutex::new(
                SessionWriter::open(&path).map_err(|e| e.to_string())?,
            )),
            stats: Arc::new(Mutex::new(tracker)),
            abort: AbortSignal::new(),
            running: Arc::new(RunState::new()),
            model: Arc::new(Mutex::new(active_model)),
        });
        Ok(())
    }

    /// 分叉一个会话（可指定截断至某个 entry_id，留空则分叉到当前 tip）。
    pub fn fork_session(
        &self,
        agent_name: &str,
        session_id: &str,
        up_to_entry_id: Option<&str>,
    ) -> Result<SessionInfo, String> {
        if session_id.is_empty()
            || !session_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err("非法会话 ID".into());
        }
        let def = agents::load_agent(agent_name)?;
        let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
        let source_path = dir.join(format!("{session_id}.jsonl"));
        if !source_path.is_file() {
            return Err("源会话不存在".into());
        }

        let mut slot = self.session.lock().map_err(|e| e.to_string())?;
        if let Some(session) = slot.as_ref() {
            if session.running.is_running() {
                return Err("当前会话仍在运行，请先停止".into());
            }
        }

        let writer = crate::session::fork_session(&source_path, &dir, up_to_entry_id)?;
        let new_session_path = writer.path().to_path_buf();
        let new_session_id = new_session_path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "无法解析新会话 ID".to_string())?
            .to_string();

        let entries = load_session(&new_session_path).map_err(|e| e.to_string())?;
        let active = crate::session::active_path(&entries);
        let messages: Vec<Message> = active
            .into_iter()
            .filter_map(|entry| match &entry.kind {
                crate::session::EntryKind::Message { message } => Some(message.clone()),
                _ => None,
            })
            .collect();
        let active_model = crate::session::active_model_from_entries(&entries);
        let effective_model = active_model.clone().or_else(|| def.provider.clone());
        let context_max = effective_model
            .as_ref()
            .map(|provider| provider.context_window)
            .filter(|window| *window > 0);
        let mut tracker = SessionStatsTracker::new(context_max);
        for message in &messages {
            tracker.record(message);
        }

        let run_state = Arc::new(RunState::new());
        *slot = Some(Session {
            agent: def,
            messages: Arc::new(tokio::sync::Mutex::new(messages)),
            writer: Arc::new(Mutex::new(writer)),
            stats: Arc::new(Mutex::new(tracker)),
            abort: AbortSignal::new(),
            running: run_state.clone(),
            model: Arc::new(Mutex::new(active_model.clone())),
        });

        Ok(SessionInfo {
            agent_name: agent_name.to_string(),
            session_id: new_session_id,
            running: false,
            run_id: run_state.current_run_id(),
            model: effective_model,
            is_custom_model: active_model.is_some(),
        })
    }

    /// 当前活会话信息；无会话返回 None。
    pub fn session_info(&self) -> Result<Option<SessionInfo>, String> {
        let session = self
            .session
            .lock()
            .map_err(|error| format!("无法读取当前会话: {error}"))?;
        let Some(session) = session.as_ref() else {
            return Ok(None);
        };
        let writer = session
            .writer
            .lock()
            .map_err(|error| format!("无法读取会话路径: {error}"))?;
        let session_id = writer
            .path()
            .file_stem()
            .and_then(|value| value.to_str())
            .map(str::to_string)
            .ok_or_else(|| "当前会话路径无有效 ID".to_string())?;
        let custom_model = session.model.lock().ok().and_then(|m| m.clone());
        let effective_model = custom_model.clone().or_else(|| session.agent.provider.clone());
        let is_custom_model = custom_model.is_some();
        Ok(Some(SessionInfo {
            agent_name: session.agent.name.clone(),
            session_id,
            running: session.running.is_running(),
            run_id: session.running.current_run_id(),
            model: effective_model,
            is_custom_model,
        }))
    }

    /// 为当前会话设置或切换模型配置。
    /// 传入 None 时恢复为 Agent 默认模型。
    pub fn set_session_model(&self, model: Option<Model>) -> Result<(), String> {
        let slot = self.session.lock().map_err(|e| e.to_string())?;
        let Some(session) = slot.as_ref() else {
            return Err("当前没有打开的会话".into());
        };
        if session.running.is_running() {
            return Err("会话正在运行，请等待完成或先停止后再切换模型".into());
        }

        let mut current_model_slot = session.model.lock().map_err(|e| e.to_string())?;
        let effective_target = model.clone().or_else(|| session.agent.provider.clone());

        // 校验目标模型的 API 密钥是否已配置
        if let Some(target) = &effective_target {
            let _ = resolve_model(&session.agent, Some(target))?;
        }

        let has_changed = *current_model_slot != model;
        if has_changed {
            if let Some(target) = &effective_target {
                let mut writer = session.writer.lock().map_err(|e| e.to_string())?;
                writer
                    .append_model_change(target)
                    .map_err(|e| e.to_string())?;
            }
            if let Some(target) = &effective_target {
                if let Ok(mut tracker) = session.stats.lock() {
                    let context_max = if target.context_window > 0 {
                        Some(target.context_window)
                    } else {
                        None
                    };
                    tracker.set_context_max(context_max);
                }
            }
            *current_model_slot = model;
        }
        Ok(())
    }

    pub fn session_running(&self) -> bool {
        self.session
            .lock()
            .map(|session| {
                session
                    .as_ref()
                    .map(|session| session.running.is_running())
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    pub fn stop_run(&self) -> Result<(), String> {
        let session = self.session.lock().map_err(|e| e.to_string())?;
        if let Some(session) = session.as_ref() {
            session.abort.abort();
        }
        Ok(())
    }

    pub fn new_session(&self) -> Result<(), String> {
        let mut slot = self.session.lock().map_err(|e| e.to_string())?;
        if let Some(session) = slot.as_ref() {
            if session.running.is_running() {
                return Err("当前会话仍在运行，请先停止".into());
            }
        }
        *slot = None;
        Ok(())
    }

    pub async fn session_messages(&self) -> Result<Vec<Message>, String> {
        let messages = {
            let session = self.session.lock().map_err(|e| e.to_string())?;
            session.as_ref().map(|session| session.messages.clone())
        };
        match messages {
            Some(messages) => Ok(messages.lock().await.clone()),
            None => Ok(Vec::new()),
        }
    }

    pub fn session_stats(&self) -> Result<crate::stats::SessionStats, String> {
        let session = self.session.lock().map_err(|e| e.to_string())?;
        match session.as_ref() {
            Some(session) => Ok(session.stats.lock().map_err(|e| e.to_string())?.snapshot()),
            None => Ok(Default::default()),
        }
    }

    /// 启动一轮 Agent；完成后的事件通过 event_sink 广播给宿主。
    pub fn send_prompt(
        &self,
        agent_name: &str,
        prompt: &str,
        model: Option<Model>,
        event_sink: EventEmitter,
    ) -> Result<(), String> {
        let mut slot = self.session.lock().map_err(|e| e.to_string())?;
        if let Some(session) = slot.as_ref() {
            if session.running.is_running() {
                return Err("会话正在运行，请等待完成或先停止".into());
            }
        }

        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return Err("空消息".into());
        }

        // 会话复用（同 Agent 续接）或新建。
        let transaction = SlotTransaction::new(&mut slot);
        let reuse_session = transaction
            .current()
            .map(|session| session.agent.name == agent_name)
            .unwrap_or(false);
        let replacement = if reuse_session {
            None
        } else {
            let def = agents::load_agent(agent_name)?;
            let sessions_dir = def.sessions_dir().ok_or("无法解析会话目录")?;
            let mut writer = SessionWriter::create(&sessions_dir).map_err(|e| e.to_string())?;
            let active_model = model.clone();
            let effective_model = active_model.as_ref().or(def.provider.as_ref());
            let context_max = effective_model
                .map(|provider| provider.context_window)
                .filter(|window| *window > 0);
            if let Some(target) = &active_model {
                if Some(target) != def.provider.as_ref() {
                    writer
                        .append_model_change(target)
                        .map_err(|e| e.to_string())?;
                }
            }
            Some(Session {
                agent: def,
                messages: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                writer: Arc::new(Mutex::new(writer)),
                stats: Arc::new(Mutex::new(SessionStatsTracker::new(context_max))),
                abort: AbortSignal::new(),
                running: Arc::new(RunState::new()),
                model: Arc::new(Mutex::new(active_model)),
            })
        };
        let session = replacement
            .as_ref()
            .or_else(|| transaction.current())
            .ok_or("无法创建会话")?;

        // 若复用已有会话且传入了明确的模型变更请求
        if reuse_session {
            if let Some(target) = &model {
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
        }

        let def = session.agent.clone();
        let current_session_model = session.model.lock().ok().and_then(|m| m.clone());
        let (model, api_key) = resolve_model(&def, current_session_model.as_ref())?;

        let user_message = Message::user_text(prompt);
        let messages = session.messages.clone();
        let writer = session.writer.clone();
        let stats = session.stats.clone();
        let running = session.running.clone();
        let abort = session.abort.clone();

        let tool_context = agents::build_tool_context(&def, abort.clone())?;
        let registry = Arc::new(ToolRegistry::for_context(&tool_context));
        let wire_tools = registry.wire_tools();
        let config = AgentLoopConfig {
            model: model.clone(),
            provider: provider_for(model.api),
            tools: registry,
            tool_context,
            options: StreamOptions {
                api_key: Some(api_key),
                temperature: None,
                max_tokens: Some(model.max_tokens),
                timeout_secs: 300,
            },
            tool_execution: ToolExecutionMode::Parallel,
            steering: MessageQueue::new(),
            follow_up: MessageQueue::new(),
            before_tool_call: None,
            after_tool_call: None,
            transform_context: None,
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

        abort.reset();
        let running_guard = RunningGuard::new(running.clone());
        let run_token = running_guard.token;
        let run_id = running.current_run_id();
        let session_id = writer_session_id(&writer)?;
        let system_prompt = agents::build_system_prompt_with_tools(&def, &wire_tools);
        let completion_sink = event_sink.clone();
        let emitter = make_emitter(
            event_sink,
            writer,
            stats,
            def.name.clone(),
            session_id.clone(),
            run_id,
        );

        tokio::spawn(async move {
            let _running_guard = running_guard;
            let context = AgentContext {
                system_prompt,
                messages: {
                    let mut messages = messages.lock().await.clone();
                    messages.push(user_message.clone());
                    messages
                },
            };
            messages.lock().await.push(user_message);

            let new_messages = run_agent_loop(
                Vec::new(), // prompts 已并入 context（会话续接语义）
                context,
                config,
                emitter,
                abort,
            )
            .await;

            let completion_messages = new_messages.clone();
            messages.lock().await.extend(new_messages);
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

        if let Some(session) = replacement {
            transaction.commit(session);
        } else {
            transaction.commit_current();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::pending;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn overlapping_generations_keep_new_run_visible() {
        let state = Arc::new(RunState::new());
        let old_guard = RunningGuard::new(state.clone());
        let new_guard = RunningGuard::new(state.clone());

        drop(old_guard);
        assert!(state.is_running());

        drop(new_guard);
        assert!(!state.is_running());
    }

    #[test]
    fn tokio_abort_drops_running_guard() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(RunState::new());
        let task_state = state.clone();

        runtime.block_on(async move {
            let handle = tokio::spawn(async move {
                let _guard = RunningGuard::new(task_state);
                pending::<()>().await;
            });
            tokio::task::yield_now().await;
            assert!(state.is_running());

            handle.abort();
            assert!(handle.await.is_err());
            assert!(!state.is_running());
        });
    }

    #[test]
    fn tokio_panic_drops_running_guard() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(RunState::new());
        let task_state = state.clone();

        runtime.block_on(async move {
            let handle = tokio::spawn(async move {
                let _guard = RunningGuard::new(task_state);
                panic!("test panic");
            });

            assert!(handle.await.is_err());
            assert!(!state.is_running());
        });
    }

    #[test]
    fn running_state_is_visible_across_threads() {
        let state = Arc::new(RunState::new());
        let started = Arc::new(Barrier::new(2));
        let observed = Arc::new(Barrier::new(2));
        let finished = Arc::new(Barrier::new(2));
        let observer_state = state.clone();
        let observer_started = started.clone();
        let observer_observed = observed.clone();
        let observer_finished = finished.clone();

        let observer = thread::spawn(move || {
            observer_started.wait();
            assert!(observer_state.is_running());
            observer_observed.wait();
            observer_finished.wait();
            assert!(!observer_state.is_running());
        });

        let guard = RunningGuard::new(state.clone());
        started.wait();
        observed.wait();
        drop(guard);
        finished.wait();
        observer.join().unwrap();
    }

    #[test]
    fn slot_transaction_restores_staged_value_after_failed_preflight() {
        let mut slot = Some("old");

        let result: Result<(), &str> = {
            let transaction = SlotTransaction::new(&mut slot);
            assert_eq!(transaction.current(), Some(&"old"));
            Err("preflight failed")
        };

        assert_eq!(result, Err("preflight failed"));
        assert_eq!(slot, Some("old"));
    }

    #[test]
    fn slot_transaction_commits_replacement() {
        let mut slot = Some("old");

        {
            let transaction = SlotTransaction::new(&mut slot);
            assert_eq!(transaction.current(), Some(&"old"));
            transaction.commit("new");
        }

        assert_eq!(slot, Some("new"));
    }

    #[test]
    fn resolve_model_prefers_session_model_over_agent_default() {
        let default_model = Model {
            id: "gpt-4o".into(),
            name: "GPT-4o".into(),
            api: crate::types::Api::OpenAICompletions,
            base_url: "https://api.openai.com/v1".into(),
            max_tokens: 4096,
            context_window: 128000,
        };
        let session_model = Model {
            id: "claude-sonnet-4-5".into(),
            name: "Claude Sonnet".into(),
            api: crate::types::Api::AnthropicMessages,
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
        };

        // 未配置 key 时的校验
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");

        // 回退默认模型
        let err = super::resolve_model(&def, None).unwrap_err();
        assert!(err.contains("未配置 API 密钥"));

        // 优先会话覆盖模型
        let err = super::resolve_model(&def, Some(&session_model)).unwrap_err();
        assert!(err.contains("api.anthropic.com"));

        // 配上 key 后成功解析
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");
        let (resolved, key) = super::resolve_model(&def, Some(&session_model)).unwrap();
        assert_eq!(resolved.id, "claude-sonnet-4-5");
        assert_eq!(key, "test-key");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
}
