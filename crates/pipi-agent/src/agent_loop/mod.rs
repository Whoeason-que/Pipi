//! Agent 主循环。移植自 `packages/agent/src/agent-loop.ts`。
//!
//! 保留的核心语义：
//! - 事件流（agent / turn / message / tool 四层生命周期）
//! - steering（运行中插话）与 follow-up（结束后追加）两条消息队列
//! - 工具批次执行：顺序 / 并行；全部结果 terminate 才提前终止
//! - stopReason 为 length 时拒绝执行所有工具调用（参数可能被截断）
//! - before/after 工具钩子（Pipi 的命令权限在 bash 工具内部检查，钩子供扩展用）
//!
//! 有意简化（与上游差异，见 lib.rs）：convertToLlm 为恒等（暂无自定义消息
//! 类型）；transformContext 只做消息投影，未实现 prepareNextTurn / shouldStopAfterTurn；
//! 并行批次按助手消息中的顺序（而非完成顺序）回报结果。

use std::sync::{Arc, Mutex};

use futures_util::future::join_all;
use serde::Serialize;
use serde_json::{Value, json};

use pipi_protocol::provider::Provider;
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput, ToolRegistry, validate_args};
use pipi_protocol::{
    AbortSignal, Api, ContentBlock, Context, Message, Model, StopReason, StreamEvent,
    StreamOptions, ToolResultContent,
};

mod drive;
mod stream;
mod tools;
mod types;
pub use drive::run_agent_loop;
use stream::stream_assistant_response;
use tools::{execute_tool_calls, fail_truncated_tool_calls};
pub use types::{
    AgentContext, AgentEvent, AgentLoopConfig, BeforeToolCallHook, BeforeToolCallOutcome, Emitter,
    MessageQueue, ToolExecutionMode, TransformContextHook,
};

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::{Api, Usage};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ----- 测试用的脚本化 Provider 与计数工具 -----

    struct ScriptedProvider {
        turns: Vec<Vec<StreamEvent>>,
        call: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Provider for ScriptedProvider {
        async fn stream(
            &self,
            _model: &Model,
            _context: &Context,
            _options: &StreamOptions,
            _abort: AbortSignal,
        ) -> pipi_protocol::provider::EventStream {
            let (tx, rx) = tokio::sync::mpsc::channel(64);
            let index = self.call.fetch_add(1, Ordering::SeqCst);
            let turn = self
                .turns
                .get(index)
                .cloned()
                .unwrap_or_else(|| self.turns.last().cloned().unwrap_or_default());
            tokio::spawn(async move {
                for ev in turn {
                    let _ = tx.send(ev).await;
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                }
            });
            rx
        }
    }

    struct CountTool {
        calls: Arc<Mutex<Vec<Value>>>,
    }

    #[async_trait::async_trait]
    impl AgentTool for CountTool {
        fn name(&self) -> &'static str {
            "count"
        }
        fn description(&self) -> String {
            "count things".into()
        }
        fn parameters(&self) -> Value {
            json!({"type": "object", "properties": {"n": {"type": "number"}}, "required": ["n"]})
        }
        async fn execute(
            &self,
            _ctx: &ToolContext,
            args: &Value,
            _on_update: &(dyn Fn(ToolOutput) + Send + Sync),
        ) -> Result<ToolOutput, String> {
            self.calls.lock().unwrap().push(args.clone());
            Ok(ToolOutput::text(format!("count={}", args["n"])))
        }
    }

    fn test_model() -> Model {
        Model {
            id: "mock".into(),
            name: "mock".into(),
            api: Api::OpenAICompletions,
            base_url: "http://localhost".into(),
            max_tokens: 1024,
            context_window: 0,
        }
    }

    fn assistant_message(content: Vec<ContentBlock>, reason: StopReason) -> Message {
        Message::Assistant {
            content,
            api: "openai-completions".into(),
            provider: "mock".into(),
            model: "mock".into(),
            usage: Usage::default(),
            stop_reason: reason,
            error_message: None,
            timestamp: 0,
            duration_ms: None,
        }
    }

    fn tool_use_turn(call_id: &str, n: i64) -> Vec<StreamEvent> {
        let call = ContentBlock::ToolCall {
            id: call_id.into(),
            name: "count".into(),
            arguments: json!({"n": n}),
        };
        vec![
            StreamEvent::Start,
            StreamEvent::ToolCallEnd {
                content_index: 0,
                call: call.clone(),
            },
            StreamEvent::Done {
                reason: StopReason::ToolUse,
                usage: Usage::default(),
                message: Box::new(assistant_message(vec![call], StopReason::ToolUse)),
            },
        ]
    }

    fn stop_turn(text: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::Start,
            StreamEvent::Done {
                reason: StopReason::Stop,
                usage: Usage::default(),
                message: Box::new(assistant_message(
                    vec![ContentBlock::Text { text: text.into() }],
                    StopReason::Stop,
                )),
            },
        ]
    }

    /// 可重试 / 终态错误的一轮（只发一个 Error 事件）。
    fn error_turn(message: &str, retryable: bool) -> Vec<StreamEvent> {
        vec![StreamEvent::Error {
            message: message.into(),
            retry: retryable.then(pipi_protocol::RetryHint::plain),
            protocol_mismatch: false,
        }]
    }

    /// 端点回报「不支持当前协议」的一轮（协议协商的触发条件）。
    fn protocol_mismatch_turn(message: &str) -> Vec<StreamEvent> {
        vec![StreamEvent::Error {
            message: message.into(),
            retry: None,
            protocol_mismatch: true,
        }]
    }

    /// 已吐出正文增量、随后失败的一轮（验证「已输出正文不重放」）。
    fn prose_then_error_turn(text: &str, message: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::TextDelta {
                content_index: 0,
                delta: text.into(),
            },
            StreamEvent::Error {
                message: message.into(),
                retry: Some(pipi_protocol::RetryHint::plain()),
                protocol_mismatch: false,
            },
        ]
    }

    /// 已吐出正文增量、随后被端点以「协议不支持」拒绝的一轮。
    fn prose_then_protocol_mismatch_turn(text: &str, message: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::TextDelta {
                content_index: 0,
                delta: text.into(),
            },
            StreamEvent::Error {
                message: message.into(),
                retry: None,
                protocol_mismatch: true,
            },
        ]
    }

    /// 工具调用参数只吐了一半就失败的一轮（Q 会话里实测的故障形态）。
    fn truncated_tool_turn(message: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::ToolCallStart {
                content_index: 0,
                id: "t1".into(),
                name: "count".into(),
            },
            StreamEvent::ToolCallDelta {
                content_index: 0,
                delta: "{\"n\": 1".into(),
            },
            StreamEvent::Error {
                message: message.into(),
                retry: Some(pipi_protocol::RetryHint::plain()),
                protocol_mismatch: false,
            },
        ]
    }

    /// 包装 [`ScriptedProvider`] 并记录每次请求实际用的协议 ——
    /// 协议协商用例靠它断言「第二次换了协议」。
    struct ApiRecordingProvider {
        inner: Arc<ScriptedProvider>,
        seen: Mutex<Vec<Api>>,
    }

    impl ApiRecordingProvider {
        fn new(turns: Vec<Vec<StreamEvent>>) -> Self {
            Self {
                inner: Arc::new(ScriptedProvider {
                    turns,
                    call: AtomicUsize::new(0),
                }),
                seen: Mutex::new(Vec::new()),
            }
        }

        fn seen(&self) -> Vec<Api> {
            self.seen.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Provider for ApiRecordingProvider {
        async fn stream(
            &self,
            model: &Model,
            context: &Context,
            options: &StreamOptions,
            abort: AbortSignal,
        ) -> pipi_protocol::provider::EventStream {
            self.seen.lock().unwrap().push(model.api);
            self.inner.stream(model, context, options, abort).await
        }
    }

    /// 快速重试策略：次数按用例给，退避压到 1ms 让测试不等待。
    fn fast_retry(max_attempts: u32) -> pipi_context::retry::RetryPolicy {
        pipi_context::retry::RetryPolicy {
            max_attempts,
            base_delay_ms: 1,
            max_delay_ms: 5,
        }
    }

    /// 收集事件的 sink：返回（事件列表, Emitter）。
    fn event_sink() -> (Arc<Mutex<Vec<AgentEvent>>>, Emitter) {
        let sink: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink2 = sink.clone();
        let emit: Emitter = Arc::new(move |event: AgentEvent| {
            sink2.lock().unwrap().push(event);
        });
        (sink, emit)
    }

    fn retry_events(events: &[AgentEvent]) -> Vec<(u32, u32)> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::RetryStart {
                    attempt,
                    max_attempts,
                    ..
                } => Some((*attempt, *max_attempts)),
                _ => None,
            })
            .collect()
    }

    fn test_config(
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
        before: Option<BeforeToolCallHook>,
    ) -> AgentLoopConfig {
        AgentLoopConfig {
            model: test_model(),
            provider,
            tools,
            tool_context: ToolContext {
                workspace: std::env::temp_dir(),
                memory_dir: None,
                read_roots: Vec::new(),
                permissions: Arc::new(Default::default()),
                sandbox: pipi_protocol::permissions::SandboxMode::DangerFullAccess,
                resolved_env: Arc::new(std::collections::BTreeMap::new()),
                abort: AbortSignal::new(),
                approver: None,
            },
            options: StreamOptions {
                auto_protocol: false,
                ..Default::default()
            },
            // 既有用例都是单次尝试的脚本化 provider：默认关掉重试，
            // 需要重试的用例自己指定策略
            retry: pipi_context::retry::RetryPolicy::NONE,
            // 协议协商默认关（等价于用户把提供商协议钉死）：需要协商的用例自己开
            tool_execution: ToolExecutionMode::Sequential,
            steering: MessageQueue::new(),
            follow_up: MessageQueue::new(),
            before_tool_call: before,
            transform_context: None,
        }
    }

    #[tokio::test]
    async fn loop_executes_tool_then_stops() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let registry = Arc::new(ToolRegistry::new(vec![Arc::new(CountTool {
            calls: calls.clone(),
        })]));

        let provider = Arc::new(ScriptedProvider {
            turns: vec![tool_use_turn("t1", 3), stop_turn("done")],
            call: AtomicUsize::new(0),
        });

        let sink: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink2 = sink.clone();
        let emit: Emitter = Arc::new(move |event: AgentEvent| {
            sink2.lock().unwrap().push(event);
        });

        let config = test_config(provider, registry, None);
        let new_messages = run_agent_loop(
            vec![Message::user_text("please count")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        // user → assistant(toolUse) → toolResult → assistant(stop)
        assert_eq!(new_messages.len(), 4);
        assert_eq!(new_messages[0].role(), "user");
        assert_eq!(new_messages[1].role(), "assistant");
        assert_eq!(new_messages[2].role(), "toolResult");
        match &new_messages[2] {
            Message::ToolResult {
                content, is_error, ..
            } => {
                assert!(!is_error);
                assert!(
                    matches!(&content[0], ToolResultContent::Text { text } if text == "count=3")
                );
            }
            other => panic!("expected toolResult, got {other:?}"),
        }
        assert_eq!(new_messages[3].role(), "assistant");
        assert_eq!(*calls.lock().unwrap(), vec![json!({"n": 3})]);

        let events = sink.lock().unwrap();
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::ToolExecutionStart { tool_name, .. } if tool_name == "count")
        ));
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolExecutionEnd {
                is_error: false,
                ..
            }
        )));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::AgentEnd { .. }))
        );
    }

    // ----- 可重试错误的重发 -----

    #[tokio::test]
    async fn retryable_error_is_retried_until_success() {
        let provider = Arc::new(ScriptedProvider {
            turns: vec![
                error_turn("流错误: status 429 Too Many Requests", true),
                stop_turn("恢复后的回复"),
            ],
            call: AtomicUsize::new(0),
        });
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.retry = fast_retry(3);

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(provider.call.load(Ordering::SeqCst), 2, "应重发一次");
        let last = messages.last().expect("至少一条消息");
        match last {
            Message::Assistant {
                content,
                stop_reason,
                error_message,
                ..
            } => {
                assert_eq!(*stop_reason, StopReason::Stop);
                assert!(error_message.is_none());
                assert!(
                    matches!(&content[0], ContentBlock::Text { text } if text == "恢复后的回复")
                );
            }
            other => panic!("期望成功的助手消息，得到 {other:?}"),
        }
        let events = sink.lock().unwrap();
        assert_eq!(
            retry_events(&events),
            vec![(1, 3)],
            "应发出一次 retry_start"
        );
    }

    #[tokio::test]
    async fn terminal_error_is_not_retried() {
        let provider = Arc::new(ScriptedProvider {
            turns: vec![
                error_turn("流错误: status 400 Bad Request", false),
                stop_turn("不该走到这里"),
            ],
            call: AtomicUsize::new(0),
        });
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.retry = fast_retry(3);

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(provider.call.load(Ordering::SeqCst), 1, "终态错误不应重发");
        match messages.last().expect("至少一条消息") {
            Message::Assistant {
                stop_reason,
                error_message,
                ..
            } => {
                assert_eq!(*stop_reason, StopReason::Error);
                assert!(error_message.as_deref().unwrap_or_default().contains("400"));
            }
            other => panic!("期望错误消息，得到 {other:?}"),
        }
        assert!(retry_events(&sink.lock().unwrap()).is_empty());
    }

    #[tokio::test]
    async fn exhausted_retries_say_how_many_attempts() {
        let provider = Arc::new(ScriptedProvider {
            turns: vec![error_turn("流错误: 连接重置", true)],
            call: AtomicUsize::new(0),
        });
        let (_sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.retry = fast_retry(3);

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(provider.call.load(Ordering::SeqCst), 3, "3 次尝试后放弃");
        match messages.last().expect("至少一条消息") {
            Message::Assistant { error_message, .. } => {
                let message = error_message.clone().unwrap_or_default();
                assert!(message.contains("已重试 3 次仍失败"), "{message}");
            }
            other => panic!("期望错误消息，得到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn prose_output_blocks_retry() {
        let provider = Arc::new(ScriptedProvider {
            turns: vec![
                prose_then_error_turn("半截正文", "流错误: 连接重置"),
                stop_turn("重放会产生重复文本"),
            ],
            call: AtomicUsize::new(0),
        });
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.retry = fast_retry(3);

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(
            provider.call.load(Ordering::SeqCst),
            1,
            "已经输出过正文就不该重放"
        );
        match messages.last().expect("至少一条消息") {
            Message::Assistant { error_message, .. } => {
                let message = error_message.clone().unwrap_or_default();
                assert!(message.contains("已输出部分内容，未自动重试"), "{message}");
            }
            other => panic!("期望错误消息，得到 {other:?}"),
        }
        assert!(retry_events(&sink.lock().unwrap()).is_empty());
    }

    #[tokio::test]
    async fn protocol_mismatch_switches_protocol_without_spending_retry_budget() {
        // 第一次请求（OpenAI 协议）被端点拒绝，第二次用 Anthropic 协议成功：
        // 换协议不是故障重试 —— 不占重试预算（这里 retry = NONE）、不发 RetryStart。
        let provider = Arc::new(ApiRecordingProvider::new(vec![
            protocol_mismatch_turn("流错误: Model does not support this protocol."),
            stop_turn("换协议后的回复"),
        ]));
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.options.auto_protocol = true;

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(
            provider.seen(),
            vec![Api::OpenAICompletions, Api::AnthropicMessages],
            "第二次请求必须换成另一种协议"
        );
        match messages.last().expect("至少一条消息") {
            Message::Assistant { stop_reason, .. } => assert_eq!(*stop_reason, StopReason::Stop),
            other => panic!("期望成功的助手消息，得到 {other:?}"),
        }
        assert!(
            retry_events(&sink.lock().unwrap()).is_empty(),
            "协议协商不占重试预算：不该出现 RetryStart"
        );
    }

    #[tokio::test]
    async fn protocol_mismatch_with_pinned_protocol_stays_terminal() {
        // 用户把提供商协议钉死（auto_protocol=false）：不换协议，错误原样收尾，
        // 只是多一句「改为自动可按模型选协议」的提示。
        let provider = Arc::new(ApiRecordingProvider::new(vec![protocol_mismatch_turn(
            "流错误: Model does not support this protocol.",
        )]));
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.options.auto_protocol = false;

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(
            provider.seen(),
            vec![Api::OpenAICompletions],
            "钉死时不换协议"
        );
        assert!(retry_events(&sink.lock().unwrap()).is_empty());
        match messages.last().expect("至少一条消息") {
            Message::Assistant { error_message, .. } => {
                let message = error_message.clone().unwrap_or_default();
                assert!(message.contains("协议已固定"), "{message}");
            }
            other => panic!("期望错误消息，得到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn protocol_mismatch_after_prose_is_not_replayed() {
        // 已经吐过正文：沿用「已输出正文不重放」，不换协议也不再试
        let provider = Arc::new(ApiRecordingProvider::new(vec![
            prose_then_protocol_mismatch_turn("半截正文", "Model does not support this protocol."),
            stop_turn("重放会产生重复文本"),
        ]));
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.options.auto_protocol = true;

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(
            provider.seen(),
            vec![Api::OpenAICompletions],
            "已输出正文：协议也不该再试"
        );
        assert!(retry_events(&sink.lock().unwrap()).is_empty());
        match messages.last().expect("至少一条消息") {
            Message::Assistant { error_message, .. } => {
                let message = error_message.clone().unwrap_or_default();
                assert!(message.contains("已输出部分内容，未自动重试"), "{message}");
            }
            other => panic!("期望错误消息，得到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn protocol_mismatch_after_both_protocols_is_terminal() {
        // 两种协议都被拒：明确告诉用户两种都试过了，不再无限换
        let provider = Arc::new(ApiRecordingProvider::new(vec![
            protocol_mismatch_turn("流错误: Model does not support this protocol."),
            protocol_mismatch_turn("流错误: Model does not support this protocol."),
        ]));
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.options.auto_protocol = true;

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(
            provider.seen(),
            vec![Api::OpenAICompletions, Api::AnthropicMessages],
            "只换一次协议"
        );
        assert!(retry_events(&sink.lock().unwrap()).is_empty());
        match messages.last().expect("至少一条消息") {
            Message::Assistant { error_message, .. } => {
                let message = error_message.clone().unwrap_or_default();
                assert!(message.contains("两种协议均已尝试"), "{message}");
            }
            other => panic!("期望错误消息，得到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn truncated_tool_call_without_prose_is_retried() {
        let provider = Arc::new(ScriptedProvider {
            turns: vec![
                truncated_tool_turn("流错误: tool call `count` arrived with malformed JSON input"),
                stop_turn("重发后的回复"),
            ],
            call: AtomicUsize::new(0),
        });
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        config.retry = fast_retry(3);

        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        assert_eq!(
            provider.call.load(Ordering::SeqCst),
            2,
            "工具参数截断应重发"
        );
        match messages.last().expect("至少一条消息") {
            Message::Assistant { stop_reason, .. } => assert_eq!(*stop_reason, StopReason::Stop),
            other => panic!("期望成功的助手消息，得到 {other:?}"),
        }
        let events = sink.lock().unwrap();
        assert_eq!(retry_events(&events), vec![(1, 3)]);
        // 半截参数绝不交给工具执行
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolExecutionStart { .. })),
            "被截断的工具调用不应执行"
        );
    }

    #[tokio::test]
    async fn abort_during_backoff_stops_immediately() {
        let provider = Arc::new(ScriptedProvider {
            turns: vec![error_turn("流错误: 连接重置", true)],
            call: AtomicUsize::new(0),
        });
        let (sink, emit) = event_sink();
        let mut config = test_config(provider.clone(), Arc::new(ToolRegistry::new(vec![])), None);
        // 退避 5 秒：若不与 abort 竞速，这个用例会一直等下去
        config.retry = pipi_context::retry::RetryPolicy {
            max_attempts: 5,
            base_delay_ms: 5_000,
            max_delay_ms: 5_000,
        };

        let abort = AbortSignal::new();
        let abort2 = abort.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            abort2.abort();
        });

        let started = std::time::Instant::now();
        let messages = run_agent_loop(
            vec![Message::user_text("hi")],
            AgentContext::default(),
            config,
            emit,
            abort,
        )
        .await;

        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "退避期间中止应立刻生效，实际等了 {:?}",
            started.elapsed()
        );
        match messages.last().expect("至少一条消息") {
            Message::Assistant { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::Aborted)
            }
            other => panic!("期望中止消息，得到 {other:?}"),
        }
        assert_eq!(retry_events(&sink.lock().unwrap()), vec![(1, 5)]);
    }

    #[tokio::test]
    async fn before_hook_can_block_tool() {
        let registry = Arc::new(ToolRegistry::new(vec![Arc::new(CountTool {
            calls: Arc::new(Mutex::new(Vec::new())),
        })]));
        let provider = Arc::new(ScriptedProvider {
            turns: vec![tool_use_turn("t1", 1), stop_turn("ok")],
            call: AtomicUsize::new(0),
        });
        let config = test_config(
            provider,
            registry,
            Some(Arc::new(|_call, _args| {
                Some(BeforeToolCallOutcome {
                    block: true,
                    reason: Some("不允许".into()),
                })
            })),
        );
        let emit: Emitter = Arc::new(|_| {});

        let new_messages = run_agent_loop(
            vec![Message::user_text("go")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        match &new_messages[2] {
            Message::ToolResult {
                is_error, content, ..
            } => {
                assert!(is_error);
                assert!(
                    matches!(&content[0], ToolResultContent::Text { text } if text == "不允许")
                );
            }
            other => panic!("expected blocked toolResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn length_stop_rejects_tool_calls() {
        let registry = Arc::new(ToolRegistry::new(vec![Arc::new(CountTool {
            calls: Arc::new(Mutex::new(Vec::new())),
        })]));
        let length_turn = vec![StreamEvent::Done {
            reason: StopReason::Length,
            usage: Usage::default(),
            message: Box::new(assistant_message(
                vec![ContentBlock::ToolCall {
                    id: "t1".into(),
                    name: "count".into(),
                    arguments: json!({"n": 1}),
                }],
                StopReason::Length,
            )),
        }];
        let provider = Arc::new(ScriptedProvider {
            turns: vec![length_turn, stop_turn("retry done")],
            call: AtomicUsize::new(0),
        });
        let config = test_config(provider, registry, None);
        let emit: Emitter = Arc::new(|_| {});

        let new_messages = run_agent_loop(
            vec![Message::user_text("go")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        match &new_messages[2] {
            Message::ToolResult {
                is_error, content, ..
            } => {
                assert!(is_error);
                assert!(
                    matches!(&content[0], ToolResultContent::Text { text } if text.contains("token limit"))
                );
            }
            other => panic!("expected rejected toolResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transform_context_prunes_before_llm_call() {
        use std::sync::atomic::AtomicUsize;

        // 记录 provider 每次看到的请求长度；第一轮结束后推 follow-up
        // 强制出现第二次 LLM 调用
        struct RecordingProvider {
            inner: ScriptedProvider,
            seen: Arc<Mutex<Vec<usize>>>,
            follow_up: MessageQueue,
        }
        #[async_trait::async_trait]
        impl Provider for RecordingProvider {
            async fn stream(
                &self,
                model: &Model,
                context: &Context,
                options: &StreamOptions,
                abort: AbortSignal,
            ) -> pipi_protocol::provider::EventStream {
                if self.inner.call.load(Ordering::SeqCst) == 0 {
                    self.follow_up.push(Message::user_text("follow-up"));
                }
                self.seen.lock().unwrap().push(context.messages.len());
                self.inner.stream(model, context, options, abort).await
            }
        }

        let registry = Arc::new(ToolRegistry::new(vec![]));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let follow_up = MessageQueue::new();
        let provider = Arc::new(RecordingProvider {
            inner: ScriptedProvider {
                turns: vec![stop_turn("first"), stop_turn("second")],
                call: AtomicUsize::new(0),
            },
            seen: seen.clone(),
            follow_up: follow_up.clone(),
        });

        let mut config = test_config(provider, registry, None);
        config.follow_up = follow_up;
        // 预算很小：第二次调用时历史超限，transform 应裁掉旧轮次
        config.transform_context =
            Some(Arc::new(
                |messages| match pipi_context::context::prune_cut_index(&messages, 80) {
                    Some(cut) => messages[cut..].to_vec(),
                    None => messages,
                },
            ));
        let emit: Emitter = Arc::new(|_| {});

        run_agent_loop(
            vec![Message::user_text("x".repeat(500))],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], 1);
        // 第二次请求被裁剪：完整历史是 3 条（user/assistant/follow-up），
        // 超预算后只应看到 follow-up 这一条
        assert_eq!(*seen.last().unwrap(), 1);
    }

    #[tokio::test]
    async fn steering_message_is_injected() {
        let registry = Arc::new(ToolRegistry::new(vec![]));
        // 第一轮调用时往 steering 队列塞消息，模拟「运行中用户插话」
        let steering = MessageQueue::new();
        let steering_for_provider = steering.clone();
        struct PushProvider {
            steering: MessageQueue,
            inner: ScriptedProvider,
        }
        #[async_trait::async_trait]
        impl Provider for PushProvider {
            async fn stream(
                &self,
                model: &Model,
                context: &Context,
                options: &StreamOptions,
                abort: AbortSignal,
            ) -> pipi_protocol::provider::EventStream {
                // 只在第一轮推 —— 否则每轮都注入新消息，循环永不停止
                if self.inner.call.load(Ordering::SeqCst) == 0 {
                    self.steering.push(Message::user_text("wait, also do X"));
                }
                self.inner.stream(model, context, options, abort).await
            }
        }
        let provider = Arc::new(PushProvider {
            steering: steering_for_provider,
            inner: ScriptedProvider {
                turns: vec![stop_turn("first"), stop_turn("second")],
                call: AtomicUsize::new(0),
            },
        });

        let mut config = test_config(provider, registry, None);
        config.steering = steering;
        let emit: Emitter = Arc::new(|_| {});

        let new_messages = run_agent_loop(
            vec![Message::user_text("start")],
            AgentContext::default(),
            config,
            emit,
            AbortSignal::new(),
        )
        .await;

        // user → assistant(first) → user(steering) → assistant(second)
        assert_eq!(new_messages.len(), 4);
        assert_eq!(new_messages[2].role(), "user");
        match &new_messages[2] {
            Message::User { content, .. } => assert!(content.contains("also do X")),
            other => panic!("expected steering user message, got {other:?}"),
        }
    }

    // ----- 中止不变量：批次里每个调用都必须留下结果 -----

    fn two_call_turn(id1: &str, id2: &str) -> Vec<StreamEvent> {
        let call1 = ContentBlock::ToolCall {
            id: id1.into(),
            name: "count".into(),
            arguments: json!({"n": 1}),
        };
        let call2 = ContentBlock::ToolCall {
            id: id2.into(),
            name: "count".into(),
            arguments: json!({"n": 2}),
        };
        vec![
            StreamEvent::Start,
            StreamEvent::ToolCallEnd {
                content_index: 0,
                call: call1.clone(),
            },
            StreamEvent::ToolCallEnd {
                content_index: 1,
                call: call2.clone(),
            },
            StreamEvent::Done {
                reason: StopReason::ToolUse,
                usage: Usage::default(),
                message: Box::new(assistant_message(vec![call1, call2], StopReason::ToolUse)),
            },
        ]
    }

    /// 在交付助手消息前触发中止的 provider：模拟「模型已产出工具调用，
    /// 但批次执行前用户按了停止」。
    struct AbortingProvider {
        inner: ScriptedProvider,
        abort: AbortSignal,
    }

    #[async_trait::async_trait]
    impl Provider for AbortingProvider {
        async fn stream(
            &self,
            model: &Model,
            context: &Context,
            options: &StreamOptions,
            abort: AbortSignal,
        ) -> pipi_protocol::provider::EventStream {
            self.abort.abort();
            self.inner.stream(model, context, options, abort).await
        }
    }

    async fn aborted_batch_yields_results_for_every_call(mode: ToolExecutionMode) {
        let registry = Arc::new(ToolRegistry::new(vec![Arc::new(CountTool {
            calls: Arc::new(Mutex::new(Vec::new())),
        })]));
        let abort = AbortSignal::new();
        let provider = Arc::new(AbortingProvider {
            inner: ScriptedProvider {
                turns: vec![two_call_turn("a1", "a2"), stop_turn("never")],
                call: AtomicUsize::new(0),
            },
            abort: abort.clone(),
        });
        let mut config = test_config(provider, registry, None);
        config.tool_execution = mode;
        let emit: Emitter = Arc::new(|_| {});

        let new_messages = run_agent_loop(
            vec![Message::user_text("go")],
            AgentContext::default(),
            config,
            emit,
            abort,
        )
        .await;

        let answered: Vec<(&str, bool)> = new_messages
            .iter()
            .filter_map(|message| match message {
                Message::ToolResult {
                    tool_call_id,
                    is_error,
                    ..
                } => Some((tool_call_id.as_str(), *is_error)),
                _ => None,
            })
            .collect();
        // 每个工具调用都要有结果（否则历史留下无主 tool_call，端点按协议 400）
        assert_eq!(
            answered,
            vec![("a1", true), ("a2", true)],
            "中止后仍应为每个调用产出错误结果"
        );
    }

    #[tokio::test]
    async fn aborted_sequential_batch_answers_every_call() {
        aborted_batch_yields_results_for_every_call(ToolExecutionMode::Sequential).await;
    }

    #[tokio::test]
    async fn aborted_parallel_batch_answers_every_call() {
        aborted_batch_yields_results_for_every_call(ToolExecutionMode::Parallel).await;
    }

    // ----- 发送前配对修复：provider 实际收到的历史必须合法 -----

    /// 记录 provider 每次请求看到的消息角色/toolCallId 的 provider。
    struct CaptureProvider {
        inner: ScriptedProvider,
        seen: Arc<Mutex<Vec<Vec<(String, String)>>>>,
    }

    #[async_trait::async_trait]
    impl Provider for CaptureProvider {
        async fn stream(
            &self,
            model: &Model,
            context: &Context,
            options: &StreamOptions,
            abort: AbortSignal,
        ) -> pipi_protocol::provider::EventStream {
            let roles = context
                .messages
                .iter()
                .map(|message| match message {
                    Message::User { .. } => ("user".to_string(), String::new()),
                    Message::Assistant { .. } => ("assistant".to_string(), String::new()),
                    Message::ToolResult {
                        tool_call_id,
                        is_error,
                        ..
                    } => (
                        if *is_error { "tool-error" } else { "tool" }.to_string(),
                        tool_call_id.clone(),
                    ),
                })
                .collect();
            self.seen.lock().unwrap().push(roles);
            self.inner.stream(model, context, options, abort).await
        }
    }

    #[tokio::test]
    async fn wire_history_repairs_dangling_tool_call_before_send() {
        let registry = Arc::new(ToolRegistry::new(vec![]));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(CaptureProvider {
            inner: ScriptedProvider {
                turns: vec![stop_turn("ok")],
                call: AtomicUsize::new(0),
            },
            seen: seen.clone(),
        });
        let config = test_config(provider, registry, None);
        let emit: Emitter = Arc::new(|_| {});

        // 历史里的破损（进程中断留下的中段悬挂）：assistant 带 tool_calls
        // 之后直接跟 user 消息，没有任何 tool 结果。
        let dangling = Message::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                arguments: json!({"command": "sleep 300"}),
            }],
            api: String::new(),
            provider: String::new(),
            model: "m".into(),
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 0,
            duration_ms: None,
        };
        let context = AgentContext {
            system_prompt: String::new(),
            messages: vec![
                Message::user_text("hi"),
                dangling,
                Message::user_text("继续"),
            ],
        };

        run_agent_loop(Vec::new(), context, config, emit, AbortSignal::new()).await;

        let seen = seen.lock().unwrap();
        let first_request = &seen[0];
        assert_eq!(
            first_request.clone(),
            vec![
                ("user".to_string(), String::new()),
                ("assistant".to_string(), String::new()),
                ("tool-error".to_string(), "c1".to_string()),
                ("user".to_string(), String::new()),
            ],
            "发出去的历史里，悬挂的 tool_calls 必须被补上结果"
        );
    }
}
