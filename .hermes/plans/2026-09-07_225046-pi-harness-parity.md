# Pipi Pi-Compatible Harness Phase 1 Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** 先让 Pipi 在 harness 的数据流和行为边界上尽可能接近 Pi，再在稳定兼容层之上逐步加入 Pipi 自己的信任、审计和验证哲学。

**Architecture:** 保留 Pipi 现有 Rust agent loop、Tauri 接线和工具执行边界；新增一个独立的 `harness` 层，先复制 Pi `coding-agent` 的 system-prompt、project-context、skills metadata 和 session-facing contract。第一阶段不复制 Pi 的 TUI、extension/package manager、MCP 或完整 compaction 实现，也不引入 `.agent/` 私有目录约定。

**Tech Stack:** Rust workspace、`pipi-core`、现有 Rig provider、Tokio、JSONL session storage；Pi 参考源码必须固定到具体 commit，不依赖 moving `main`。

---

## 当前上下文与约束

- 现有上下文拼装主要位于 `crates/pipi-core/src/agents.rs`。
- 现有项目文档发现位于 `crates/pipi-core/src/project_doc.rs`，skills 位于 `crates/pipi-core/src/skills.rs`。
- 现有 loop 位于 `crates/pipi-core/src/agent_loop.rs`，已经包含 steering/follow-up、tool loop、abort 等 Pipi 当前能力；不要在本阶段重写它。
- `src-tauri/src/chat.rs` 的 generation-aware running state、测试和现有未提交修改必须保持不变。
- 保留 `app-icon.png`、`src-tauri/icons/*` 和已有 `.hermes/plans/20260907_115354-pipi-agent-hardening.md`，不回滚、不暂存、不提交。
- 第一阶段只做 Pi-compatible baseline；`TrustLevel`、prompt injection 防护、审计事件、compaction、MCP、subagent 属于后续阶段。
- Rust 执行层始终是最终权限边界；system prompt 不得被当作安全控制。

## Pi parity 的目标范围

### 本阶段复制

1. `BuildSystemPromptOptions` 语义：custom prompt、selected tools、tool snippets、guidelines、append prompt、cwd、context files、skills。
2. Pi 默认 system prompt 的章节顺序和条件行为。
3. context file loader 的候选文件、祖先目录顺序、去重和 agent-global context。
4. skills 的 metadata-only 注入；`SKILL.md` 正文继续按需通过工具读取。
5. prompt template 的独立加载边界，但先不实现 Pi CLI 的 slash command/UI。
6. session-facing 的 base prompt / current prompt 区分，使后续 tool 或 extension 变化可以重新构造 prompt。

### 本阶段明确不复制

- Pi TUI、extension runtime、package manager、theme loader。
- Pi 的 Node/TypeScript 目录结构和 API 形状。
- 任何未经审查的第三方大模块。
- Pipi 自有 trust hierarchy、prompt injection scanner、compaction policy。
- `.agent/`、`.agents/` 自动扫描。若以后需要，单独设计兼容规范。

---

## 实施顺序

### Task 1: 固定 Pi 参考基线并建立行为清单

**Objective:** 固定 Pi 参考 commit，并把需要复制的输入、输出和边界写成可测试清单。

**Files:**
- Create: `.hermes/references/pi-harness-<sha>.md`（仅记录 commit、参考文件和差异说明）
- Inspect: `crates/pipi-core/src/agents.rs`
- Inspect: `crates/pipi-core/src/project_doc.rs`
- Inspect: `crates/pipi-core/src/skills.rs`
- Inspect: `crates/pipi-core/src/agent_loop.rs`

**Steps:**

1. 查询 Pi 仓库的具体 commit SHA，保存 SHA；后续不再以 `main` 的行号或未固定源码作为规范。
2. 固定以下参考文件：`packages/agent/src/agent-loop.ts`、`packages/agent/src/harness/system-prompt.ts`、`packages/agent/src/harness/context.ts`、`packages/coding-agent/src/core/system-prompt.ts`、`packages/coding-agent/src/core/resource-loader.ts`、`packages/coding-agent/src/core/skills.ts`、`packages/coding-agent/src/core/prompt-templates.ts`。
3. 为每个差异标记 `copy-now`、`Pipi-existing`、`defer`，防止第一阶段混入自有设计。

**Validation:** 参考文件全部来自同一个 SHA；行为清单能覆盖空 context、custom prompt、工具变化、skill metadata 和多层 `AGENTS.md`。

### Task 2: 抽出 Pipi 的 harness 边界

**Objective:** 将 prompt 组装从 Agent 配置中抽出，但不改变对外的 AgentContext 和 provider 调用路径。

**Files:**
- Create: `crates/pipi-core/src/harness/mod.rs`
- Create: `crates/pipi-core/src/harness/system_prompt.rs`
- Modify: `crates/pipi-core/src/lib.rs`
- Modify: `crates/pipi-core/src/agents.rs`
- Test: `crates/pipi-core/src/harness/system_prompt.rs` 或对应测试模块

**Steps:**

1. 定义与 Pi 对齐但使用 Rust 命名的输入结构：`BuildSystemPromptOptions`、`ContextFile`、`SkillMetadata`。
2. 实现纯函数 `build_system_prompt(options)`；它只负责拼接，不读取文件、不检查权限、不访问 session。
3. 保留 Pi 的条件语义：custom prompt 替换默认 prompt，但 append prompt、project context 和 skills 仍按对应规则追加；selected tools 控制工具说明和工具 guidelines。
4. 让 `agents.rs` 只负责收集输入并调用 harness；保留现有 `AgentContext.system_prompt` 输出类型和 `provider.rs` 的 `preamble` 传递链路。
5. 暂不加入 `TrustLevel`、hash、token budget 等 Pipi 扩展字段，避免破坏 Pi parity 的可比性。

**TDD:**

1. 先写空输入、默认工具、custom prompt、append prompt、多个 context file 和 skills metadata 的失败测试。
2. 运行 `cargo test -p pipi-core harness::system_prompt`，确认测试先失败。
3. 实现最小纯函数并使测试通过。
4. 添加 golden/snapshot 字符串断言，固定章节顺序和 XML-like 包装格式。

### Task 3: 复制 Pi 的 project context loader 语义

**Objective:** 将现有只识别 `AGENTS.md` 的发现逻辑收敛为独立 resource loader，并保持路径边界可验证。

**Files:**
- Create: `crates/pipi-core/src/harness/resources.rs`
- Modify: `crates/pipi-core/src/project_doc.rs`
- Modify: `crates/pipi-core/src/agents.rs`
- Test: `crates/pipi-core/src/harness/resources.rs`

**Steps:**

1. 实现 Pi 当前 loader 的候选文件顺序：`AGENTS.override.md`、`AGENTS.md`、`AGENTS.MD`、`CLAUDE.md`、`CLAUDE.MD`；若 Pipi 的固定参考 SHA 不同，以该 SHA 为准。
2. 保留 Agent-global 文件和 cwd 到 project root 的祖先文件收集，输出顺序与 Pi 一致；同一路径 canonicalize 后去重。
3. 所有发现必须限制在项目 root / Pipi Agent root 范围内；禁止通过 `..` 或 symlink 把项目上下文扩展到任意外部目录。
4. 为缺失文件、大小上限、嵌套 cwd、大小写候选、重复路径和 symlink 建立测试。
5. `agents.rs` 不再直接拼接文件正文，只消费 loader 返回的 `ContextFile` 列表。

**Prerequisite:** 在允许模型通过返回路径读取 context/skill 正文前，先补齐 `crates/pipi-core/src/tools/read.rs` 和 `crates/pipi-core/src/tools/mod.rs` 的 workspace containment 测试与实现；绝对路径不能绕过 workspace 边界。

### Task 4: 复制 Pi 的 skills metadata loader

**Objective:** 让 user skills 和 project skills 的发现、校验、metadata 注入方式接近 Pi，但继续按需读取正文。

**Files:**
- Modify: `crates/pipi-core/src/skills.rs`
- Modify: `crates/pipi-core/src/harness/resources.rs`
- Modify: `crates/pipi-core/src/agents.rs`
- Test: `crates/pipi-core/src/skills.rs`

**Steps:**

1. 保留 Pipi Agent root 的 `skills/`；增加 Pi-compatible project skills root（对应 Pi 的 project config directory，具体目录名以固定 SHA 的 `CONFIG_DIR_NAME` 为准）。
2. 复制 Pi 的 skill discovery 规则：目录中存在 `SKILL.md` 时视为 skill root；否则扫描允许的直接 markdown 文件并递归子目录；跳过隐藏目录和依赖目录。
3. 校验 skill name/description；诊断无效 skill 而不是让一个坏 skill 使整个 Agent 启动失败。
4. system prompt 只注入 name、description、绝对路径和必要的 invocation metadata；正文仍由 `read` 工具按需获取。
5. 明确 precedence 和 duplicate 规则，并以测试固定，不把 Pipi 的未来 trust policy 混入本阶段。

### Task 5: 对齐 host/session 的 base prompt 生命周期

**Objective:** 让 prompt builder 与当前 session 状态解耦，支持 Pi 风格的 base prompt 重建而不重写 Agent loop。

**Files:**
- Modify: `crates/pipi-core/src/agents.rs`
- Modify: `crates/pipi-core/src/context.rs`
- Inspect/Modify only if required: `crates/pipi-core/src/agent_loop.rs`
- Test: `crates/pipi-core/src/agents.rs`、`crates/pipi-core/src/agent_loop.rs`

**Steps:**

1. 区分 immutable/base system prompt 和当前 turn 的 context messages。
2. 工具集合、cwd 或资源列表变化时只重建 base prompt，不改变 session JSONL message history。
3. 保留现有 steering/follow-up、abort、generation-aware running state 行为。
4. 加入测试证明 prompt rebuild 不会丢失 session messages、不会重复追加 context、不会让旧 run 覆盖新 run 状态。
5. 暂不实现自动 compaction；只留下 `prepare_next_turn` 或等价扩展点。

### Task 6: 端到端 harness parity 验证

**Objective:** 证明 Pi-compatible harness 已经真正接入 Tauri → core → provider，而不是只存在于孤立单元测试。

**Files:**
- Create/Modify: `crates/pipi-core/tests/harness_integration.rs`（若现有测试布局更合适则放入对应模块）
- Modify only if required: `src-tauri/src/chat.rs`
- Test fixtures: `crates/pipi-core/tests/fixtures/harness/`

**Tests:**

1. fixture project 中的多层 `AGENTS.md`/override 文件按预期进入 prompt。
2. user/project skills 只进入 metadata，模型调用 read 后才能拿到正文。
3. custom prompt、tool selection 和 cwd 变化会产生预期 prompt，但不会修改 session history。
4. read 工具拒绝 workspace 外绝对路径、`..` 穿越和指向 workspace 外的 symlink。
5. provider mock 收到的 preamble 与 harness snapshot 一致。
6. `cargo test -p pipi-core --all-targets`。
7. `cargo test -p pipi --lib chat::tests`。
8. `cargo check --workspace`。
9. `npx tsc --noEmit && npm run build`。
10. `git diff --check`，并确认图标与既有 `.hermes/plans/` 文件未被修改。

---

## 第二阶段：在 Pi-compatible layer 之上加入 Pipi 哲学

第一阶段通过后，再按以下顺序设计，不要提前污染 parity 层：

1. `PromptSection` / `InstructionSource`：记录来源、作用域、字节数、截断状态和 hash。
2. trust hierarchy：系统策略 > 用户指令 > 项目指令 > skill 文档 > tool output > model-generated text。
3. untrusted project instruction 包装和 prompt injection 诊断。
4. 可验证执行协议：理解 → 最小修改 → 验证 → 明确报告未验证部分。
5. provider retry/timeout、持久输入队列和可恢复 turn。
6. compaction、session tree/fork、MCP、subagent。

这些功能应作为 `harness` 的新层或 decorator 加入，而不是直接修改 Pi-compatible 的纯 prompt builder；这样可以持续比较“Pi baseline”和“Pipi policy mode”的行为差异。

## 风险与取舍

- **复制 moving main 的风险：** Pi 快速变化会使 snapshot 无法复现；必须固定 SHA。
- **过早引入 Pipi 哲学的风险：** trust、审计和验证协议会改变 prompt 输出，导致无法判断是移植问题还是设计差异；延后到第二阶段。
- **路径安全风险：** Pi 的资源路径不能直接成为 Pipi 的权限模型；所有 read/write/bash 安全检查仍由 Rust 工具层强制。
- **范围膨胀风险：** 不复制 Pi 的 extensions、TUI、themes、package manager；只有在 Pipi 需要该能力时单独立项。
- **第三方依赖取舍：** prompt builder 和 context loader 先用 Rust 标准库及现有依赖实现；只有需要 `.gitignore`/glob 兼容时，再单独评估 `ignore`/`globset` 的许可证、维护活跃度和依赖重量。

## 完成标准

- Pipi 的 harness 输入结构和 system prompt 章节顺序可与固定 SHA 的 Pi 对照。
- context files 和 skills 的 loader 有独立单元测试与边界测试。
- Tauri 实际 provider preamble 使用新 harness 输出。
- 现有 agent loop、abort、generation state、session persistence 无回归。
- 所有验证命令通过；strict Clippy 的既有基线问题单独记录，不伪装成本阶段已解决。
- 未创建未经用户确认的 commit。
