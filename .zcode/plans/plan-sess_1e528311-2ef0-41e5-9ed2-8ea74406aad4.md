## 给所有可重试错误加入 retry（Pipi 核心 + 前端 + 设置）

### 先说明我替你定的四个取舍（你可否决）

| 取舍 | 决定 | 依据 |
|---|---|---|
| 部分输出已可见时 | **未出文本才重试；工具调用进行中被打断属例外，仍重试**（重试前丢弃半截工具块，绝不把半截参数交给工具） | 四家里只有 hermes 专门处理过这件事（`reference/hermes/agent/chat_completion_helpers.py:3421-3452` 的 `_partial_tool_in_flight` 例外）。实测那两次报错都是工具参数截断，若不重试就白丢一轮；而重试会重复已显示的正文文本，所以正文一旦流出就不重试 |
| 配置落点 | **全局 `settings.json` 新增 `retry` 段**（默认 3 次尝试 / 1s 基退避 / 30s 封顶 / 25% 抖动 / 尊重 `Retry-After`），设置视图加导航分组 | pi(`settings.retry.maxRetries/baseDelayMs`)、codex(`request_max_retries/stream_max_retries`) 都是可配；Pipi 有 `CompactionSettings` 的「字段级 default + 夹取」现成范式 |
| 让不让用户看见 | **对话里显示一行系统提示**（「流中断，2 秒后重试（1/3）· 原因」） | 你上次就是被这行裸错误搞懵的；Pipi 已有 compaction 系统条目的渲染范式可直接复用 |
| 无进展超时是否重试 | **最多重试 1 次**（每次尝试最长 300s，三次就是 15 分钟，用户会以为卡死）；连接建立阶段失败不受此限 | 超时是唯一「每次尝试都很贵」的类；codex 的 idle timeout 比 Pipi 短得多才敢照样退避 |

### 一、错误分类：结构化，不靠字符串正则

rig 0.42 的错误本身是带结构的（`completion/request.rs:82-116`、`provider_response.rs:17` 的 `status`、`http_client` 的 `non_success_status()` 与 `Retry-After` 头），所以**在我们自己的边界上做一次映射**，而不是解析格式化后的字符串。

新增 `crates/pipi-core/src/retry.rs`：

```rust
pub struct RetryPolicy { pub max_attempts: u32, pub base_delay_ms: u64, pub max_delay_ms: u64 }
impl RetryPolicy {
    pub fn disabled() -> Self;                                   // 测试与「关掉重试」用
    pub fn delay_for(&self, attempt: u32, hint_ms: Option<u64>) -> Option<Duration>; // None = 不等待，直接终态
}
pub enum RetryVerdict { Retryable, Terminal }
pub struct RetryHint { pub after_ms: Option<u64> }
pub fn classify_rig_error(err: &rig::completion::CompletionError) -> (RetryVerdict, Option<RetryHint>);
```

判定表（照 codex 的 `CodexErrorDetails::retry_delay()`（`reference/codex/codex-rs/protocol/src/error.rs:379-425`）那张 Rust 原生映射表的形状来）：

| rig 错误 | 判定 |
|---|---|
| `ProviderResponse(e)` / `HttpError(e)` 带状态码 | 408 / 409 / 429 / 5xx → **可重试**（读 `Retry-After` → `after_ms`）；400 / 401 / 403 / 404 / 413 / 422 → **终态** |
| `HttpError`（无状态码：连接重置、发送失败、DNS） | **可重试** |
| `ProviderError(msg)` | 命中传输特征（`error sending request` / `connection` / `reset` / `timeout` / `broken pipe`）→ 可重试；否则终态（保守） |
| `ResponseError(msg)` | 「malformed JSON input」（工具参数截断）与「stream ended/closed before …」→ **可重试**；其余终态 |
| `RequestError` / `JsonError` / `UrlError` | **终态**（本地构造/序列化问题，重试无用） |
| provider 自产的「无进展超时」（`provider.rs:419` / `:491`） | 可重试，但**受「最多 1 次」限制** |
| 流提前结束（`!saw_final && !abort.is_aborted()`，`provider.rs:495-498`） | **可重试**（顺带修掉现在被误报成「已中止」的问题） |

`Retry-After` 超过 `max_delay_ms` 时不等待，直接终态并在错误里说明「服务端要求 N 秒后再试」。

**明确不可重试**（写进注释与文档）：上下文超限 400（`Q` 上报过：`maximum context length is 1048576`）、配额/billing、429-但-`insufficient_quota`、鉴权失败、工具配对 400、模型不存在、**用户中止**。

### 二、接口改动（小面积）

- `crates/pipi-core/src/types.rs`：`StreamEvent::Error { message, retry: Option<RetryHint> }` —— `None` 表示终态。改动点共 4 处（`provider.rs:98` 的 `send_error`、`agent_loop.rs:381` 的匹配、`compaction/llm.rs` 的匹配、两个测试的匹配）。
- `crates/pipi-core/src/provider.rs`：在 rig 错误进入事件流的地方（`:416`、`:438`、`:491`、`:495-498`）调用 `classify_rig_error` 并带上 `retry`；`ProviderError(String)` 与 `ResponseError(String)` 用一个小函数做特征匹配（这是唯一的文本匹配，且只在这两类上）。
- `crates/pipi-core/src/agent_loop.rs`：`AgentLoopConfig` 新增 `retry: RetryPolicy`；新增事件 `AgentEvent::RetryStart { attempt, max_attempts, delay_ms, cause }`。
- `StreamOptions`（`types.rs:315-338`）**不动** —— 策略属于循环层，不是传输层，这样 `tests/provider_timeout.rs` 等直接构造 `StreamOptions` 的测试不受波及。

### 三、重试循环落在 agent_loop 一层（唯一对话路径重试点）

`stream_assistant_response`（`agent_loop.rs:265-392`）内部包一层 `'attempt: loop`：

1. 每次尝试重置 `blocks`，重新 `config.provider.stream(...)`（重新发请求，不做断点续流 —— 四家上游都不做）。
2. 收到 `Error { retry: Some(hint) }` 或「非中止的 channel 关闭」时判断能否重试：
   - `blocks` 里已有 `Text`/`Thinking` 增量 → **不重试**，按现在的方式报错（避免重复文本）；
   - 只有 `ToolCall` 块或空 → 允许重试：先 emit `AgentEvent::RetryStart`（前端据此丢弃半截条目），重置 `blocks`；
   - 用户已 abort → 立即返回 `Aborted`，**绝不重试**；
   - 超时类且已重试过 1 次 → 不再重试；
   - 尝试次数耗尽 → 终态，错误消息润色为「已重试 N 次仍失败：{原因}」。
3. 退避 `sleep` 必须与 `abort.wait_aborted()` 用 `tokio::select!` 竞速（现有 `AbortSignal::wait_aborted` 是 50ms 轮询，`types.rs:364-371`），保证停止按钮在退避期间立刻生效。
4. 连接阶段（`provider.rs:414`）目前**没有**与 abort 竞速（既有缺口），这次顺手补上。

### 四、摘要压缩共用同一策略

`compaction/llm.rs:180-239` 的 `summarize`：它把文本攒在本地、出错即丢，**整次调用重试天然安全**（无 UI 增量），所以不套用「未出文本」限制，改为「可重试类就重试」。顺带补一个现存缺口：流干净结束但没有终态帧时，现在会把半截摘要当成功返回（`llm.rs:204-238` 没有 `saw_final` 校验）——加上校验。

### 五、配置与前端

- `crates/pipi-core/src/settings.rs`：新增 `RetrySettings { max_attempts, base_delay_ms, max_delay_ms }`（字段级 default，旧 `settings.json` 可直接读）+ 范围夹取（attempts 1..=5、base 100..=10_000、max 1_000..=120_000）。
- `crates/pipi-core/src/runtime.rs`：三处构造点（`:667` 摘要、`:1124` 子 Agent、`:1931` 主对话）从 settings 取策略塞进 `AgentLoopConfig`。
- `src/types.ts`：`RetrySettings` + `Settings.retry`。
- `src/App.tsx`：设置视图新增导航分组「重试」（三个字段：最大尝试次数 / 起始退避 / 退避上限，附一句说明「超时类最多额外重试 1 次」）。
- `src/chat-runtime.ts`：`AgentEvent` 加 `retry_start`；reducer 加 case —— 丢弃当前 streaming 条目 + 追加一条系统条目（`ChatEntry.kind` 由 `"compaction"` 扩为 `"compaction" | "retry"`）；`src/Chat.tsx` 系统行渲染复用现有样式。
- `src/dev-mock.ts`：演示剧本插一次 `retry_start`，便于浏览器手测。

### 六、测试

1. **`retry.rs` 单元**：分类表逐行断言（`ResponseError`/`ProviderError`/`RequestError` 可直接构造；带状态码的两类通过一个把状态码+`Retry-After` 映射成 `(RetryVerdict, RetryHint)` 的纯函数测，避免构造 rig 内部类型）；退避序列与 `Retry-After` 优先级；`Retry-After` 超上限 → 终态。
2. **`agent_loop.rs` 单元**（复用 `ScriptedProvider`，`agent_loop.rs:748-777`）：新增 `error_turn(msg, retry)` 辅助，断言四个场景 —— `[429 → 正常回复]` 重试后成功且最终不是 error 消息、`[400 → …]` 不重试、`[先吐文本再断流]` 不重试、`[工具调用参数截断 → 正常回复]` 重试且 `RetryStart` 事件被发出；再加「退避期间 abort 立刻中止」。
3. **e2e**（`crates/pipi-core/tests/retry_runtime.rs`）：扩展 `agent_composition.rs:245` 的 `http_response` 支持非 200（新增 `http_response_status`），剧本「首个请求回 429 + `Retry-After: 1`，第二个请求回正常 SSE」，断言会话成功结束、收到 `retry_start`、最终助手消息非 error；再补一条 400 不重试。
4. **回归**：`cargo test -p pipi-core`（230 单测 + 全部集成测试）、`cargo test -p av`、`cargo check --workspace`、`cargo clippy -p pipi-core --all-targets`、`npx tsc --noEmit`、`npm run test:frontend`。
5. **浏览器手测**（走 dev-mock 剧本）：看到重试系统行、正文不重复、停止按钮在退避期间可打断。

### 七、文档

- `README.md`：新增「请求重试」小节（判定表 + 策略默认值 + 「不做断点续流 / 不做上下文超限自动压缩重试」的边界声明），设置项表格补 `retry` 一行。
- `AGENTS.md`：加一条约定 —— 重试只在 agent_loop 一层做（provider 层只负责分类与 `Retry-After`），**禁止把用户中止当可重试错误**，改判定表必须更新表驱动测试。
- 顺带修 `provider.rs:495-498` 的注释（「异常断开」现在会被正确报错，而不是伪装成中止）。

### 八、明确不做（范围边界）

- 请求级断点续流（rig 的 SSE 重连需要消费者在 `Err` 后继续 poll 才生效，且 4xx/5xx 本就不重试；四家上游都没做续流）。
- 上下文超限的「压缩后重试一次」（那是另一个特性；本次按终态处理并在错误里说清原因）。
- 凭据轮换 / fallback 模型 / 成本感知重试预算（hermes 的重型机制，超出本次范围）。
- 不改 `timeout_secs` 的 300s 默认值。
