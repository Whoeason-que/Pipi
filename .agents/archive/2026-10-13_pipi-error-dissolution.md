# 三项消解实施计划：pipi-error 合流 / send_prompt 返回身份 / 压缩管线静态化

三项互相独立，推荐顺序 **① → ③ → ②**（风险递增；①③ 不碰 IPC 契约，跑 workspace 测试就能收；② 契约三处同步且需要你的 GUI 冒烟）。每项独立 commit，分支 `feat/harness-prompt-scopes`，不推远端。

## 前置
确认工作树干净（现有 14 个提交之后；若有未提交改动先按主题安顿）；跑一次基线 `cargo test -p pipi-protocol -p pipi-context -p pipi-provider -p pipi-agent -p pipi-core` + `npm run test:frontend` 全绿再动手。

## ① pipi-error 并入 pipi-protocol，删除 crate

核实结论：`RetryHint` 是唯一真正跨层的类型（还嵌在 `StreamEvent::Error` 的 wire 上，40 处引用）；`ErrorCode`/`PipiError` 的唯一引用方是 dto.rs 的 `ErrorEnvelope`，而 `ErrorEnvelope` 全仓零构造、零读、前端零引用——整条死链删除（与批次 0 同一标准）。

1. 新建 `crates/pipi-protocol/src/error.rs`：`RetryHint`（结构体 + `plain/after/timeout` 三个 const fn + doc）；移植其测试时去掉 `PipiError` 部分，直接断言 `after_ms`/`idle_timeout`。
2. `pipi-protocol/src/lib.rs`：声明 `pub mod error;` + `pub use error::RetryHint;`（属主 crate 的门面，允许）。
3. `pipi-protocol/src/dto.rs`：删 `ErrorEnvelope`（400-408）、`impl From<&PipiError>`（410-418）、测试 `error_envelope_exposes_only_safe_error_fields`（440-450）；:6 的 import 改为 `use crate::RetryHint;`。
4. 引用方改路径（11 处 / 7 文件）→ `pipi_protocol::RetryHint`：pipi-context 的 `retry.rs:8`、`compaction/llm.rs:242,324`；pipi-provider 的 `classify.rs:6`、`lib.rs:54`；pipi-agent 的 `agent_loop/mod.rs:167,190,225`（测试）、`agent_loop/stream.rs:91,101`（全限定路径）。
5. Cargo：四个成员 `Cargo.toml` 删 pipi-error 依赖；根 `Cargo.toml` 删 members(:4) 与 `[workspace.dependencies]`(:37) 各一行；Cargo.lock 随构建更新一并提交。
6. 删除目录 `crates/pipi-error/`。
7. `architecture.toml`：删 `[crates.pipi-error]` 段（16-18），从 protocol/context/provider/agent 的 `may-depend-on` 去掉 `"pipi-error"`。
8. 文档同步：AGENTS.md:20 代码树行、README.md:225 架构图行。

验证：`cargo xtask arch`（本项机器门禁）、`cargo check --workspace`、上述 5 个 crate 测试、`cargo clippy --workspace --all-targets`。风险：只剩漏改引用，全限定路径也会被编译器兜住。

## ③ pipi-context 压缩管线：删 dyn 流水线，改具象顺序调用

核实结论：三个策略全是无状态 unit struct（`BoundaryPrune`/`ToolOutputPrune`/`LlmSummarize`）；全仓没有第二个 trait 实现方、没有测试假策略、没有测试调工厂。`Plan`/`validate`/`validate_result` 与「投影式不落盘、替换式落盘」的语义分界全部保留。

1. `compaction/tool_output.rs`：`impl Projection for ToolOutputPrune` → `impl ToolOutputPrune`，方法收成 `pub(super)`（`NAME`、`applies`、`plan`）；文件内 4 个测试不变。
2. `compaction/llm.rs`：`impl Replacement for LlmSummarize` → inherent；`run` 由 `BoxFuture` 改 `pub(super) async fn run(&self, prep: &Preparation<'_>, env: &StrategyEnv<'_>) -> Result<Plan, String>`；测试 llm.rs:482-529 直调 run，签名兼容。
3. `compaction/mod.rs`：删 `trait Projection`(280-286)、`trait Replacement`(293-303)、`type BoxFuture`(289)、`projections()`(326-329)、`replacements()`(331-334)。`project()` 循环体提为私有 helper（validate → apply → validate_result → 失败 `continue`+eprintln 的语义与文本 1:1 搬移），主函数改为显式顺序调用：
   `let current = apply_projection(current, budget, ToolOutputPrune::NAME, |prep| ToolOutputPrune.plan_if_applicable(prep));`（先清旧工具输出）→ 同法跑 `BoundaryPrune`（硬裁）。`BoundaryPrune` 定义处（mod.rs:306）改具象带 `NAME`。
4. `compact()`：删替换式循环，单策略直调 `LlmSummarize`；逐条对照保留旧行为：空历史早退「空上下文无需压缩」、`applies` 不满足与 `plan.is_noop()` 都走「没有可用的压缩策略」、`run` 错误原样上抛、validate 失败 `?` 整体失败。
5. 文档同步：mod.rs 模块 doc（1-16，「两类刻意用两个 trait」改为「两条路径（project/compact）就是语义分界」）、README.md:253-285（表格两行措辞、284 行「加一档 = 一个具象函数 + 一行调用」）、AGENTS.md:182-189 去 trait 名保留两类语义描述。
6. 零改动：`Plan`/`validate`/`validate_result`/`Preparation`/`StrategyEnv`/`Budget`/`needs_compaction`、调用方（pipi-agent `session/run.rs:191-194`、`session/compaction.rs:57`）、pipi-agent/pipi-core 的全部接口。

验证：`cargo test -p pipi-context`（**必含 `tests/compaction_corpus.rs`**——合成语料 + 真实会话的不变量回归，是行为等价的证据）、`cargo test -p pipi-agent -p pipi-core`、clippy、`cargo xtask arch`。

## ② send_prompt 返回 `SessionInfo`，前端主路径确定化

前置事实（已核实）：会话 id 由核心生成（`{now_millis}-{uuid}`），事件与 IPC 响应走两条独立通道、先后无保证，因此**兜底不能为零**；本项把主路径确定化并让身份/模型信息一次到位，事件先到时保留现有 draft 认领为兜底。

宿主侧：
1. `runtime/slots.rs`：`session_info_of` 加 `pub(super)`（仅可见性；纯内存快照，不读盘）。
2. `runtime/execution.rs`：`send_prompt` 签名改 `Result<SessionInfo, String>`；在 `let run_id = reservation.run_id();`（:105）之后插 `let info = self.session_info_of(session)?;`（此时 sessionId/run_id/running/model/temporary 已全部确定；放在 `prepare` 之前，锁中毒失败不会留下半写会话），末尾 `Ok(info)`。新会话入 map 的时机（:219）不动。
3. `commands.rs`：`send_prompt` 签名同步（:383-395）；dispatch :770 的 `to_value` 与已有 `SessionInfo` 导入不用动。
4. `crates/pipi/src/chat.rs:120-127`（Tauri 转发）返回类型同步；Web 壳是通用转发，零改动。

契约与前端：
5. `src/ipc.ts:41`：`send_prompt` 的 `result` 改 `SessionInfoView`（`tests/ipc-contract.test.ts` 从 commands.rs ↔ ipc.ts 自动推导，自动变绿）。
6. `src/session-controller.ts send()`（:247）：消费返回值——`this.identity === null` 时 `adopt(info, true)`（主路径一次到位）；identity 已由兜底认领时只补 `model`/`isCustomModel`（不 announce，避免二次 revision 与刷新）。`session-runtime.ts` 的 drafts/awaitingIdentity 与 `agent_start` 认领保留，注释写明是「事件先于响应的兜底」。
7. `src/dev-mock.ts`（:801-817）：send_prompt 桩返回 `SessionInfoView` 形状（新会话给 mock id）。
8. 测试：`tests/session-controller.test.ts` 的 Host.invoke 加 send_prompt 分支返回 info；改写 :118-127（断言改为响应确立身份）；**新增两例**——①响应先到：无事件即确立身份且 model 正确；②事件先到（deferred promise 控制顺序）：兜底认领后返回值只补 model。Rust 侧约 20 处调用方丢弃返回值即兼容；在 `send_prompt_runtime.rs` 补一条断言：返回的 `sessionInfo.sessionId` 与随后第一个事件的 sessionId 一致（证明响应是权威身份）。

验证：`cargo test -p pipi-core`（含 `--features server`）、`npm run test:frontend`、`npx tsc --noEmit`、`cargo xtask arch`。GUI 冒烟留给你：新建会话发首条消息（侧栏/标题即时出现、模型显示正确）、后台运行与停止、压缩分叉换会话仍正常。

## 明确不做（防重复讨论）
- ② 不删 drafts 兜底、不改水合 4-IPC 聚合、stop 轮询与 chatKey（各自独立，属另一批）。
- ③ 不动 Budget/Plan/validate 与压缩触发口径；不加新策略、不改顺序。
- ① 不保留死类型（要错误码面将来从 git 历史取只要几行）；不引入 protocol 之外的新依赖边。
- 各项完成后给出净行数账单与本轮决策记录；不推远端，GUI 冒烟清单在总结里留给你。