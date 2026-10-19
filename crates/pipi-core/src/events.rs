//! 宿主事件协议：运行时发给宿主的信封与事件枚举。
//!
//! 这是宿主的**叶子模块**：只依赖协议与 Agent 事件类型，不依赖
//! `runtime` 或 `approval`。于是 `approval` 能发审批请求、`runtime` 能发全部
//! 事件，而两者之间不再有类型上的环。宿主把 [`RuntimeEvent`] 映射成自己的
//! 传输（Tauri event 或 WebSocket frame），不做业务解释。

use std::sync::{Arc, RwLock};

use serde::Serialize;

use pipi_agent::agent_loop::AgentEvent;
use pipi_agent::stats::SessionStats;

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
    pub stats: SessionStats,
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

/// 压缩换会话后发给宿主：当前会话已切到新 id（原会话可能已归档）。
///
/// **envelope 用的是切换前的 session_id** —— 前端此刻的身份还是旧的，
/// 用新 id 会先被 `eventMatchesSession` 丢掉；新 id 放在 payload 里。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSwitchedEnvelope {
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
    /// 切换后的会话 id（前端应把身份更新为它）。
    pub to_session_id: String,
    /// 原会话是否已移入归档。
    pub archived: bool,
}

/// 一个独立 child session 已经创建并写入磁盘，宿主应刷新对应 Agent 的会话列表。
///
/// 这条事件不绑定当前打开的父会话：后台 `run_agent` 可能在父轮结束后才创建
/// child，前端也可能已经切到了别的 Agent。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionChangedEnvelope {
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
}

/// 发给宿主的审批请求。`missing` 是白名单未命中的段落，「总是允许」时按段写入。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestEnvelope {
    pub request_id: String,
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
    pub command: String,
    pub missing: Vec<String>,
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
    #[serde(rename = "approval-request")]
    ApprovalRequest(ApprovalRequestEnvelope),
    #[serde(rename = "session-switched")]
    SessionSwitched(SessionSwitchedEnvelope),
    #[serde(rename = "session-changed")]
    SessionChanged(SessionChangedEnvelope),
}

/// 运行时事件接收器。
pub type EventEmitter = Arc<dyn Fn(RuntimeEvent) + Send + Sync>;

/// 一个宿主的事件出口。桌面同步投递，浏览器独立订阅；发起命令的客户端不拥有事件。
#[derive(Clone)]
pub struct EventHub {
    sender: tokio::sync::broadcast::Sender<RuntimeEvent>,
    desktop: Arc<RwLock<Option<EventEmitter>>>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self {
            sender: tokio::sync::broadcast::channel(1024).0,
            desktop: Arc::new(RwLock::new(None)),
        }
    }
}

impl EventHub {
    pub fn set_desktop_sink(&self, sink: EventEmitter) {
        *self.desktop.write().expect("事件接收器锁损坏") = Some(sink);
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<RuntimeEvent> {
        self.sender.subscribe()
    }

    pub fn emitter(&self) -> EventEmitter {
        let hub = self.clone();
        Arc::new(move |event| {
            let sink = hub.desktop.read().ok().and_then(|sink| sink.clone());
            if let Some(sink) = sink {
                sink(event.clone());
            }
            let _ = hub.sender.send(event);
        })
    }
}
