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
~/.pipi/agents/my-agent/
├── agent.json          # Agent 清单：模型、工作目录、MCP 服务器等
├── AGENTS.md           # 系统级指令，每次运行注入上下文
├── skills/             # 技能包（SKILL.md + 随附文件）
│   └── git-safety/
│       └── SKILL.md
├── memory/             # 持久记忆（Markdown，人机共写）
│   └── user-preferences.md
├── workspace/          # Agent 目录内的默认工作区（也可指向任意本地路径）
└── sessions/           # 运行记录（JSONL，append-only）
```

- 没有数据库、没有私有格式、没有锁定。
- 任何编辑器都能改，git 就是版本管理，网盘就是同步。
- Pipi 的 UI 只是这些文件之上的一个**视图和执行器**。

### 3. 小核心，组合优于配置

核心只内置最小工具集（`read` / `write` / `edit` / `bash` / `memory`），其余能力全部来自组合：

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
| 记忆 | `memory/*.md` | 由 `memory` 工具读写的持久记忆，跨会话生效 |
| 命令权限 | `agent.json` → `permissions.bash` | bash 白名单 / 黑名单；引号感知的复合命令逐段检查 |
| 沙箱 | `agent.json` → `permissions.sandbox` | `read-only` / `workspace-write` / `danger-full-access`（移植自 codex）：强制删除类命令、越出工作目录的写入与重定向在非完全访问下被拒绝 |
| 工具开关 | `agent.json` → `permissions.tools` | 内置工具（read/write/edit/bash/memory）按需启用 |
| MCP | `agent.json` → `mcpServers` | Stdio MCP 服务器，会话启动时按需拉起（M3） |
| 会话 | `sessions/*.jsonl` | Append-only 的运行记录，一文件一会话，树状条目（id/parentId）支持分叉 |

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
│  ├─ tools         read/write/edit/   │  最小内置集，限定于 workspace
│  │                bash/memory        │
│  ├─ provider      anthropic +        │  流式 SSE，事件驱动
│  │                openai-compat      │
│  ├─ session       sessions/*.jsonl   │  append-only，崩溃安全
│  ├─ permissions   bash 白/黑名单      │
│  └─ agents        扫描 ~/.pipi/agents │  一切皆文件
└──────────────────────────────────────┘
```

### 与 pi 的关系

核心从 [pi](https://github.com/earendil-works/pi) 移植为 Rust，模块映射：

| pi | pipi-core | 备注 |
| --- | --- | --- |
| `packages/ai` types | `types` | 消息/内容块/事件协议，JSON 字段名与上游一致 |
| `packages/ai` api adapters | `provider` | 只移植 anthropic-messages、openai-completions 两个 |
| `packages/agent` agent-loop | `agent_loop` | 事件流 + steering/follow-up + 工具批次执行 |
| `packages/agent` harness/tools | `tools` | read/write/edit/bash + 新增 memory |
| `packages/agent` harness/utils/truncate | `truncate` | 2000 行 / 50KB，同一套提示文案 |
| `packages/agent` harness/session | `session` | 树状 JSONL Entry（id/parentId/seq） |

有意推迟移植（需要时再从上游搬）：compaction、hooks 全集、transformContext/
prepareNextTurn、其余 provider、图片工具。Pipi 自己新增：`permissions`
（命令权限）、`agents`（Agent 注册表）、memory 工具。

### 与 codex 的关系

命令安全与沙箱概念移植自 [openai/codex](https://github.com/openai/codex)
（Apache-2.0，见 `pipi-core/src/permissions/safety.rs` 的 attribution）：

- `SandboxMode`（read-only / workspace-write / danger-full-access）→
  `permissions::SandboxMode`，语义一致（kebab-case 序列化兼容）。
- `is_dangerous_command`（rm -f 家族 + sudo/env/trap/bash-c 包装器解包 +
  深度上限 fail-closed）→ `permissions::safety`。上游用 tree-sitter 解析
  `bash -c` 脚本，我们不引入该依赖：脚本含语法关键字/命令替换时按危险
  处理（fail-closed）。
- 尚未移植：OS 级沙箱（Landlock/Seatbelt）—— Pipi 当前是用户态粗粒度
  闸门（白/黑名单 + 危险启发式 + 重定向/写入路径约束），真正的强隔离
  列入 M3 后的路线。

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

## 路线图

- [x] **M0 — 项目骨架**：Tauri 2.0 跑通，设计文档定稿
- [x] **M1½ — 核心移植**：pi 的 agent loop / 工具 / provider / session 移植为 Rust（`crates/pipi-core`），命令权限与工作目录进 `agent.json`，38 个核心测试
- [ ] **M1 — 最小闭环**：会话 UI + 流式对话（接通 agent_loop 与前端事件）+ provider 配置 UI
- [ ] **M2 — Agent 管理**：UI 编辑 `agent.json` / `AGENTS.md` 双向同步，memory 渐进召回
- [ ] **M3 — Skills 与 MCP**：加载技能包（渐进式注入）、接入 stdio MCP 服务器
- [ ] **M4 — 打磨**：会话树状分叉、Agent 模板市场（本地文件分发）、多语言

## 致谢

- [pi](https://github.com/earendil-works/pi) —— 本项目大量设计灵感来自它：一切皆文件、小核心、渐进式上下文、会话即树。如果 Pipi 的方向让你兴奋，请先给它一个 star。
- [openai/codex](https://github.com/openai/codex) —— 命令安全评估与沙箱模式移植自它（Apache-2.0）。

## License

MIT（`pipi-core/src/permissions/safety.rs` 移植自 Apache-2.0 项目 openai/codex，保留其许可声明）。
