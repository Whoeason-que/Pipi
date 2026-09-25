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
crates/av/          agent.toml 环境契约（独立工具）：schema / 发现 / 合并 /
                    env 解析 / requires；lib 供 pipi-core 复用，bin 为调试 CLI
crates/pipi-error/  跨层稳定错误语义（错误码、RetryHint）
crates/pipi-protocol/ 持久化与 transport DTO（JSONL/IPC 消息、流事件、AbortSignal）
crates/pipi-tools/  内置 bash/read/write/edit/glob/grep/memory、命令权限与截断
crates/pipi-harness/ 纯项目上下文发现与 system prompt 渲染（不依赖运行时）
crates/pipi-provider/ rig 的 provider HTTP/SSE 适配与错误分类（不做重发）
crates/pipi-core/   Rust 领域核心（不依赖 Tauri）：agent_loop / session / context /
                    skills / stats / project_doc / settings / agents / catalog（models.dev）/
                    以兼容 re-export 维持旧有 tools/provider/harness 路径
crates/pipi-app/    应用服务层：会话槽、后台调度、审批交互和宿主事件协议；Tauri /
                    Web server 共同依赖它，不在壳层复制运行时逻辑
src-tauri/          Tauri 薄壳：commands.rs 只做 IPC 转发，不含业务逻辑
src/                React + TypeScript 前端
```

核心从 [pi](https://github.com/earendil-works/pi) 移植而来，模块映射与「有意
不移植清单」见 README「与 pi 的关系」一节。改核心逻辑前先看上游对应实现。
命令安全（`permissions/safety.rs`）与沙箱模式移植自 openai/codex，
映射见 README「与 codex 的关系」。

## 常用命令

| 命令 | 说明 |
| --- | --- |
| `npm install` | 安装前端依赖 |
| `npm run tauri dev` | 启动开发模式（前端 + 桌面壳） |
| `npm run tauri build` | 打包 |
| `cargo test -p pipi-core` | 核心单元测试（改核心必跑） |
| `cargo test -p av` | agent.toml 契约测试（改 av 必跑） |
| `cargo check --workspace` | 全量编译检查 |
| `cd src-tauri && cargo clippy` | Rust lint |
| `npx tsc --noEmit` | 前端类型检查 |

## 约定

- Rust 数据模型一律 `derive(Serialize, Deserialize)` 并 `#[serde(rename_all = "camelCase")]`，与前端类型对齐。
- 消息 / 会话的 JSON 字段名与 pi 保持一致（`role`、`toolResult`、`toolCall`、`parentId`……），移植改动不许悄悄改格式。
- 可复用领域逻辑进 `pipi-core`；会话槽、后台编排与宿主事件只进 `pipi-app`；
  `src-tauri` 是薄壳，不写业务逻辑。
- Agent 数据根目录是 `~/.pipi/agents/<name>/`，结构见 README；不要把 Agent 状态存到别处。
- 错误处理：Tauri command 返回 `Result<T, String>`，消息用用户可读的中文。
- 前端依赖走白名单，样式仍然手写；新增依赖需要充分理由。已批准的依赖：
  - react-markdown + remark-gfm + rehype-highlight（agent 输出的 Markdown 渲染，
    见 src/Markdown.tsx；不启用 rehype-raw，模型输出不可信）；
  - react-select（模型/供应商选择器的搜索与键盘导航，见 src/Select.tsx）——统一以
    `unstyled` + 自有 `.pipi-select__*` 样式使用，不让第三方样式体系渗进来。
- 模型目录（提供商与模型清单）**不再手写或生成**：运行时从 models.dev 拉取并缓存到
  `~/.pipi/cache/models.json`，实现见 `crates/pipi-core/src/catalog.rs`。上游给的是
  AI SDK 语义的 baseUrl，未经 `CURATION` 表核实不得直接当 Pipi 的 baseUrl 使用。
- 目录骨架必须与 README「Agent 的组成」表格一致；改动时两边同步更新。
- provider 协议层用 rig（`rig` crate，依赖重命名自 rig-core）：新增 provider
  能力优先看 rig 是否已支持，不要回退到手写 SSE；映射偏差记录在 provider.rs。
- 统计口径（stats.rs，移植自 hermes）：滚动窗口 N=10、命中率 = cache_read/prompt、
  数据不足时省略而非显示 0 —— 改统计先对齐这三个语义。用量口径同 pi：`Usage.input`
  只计未命中缓存的提示词 token（OpenAI 兼容端点的 `prompt_tokens` 已含缓存，拆分在
  `provider::from_rig_usage` 完成）；读用量一律用 `Usage::prompt_tokens()`，不要再自行
  相加 —— 相加会把命中量算两遍，命中率恒为 50%。摘要压缩这类一次性调用的用量走
  `SessionStatsTracker::record_ledger`（只进累计值，不改写「当前上下文占用」）。
- 上下文压缩（`compaction/`）分两类，加档位前先判断属于哪类：**投影式**
  （`Projection`：纯函数、确定性、可从原始历史重算 → **不落盘**，只在
  `transform_context` 里应用；写进历史会造成 live 与重开不一致）与**替换式**
  （`Replacement`：信息已丢失 → 必须落盘成 compaction 条目并在回放时重建）。
  策略只能返回 `Plan`，不得直接改 `messages`；切点不得孤立 toolResult、就地改写
  不得改角色、压缩后破损度不得增加 —— 这些不变量统一由 `validate` /
  `validate_result` 把关（fail-closed），预算参数集中在 `Budget::from_window`。
  改压缩必须跑 `tests/compaction_corpus.rs`（真实会话 + 合成语料的不变量回归）。
  触发口径只有一处：`Budget::trigger_tokens`（窗口 × 百分比，Agent 级
  `agent.json.compactThresholdPercent`，默认 75）；可选的
  `agent.json.compactTargetPercent` 只决定摘要后保留多少近期原文，以压缩前估算
  token 为基数，缺失时继续使用固定 2 万 token 预算，不改变触发线。手动压缩与自动压缩共用
  `runtime::run_compaction`，差别只在 `CompactionTrigger`（手动跳过阈值预检，
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
  的唯一来源。改这里必须跑 `tests/concurrent_sessions.rs`（多 Agent 同时跑、
  **同一个 Agent 两条会话同时跑**、停一条不影响其他、只释放空闲会话、事件与落盘
  互不串），并且别把前端拖回去：ChatView 卸载**不得**中止运行、切换 Agent 或切换
  会话**不得**被拦（后台运行是产品行为，不是 bug）。
- 权限是安全边界：bash 命令检查在 `pipi-core/src/tools/bash.rs` 执行前发生，
  改权限逻辑（`permissions/`）必须带测试，且宁可拒绝不可放行 —— 无法静态
  分析的命令一律视为危险。两条已定的判据边界，改动前先想清楚：
  **重定向**只拦「写入文件系统」（`>` `>>` `2>文件` `&>` `>|` `<>` `>&词`）
  与「从文件读入」（`<文件` `<(cmd)`），`2>/dev/null`、`2>&1`、`>&-`、
  heredoc / herestring 必须放行（它们既不落盘也不读文件，且实测占拒绝量的
  绝大多数，拦下来只有摩擦）—— 见 `permissions::check_write_redirect`；
  **`rm -f` 家族**只放行工作区内、非仓库根 / 工作区根的字面量绝对路径，
  包装调用（sudo/env/bash -c）、变量与通配符目标必须拒绝 —— 见
  `permissions::forced_rm_inside_workspace`。这两条改判据都要更新表驱动测试。
- 请求重试分两层，别把重试加到 provider 层：**provider 只分类**（rig 的结构化错误
  → 可重试 / 终态 + `Retry-After`，见 `retry::classify_rig_error`），**重发只在
  agent_loop 一处做**（`stream_assistant_response` 的尝试循环；摘要调用在
  `compaction::llm` 自己那层共用同一策略）—— 双层重试会放大成 N×M 次请求。
  三条不变量：**用户中止永远不可重试**；**已经输出过正文的那一轮不重放**（例外是
  只有工具调用块，半截参数绝不交给工具执行）；**无进展超时最多额外重试 1 次**。
  改分类表或判定规则必须更新 `retry.rs` 的表驱动测试与 `tests/retry_runtime.rs`。
- agent.toml 契约（`crates/av`，README「agent.toml 契约（av 标准）」一节）：
  未知键/未知段一律拒绝、秘密值只许引用式（永不内联）、`AV_*` 是运行时
  保留命名空间、项目层文件不得声明身份与权限段 —— 改 schema/合并/解析
  逻辑必须带测试，宁可拒绝不可放行；工具子进程环境一律取
  `ToolContext.resolved_env`（会话启动解析一次），不许在 spawn 点读进程环境。
