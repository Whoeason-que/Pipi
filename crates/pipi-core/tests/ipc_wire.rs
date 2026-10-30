//! 同一份 JSON 同时经 Rust 序列化和前端 DTO 检查，保护跨宿主 wire 字段。
use pipi_agent::agent_loop::AgentEvent;
use pipi_agent::stats::SessionStats;
use pipi_core::events::{
    AgentEventEnvelope, RuntimeEvent, SessionChangedEnvelope, SessionErrorEnvelope,
    SessionSwitchedEnvelope, StatsEventEnvelope,
};
use pipi_core::runtime::SessionInfo;
use pipi_core::settings::Settings;
use pipi_protocol::{BackgroundTaskSnapshot, Message, Model};
use pipi_venv::agents::AgentDefinition;
use pipi_venv::skills::{AgentSkills, SkillLibrary};
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
    roundtrip::<pipi_core::workspace::WorkspaceListing>(&wire["workspaceListing"]);
    roundtrip::<pipi_core::workspace::WorkspaceFile>(&wire["workspaceFile"]);
    roundtrip::<pipi_core::workspace::WorkspaceChanges>(&wire["workspaceChanges"]);
    roundtrip::<pipi_core::workspace::WorkspaceDiff>(&wire["workspaceDiff"]);
    roundtrip::<AgentDefinition>(&wire["agent"]);
    roundtrip::<pipi_venv::agents::AgentEditableFiles>(&wire["agentFiles"]);
    roundtrip::<Model>(&wire["model"]);
    roundtrip::<pipi_core::runtime::SessionSnapshot>(&wire["snapshot"]);
    roundtrip::<Settings>(&wire["settings"]);
    roundtrip::<pipi_core::remote::RemoteAccessStatus>(&wire["remoteAccess"]);
    roundtrip::<Vec<Message>>(&wire["messages"]);
    roundtrip::<pipi_core::icon_themes::IconThemeList>(&wire["iconThemeList"]);
    roundtrip::<BackgroundTaskSnapshot>(&wire["background"]);
    roundtrip::<SkillLibrary>(&wire["skillLibrary"]);
    roundtrip::<AgentSkills>(&wire["agentSkills"]);
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
    let icon_list = pipi_core::icon_themes::IconThemeList {
        selected: Some("fixture.icons:demo".into()),
        themes: vec![pipi_core::icon_themes::IconThemeInfo {
            id: "fixture.icons:demo".into(),
            label: "Demo".into(),
        }],
    };
    assert_eq!(
        serde_json::to_value(icon_list).unwrap(),
        wire["iconThemeList"]
    );
    let events = vec![
        RuntimeEvent::AgentEvent(AgentEventEnvelope {
            agent_name: "fixture".into(),
            session_id: "session-1".into(),
            run_id: 7,
            event: AgentEvent::AgentEnd { messages: vec![] },
        }),
        RuntimeEvent::AgentEvent(AgentEventEnvelope {
            agent_name: "fixture".into(),
            session_id: "session-1".into(),
            run_id: 7,
            event: AgentEvent::CompactionFailed {
                message: "fixture compaction failure".into(),
            },
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
        RuntimeEvent::PromptQueue(pipi_core::events::PromptQueueEnvelope {
            agent_name: "fixture".into(),
            session_id: "session-1".into(),
            run_id: 7,
            queue: pipi_core::events::PromptQueueSnapshot {
                revision: 1,
                paused: false,
                error: None,
                messages: vec![pipi_core::events::QueuedPrompt {
                    content: None,
                    image_count: 0,
                    id: "queued-1".into(),
                    text: "next task".into(),
                    send_at: 12345,
                }],
            },
        }),
        RuntimeEvent::Notification(pipi_core::notifications::NotificationEnvelope {
            agent_name: "fixture".into(),
            session_id: "session-1".into(),
            run_id: 7,
            id: "notification-1".into(),
            kind: pipi_core::notifications::NotificationKind::Completed,
            title: "Pipi · fixture".into(),
            body: "Reply completed".into(),
        }),
    ];
    assert_eq!(serde_json::to_value(events).unwrap(), wire["events"]);
}
