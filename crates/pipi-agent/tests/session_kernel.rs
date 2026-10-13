//! 只用公开 API 与内存端口验证单会话生命周期，不装配 core 或具体能力。
use async_trait::async_trait;
use pipi_agent::agent_loop::ToolExecutionMode;
use pipi_agent::session::*;
use pipi_context::retry::RetryPolicy;
use pipi_protocol::provider::{EventStream, Provider};
use pipi_protocol::session::*;
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput, ToolRegistry};
use pipi_protocol::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Ledger {
    entries: Mutex<Vec<SessionEntry>>,
    fail: AtomicBool,
    closed: AtomicBool,
    /// 提交点被调用的次数（git 载体下这就是「一轮一个 commit」的落地次数）。
    commits: AtomicUsize,
}

struct MemoryLog {
    data: Arc<Ledger>,
    ids: Vec<String>,
    tip: Option<String>,
    id: String,
    temporary: bool,
    fail_compaction: bool,
    trace: Option<Arc<Mutex<Vec<String>>>>,
}

impl MemoryLog {
    fn new(data: Arc<Ledger>) -> Self {
        let entries = data.entries.lock().unwrap().clone();
        let ids = replay(&active_path(&entries)).1;
        let tip = entries.last().map(|e| e.id.clone());
        data.closed.store(false, Ordering::SeqCst);
        Self {
            data,
            ids,
            tip,
            id: "memory-session".into(),
            temporary: true,
            fail_compaction: false,
            trace: None,
        }
    }
    fn append(&mut self, kind: EntryKind) -> std::io::Result<String> {
        if self.data.fail.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("injected write failure"));
        }
        let mut entries = self.data.entries.lock().unwrap();
        let id = format!("entry-{}", entries.len());
        let entry = SessionEntry {
            id: id.clone(),
            parent_id: self.tip.clone(),
            seq: entries.len() as u64,
            timestamp: 0,
            kind,
        };
        entries.push(entry);
        self.tip = Some(id.clone());
        self.ids = replay(&active_path(&entries)).1;
        Ok(id)
    }
}

impl Drop for MemoryLog {
    fn drop(&mut self) {
        self.data.closed.store(true, Ordering::SeqCst);
        if let Some(trace) = &self.trace {
            trace.lock().unwrap().push(format!("close:{}", self.id));
        }
    }
}

impl SessionLog for MemoryLog {
    fn session_id(&self) -> &str {
        &self.id
    }
    fn is_temporary(&self) -> bool {
        self.temporary
    }
    fn tip_id(&self) -> Option<&str> {
        self.tip.as_deref()
    }
    fn message_ids(&self) -> &[String] {
        &self.ids
    }
    fn mark_env_recorded(&mut self) {}
    fn append_message(&mut self, message: &Message) -> std::io::Result<String> {
        self.append(EntryKind::Message {
            message: message.clone(),
        })
    }
    fn append_custom(&mut self, custom_type: &str) -> std::io::Result<String> {
        self.append(EntryKind::Custom {
            custom_type: custom_type.into(),
        })
    }
    fn append_model_change(&mut self, model: &Model) -> std::io::Result<String> {
        self.append(EntryKind::ModelChange {
            provider: model.clone(),
        })
    }
    fn append_env(&mut self, declared: Vec<EnvDeclared>) -> std::io::Result<Option<String>> {
        self.append(EntryKind::Env { declared }).map(Some)
    }
    fn append_compaction(&mut self, r: &CompactionRecord<'_>) -> std::io::Result<String> {
        if self.fail_compaction {
            return Err(std::io::Error::other("injected compaction failure"));
        }
        self.append(EntryKind::Compaction {
            summary: r.summary.into(),
            keep_from_entry: r.keep_from_entry.into(),
            source_tip: r.source_tip.into(),
            strategy: r.strategy.into(),
            usage: r.usage,
        })
    }
    fn commit_point(&mut self) -> std::io::Result<()> {
        self.data.commits.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct ScriptedProvider {
    hang: bool,
    panic: bool,
}
#[async_trait]
impl Provider for ScriptedProvider {
    async fn stream(
        &self,
        _: &Model,
        _: &Context,
        _: &StreamOptions,
        abort: AbortSignal,
    ) -> EventStream {
        assert!(!self.panic, "injected provider panic");
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let hang = self.hang;
        tokio::spawn(async move {
            if hang {
                abort.wait_aborted().await;
                return;
            }
            let message = Message::assistant_text("answer", "mock");
            tx.send(StreamEvent::Done {
                reason: StopReason::Stop,
                usage: Usage::default(),
                message: Box::new(message),
            })
            .await
            .unwrap();
        });
        rx
    }
}

fn model() -> Model {
    Model {
        id: "mock".into(),
        name: "Mock".into(),
        api: Api::OpenAICompletions,
        base_url: String::new(),
        max_tokens: 32,
        context_window: 0,
    }
}
fn storage(data: Arc<Ledger>) -> SessionStorage {
    SessionStorage {
        log: Box::new(MemoryLog::new(data)),
        repo: None,
    }
}
fn session(data: Arc<Ledger>) -> AgentSession {
    AgentSession::create(storage(data), Some(model()), None, AbortSignal::new()).unwrap()
}
fn request(hang: bool, panic: bool) -> RunRequest {
    RunRequest {
        prompt: "question".into(),
        system_prompt: String::new(),
        model: model(),
        provider: Arc::new(ScriptedProvider { hang, panic }),
        tools: Arc::new(ToolRegistry::new(Vec::new())),
        tool_context: ToolContext {
            workspace: std::env::temp_dir(),
            memory_dir: None,
            read_roots: Vec::new(),
            permissions: Arc::new(Default::default()),
            sandbox: pipi_protocol::permissions::SandboxMode::DangerFullAccess,
            resolved_env: Arc::new(BTreeMap::new()),
            abort: AbortSignal::new(),
            approver: None,
        },
        options: StreamOptions::default(),
        retry: RetryPolicy::NONE,
        tool_execution: ToolExecutionMode::Parallel,
        budget: None,
        compaction: None,
        timeout: None,
    }
}

#[tokio::test]
async fn input_is_durable_before_poll_and_history_replays_once() {
    let data = Arc::new(Ledger::default());
    let session = session(data.clone());
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let prepared = session
        .reserve_run(AbortPolicy::Reset)
        .unwrap()
        .prepare(
            request(false, false),
            SessionHooks {
                emit: Arc::new(move |e| captured.lock().unwrap().push(e)),
                rekey: None,
            },
        )
        .unwrap();
    assert_eq!(data.entries.lock().unwrap().len(), 1);
    assert!(session.reserve_run(AbortPolicy::Reset).is_err());
    let outcome = prepared.run().await.unwrap();
    assert_eq!(outcome.messages.len(), 1);
    assert!(!session.snapshot().unwrap().running);
    let expected = session.messages().await;
    assert_eq!(expected.len(), 2);
    assert!(matches!(
        events.lock().unwrap().last().unwrap().kind,
        SessionEventKind::Agent(pipi_agent::agent_loop::AgentEvent::AgentEnd { .. })
    ));
    drop(session);
    assert!(data.closed.load(Ordering::SeqCst));
    let entries = data.entries.lock().unwrap().clone();
    let restored =
        AgentSession::restore(storage(data), entries, Some(model()), AbortSignal::new()).unwrap();
    assert_eq!(restored.messages().await, expected);
}

#[test]
fn failed_prepare_and_unpolled_drop_release_the_slot() {
    let data = Arc::new(Ledger::default());
    let session = session(data.clone());
    data.fail.store(true, Ordering::SeqCst);
    assert!(
        session
            .reserve_run(AbortPolicy::Reset)
            .unwrap()
            .prepare(request(false, false), SessionHooks::default())
            .is_err()
    );
    assert!(!session.snapshot().unwrap().running);
    data.fail.store(false, Ordering::SeqCst);
    let prepared = session
        .reserve_run(AbortPolicy::Reset)
        .unwrap()
        .prepare(request(false, false), SessionHooks::default())
        .unwrap();
    drop(prepared);
    assert!(!session.snapshot().unwrap().running);
}

#[tokio::test]
async fn timeout_is_persisted_and_preserved_abort_is_not_reset() {
    let data = Arc::new(Ledger::default());
    let session = session(data.clone());
    let mut req = request(true, false);
    req.timeout = Some(RunTimeout {
        duration: std::time::Duration::from_millis(5),
        message: "test timeout".into(),
    });
    let outcome = session
        .reserve_run(AbortPolicy::Reset)
        .unwrap()
        .prepare(req, SessionHooks::default())
        .unwrap()
        .run()
        .await
        .unwrap();
    assert!(matches!(
        outcome.messages.last(),
        Some(Message::Assistant {
            stop_reason: StopReason::Error,
            ..
        })
    ));
    assert_eq!(session.messages().await.len(), 2);
    session.stop().unwrap();
    let reservation = session.reserve_run(AbortPolicy::Preserve).unwrap();
    assert!(session.abort_signal().is_aborted());
    drop(reservation);
    let reservation = session.reserve_run(AbortPolicy::Reset).unwrap();
    assert!(!session.abort_signal().is_aborted());
    drop(reservation);
}

#[tokio::test]
async fn provider_panic_releases_prepared_run() {
    let session = session(Arc::new(Ledger::default()));
    let prepared = session
        .reserve_run(AbortPolicy::Reset)
        .unwrap()
        .prepare(request(false, true), SessionHooks::default())
        .unwrap();
    assert!(tokio::spawn(prepared.run()).await.is_err());
    assert!(!session.snapshot().unwrap().running);
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Fault {
    None,
    Fork,
    ForkAppend,
    Append,
    Archive,
}

struct MemoryRepo {
    ledgers: Mutex<BTreeMap<String, Arc<Ledger>>>,
    trace: Arc<Mutex<Vec<String>>>,
    fault: Fault,
}
impl MemoryRepo {
    fn seeded(fault: Fault) -> Arc<Self> {
        let data = Arc::new(Ledger::default());
        let mut log = MemoryLog::new(data.clone());
        for _ in 0..8 {
            log.append_message(&Message::user_text("old question ".repeat(200)))
                .unwrap();
            let mut answer = Message::assistant_text("old answer ".repeat(200), "mock");
            if let Message::Assistant { usage, .. } = &mut answer {
                *usage = Usage {
                    input: 100,
                    output: 50,
                    ..Default::default()
                };
            }
            log.append_message(&answer).unwrap();
        }
        drop(log);
        Arc::new(Self {
            ledgers: Mutex::new(BTreeMap::from([("source".into(), data)])),
            trace: Arc::new(Mutex::new(Vec::new())),
            fault,
        })
    }
    fn log(&self, id: &str) -> Box<dyn SessionLog> {
        let data = self.ledgers.lock().unwrap().get(id).unwrap().clone();
        let mut log = MemoryLog::new(data);
        log.id = id.into();
        log.temporary = false;
        log.trace = Some(self.trace.clone());
        log.fail_compaction =
            self.fault == Fault::Append || (id == "fork" && self.fault == Fault::ForkAppend);
        Box::new(log)
    }
    fn session(self: &Arc<Self>) -> AgentSession {
        AgentSession::restore(
            SessionStorage {
                log: self.log("source"),
                repo: Some(self.clone()),
            },
            self.entries("source").unwrap(),
            Some(model()),
            AbortSignal::new(),
        )
        .unwrap()
    }
}
impl SessionRepo for MemoryRepo {
    fn create(&self) -> Result<Box<dyn SessionLog>, String> {
        Err("unused".into())
    }
    fn open(&self, id: &str) -> Result<Box<dyn SessionLog>, String> {
        Ok(self.log(id))
    }
    fn exists(&self, id: &str) -> Result<bool, String> {
        Ok(self.ledgers.lock().unwrap().contains_key(id))
    }
    fn entries(&self, id: &str) -> Result<Vec<SessionEntry>, String> {
        Ok(self
            .ledgers
            .lock()
            .unwrap()
            .get(id)
            .unwrap()
            .entries
            .lock()
            .unwrap()
            .clone())
    }
    fn list(&self) -> Vec<SessionSummary> {
        Vec::new()
    }
    fn list_archived(&self) -> Result<Vec<SessionSummary>, String> {
        Ok(Vec::new())
    }
    fn fork(&self, id: &str, _: Option<&str>) -> Result<Box<dyn SessionLog>, String> {
        if self.fault == Fault::Fork {
            return Err("injected fork failure".into());
        }
        let data = Arc::new(Ledger::default());
        *data.entries.lock().unwrap() = self.entries(id)?;
        self.ledgers.lock().unwrap().insert("fork".into(), data);
        Ok(self.log("fork"))
    }
    fn archive(&self, id: &str) -> Result<(), String> {
        assert!(
            self.ledgers
                .lock()
                .unwrap()
                .get(id)
                .unwrap()
                .closed
                .load(Ordering::SeqCst),
            "old writer must close before archive"
        );
        self.trace.lock().unwrap().push(format!("archive:{id}"));
        if self.fault == Fault::Archive {
            Err("injected archive failure".into())
        } else {
            Ok(())
        }
    }
    fn restore(&self, _: &str) -> Result<(), String> {
        Err("unused".into())
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.ledgers.lock().unwrap().remove(id);
        self.trace.lock().unwrap().push(format!("delete:{id}"));
        Ok(())
    }
    fn delete_archived(&self, _: &str) -> Result<(), String> {
        Err("unused".into())
    }
}

#[derive(Default)]
struct SummaryProvider {
    options: Mutex<Vec<StreamOptions>>,
}
#[async_trait]
impl Provider for SummaryProvider {
    async fn stream(
        &self,
        _: &Model,
        _: &Context,
        options: &StreamOptions,
        _: AbortSignal,
    ) -> EventStream {
        self.options.lock().unwrap().push(options.clone());
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let usage = Usage {
            input: 7,
            output: 3,
            ..Default::default()
        };
        tx.send(StreamEvent::Done {
            reason: StopReason::Stop,
            usage,
            message: Box::new(Message::assistant_text("short summary", "mock")),
        })
        .await
        .unwrap();
        rx
    }
}
fn compact_request(provider: Arc<dyn Provider>, fork: bool) -> CompactionRequest {
    let mut budget = pipi_context::compaction::Budget::from_window(4096, 75);
    budget.keep_recent = 64;
    CompactionRequest {
        model: model(),
        provider,
        options: StreamOptions {
            auto_protocol: false,
            ..Default::default()
        },
        retry: RetryPolicy::NONE,
        budget,
        policy: CompactionPolicy {
            fork_before_compact: fork,
            archive_original: true,
        },
    }
}

#[tokio::test]
async fn compaction_faults_preserve_ledger_identity_and_reopen_totals() {
    for fault in [
        Fault::None,
        Fault::Fork,
        Fault::ForkAppend,
        Fault::Append,
        Fault::Archive,
    ] {
        let repo = MemoryRepo::seeded(fault);
        let session = Arc::new(repo.session());
        let before = session.messages().await;
        let events = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(SummaryProvider::default());
        let event_sink = events.clone();
        let trace = repo.trace.clone();
        let observed = session.clone();
        let hooks = SessionHooks {
            emit: Arc::new(move |event| event_sink.lock().unwrap().push(event)),
            rekey: Some(Arc::new(move |old, new| {
                let snapshot = observed.snapshot().unwrap();
                assert!(snapshot.running);
                assert_eq!(snapshot.session_id, new);
                trace.lock().unwrap().push(format!("rekey:{old}:{new}"));
            })),
        };
        let outcome = session
            .reserve_run(AbortPolicy::Reset)
            .unwrap()
            .prepare_compaction(
                compact_request(provider.clone(), fault != Fault::Append),
                hooks,
            )
            .run()
            .await
            .unwrap();
        assert!(!session.snapshot().unwrap().running);
        let messages = session.messages().await;
        let events = events.lock().unwrap().clone();
        let options = provider.options.lock().unwrap().clone();
        assert_eq!(options.len(), 1, "{fault:?}");
        assert!(!options[0].auto_protocol);
        assert_eq!(options[0].session_id.as_deref(), Some("source"));
        assert_eq!(options[0].timeout_secs, 300);
        if fault == Fault::Append {
            assert_eq!(messages, before);
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e.kind, SessionEventKind::Error(_)))
            );
        } else {
            assert!(messages.len() < before.len(), "{fault:?}");
            assert_eq!(session.stats().unwrap().input, 807);
            assert_eq!(session.stats().unwrap().calls, 9);
            assert_eq!(session.stats().unwrap().context_used, Some(100));
        }
        let switched = matches!(fault, Fault::None | Fault::Archive);
        assert_eq!(outcome.session_id, if switched { "fork" } else { "source" });
        if switched {
            let trace = repo.trace.lock().unwrap();
            assert_eq!(
                &trace[..3],
                ["close:source", "archive:source", "rekey:source:fork"]
            );
            let switched_index = events
                .iter()
                .position(|e| matches!(e.kind, SessionEventKind::Switched { .. }))
                .unwrap();
            assert_eq!(events[switched_index].session_id, "source");
            assert!(
                events[switched_index + 1..]
                    .iter()
                    .all(|e| e.session_id == "fork")
            );
        } else {
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e.kind, SessionEventKind::Switched { .. }))
            );
            assert!(!repo.exists("fork").unwrap());
        }
        assert!(matches!(
            events.last().unwrap().kind,
            SessionEventKind::Agent(pipi_agent::agent_loop::AgentEvent::AgentEnd { .. })
        ));
        let stats = session.stats().unwrap();
        drop(session);
        let restored = AgentSession::restore(
            SessionStorage {
                log: repo.open(&outcome.session_id).unwrap(),
                repo: Some(repo.clone()),
            },
            repo.entries(&outcome.session_id).unwrap(),
            Some(model()),
            AbortSignal::new(),
        )
        .unwrap();
        assert_eq!(
            without_summary_timestamp(restored.messages().await),
            without_summary_timestamp(messages)
        );
        assert_eq!(restored.stats().unwrap().input, stats.input);
        assert_eq!(restored.stats().unwrap().output, stats.output);
        assert_eq!(restored.stats().unwrap().calls, stats.calls);
        assert_eq!(restored.stats().unwrap().context_used, stats.context_used);
    }
}

#[tokio::test]
async fn temporary_compaction_only_updates_injected_memory_log() {
    let repo = MemoryRepo::seeded(Fault::None);
    let data = repo.ledgers.lock().unwrap().get("source").unwrap().clone();
    let entries = data.entries.lock().unwrap().clone();
    let session = AgentSession::restore(
        storage(data.clone()),
        entries,
        Some(model()),
        AbortSignal::new(),
    )
    .unwrap();
    let outcome = session
        .reserve_run(AbortPolicy::Reset)
        .unwrap()
        .prepare_compaction(
            compact_request(Arc::new(SummaryProvider::default()), true),
            SessionHooks::default(),
        )
        .run()
        .await
        .unwrap();
    assert_eq!(outcome.session_id, "memory-session");
    assert!(matches!(
        data.entries.lock().unwrap().last().unwrap().kind,
        EntryKind::Compaction { .. }
    ));
    assert_eq!(
        without_summary_timestamp(session.messages().await),
        without_summary_timestamp(rebuild_messages(&active_path(
            &data.entries.lock().unwrap()
        )))
    );
}

struct ToolProvider;
#[async_trait]
impl Provider for ToolProvider {
    async fn stream(
        &self,
        _: &Model,
        _: &Context,
        _: &StreamOptions,
        _: AbortSignal,
    ) -> EventStream {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let mut message = Message::assistant_text("", "mock");
        if let Message::Assistant {
            content,
            stop_reason,
            ..
        } = &mut message
        {
            *content = vec![
                ContentBlock::ToolCall {
                    id: "ready-call".into(),
                    name: "ready".into(),
                    arguments: serde_json::json!({}),
                },
                ContentBlock::ToolCall {
                    id: "wait-call".into(),
                    name: "wait".into(),
                    arguments: serde_json::json!({}),
                },
            ];
            *stop_reason = StopReason::ToolUse;
        }
        tx.send(StreamEvent::Done {
            reason: StopReason::ToolUse,
            usage: Usage::default(),
            message: Box::new(message),
        })
        .await
        .unwrap();
        rx
    }
}
struct SlowTool;
#[async_trait]
impl AgentTool for SlowTool {
    fn name(&self) -> &'static str {
        "wait"
    }
    fn description(&self) -> String {
        "Wait forever".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object"})
    }
    async fn execute(
        &self,
        _: &ToolContext,
        _: &serde_json::Value,
        _: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        std::future::pending().await
    }
}
#[tokio::test]
async fn tool_timeout_settles_pairing_before_terminal_and_replays_once() {
    let data = Arc::new(Ledger::default());
    let session = session(data.clone());
    let mut req = request(false, false);
    req.provider = Arc::new(ToolProvider);
    req.tools = Arc::new(ToolRegistry::new(vec![
        Arc::new(ReadyTool),
        Arc::new(SlowTool),
    ]));
    req.tool_execution = ToolExecutionMode::Sequential;
    req.timeout = Some(RunTimeout {
        duration: std::time::Duration::from_millis(20),
        message: "tool timeout".into(),
    });
    session
        .reserve_run(AbortPolicy::Reset)
        .unwrap()
        .prepare(req, SessionHooks::default())
        .unwrap()
        .run()
        .await
        .unwrap();
    let messages = session.messages().await;
    assert_eq!(messages.len(), 5);
    assert!(
        matches!(&messages[3], Message::ToolResult { tool_call_id, is_error: true, .. } if tool_call_id == "wait-call")
    );
    assert!(matches!(
        messages[4],
        Message::Assistant {
            stop_reason: StopReason::Error,
            ..
        }
    ));
    assert_eq!(
        messages,
        rebuild_messages(&active_path(&data.entries.lock().unwrap()))
    );
    assert!(!session.snapshot().unwrap().running);
}

#[tokio::test]
async fn restore_repairs_only_missing_results_in_a_partial_tool_batch() {
    let data = Arc::new(Ledger::default());
    let mut log = MemoryLog::new(data.clone());
    let mut stream = ToolProvider
        .stream(
            &model(),
            &Context {
                system_prompt: None,
                messages: Vec::new(),
                tools: Vec::new(),
            },
            &StreamOptions::default(),
            AbortSignal::new(),
        )
        .await;
    let Some(StreamEvent::Done { message, .. }) = stream.recv().await else {
        panic!("expected tool turn")
    };
    log.append_message(&message).unwrap();
    log.append_message(&Message::ToolResult {
        tool_call_id: "ready-call".into(),
        tool_name: "ready".into(),
        content: vec![ToolResultContent::Text {
            text: "done".into(),
        }],
        details: None,
        is_error: false,
        timestamp: 0,
    })
    .unwrap();
    drop(log);
    let entries = data.entries.lock().unwrap().clone();
    let restored = AgentSession::restore(
        storage(data.clone()),
        entries,
        Some(model()),
        AbortSignal::new(),
    )
    .unwrap();
    let messages = restored.messages().await;
    assert_eq!(messages.len(), 3);
    assert!(
        matches!(&messages[2], Message::ToolResult { tool_call_id, is_error: true, .. } if tool_call_id == "wait-call")
    );
    drop(restored);
    let entries = data.entries.lock().unwrap().clone();
    let reopened = AgentSession::restore(
        storage(data.clone()),
        entries,
        Some(model()),
        AbortSignal::new(),
    )
    .unwrap();
    assert_eq!(reopened.messages().await, messages);
    assert_eq!(data.entries.lock().unwrap().len(), 3);
}

// 摘要是回放时生成的 user 投影；其合成时间戳不属于持久化内容。
fn without_summary_timestamp(mut messages: Vec<Message>) -> Vec<Message> {
    for message in &mut messages {
        if summary_text(message).is_some()
            && let Message::User { timestamp, .. } = message
        {
            *timestamp = 0;
        }
    }
    messages
}

struct ReadyTool;
#[async_trait]
impl AgentTool for ReadyTool {
    fn name(&self) -> &'static str {
        "ready"
    }
    fn description(&self) -> String {
        "Reply immediately".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object"})
    }
    async fn execute(
        &self,
        _: &ToolContext,
        _: &serde_json::Value,
        _: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        Ok(ToolOutput::text("done"))
    }
}

#[tokio::test]
async fn unpolled_prepared_input_and_startup_failure_remain_coherent() {
    let data = Arc::new(Ledger::default());
    let session = session(data.clone());
    let prepared = session
        .reserve_run(AbortPolicy::Reset)
        .unwrap()
        .prepare(request(false, false), SessionHooks::default())
        .unwrap();
    drop(prepared.run());
    assert!(!session.snapshot().unwrap().running);
    session
        .reserve_run(AbortPolicy::Reset)
        .unwrap()
        .fail_start("next question", "mock", "injected startup failure".into())
        .unwrap();
    assert_eq!(
        session.messages().await,
        rebuild_messages(&active_path(&data.entries.lock().unwrap()))
    );
    assert_eq!(session.stats().unwrap().calls, 1);
}

#[tokio::test]
async fn final_event_runs_after_task_releases_its_writer_references() {
    let data = Arc::new(Ledger::default());
    let session = session(data.clone());
    let observed = data.clone();
    let prepared = session.reserve_run(AbortPolicy::Reset).unwrap()
        .prepare(request(false, false), SessionHooks {
            emit: Arc::new(move |event| {
                if matches!(event.kind, SessionEventKind::Agent(pipi_agent::agent_loop::AgentEvent::AgentEnd { .. })) {
                    assert!(observed.closed.load(Ordering::SeqCst), "completion must permit the host to release/archive the session immediately");
                }
            }),
            rekey: None,
        }).unwrap();
    drop(session);
    prepared.run().await.unwrap();
    assert!(data.closed.load(Ordering::SeqCst));
}

/// 提交点必须落在 AgentEnd 之前（以及结构上必然在运行守卫释放之前）：git 载体
/// 在轮末才落盘，「running 已释放」不等于「历史已落盘」—— 前端收到 AgentEnd 后
/// 立刻发的新一轮会与未完成的提交竞争同一分支。
#[tokio::test]
async fn the_turn_is_committed_before_the_final_event() {
    let data = Arc::new(Ledger::default());
    let session = session(data.clone());
    // 会话创建自己也会提交一次（空会话可见），这里只盯本轮
    let baseline = data.commits.load(Ordering::SeqCst);
    let observed = data.clone();
    let prepared = session
        .reserve_run(AbortPolicy::Reset)
        .unwrap()
        .prepare(
            request(false, false),
            SessionHooks {
                emit: Arc::new(move |event| {
                    if matches!(
                        event.kind,
                        SessionEventKind::Agent(
                            pipi_agent::agent_loop::AgentEvent::AgentEnd { .. }
                        )
                    ) {
                        assert!(
                            observed.commits.load(Ordering::SeqCst) > baseline,
                            "AgentEnd 之前必须已经提交本轮"
                        );
                    }
                }),
                rekey: None,
            },
        )
        .unwrap();
    drop(session);
    prepared.run().await.unwrap();
    assert!(data.commits.load(Ordering::SeqCst) > baseline);
}
