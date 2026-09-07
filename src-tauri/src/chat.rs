//! 会话运行时 —— M1 接线：把 agent_loop 接到 Tauri 事件上。
//!
//! 职责：解析 Agent 的 provider/密钥 → 组装 loop 配置 → 启动循环 →
//! AgentEvent 转发为 `agent-event` 事件 → 消息同步落盘 sessions/*.jsonl →
//! 每轮助手消息后发 `session-stats`（hermes 设计的统计快照）。
//! 业务逻辑仍在 pipi-core，这里只做桥接。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, State};

use pipi_core::agent_loop::{
    run_agent_loop, AgentContext, AgentLoopConfig, AgentEvent, MessageQueue, ToolExecutionMode,
};
use pipi_core::agent_loop::Emitter as LoopEmitter;
use pipi_core::agents::{self, AgentDefinition};
use pipi_core::provider::provider_for;
use pipi_core::session::{list_session_summaries, load_session, SessionSummary, SessionWriter};
use pipi_core::settings::load_settings;
use pipi_core::stats::SessionStatsTracker;
use pipi_core::tools::ToolRegistry;
use pipi_core::types::{AbortSignal, Message, Model, StreamOptions};

/// 一个（可能跨多轮的）会话的运行状态。
///
/// 低位是 running 标志，其余位是代际；结束操作只允许清除自己开始的代际。
pub struct RunState {
    value: AtomicUsize,
}

impl RunState {
    const RUNNING_BIT: usize = 1;

    fn new() -> Self {
        Self {
            value: AtomicUsize::new(0),
        }
    }

    fn begin(&self) -> usize {
        let mut current = self.value.load(Ordering::Acquire);
        loop {
            let generation = current >> 1;
            let next_generation = generation.wrapping_add(1);
            let next = (next_generation << 1) | Self::RUNNING_BIT;
            match self.value.compare_exchange(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return next,
                Err(actual) => current = actual,
            }
        }
    }

    fn finish(&self, token: usize) {
        let idle = token & !Self::RUNNING_BIT;
        let _ = self.value.compare_exchange(
            token,
            idle,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn is_running(&self) -> bool {
        self.value.load(Ordering::Acquire) & Self::RUNNING_BIT != 0
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

/// 一个（可能跨多轮的）会话的活状态。
pub struct Session {
    pub agent: AgentDefinition,
    pub messages: Arc<tokio::sync::Mutex<Vec<Message>>>,
    pub writer: Arc<Mutex<SessionWriter>>,
    pub stats: Arc<Mutex<SessionStatsTracker>>,
    pub abort: AbortSignal,
    pub running: Arc<RunState>,
}

#[derive(Default)]
pub struct ChatState {
    pub session: Mutex<Option<Session>>,
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

fn resolve_model(def: &AgentDefinition) -> Result<Model, String> {
    let provider = def.provider.clone().ok_or_else(|| {
        "该 Agent 还未绑定模型提供商（在 Agent 详情页绑定，并在设置中配置密钥）".to_string()
    })?;
    let settings = load_settings();
    let api_key = settings
        .providers
        .iter()
        .find(|p| p.api == provider.api && p.base_url == provider.base_url)
        .and_then(|p| p.resolve_api_key())
        .ok_or_else(|| format!("提供商 {} 未配置 API 密钥（设置 → 模型提供商）", provider.base_url))?;
    let _ = api_key; // StreamOptions 在 loop 配置里传
    Ok(Model {
        id: provider.id.clone(),
        name: provider.name.clone(),
        api: provider.api,
        base_url: provider.base_url.clone(),
        max_tokens: provider.max_tokens,
        context_window: provider.context_window,
    })
}

/// AgentEvent 转发为前端事件；MessageEnd 时落盘 + 更新统计。
fn make_emitter(
    app: AppHandle,
    writer: Arc<Mutex<SessionWriter>>,
    stats: Arc<Mutex<SessionStatsTracker>>,
) -> LoopEmitter {
    Arc::new(move |event: AgentEvent| {
        let _ = app.emit("agent-event", &event);
        if let AgentEvent::MessageEnd { message } = &event {
            if let Ok(mut w) = writer.lock() {
                let _ = w.append_message(message);
            }
            if message.role() == "assistant" {
                if let Ok(mut s) = stats.lock() {
                    s.record(message);
                    let _ = app.emit("session-stats", s.snapshot());
                }
            }
        }
    })
}

#[tauri::command]
pub fn list_sessions(agent_name: String) -> Result<Vec<SessionSummary>, String> {
    let def = agents::load_agent(&agent_name)?;
    let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
    Ok(list_session_summaries(&dir))
}

/// 打开（续写）一个已有会话：消息载入内存，后续 send_prompt 追加到同一文件。
#[tauri::command]
pub fn open_session(
    state: State<ChatState>,
    agent_name: String,
    session_id: String,
) -> Result<(), String> {
    // id 是文件名 stem，只允许安全字符（防路径穿越）
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("非法会话 ID".into());
    }
    let def = agents::load_agent(&agent_name)?;
    let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
    let path = dir.join(format!("{session_id}.jsonl"));
    if !path.is_file() {
        return Err("会话不存在".into());
    }

    let mut slot = state.session.lock().map_err(|e| e.to_string())?;
    if let Some(s) = slot.as_ref() {
        if s.running.is_running() {
            return Err("当前会话仍在运行，请先停止".into());
        }
    }

    let entries = load_session(&path).map_err(|e| e.to_string())?;
    let messages: Vec<Message> = entries
        .iter()
        .filter_map(|e| match &e.kind {
            pipi_core::session::EntryKind::Message { message } => Some(message.clone()),
            _ => None,
        })
        .collect();
    let context_max = def.provider.as_ref().map(|p| p.context_window).filter(|w| *w > 0);
    let mut tracker = SessionStatsTracker::new(context_max);
    for m in &messages {
        tracker.record(m);
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
    });
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub agent_name: String,
    pub session_id: String,
    pub running: bool,
}

/// 当前活会话信息（侧栏高亮用）；无会话返回 None。
#[tauri::command]
pub fn session_info(state: State<ChatState>) -> Option<SessionInfo> {
    let session = state.session.lock().ok()?;
    let s = session.as_ref()?;
    let session_id = s
        .writer
        .lock()
        .ok()?
        .path()
        .file_stem()
        .and_then(|x| x.to_str())
        .map(str::to_string)?;
    Some(SessionInfo {
        agent_name: s.agent.name.clone(),
        session_id,
        running: s.running.is_running(),
    })
}

#[tauri::command]
pub fn session_running(state: State<ChatState>) -> bool {
    state
        .session
        .lock()
        .map(|s| {
            s.as_ref()
                .map(|x| x.running.is_running())
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

#[tauri::command]
pub fn stop_run(state: State<ChatState>) -> Result<(), String> {
    let session = state.session.lock().map_err(|e| e.to_string())?;
    if let Some(session) = session.as_ref() {
        session.abort.abort();
    }
    Ok(())
}

#[tauri::command]
pub fn new_session(state: State<ChatState>) -> Result<(), String> {
    let mut slot = state.session.lock().map_err(|e| e.to_string())?;
    if let Some(s) = slot.as_ref() {
        if s.running.is_running() {
            return Err("当前会话仍在运行，请先停止".into());
        }
    }
    *slot = None;
    Ok(())
}

/// 发送一轮提示：启动（或续接）会话并异步跑 agent loop。
#[tauri::command]
pub fn send_prompt(
    app: AppHandle,
    state: State<'_, ChatState>,
    agent_name: String,
    prompt: String,
) -> Result<(), String> {
    let mut slot = state.session.lock().map_err(|e| e.to_string())?;
    if let Some(s) = slot.as_ref() {
        if s.running.is_running() {
            return Err("会话正在运行，请等待完成或先停止".into());
        }
    }

    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err("空消息".into());
    }

    // 会话复用（同 Agent 续接）或新建
    let transaction = SlotTransaction::new(&mut slot);
    let reuse_session = transaction
        .current()
        .map(|session| session.agent.name == agent_name)
        .unwrap_or(false);
    let replacement = if reuse_session {
        None
    } else {
        let def = agents::load_agent(&agent_name)?;
        let sessions_dir = def.sessions_dir().ok_or("无法解析会话目录")?;
        let writer = SessionWriter::create(&sessions_dir).map_err(|e| e.to_string())?;
        let context_max = def.provider.as_ref().map(|p| p.context_window).filter(|w| *w > 0);
        Some(Session {
            agent: def,
            messages: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            writer: Arc::new(Mutex::new(writer)),
            stats: Arc::new(Mutex::new(SessionStatsTracker::new(context_max))),
            abort: AbortSignal::new(),
            running: Arc::new(RunState::new()),
        })
    };
    let session = replacement
        .as_ref()
        .or_else(|| transaction.current())
        .ok_or("无法创建会话")?;
    let def = session.agent.clone();
    let mut model = resolve_model(&def)?;

    let user_message = Message::user_text(prompt);
    let messages = session.messages.clone();
    let writer = session.writer.clone();
    let stats = session.stats.clone();
    let running = session.running.clone();
    let abort = session.abort.clone();

    // loop 配置：provider 密钥从 settings 解析后进 StreamOptions
    let settings = load_settings();
    let api_key = session
        .agent
        .provider
        .as_ref()
        .and_then(|p| {
            settings
                .providers
                .iter()
                .find(|s| s.api == p.api && s.base_url == p.base_url)
                .and_then(|s| s.resolve_api_key())
        })
        .ok_or_else(|| "提供商未配置 API 密钥（设置 → 模型提供商）".to_string())?;
    model = Model {
        id: model.id.clone(),
        name: model.name.clone(),
        api: model.api,
        base_url: model.base_url.clone(),
        max_tokens: model.max_tokens,
        context_window: model.context_window,
    };

    let tool_context = agents::build_tool_context(&def, abort.clone())?;
    let registry = Arc::new(ToolRegistry::for_context(&tool_context));
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
        let mut w = writer
            .lock()
            .map_err(|e| format!("无法锁定会话写入器: {e}"))?;
        w.append_message(&user_message)
            .map_err(|e| format!("无法写入用户消息: {e}"))?;
    }

    // stop_run 只中止上一轮；新的轮次复用会话时必须清除旧状态。
    abort.reset();
    let running_guard = RunningGuard::new(running);

    let system_prompt = agents::build_system_prompt(&def);
    let emitter: LoopEmitter = make_emitter(app, writer, stats);

    tauri::async_runtime::spawn(async move {
        let _running_guard = running_guard;
        // 会话历史 + 本轮 prompt
        let mut context = AgentContext {
            system_prompt,
            messages: {
                let mut m = messages.lock().await.clone();
                m.push(user_message.clone());
                m
            },
        };
        messages.lock().await.push(user_message);

        let new_messages = run_agent_loop(
            Vec::new(), // prompts 已并入 context（会话续接语义）
            context.clone(),
            config,
            emitter,
            abort,
        )
        .await;

        // 回写会话历史（供下一轮续接）
        messages.lock().await.extend(new_messages);
        let _ = &mut context;
    });

    if let Some(session) = replacement {
        transaction.commit(session);
    } else {
        transaction.commit_current();
    }
    Ok(())
}

/// 读取当前会话消息（切回聊天视图时恢复）。
#[tauri::command]
pub async fn session_messages(state: State<'_, ChatState>) -> Result<Vec<Message>, String> {
    // 先取出 Arc 并释放锁，再跨 await 读取
    let messages = {
        let session = state.session.lock().map_err(|e| e.to_string())?;
        session.as_ref().map(|s| s.messages.clone())
    };
    match messages {
        Some(m) => Ok(m.lock().await.clone()),
        None => Ok(Vec::new()),
    }
}

/// 读取最近一次统计快照。
#[tauri::command]
pub fn session_stats(state: State<ChatState>) -> Result<pipi_core::stats::SessionStats, String> {
    let session = state.session.lock().map_err(|e| e.to_string())?;
    match session.as_ref() {
        Some(s) => Ok(s.stats.lock().unwrap().snapshot()),
        None => Ok(Default::default()),
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::{RunState, RunningGuard, SlotTransaction};

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
            transaction.commit("new");
        }

        assert_eq!(slot, Some("new"));
    }
}
