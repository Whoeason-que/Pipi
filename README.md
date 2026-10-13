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
    ├── agent.json          # Agent 清单：模型、工作目录、权限、沙箱、压缩阈值、提示词策略、MCP 服务器等
    ├── agent.toml          # 环境契约（av 标准）：env 声明、工具链断言、资源覆盖（可选）
    ├── agent.lock          # 技能解析锁：use 声明过的技能 → 精确修订 + 内容哈希（可选）
    ├── harness/            # 系统提示词素材（由 agent.json 的 systemPrompt 策略决定是否取材）
    │   ├── AGENTS.md       #   系统级指令，每次运行注入上下文
    │   ├── skills/         #   技能包（SKILL.md + 随附文件）
    │   │   └── git-safety/
    │   │       └── SKILL.md
    │   └── memory/         #   持久记忆（Markdown，人机共写）
    ├── workspace/          # Agent 目录内的默认工作区（也可指向任意本地路径）
    ├── jobs/               # 托管后台任务的状态 JSONL 与完整输出日志
    └── sessions/           # 运行记录（JSONL，append-only）
```

全局技能 store 由 av 独立管理（`AV_HOME` 可覆盖，默认 `~/.av`）：

```
~/.av/
├── skills.lock                     # store 索引：每个已装版本的来源、修订与内容哈希
└── skills/<name>/<hash16>/         # 不可变内容目录，名字 = 内容 sha256 前 16 位
    └── SKILL.md                    # 同名不同版本并存，不同项目可锁不同修订
```

项目侧还有一份可选的项目层契约：`<项目根>/agent.toml`（随仓库提交）与
`agent.local.toml`（本地覆盖层）。两层合并语义与注入规则见下文
「agent.toml 契约（av 标准）」一节。

- 没有数据库、没有私有格式、没有锁定。
- 任何编辑器都能改，git 就是版本管理，网盘就是同步。
- Pipi 的 UI 只是这些文件之上的一个**视图和执行器**。

### 3. 小核心，组合优于配置

核心只默认启用最小工具集（`read` / `write` / `edit` / `bash` / `memory` / `glob` / `grep`）。
跨 Agent 的 `create_agent` / `run_agent` / `read_agent`，以及会让进程跨越当前模型
轮次继续运行的 `submit_background_task` / `query_background_tasks` /
`manage_background_task`，都是显式选择的组合能力，不会因升级或新建 Agent 而自动
开启。其余能力来自组合：

- **Skills** —— 用 Markdown 写的能力包，渐进式加载：只有描述常驻上下文，正文在被触发时才进入（同 pi 的做法）。技能可以像库一样**安装到系统**（`av skill install`，全局 store `~/.av/skills`），再由契约里的一行 `use` 声明启用（见下文契约一节）。
- **MCP** —— 标准工具协议，在 `agent.json` 里声明即可接入。
- 不做大而全的设置面板，做一组可组合的文件。

### 4. 本地优先

数据全部在本地，无账号、无云同步、无遥测。Rust 核心驱动 Agent 循环，前端只负责渲染。

## Agent 的组成

| 组件 | 载体 | 说明 |
| --- | --- | --- |
| 工作目录 | `agent.json` → `workspace` | Agent 可操作的文件系统范围，可指向任意本地项目（如 `~/projects/my-app`），缺省为 Agent 目录内的 `workspace/` |
| 模型 | `agent.json` → `provider` | OpenAI / Anthropic 兼容 API（流式）；`model` 字段仅作展示标签 |
| 系统指令 | `harness/AGENTS.md` | 人直接读写的 Markdown，每次运行注入为系统提示（是否取材由 `systemPrompt` 策略决定） |
| 技能 | `harness/skills/<name>/SKILL.md`；全局 store `~/.av/skills` | 能力包；仅 frontmatter 描述常驻，正文按需加载（M3）。全局安装的技能用 `[resources.skills].use` 声明启用（装 ≠ 启用，见契约一节） |
| 记忆 | `harness/memory/*.md` | 由 `memory` 工具读写的持久记忆，跨会话生效；索引（路径+摘要）常驻系统提示，正文由模型按需 read |
| 系统提示词策略 | `agent.json` → `systemPrompt` | `both`（默认：Pipi 侧 + 工作区侧）/ `agent`（只取 `harness/`）/ `workspace`（只取项目上下文）/ `none`（完全不发 system 消息，只发对话与工具定义）。策略只影响提示词：权限、沙箱、env 与受信任读取根不受影响，`workspace` 策略的 Agent 照样能用 memory 工具读写 `harness/memory/` |
| 命令权限 | `agent.json` → `permissions.bash` | bash 白名单 / 黑名单；引号感知的复合命令逐段检查；Allowlist 模式下白名单外的非危险命令可交互审批救回（拒绝 / 允许一次 / 总是允许——按段写回白名单），黑名单命中、危险命令与沙箱约束不可审批 |
| 沙箱 | `agent.json` → `permissions.sandbox` | `read-only` / `workspace-write` / `danger-full-access`（移植自 codex）：强制删除类命令、写入文件系统的重定向（`>` `>>` `2>文件`）与从文件读入的重定向（`<文件` `<(cmd)`）在非完全访问下被拒绝。`rm -f` 家族只放行「工作区内、非仓库根 / 工作区根」的绝对路径字面量；`2>/dev/null`、`2>&1`、管道与 heredoc 不受限 |
| 工具开关 | `agent.json` → `permissions.tools` | 基础工具按需启用；Agent 组合工具与后台任务托管工具必须显式开启 |
| Agent 分组 | `agent.json` → `subagent` | `false`（默认）显示在 AGENTS；Agent 的 `create_agent` 工具创建时写为 `true` 并显示在 SUBS；可在设置中更改 |
| 压缩阈值 | `agent.json` → `compactThresholdPercent` | 上下文占用达到模型窗口的这个百分比时自动压缩（默认 75）；窗口未知（0）时不压缩 |
| 压缩后目标 | `agent.json` → `compactTargetPercent` | 可选的压缩后上下文目标，占本次压缩前估算 token 的百分比（1–99）；未设置时仍保留最多 2 万 token 近期原文，避免改变旧 Agent 行为 |
| 请求重试 | `settings.json` → `retry` | 可重试错误的重发策略：`maxAttempts`（含首次，1 = 不重试）/ `baseDelayMs` / `maxDelayMs`；默认 3 次尝试、1s 起指数退避、30s 封顶、25% 抖动，尊重 `Retry-After` |
| MCP | `agent.json` → `mcpServers` | Stdio MCP 服务器，会话启动时按需拉起（M3） |
| 环境契约 | `agent.toml`（项目根 / Agent 定义目录）+ `agent.local.toml` + `agent.lock` | av 标准：env 声明、工具链断言、资源覆盖与技能包管理（`av skill`）；会话启动解析一次，秘密值永不内联 |
| 会话 | `sessions.git/`（git 分支） | 一轮一个 commit，一条 entry 一个文件（内容就是那一行 JSON）；压缩＝新建分支、`git log` 即对话。旧 `sessions/*.jsonl` 会话照旧可读可续写，导入后迁到 git（Agent 工作台「文件」页） |
| 后台任务 | `jobs/<job-id>.jsonl` + `.log` | shell 进程与 child Agent 的生命周期、状态和输出；任务按 Agent/session 所有，重启只显示 orphaned，不按旧 PID 或 future 自动接管 |

### 默认 Agent

首次启动（或 `~/.pipi/agents/Pipi/` 不存在）时，核心会自动播种一个名为 **Pipi** 的默认 Agent：
启用全部基础工具、`workspace-write` 沙箱、工作目录为自身目录下的 `workspace/`，
与手动新建的普通 Agent 完全同构（`subagent: false`）—— 想改就改 `~/.pipi/agents/Pipi/agent.json`。

**已存在时一律不覆盖**（哪怕文件被改坏也不动用户数据）；删掉该目录后下次启动会重新播种，
想彻底移除它请改名而不是删除。

### Agent 设置工作台与临时测试

点击 Agent 名称或齿轮会进入设置工作台：中间是复用正式会话交互链路的测试对话，
右侧收纳模型、工作目录、权限、技能、Agent 文件和运行诊断。测试只读取**已经保存**的
`agent.json`、`harness/AGENTS.md`、`harness/memory/*.md` 与技能声明（`agent.toml` + `agent.lock`）；
保存配置或文件后会换成一个新的空测试上下文。

右栏的**技能**页管理这个 Agent 声明启用的 store 技能：启用 / 禁用、解析状态
（已就绪 / 内容缺失 / 未解析 / 被 only/exclude 过滤）、以及「同步声明技能」
（缺内容按锁复现，勾选「修复漂移」才重装与锁不一致的内容）。这些开关**立即写盘**
（与 `av skill add/remove` 同一实现，不需要「保存配置」），写完会重建临时测试会话，
让改动当场生效、fail-closed 错误当场暴露。

工作台测试的会话账本只存在于当前 Pipi 进程内，不创建 `sessions/*.jsonl`、不进入
会话列表，也不能归档或分叉；切换 Agent 或普通会话不会中止它，返回工作台时会从
内存重新水合。这里的“临时”**只表示聊天记录和统计不持久化**：模型仍按 Agent 的
真实工具、权限、沙箱和工作目录运行，命令、文件写入及其他外部副作用都会保留，
清空测试不会回滚这些改动。需要可审计记录时，应从左侧新建正式会话。

### 左侧图标栏与分区

窗口最左边是一条常驻的图标栏，把几件事拆成各自的分区，而不是挤在同一栏里：

- **Agent**（普通 Agent 列表，含新建入口与已归档区）；
- **Subagent**（`agent.json` 里 `subagent: true` 的 Agent 单独一栏）；
- **搜索**（跨 Agent 与会话的检索面板，点击结果直接跳转）；
- **技能**（全局技能 store 的技能库：已安装版本与来源、内容校验、安装 / 移除，
  以及「哪些 Agent 正在用它、一键启用 / 禁用」）；
- **设置**（全局设置，见下节）。

当前分区在图标栏上高亮；Agent 列表不再就地过滤（搜索有自己的面板）。窄屏（≤860px）
下图标栏仍常驻，点分区图标即展开对应抽屉。

**技能库**是 `av` 技能包系统的前端视图，语义与 CLI 完全一致：安装只把内容放进
`~/.av/skills`（装 ≠ 启用），启用才写 Agent 契约里的 `[resources.skills].use`
与同层 `agent.lock`。启用 / 禁用写的是该 Agent **生效的最高声明层**契约（声明落在
工作区的项目层时就写那个文件），点选技能即可看到影响面；「校验全部」重算内容哈希
查缺失与漂移，漂移要在 Agent 工作台的「技能」页勾选「修复漂移」重装。git 源会联网
取精确 commit，所以安装 / 同步走阻塞线程池、界面不阻塞。

### 全局设置

图标栏底部的齿轮打开**全局设置**：它不是弹窗，而是主区的一个 tab（与对话界面同构 ——
悬浮顶栏 + 返回按钮 + 左侧分组导航）。左导航按类别划分设置：

- **外观**：深色 / 浅色主题；
- **上下文**：压缩是否先分叉新会话、分叉后是否归档原会话；
- **模型提供商**：端点与密钥、默认提供商、模型目录刷新；协议默认「自动」（按模型
  决定，见「与 opencode 的关系」的模型目录一条），也可对某一家固定协议；
- **账户**：Web 模式下的远程访问凭据与退出登录（仅在需要认证时出现）。

改动即时保存（写入 `~/.pipi/settings.json`）；齿轮在设置打开时高亮，再点一次或点顶栏
返回即回到设置前的界面；点图标栏的其他分区、侧栏点 Agent 或新建 Agent 也会切走设置。
窄屏（≤860px）下左导航压成横向标签条。

### Agent 组合（第一阶段）

Agent 可以通过三项显式工具组合已有 Agent：

- `create_agent`：用结构化参数创建一个持久的 Agent，并将 `agent.json` 的
  `subagent` 设为 `true`；模型、工作目录、沙箱和基础工具从调用者继承，
  `instructions` 写入新 Agent 的 `harness/AGENTS.md`，并建好 `harness/` 骨架。
  设置界面可将它改回普通 Agent。
  新 Agent 不继承三项 Agent 组合工具。
- `run_agent`：在目标 Agent 下创建一个全新 session。若当前 Agent 同时启用了
  `query_background_tasks` 与 `manage_background_task`，默认提交为托管后台任务并
  立即返回 `jobId`；可用 `background:false` 保留同步等待并返回最终文本与
  `sessionId`。异步任务通过同一组后台任务工具查询输出、等待变化或终止。
  单次 child 运行有 10 分钟时间上限，超时与中止一样落盘为可读取的终态；运行
  期间的工具调用与轮次完成作为任务输出记录。
- `read_agent`：按 `sessionId` 读取目标 Agent 已落盘的最终输出；省略时读取
  最近活跃的 session。

Agent 组合仍固定为单层：child 运行时只注册基础工具，不支持递归委派。shell 进程
与异步 child Agent 共用三项显式托管工具，任务按父 Agent 的 session 所有，完整
状态写入父 Agent 的 `jobs/`；应用重启后运行态任务只显示为 `orphaned`，不冒险
接管遗留 PID 或未完成的 child future。

## 架构

```
┌──────────────────────────────────────┐
│        前端 · React + TypeScript      │  只渲染：Agent 列表、会话、文件视图
└────────────────┬─────────────────────┘
                 │ Tauri IPC（invoke / events）
┌────────────────▼─────────────────────┐
│  crates/pipi    Tauri 薄壳           │  commands：只做 IPC 转发
│  pipi-core      Web 宿主（feature）  │  axum WS + /api/invoke + 鉴权
├──────────────────────────────────────┤
│  pipi-core      宿主层               │  命令表、会话槽、运行编排、审批、后台
│  ├─ runtime      多会话索引与装配    │  主轮 / child / 压缩共用单会话内核
│  ├─ commands     宿主命令表          │  参数结构 + 实现 + dispatch（只有一份）
│  ├─ settings     全局设置            │  供应商、压缩阈值、重试策略
│  └─ tools::agent Agent 组合工具      │  显式启用；基础工具来自 pipi-tools
├──────────────────────────────────────┤
│  pipi-agent     L5 单会话内核        │  生命周期 / 账本 / 主循环 / 压缩编排
│  pipi-context   上下文与运行策略     │  预算 / 裁剪 / 压缩流水线 / 重试退避
│  pipi-venv      Agent 环境域         │  清单与注册表 / av 契约（含 av CLI）
│                                      │  / 技能中心 / 纯提示词层
├──────────────────────────────────────┤
│  pipi-protocol  契约层               │  DTO + 会话条目格式 + 端口（唯一被所有层依赖）
│  pipi-session   会话载体             │  git 分支（一轮一 commit）+ 旧 JSONL 兼容
│  pipi-tools     工具实现             │  内置工具 + bash 权限策略
│  pipi-provider  模型面               │  rig 适配 + models.dev 目录（唯一 HTTP）
│  pipi-error     错误语义             │  错误码与 RetryHint
└──────────────────────────────────────┘
```

前端的会话状态独立于组件生命周期：`session-runtime.ts` 统一订阅宿主事件，按
`(Agent 名, sessionId)` 路由到 `session-controller.ts`。控制器负责历史快照、
run 代际、停止确认与压缩重键；`Chat.tsx` 只持有输入、滚动和检查器等界面状态。
退出或卸载视图不会中止运行。实际运行状态仍由 Rust 会话守卫与 `session_infos`
确认，持久历史仍来自 JSONL；前端缓存不设置运行会话数量上限。

`pipi_agent::session::AgentSession` 拥有单会话的消息、账本、模型绑定、统计、
运行守卫与 abort / steering。宿主注入存储端口、provider、工具与已解析的预算和选项；
同步预约并落输入后返回可调度的 `PreparedRun`，内核不选择执行器、不读取产品配置。
手动压缩、交互运行和一次性子运行共用进程级 runId 分配与同会话互斥。

`pipi_core::runtime` 的公共入口位于 `runtime/mod.rs`，内部按 `slots`（多会话索引）、
`lifecycle`（宿主生命周期）、`execution` / `child` / `compaction`（能力装配与调度）、
`model`（模型与凭据解析）、`events`（身份信封与宿主重键）拆分。内核压缩更换账本后
通知宿主移动索引和后台任务归属，再发旧 id 的 `session-switched`，后续事件用新 id。
宿主事件协议（`RuntimeEvent` 与各 envelope）在 `pipi_core::events`，审批与
runtime 都依赖这个叶子模块，`background` 通过 `ChildAgentRunner` 端口拿到 child
运行能力。桌面与 Web 共用这些实现（Web 宿主是 `pipi_core::server`，`--features
server`）。前端 `ipc.ts` 声明命令参数、返回值和事件类型，`npm run test:frontend`
会核对桌面注册、Web 路由和共享 JSON wire 样例；宿主层的
`cargo test -p pipi-core` 验证同一份样例的 Rust 序列化。

复杂度优化的实施记录、基线和验证范围见 [docs/complexity-optimization.md](docs/complexity-optimization.md)。

### 上下文压缩（策略与流水线）

把「一次压缩」拆成 **计划 → 校验 → 应用/落盘**，
策略只产出 `Plan`、不直接改历史 —— 不变量统一校验、UI 能观测（事件带策略名与
前后 token）、落盘能回放，都只依赖这一层。策略按**能否从原始历史重算**分两类，
这条线决定扩展成本：

| 类别 | 例子 | 持久化 | 应用位置 |
| --- | --- | --- | --- |
| `Projection`（投影式，纯函数、确定性） | 旧工具输出清理、边界硬裁 | **不落盘**（回放时重算） | `transform_context`（组装请求时） |
| `Replacement`（替换式，信息已丢失） | LLM 摘要 | 必须落盘为 `compaction` 条目 | turn 边界（单会话内核触发） |

- 「何时压」由 `pipi-agent::session` 决定，策略只回答「我能不能压」。触发口径只有一处
  （`Budget::trigger_tokens` = **窗口 × 百分比**，默认 75%，按 Agent 在
  `agent.json` 的 `compactThresholdPercent` 调；窗口未知时为 0 → 不压缩）；
  **手动压缩**（右栏「状态」→ 上下文行的「立即压缩」）跳过阈值 —— 用户点了就压，
  其余路径完全相同（同一个 `run_compaction`，因此同样分叉/归档/换会话）；
- 「压到多少」独立于触发线：可选的 Agent 级 `compactTargetPercent` 以本次压缩前
  的估算 token 为基数，先给摘要预留输出预算，再决定保留多少近期原文；未设置时
  沿用固定 2 万 token 尾部预算。切点只落 user 边界，故目标是近似值；压缩事件
  展示前后估算 token 与保留比例。该设置不改变何时触发自动压缩；保存后对已打开会话随后启动的轮次或手动压缩生效，正在执行的轮次沿用启动时的设置；
- 投影式流水线按成本从低到高跑：**先清旧工具输出**（不花 LLM 调用、不动对话
  结构，对齐 Claude Code 的「先清旧工具输出再摘要」与 Anthropic context editing
  的 `clear_tool_uses_*`），仍超预算才硬裁；因为不落盘，改这一档不动会话格式；
- 替换式目前只有摘要一档，落盘字段对齐 pi：`keep_from_entry`（= 上游
  `firstKeptEntryId`，回放时保住这段原文）、`strategy`、`usage`（摘要调用的用量
  计入会话账本，但不改写「当前上下文占用」口径，见 `stats::record_ledger`）；
  重开时从活跃路径的原始消息和压缩用量重建累计统计，被摘要替换的请求仍计费；
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

### 请求重试（可重试错误的重发）

上游供应商的失败里有一半是瞬时的（限流、5xx、连接被重置、代理把 SSE 掐断），
重发一次往往就好；另一半（请求不合法、鉴权失败、上下文超限、配额耗尽）重发只会白等。
所以**分类**与**重发**分在两层做（`retry` 模块）：

- **provider 层只分类**：把 rig 的结构化错误映射成「可重试 / 终态」，并把服务端
  要求的等待（`Retry-After`）带进 `StreamEvent::Error`。分类表：

  | 错误 | 判定 |
  | --- | --- |
  | HTTP 408 / 409 / 429 / 5xx | 可重试 |
  | 传输失败（发送失败、连接重置、DNS、流提前结束） | 可重试 |
  | 工具调用参数被截断（rig `UnparseableToolInput`） | 可重试 |
  | HTTP 400 / 401 / 403 / 404 / 413 / 422 | 终态 |
  | 429 但 body 是配额 / 计费（`insufficient_quota` 等） | 终态 |
  | 请求构造、序列化、URL 错误 | 终态 |
  | 用户中止 | 永不重试 |

- **agent_loop 层重发**（唯一的重发点，避免多层重试放大）：按策略退避后重发整轮
  请求，退避期间与 `AbortSignal` 竞速（点停止立刻生效），并向前端发 `retry_start`
  （对话里出现一行「↻ 2 秒后重试（1/2）· 原因」）。三条硬规则：
  1. **已经向用户输出过正文就不再重放**（重复文本比失败更糟）——提示为
     「已输出部分内容，未自动重试」，用户可手动继续；
  2. **例外**：只有工具调用块（参数被打断）时允许重放，半截参数绝不交给工具执行；
  3. **无进展超时最多额外重试 1 次**（每次尝试都要花掉整个 timeout 预算，
     三次就是 15 分钟，用户会以为卡死）。
- **摘要压缩调用共用同一策略**（`compaction::llm::summarize`）：它把文本攒在本地、
  没有增量发给用户，重放天然安全，所以不受「已输出正文」那条限制；顺带补上
  「流没走到结束帧就不算成功」的校验（此前会把半截摘要当成功）。
- 策略配置：`~/.pipi/settings.json` 的 `retry` 段（`maxAttempts` / `baseDelayMs` /
  `maxDelayMs`，字段级 default 兼容旧文件，读入时夹到安全区间），设置面板的
  「重试」分组可改。

**不做**：请求级断点续流（四家上游都不做，rig 的 SSE 重连需要消费者在错误后继续
poll 才生效，且 4xx/5xx 本就不在其重试范围内）；上下文超限的「压缩后自动重发」
（那是另一个特性，当前按终态处理并在错误里说清原因）；凭据轮换与备用模型切换。

### 并发模型（多路会话同时运行）

- **会话槽按 (Agent 名, 会话 id) 索引**：`RuntimeState` 持的是
  `Mutex<HashMap<SessionKey, HostSession>>`，每个槽持有一个 `AgentSession`，内核拥有
  abort / 运行守卫 / writer / 统计，所以**同一个 Agent 可以同时开多条会话、每条
  各跑一轮；不同 Agent 更是互不影响，路数不限**（没有并发上限设置、没有信号量；
  每轮运行是注入运行时上的一个独立任务）。桌面壳与 Web 服务同款语义。
- **运行守卫按会话**：同一条会话同时只能跑一轮（**同会话串行是硬不变量**，第二轮
  会被拒并提示先停止或走插话）；别的会话、别的 Agent 照跑。`stop_run` 只停指定的
  那一条。
- **换会话要换键**：压缩分叉出新会话时，条目从旧键搬到新键（同一个 `AgentSession`
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

| pi | crate / 模块 | 备注 |
| --- | --- | --- |
| `packages/ai` types | `pipi-protocol` | 消息/内容块/事件协议，JSON 字段名与上游一致 |
| `packages/ai` api adapters | `pipi-provider` | 采用 rig（第三方）承载协议层，本仓库只做 pi 风格消息/事件的映射 —— 采纳 opencode「provider 交给 Vercel AI SDK」的同款决策 |
| `packages/agent` agent-loop | `pipi-agent::agent_loop` | 事件流 + steering/follow-up + 工具批次执行；steering 已接线到 UI（运行中插话，迟到消息在运行结束收割、下次运行重放，不丢失） |
| `packages/agent` harness/tools | `pipi-tools` + `pipi-core::tools::agent` | `pipi-tools` 提供 read/write/edit/bash/memory/glob/grep；核心保留显式的 Agent 创建、运行与输出读取组合工具 |
| `packages/agent` harness/utils/truncate | `pipi-tools::truncate` | 2000 行 / 50KB，同一套提示文案 |
| `packages/agent` harness prompt/resources | `pipi-venv::prompt` | 项目上下文发现、UTF-8 预算和纯 system prompt 渲染；不依赖 Agent 存储或运行时 |
| `packages/agent` harness/session | `pipi-protocol::session`（条目格式与回放）+ `pipi-session`（载体） | 树状 Entry（id/parentId/seq）；载体默认是 git 分支（一轮一 commit，压缩＝建分支），旧 JSONL 文件按 id 兼容；Pipi 增量新增 `compaction` 条目类型 |
| `packages/agent` harness/runtime | `pipi-agent::session` | 单会话生命周期、运行守卫、历史恢复、累计用量和压缩持久化；通过协议端口注入能力 |
| （Pipi 应用层） | `pipi-core::runtime` | 会话槽、后台 Agent 编排和宿主事件协议；Tauri 与 Web server 共用，IPC payload 不变 |
| `packages/agent` compaction（启发式） | `pipi-context::context` | token 估算（chars/4）、`prune_cut_index` 切点决策、`transformContext` 钩子 |
| `packages/agent` compaction（LLM 摘要替换） | `pipi-context::compaction` | 见下文「上下文压缩」：摘要替换旧轮次 + 保留近期原文（keepRecentTokens=20000）；摘要骨架 prompt、旧摘要经 `<previous-summary>` 交回 update 版指令、文件操作清单追加在摘要末尾；落盘为 `compaction` 条目（含 `keep_from_entry`），重开/分叉时回放 |
| `packages/agent` skills（frontmatter） | `pipi-venv::skills` | 渐进式披露：索引常驻上下文，全文模型按需 read；技能包管理（store/锁/安装）见「agent.toml 契约」一节 |

有意推迟移植（需要时再从上游搬）：hooks 全集、transformContext/
prepareNextTurn、其余 provider、图片工具。Pipi 自己新增：`pipi-tools::permissions`
（命令权限）、`pipi-venv::agents`（Agent 注册表）、`pipi-provider::catalog`（模型目录，
models.dev）、`pipi-venv::skills`（技能中心）、
memory 工具（含渐进召回注入）、glob/grep 检索工具。

**用量口径**（`pipi_protocol::Usage`，与 pi 的 `AssistantMessage["usage"]` 一致）：`input`
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
  核实」这一张 `CURATION` 表（`crates/pipi-provider/src/catalog/mod.rs`）。上游给的是 AI SDK
  语义的 baseUrl，未经核实不得直接当 Pipi 的 baseUrl 用 —— 例如上游 DeepSeek 只给
  `https://api.deepseek.com`（无版本段），而 rig 的 OpenAI 兼容客户端会往后拼路径。
  **协议默认由系统管理**：上游的模型级标注（`model.provider.npm` —— 很多网关按模型分
  协议，例：opencode zen/go 的 Claude 系只有 Anthropic Messages，其余模型只有 OpenAI
  兼容）与运行期实证（端点回「不支持这个协议」就换另一种再试一次，成功的那套记在
  进程内）一起决定每个模型实际走哪套 wire，用户不需要配置协议；想钉死某家的协议时，
  在设置里把该提供商的「API 协议」从「自动」改成具体协议即可（`ProviderConfig.apiMode`）。
- **hermes**（NousResearch/hermes-agent）：会话统计面板整套语义来自它 ——
  滚动 N 次调用的平均 tok/s（sum(output)/sum(latency)）、缓存命中率
  （cache_read / prompt 总量，prompt 口径见上）、上下文占用用最近一次请求的实际值
  而非累计值，以及「数据不足时省略而不是编造 0」的原则。见 `pipi-agent::stats`。

### 与 codex 的关系

命令安全与沙箱概念移植自 [openai/codex](https://github.com/openai/codex)
（Apache-2.0，见 `crates/pipi-tools/src/permissions/safety.rs` 的 attribution）：

- `SandboxMode`（read-only / workspace-write / danger-full-access）→
  `pipi_tools::permissions::SandboxMode`，语义一致（kebab-case 序列化兼容）。
- `is_dangerous_command`（rm -f 家族 + sudo/env/trap/bash-c 包装器解包 +
  深度上限 fail-closed）→ `pipi_tools::permissions::safety`。上游用 tree-sitter 解析
  `bash -c` 脚本，我们不引入该依赖：脚本含语法关键字/命令替换时按危险
  处理（fail-closed）。
- AGENTS.md 项目文档发现（`core/src/agents_md.rs`：项目根定位 + 根→近
  逐层收集 + 字节预算）→ `pipi-venv::prompt` 的资源发现（`resources.rs`）。
- 环境上下文注入（`environment_context`：cwd/沙箱/平台/日期）→
  `context::environment_context`。
- bash 输出截断时的 spill 文件来自 pi 的 output-capture：完整输出落盘、
  路径附在提示里。
- 尚未移植：OS 级沙箱（Landlock/Seatbelt）—— Pipi 当前是用户态粗粒度
  闸门（白/黑名单 + 危险启发式 + 重定向/写入路径约束），真正的强隔离
  列入 M3 后的路线。
- 用户态闸门的分工（有意为之）：重定向约束只覆盖**写入文件系统**（拦
  `>` `>>` `2>文件`）与**从文件读入**（拦 `<文件` `<(cmd)`）；`2>/dev/null`、
  `2>&1`、`>&-`、heredoc 既不落盘也不读文件，一律放行。它保护的是「工作区
  写入必须走 `write` / `edit` 的 canonical containment」，**不是防逃逸**——
  `tee`、`python -c "open(p,'w')"` 本就在用户态闸门之外（见
  `permissions` 模块文档的局限声明），多拦不含写入的构造只产生摩擦。
  `rm -f` 家族同理：只放行工作区内、非仓库根 / 工作区根的字面量绝对路径，
  用来清构建与缓存产物；含变量 / 通配符或经 `cd` 的相对路径照旧拒绝。

## agent.toml 契约（av 标准）

Agent 运行时环境的声明式契约，实现在 [`crates/pipi-venv/src/av`](crates/pipi-venv/src/av)
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
instructions = ["harness/AGENTS.md"]  # 三态：缺席=约定发现 | 列表=完全替换 | []=禁用
max-bytes = 16384                     # 项目层字节预算（缺省 16KiB）
[resources.skills]
sources = [".pi/skills"]              # 按层替换约定目录
use = ["pdf", "commit-helper"]        # 启用全局 store（~/.av/skills）里已安装的技能
only = ["git-safety", "review-*"]     # 按技能名 glob 白名单
exclude = ["experimental-*"]          # 黑名单（优先于 only）
```

### 技能包管理（av skill）

像 uv 管 Python 依赖那样管技能：**安装 ≠ 启用**，声明与解析分离。

- **全局 store**：`~/.av/skills/<name>/<hash16>/`（`AV_HOME` 可覆盖）。
  内容目录以内容 sha256 命名、一经发布不可变；同名不同版本并存，
  不同项目可以锁不同修订。
- **安装**：`av skill search <关键词>` 在 skills.sh 搜索；`av skill install
  <来源>` 只装进 store；`av skill add <来源>` 一步完成安装 + 在契约里写
  `use` + 生成同层 `agent.lock`。来源支持 `owner/repo[@ref][#子目录]`
  （GitHub 短名，skills.sh 生态约定）、`gitlab:owner/repo`、完整 git URL
  （https/ssh/git@）、本地目录与 GitHub/GitLab 的 tree 网页 URL；
  git 走系统 `git`，浅取到精确 commit（不引入 git 库，也不做归档直链下载）。
- **声明启用**：契约里 `[resources.skills] use = [...]`（三态：缺席 =
  不启用 | 列表 = 启用这些 | `[]` = 显式禁用；与其它数组一致，最高声明层
  整体替换）。解析结果记录在同目录 `agent.lock`：来源、ref、精确 commit、
  内容哈希、来源内路径。
- **会话启动只读、不联网**：逐名解析到 store 内容目录，注入技能索引
  （目录同时进受信任读取根，模型用 `read` 按需加载全文）；缺锁、缺条目、
  缺内容一律 fail-closed 并提示 `av skill sync` —— 不在会话启动时静默联网安装。
- **可复现**：克隆下来的仓库带着 `agent.toml` + `agent.lock`，`av skill
  sync` 按锁重建（git 源取锁定 revision，复现内容哈希必须一致，否则拒绝
  安装）；`av skill verify` 重算哈希查漂移；`av skill update` 重解析 ref 到
  新修订；`av skill remove <名称> [--purge]` 移除声明（`--purge` 才动 store）。
- **供应链约束**：拒绝符号链接与路径逃逸，单技能限额（≤500 文件 /
  ≤5 MiB）；`sync` 先打印将安装的来源清单，非交互环境需要 `--yes`。

与 pi / codex 的既有约定对齐：项目层文件与 AGENTS.md 同属"随仓库走的
指令"，但**项目层文件不得声明身份与权限段**（`[model]`、`[permissions]`、
`[tools]`、`[mcp.*]` 出现即硬错误）——克隆来的仓库不能放宽沙箱或更换模型；
改动只发生在用户自己的 Agent 定义文件里。

应用内的**技能库**（图标栏「技能」）与 Agent 工作台的「技能」页是这套机制的
前端视图，读写与 CLI 共用同一份 av 实现：安装 / 校验 / 移除在技能库，启用 /
禁用 / 同步在 Agent 侧。两个面板都遵守同一条不变式 —— **写入只由用户显式触发**，
会话启动路径依旧只读、不联网（缺锁 / 缺内容 fail-closed 并提示同步）。

CLI：`av check`（静态校验，含声明技能是否已锁且已装）、`av env`（打印最终
环境，秘密脱敏，`--json`）、`av doctor`（requires 实测 + 技能内容哈希深检）、
`av skill search|install|add|sync|list|verify|update|remove`（技能包管理）。

## 开发

依赖：Node.js ≥ 20、Rust stable，Linux 下还需 Tauri 系统依赖：

```bash
# Debian / Ubuntu
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

```bash
npm install

npm run tauri:dev      # 开发模式
npm run tauri:build    # 打包

# 只动核心时（不启动桌面壳）
cargo test -p pipi-agent -p pipi-core -p pipi-venv
cargo check --workspace

# 分层与依赖方向检查（改 Cargo.toml 依赖或新增 crate 必跑）
cargo xtask arch
```

## Web/远程模式

Pipi 也可以把 React 前端和宿主运行时以浏览器服务方式启动。服务默认
只监听 127.0.0.1:1421，适合由 Tailscale Serve 转发到 Tailnet 内的手机；
运行时仍然在电脑上执行，手机只负责显示界面和发送操作。

先构建前端，再启动 Web 服务：

    npm run build
    cargo run -p pipi-core --features server --bin pipi-server

另一个终端将本地服务提供给 Tailnet：

    tailscale serve 1421

手机安装并登录 Tailscale 后，打开命令输出的 HTTPS 地址即可。需要认证时，
启动服务时设置 PIPI_AUTH_TOKEN，并首次使用带 token 查询参数打开地址；服务
会写入 HttpOnly cookie，后续 API 与 WebSocket 请求会自动携带认证信息：

    PIPI_AUTH_TOKEN=替换为随机长字符串 cargo run -p pipi-core --features server --bin pipi-server

默认服务只允许本机访问；如果修改 PIPI_SERVER_ADDR 监听局域网或 Tailnet
地址，必须启用 PIPI_AUTH_TOKEN，并同时配置 Tailscale ACL。不要使用 Funnel
将具备文件写入和 bash 能力的 Agent 暴露到公网。

## 路线图

- [x] **M0 — 项目骨架**：Tauri 2.0 跑通，设计文档定稿
- [x] **M1½ — 核心移植**：pi 的 agent loop / 工具 / provider / session 移植为 Rust（现为 `pipi-agent` / `pipi-core` 等 crate），命令权限与工作目录进 `agent.json`
- [x] **M2 — Agent 管理**：UI 全字段编辑 `agent.json` / `harness/AGENTS.md` 双向同步，memory 渐进召回（索引常驻 + 按需读取）
- [ ] **M3 — Skills 与 MCP**：Skills 渐进注入已实现；技能库与 Agent 技能页已接线（安装 / 校验 / 启用 / 同步）；stdio MCP 服务器接入未动
- [x] **M4（部分）— 会话树状分叉**：`fork_session` 已实现并接线 UI；LLM 摘要式 compaction 已实现（turn 边界触发，摘要落盘可回放）。模板市场、多语言未动
- [x] **M5 — 会话进 git**：会话载体从 JSONL 换成 git 分支（一轮一个 commit，`git log` 即对话、压缩＝新建分支、`git bundle` 可分发）；旧会话照旧可读，Agent 工作台「文件」页可一键导入（逐条校验、幂等、原文件保留）

## 致谢

- [pi](https://github.com/earendil-works/pi) —— 本项目大量设计灵感来自它：一切皆文件、小核心、渐进式上下文、会话即树。如果 Pipi 的方向让你兴奋，请先给它一个 star。
- [openai/codex](https://github.com/openai/codex) —— 命令安全评估与沙箱模式移植自它（Apache-2.0）。

## License

MIT（`crates/pipi-tools/src/permissions/safety.rs` 移植自 Apache-2.0 项目 openai/codex，保留其许可声明）。
