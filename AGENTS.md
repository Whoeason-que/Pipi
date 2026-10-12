# AGENTS.md

给在此仓库工作的 AI 编码代理（以及人类协作者）的说明。

## 项目是什么

Pipi —— 一个以 **Agent（而非会话）** 为中心的本地优先桌面应用，Tauri 2.0 架构：
React + TypeScript 前端负责渲染，Rust 核心负责 Agent 循环、工具执行与文件存储。

**README.md 是设计北极星。** 提案任何新功能前，先对照其中的设计哲学：

1. Agent 是一等公民，会话只是副产品
2. 一切皆文件（不引入数据库、不引入私有格式）
3. 小核心，组合优于配置
4. 本地优先（无账号、无遥测）

## 代码结构

```
crates/pipi-error/    跨层稳定错误语义（错误码、RetryHint）
crates/pipi-protocol/ 契约层：DTO 与 wire 形状、会话条目格式与存储端口
                      （SessionLog / SessionRepo）、工具/权限/provider 端口，
                      以及跨层共用的零依赖助手（fs 守卫、文本转义、id）
crates/pipi-tools/    内置工具实现（bash/read/write/edit/glob/grep/memory）与
                      bash 权限策略（切分 / 白黑名单 / 危险命令 / 沙箱重定向）
crates/pipi-provider/ 模型面：rig 的 HTTP/SSE 适配与错误分类（不做重发）+
                      models.dev 模型目录（拉取 / CURATION / 本地缓存）
crates/pipi-context/  上下文与运行策略：token 预算 / 裁剪与配对修复 / 压缩
                      流水线 / 重试退避（只依赖契约层，不碰载体与网络）
crates/pipi-session/  会话载体：JSONL 文件（未来的 git 分支同属这里）；
                      唯一允许 spawn git 的 crate
crates/pipi-venv/     Agent 环境域：agents（清单与注册表、契约层收集、prompt
                      装配）/ av（agent.toml 契约 + 全局技能 store + `av` CLI）
                      / skills（技能中心）/ prompt（纯提示词层）
crates/pipi-agent/    引擎：主循环（agent_loop）与用量统计（stats）
crates/pipi-core/     宿主层：runtime（会话槽与运行编排）/ events / approval /
                      background / commands（**宿主命令表**）/ settings /
                      tools（Agent 组合工具）；`--features server` 的 Web 宿主
crates/pipi-test-support/ 测试脚手架（HOME/AV_HOME 串行化改写），仅 dev-dependency
crates/xtask/        仓库维护任务：cargo xtask arch 按 architecture.toml 检查
                     分层与依赖方向
crates/pipi/         Tauri 薄壳：commands.rs 只做 IPC 转发，不含业务逻辑
src/                 React + TypeScript 前端
```

前端会话投影由 `src/session-runtime.ts`（全局订阅、会话索引）与
`src/session-controller.ts`（水合、run 身份、停止、重键）拥有；界面组件只订阅
快照，不在卸载时停止运行，也不另注册宿主会话事件。**命令表只有一份**：
`crates/pipi-core/src/commands.rs` 定义每个命令的 `XxxArgs`（serde camelCase）、实现与
`dispatch`；两个壳只做转发（Tauri 用 `InvokeArgs<XxxArgs>` 扁平反序列化，Web
直接调 `dispatch`），`src/ipc.ts` 是前端侧的类型化边界。加命令 = 改
`COMMANDS` + 一个结构体 + 一个函数 + 两处一行转发，再跑 `npm run test:frontend`
（契约测试以 `commands.rs` 为准核对三方）与 `cargo test -p pipi-core`；wire 字段
变动同步 `tests/fixtures/ipc-wire.json`。不要绕回 `invoke<T>(string)`，也不要在
壳层手写参数解析。
`crates/pipi-core/src/runtime/` 的子模块保持私有，公共调用继续经过 `runtime` 入口。
宿主事件载荷**一律**带身份 envelope（`agentName` + `sessionId` + `runId`）：
前端不再容忍「裸事件」形状（2026-10-05 删除），新增事件类型必须带这三个字段。

核心从 [pi](https://github.com/earendil-works/pi) 移植而来，模块映射与「有意
不移植清单」见 README「与 pi 的关系」一节。改核心逻辑前先看上游对应实现。
命令安全（`permissions/safety.rs`）与沙箱模式移植自 openai/codex，
映射见 README「与 codex 的关系」。

## 常用命令

| 命令 | 说明 |
| --- | --- |
| `npm install` | 安装前端依赖 |
| `npm run tauri:dev` | 启动开发模式（前端 + 桌面壳） |
| `npm run tauri:build` | 打包 |
| `cargo test -p pipi-core` | 宿主单元测试 + 集成回归（改宿主必跑） |
| `cargo test -p pipi-core --features server` | Web 宿主（鉴权 / 路由 / 静态回退）回归 |
| `npm run test:frontend` | 前端状态机、会话控制器、IPC 与 DTO 契约 |
| `cargo test -p pipi-venv` | 契约 / 技能 / 提示词测试（改 av 或技能必跑；含本地 git 安装端到端） |
| `cargo run -p pipi-venv --bin av -- skill list` | 调试 av CLI（`check`/`env`/`doctor`/`skill …`；`AV_HOME` 可隔离） |
| `cargo check --workspace` | 全量编译检查 |
| `cargo xtask arch` | 分层与依赖方向检查（改 `Cargo.toml` 依赖或新增 crate 必跑） |
| `cargo clippy --workspace --all-targets` | Rust lint |
| `npx tsc --noEmit` | 前端类型检查 |

## 约定

- Rust 数据模型一律 `derive(Serialize, Deserialize)` 并 `#[serde(rename_all = "camelCase")]`，与前端类型对齐。
- 消息 / 会话的 JSON 字段名与 pi 保持一致（`role`、`toolResult`、`toolCall`、`parentId`……），移植改动不许悄悄改格式。
- **分层与依赖方向以 `architecture.toml` 为准**（`cargo xtask arch` 机检）：
  契约层（`pipi-protocol`）只放 DTO 与端口，不放实现；引擎（`pipi-agent`）不碰
  agent.toml 契约、不直连网络（provider 只以契约层端口出现）；宿主
  （`pipi-core`）持有命令表、会话槽与后台编排；Tauri 壳（`crates/pipi`）与 Web
  宿主（`pipi-core` 的 `server` feature）是薄壳，不写业务逻辑。
  改依赖或加 crate 先改 `architecture.toml` 再跑检查 —— 想放宽边界要在那里
  写明理由，不要在成员 `Cargo.toml` 里偷偷加。
- 不设转口 re-export：类型与逻辑由属主 crate 导出，消费者在自己的
  `Cargo.toml` 里直接依赖（`pipi-protocol` / `pipi-tools` / `pipi-provider` /
  `pipi-context` / `pipi-session` / `pipi-venv` / `pipi-agent` / `pipi-core`）。
  历史上 `pipi_core::{
  types,tools,permissions,provider,harness,skills,truncate,project_doc}`、`pipi-app`
  的整包 glob、`pipi_tools::{types, session}` 与 `resolve_write_path` 转发、
  `pipi_app::runtime::SessionSummary`、`src-tauri` 的 `ChatState` 别名均已删除；
  2026-10-12 的结构重构又把 `pipi-app` / `pipi-server` / `pipi-skills` /
  `pipi-catalog` / `pipi-harness` / `av` 并成 `pipi-core` / `pipi-venv` /
  `pipi-provider`，**旧 crate 名不要再出现在新代码里**。
- 模块与测试布局：单个模块文件超过 ~500 行**生产代码**就拆成目录
  （`foo/mod.rs` 放模块文档 + `mod` 声明 + `pub use` 门面，实现按**改动理由**分文件；
  测试行数不计入这 500 行）。**单元测试内联在被测文件底部**
  （`#[cfg(test)] mod tests { use super::*; … }`，与 quant 各 Rust 仓一致）：
  测哪个文件就放哪个文件，确实横跨多个兄弟文件的（整模块不变量）放该目录模块的
  `mod.rs`；不再建 `src/foo/tests.rs` / `<name>_tests.rs`，也不用 `#[path]`。
  跨文件共用的 fixture 进 crate 根的 `src/test_support.rs`（`mod test_support;`），
  只在模块内共用的夹具放模块自己的 `test_support.rs`（`#[cfg(test)] mod test_support;`），
  测试用 `use crate::test_support::…` 引用。临时目录一律
  `pipi_test_support::temp_dir(prefix)`，不要再手写一份。**集成测试（只用 pub API、
  编译成独立 crate）放 `crates/<crate>/tests/*.rs`，无需任何声明**：整条链路 /
  整个生命周期的黑盒回归都放这里（如 `pipi-tools/tests/permissions_policy.rs`、
  `pipi-venv/tests/skill_lifecycle.rs`、`pipi-context/tests/compaction_corpus.rs`），只走公开入口的用例不要塞回单元测试。
  `mod.rs` 里的 `pub use` 是 Rust 模块系统要求的门面，**不是**被禁止的转口
  re-export —— 后者指的是跨 crate 的整包转发。
- Cargo 是 workspace 结构（edition 2024，rust-version 1.95）：版本 / edition 在根
  `Cargo.toml` 的 `[workspace.package]`，依赖版本与内部路径依赖都在
  `[workspace.dependencies]`，成员一律用 `version.workspace = true` /
  `{ workspace = true }` 继承，不要在成员里硬编码版本。一个坑：
  `default-features` 只有在工作区条目里也显式写了才会被成员覆盖（不写就静默
  忽略、默认特性照开）——`av` 的 `search` 与 `reqwest` 靠这条维持「不同成员用
  不同默认特性」，别删工作区条目里的 `default-features`。
- 测试里改写进程环境（`HOME` / `AV_HOME` / `PIPI_HOME`）走
  `pipi_test_support::{set_env_var, remove_env_var}`：edition 2024 起
  `std::env::set_var` 是 unsafe，unsafe 块与 `SAFETY` 说明收敛在那里；调用方
  必须先持有 `pipi_test_support::HOME_LOCK`（每个测试进程一把，**全仓库只有这
  一把锁** —— 曾出现 av 自建 `AV_HOME_LOCK` 与它并行、合并成 `pipi-venv` 后
  偶发失败）。**读取方同样要持锁**：断言 `~/.pipi/agents/...` 这类派生路径的
  用例若在别人改 HOME 的中途读取，会看到跨状态的结果。
- Agent 数据根目录是 `~/.pipi/agents/<name>/`，结构见 README；不要把 Agent 状态存到别处。
- 错误处理：Tauri command 返回 `Result<T, String>`，消息用用户可读的中文。
- 前端依赖走白名单，样式仍然手写；新增依赖需要充分理由。已批准的依赖：
  - react-markdown + remark-gfm + rehype-highlight（agent 输出的 Markdown 渲染，
    见 src/Markdown.tsx；不启用 rehype-raw，模型输出不可信）；
  - react-select（模型/供应商选择器的搜索与键盘导航，见 src/Select.tsx）——统一以
    `unstyled` + 自有 `.pipi-select__*` 样式使用，不让第三方样式体系渗进来。
- 模型目录（提供商与模型清单）**不再手写或生成**：运行时从 models.dev 拉取并缓存到
  `~/.pipi/cache/models.json`，实现见 `crates/pipi-provider/src/catalog/mod.rs`。上游给的是
  AI SDK 语义的 baseUrl，未经 `CURATION` 表核实不得直接当 Pipi 的 baseUrl 使用。
- 目录骨架必须与 README「Agent 的组成」表格一致；改动时两边同步更新。
- provider 协议层用 rig（`rig` crate，依赖重命名自 rig-core）：新增 provider
  能力优先看 rig 是否已支持，不要回退到手写 SSE；映射偏差记录在 provider.rs。
  rig 破坏性变更很频繁：0.43 是 wire + transport + fold 重写（client 由配置
  `.connect(transport)` 组装、流事件是 part 生命周期、`Usage` 计数全是
  `Option<u64>`），升级大版本前先读它的 CHANGELOG，别按旧 API 猜。
- 统计口径（`pipi-agent/src/stats.rs`，移植自 hermes）：滚动窗口 N=10、命中率 = cache_read/prompt、
  数据不足时省略而非显示 0 —— 改统计先对齐这三个语义。用量口径同 pi：`Usage.input`
  只计未命中缓存的提示词 token（rig 0.43 起各供应商上报的 `input_tokens` 都含缓存
  读写，拆分统一在 `provider::from_rig_usage` 完成）；读用量一律用 `Usage::prompt_tokens()`，不要再自行
  相加 —— 相加会把命中量算两遍，命中率恒为 50%。摘要压缩这类一次性调用的用量走
  `SessionStatsTracker::record_ledger`（只进累计值，不改写「当前上下文占用」）。
- 上下文压缩（`pipi-context/src/compaction/`）分两类，加档位前先判断属于哪类：**投影式**
  （`Projection`：纯函数、确定性、可从原始历史重算 → **不落盘**，只在
  `transform_context` 里应用；写进历史会造成 live 与重开不一致）与**替换式**
  （`Replacement`：信息已丢失 → 必须落盘成 compaction 条目并在回放时重建）。
  策略只能返回 `Plan`，不得直接改 `messages`；切点不得孤立 toolResult、就地改写
  不得改角色、压缩后破损度不得增加 —— 这些不变量统一由 `validate` /
  `validate_result` 把关（fail-closed），预算参数集中在 `Budget::from_window`。
  改压缩必须跑 `pipi-context/tests/compaction_corpus.rs`（真实会话 + 合成语料的不变量回归）。
  触发口径只有一处：`Budget::trigger_tokens`（窗口 × 百分比，Agent 级
  `agent.json.compactThresholdPercent`，默认 75）；可选的
  `agent.json.compactTargetPercent` 只决定摘要后保留多少近期原文，以压缩前估算
  token 为基数，缺失时继续使用固定 2 万 token 预算，不改变触发线。手动压缩与自动压缩共用
  `pipi-core::runtime::run_compaction`，差别只在 `CompactionTrigger`（手动跳过阈值预检，
  并且要在首尾补一对 AgentStart/AgentEnd —— 前端靠 agent_end 落 running）。
  压缩默认**分叉 + 归档**（settings 的 `compaction.forkBeforeCompact` /
  `archiveOriginal`）：压缩写进新会话、原会话留作完整记录。换会话是**原地替换
  writer 与 messages**（`running`/`abort`/`steering` 必须共享同一批对象，重建
  Session 会让停止按钮与 steering 断链），并且**事件身份必须跟着切换**：
  `session-switched` 用旧 id（前端此刻身份还是旧的）、后续事件用新 id；
  归档只在旧 writer 关闭之后做 —— 否则 Linux 上打开的 fd 会继续往被移动的文件追加。
- 并发模型是**按会话**的：`RuntimeState` 的会话槽是
  `Mutex<HashMap<SessionKey, Session>>`，`SessionKey = (Agent 名, 会话 id)` ——
  同一个 Agent 可以同时开多条会话、每条各跑一轮，数量不限（**没有并发上限，别引入
  全局信号量**）。守卫也按会话：**同一条会话同时只能跑一轮是硬不变量**（第二轮必须
  拒绝并提示先停止或走 steering），别的会话照跑。压缩分叉换 id 时要把条目从旧键
  搬到新键（`SessionRekey`，同一条会话、新 id），并保持 `session-switched` 的语义。
  所有会话类命令都带 `agentName` + `sessionId`；`session_infos` 是「哪些会话在跑」
  的唯一来源。改这里必须跑 `pipi-core/tests/concurrent_sessions.rs`（多 Agent 同时跑、
  **同一个 Agent 两条会话同时跑**、停一条不影响其他、只释放空闲会话、事件与落盘
  互不串），并且别把前端拖回去：ChatView 卸载**不得**中止运行、切换 Agent 或切换
  会话**不得**被拦（后台运行是产品行为，不是 bug）。
- 权限是安全边界：bash 命令检查在 `pipi-tools/src/bash.rs` 执行前发生，
  改权限逻辑（`permissions/`）必须带测试，且宁可拒绝不可放行 —— 无法静态
  分析的命令一律视为危险。两条已定的判据边界，改动前先想清楚：
  **重定向**只拦「写入文件系统」（`>` `>>` `2>文件` `&>` `>|` `<>` `>&词`）
  与「从文件读入」（`<文件` `<(cmd)`），`2>/dev/null`、`2>&1`、`>&-`、
  heredoc / herestring 必须放行（它们既不落盘也不读文件，且实测占拒绝量的
  绝大多数，拦下来只有摩擦）—— 见 `permissions/redirect.rs` 的 `check_write_redirect`；
  **`rm -f` 家族**只放行工作区内、非仓库根 / 工作区根的字面量绝对路径，
  包装调用（sudo/env/bash -c）、变量与通配符目标必须拒绝 —— 见
  `permissions/rm.rs` 的 `forced_rm_inside_workspace`。这两条改判据都要更新
  `crates/pipi-tools/tests/permissions_policy.rs` 的表驱动测试（白盒解析回归
  内联在 `permissions/{parse,redirect,paths}.rs` 各自文件底部）。
- 请求重试分两层，别把重试加到 provider 层：**provider 只分类**（rig 的结构化错误
  → 可重试 / 终态 + `Retry-After`，见 `retry::classify_rig_error`），**重发只在
  agent_loop 一处做**（`stream_assistant_response` 的尝试循环；摘要调用在
  `compaction::llm` 自己那层共用同一策略）—— 双层重试会放大成 N×M 次请求。
  三条不变量：**用户中止永远不可重试**；**已经输出过正文的那一轮不重放**（例外是
  只有工具调用块，半截参数绝不交给工具执行）；**无进展超时最多额外重试 1 次**。
  改分类表或判定规则必须更新 `retry.rs` 的表驱动测试与 `tests/retry_runtime.rs`。
- agent.toml 契约（`crates/pipi-venv/src/av`，README「agent.toml 契约（av 标准）」一节）：
  未知键/未知段一律拒绝、秘密值只许引用式（永不内联）、`AV_*` 是运行时
  保留命名空间、项目层文件不得声明身份与权限段 —— 改 schema/合并/解析
  逻辑必须带测试，宁可拒绝不可放行；工具子进程环境一律取
  `ToolContext.resolved_env`（会话启动解析一次），不许在 spawn 点读进程环境。
- 技能包管理（`pipi-venv` 的 `av::{store,sources,install}`）：全局 store 在
  `~/.av/skills/<name>/<hash16>/`（内容寻址、发布后不可变，同名多版本并存）。
  **安装 ≠ 启用** —— `[resources.skills].use` 声明才生效，同层 `agent.lock`
  是解析权威；**会话启动只读、不联网**，缺锁/缺内容即 fail-closed 并提示
  `av skill sync`；写入只发生在用户显式触发时 —— `av skill install/add/sync/
  update/remove`，或应用内技能面板（`pipi-venv::skills`，走同一份 av
  实现：`av::contract_edit` 编辑 `use`、`av::store::write_text_atomic` 落盘、
  `av::install` 物化），任何隐式写入都不允许。git 一律 spawn 系统 `git`
  （不引 git2），来源不接受符号链接，单技能限额 500 文件 / 5 MiB。改
  store/lock/sources/install/contract_edit 必须带测试（篡改、逃逸、限额、
  修订钉扎、锁不一致拒绝、编辑后 schema 校验），并保持 `av` CLI 的 `--json`
  与退出码 0/1 约定。`~/.pipi` 与 `~/.av` 的路径拼装一律走 `pipi_venv::av::paths`
  （`PIPI_HOME`/`AV_HOME` 可覆盖），不许在别处手写。`search` feature 只属于
  av CLI（`pipi-core` 以 `default-features = false` 依赖 `pipi-venv`），不要把
  HTTP 客户端链进桌面应用；`pipi-venv::skills` 的 `install_skill` /
  `sync_agent_skills` / `verify_store_skills` 会阻塞甚至联网，宿主必须放到
  阻塞线程池（Tauri/Web 都不许在事件循环上直接调）。
