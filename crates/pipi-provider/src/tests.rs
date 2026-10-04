use super::*;
use pipi_protocol::Tool;

#[test]
fn rig_calls_share_the_same_http_client_pool() {
    assert!(std::ptr::eq(shared_http_client(), shared_http_client()));
}

fn context() -> Context {
    Context {
        system_prompt: Some("be brief".into()),
        messages: vec![
            Message::user_text("hello"),
            Message::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "t1".into(),
                    name: "read".into(),
                    arguments: json!({"path": "a.md"}),
                }],
                api: String::new(),
                provider: String::new(),
                model: "m".into(),
                usage: Usage::default(),
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: 0,
                duration_ms: None,
            },
            Message::ToolResult {
                tool_call_id: "t1".into(),
                tool_name: "read".into(),
                content: vec![ToolResultContent::Text { text: "out".into() }],
                is_error: false,
                details: None,
                timestamp: 0,
            },
        ],
        tools: vec![Tool {
            name: "read".into(),
            description: "d".into(),
            parameters: json!({"type": "object"}),
        }],
    }
}

#[test]
fn maps_messages_to_rig() {
    let msgs = to_rig_messages(&context(), Api::AnthropicMessages);
    // user → assistant(toolcall) → user(toolresult)
    assert_eq!(msgs.len(), 3);
    assert!(matches!(msgs[0], RigMessage::User { .. }));
    assert!(matches!(msgs[1], RigMessage::Assistant { .. }));
    match &msgs[2] {
        RigMessage::User { content } => match &content[0] {
            rig::message::UserContent::ToolResult(tr) => {
                assert_eq!(tr.name.as_str(), "read");
                assert_eq!(tr.call.to_string(), "t1");
            }
            other => panic!("expected tool result, got {other:?}"),
        },
        other => panic!("expected user, got {other:?}"),
    }
}

#[test]
fn maps_tools_to_rig() {
    let tools = to_rig_tools(&context());
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name.as_str(), "read");
    assert_eq!(tools[0].parameters["type"], json!("object"));
}

#[test]
fn maps_finish_reasons() {
    assert_eq!(
        map_finish_reason(Some(&FinishReason::ToolCalls), false),
        StopReason::ToolUse
    );
    assert_eq!(
        map_finish_reason(Some(&FinishReason::Length), false),
        StopReason::Length
    );
    assert_eq!(
        map_finish_reason(Some(&FinishReason::Stop), false),
        StopReason::Stop
    );
    assert_eq!(map_finish_reason(None, true), StopReason::ToolUse);
    assert_eq!(map_finish_reason(None, false), StopReason::Stop);
}

#[test]
fn usage_subtracts_cache_reads_and_writes_from_input() {
    // rig 0.43 的统一口径：input_tokens 已含缓存读写，必须扣掉 ——
    // 否则 input 与 cache_read 重叠，命中率恒为 50%。
    struct Case {
        name: &'static str,
        usage: RigUsage,
        input: u64,
        cache_read: u64,
        cache_write: u64,
        total: u64,
    }
    let cases = [
        Case {
            name: "缓存读被扣除",
            usage: RigUsage {
                input_tokens: Some(1000),
                output_tokens: Some(50),
                total_tokens: Some(1050),
                cached_input_tokens: Some(900),
                cache_creation_input_tokens: Some(0),
                tool_use_prompt_tokens: Some(0),
                reasoning_tokens: Some(0),
            },
            input: 100,
            cache_read: 900,
            cache_write: 0,
            total: 1050,
        },
        Case {
            name: "缓存写同样扣除，total 未上报时自行汇总",
            usage: RigUsage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                total_tokens: None,
                cached_input_tokens: Some(80),
                cache_creation_input_tokens: Some(5),
                tool_use_prompt_tokens: None,
                reasoning_tokens: None,
            },
            input: 15,
            cache_read: 80,
            cache_write: 5,
            total: 120,
        },
        Case {
            name: "真实会话：命中率 ≈99.98% 而不是 50%",
            usage: RigUsage {
                input_tokens: Some(610_566),
                output_tokens: Some(165),
                total_tokens: None,
                cached_input_tokens: Some(610_432),
                cache_creation_input_tokens: Some(0),
                tool_use_prompt_tokens: None,
                reasoning_tokens: None,
            },
            input: 134,
            cache_read: 610_432,
            cache_write: 0,
            total: 610_731,
        },
        Case {
            name: "端点口径混乱（cached > prompt）时不下溢",
            usage: RigUsage {
                input_tokens: Some(10),
                output_tokens: Some(1),
                total_tokens: Some(11),
                cached_input_tokens: Some(500),
                cache_creation_input_tokens: Some(0),
                tool_use_prompt_tokens: None,
                reasoning_tokens: None,
            },
            input: 0,
            cache_read: 500,
            cache_write: 0,
            total: 11,
        },
        Case {
            name: "计数缺失与上报 0 同等对待",
            usage: RigUsage::default(),
            input: 0,
            cache_read: 0,
            cache_write: 0,
            total: 0,
        },
    ];

    for case in &cases {
        let mapped = from_rig_usage(&case.usage);
        assert_eq!(mapped.input, case.input, "{}", case.name);
        assert_eq!(mapped.cache_read, case.cache_read, "{}", case.name);
        assert_eq!(mapped.cache_write, case.cache_write, "{}", case.name);
        assert_eq!(mapped.total_tokens, case.total, "{}", case.name);
        // 提示词总量 = input + cache_read + cache_write，命中不重复计
        assert_eq!(
            mapped.prompt_tokens(),
            case.input + case.cache_read + case.cache_write,
            "{}",
            case.name
        );
    }

    // 端点上报 total 时原样透传，未上报时汇总值与 prompt + completion 自洽
    let reported = from_rig_usage(&cases[0].usage);
    assert_eq!(reported.total(), 1050);
    let derived = from_rig_usage(&cases[2].usage);
    assert_eq!(derived.total(), derived.total_tokens);
    let hit = from_rig_usage(&cases[0].usage);
    let ratio = hit.cache_read as f64 / hit.prompt_tokens() as f64;
    assert!((ratio - 0.9).abs() < 1e-9, "ratio={ratio}");
    let pct = derived.cache_read as f64 / derived.prompt_tokens() as f64 * 100.0;
    assert!((pct - 99.98).abs() < 0.01, "pct={pct}");
}

#[test]
fn accumulator_finalizes_tool_args_and_duration() {
    let mut acc = Accumulator::default();
    acc.text_delta(0, "hi");
    acc.tool_call_delta(1, "{\"c");
    acc.tool_call_delta(1, "md\":\"ls\"}");
    acc.tool_call_end(1, "t1", "bash", json!({"cmd": "ls"}));
    let msg = acc.finalize("m", Usage::default(), StopReason::ToolUse, 1234);
    let Message::Assistant {
        content,
        duration_ms,
        ..
    } = msg
    else {
        panic!("expected assistant");
    };
    assert_eq!(duration_ms, Some(1234));
    assert_eq!(content.len(), 2);
    assert_eq!(
        content[1],
        ContentBlock::ToolCall {
            id: "t1".into(),
            name: "bash".into(),
            arguments: json!({"cmd": "ls"}),
        }
    );
}

#[test]
fn accumulator_keeps_parallel_tool_calls_apart() {
    // 两个工具调用的参数片段交错到达：按 part 归位，不能互相串台
    let mut acc = Accumulator::default();
    acc.tool_call_delta(0, "{\"a\":1}");
    acc.tool_call_delta(1, "{\"b\":2}");
    acc.tool_call_delta(0, "");
    acc.tool_call_end(0, "t1", "one", json!({"a": 1}));
    acc.tool_call_end(1, "t2", "two", json!({"b": 2}));
    let msg = acc.finalize("m", Usage::default(), StopReason::ToolUse, 1);
    let Message::Assistant { content, .. } = msg else {
        panic!("expected assistant");
    };
    assert_eq!(
        content,
        vec![
            ContentBlock::ToolCall {
                id: "t1".into(),
                name: "one".into(),
                arguments: json!({"a": 1}),
            },
            ContentBlock::ToolCall {
                id: "t2".into(),
                name: "two".into(),
                arguments: json!({"b": 2}),
            },
        ]
    );
}

#[test]
fn extra_headers_only_for_providers_that_require_session_id() {
    // 普通供应商：一个头都不塞，避免给无关请求改变行为
    assert!(extra_headers("https://api.deepseek.com/v1", Some("s-1")).is_none());
    assert!(extra_headers("https://api.anthropic.com", None).is_none());
    assert!(extra_headers("", Some("s-1")).is_none());

    // OpenCode Go：UA + 会话头都要有
    let headers = extra_headers("https://opencode.ai/zen/go/v1", Some("1789230239133-abc"))
        .expect("opencode-go 应带额外请求头");
    assert_eq!(
        headers
            .get("x-opencode-session")
            .and_then(|v| v.to_str().ok()),
        Some("1789230239133-abc"),
    );
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        user_agent.starts_with("pipi/"),
        "UA 应为 pipi/<版本>，实际 {user_agent}"
    );
}

#[test]
fn extra_headers_without_session_id_still_identifies_client() {
    let headers =
        extra_headers("https://opencode.ai/zen/go/v1", None).expect("命中供应商时至少应带 UA");
    assert!(headers.get("x-opencode-session").is_none());
    assert!(headers.get("user-agent").is_some());

    // 空串视为没有会话 ID，不得塞空值头（HeaderValue 允许空串，但供应商侧无法路由）
    let headers = extra_headers("https://opencode.ai/zen/go/v1", Some("")).unwrap();
    assert!(headers.get("x-opencode-session").is_none());
}

#[test]
fn thinking_blocks_replay_with_the_matching_issuer() {
    // reasoning 只能被签发它的那家读回：两套协议的 issuer 必须与自己一致
    assert_eq!(thinking_issuer(Api::AnthropicMessages), "anthropic");
    assert_eq!(thinking_issuer(Api::OpenAICompletions), "openai");
    let context = Context {
        system_prompt: None,
        messages: vec![Message::Assistant {
            content: vec![ContentBlock::Thinking {
                thinking: "hmm".into(),
                thinking_signature: Some("sig".into()),
            }],
            api: String::new(),
            provider: String::new(),
            model: "m".into(),
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
            duration_ms: None,
        }],
        tools: Vec::new(),
    };
    let msgs = to_rig_messages(&context, Api::AnthropicMessages);
    let RigMessage::Assistant { content, .. } = &msgs[0] else {
        panic!("expected assistant");
    };
    let AssistantContent::Reasoning(sealed) = &content[0] else {
        panic!("expected reasoning");
    };
    assert_eq!(sealed.issuer().as_str(), "anthropic");
    assert!(sealed.open(sealed.issuer()).is_some());
}
