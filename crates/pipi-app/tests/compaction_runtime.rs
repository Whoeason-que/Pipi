//! 压缩的端到端回归：真实 runtime + 本地 SSE 端点。
//!
//! 覆盖三件事（都是这次改动新增/修复的行为）：
//! 1. turn 边界真的会压缩，并把 `keep_from_entry` + 摘要 usage 落盘；
//! 2. **重开会话时保留区间的原文还在**（此前回放只留摘要，那段精确原文丢失）；
//! 3. 摘要调用的用量进累计账本，但不改写「当前上下文占用」口径。
//!
//! 端点按请求体分流：带 `<conversation>` 的是摘要请求，否则是普通对话请求。
//! 本测试自己改写 HOME（独立测试进程），不碰用户真实的 `~/.pipi`。

use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipi_app::runtime::{EventEmitter, RuntimeEvent, RuntimeState};
use pipi_core::agents;
use pipi_core::session::{load_session, EntryKind, SessionWriter};
use pipi_core::settings::{save_settings, ProviderConfig, Settings, Theme};
use pipi_core::types::{Api, Message, Model};

/// 本文件的用例都改写进程级 `HOME`，必须串行执行（同 `session_repair.rs`）。
static HOME_LOCK: Mutex<()> = Mutex::new(());

/// 摘要请求上报的用量（数字刻意与普通请求拉开，便于断言账本）
const SUMMARY_INPUT: u64 = 50_000;
const SUMMARY_OUTPUT: u64 = 700;
/// 普通请求上报的用量
const CHAT_INPUT: u64 = 120;
const CHAT_OUTPUT: u64 = 8;

fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(1) => {
                buffer.push(byte[0]);
                if buffer.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            _ => return String::new(),
        }
    }
    let headers = String::from_utf8_lossy(&buffer).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    let _ = stream.read_exact(&mut body);
    String::from_utf8_lossy(&body).to_string()
}

fn sse_frame(text: &str, usage: (u64, u64)) -> String {
    let (input, output) = usage;
    let total = input + output;
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!(
        "{}\n{}\n{}",
        format_args!(
            r#"data: {{"id":"c1","object":"chat.completion.chunk","created":1,"model":"mock","choices":[{{"index":0,"delta":{{"role":"assistant","content":"{escaped}"}},"finish_reason":null}}]}}"#
        ),
        format_args!(
            r#"data: {{"id":"c1","object":"chat.completion.chunk","created":1,"model":"mock","choices":[{{"index":0,"delta":{{}},"finish_reason":"stop"}}]}}"#
        ),
        format_args!(
            r#"data: {{"id":"c1","object":"chat.completion.chunk","created":1,"model":"mock","choices":[],"usage":{{"prompt_tokens":{input},"completion_tokens":{output},"total_tokens":{total}}}}}"#
        )
    )
}

/// 端点：回一个内容分片后保持静默 —— 让一轮运行停在半途（用于并发守卫测试）。
fn spawn_stalling_endpoint() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for mut stream in listener.incoming().flatten() {
            // 读请求头与会话体（不解析，只要读干净）
            let _ = read_request(&mut stream);
            let chunk = r#"{"id":"c1","object":"chat.completion.chunk","model":"mock","choices":[{"index":0,"delta":{"content":"开头"},"finish_reason":null}]}"#;
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n");
            let _ = stream.write_all(format!("data: {chunk}\n\n").as_bytes());
            let _ = stream.flush();
            held.push(stream);
        }
    });
    addr
}

/// 启动 mock 端点：摘要请求回摘要正文，普通请求回一句短答；记录收到的请求数。
fn spawn_endpoint() -> (std::net::SocketAddr, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let chats = Arc::new(AtomicUsize::new(0));
    let summaries = Arc::new(AtomicUsize::new(0));
    let (chat_counter, summary_counter) = (chats.clone(), summaries.clone());

    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let body = read_request(&mut stream);
            let (payload, usage) = if body.contains("<conversation>") {
                summary_counter.fetch_add(1, Ordering::SeqCst);
                (
                    "## Goal\n把 crate 过一遍\n\n## Progress\n### Done\n前 40 轮已跑完".to_string(),
                    (SUMMARY_INPUT, SUMMARY_OUTPUT),
                )
            } else {
                chat_counter.fetch_add(1, Ordering::SeqCst);
                ("收到。".to_string(), (CHAT_INPUT, CHAT_OUTPUT))
            };
            let frames = sse_frame(&payload, usage);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n");
            for frame in frames.split('\n') {
                let _ = stream.write_all(format!("{frame}\n\n").as_bytes());
            }
            let _ = stream.write_all(b"data: [DONE]\n\n");
            let _ = stream.flush();
        }
    });
    (addr, chats, summaries)
}

fn temp_home() -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!(
        "pipi-compaction-e2e-{}-{}",
        std::process::id(),
        pipi_core::session::new_id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("创建临时 HOME");
    std::env::set_var("HOME", &home);
    home
}

fn fixture(
    agent: &str,
    base_url: &str,
    context_window: u64,
    compaction: pipi_core::settings::CompactionSettings,
) {
    let model = Model {
        id: "compaction-model".into(),
        name: "Compaction Model".into(),
        api: Api::OpenAICompletions,
        base_url: base_url.into(),
        max_tokens: 256,
        context_window,
    };
    agents::create_agent(
        agent,
        "压缩端到端",
        None,
        None,
        Some("compaction-model"),
        Some(model),
    )
    .expect("播种测试 Agent");
    save_settings(&Settings {
        theme: Theme::Dark,
        providers: vec![ProviderConfig {
            id: "local-mock".into(),
            name: "Local Mock".into(),
            api: Api::OpenAICompletions,
            base_url: base_url.into(),
            env_key: None,
            api_key: Some("test-key".into()),
        }],
        default_provider_id: None,
        retry: Default::default(),
        compaction,
    })
    .expect("写入测试设置");
}

/// 该 Agent 当前打开的会话 id（测试里一般只有一条）。
fn open_session_id(state: &RuntimeState, agent_name: &str) -> String {
    state
        .session_infos()
        .expect("读取会话列表")
        .into_iter()
        .find(|info| info.agent_name == agent_name)
        .unwrap_or_else(|| panic!("{agent_name} 应当有打开的会话"))
        .session_id
}

/// 往会话里预置一段历史（每轮：user + assistant(tool_call) + toolResult）。
/// 尾部留在 user 边界上，于是压缩会产生非空的保留区间。
fn seed_history(sessions_dir: &std::path::Path) -> (String, String) {
    seed_history_turns(sessions_dir, 40)
}

fn seed_history_turns(sessions_dir: &std::path::Path, turns: u32) -> (String, String) {
    let mut writer = SessionWriter::create(sessions_dir).expect("创建会话文件");
    for turn in 0..turns {
        writer
            .append_message(&Message::user_text(format!(
                "第 {turn} 轮：看文件并跑测试 {}",
                "补".repeat(500)
            )))
            .unwrap();
        let call_id = format!("call_{turn}");
        writer
            .append_message(&Message::Assistant {
                content: vec![
                    pipi_core::types::ContentBlock::Text {
                        text: format!("先读文件（第 {turn} 轮）。"),
                    },
                    pipi_core::types::ContentBlock::ToolCall {
                        id: call_id.clone(),
                        name: "read".into(),
                        arguments: serde_json::json!({ "path": format!("src/m{turn}.rs") }),
                    },
                ],
                api: String::new(),
                provider: String::new(),
                model: "compaction-model".into(),
                usage: Default::default(),
                stop_reason: pipi_core::types::StopReason::ToolUse,
                error_message: None,
                timestamp: 0,
                duration_ms: None,
            })
            .unwrap();
        writer
            .append_message(&Message::ToolResult {
                tool_call_id: call_id,
                tool_name: "read".into(),
                content: vec![pipi_core::types::ToolResultContent::Text {
                    text: format!("// m{turn}\n{}", "fn f() {}\n".repeat(400)),
                }],
                is_error: false,
                details: None,
                timestamp: 0,
            })
            .unwrap();
    }
    let session_id = writer
        .path()
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("会话文件名")
        .to_string();
    let path = writer.path().to_string_lossy().to_string();
    (session_id, path)
}

fn collect_events() -> (EventEmitter, Arc<Mutex<Vec<RuntimeEvent>>>) {
    let seen: Arc<Mutex<Vec<RuntimeEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink: EventEmitter = {
        let seen = seen.clone();
        Arc::new(move |event: RuntimeEvent| {
            seen.lock().unwrap().push(event);
        })
    };
    (sink, seen)
}

fn wait_for<F: Fn(&[RuntimeEvent]) -> bool>(
    seen: &Arc<Mutex<Vec<RuntimeEvent>>>,
    done: F,
) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = seen.lock().unwrap();
        if done(&snapshot) {
            return snapshot
                .iter()
                .filter_map(|event| match event {
                    RuntimeEvent::AgentEvent(envelope) => Some(
                        format!("{:?}", envelope.event)
                            .chars()
                            .take(60)
                            .collect::<String>(),
                    ),
                    _ => None,
                })
                .collect();
        }
        drop(snapshot);
        if Instant::now() > deadline {
            let snapshot = seen.lock().unwrap();
            let dump: Vec<String> = snapshot
                .iter()
                .map(|event| match event {
                    RuntimeEvent::AgentEvent(envelope) => {
                        format!("{:?}", envelope.event).chars().take(80).collect()
                    }
                    RuntimeEvent::SessionStats(_) => "session-stats".to_string(),
                    RuntimeEvent::SessionError(envelope) => {
                        format!("session-error: {}", envelope.message)
                    }
                    RuntimeEvent::ApprovalRequest(_) => "approval-request".to_string(),
                    RuntimeEvent::SessionSwitched(envelope) => format!(
                        "session-switched → {} (archived={})",
                        envelope.to_session_id, envelope.archived
                    ),
                    RuntimeEvent::SessionChanged(envelope) => format!(
                        "session-changed: {}:{}",
                        envelope.agent_name, envelope.session_id
                    ),
                })
                .collect();
            panic!("等待超时；已收到 {} 个事件：{dump:#?}", snapshot.len());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 一次压缩 e2e 的观察结果（三种设置组合共用）。
struct Case {
    home: std::path::PathBuf,
    state: RuntimeState,
    runtime: tokio::runtime::Runtime,
    sessions_dir: std::path::PathBuf,
    old_id: String,
    old_path: std::path::PathBuf,
    events: Arc<Mutex<Vec<RuntimeEvent>>>,
}

impl Case {
    /// 打开种子会话 → 发一条消息 → 等到压缩完成。
    fn run(compaction: pipi_core::settings::CompactionSettings) -> Case {
        let home = temp_home();
        let (addr, _, _) = spawn_endpoint();
        let base_url = format!("http://{addr}/v1");
        fixture("compaction-e2e", &base_url, 60_000, compaction);
        let def = agents::load_agent("compaction-e2e").expect("加载 Agent");
        let sessions_dir = def.sessions_dir().expect("会话目录");
        let (old_id, old_path) = seed_history(&sessions_dir);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("构建注入运行时");
        let state = RuntimeState::new(runtime.handle().clone());
        state
            .open_session("compaction-e2e", &old_id)
            .expect("打开会话");
        let (sink, seen) = collect_events();
        state
            .send_prompt("compaction-e2e", Some(&old_id), "继续", None, sink)
            .expect("发送消息");
        // AgentEnd 之后 runtime 才做 turn 边界的压缩，所以等 CompactionEnd
        wait_for(&seen, |events| {
            events.iter().any(|event| {
                matches!(
                    event,
                    RuntimeEvent::AgentEvent(envelope)
                        if matches!(
                            envelope.event,
                            pipi_core::agent_loop::AgentEvent::CompactionEnd { .. }
                        )
                )
            })
        });
        Case {
            home,
            state,
            runtime,
            sessions_dir,
            old_id,
            old_path: std::path::PathBuf::from(old_path),
            events: seen,
        }
    }

    fn new_session_id(&self) -> String {
        // 压缩换会话后 map 里的键也搬到新 id：直接查该 Agent 当前打开的会话
        open_session_id(&self.state, "compaction-e2e")
    }

    /// 事件里 SessionSwitched 的目标 id（没有则为 None）。
    fn switched_to(&self) -> Option<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .find_map(|event| match event {
                RuntimeEvent::SessionSwitched(envelope) => Some(envelope.to_session_id.clone()),
                _ => None,
            })
    }

    /// 压缩条目里的（keep_from_entry, strategy, usage, summary）。
    fn compaction_entry(
        path: &std::path::Path,
    ) -> (String, String, Option<pipi_core::types::Usage>, String) {
        let entries = load_session(path).expect("读回会话");
        entries
            .iter()
            .find_map(|entry| match &entry.kind {
                EntryKind::Compaction {
                    keep_from_entry,
                    strategy,
                    usage,
                    summary,
                    ..
                } => Some((
                    keep_from_entry.clone(),
                    strategy.clone(),
                    *usage,
                    summary.clone(),
                )),
                _ => None,
            })
            .expect("应当写入 compaction 条目")
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

#[test]
fn compaction_forks_to_new_session_and_archives_original() {
    let _guard = HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let case = Case::run(pipi_core::settings::CompactionSettings {
        fork_before_compact: true,
        archive_original: true,
    });
    let new_id = case.new_session_id();
    assert_ne!(new_id, case.old_id, "压缩后当前会话应当是新分叉出来的那个");
    assert_eq!(
        case.switched_to().as_deref(),
        Some(new_id.as_str()),
        "必须发出 SessionSwitched 指到新会话"
    );

    // 新会话文件：活跃区里存在，含压缩条目 + 溯源标记
    let new_path = case.sessions_dir.join(format!("{new_id}.jsonl"));
    assert!(new_path.is_file(), "新会话文件应当存在");
    let (keep_from_entry, strategy, usage, summary) = Case::compaction_entry(&new_path);
    assert_eq!(strategy, "llm-summarize");
    assert!(
        !keep_from_entry.is_empty(),
        "尾部有 user 边界时必须记录保留区间"
    );
    assert_eq!(usage.expect("摘要用量必须落盘").input, SUMMARY_INPUT);
    assert!(summary.contains("## Goal"));
    let new_entries = load_session(&new_path).expect("读回新会话");
    assert!(
        new_entries.iter().any(|entry| matches!(
            &entry.kind,
            EntryKind::Custom { custom_type } if custom_type == &format!("compaction-fork:{}", case.old_id)
        )),
        "新会话应当带溯源标记"
    );

    // 原会话已归档，且归档副本是**完整记录**（没有压缩条目）
    assert!(!case.old_path.exists(), "原会话应当已离开活跃区");
    let archived = case
        .sessions_dir
        .join(pipi_core::agents::ARCHIVE_DIR)
        .join(format!("{}.jsonl", case.old_id));
    assert!(archived.is_file(), "归档区应当有原会话");
    let archived_entries = load_session(&archived).expect("读回归档会话");
    assert!(
        !archived_entries
            .iter()
            .any(|entry| matches!(entry.kind, EntryKind::Compaction { .. })),
        "归档的原会话应当是压缩前的完整记录"
    );
    let archived_messages =
        pipi_core::session::rebuild_messages(&pipi_core::session::active_path(&archived_entries));
    assert!(
        archived_messages.len() >= 121,
        "归档记录应当保留全部 40 轮 + 本轮新消息，实际 {}",
        archived_messages.len()
    );

    // 当前会话是压缩后的视图：摘要在前，长度明显变短
    let messages = case
        .runtime
        .block_on(
            case.state
                .session_messages("compaction-e2e", &case.new_session_id()),
        )
        .expect("读取消息");
    assert_eq!(
        pipi_core::session::summary_text(&messages[0]),
        Some(summary.as_str())
    );
    assert!(
        messages.len() < archived_messages.len(),
        "当前会话应当是压缩后的视图：{} vs {}",
        messages.len(),
        archived_messages.len()
    );

    // 摘要用量进账本，但不改写「当前上下文占用」
    let stats = case
        .state
        .session_stats("compaction-e2e", &case.new_session_id())
        .expect("读取统计");
    assert!(stats.input >= SUMMARY_INPUT);
    assert_eq!(stats.context_used, Some(CHAT_INPUT));

    // 键也跟着搬到了新 id：新会话是「打开中的那条」，旧 id 已经不是
    //（没搬的话会有两个问题：旧键仍指向新文件、旧文件却已归档）
    let new_id = case.new_session_id();
    assert!(
        case.state
            .session_info("compaction-e2e", &new_id)
            .expect("读取新会话")
            .is_some(),
        "换会话后新 id 应当是打开中的那条"
    );
    assert!(
        case.state
            .session_info("compaction-e2e", &case.old_id)
            .expect("读取旧会话")
            .is_none(),
        "换会话后旧 id 不应还留在 map 里"
    );
    // 旧会话已经不打开，可以归档/删除（占用检查按会话判定）
    assert!(
        case.state
            .open_session("compaction-e2e", &case.old_id)
            .is_err(),
        "旧会话文件已归档，重新打开应当失败"
    );
}

#[test]
fn compaction_keeps_original_in_place_when_archive_is_off() {
    let _guard = HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let case = Case::run(pipi_core::settings::CompactionSettings {
        fork_before_compact: true,
        archive_original: false,
    });
    let new_id = case.new_session_id();
    assert_ne!(new_id, case.old_id);
    // 分叉照常，但原会话留在活跃区
    assert!(case.old_path.is_file(), "关闭归档后原会话应留在活跃区");
    assert!(
        !case
            .sessions_dir
            .join(pipi_core::agents::ARCHIVE_DIR)
            .join(format!("{}.jsonl", case.old_id))
            .exists(),
        "关闭归档后不应出现在归档区"
    );
    assert!(
        case.sessions_dir.join(format!("{new_id}.jsonl")).is_file(),
        "新会话仍然照常创建"
    );
    let events = case.events.lock().unwrap();
    let archived_flag = events.iter().find_map(|event| match event {
        RuntimeEvent::SessionSwitched(envelope) => Some(envelope.archived),
        _ => None,
    });
    assert_eq!(archived_flag, Some(false), "事件应当如实报告未归档");
}

#[test]
fn compaction_stays_in_place_when_fork_is_off() {
    let _guard = HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let case = Case::run(pipi_core::settings::CompactionSettings {
        fork_before_compact: false,
        archive_original: true,
    });
    assert_eq!(case.new_session_id(), case.old_id, "关闭分叉时会话身份不变");
    assert!(
        case.switched_to().is_none(),
        "关闭分叉时不应发 SessionSwitched"
    );
    assert!(case.old_path.is_file(), "原文件就地写入压缩条目");
    // 压缩条目落在原文件上（原地压缩的既有行为）
    let (_, strategy, _, _) = Case::compaction_entry(&case.old_path);
    assert_eq!(strategy, "llm-summarize");
}

/// 手动压缩：**没到阈值**也要压（这正是手动的意义），并且走与自动相同的
/// 分叉 + 归档路径；界面靠一对 Agent 事件收尾（否则 running 卡住）。
#[test]
fn manual_compaction_ignores_threshold_and_switches_session() {
    let _guard = HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = temp_home();
    let (addr, _, summaries) = spawn_endpoint();
    let base_url = format!("http://{addr}/v1");
    fixture(
        "compaction-e2e",
        &base_url,
        200_000,
        pipi_core::settings::CompactionSettings {
            fork_before_compact: true,
            archive_original: true,
        },
    );
    let def = agents::load_agent("compaction-e2e").expect("加载 Agent");
    let sessions_dir = def.sessions_dir().expect("会话目录");
    // 40 轮 ≈ 五十几 k token：已超过保留预算（可压），但远低于
    // 200k × 75% = 150k 的触发线 —— 手动压缩的意义正在于此
    let (old_id, old_path) = seed_history(&sessions_dir);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("构建注入运行时");
    let state = RuntimeState::new(runtime.handle().clone());
    state
        .open_session("compaction-e2e", &old_id)
        .expect("打开会话");
    let (sink, seen) = collect_events();
    state
        .compact_now("compaction-e2e", &old_id, sink)
        .expect("手动压缩应当被接受");

    wait_for(&seen, |events| {
        events.iter().any(|event| {
            matches!(
                event,
                RuntimeEvent::AgentEvent(envelope)
                    if matches!(
                        envelope.event,
                        pipi_core::agent_loop::AgentEvent::CompactionEnd { .. }
                    )
            )
        })
    });
    assert_eq!(
        summaries.load(Ordering::SeqCst),
        1,
        "手动压缩要真的调一次摘要"
    );

    // 事件序列：AgentStart 开头、AgentEnd 收尾（前端靠它把 running 落下）
    let kinds: Vec<&'static str> = seen
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::AgentEvent(envelope) => Some(match envelope.event {
                pipi_core::agent_loop::AgentEvent::AgentStart => "agent_start",
                pipi_core::agent_loop::AgentEvent::AgentEnd { .. } => "agent_end",
                pipi_core::agent_loop::AgentEvent::CompactionStart => "compaction_start",
                pipi_core::agent_loop::AgentEvent::CompactionEnd { .. } => "compaction_end",
                _ => "other",
            }),
            _ => None,
        })
        .collect();
    assert_eq!(kinds.first(), Some(&"agent_start"), "{kinds:?}");
    assert_eq!(kinds.last(), Some(&"agent_end"), "{kinds:?}");
    assert!(kinds.contains(&"compaction_start") && kinds.contains(&"compaction_end"));
    let new_id = open_session_id(&state, "compaction-e2e");
    assert!(
        !state.session_running("compaction-e2e", &new_id),
        "结束后必须不再是运行中"
    );
    assert_ne!(new_id, old_id, "手动压缩同样分叉到新会话");
    assert!(sessions_dir.join(format!("{new_id}.jsonl")).is_file());
    assert!(!std::path::Path::new(&old_path).exists(), "原会话应已归档");
    assert!(sessions_dir
        .join(pipi_core::agents::ARCHIVE_DIR)
        .join(format!("{old_id}.jsonl"))
        .is_file());
    let _ = std::fs::remove_dir_all(&home);
}

/// 会话正在运行时手动压缩被拒（UI 会禁用按钮，核心也必须挡住）。
#[test]
fn manual_compaction_refused_while_running() {
    let _guard = HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = temp_home();
    let addr = spawn_stalling_endpoint();
    let base_url = format!("http://{addr}/v1");
    fixture(
        "compaction-e2e",
        &base_url,
        200_000,
        pipi_core::settings::CompactionSettings {
            fork_before_compact: true,
            archive_original: true,
        },
    );
    let def = agents::load_agent("compaction-e2e").expect("加载 Agent");
    let sessions_dir = def.sessions_dir().expect("会话目录");
    let (session_id, _) = seed_history(&sessions_dir);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("构建注入运行时");
    let state = RuntimeState::new(runtime.handle().clone());
    state
        .open_session("compaction-e2e", &session_id)
        .expect("打开会话");
    let (sink, _seen) = collect_events();
    // 这一轮会停在半途（收到一个分片后静默）
    state
        .send_prompt(
            "compaction-e2e",
            Some(&session_id),
            "跑一下",
            None,
            sink.clone(),
        )
        .expect("启动一轮");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !state.session_running("compaction-e2e", &session_id) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        state.session_running("compaction-e2e", &session_id),
        "这一轮应当处于运行中"
    );

    let error = state
        .compact_now("compaction-e2e", &session_id, sink)
        .expect_err("运行中必须拒绝手动压缩");
    assert!(error.contains("正在运行"), "{error}");

    // 收尾：停止这一轮，别把测试挂在半路
    state.stop_run("compaction-e2e", &session_id).expect("停止");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && state.session_running("compaction-e2e", &session_id) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !state.session_running("compaction-e2e", &session_id),
        "停止后应当落地"
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// 历史太短（都在保留预算内）时手动压缩返回可读错误，并如实报给前端。
#[test]
fn manual_compaction_reports_when_nothing_to_compress() {
    let _guard = HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = temp_home();
    let (addr, _, summaries) = spawn_endpoint();
    let base_url = format!("http://{addr}/v1");
    fixture(
        "compaction-e2e",
        &base_url,
        200_000,
        pipi_core::settings::CompactionSettings {
            fork_before_compact: true,
            archive_original: true,
        },
    );
    let def = agents::load_agent("compaction-e2e").expect("加载 Agent");
    let sessions_dir = def.sessions_dir().expect("会话目录");
    let (session_id, path) = seed_history_turns(&sessions_dir, 1);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("构建注入运行时");
    let state = RuntimeState::new(runtime.handle().clone());
    state
        .open_session("compaction-e2e", &session_id)
        .expect("打开会话");
    let (sink, seen) = collect_events();
    state
        .compact_now("compaction-e2e", &session_id, sink)
        .expect("调用本身应被接受");

    wait_for(&seen, |events| {
        events.iter().any(|event| {
            matches!(
                event,
                RuntimeEvent::AgentEvent(envelope)
                    if matches!(envelope.event, pipi_core::agent_loop::AgentEvent::AgentEnd { .. })
            )
        })
    });
    let error = seen
        .lock()
        .unwrap()
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::SessionError(envelope) => Some(envelope.message.clone()),
            _ => None,
        })
        .expect("应当通过会话错误通道说明原因");
    assert!(error.contains("压缩"), "{error}");
    assert_eq!(
        summaries.load(Ordering::SeqCst),
        0,
        "没有可压的就不该调摘要"
    );
    // 会话没有被换掉、文件还在原地
    assert_eq!(
        state
            .session_info("compaction-e2e", &session_id)
            .unwrap()
            .unwrap()
            .session_id,
        session_id,
        "失败时不该换会话"
    );
    assert!(std::path::Path::new(&path).is_file());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn compaction_persists_kept_range_and_usage_then_replay_keeps_it() {
    let _guard = HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = temp_home();
    let (addr, chats, summaries) = spawn_endpoint();
    let base_url = format!("http://{addr}/v1");
    fixture(
        "compaction-e2e",
        &base_url,
        60_000,
        pipi_core::settings::CompactionSettings {
            fork_before_compact: false,
            archive_original: false,
        },
    );
    let def = agents::load_agent("compaction-e2e").expect("加载 Agent");
    let sessions_dir = def.sessions_dir().expect("会话目录");
    let (session_id, session_path) = seed_history(&sessions_dir);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("构建注入运行时");
    let state = RuntimeState::new(runtime.handle().clone());
    state
        .open_session("compaction-e2e", &session_id)
        .expect("打开会话");
    let (sink, seen) = collect_events();

    state
        .send_prompt("compaction-e2e", Some(&session_id), "继续", None, sink)
        .expect("发送消息");

    // 等这一轮结束（AgentEnd 之后 runtime 才会做 turn 边界的压缩，所以再等
    // CompactionEnd；没有 CompactionEnd 时用超时兜底断言）
    wait_for(&seen, |events| {
        events.iter().any(|event| {
            matches!(
                event,
                RuntimeEvent::AgentEvent(envelope)
                    if matches!(
                        envelope.event,
                        pipi_core::agent_loop::AgentEvent::CompactionEnd { .. }
                    )
            )
        })
    });
    assert_eq!(chats.load(Ordering::SeqCst), 1, "只该有一次普通对话请求");
    assert_eq!(summaries.load(Ordering::SeqCst), 1, "应当发生一次摘要调用");

    // 1) 落盘的压缩条目：保留区间 + 策略 + 用量
    let entries = load_session(std::path::Path::new(&session_path)).expect("读回会话");
    let compaction = entries
        .iter()
        .find_map(|entry| match &entry.kind {
            EntryKind::Compaction {
                keep_from_entry,
                strategy,
                usage,
                summary,
                ..
            } => Some((
                keep_from_entry.clone(),
                strategy.clone(),
                *usage,
                summary.clone(),
            )),
            _ => None,
        })
        .expect("应当写入 compaction 条目");
    let (keep_from_entry, strategy, usage, summary) = compaction;
    assert_eq!(strategy, "llm-summarize");
    assert!(
        !keep_from_entry.is_empty(),
        "尾部有 user 边界时必须记录保留区间起点"
    );
    let usage = usage.expect("摘要调用的用量必须落盘");
    assert_eq!(usage.input, SUMMARY_INPUT);
    assert!(
        summary.contains("## Goal"),
        "摘要正文应为模型返回的内容：{summary}"
    );

    // 2) 重开会话：摘要 + 保留区间的原文都还在
    //    （先释放再重开：否则拿的是内存里那份，验证不到回放）
    state.new_session("compaction-e2e").expect("释放会话槽");
    state
        .open_session("compaction-e2e", &session_id)
        .expect("重开会话");
    let messages = runtime
        .block_on(state.session_messages("compaction-e2e", &session_id))
        .expect("读取消息");
    assert_eq!(
        pipi_core::session::summary_text(&messages[0]),
        Some(summary.as_str()),
        "首条应是上次的摘要"
    );
    let kept_tool_results = messages
        .iter()
        .filter(|message| matches!(message, Message::ToolResult { .. }))
        .count();
    assert!(
        kept_tool_results > 0,
        "重开后保留区间的原文必须还在（这正是本次修复点）：{} 条消息",
        messages.len()
    );
    assert!(
        messages.len() > 1 + kept_tool_results,
        "除保留原文外还应有压缩之后的新消息"
    );

    // 3) 摘要用量进累计账本，但不改写「当前上下文占用」
    let stats = state
        .session_stats("compaction-e2e", &session_id)
        .expect("读取统计");
    assert!(
        stats.input >= SUMMARY_INPUT,
        "摘要调用的输入必须计入累计：{}",
        stats.input
    );
    assert_eq!(
        stats.context_used,
        Some(CHAT_INPUT),
        "上下文占用应是最近一次对话请求的 prompt，而不是摘要请求的"
    );

    let _ = std::fs::remove_dir_all(&home);
}
