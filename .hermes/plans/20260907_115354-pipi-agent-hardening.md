# Pipi Agent 可靠性与安全闭环实施计划

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** 将 Pipi 从“可运行的单 Agent vertical slice”收敛为可安全执行、可中断、可恢复、可压缩、可审计的单 Agent 产品，再扩展 MCP 和 subagent。

**Architecture:** 保留 `pipi-core` 的 Rust 小核心和文件优先模型。协议层继续使用现有 `rig`，不复制其他 Agent 的 provider 适配器；从 Pi/Codex/Hermes/OpenCode 只移植已经稳定的语义、状态机和数据模型，并通过薄适配层接入 Pipi。高风险能力以 feature gate 和明确的运行时策略隔离，Tauri 只负责 IPC 转发。

**Tech Stack:** Rust 2021、Tauri 2、React/TypeScript、`rig`、`tokio`、`serde`；增量考虑 `thiserror`、`tokio-util`、`tracing`、`backon`、`wiremock`、`tempfile`、官方 `rmcp`、Linux `landlock`。

---

## 0. 范围、基线与依赖准入

### 目标

先固定行为基线，避免把安全修复、可靠性修复和功能扩展混成一个不可审查的大改动。

### 约束

- 不修改或回滚当前工作区已有的图标二进制变更。
- 不把 `danger-full-access`、已有 session JSONL 或 Agent 配置静默迁移成另一种语义。
- 每个阶段单独完成：测试 RED → 最小实现 → GREEN → diff 审查。
- 不引入数据库；JSONL 仍是会话事实来源。
- 任何从上游移植的文件在模块头部记录上游项目、路径和固定 commit；先确认许可证兼容性。

### 依赖准入规则

| 能力 | 选择 | 原因 | 不采用 |
|---|---|---|---|
| LLM 协议 | 保留 `rig` | 当前已有 provider 映射，避免手写 SSE | 不复制 Pi/OpenCode provider SDK |
| 取消 | `tokio-util::sync::CancellationToken` | 与 Tokio 任务生命周期一致 | 不继续扩展自定义轮询型 abort |
| 错误 | `thiserror` | 保留结构化领域错误，在 Tauri 边界再转 `String` | 不用 `anyhow` 抹平错误分类 |
| 日志 | `tracing` + 现有 Tauri 日志出口 | session/run/tool 可关联 | 不用散落 `println!` |
| retry 延迟 | `backon`（仅作为 delay/backoff engine） | 支持异步退避和 jitter | 不直接套 middleware 重放整轮请求 |
| Provider mock | `wiremock`（dev-dependency） | 可模拟 429、5xx、断流和延迟 | 不用真实 API 验证重试 |
| 临时文件 | `tempfile`（dev，必要时 runtime） | 原子保存和隔离测试 | 不手写临时目录清理协议 |
| MCP | 官方 `rmcp` | 协议和 transport 由 SDK 维护 | 不从 Hermes/OpenCode 复制完整 MCP client |
| Linux 文件隔离 | `landlock`，target-specific optional dependency | OS 级边界，适配 Ubuntu | 不把字符串检查当 sandbox |

初始阶段不加入 `reqwest-middleware`、完整 Codex sandbox 源码或大型异步运行时替换；它们会隐藏 Pipi 的副作用/状态契约。

### 验证

```bash
git status --short --branch
git diff --check
cargo metadata --no-deps --format-version 1
cargo tree -p pipi-core --edges normal
cargo test -p pipi-core --all-targets
npm run build
npx tsc --noEmit
```

保留当前已知基线：核心测试曾通过 76 项，前端构建和类型检查通过；strict clippy 的既有失败单独记录，不与本计划混淆。

---

## 1. 先修 `send_prompt` 的事务边界与错误状态

**优先级：P0**

**Files:**

- Modify: `src-tauri/src/chat.rs:217-337`
- Modify: `src-tauri/src/chat.rs` 的 `ChatState`/`Session` 定义
- Modify: `crates/pipi-core/src/session.rs`
- Test: `src-tauri` 相关测试或 `crates/pipi-core/tests/` 跨层测试

### RED

增加测试覆盖：

1. model 解析失败后，原 session 仍在 state 中；
2. API key 缺失后，原 session 仍可继续发送；
3. tool context 构造失败后不留下不可执行的用户消息；
4. `SessionWriter::append_message` 失败时向上返回错误，而不是静默吞掉；
5. 所有错误路径最终 `running == false`。

### 实现

- 不要在所有前置检查之前 `slot.take()`；可以先借用现有 session，或使用可恢复的 transaction guard。
- 只有 model、key、workspace、tool registry 和 config 全部准备成功后，才设置 `running`、追加用户消息并 spawn loop。
- 若必须取出 session，使用 RAII/显式恢复路径保证每个 `?` 返回前重新放回 `ChatState`。
- 把 provider、tool、session 错误转换为结构化 core error；Tauri 最外层仍返回用户可读的 `String`。
- 为 run/session 分配关联 ID，供日志和 UI 错误事件使用。

### GREEN 验收

- 任意 preflight 失败都不丢 session；
- JSONL 不出现没有对应执行状态的孤立用户消息；
- 后续发送仍能继续同一 session；
- `cargo test -p pipi-core --all-targets` 和对应 Tauri 测试通过。

---

## 2. 统一错误事件、取消和前端状态收敛

**优先级：P0**

**Files:**

- Modify: `crates/pipi-core/src/types.rs:175-187, 286-332`
- Modify: `crates/pipi-core/src/agent_loop.rs`
- Modify: `src-tauri/src/chat.rs:188-205, 309-334`
- Modify: `src/Chat.tsx:55-67, 122-200, 216-228`
- Test: Rust event tests；前端事件 reducer 测试（若新增测试工具，单独评估依赖）

### RED

用错误和 abort fixture 验证：

- provider 返回错误时 UI 显示 `errorMessage`，不出现空助手气泡；
- stop 后同一 session 的下一次发送可以正常开始；
- loop 无论 completed、error、aborted 都发出唯一 `agent_end`；
- `running` 在 UI、Tauri、session 三层最终一致。

### 实现

- 使用 `tokio_util::CancellationToken`，或让现有 `AbortSignal` 内部包装它；保留对现有 API 的最小兼容层。
- stop 时取消当前 token；新 turn 创建 child token，不能复用永久 cancelled token。
- React `messageText()` 和消息渲染同时处理 `errorMessage`、`stopReason`、`isError`。
- 在 `agent_end` 中统一清理 `running`、stream key、未完成工具状态和当前 turn 队列。
- 错误消息不得包含 API key、完整 Authorization header 或敏感环境变量。

### GREEN 验收

- provider error、tool error、abort 三种 UI 都可区分；
- stop → send 的顺序测试通过；
- 不产生重复 `agent_end` 或卡死的 running 状态。

---

## 3. ProviderRetryPolicy：移植语义，不重放副作用

**优先级：P0**

**Files:**

- Create: `crates/pipi-core/src/retry.rs`
- Modify: `crates/pipi-core/src/provider.rs`
- Modify: `crates/pipi-core/src/agent_loop.rs`
- Modify: `crates/pipi-core/Cargo.toml`
- Test: `crates/pipi-core/tests/provider_retry.rs`

### 上游参考

- Pi：`packages/ai/src/utils/provider-retry.ts` 的 Retry-After、退避和可中断等待语义。
- Hermes：`agent/retry_utils.py` 的 provider 错误分类和 jittered backoff。
- OpenCode：`packages/opencode/src/session/retry.ts` 的最大次数和错误分类。
- Codex：turn 内区分可重试请求与已经产生工具副作用的 turn。

### RED

用 `wiremock`/等价 mock 覆盖：

1. 429 + `Retry-After`；
2. 408/5xx；
3. 首 token 前网络断开；
4. 已产生 tool call 后连接断开；
5. abort 发生在退避等待期间；
6. 达到最大次数后保留最终错误分类。

### 实现边界

- `RetryPolicy` 只决定“错误是否可重试、等待多久、还能试几次”；不要让通用 middleware 自动重放完整 agent turn。
- 先检查 `rig 0.42` 暴露的 provider 错误元数据；能拿到 status/header 时使用 Retry-After，拿不到时不得伪造精确服务端等待时间。
- 每个请求尝试发出可渲染的 retry event，包含 attempt、分类和 delay，不包含凭据。
- 尚未发出工具调用时可重建 stream 请求；已经执行工具后只能恢复当前状态，禁止透明重放有副作用的 tool call。
- 使用 `backon` 只负责异步 backoff/jitter，取消由 `CancellationToken` 控制。

### GREEN 验收

- 429/5xx/断流都按策略处理；
- abort 能打断退避；
- 工具副作用只执行一次；
- 错误分类能区分 rate-limit、server、network、auth、context-overflow；
- 没有 retry 时行为与当前成功路径保持一致。

---

## 4. 接通启发式裁剪，再实现结构化 compaction

**优先级：P0**

**Files:**

- Modify: `crates/pipi-core/src/context.rs:1-101`
- Modify: `crates/pipi-core/src/agent_loop.rs:246-263`
- Modify: `src-tauri/src/chat.rs:288-305`
- Modify: `crates/pipi-core/src/session.rs:15-32`
- Test: `crates/pipi-core/tests/context_compaction.rs`

### RED

覆盖：

- context 超过预算时确实调用 transform；
- tool call/result 不被拆开；
- 压缩失败时原历史不丢失；
- provider overflow 能进入同一 compaction 状态机；
- 压缩后只自动继续一次，避免无限循环。

### 实现顺序

1. 先把现有 `prune_transform` 接入 `AgentLoopConfig`，使 M1 保底行为真实生效。
2. 将 Pi 的 compaction 语义移植为 Rust：结构化摘要、保留最近完整轮次、`previousSummary`、摘要失败 fallback。
3. 为 JSONL 增加向后兼容的 `Compaction`/`BranchSummary` entry；不要改已有 `MessageEntry` 字段含义。
4. 引入 tool-call/result 配对 sanitizer；压缩窗口不得留下孤儿 tool result。
5. compaction 成功后写 checkpoint，再由 loop 自动继续；失败只记录错误，不删除原消息。

### 不移植

不直接复制 Pi 的 provider adapter 或整个 harness；Pipi 继续通过 `rig` 发摘要请求，并将摘要状态保存在自己的 JSONL 模型中。

### GREEN 验收

- 长会话可跨多次请求运行；
- JSONL 重开后能恢复摘要和最近消息；
- compaction 前后 tool pair 合法；
- context 超限不再直接变成空响应或无分类 provider error。

---

## 5. 收紧 workspace 与 OS 执行安全

**优先级：P0**

**Files:**

- Modify: `crates/pipi-core/src/permissions/mod.rs:261-372`
- Modify: `crates/pipi-core/src/permissions/safety.rs:25-145`
- Modify: `crates/pipi-core/src/tools/mod.rs`
- Modify: `crates/pipi-core/src/tools/read.rs:63-70`
- Modify: `crates/pipi-core/src/tools/write.rs`
- Modify: `crates/pipi-core/src/tools/edit.rs`
- Modify: `crates/pipi-core/src/tools/bash.rs`
- Optional: `crates/pipi-core/src/sandbox/linux.rs`
- Test: `crates/pipi-core/tests/security_boundary.rs`

### RED

必须先有拒绝测试：

- workspace 内的正常读写；
- `..` 穿越；
- workspace 内 symlink 指向外部；
- 外部文件的 hard link/重命名竞态（在平台能力允许时）；
- `bash -c`、shell wrapper、无空格连接符、命令替换；
- 网络和环境变量策略；
- read-only、workspace-write、danger-full-access 三种模式。

### 实现顺序

1. 引入 `WorkspaceRoot`，启动时保存真实 canonical root。
2. 统一 read/write/edit/bash 的路径检查；已有文件检查真实路径，目标不存在时检查 canonical parent，并拒绝边界 symlink。
3. 新 Agent 默认改为 `workspace-write` 或显式审批；已有 `agent.json` 的明确模式原样保留。
4. Linux 增加 `landlock` optional dependency，在子进程 spawn 边界落实文件系统规则；Landlock 不覆盖的网络/进程语义要明确记录。
5. 命令字符串启发式继续保留，但只作为前置拒绝/提示，不再宣称它是完整 sandbox。
6. 不支持 OS 级隔离的平台必须在 UI 中显示实际模式，而不是显示“安全沙箱”。

### GREEN 验收

- 任何工具都不能通过 symlink 写出 workspace；
- read 不可读取 workspace 外部路径，除非用户明确选择 full access；
- 默认新 Agent 不会直接执行 danger-full-access；
- Linux sandbox 失败时 fail-closed，不自动降级为 unrestricted execution；
- 安全测试在 CI 和本机都能重复运行。

---

## 6. Session tree、fork、crash replay 与原子写入

**优先级：P1**

**Files:**

- Modify: `crates/pipi-core/src/session.rs`
- Modify: `src-tauri/src/chat.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src/App.tsx`
- Modify: `src/Chat.tsx`
- Test: `crates/pipi-core/tests/session_recovery.rs`

### 实现

- 保持 append-only JSONL；增加显式 `active_path`、fork/branch、revert 所需的 core API。
- 新增 Tauri IPC：列出树、选择 active path、fork session；UI 不直接解析 JSONL。
- 保持 `id/parentId/seq` 兼容；为 compaction checkpoint 和 branch summary 增加新 entry 类型。
- `SessionWriter` 追加后检查 flush；Agent/Settings 写入使用 `tempfile` + rename 的原子替换路径。
- 恢复时区分正常尾行、损坏尾行、半写 entry 和重复 replay；失败要可见而不是静默跳过所有错误。

### GREEN 验收

- 线性 session 旧文件仍能打开；
- fork 后两个分支互不污染；
- 崩溃恢复不重复执行 tool；
- 损坏尾行只丢弃损坏 entry，并显示恢复提示；
- UI 可以选择活动路径。

---

## 7. 建立跨层回归矩阵

**优先级：P1，穿插在每个阶段，不得最后补**

### 最小测试层次

1. `pipi-core` 单元测试：纯函数、状态机、权限规则。
2. Provider mock integration：`wiremock` 模拟流式成功、429、断流、tool call、overflow。
3. Session integration：临时目录中创建 Agent、写消息、错误、stop、重开、fork、compaction。
4. Security integration：真实临时文件和 symlink，不用只测字符串。
5. Tauri command/event integration：验证 invoke 参数、event 顺序、agent_end、session-stats。
6. Desktop E2E：等 core/IPC 稳定后再加入，不用 UI E2E 掩盖核心测试缺口。

### 每个 P0 的完成门槛

- 至少 1 个 core regression；
- 至少 1 个跨层测试；
- 正常路径和错误路径都被验证；
- `cargo fmt --check`、`cargo check --workspace`、`cargo test -p pipi-core --all-targets`；
- `npm run build`、`npx tsc --noEmit`；
- 安全边界改动还要运行专门的拒绝测试。

---

## 8. 最小 MCP vertical slice

**优先级：P1，必须在 P0 全部稳定后开始**

**Files:**

- Create: `crates/pipi-core/src/mcp.rs`
- Modify: `crates/pipi-core/src/agents.rs`
- Modify: `crates/pipi-core/src/tools/mod.rs`
- Modify: `crates/pipi-core/Cargo.toml`
- Modify: `src-tauri/src/chat.rs`
- Test: `crates/pipi-core/tests/mcp_stdio.rs`

### 实现顺序

1. 使用 `rmcp` 接入单一 stdio server；
2. 完成 initialize、tools/list、schema 转换和工具调用；
3. 增加启动超时、调用超时、关闭、stderr 隔离和错误结果；
4. 将 MCP tool call/result 写入 session；
5. 再实现 tools/list changed、重连、HTTP/OAuth。

`McpServerConfig` 在上述链路完成前只能称为配置声明，不能在 README/UI 中称为“已支持 MCP”。MCP 子进程必须继承同一 workspace/环境安全策略，不能成为 sandbox 逃逸旁路。

---

## 9. Skills、subagent 与产品层扩展

**优先级：P2**

P0/P1 稳定后再做：

- Skills 渐进式加载和 Agent 管理 UI；
- memory 召回策略；
- 运行中 steering/follow-up 的 UI 接线；
- 动态工具、插件 hooks；
- subagent runtime 和任务生命周期；
- 图片/多模态 provider 映射；
- session tree 完整 UI。

从 Hermes/Codex/OpenCode 只采纳协议和状态不变量，不复制完整桌面/网关/插件平台。Pipi 的产品边界仍然是本地优先、Agent-first、文件可审计。

---

## 10. 执行顺序与停止条件

### 推荐批次

1. 基线和依赖准入；
2. `send_prompt` 事务 + error/abort UI；
3. ProviderRetryPolicy；
4. context prune 接线 + compaction；
5. workspace/sandbox；
6. session tree/recovery；
7. 跨层回归矩阵；
8. MCP stdio；
9. P2 产品扩展。

### 每批次必须输出

- 修改文件和数据契约；
- 上游移植来源及固定 commit；
- RED/GREEN 测试结果；
- `git diff --check` 和工作区状态；
- 未解决风险。

### 停止条件

- 任一 P0 安全测试失败：停止 MCP/subagent 开发；
- Provider retry 不能证明“工具副作用不重复”：停止扩大重试范围；
- compaction 会丢失历史或破坏 tool pair：停止 session tree 开发；
- OS sandbox 在当前平台不可用：fail-closed 并明确显示限制，不得静默降级；
- 第三方库 API 不稳定或许可证/维护状态不满足准入：隔离在适配层，不能污染核心数据模型。

**建议第一批只实施第 1、2 阶段，完成后再审查 diff 和测试结果；不要一次性实现整份路线图。**
