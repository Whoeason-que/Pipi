## A. 自动压缩阈值改为百分比（agent 级，默认 75%）

1. **`Budget` 增加 `threshold_percent`**：`from_window(context_window, threshold_percent)`；`trigger_tokens() = 窗口 × 百分比 / 100`（窗口 0 仍整体禁用）；`keep_budget()` 以触发线为基数（`trigger − reserve − 2×summary_max`，再 `min keep_recent`）——保留原有的余量结构，只是把「窗口 − 预留」换成百分比触发线。
2. **`AgentDefinition` 增加 `compactThresholdPercent: u8`**（serde default = 75，旧 agent.json 缺字段照常可读）。越界兜底放在访问器里（0 或 >100 → 用默认 75），不 panic、不拒载 —— 与现有「serde 默认用于读旧文件、业务默认在 create/UI 侧给」的约定一致。
3. **调用点**：`send_prompt`、子 Agent 的投影 transform、手动压缩三处统一 `Budget::from_window(model.context_window, def.compact_threshold_percent())`。
4. **`context::prune_cut_index` / `prune_oldest` 改为接收 `budget_tokens`**（投影硬裁的预算 = `trigger_tokens()`），并删掉只被 `needs_compaction` 使用的 `should_compact` —— 触发规则收敛到 `Budget::trigger_tokens` 一处（否则两套口径会漂移）。
5. **UI**：Agent 详情「信息与权限」段加一行数字输入（1–100，带 % 与说明），走既有 `metaDirty` + `saveMeta`（整体 `save_agent`）。同步 `src/types.ts`、`dev-mock.ts` 的 demoAgent、Rust 测试里的 7 处字面量、README 的目录骨架说明。

## B. 手动压缩（遵守分叉/归档设置）

1. **core 抽公共实现**：把 `send_prompt` 尾部那段压缩（runtime.rs:1549-1660）抽成模块级 `async fn run_compaction(...)`，两个入口共用，保证「摘要 → 分叉/归档 → 换会话」完全同一条路径。加 `enum CompactionTrigger { Auto, Manual }`：Auto 保留 `needs_compaction` 预检；Manual 跳过预检（这才是手动的意义），但历史太短时策略仍会返回可读错误（「尚未超出保留预算」→ UI 直接显示）。
2. **`RuntimeState::compact_now(&self, agent_name, event_sink) -> Result<(), String>`**：锁槽校验 agent 匹配 + 非 running → `resolve_model` 取模型与密钥 → `abort.reset()` + `RunningGuard`（占住 running 位，阻止并发一轮；停止按钮可中止这次摘要调用）→ 同步段取 `sessions_dir` / `load_settings().compaction` / `Budget` → `spawn_run` 执行：
   `AgentStart → run_compaction(Manual) → AgentEnd`（不发任何 message 事件）。
   **为什么带一对 Agent 事件**：前端 reducer 在 `compaction_start` 置 `running: true`，而只有 `agent_end` / `hydrate` 会清 —— 只发 Compaction* 会让界面永久卡在「运行中」（停止按钮、模型切换、新会话全被禁）。
3. **IPC**：Tauri 新增 `compact_now` command（`app: AppHandle` 自建 sink，与 send_prompt 同款，前端不传回调）+ `lib.rs` 注册；pipi-server 的 invoke 分发加同名分支（自建 per-connection emitter）。
4. **前端入口**：右栏检查器「状态」tab 的「上下文」行加一枚「立即压缩」小按钮。
   - `InspectorProps` **只加一个** `onCompact?: () => void`；
   - 禁用态从既有 props 派生：`running`、无会话、或 entries 里已有 transient 的 compaction 条目（正在压）；
   - 错误走既有 `onError`；压缩过程中的折叠条目、切会话、侧栏刷新全部复用既有 `compaction_end` / `session-switched` 通道，**不加新事件**。

## C. 测试与验证

- `agents.rs`：旧 agent.json 缺字段取 75；越界值兜底；camelCase 往返序列化。
- `compaction`：百分比触发线与保留预算的算术（含窗口 0 禁用）；手动触发不受阈值限制。
- `context.rs`：`prune_*` 改签名后的既有用例调整。
- e2e（`compaction_runtime.rs`）新增：① **历史未超阈值**时 `compact_now` 仍压缩成功（分叉 + 归档 + `SessionSwitched`），事件序列含 AgentStart/AgentEnd，结束后 `session_running()` 为 false；② 历史太短时返回可读错误；③ 运行中调用被拒。
- 前端：`tsc` + `vite build`；浏览器实测按钮禁用态与点击后的折叠条目。
- 跑 `tests/compaction_corpus.rs`（AGENTS.md 要求）。

## D. 文档

README 压缩一节补「阈值 = 窗口 × 百分比（`agent.json` 的 `compactThresholdPercent`，默认 75）」与手动压缩入口；目录骨架/组成表补该字段。AGENTS.md 补一句：阈值口径只有 `Budget::trigger_tokens` 一处，手动与自动共用 `run_compaction`。

## 已知取舍

- 阈值只做 agent 级（不设全局默认）—— 按你说的「在 agent 设置里调节」；将来若要全局默认，需要引入「全局默认 + agent 覆盖」的合并层（目前没有先例）。
- 手动压缩遵守分叉/归档设置 ⇒ 点一下会看到侧栏多一个会话、原会话进归档（你已确认）。
- 百分比越低压得越勤；因保留预算以触发线为基数，窗口很小时保留的原文会更少（继承原有的保守性）。