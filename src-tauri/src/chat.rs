//! 会话运行时 —— M1 接线：把 agent_loop 接到 Tauri 事件上。
//!
//! 职责：解析 Agent 的 provider/密钥 → 组装 loop 配置 → 启动循环 →
//! AgentEvent 转发为 `agent-event` 事件 → 消息同步落盘 sessions/*.jsonl →
//! 每轮助手消息后发 `session-stats`（hermes 设计的统计快照）。
//! 业务逻辑仍在 pipi-core，这里只做桥接。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, State};

use pipi_core::agent_loop::{
    run_agent_loop, AgentContext, AgentLoopConfig, AgentEvent, MessageQueue, ToolExecutionMode,
};
use pipi_core::agent_loop::Emitter as LoopEmitter;
use pipi_core::agents::{self, AgentDefinition};
use pipi_core::provider::provider_for;
use pipi_core::session::SessionWriter;
use pipi_core::settings::load_settings;
use pipi_core::stats::SessionStatsTracker;
use pipi_core::tools::ToolRegistry;
use pipi_core::types::{AbortSignal, Message, Model, StreamOptions};

/// 一个（可能跨多轮的）会话的活状态。
pub struct Session {
    pub agent: AgentDefinition,
    pub messages: Arc<tokio::sync::Mutex<Vec<Message>>>,
    pub writer: Arc<Mutex<SessionWriter>>,
    pub stats: Arc<Mutex<SessionStatsTracker>>,
    pub abort: AbortSignal,
    pub running: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct ChatState {
    pub session: Mutex<Option<Session>>,
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
pub fn session_running(state: State<ChatState>) -> bool {
    state
        .session
        .lock()
        .map(|s| {
            s.as_ref()
                .map(|x| x.running.load(Ordering::Relaxed))
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
        if s.running.load(Ordering::Relaxed) {
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
        if s.running.load(Ordering::Relaxed) {
            return Err("会话正在运行，请等待完成或先停止".into());
        }
    }

    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err("空消息".into());
    }

    // 会话复用（同 Agent 续接）或新建
    let session = match slot.take() {
        Some(s) if s.agent.name == agent_name => s,
        _ => {
            let def = agents::load_agent(&agent_name)?;
            let sessions_dir = def.sessions_dir().ok_or("无法解析会话目录")?;
            let writer = SessionWriter::create(&sessions_dir).map_err(|e| e.to_string())?;
            let context_max = def.provider.as_ref().map(|p| p.context_window).filter(|w| *w > 0);
            Session {
                agent: def,
                messages: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                writer: Arc::new(Mutex::new(writer)),
                stats: Arc::new(Mutex::new(SessionStatsTracker::new(context_max))),
                abort: AbortSignal::new(),
                running: Arc::new(AtomicBool::new(false)),
            }
        }
    };
    let mut model = resolve_model(&session.agent)?;

    let user_message = Message::user_text(prompt);
    let messages = session.messages.clone();
    let writer = session.writer.clone();
    let stats = session.stats.clone();
    let running = session.running.clone();
    let abort = session.abort.clone();
    let def = session.agent.clone();

    running.store(true, Ordering::Relaxed);
    // 首条消息先落盘（崩溃也会留下用户输入）
    if let Ok(mut w) = writer.lock() {
        let _ = w.append_message(&user_message);
    }

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
    let system_prompt = agents::build_system_prompt(&def);
    let emitter: LoopEmitter = make_emitter(app, writer, stats);

    tauri::async_runtime::spawn(async move {
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
        running.store(false, Ordering::Relaxed);
    });

    *slot = Some(session);
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
