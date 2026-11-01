//! Agent 统计命令的跨会话读侧回归：从真实 git 会话分支读回并聚合。

use pipi_core::commands::{AgentStatsArgs, agent_stats};
use pipi_protocol::session::SessionRepo;
use pipi_protocol::{Message, StopReason, Usage};
use pipi_session::GitSessionRepo;

#[test]
fn agent_stats_aggregates_committed_sessions() {
    let _guard = pipi_test_support::HOME_LOCK.lock().unwrap();
    let home = pipi_test_support::temp_dir("pipi-agent-stats");
    let previous = std::env::var("HOME").unwrap_or_default();
    pipi_test_support::set_env_var("HOME", &home);

    pipi_venv::agents::create_agent("stats-test", "stats", None, None, None, None).unwrap();
    let def = pipi_venv::agents::load_agent("stats-test").unwrap();
    let sessions_dir = def.sessions_dir().unwrap();
    let agent_dir = sessions_dir.parent().unwrap();
    let repo = GitSessionRepo::new(agent_dir.join("sessions.git"));

    let mut log = repo.create().unwrap();
    log.append_message(&Message::user_text("hello")).unwrap();
    log.append_message(&Message::Assistant {
        content: Vec::new(),
        api: "openai-completions".into(),
        provider: "test".into(),
        model: "priced-model".into(),
        usage: Usage {
            input: 1_000,
            output: 200,
            cache_read: 500,
            cache_write: 0,
            total_tokens: 1_700,
        },
        stop_reason: StopReason::Stop,
        error_message: None,
        error_kind: None,
        timestamp: 10,
        duration_ms: Some(1_500),
    })
    .unwrap();
    log.append_message(&Message::ToolResult {
        tool_call_id: "call-1".into(),
        tool_name: "read".into(),
        content: Vec::new(),
        is_error: true,
        details: None,
        timestamp: 11,
    })
    .unwrap();
    log.commit_point().unwrap();
    drop(log);

    let stats = agent_stats(AgentStatsArgs {
        agent_name: "stats-test".into(),
        range_days: None,
    })
    .expect("stats should read committed sessions");

    assert_eq!(stats.agent_name, "stats-test");
    assert_eq!(stats.totals.sessions, 1);
    assert_eq!(stats.totals.user_messages, 1);
    assert_eq!(stats.totals.assistant_calls, 1);
    assert_eq!(stats.totals.input, 1_000);
    assert_eq!(stats.totals.output, 200);
    assert_eq!(stats.totals.cache_read, 500);
    assert_eq!(stats.totals.tool_calls, 1);
    assert_eq!(stats.totals.tool_errors, 1);
    assert_eq!(stats.sessions.len(), 1);
    assert_eq!(stats.tools[0].name, "read");
    assert!(!stats.sessions_truncated);

    pipi_test_support::set_env_var("HOME", &previous);
    let _ = std::fs::remove_dir_all(&home);
}
