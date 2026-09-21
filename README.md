# Pipi

> 现在的 agent 应用都太重了。Pipi 想回到原点：**你创建的不是会话，是 Agent。**

Pipi 是一个用 Tauri 2.0 构建的、本地优先的 Agent 桌面应用。名字致敬 [pi](https://github.com/earendil-works/pi)。

---

## 为什么是 Pipi

主流 agent 应用的交互以"会话/聊天"为中心：开一个新聊天，重新选模型、重新描述背景、重新配置工具，聊完之后一切归零。这带来三个根本问题：

- **状态无法沉淀** —— 你的偏好、项目背景、工具配置散落在无数会话里，每个新会话都从零开始。
- **能力无法复用** —— 在一个会话里调好的 prompt 和工作流，换个会话就要重来一遍。
- **上下文不透明** —— 模型能看到的系统指令、工具、记忆被应用黑箱化，难以审视和修改。

Pipi 把抽象层级上移一层：**Agent 是一等公民**。

| | 会话中心（常见 AI 聊天应用） | Agent 中心（Pipi） |
| --- | --- | --- |
| 核心对象 | Chat / Thread | Agent |
| 配置 | 散落在每个会话里 | 沉淀在 Agent 上，一次配置持续生效 |
| 上下文 | 每次从零开始 | 工作目录、记忆、技能随 Agent 常驻 |
| 生命周期 | 聊完即弃，列表越堆越长 | Agent 长期存在，会话只是运行日志 |
| 数据 | 数据库 + 云同步 + 账号 | 一组本地文件 |

## 设计哲学

### 1. Agent 是一等公民，会话只是副产品

创建一个 Agent，就是声明一个持久实体：它绑定一个工作目录、一组技能、一份记忆、若干 MCP 服务器和一个模型。会话（session）只是 Agent 的一次运行记录，是派生物——可以归档、可以丢弃，Agent 本体永远在那里。

### 2. 一切皆文件

一个 Agent 的全部定义，就是磁盘上的一组普通文件：

```
~/.pipi/
├── settings.json       # 全局设置：主题、模型提供商（密钥支持环境变量引用）
└── agents/my-agent/
    ├── agent.json          # Agent 清单：模型、工作目录、权限、沙箱、压缩阈值、MCP 服务器等
    ├── agent.toml          # 环境契约（av 标准）：env 声明、工具链断言、资源覆盖（可选）
    ├── AGENTS.md           # 系统级指令，每次运行注入上下文
    ├── skills/             # 技能包（SKILL.md + 随附文件）
    │   └── git-safety/
    │       └── SKILL.md
    ├── memory/             # 持久记忆（Markdown，人机共写）
    ├── workspace/          # Agent 目录内的默认工作区（也可指向任意本地路径）
    └── sessions/           # 运行记录（JSONL，append-only）
```

项目侧还有一份可选的项目层契约：`<项目根>/agent.toml`（随仓库提交）与
`agent.local.toml`（本地覆盖层）。两层合并语义与注入规则见下文
「agent.toml 契约（av 标准）」一节。

- 没有数据库、没有私有格式、没有锁定。
- 任何编辑器都能改，git 就是版本管理，网盘就是同步。
- Pipi 的 UI 只是这些文件之上的一个**视图和执行器**。

### 3. 小核心，组合优于配置

核心只默认启用最小工具集（`read` / `write` / `edit` / `bash` / `memory` / `glob` / `grep`）。
跨 Agent 的 `create_agent` / `run_agent` / `read_agent` 是显式选择的组合能力，
不会因升级或新建 Agent 而自动开启。其余能力来自组合：

- **Skills** —— 用 Markdown 写的能力包，渐进式加载：只有描述常驻上下文，正文在被触发时才进入（同 pi 的做法）。
- **MCP** —— 标准工具协议，在 `agent.json` 里声明即可接入。
- 不做大而全的设置面板，做一组可组合的文件。

### 4. 本地优先

数据全部在本地，无账号、无云同步、无遥测。Rust 核心驱动 Agent 循环，前端只负责渲染。

## Agent 的组成

| 组件 | 载体 | 说明 |
| --- | --- | --- |
| 工作目录 | `agent.json` → `workspace` | Agent 可操作的文件系统范围，可指向任意本地项目（如 `~/projects/my-app`），缺省为 Agent 目录内的 `workspace/` |
| 模型 | `agent.json` → `provider` | OpenAI / Anthropic 兼容 API（流式）；`model` 字段仅作展示标签 |
| 系统指令 | `AGENTS.md` | 人直接读写的 Markdown，每次运行注入为系统提示 |
| 技能 | `skills/<name>/SKILL.md` | 能力包；仅 frontmatter 描述常驻，正文按需加载（M3） |
| 记忆 | `memory/*.md` | 由 `memory` 工具读写的持久记忆，跨会话生效；索引（路径+摘要）常驻系统提示，正文由模型按需 read |
| 命令权限 | `agent.json` → `permissions.bash` | bash 白名单 / 黑名单；引号感知的复合命令逐段检查；Allowlist 模式下白名单外的非危险命令可交互审批救回（拒绝 / 允许一次 / 总是允许——按段写回白名单），黑名单命中、危险命令与沙箱约束不可审批 |
| 沙箱 | `agent.json` → `permissions.sandbox` | `read-only` / `workspace-write` / `danger-full-access`（移植自 codex）：强制删除类命令、越出工作目录的写入与重定向在非完全访问下被拒绝 |
| 工具开关 | `agent.json` → `permissions.tools` | 基础工具按需启用；`create_agent` / `run_agent` / `read_agent` 必须显式开启 |
| 压缩阈值 | `agent.json` → `compactThresholdPercent` | 上下文占用达到模型窗口的这个百分比时自动压缩（默认 75）；窗口未知（0）时不压缩 |
| MCP | `agent.json` → `mcpServers` | Stdio MCP 服务器，会话启动时按需拉起（M3） |
| 环境契约 | `agent.toml`（项目根 / Agent 定义目录）+ `agent.local.toml` | av 标准：env 声明、工具链断言、资源覆盖；会话启动解析一次，秘密值永不内联 |
| 会话 | `sessions/*.jsonl` | Append-only 的运行记录，一文件一会话，树状条目（id/parentId）支持分叉 |

### 默认 Agent

首次启动（或 `~/.pipi/agents/Pipi/` 不存在）时，核心会自动播种一个名为 **Pipi** 的默认 Agent：
启用全部基础工具、`workspace-write` 沙箱、工作目录为自身目录下的 `workspace/`，
与手动新建的 Agent 完全同构 —— 想改就改 `~/.pipi/agents/Pipi/agent.json`。

**已存在时一律不覆盖**（哪怕文件被改坏也不动用户数据）；删掉该目录后下次启动会重新播种，
想彻底移除它请改名而不是删除。

### Agent 组合（第一阶段）

Agent 可以通过三项显式工具组合已有 Agent，而不引入独立的 subagent 类型：

- `create_agent`：用结构化参数创建一个普通、持久的 Agent；模型、工作目录、
  沙箱和基础工具从调用者继承，`instructions` 写入新 Agent 的 `AGENTS.md`。
  新 Agent 不继承三项 Agent 组合工具。
- `run_agent`：在目标 Agent 下创建一个全新 session，使用目标自己的配置同步
  运行，完成后把最终文本与 `sessionId` 返回调用者。单次运行有 10 分钟
  时间上限，超时与中止一样落盘为可读取的终态；运行期间的工具调用与轮次
  完成作为进度回流传回父会话。
- `read_agent`：按 `sessionId` 读取目标 Agent 已落盘的最终输出；省略时读取
  最近活跃的 session。

第一阶段固定为单层、同步委派：child 运行时只注册基础工具，不支持递归、后台
运行、所有权、消息邮箱或并行 child。完整过程仍写入目标 Agent 自己的
`sessions/*.jsonl`，所以输出既能由 `run_agent` 直接取得，也能之后用
`read_agent` 重读。

## 架构

```
┌─────────────────────────────────────┐
│        前端 · React + TypeScript     │  只渲染：Agent 列表、会话、文件视图
└────────────────┬────────────────────┘
                 │ Tauri IPC（invoke / events）
┌────────────────▼────────────────────┐
│  src-tauri      Rust 薄壳            │  commands：list/create/save agent
├─────────────────────────────────────┤
│  crates/pipi-core  Rust 核心         │  不依赖 Tauri，可独立测试
│  ├─ agent_loop    工具调用循环        │  LLM → 工具调用 → 执行 → 回喂
│  ├─ tools         文件工具 + Agent   │  workspace 工具默认启用；Agent
│  │                组合工具            │  组合工具显式启用
│  ├─ provider      anthropic +        │  流式 SSE，事件驱动
│  │                openai-compat      │
│  ├─ session       sessions/*.jsonl   │  append-only，崩溃安全
│  ├─ runtime       会话槽 + 事件协议    │  槽按 Agent 索引：可同时多轮
│  ├─ compaction    上下文压缩策略流水线  │  投影式（不落盘）+ 替换式（落盘）
│  ├─ permissions   bash 白/黑名单      │
│  └─ agents        扫描 ~/.pipi/agents │  一切皆文件
└──────────────────────────────────────┘
```

### 上下文压缩（策略与流水线）

把「一次压缩」拆成 **计划 → 校验 → 应用/落盘**，
策略只产出 `Plan`、不直接改历史 —— 不变量统一校验、UI 能观测（事件带策略名与
前后 token）、落盘能回放，都只依赖这一层。策略按**能否从原始历史重算**分两类，
这条线决定扩展成本：

| 类别 | 例子 | 持久化 | 应用位置 |
| --- | --- | --- | --- |
| `Projection`（投影式，纯函数、确定性） | 旧工具输出清理、边界硬裁 | **不落盘**（回放时重算） | `transform_context`（组装请求时） |
| `Replacement`（替换式，信息已丢失） | LLM 摘要 | 必须落盘为 `compaction` 条目 | turn 边界（runtime 触发） |

- 「何时压」由 runtime 决定，策略只回答「我能不能压」。触发口径只有一处
  （`Budget::trigger_tokens` = **窗口 × 百分比**，默认 75%，按 Agent 在
  `agent.json` 的 `compactThresholdPercent` 调；窗口未知时为 0 → 不压缩）；
  **手动压缩**（右栏「状态」→ 上下文行的「立即压缩」）跳过阈值 —— 用户点了就压，
  其余路径完全相同（同一个 `run_compaction`，因此同样分叉/归档/换会话）；
- 投影式流水线按成本从低到高跑：**先清旧工具输出**（不花 LLM 调用、不动对话
  结构，对齐 Claude Code 的「先清旧工具输出再摘要」与 Anthropic context editing
  的 `clear_tool_uses_*`），仍超预算才硬裁；因为不落盘，改这一档不动会话格式；
- 替换式目前只有摘要一档，落盘字段对齐 pi：`keep_from_entry`（= 上游
  `firstKeptEntryId`，回放时保住这段原文）、`strategy`、`usage`（摘要调用的用量
  计入会话账本，但不改写「当前上下文占用」口径，见 `stats::record_ledger`）；
- 硬不变量集中在 `compaction::validate` / `validate_result`：切点不得孤立
  toolResult、就地改写不得改角色、压缩后破损度不得增加（历史里本来就有的中段
  悬挂不算在压缩头上）；
- 参数集中在 `Budget::from_window`，加一档投影式压缩 = 一个纯函数 + 流水线里
  一行登记。

**压缩不原地改写会话**（可关）：摘要成功后先**分叉**出新会话（活跃路径完整拷贝 +
压缩条目 + 溯源标记 `compaction-fork:<原 id>`），把 writer 换过去，再把原会话移入
归档 —— 于是原会话始终是一份**完整记录**，新会话是压缩后的继续。归档链是嵌套的：
第二次压缩归档的是第一次的压缩产物，完整历史永远在最外层的归档里。

- 设置（`~/.pipi/settings.json` 的 `compaction` 段）：`forkBeforeCompact`（关掉即回到
  原地压缩）与 `archiveOriginal`（关掉则原会话留在活跃列表），默认都为 `true`。
- 失败降级：摘要调用失败 → 本轮不压缩；分叉失败 → 退回原地压缩；写新文件失败 →
  删掉半成品再退回原地；归档失败 → 记日志继续（原会话留在活跃列表）。
- 会话身份变了要通知前端：切完 writer 先发 `session-switched`（envelope 用**旧 id**，
  因为此刻前端身份还是旧的），再让后续事件带新 id —— 顺序反了前端会收不到切换、
  `running` 卡在 true。

### 并发模型（多路会话同时运行）

- **会话槽按 (Agent 名, 会话 id) 索引**：`RuntimeState` 持的是
  `Mutex<HashMap<SessionKey, Session>>`，不是「当前那个会话」。每个槽自带
  abort / 运行守卫 / writer / 统计，所以**同一个 Agent 可以同时开多条会话、每条
  各跑一轮；不同 Agent 更是互不影响，路数不限**（没有并发上限设置、没有信号量；
  每轮运行是注入运行时上的一个独立任务）。桌面壳与 Web 服务同款语义。
- **运行守卫按会话**：同一条会话同时只能跑一轮（**同会话串行是硬不变量**，第二轮
  会被拒并提示先停止或走插话）；别的会话、别的 Agent 照跑。`stop_run` 只停指定的
  那一条。
- **换会话要换键**：压缩分叉出新会话时，条目从旧键搬到新键（同一个 `Session`
  对象：运行守卫 / abort / writer 跟着走），随后发 `session-switched` 让前端把
  视图身份切到新 id。
- **身份进 IPC**：会话类命令都带 `agentName` + `sessionId`
  （`session_info` / `session_running` / `stop_run` / `steer` / `session_stats` /
  `session_messages` / `set_session_model` / `compact_now`；`send_prompt` 的
  `sessionId` 为空 = 新建一条）；`session_infos` 返回所有打开中的会话，前端由此
  得到「哪些会话正在跑」的**集合**，侧栏把标记打在那条**会话行**上。
- **事件本就带身份**：所有 envelope 都带 `agentName` + `sessionId` + `runId`，前端
  按身份过滤 —— 所以多会话并发不需要新的分发通道。
- **前端**：离开会话视图**不再中止运行**（后台继续跑、继续落盘；回到视图时用
  `session_messages` / `session_info` 重新水合）；切换 Agent / 切换会话都不再被拦 ——
  会话之间是独立的路，运行中的会话也可以随时打开查看。`new_session(agent)` 只释放该
  Agent 名下**空闲**的会话（正在跑的那条留着），用于「新建会话」与离开时的清理。
- **跨进程**：桌面壳与 Web 服务各自一个 `RuntimeState`，进程之间没有写者锁 —— 已知
  限制：别让两边同时开同一个会话。

### 与 pi 的关系

核心从 [pi](https://github.com/earendil-works/pi) 移植为 Rust，模块映射：

| pi | pipi-core | 备注 |
| --- | --- | --- |
| `packages/ai` types | `types` | 消息/内容块/事件协议，JSON 字段名与上游一致 |
| `packages/ai` api adapters | `provider` | 采用 rig（第三方）承载协议层，本仓库只做 pi 风格消息/事件的映射 —— 采纳 opencode「provider 交给 Vercel AI SDK」的同款决策 |
| `packages/agent` agent-loop | `agent_loop` | 事件流 + steering/follow-up + 工具批次执行；steering 已接线到 UI（运行中插话，迟到消息在运行结束收割、下次运行重放，不丢失） |
| `packages/agent` harness/tools | `tools` | read/write/edit/bash + 新增 memory/glob/grep；Pipi 增加显式的 Agent 创建、运行与输出读取工具 |
| `packages/agent` harness/utils/truncate | `truncate` | 2000 行 / 50KB，同一套提示文案 |
| `packages/agent` harness/session | `session` | 树状 JSONL Entry（id/parentId/seq）；Pipi 增量新增 `compaction` 条目类型 |
| `packages/agent` compaction（启发式） | `context` | token 估算（chars/4）、`prune_cut_index` 切点决策、`transformContext` 钩子 |
| `packages/agent` compaction（LLM 摘要替换） | `compaction` | 见下文「上下文压缩」：摘要替换旧轮次 + 保留近期原文（keepRecentTokens=20000）；摘要骨架 prompt、旧摘要经 `<previous-summary>` 交回 update 版指令、文件操作清单追加在摘要末尾；落盘为 `compaction` 条目（含 `keep_from_entry`），重开/分叉时回放 |
| `packages/agent` skills（frontmatter） | `skills` | 渐进式披露：索引常驻上下文，全文模型按需 read |

有意推迟移植（需要时再从上游搬）：hooks 全集、transformContext/
prepareNextTurn、其余 provider、图片工具。Pipi 自己新增：`permissions`
（命令权限）、`agents`（Agent 注册表）、`catalog`（模型目录，models.dev）、
memory 工具（含渐进召回注入）、glob/grep 检索工具。

**用量口径**（`types::Usage`，与 pi 的 `AssistantMessage["usage"]` 一致）：`input`
只计**未命中缓存**的提示词 token，命中/写入分别落在 `cache_read` / `cache_write`，
于是「提示词总量 = input + cache_read + cache_write」、「命中率 = cache_read / 提示词
总量」这两条算式在各协议下都成立。各协议的上报口径不同 —— OpenAI 兼容的
`prompt_tokens` **已含** `prompt_tokens_details.cached_tokens`（DeepSeek 的
`prompt_cache_hit_tokens` 同理），Anthropic 的 `input_tokens` 则不含 —— rig 两种都原样
透传，所以拆分在 `provider::from_rig_usage` 按 `Api::prompt_tokens_include_cache`
完成（对齐 pi `openai-completions.ts` 的 `Math.max(0, promptTokens - cacheRead - cacheWrite)`）。
上层（stats / 前端脚注）只读归一化后的值，不再自行相加。

### 与 opencode / hermes 的关系

- **opencode**（[anomalyco/opencode](https://github.com/anomalyco/opencode)，原 sst/opencode）：
  「provider 层用第三方库」的架构决策来自它
  （它用 Vercel AI SDK，我们用 rig）；`session.ts` 里 usage 的归一化口径
  （cache read/write 从输入中拆分）与我们的 Usage 字段一致（拆分位置见上文
  「用量口径」）。glob/grep 检索
  工具对齐它的结果上限约定：单次 100 条 + "use a more specific pattern"
  注释，`include` 支持 `*.{ts,tsx}` 花括号展开。
- **工具配对不变量**（pi + opencode）：OpenAI / Anthropic 都要求「带
  `tool_calls` 的 assistant 消息后面必须紧跟回答每个 `tool_call_id` 的 tool
  消息」，历史里任何破损（进程中断、批次中止、手工编辑）都会让请求被 400
  拒绝。Pipi 三层保证：批次中止时仍为每个调用产出错误结果（对齐 pi 的
  post-tools 不变量与 `createErrorToolResult`）；打开会话时修复被中断的
  **尾部回合**并落盘（对齐 pi recovery 对 orphaned 任务的结算）；发送前
  `context::repair_tool_pairing` 兜底（对齐 opencode `session/message-v2.ts`
  给 pending/running 调用补 `output-error` 的做法）。
- **模型目录**：运行时从 [models.dev](https://models.dev)（MIT，opencode 用的模型目录）
  拉取并缓存到 `~/.pipi/cache/models.json`；Pipi 只保留「收录哪些家 + 端点/协议的人工
  核实」这一张 `CURATION` 表（`crates/pipi-core/src/catalog.rs`）。上游给的是 AI SDK
  语义的 baseUrl，未经核实不得直接当 Pipi 的 baseUrl 用 —— 例如上游 DeepSeek 只给
  `https://api.deepseek.com`（无版本段），而 rig 的 OpenAI 兼容客户端会往后拼路径。
- **hermes**（NousResearch/hermes-agent）：会话统计面板整套语义来自它 ——
  滚动 N 次调用的平均 tok/s（sum(output)/sum(latency)）、缓存命中率
  （cache_read / prompt 总量，prompt 口径见上）、上下文占用用最近一次请求的实际值
  而非累计值，以及「数据不足时省略而不是编造 0」的原则。见 `stats.rs`。

### 与 codex 的关系

命令安全与沙箱概念移植自 [openai/codex](https://github.com/openai/codex)
（Apache-2.0，见 `pipi-core/src/permissions/safety.rs` 的 attribution）：

- `SandboxMode`（read-only / workspace-write / danger-full-access）→
  `permissions::SandboxMode`，语义一致（kebab-case 序列化兼容）。
- `is_dangerous_command`（rm -f 家族 + sudo/env/trap/bash-c 包装器解包 +
  深度上限 fail-closed）→ `permissions::safety`。上游用 tree-sitter 解析
  `bash -c` 脚本，我们不引入该依赖：脚本含语法关键字/命令替换时按危险
  处理（fail-closed）。
- AGENTS.md 项目文档发现（`core/src/agents_md.rs`：项目根定位 + 根→近
  逐层收集 + 字节预算）→ `project_doc`。
- 环境上下文注入（`environment_context`：cwd/沙箱/平台/日期）→
  `context::environment_context`。
- bash 输出截断时的 spill 文件来自 pi 的 output-capture：完整输出落盘、
  路径附在提示里。
- 尚未移植：OS 级沙箱（Landlock/Seatbelt）—— Pipi 当前是用户态粗粒度
  闸门（白/黑名单 + 危险启发式 + 重定向/写入路径约束），真正的强隔离
  列入 M3 后的路线。

## agent.toml 契约（av 标准）

Agent 运行时环境的声明式契约，实现在独立 crate [`crates/av`](crates/av)
（lib + CLI，pipi-core 以库形态消费同一份实现）。设计参考 uv：

- **一切皆文件**：环境不靠 shell 仪式（export / source），全部由文件声明；
- **确定性**：cwd 向上找最近的 `agent.toml`（`.git` 定位项目根，不越界继承
  外层 repo）；`agent.local.toml`（同目录本地层）覆盖之；
- **分层合并**：进程环境 → Agent 定义目录 `agent.toml` → 项目
  `agent.toml` → `agent.local.toml`；标量高层覆盖、映射按键合并、
  数组整体替换；
- **spawn 点注入**：会话启动时解析一次，bash 子进程环境由解析结果整体
  重建（`env_clear` + envs）——不靠"模型记得 export"，同会话内一致；
- **fail-closed**：未知键/未知段拒绝、秘密引用取不到即报错、requires
  版本不可判定即失败、路径逃逸包含根即拒绝；
- **秘密永不内联**：`secrets` 只接受 `{ env = "..." }` 透传或
  `{ file = "..." }`（0600）引用；会话记账（JSONL `env` 条目）只记
  键 + 来源层，秘密值永不落盘；
- **保留命名空间**：`AV` / `AV_*`（`AV_AGENT`、`AV_WORKSPACE`、
  `AV_SANDBOX`、`AV_SESSION`……嵌套感知变量）由运行时注入，声明文件
  不得触碰；
- **env 与权限分离**：env 只影响子进程看到什么，不影响它能做什么
  （命令白名单 / 沙箱照旧）。

```toml
schema = 1                            # 契约版本

[env]
inherit = "all"                       # all（默认）| core | none
ignore = ["AWS_*", "OPENAI_API_KEY"]  # 仅作用于继承的键
set = { RUST_BACKTRACE = "1" }
path-prepend = ["/opt/homebrew/bin"]
[env.secrets]                         # 值只能引用式取得
GITHUB_TOKEN = { file = "~/.pipi/secrets/gh.token" }

[[requires]]                          # 只校验、不安装（fail-closed）
command = "node"
version = ">=20"

[resources]                           # 资源覆盖：只允许路径，绝不内联正文
instructions = ["AGENTS.md"]          # 三态：缺席=约定发现 | 列表=完全替换 | []=禁用
max-bytes = 16384                     # 项目层字节预算（缺省 16KiB）
[resources.skills]
sources = [".pi/skills"]              # 按层替换约定目录
only = ["git-safety", "review-*"]     # 按技能名 glob 白名单
exclude = ["experimental-*"]          # 黑名单（优先于 only）
```

与 pi / codex 的既有约定对齐：项目层文件与 AGENTS.md 同属"随仓库走的
指令"，但**项目层文件不得声明身份与权限段**（`[model]`、`[permissions]`、
`[tools]`、`[mcp.*]` 出现即硬错误）——克隆来的仓库不能放宽沙箱或更换模型；
改动只发生在用户自己的 Agent 定义文件里。

CLI（调试用）：`av check`（静态校验）、`av env`（打印最终环境，秘密
脱敏，`--json`）、`av doctor`（requires 实测）。

## 开发

依赖：Node.js ≥ 20、Rust stable，Linux 下还需 Tauri 系统依赖：

```bash
# Debian / Ubuntu
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

```bash
npm install

npm run tauri dev      # 开发模式
npm run tauri build    # 打包

# 只动核心时（不启动桌面壳）
cargo test -p pipi-core
cargo check --workspace
```

## Web/远程模式

Pipi 也可以把 React 前端和 pipi-core 运行时以浏览器服务方式启动。服务默认
只监听 127.0.0.1:1421，适合由 Tailscale Serve 转发到 Tailnet 内的手机；
运行时仍然在电脑上执行，手机只负责显示界面和发送操作。

先构建前端，再启动 Web 服务：

    npm run build
    cargo run -p pipi-server

另一个终端将本地服务提供给 Tailnet：

    tailscale serve 1421

手机安装并登录 Tailscale 后，打开命令输出的 HTTPS 地址即可。需要认证时，
启动服务时设置 PIPI_AUTH_TOKEN，并首次使用带 token 查询参数打开地址；服务
会写入 HttpOnly cookie，后续 API 与 WebSocket 请求会自动携带认证信息：

    PIPI_AUTH_TOKEN=替换为随机长字符串 cargo run -p pipi-server

默认服务只允许本机访问；如果修改 PIPI_SERVER_ADDR 监听局域网或 Tailnet
地址，必须启用 PIPI_AUTH_TOKEN，并同时配置 Tailscale ACL。不要使用 Funnel
将具备文件写入和 bash 能力的 Agent 暴露到公网。

## 路线图

- [x] **M0 — 项目骨架**：Tauri 2.0 跑通，设计文档定稿
- [x] **M1½ — 核心移植**：pi 的 agent loop / 工具 / provider / session 移植为 Rust（`crates/pipi-core`），命令权限与工作目录进 `agent.json`
- [x] **M2 — Agent 管理**：UI 全字段编辑 `agent.json` / `AGENTS.md` 双向同步，memory 渐进召回（索引常驻 + 按需读取）
- [ ] **M3 — Skills 与 MCP**：Skills 渐进注入已实现；stdio MCP 服务器接入未动
- [x] **M4（部分）— 会话树状分叉**：`fork_session` 已实现并接线 UI；LLM 摘要式 compaction 已实现（turn 边界触发，摘要落盘可回放）。模板市场、多语言未动

## 致谢

- [pi](https://github.com/earendil-works/pi) —— 本项目大量设计灵感来自它：一切皆文件、小核心、渐进式上下文、会话即树。如果 Pipi 的方向让你兴奋，请先给它一个 star。
- [openai/codex](https://github.com/openai/codex) —— 命令安全评估与沙箱模式移植自它（Apache-2.0）。

## License

MIT（`pipi-core/src/permissions/safety.rs` 移植自 Apache-2.0 项目 openai/codex，保留其许可声明）。
