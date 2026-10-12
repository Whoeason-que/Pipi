## 修复计划：重构审查的 P1 + 机械项（8 个 commit）

判定口径已复核：每个改动都定点到 `file:line`，测试与验证命令随项给出。范围不含协议层纯度整改（write_atomic/ToolRegistry 迁出等），也不动 `commit_point`（它是 `docs/git-session-store.md` 里 git 载体的既定预留钩子）。

### commit 1 — `fix(session): 会话文件对符号链接 fail-closed`
- `crates/pipi-session/src/repo.rs`：`session_path()`（:27）在 `join("{id}.jsonl")` 后补 `pipi_protocol::fs::ensure_real_file(&path, "会话文件")?`。单点守卫同时覆盖 `open`/`exists`/`entries`/`fork`/`restore` 目标/`delete` 预检；语义：符号链接 → `Err`，NotFound → `Ok(false)` 继续返回路径（`exists` 语义与今天 `is_file()==false` 一致）。`create()` 不受影响。
- `crates/pipi-protocol/src/session.rs:172`：`exists` 的端口文档写明「实现必须对符号链接 fail-closed」。
- 测试（`repo.rs` 底部内联 `#[cfg(unix)]`，抄 `pipi-protocol/src/fs.rs:132` 的 `std::os::unix::fs::symlink` 模式）：新增一条用例——真实会话 + 一条指向它的 `link.jsonl`，断言 `exists`/`entries`/`open`/`fork` 全部 `Err(不能是符号链接)`，而缺失 id 的 `exists` 仍 `Ok(false)`。
- 已知行为变化（预期）：`open_session`/`fork_session`/`read_agent_output`/`send_prompt` 对符号链接会话从「静默跟随」变报错。

### commit 2 — `fix(ipc): 前端兜底表补全并加双向契约断言`
- `src/platform.ts:326-343`：表值类型放宽为 `{ sessionId?: "required" | "optional" }`；补 12 条——`archive_session`/`restore_session`/`delete_session`/`delete_archived_session`（sessionId required）与 `list_sessions`/`list_archived_sessions`/`list_agent_files`/`read_agent_file`/`write_agent_file`/`agent_skills`/`set_skill_enabled`/`sync_agent_skills`（仅 agentName）；更新上方注释（表已从「会话级」扩到「一切带 agentName 的命令」）。
- `tests/ipc-contract.test.ts` 加两条断言：① 由已解析的 commands.rs 字段推出「含 `agentName` 的命令」集合，与 platform.ts 表键**集合相等**，且 sessionId 的 required/optional 与 `String`/`Option<String>` 一一对应（以后漏补会直接红）；② 两个壳文件里凡形参含 `args:` 的 `#[tauri::command]` 必须是 `InvokeArgs<XxxArgs>`（现 42 条带参命令全符合）。
- 验证：`npm run test:frontend`、`npx tsc --noEmit`。

### commit 3 — `chore(deps): 删 4 个零引用 dev-dep 与过期元数据`
- 删：`crates/pipi-agent/Cargo.toml` 的 `pipi-session`、`pipi-test-support`；`crates/pipi-context/Cargo.toml` 的 `pipi-test-support`；`crates/pipi-core/Cargo.toml:38-39` 的 `dirs`（连同引用已删 `agents.rs` 的注释）。保留 `pipi-context` 的 `pipi-session`（`tests/compaction_corpus.rs:15` 在用）与 `pipi-agent` 的 `pipi-provider`。
- `crates/pipi-agent/Cargo.toml:6` description → 「Pipi Agent 引擎：主循环与用量统计」。
- architecture.toml 不用动（arch 检查是白名单制，`:140-148` 不要求“声明即使用”）。
- 验证：`cargo check --workspace --all-targets`、`cargo test --workspace --exclude pipi`。

### commit 4 — `fix(core): server 构建显式声明 tokio 特性`
- `crates/pipi-core/Cargo.toml:11`：`server = ["dep:axum", "dep:tower-http", "tokio/net"]`（`tokio::net` 现在靠 axum 默认特性间接提供）。
- `:31` 的 tokio features 补 `"fs"` 并加一行注释——`background/{mod,persist}.rs` 无门控地用 `tokio::fs`，今天白嫖 `pipi-tools` 的 `fs`；桌面构建的分辨率不变（特性是并集）。
- 不加 `net` 到基础列表（避免桌面白拿），不加 `signal`（全仓无使用点）。
- 验证：`cargo tree -p pipi-core --no-default-features`（无 axum）与 `--features server`（有 axum）、`cargo test -p pipi-core --features server`、`cargo check -p pipi`。

### commit 5 — `chore(tests): HOME_LOCK 统一到 pipi_test_support`
- 6 个文件（`crates/pipi-core/tests/` 下）：4 个自建锁的（`agent_composition.rs:23`、`compaction_runtime.rs:25`、`concurrent_sessions.rs:26`、`session_repair.rs:10`）删本地 `static`、改用 `pipi_test_support::HOME_LOCK`，`unsafe set_var` 换成 `pipi_test_support::set_env_var`（SAFETY 注释一并删）；2 个无锁直写的（`request_paths.rs:28`、`send_prompt_runtime.rs:27`）补持锁 + 换 helper。
- 毒化策略统一为 `.unwrap_or_else(|error| error.into_inner())`。dev-dep 已具备，清单不动。必须整文件替换（不能两把锁并存——AGENTS.md 记的双锁事故）。
- 明确不做：6 处 `await_holding_lock` 告警保持原样（这些用例**故意**跨 await 持锁，保证整轮运行期间 HOME 稳定；收窄作用域会让它们失去保护）。
- 改完 AGENTS.md:125-131 的「全仓库只有这一把锁」由描述变事实，无需改文档。
- 验证：`cargo test -p pipi-core`（覆盖 6 个集成文件）。

### commit 6 — `refactor: 会话 id 规则去重（validate_session_id / kept_start）`
- `crates/pipi-core/src/tools/agent.rs`：删私有副本（:531-540），在 :18 的 `pipi_protocol::session::{...}` 导入里加 `validate_session_id`。
- `crates/pipi-protocol/src/session.rs:294`：`kept_start` 由私有改 `pub fn`（doc 注明「载体侧映射维护复用，规则只有一份」）。
- `crates/pipi-session/src/lib.rs`：删本地副本（:13-22），从未导入处引 `pipi_protocol::session::kept_start`；`ledger.rs:29` 的 intra-doc 链接改为 `pipi_protocol::session::kept_start` 全路径。
- 保留 `crates/pipi-core/src/runtime/mod.rs:182` 的转发壳（有文档说明，是宿主侧唯一入口）。
- 验证：`cargo test -p pipi-protocol -p pipi-session -p pipi-core`。

### commit 7 — `docs: README 映射表与陈旧路径/元数据`
- `README.md`「与 pi 的关系」表 4 行（按内容定位，不按行号）：`pipi-agent::session` → `pipi-protocol::session`（条目/回放）+ `pipi-session`（JSONL 载体）；`pipi-agent::context` → `pipi-context::context`；`pipi-agent::compaction` → `pipi-context::compaction`；`av::skills` → `pipi-venv::skills`。顺带 `git grep` 全 README 确认无其它 `pipi-agent::` / `av::` 陈旧项（`av skill …` 指 CLI 名，保持）。
- `docs/git-session-store.md:193-196`：`slots/mod.rs`→`crates/pipi-core/src/runtime/slots.rs`、`execution/mod.rs`→`runtime/execution.rs`、`queries.rs::set_session_model`→`runtime/slots.rs:552`。
- `AGENTS.md:208`：`retry::classify_rig_error` → `pipi_provider::classify::classify_rig_error`。
- `crates/pipi-agent/src/lib.rs:12` 修 `//! //! ` 破行；`agent_loop.rs:146,281` 的 `[crate::retry]` → `[pipi_context::retry]`。
- `crates/pipi-context/src/context.rs:8-11` 删 environment_context 段（改用一行指引到 `pipi-venv/src/prompt/environment.rs`）；`src/lib.rs:5` 表格去掉「环境上下文」。
- `crates/pipi-venv/tests/skill_lifecycle.rs:2` `pipi_skills`→`pipi_venv::skills`；`agents/prompt.rs:300,459` 测试字面量 `__pipi_harness_wire_test__`/`__pipi_skills_sources__`→`__pipi_venv_*`（含断言内同名引用）。
- 验证：`cargo test -p pipi-agent -p pipi-context -p pipi-venv`。

### commit 8 — `chore(arch): architecture.toml 措辞与死名；忽略 .zcode/plans`
- `architecture.toml:38-39`：「唯一允许 spawn git 的 crate」改为事实口径——**会话存储**的 git 调用只在这里，技能包的 git 由 `pipi-venv` 的 av 负责，并注明 xtask 不检查进程 spawn（靠约定）。
- 删 `forbid` 里已不是 crate 名的 `"av"`（protocol 段 :30、agent 段）——`arch.rs:133` 只在真实依赖上匹配，留着是假防线。
- `.gitignore` 与 `.zcodeignore` 加 `.zcode/plans/`；`git rm --cached .zcode/plans/*.md`（3 份文件保留在本地，不删工作区文件）。注意 `.zcodeignore` 顶部块会随 `.gitignore` 同步，规则写在各自的自定义区域。
- 验证：`cargo xtask arch`。

### 收尾批次验证（全绿后再提交最后一个 commit）
`cargo xtask arch` → `cargo check --workspace --all-targets` → `cargo test --workspace --exclude pipi` → `cargo test -p pipi-core --features server` → `cargo clippy --workspace --all-targets`（期望仍是 11 处历史告警、零新增）→ `npm run test:frontend` → `npx tsc --noEmit` → `cargo check -p pipi`。另外人工抽查一次符号链接端到端（临时 HOME 下 `ln -s` 一条会话文件，确认 `read_agent_output` 报「不能是符号链接」）。

### 提交方式
按仓库惯例每个 commit 一个逻辑边界、直接提交到 `main`（8 个 commit 的顺序如上，commit 1–6 是代码，7–8 是文档/配置）。如果你只想先看 diff 不提交，说一声我改成只改不提交。

### 不做的部分（明确排除）
协议层纯度整改（把 `write_atomic`/`ToolRegistry` 等实现性助手迁出 `pipi-protocol`）、`commit_point` 去留、6 处 `await_holding_lock` 告警、以及 10-05 那轮提交的复查。