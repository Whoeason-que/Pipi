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
        sandbox: pipi_tools::permissions::SandboxMode::DangerFullAccess,
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
