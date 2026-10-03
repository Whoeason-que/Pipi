# 实施计划：Compaction + M2 收尾 + glob/grep（解耦设计）

三条工作线触碰的模块基本不相交，可独立开发、独立测试、独立提交：

```
工作线 A（compaction）  → 新模块 compaction.rs + session.rs/runtime.rs 小改
工作线 B（M2 收尾）     → agents.rs + harness/system_prompt.rs + commands + App.tsx
工作线 C（glob/grep）   → 新文件 tools/glob.rs、tools/grep.rs + 3 处列表登记
```

---

## 工作线 A：LLM 摘要式 Compaction

**设计原则：压缩决策与压缩执行解耦。** runtime 只决定「何时」和「持久化」，compaction 模块只负责「怎么压」。

### A1. 新模块 `crates/pipi-core/src/compaction.rs`
- `pub async fn compact(provider, model, options, messages, abort) -> Result<Compacted, String>`
  - 摘要 prompt：把历史序列化为文本，构造 `Context { tools: vec![], messages: [user(历史+摘要指令)] }`
  - 复用 `provider.stream(...)`（空 tools 即纯文本调用），循环收集 `StreamEvent` 直到 `Done`，取最终消息文本
  - 保留策略对齐 pi：摘要替换旧轮次 + **保留最近若干完整轮次原文**（从 user 边界切，够放预算为止，复用 `prune_oldest` 的切点逻辑思路）
  - 返回 `Compacted { messages: Vec<Message> }`，第一条是与 pi 一致的 User 消息（`<compaction summary>` 包裹）
- 单元测试：prompt 构造、保留轮次切点、失败透传（不 mock LLM，只测纯函数部分）

### A2. 会话持久化：`session.rs` 新增 `EntryKind::Compaction`
- `Compaction { summary: String, source_tip: String }`（新 variant，serde snake_case，纯增量、旧文件不受影响；append-only 语义不破坏，fork 原样拷贝天然兼容）
- `append_compaction(&mut self, summary)` 方法
- **抽取公共函数** `pub fn rebuild_messages(entries) -> Vec<Message>`：遇到 Compaction 条目时丢弃其之前的所有 Message、注入摘要 User 消息 —— 消除 runtime.rs 两处（L567/L636）重复的重建逻辑

### A3. runtime 接线 `runtime.rs`
- **触发**：`send_prompt` 的 run 任务里，`run_agent_loop` 返回后（L961-964 之间）检查 `should_compact(estimate_context_tokens(...), model.context_window, DEFAULT_RESERVE_TOKENS)`（仅 `context_window > 0` 时）；超限则调 `compaction::compact`，成功后：替换 `session.messages`、`append_compaction` 落盘
- **保底**：`transform_context: Some(prune_transform(...))`（L912，终于接上死代码）—— 摘要失败或压缩后仍超限时，请求前硬裁剪兜底
- **事件**：`AgentEvent` 加 `CompactionStart` / `CompactionEnd { summary }` 两个 variant，经 completion_sink 发出（摘要调用的 usage 不计入会话统计）
- 摘要 LLM 调用期间 abort 可中断（传 AbortSignal）

### A4. 前端
- `chat-runtime.ts` AgentEvent 类型加两个 variant；chatReducer 插入一条系统样式的 ChatEntry（显示「已压缩上下文」，摘要可折叠查看）
- `Chat.tsx` 渲染分支加样式；stats 的 contextUsed 压缩后自然回落，无需改

---

## 工作线 B：M2 收尾（编辑 + memory 渐进召回）

### B1. `agents.rs` 校验收口
- 从 `create_agent`（L370-394）抽取 `pub fn validate_definition(def) -> Result<(), String>`（工具子集 ⊆ KNOWN_TOOLS、workspace 绝对路径、名称合法），`create_agent` 与 `save_agent` 都调用 —— 防 UI 全字段编辑绕过校验

### B2. agent.json 全字段编辑（前端）
- `App.tsx` AgentDetail（L1141-1345）：把只读 section 改为可编辑表单（description / workspace / tools checkbox / bash 白黑名单 / sandbox / mcp_servers 占位），复用 CreateForm（L1357-1614）的表单模式，保存走现有 `save_agent`
- 权限改动在下次会话生效（system prompt / ToolRegistry 每轮重建），无需额外通知机制

### B3. AGENTS.md / memory 编辑器
- 核心新增（agents.rs）：`read_agent_file(name, rel_path)` / `write_agent_file(name, rel_path, content)` —— 限制在 agent 目录内、仅 `.md`、复用 `ensure_real_file` 防符号链接
- Tauri command 两个 + 注册 4 处：`src-tauri/src/commands.rs`、`src-tauri/src/lib.rs`、`crates/pipi-server/src/main.rs`（invoke_command match）、`src/dev-mock.ts`
- 前端：AgentDetail「文件」区块（L1320-1327）加编辑入口；textarea 复用 Chat.tsx composer 模式（自动增高 + IME composition 处理）
- AGENTS.md 保存后无需通知核心（system prompt 每次 send_prompt 重建）

### B4. memory 渐进召回（仿 skills 渐进披露）
- `harness/system_prompt.rs`：`BuildSystemPromptOptions` 加 `memory_files: Vec<MemoryMetadata>`（name/path/摘要），新增 `append_memory_index()`（仿 `append_skills` L169-198）—— 只注入文件名 + 首个标题/首行摘要 + 路径，正文由模型用 read 工具按需读
- `agents.rs` `build_system_prompt_with_tools`：扫描 `def.memory_dir()`（复用 tools/memory.rs 的 `collect_markdown` 逻辑，抽为共享函数），生成索引
- 空目录时不注入任何内容（零打扰）

---

## 工作线 C：glob / grep 工具

### C1. 依赖
- `crates/pipi-core/Cargo.toml` 加 `glob = "0.3"`、`walkdir = "2.5"`、`regex = "1.13"` —— 三者均已在 Cargo.lock 依赖树中，零新增编译成本

### C2. `tools/glob.rs`（照 read.rs 模式）
- 参数：`pattern`（相对 workspace 的 glob，如 `src/**/*.rs`）、可选 `path`（搜索根）
- 用 `resolve_read_path` 约束根目录（防 `..` / 符号链接逃逸），walkdir 遍历 + glob 匹配
- 输出按修改时间排序的路径列表，超量用 `truncate_head` 截断

### C3. `tools/grep.rs`
- 参数：`pattern`（regex）、可选 `path`（搜索根）、可选 `include`（glob 过滤文件名）
- regex 搜索文本文件（跳过二进制），输出 `file:line: text`，`truncate_head` 截断
- 只读工具：`requires_sequential()` 保持默认 false，不过 `assess_bash`，只受 `tool_enabled` 开关 + read_roots 约束

### C4. 登记（3 处列表）
- `permissions/mod.rs:232` `KNOWN_TOOLS` 加 `"glob"`, `"grep"`（serde default 使新旧 agent.json 默认开启）
- `tools/mod.rs`：`pub mod glob/grep` + `for_context` 加两个 `tool_enabled` 分支
- `src/App.tsx:74` 前端 `KNOWN_TOOLS` 加两项；`src/dev-mock.ts:13` mock 数组加两项
- 系统提示的工具描述经 `tool_snippets` 自动注入，无需改 system_prompt.rs；命名 `grep` 后 system_prompt 里「没有 grep 时用 bash 搜索」的 guideline 会自动让位，属预期

---

## 验证与收尾

- 每条工作线：`cargo test -p pipi-core` + `cargo check --workspace` + `cd src-tauri && cargo clippy` + `npx tsc --noEmit`
- 分 3 个 commit 提交（A/B/C 各一），互不阻塞、可独立回滚
- 更新 README：
  - compaction 从「有意推迟」清单移到模块映射表（对应 pi 的 compaction 完整移植）
  - 路线图勾选 Skills（已完成）、fork（已完成）、compaction（本次）；测试数描述更正
  - 「Agent 的组成」表加 glob/grep 工具与 memory 索引注入说明
- AGENTS.md 约定遵守：serde camelCase、JSONL 字段名与 pi 一致（Compaction 条目为增量 variant）、业务逻辑只进 pipi-core、新依赖有据（均在已有依赖树中）

## 风险与对策
- 摘要 LLM 调用失败 → 不压缩、发 CompactionEnd(error) 事件提示，prune_transform 保底，不阻塞下一轮
- 压缩后 toolResult 孤儿 → 摘要+保留轮次都从 user 边界切，与 prune_oldest 同一不变量
- 前端旧会话含 Compaction 条目 → rebuild_messages 统一处理，fork/open 两条路径都走它