# 结构重构计划：契约层 / L4 实现 / 宿主 / 壳

## 目标布局（15 → 12 个成员）

| crate | 内容 | 来源 |
| --- | --- | --- |
| `pipi-error` | 不变 | — |
| `pipi-protocol` | **契约层**：DTO + 会话条目格式 + 端口（SessionLog/SessionRepo/Provider/AgentTool/ToolContext/ToolRegistry 容器）+ 零依赖助手（new_id/escape_path_for_prompt/fs 守卫/阈值常量） | 原 protocol + 各处端口 |
| `pipi-tools` | 内置工具实现 + 权限策略 + `builtin_registry` + truncate/process/memory | 原 pipi-tools 减端口 |
| `pipi-provider` | rig 适配 + classify + provider_for + **catalog** | 原 provider + catalog |
| `pipi-context` | 预算/裁剪/配对修复/环境上下文 + compaction 流水线 + **retry 退避** | 原 agent 的 context/compaction/retry |
| `pipi-session` | JSONL 载体（SessionWriter/JsonlSessionRepo/load_session）+ 未来 git 载体 | 原 agent 的 session 载体 |
| `pipi-venv` | agents 域（注册表/清单/契约收集/prompt 装配）+ av（含 `av` bin）+ skills + prompt 素材 | 原 core::agents + av + skills + harness |
| `pipi-agent` | agent_loop + stats（引擎，只认端口） | 原 agent 减 context/compaction/retry/session |
| `pipi-core` | 宿主：runtime/events/approval/background/commands + settings + 组合工具 + server(feature) | 原 app + core 剩余 + server |
| `pipi`（`crates/pipi`） | Tauri 薄壳 | 原 src-tauri |
| `pipi-test-support` / `xtask` | 不变 | — |

依赖边（architecture.toml 蓝本）：

```toml
[crates.pipi-error]    may-depend-on = []
[crates.pipi-protocol] may-depend-on = ["pipi-error"]      # forbid: av,pipi-tools,pipi-agent,pipi-core,reqwest,rig
[crates.pipi-tools]    may-depend-on = ["pipi-protocol"]   # forbid: reqwest,rig
[crates.pipi-provider] may-depend-on = ["pipi-error","pipi-protocol"]
[crates.pipi-context]  may-depend-on = ["pipi-error","pipi-protocol"]
[crates.pipi-session]  may-depend-on = ["pipi-protocol"]   # 唯一 spawn git
[crates.pipi-venv]     may-depend-on = ["pipi-protocol"]   # lib + av bin
[crates.pipi-agent]    may-depend-on = ["pipi-error","pipi-protocol","pipi-context"]
[crates.pipi-core]     may-depend-on = ["pipi-agent","pipi-context","pipi-protocol","pipi-provider","pipi-session","pipi-tools","pipi-venv"]
[crates.pipi]          may-depend-on = ["pipi-agent","pipi-core","pipi-protocol","pipi-provider","pipi-venv"]
```

三个关键去环点：**格式与端口在 protocol**（协议层低于所有实现）、**retry 与 compaction 同在 context**（agent_loop 与 LLM 摘要重试共用）、**agents 域与 skills 同在 venv**（skills 的 7 个入口签名一行不改）。

## 阶段（每阶段结束必须全绿；按顺序提交到 main）

**S0 基线与落点**
- 记录基线：`cargo test --workspace`、`cargo check --workspace`、`cargo xtask arch`、`npm run test:frontend`、`npx tsc --noEmit`。
- `git tag archive/pre-restructure`；确认 `git rev-list --count main..HEAD == 24` 且反向为 0，`git checkout main && git merge --ff-only refactor/crate-split`。之后全部在 main 上提交。

**S1 契约层扩容（pipi-protocol，5 个提交）** — 最大的一步
1. 会话格式与端口：搬入 `EntryKind`/`SessionEntry`/`EnvDeclared`/`CompactionRecord`/`SessionSummary`/`SessionLog`/`SessionRepo`/`validate_session_id`/`new_id`/`ARCHIVE_DIR`/`replay`/`active_path`/`rebuild_messages`/`active_model_from_entries`/`summary_message`/`summary_text`/`SUMMARY_*`。新增依赖 `uuid`。消费点改 import：`pipi-app/runtime/{mod,store,slots,execution,compaction}.rs`、`pipi-core/src/tools/agent.rs`（顺手删掉其 530 行处重复的 `validate_session_id`）、`pipi-agent/src/session/*`。
2. 工具端口：搬入 `ToolOutput`/`ToolContext`(+ensure_writable)/`AgentTool`/`resolve_path`/`resolve_read_path`/`validate_args`/`ToolRegistry`(new/push/wire_tools/get/names)/权限数据（SandboxMode、Segment、BashMode、BashPermissions、DEFAULT_TOOLS/AGENT_TOOLS/BACKGROUND_TASK_TOOLS/KNOWN_TOOLS、BashAssessment、CommandApprover、PermissionsConfig+tool_enabled）。新增依赖 `async-trait`、`tempfile`。`assess_bash`/`assess_bash_classified` 从 impl 改成 pipi-tools 自由函数；`fs::{write_atomic,ensure_real_directory,ensure_real_file}` → `protocol::fs`；`escape_path_for_prompt`、`DEFAULT_THRESHOLD_PERCENT`、`normalize_threshold_percent` → protocol。pipi-tools 保留实现 + `builtin_registry(ctx)`（原 for_context）。
3. Provider 端口：`Provider` trait + `EventStream` 搬入 protocol，新增依赖 `tokio`（**features=["sync"]**）；`provider_for`/RigProvider/classify 留在 provider。
4. protocol 目录化：lib.rs 预计涨到约 1100 行，按 500 行规则拆 `src/{lib,dto,session,tools,provider,fs,ids}.rs`，内联测试随符号走。
5. （可选）xtask 增加 feature 子集检查（当前只查依赖名）：protocol 段写死 `tokio` 仅允许 `sync`。
- 验证：workspace 全绿 + `cargo xtask arch`（先更新 protocol 段）。

**S2 拆 pipi-session**
- 新建 crate；搬 `session/{ledger,repo}.rs` 与 `history.rs` 的 `load_session` + 内联测试（ledger 266 行、repo 102 行）。
- 装配点收口：`pipi-app/src/runtime/store.rs` 与 `pipi-core/src/tools/agent.rs:507` 都改 `pipi_session::JsonlSessionRepo`。
- 依赖仅 `pipi-protocol`；`pipi-app/tests/{concurrent_sessions,session_repair,compaction_runtime}.rs` 的 import 改路径。

**S3 拆 pipi-context**
- 新建 crate；搬 `context.rs`、`retry.rs`、`compaction/{mod,llm,tool_output}.rs` + `tests/compaction_corpus.rs`。
- `pipi-agent` 留 `agent_loop.rs`、`stats.rs` + `tests/{retry_runtime,provider_usage,provider_timeout}.rs`。
- import 更新：agent_loop（`crate::context::` → `pipi_context::`）、pipi-app（runtime/compaction.rs、execution.rs 的 Budget/compact/needs_compaction/StrategyEnv/project/estimate_context_tokens）。

**S4 provider + catalog 合并**
- catalog 缓存路径改为注入：`load_catalog(refresh, cache_path)`，调用点（pipi-app/commands.rs:96 与壳）传 `av::paths::catalog_cache_path()`；`write_atomic` 用 protocol。
- 源码进 `crates/pipi-provider/src/catalog/`（现 lib.rs 951 行本就超 500，顺手按职责拆）；删 `crates/pipi-catalog`；`pipi_catalog::ModelCatalog` → `pipi_provider::catalog::ModelCatalog`（app + 壳）。

**S5 组 pipi-venv**
- 新建 crate：`src/av/**`（原 av 全部；`cli` 保持私有，`src/av/mod.rs` 暴露 `pub fn run_cli()`）、`src/bin/av.rs`（bin 名仍 `av`，保留 `search` feature 与 optional ureq）、`src/skills/**`、`src/prompt/**`（原 harness）、`src/agents/**`（原 core::agents 9 个文件）。
- 依赖仅 `pipi-protocol`；删 `crates/{av,pipi-skills,pipi-harness}`；`crates/pipi-skills/tests/skill_lifecycle.rs` → `crates/pipi-venv/tests/`。
- import 大面更新：宿主 `pipi_core::agents::` → `pipi_venv::agents::`、`av::` → `pipi_venv::av::`；venv 内部 skills 调 `av::` → `crate::av::`；壳同步。
- 起点命令变化：`cargo run -p av` → `cargo run -p pipi-venv --bin av`（S8 记文档）。

**S6 合并宿主 pipi-core**
- `crates/pipi-core` = 原 app（runtime/events/approval/background/commands）+ 原 core 的 `settings.rs`/`tools/{agent,background}.rs` + `src/server/**`（原 pipi-server main.rs 拆模块）。
- server 作 feature：`[features] server = ["dep:axum","dep:tower-http","dep:tower"]` + `[[bin]] name="pipi-server" required-features=["server"]`；删除 server 里 5 个未使用依赖（catalog/core/protocol/skills/tools）。
- `crates/pipi-app/tests/*.rs`（7 文件 3070 行）→ `crates/pipi-core/tests/`，`use pipi_app::` → `use pipi_core::`；删 `crates/{pipi-app,pipi-server}`。
- 验证：`cargo test -p pipi-core`（含 ipc_wire）+ `cargo run -p pipi-core --features server --bin pipi-server` 后 `curl /health`。

**S7 壳搬迁**
- `git mv src-tauri crates/pipi`；`tauri.conf.json` 的 `frontendDist` `../dist` → `../../dist`；根 `package.json` 的 `tauri` 脚本加 `--config crates/pipi/tauri.conf.json`（hook 的 cwd 若不对，改用 `npm --prefix ../.. run dev`）。
- `.gitignore`/`.zcodeignore` 的 `src-tauri/gen/schemas` → `crates/pipi/gen/schemas`；`vite.config.ts:23` 的 watch 忽略同步。
- `tests/ipc-contract.test.ts` 四处硬编码：24（→ `crates/pipi-core/src/commands.rs`）、81/97（→ `crates/pipi/src/*.rs`）、84（正则 `pipi_app::commands::` → `pipi_core::commands::`）、102（→ `crates/pipi-core/src/server/…`）。
- 根 Cargo.toml members：`src-tauri` → `crates/pipi`；architecture.toml 的 `[crates.pipi]` 键不变（包名仍是 `pipi`）。

**S8 规则与文档**
- architecture.toml 重写为上表 12 条；`pipi-agent` 保留 forbid av/reqwest/rig 与「不 spawn 进程」约定；`pipi-session` 标注唯一允许 spawn git（可选 xtask 源码扫描）。
- AGENTS.md：结构树、命令表（`cd src-tauri && cargo clippy` → `cargo clippy -p pipi`；`cargo test -p pipi-app` → `-p pipi-core`）、分层与禁止转口 re-export 的举例。
- README.md 架构图（201-220）、与 pi 映射表（360-444）、常用命令（543-574）；`docs/git-session-store.md` §7/§10 的落点改为 pipi-session（trait 在 protocol、装配点 store.rs）。

**S9 全量验证**：`cargo test --workspace`、`cargo check --workspace`、`cargo clippy --workspace --all-targets`、`cargo fmt --all --check`、`cargo xtask arch`、`npm run test:frontend`、`npx tsc --noEmit`、`npm run tauri dev` 冒烟、server 冒烟。

## 风险与对策

| 风险 | 对策 |
| --- | --- |
| protocol 依赖面扩大（tokio/sync、async-trait、tempfile、uuid） | 文档写死「tokio 只允许 sync」；可选 xtask feature 检查；确认仍不依赖 av/tools/agent/core/reqwest/rig |
| 禁止转口 re-export，每步必须一次性改完 import | 每阶段以 `cargo check --workspace` 的编译错误为准绳，编译器给全清单 |
| JSONL/IPC 形状 | serde 属性原样搬；由 `pipi-core/tests/ipc_wire.rs` + `tests/fixtures/ipc-wire.json` + 前端 ipc-contract 三方核对兜底 |
| tauri 移动的路径/CLI 定位 | 先跑 dev 冒烟；备选 `--config` 与 `npm --prefix` 两条路 |
| 大 crate 测试粒度变粗 | 按模块跑（如 `cargo test -p pipi-core runtime::`）；集成测试仍在 tests/ |
| 中途状态不可用 | 每阶段一个可回退提交；S0 的 tag 是总锚点 |

## 不在本计划内
git 载体实现（P2，落点已是 `pipi-session`）、`Result<_, String>` → per-crate error enum、前端功能改动、MCP。