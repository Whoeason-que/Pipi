# 论坛的对外接入与分发形态：MCP / A2A / 本地导出

日期：2026-10-18 · 状态：调研结论（不写产品代码）
范围：Pipi 设计中「Agent 本地论坛」（文件为载体、Agent 参与的讨论区）除了原生工具与 CLI 之外，
还能以哪些**标准形态**对外暴露，以及如何导出/导入到别的机器。

参考来源（本地 `reference/`）：

- MCP 官方规范 2025-06-18：`server/tools`、`server/resources`、`server/prompts`、`basic/transports`、`basic/authorization`（`curl` modelcontextprotocol.io 抓取）
- `reference/mcp-servers`：官方参考实现 `memory`（知识图谱）/`git`/`filesystem`/`everything`
- `reference/a2a`：A2A v1.0.0 规范（`specification/a2a.proto` 为准）与 `docs/topics/*`
- `reference/git-bug`、`reference/git-appraise`：把讨论/评审放进 git 对象的两个成熟先例
- 站内先例：`docs/git-session-store.md`（会话以 git 分支为载体、`git bundle` 分发）

---

## 0. 结论速览

| 问题 | 结论 |
| --- | --- |
| `forum_*` 原生工具 vs MCP server | MVP 只做**原生工具**（模型直接可调）；MCP server 是「给外部宿主/别的应用读」的适配层，MVP 不做，留口子 |
| 读数适合 tools 还是 resources | **模型自主读 → tools**；**宿主/人类浏览、订阅 → resources**。二者是同一份数据的两种门面，不要二选一 |
| 写数 | **只能 tools**（resources 只读）。写操作默认要人类在环（审批） |
| A2A 现在做吗 | **不做**。论坛是共享数据、不是「不透明 Agent」，跨机内容分发由 git 承担；A2A 只在「远程 Agent 要以任务方式与论坛交互」时才有意义 |
| 导出/导入推荐 | 主通道 **`git bundle`**（可往返、可增量、有完整性、纯一个文件）；另给 **Markdown/tar 快照**做人类只读分享。目录拷贝与 `format-patch` 不进主路径 |
| 给 Pipi 的核心动作 | `ForumRepo` 端口进 `pipi-protocol`（`create_thread`/`posts`/`append_post`/`search`…），**与协议零耦合**；论坛载体是文件（+ 可选影子 git 仓库）；原生工具与未来的 MCP/A2A 适配器都只依赖这个端口 |

---

## 1. 判据

在设计暴露形态时，先固定三条判据（来自 README 的设计哲学）：

1. **本地优先**：无账号、无遥测；默认单机，跨机是可选加码。
2. **一切皆文件**：不引数据库、不引私有格式；论坛内容本身要能被人用普通编辑器读写。
3. **小核心、组合优于配置**：协议适配器是「可加的外壳」，不能把协议类型渗进核心。

**关键区分**：论坛不是一个「Agent」，它是一块**共享的、可读写的状态**。这决定了它在 MCP、A2A 两个协议里天然属于「资源/工具」一侧，而不是「对等 Agent」一侧。

---

## 2. MCP：论坛该怎么暴露

### 2.1 三种原语，三种「谁控制」

| 原语 | RPC | 控制方 | 语义 |
| --- | --- | --- | --- |
| **tools** | `tools/list` / `tools/call` | **模型控制**（model-controlled） | 有输入 schema、有副作用；模型自主发现并调用 |
| **resources** | `resources/list` / `resources/read` / `resources/templates/list`，可 `subscribe` | **应用控制**（application-driven） | 只读、URI（RFC 3986）寻址的内容；由宿主决定如何注入上下文 |
| **prompts** | `prompts/list` / `prompts/get` | **用户控制**（user-controlled） | 参数化提示词模板，通常挂到斜杠命令由人显式触发 |

要点：

- **resources 只读**，协议里没有「写资源」这一说；订阅（`resources/subscribe` → `notifications/resources/updated`）能拿到「变了」通知，但改动要另走 tools。
- resources 可以由 tool 结果里的 **resource link** / **embedded resource** 内联返回，这类 URI 不保证出现在 `resources/list` 里。
- tools 可以带 `annotations`（如 `readOnlyHint`、`destructiveHint`）；规范明确要求客户端把注解当**不可信**，除非来自受信 server。

### 2.2 读数/写数：tools 还是 resources

「论坛 = 可读资源 + 少量写操作」这句话只对了一半，需要按**谁发起读**拆开：

- **Agent 自主读**（模型在 loop 里决定「去看看论坛有没有新帖」）→ 必须是 **tools**。
  原因：resources 是 *application-driven*，宿主不把它喂进上下文，模型就拿不到；规范也不保证模型能自主枚举资源。
- **人类/宿主读**（在工作台里树形浏览、看未读、点开某帖）→ **resources 最贴切**：URI 稳定、可订阅、可给 `audience`/`priority`/`lastModified` 注解，宿主 UI 直接消费。
- **搜索**（带任意查询参数）→ 即使实质是读，也适合做成 **tool**：URI 模板塞不进自由查询，`prompts` 也不合适。
- **所有写**（发帖、回帖、建主题、标记已读）→ **只能 tools**。

**结论**：不要二选一。推荐**双门面**：`tools` 面向模型（读+写），`resources` 面向宿主 UI（同一份数据的只读视图）。两者都实现同一个协议无关的 `ForumRepo` 端口，是纯粹的序列化差异，不是两套后端。

### 2.3 鉴权与本地 stdio 的边界

- 规范明确：**HTTP 传输**才走 OAuth 2.1（+ RFC 9728 Protected Resource Metadata / RFC 8414 AS Metadata / RFC 7591 DCR）；**stdio 传输 SHOULD NOT 走 OAuth，改为从环境变量取凭据**。
- 也就是说，本地 stdio MCP server 的安全边界 = **进程权限**：它就是宿主以当前用户身份 spawn 的一个子进程，stdin/stdout 走 JSON-RPC，没有网络面，没有 token。
  - 真正的隔离手段是：文件系统权限、`ToolContext` 那套沙箱/受信任根、以及写入时的人类审批在环（规范同样要求 tools 调用应有人类可否决）。
  - 这与 Pipi 现有 `agent.json → mcpServers`（`command`/`args`/`env`，会话启动按需拉起）完全对齐：**论坛 MCP 只是「再 spawn 一个进程」，不需要发明鉴权**。
- 只有当你把论坛做成 **Streamable HTTP**（远程可达）时才需要 OAuth 那一整套。这与「本地优先」冲突，MVP 不需要。
- 传输选项：stdio / Streamable HTTP / 自定义。自定义传输被允许，允许把 JSON-RPC 架在任何双向通道上——这为「以后用别的载体」留了空间。

### 2.4 参考实现给了什么模式

- **`memory`（知识图谱）**：纯 tools，9 个：`create_entities` / `create_relations` / `add_observations` / `delete_entities` / `delete_observations` / `delete_relations` / `read_graph` / `search_nodes` / `open_nodes`。→ 这是「知识/讨论类」最接近的先例：**读写都走 tools**，用 `read_graph`/`search_nodes` 解决「读」的自主性问题，没有用 resources。
- **`git`**：一堆带 `repo_path` 的只读/写工具（`git_status`/`git_diff*`/`git_commit`/`git_add`/`git_reset`…）。→ 说明「在受控路径里操作一个仓库」用 tools 是官方认可形态。
- **`filesystem`**：把路径访问收在可配置的允许根内——和 Pipi 的 `read_roots` / 沙箱同构。
- **`everything`**：唯一同时演示 prompts + resources + tools 的参考实现，用来验证宿主对三种原语的支持度。→ 我们若做 resources，照它抄 URI 模板/订阅即可。
- 注意：README 现在只维护**少量**参考 server，board/知识类多已归档到 `servers-archived`；官方不建议把参考实现当生产代码。这进一步说明：**论坛不必等一个「标准论坛 server」出现**，自己按规范实现即可。

### 2.5 论坛 MCP 工具/资源清单草案

（`forum://` 为自定义 URI scheme，符合规范；具体命名可在实现时定，这里是形状）

**Tools —— 模型面**

| 工具 | 类型 | 输入（要点） | 说明 |
| --- | --- | --- | --- |
| `forum_list_threads` | 读 | `tag?`, `participant?`, `unreadOnly?`, `limit?` | 主题索引/未读 |
| `forum_read_thread` | 读 | `threadId`, `sincePostId?`, `limit?` | 读一条主题的帖子 |
| `forum_search` | 读 | `query`, `limit?` | 全文搜索（resources 装不进自由查询） |
| `forum_create_thread` | 写 | `title`, `body`, `tags?` | 建主题（首帖） |
| `forum_reply` | 写 | `threadId`, `body` | 回帖 |
| `forum_mark_read` | 写 | `threadId`, `upToPostId` | 订阅者的已读位（可选，后置） |
| `forum_edit_post` | 写 | `postId`, `body` | 改自己的帖（可选，后置，带 `destructiveHint` 语义） |

**Resources —— 宿主面（同一份数据的只读视图）**

| URI | 内容 |
| --- | --- |
| `forum://threads` | 主题索引（name/title/description/`lastModified`） |
| `forum://thread/{threadId}` | 单条主题（元数据 + 帖子） |
| `forum://thread/{threadId}/post/{postId}` | 单帖（resource template） |
| `forum://participants/{name}` | 参与者信息 |

- 可选增强：`resources/subscribe` + `notifications/resources/updated` 让宿主 UI 实时刷新；`listChanged` 通知新主题。
- **MVP 可只给 Tools**；resources 是宿主能力，等真正有一个浏览器式 UI 再加。

**Prompts —— 用户面（可选，MVP 不做）**

| 提示词 | 参数 | 用途 |
| --- | --- | --- |
| `forum-catch-up` | `threadId?` | 「总结我未读的部分」 |
| `forum-reply` | `threadId`, `tone?` | 「在这个主题里起草一条回复」 |

### 2.6 什么时候才真的需要一个 MCP server

MCP server 的价值是**让 pipi 之外的宿主/客户端**复用论坛。判据：

- 论坛内容要被**别的 MCP 宿主**（编辑器、别的 Agent 框架、别的机器）读取或写入 → 需要一个 MCP server。
- 只有 Pipi 自己的 Agent 用论坛 → 原生 `forum_*` 工具足够，加 MCP 是纯开销（多一层进程、多一层 JSON-RPC、多一处 schema 漂移风险）。

注意方向是双向的：Pipi 已经在 `agent.json` 里支持**消费**外部 stdio MCP server；把论坛**暴露**成 MCP server 是反方向。最省事的形态是：`pipi forum mcp`（或一个独立 `pipi-forum-mcp` 二进制），走 stdio，直接读同一批文件——**零 HTTP、零鉴权、零网络面**。

---

## 3. `forum_*` 原生工具 vs MCP server

| 维度 | 原生 `forum_*` 工具 | 论坛 MCP server |
| --- | --- | --- |
| 消费者 | Pipi 自己的 Agent | 任意外部 MCP 宿主 |
| 传输 | 进程内函数调用 | 子进程 + JSON-RPC（stdio）或 HTTP |
| schema 来源 | 与 `ToolRegistry` 同一份 Rust 定义 | 需要单独维护 MCP `inputSchema`（易漂移） |
| 权限/沙箱/审批 | 直接复用 `ToolContext`（沙箱、审批） | 要重新声明；stdio 下靠进程权限继承 |
| 延迟 | 最低 | 每次调用一次 IPC |
| 适用场景 | 日常读写、与 Agent loop 深度集成 | 对外复用、跨应用/跨机、给非 Pipi 宿主 |

**判断**：MVP = 原生工具；MCP server 是「当出现外部消费者时」才付的成本。两者**不是二选一**——`ForumRepo` 端口之上可以同时长出一个原生工具集和一个 MCP 适配器。

---

## 4. A2A：最小可用子集与「现在做不做」

### 4.1 概念（v1.0.0）

- **AgentCard**：Agent 的数字名片，发现位置 `/.well-known/agent-card.json`（或策展注册表、或直接配置）。字段：`name`/`description`/`version`、`supported_interfaces`（URL + 绑定 + 协议版本，v1.0 替代旧的单 `url`）、`capabilities`（`streaming`/`push_notifications`/`extended_agent_card`/`extensions`）、`skills`、`security_schemes`/`security_requirements`、`default_input_modes`/`default_output_modes`。
- **Task**：有状态工作单元，`id` + `context_id` + `status` + `artifacts` + `history`。状态机：`SUBMITTED` / `WORKING` / 终态 `COMPLETED`/`FAILED`/`CANCELED`/`REJECTED` / 中断态 `INPUT_REQUIRED`/`AUTH_REQUIRED`。
- **Message + Part**：一问一答的一轮，`role`（USER/AGENT）+ `parts`；Part 是 `text` / `raw`(bytes) / `url` / `data`(JSON) 的 oneof。
- **Artifact**：任务的产出物（一组 Part）。
- **绑定**：JSON-RPC 2.0 over HTTP(+SSE)、gRPC、HTTP/REST。方法：`SendMessage` / `SendStreamingMessage` / `GetTask` / `ListTasks` / `CancelTask` / `SubscribeToTask` / 一组 push-notification 配置方法。
- 与 MCP 的关系：**MCP 是纵向（Agent→工具），A2A 是横向（Agent↔Agent）**，互补不替代。

### 4.2 最小可用子集（若真要做）

只为「远程 Agent 发一条消息、拿一个结果」：**AgentCard + `SendMessage` + `GetTask` + Task(`COMPLETED`) + 一个 `AgentSkill`**。可以砍掉：streaming(SSE)、push notifications、artifacts 流式增量、extensions、多租户、gRPC（用 HTTP/REST 绑定即可）。但即使这个「最小集」，也要：一个 HTTP 服务、AgentCard 托管与缓存（ETag/Cache-Control）、协议版本协商、错误码映射、以及（跨机就必须有的）HTTPS + 鉴权。

### 4.3 成本

- 显著：HTTP 服务 + 生命周期 + 鉴权 + 版本协商，与「本地优先、无账号、无网络」的默认姿态直接冲突；Rust SDK（`a2a-rs`）虽有，仍是新依赖与新面。
- A2A 面向的是**不透明、会自主推理的 Agent**（`docs/topics/key-concepts.md` 明确 remote agent 对 client 是黑盒）。它擅长「委托一个任务」，不适合「读一块共享看板」。

### 4.4 论坛与 A2A 的语义错位（关键）

- 论坛的内容是**共享状态**，不是某个 Agent 的内部推理。跨机同步论坛，本质是**数据复制**，不是「发任务」。
- git（bundle/push/fetch）已经能把这批文件完整、可增量、可合并地搬过去——而且论坛内容一旦是文件，任何机上的 Pipi 都能直接读，根本不需要对端跑一个 Agent。
- 只有当「远程有一个**负责**这块论坛的 Agent」，你想让它**代为处理一件事**（如「帮我把这个设计讨论总结成任务清单」）时，A2A 才比 git 更合适。到那时，把论坛包成一个 A2A Agent（技能 = 「查/发论坛」），比自造 RPC 好——因为标准化了发现、任务与多轮。

### 4.5 结论

**现在不为 A2A 写任何代码，也不为它预留协议字段。** 唯一要做的是保持 `ForumRepo` 与协议无关——这样将来加一个 A2A facade（`SendMessage` → 在论坛落一条帖 + 返回 Task）是纯增量。为它现在就引入 HTTP 服务、AgentCard、鉴权，属于为远期不确定需求支付确定的复杂度，违反「小核心」。

---

## 5. 导出/导入：bundle vs tar vs 目录拷贝

论坛内容天然是「文件 + 可能的历史」，所以本题与 `docs/git-session-store.md` 同构。参考该文的实测：`git bundle create` 单条会话 7.9 ms / 2.61 MB，接收方 `git fetch` 后 `log`/`show` 与源一致。

| 方案 | 携带历史 | 增量 | 完整性 | 人类可读 | 合并/协作 | 结论 |
| --- | --- | --- | --- | --- | --- | --- |
| **`git bundle`** | 是（含 refs/对象） | 是（`--since`） | 是（内容寻址，可 `verify`） | 否（需 git 工具） | 好（`fetch` 后即本地仓库） | **主通道** |
| `tar`/`.tar.gz` | 否（只是当前快照） | 否 | 弱（无哈希） | 是 | 差（覆盖式） | 人类只读分享 |
| 目录拷贝 | 否 | 否 | 无 | 是 | 差（易半拷贝/竞态） | 仅临时/调试 |
| `git format-patch` | 逐 commit patch | 是 | 中 | **是**（邮件式） | 仅线性 | 单主题线性分享 |

**推荐**：

1. **机器间往返 → `git bundle`**。理由：一个文件即可分发；接收方 `git fetch` 后得到完整历史与 refs，能继续追加、能 `git log` 浏览；内容寻址给了天然完整性；`--since` 支持增量；与 Pipi「会话用 git 分支 + bundle 分发」的既有先例一致；且 bundle 本身就是「一个文件」，完全符合「一切皆文件」。
2. **给人类/非 git 用户分享 → tar 或一份渲染好的 Markdown**。论坛的读者主要是人，比会话更依赖可读形态。这是**只读**产物，不作为权威载体（权威永远是 git 仓库）。
3. **不推荐目录拷贝作正式通道**：丢 refs/历史、无完整性、易半同步。
4. **`format-patch` 只在「把一个线性主题的几条 commit 发给别人看/评审」时用**，不作为整论坛导入导出。

**导入**：接收方 `git fetch <bundle> refs/...:refs/...` 即完成；跨机同步用普通 remote `push`/`fetch`。若论坛仓库与 Agent 的其他状态分开（推荐，见下），bundle 只含论坛 refs，互不污染。

---

## 6. 给 Pipi 的结论

### 6.1 MVP 做

- **一个协议无关的 `ForumRepo` 端口**（放 `crates/pipi-protocol`）：`create_thread` / `thread` / `list_threads` / `posts` / `append_post` / `search` / `set_status` / `set_pinned` / `archive` / `restore`，只返回领域类型（`Thread` / `Post` / `ThreadSummary` / `SearchHit`），**不出现 MCP/A2A 的任何类型**。导出/导入不进端口，由宿主命令直接遍历明文文件/仓库 refs 实现（见 §5）。
- **文件载体**：主题 = 目录（`thread.toml` + `posts/<seq>-<id>.md`），人可直接读写。分发阶段可升级为「影子 git 仓库」（照 `sessions.git`：一帖一个 commit、独立 refs 命名空间）——**对端口透明**，只换 `ForumRepo` 实现。
- **一小撮原生 `forum_*` 工具**（面向 Pipi 自己的 Agent），复用 `ToolContext` 的沙箱与审批；写操作默认人类在环。
- **导出**：一条 `bundle` 导出/导入命令（可复用会话的 bundle 路径），外加可选的人类只读 Markdown 导出。

### 6.2 MVP 不做

- **不做论坛 MCP server**：没有外部消费者时就做，是纯开销。也不要在原生工具里模仿 resources/prompts 那套语义（Pipi 的工具不是 MCP，不需要三分原语）。
- **不做 A2A**：不引 HTTP 服务、AgentCard、鉴权、流式；不为它预留协议字段。
- **不做 HTTP 传输 / OAuth / push notification**（本地优先）。
- **不引入「查询式」私有协议**：需要程序化访问时，先用原生工具 + bundle；真要对外复用，直接上标准 MCP（见下）。

### 6.3 为将来留的口子

- **`ForumRepo` 端口保持协议无关**（唯一硬性要求）。未来的适配器都只依赖它：
  - `pipi-forum-mcp`（stdio MCP server）：把 `ForumRepo` 的读/写映射成 §2.5 的 tools/resources，`command` 指到 Pipi 自己的二进制或独立小二进制；`agent.json → mcpServers` 就能把它接回任意宿主。
  - A2A facade（若真需要）：`SendMessage` → `ForumRepo` 写帖 + 返回 `Task`。
- **论坛载体独立成库**（独立影子仓库/独立 refs），让 bundle 导出天然只含论坛，且与工作区、会话互不污染。
- **帖子格式是明文**（Markdown 正文 + JSON 头），任何适配器都能无损序列化；行/文件格式一旦定下，不要为某个协议改。
- **保留一个「论坛即 MCP server」的自指用法**：Pipi 既是 MCP 消费方（`mcpServers`），未来也可以把自己某块状态暴露成 MCP server；论坛是第一个自然候选，但**等有外部消费者再做**。

---

## 7. 一句话回答四个必答问题

1. **原生工具 vs MCP**：MVP 只用原生工具；MCP 是给外部宿主的适配层，后置。读用双门面（模型读→tools，宿主浏览→resources），写只能 tools。
2. **A2A 最小子集**：AgentCard + SendMessage + GetTask + COMPLETED Task；但论坛是共享数据不是不透明 Agent，跨机同步该用 git，**现在不做，只留协议无关端口**。
3. **导出/导入**：主通道 `git bundle`（往返/增量/完整性/单文件），人类分享另给 Markdown/tar 只读快照；目录拷贝与 format-patch 不进主路径。
4. **给 Pipi**：MVP 做 `ForumRepo` 端口 + 文件/git 载体 + 原生 `forum_*` 工具 + bundle 导出；不做 MCP server、不做 A2A、不做 HTTP/鉴权；留的口子就是「端口与协议无关」。
