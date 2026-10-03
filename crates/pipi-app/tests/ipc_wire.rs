//! 同一份 JSON 同时经 Rust 序列化和前端 DTO 检查，保护跨宿主 wire 字段。
use pipi_app::approval::ApprovalRequestEnvelope;
use pipi_app::runtime::{
    AgentEventEnvelope, RuntimeEvent, SessionChangedEnvelope, SessionErrorEnvelope, SessionInfo,
    SessionSwitchedEnvelope, StatsEventEnvelope,
};
use pipi_core::agent_loop::AgentEvent;
use pipi_core::agents::AgentDefinition;
use pipi_core::settings::Settings;
use pipi_core::stats::SessionStats;
use pipi_protocol::{BackgroundTaskSnapshot, Message, Model};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

fn roundtrip<T: DeserializeOwned + Serialize>(value: &Value) {
    let decoded: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), *value);
}

#[test]
fn shared_fixture_preserves_optional_fields_message_roles_and_all_host_event_envelopes() {
    let wire: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/ipc-wire.json")).unwrap();
    roundtrip::<AgentDefinition>(&wire["agent"]);
    roundtrip::<Model>(&wire["model"]);
    roundtrip::<Settings>(&wire["settings"]);
    roundtrip::<Vec<Message>>(&wire["messages"]);
    roundtrip::<BackgroundTaskSnapshot>(&wire["background"]);
    let stats = SessionStats {
        input: 10,
        output: 2,
        cache_read: 3,
        calls: 1,
        ..Default::default()
    };
    assert_eq!(serde_json::to_value(&stats).unwrap(), wire["stats"]);
    let session = SessionInfo {
        agent_name: "fixture".into(),
        session_id: "session-1".into(),
        temporary: false,
        running: true,
        background_tasks: 0,
        run_id: 7,
        model: None,
        is_custom_model: false,
    };
    assert_eq!(serde_json::to_value(session).unwrap(), wire["session"]);
    let events = vec![
        RuntimeEvent::AgentEvent(AgentEventEnvelope {
            agent_name: "fixture".into(),
            session_id: "session-1".into(),
            run_id: 7,
            event: AgentEvent::AgentEnd { messages: vec![] },
        }),
        RuntimeEvent::SessionStats(StatsEventEnvelope {
            agent_name: "fixture".into(),
            session_id: "session-1".into(),
            run_id: 7,
            stats,
        }),
        RuntimeEvent::SessionError(SessionErrorEnvelope {
            agent_name: "fixture".into(),
            session_id: "session-1".into(),
            run_id: 7,
            message: "fixture failure".into(),
        }),
        RuntimeEvent::ApprovalRequest(ApprovalRequestEnvelope {
            agent_name: "fixture".into(),
            session_id: "session-1".into(),
            run_id: 7,
            request_id: "approval-1".into(),
            command: "git status".into(),
            missing: vec!["git status".into()],
        }),
        RuntimeEvent::SessionSwitched(SessionSwitchedEnvelope {
            agent_name: "fixture".into(),
            session_id: "session-1".into(),
            run_id: 7,
            to_session_id: "session-2".into(),
            archived: true,
        }),
        RuntimeEvent::SessionChanged(SessionChangedEnvelope {
            agent_name: "fixture".into(),
            session_id: "child-1".into(),
            run_id: 8,
        }),
    ];
    assert_eq!(serde_json::to_value(events).unwrap(), wire["events"]);
}
