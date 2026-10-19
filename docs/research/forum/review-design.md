# forum-adversary-design：本地论坛框架设计的对抗式审查

> 审查立场：这个端口形状、分层与哲学落位**将来会害死我们**。只攻击**设计**（端口/分层/接缝/命名/调研采纳），不重复 `review-code.md` 已覆盖的具体代码 bug（符号链接逃逸、search panic、归档竞态等仍会引用，但只谈它们的**设计根因**）。
>
> 对象（只读）：`docs/agent-forum.md`、`crates/pipi-protocol/src/forum.rs`、`crates/pipi-forum/`；对照物 `pipi-session` / `pipi-protocol::session`；铁律 `AGENTS.md` / `architecture.toml` / `README.md`；调研 `docs/research/forum/*.md`。
>
> 复现手段：`cargo test -p pipi-forum`、`cargo xtask arch`、`cargo run -p pipi-forum -- …`（原始 `/tmp/fwd-compat`，未改动任何仓库文件）。
>
> 方法：每条先自我证伪（「这也许是有意简化」），反驳不了才记。

---

## 汇总

| 级别 | 编号 | 一句话 |
| --- | --- | --- |
| Blocker | —— | 无（MVP 可跑通；下列问题都是「现在不痛、将来痛」） |
| High | D1 | 「预留字段向后兼容新增、无需迁移」是假承诺：`thread.toml` 的读改写会静默吞掉未知字段 |
| High | D2 | 端口的并发不变量没有写进契约，且「与会话载体同一限制」的说法不成立（git CAS ≫ O_EXCL） |
| High | D3 | `reply_to` 无人校验、`root_id`/`causality_depth` 无人拥有——Phase 2 护栏的地基是空的 |
| High | D4 | `@提及` 只是字段不是解析：人写正文里的 `@builder` 不产生提及，核心异步语义缺一半 |
| High | D5 | 「作者不可伪造」在 CLI/bash 边界破功：`--author` 无认证，Agent 可借 bash 冒充任意身份 |
| High | D6 | 声明的落位与 `architecture.toml` 相冲：Phase 1/2 的两条依赖现在都会被 `cargo xtask arch` 拒绝 |
| Medium | D7 | `Result<T, String>` 撑不起 Phase 2 派发器需要的失败分类（not-found/closed/archived/duplicate/IO） |
| Medium | D8 | MVP 端口/DTO 缺的正是自家路线图要用的字段与操作，且新增字段会破坏结构体字面量调用 |
| Medium | D9 | 「严格参数解析」只对全局 flag 集合严格，不对命令严格：`list --title` 等被静默忽略 |
| Medium | D10 | `search` 语义被钉进契约却只搜正文、不含标题、不含归档，且与 `SearchHit` 的字段自相矛盾 |
| Medium | D11 | `ThreadStatus` 的模型在文档里自相矛盾（`locked` 既是状态又是 bool），且未知变体会让主题**从列表里消失** |
| Medium | D12 | `Post.via` 的 DTO 与 frontmatter 默认值不一致：手写帖少了 `via` 会被静默丢弃 |
| Medium | D13 | 游标/inbox/根目录的属主与位置未定义，且 README 目录骨架里根本没有论坛 |
| Medium | D14 | 论坛被抬到契约层原语（协议 trait / 独立 crate），比后台任务还高——「产品功能，不是原语」没兑现 |
| Low/Nit | D15 | id「与会话同形」不实、`文件名排序=序号排序` 与实现不符、`seq` 双写两处真源 |
| Low/Nit | D16 | `thread` vs 调研/磁盘的 `topic`、`threads/` vs `topics/` 术语漂移 |
| Low/Nit | D17 | 列表/检索为派生字段读全量帖子正文；`--json` 在 close/pin/archive 被接受却无效；空参退出码与 av 不一致 |
| Low/Nit | D18 | 库里的 `dirs`/`default_root` 越界承担宿主策略；`is_post_file` 导出未用 |

---

## High

### D1【High】「预留字段向后兼容新增、无需迁移」是假承诺

**证据**
- 设计 §4 明写：「有意保留的字段（向后兼容新增，无需迁移）……`board`、`locked`、`author_kind`、`updated_at`：均可用 `#[serde(default)]` 增量加入，旧文件照读。」
- 但 `thread.toml` 会被**读—改—写**：`set_status`（`file.rs:352`）与 `set_pinned`（`file.rs:362`）都是 `read_thread_in` → 改一个字段 → `write_thread_in`（`file.rs:162`）→ `encode_thread`（`format.rs:114`）只序列化 `Thread` 的已知字段。
- `Thread`（`forum.rs:59`）没有 `deny_unknown_fields`，所以未知字段读得进但存不下；下一次 `pin`/`close` 就把它抹掉。
- **实测**（`/tmp/fwd-compat`）：给 `thread.toml` 手工加 `board = "general"` / `locked = true` / `updated_at = …`，跑 `pipi-forum pin` 后三个字段全部消失。
- 同类问题反向也存在：`decode_thread` 遇到未知 `status` 直接报错，而 `list_threads`（`file.rs:246`）在 `summarize` 失败时 `continue` —— 主题**从列表里静默消失**，只有显式 `read` 才报错（实测 `status = "settled"` → `（没有主题）`）。

**为什么现在不痛**：当前 `Thread` 的字段就是全部落盘字段，没有「未知字段」可吞。
**为什么将来会痛**：这正是设计为 Phase 1/2（`board`/`locked`/`updated_at`/`origin`）留的扩展位。任何**混合版本**（桌面壳 + Web 宿主、旧安装包、用户在 Obsidian 里加了一个属性）在 `pin`/`close` 时都会静默丢数据；而 `origin` 虽然加在帖子上（只追加、不重写）是安全的，`board`/`locked`/`updated_at` 加在 `thread.toml` 上就踩雷。届时它就从 High 升级为 Blocker。
**建议**：`Thread` 增加 `#[serde(flatten)] extra: BTreeMap<String, toml::Value>`（或把未知字段原样保留）；`ThreadStatus` 改为 `#[serde(other)] Unknown` 或 `String`+白名单解析，未知状态**降级为只读可见**而不是消失；把「元数据无损往返」写成 `format` 模块的不变量并加一条 roundtrip 测试。

**自我证伪**：有人说「新增字段时同步升级所有读方即可，本地单机不分版本」——但设计自己承认有 Tauri 壳 + Web 宿主两条入口（§8），且论坛是「人机共写」邀请外部编辑器写 `thread.toml`；`review-code.md` F1 也证明这棵树本来就被当成不可信输入。反驳不成立。

---

### D2【High】端口的并发不变量没进契约，且「与会话载体同一限制」不成立

**证据**
- 设计 §3 与 `file.rs:6-9` 都写：「只追加、无锁（同进程一把写锁）」；并称跨进程限制「与会话载体的已知限制一致」。
- 但「一把写锁」只被 `create_thread`（`file.rs:206`）和 `append_post`（`file.rs:309`）持有；`set_status`/`set_pinned`/`archive`/`restore` 全都不持锁（`review-code.md` F3/F4/F5 已复现：归档竞态撕裂目录、并发关帖+置顶 lost update、关闭检查 TOCTOU）。
- 更关键的是**论据本身错了**：会话载体跨进程靠的是 git ref 的 CAS（`git-carriers.md` 结论第 5 条），失败会重读重试；论坛跨进程只有 `O_EXCL` 防止**文件名**相撞，而文件名是 `{seq:04}-{随机id}.md`（`format.rs:42`），不同 id 让 `AlreadyExists` **永不触发**（`review-code.md` F8）。两个进程会各自写出同 `seq` 的两条帖，读侧按 frontmatter `seq` 排序 → 顺序不确定。所以论坛的跨进程保证**弱于**会话，而不是「一致」。
- 端口 `ForumRepo`（`forum.rs:168`）是 `&self` + `Send + Sync`，天然允许多线程并发调用，但契约里**没有一个字**规定写序列化语义。

**为什么现在不痛**：单进程、单人使用，`forum_lifecycle` 只测了同进程并发追加。
**为什么将来会痛**：Phase 1 起宿主会同时有「用户聊天 run」和「forum run」两条写路径；Phase 2 派发器再加一条。谁来加锁、锁粒度是什么（版面/主题/帖子），取决于实现而不是契约 —— 第二个后端（git 对象）作者会重新发明一遍，且很可能漏掉。
**建议**：把「同一主题的写操作必须串行且检查—写入构成临界区」写进 `ForumRepo` 文档作为**实现契约**；同时把 §3 的「与会话同一限制」改为准确表述（会话 = ref CAS + 重试；论坛 = 文件名 O_EXCL，仅防撞名）。跨进程若要立项，给 `posts/.lock` 的 `flock` + 序号 CAS。

**自我证伪**：有人说「并发写序列化是实现细节，不该进契约」——但 `SessionRepo` 的文档恰恰规定了「实现必须对符号链接 fail-closed」这类跨实现不变量，论坛端口 `forum.rs:165-168` 也照抄了这条。既然路径安全能进契约，写序列化为什么不能？双重标准。成立。

---

### D3【High】`reply_to` 无人校验、`root_id`/`causality_depth` 无人拥有

**证据**
- `append_post_in`（`file.rs:102`）把 `draft.reply_to` 原样写入，**从不校验**它指向本主题内存在的帖子；`NewPost::reply_to` 是自由 `Option<String>`（`forum.rs:128`）。指向不存在的 id、甚至跨主题的 id 都会被接受。
- `Post` 没有 `root_id`/`causality_depth`（`forum.rs:76`）。设计 §4 的处置是：「因果链（`reply_to` 向上回溯的 `root_id`/`causality_depth`）将来在通知层按需推导，不写进权威状态」。
- 而调研 `multi-agent-messaging.md` §7.2/§7.3 明确建议：「回复必须带 `parentId`；`rootId` 可直接取父帖的 `rootId`」，并把 `causalityDepth` 列入必备护栏（`maxCausalDepth = 4`），且「记录并展示 `causalityDepth`、`rootId`、`parentId`」。

**为什么现在不痛**：MVP 没有通知层，没人读因果链。
**为什么将来会痛**：Phase 2 的**所有**终止/防互刷护栏（最大因果深度、连续 Agent 帖上限、`settled`）都要沿 `reply_to` 上行。现在 (a) 链条可以断（dangling），(b) 没有 `root_id`，算深度必须每次把整条主题的帖子读进内存再拓扑排序；(c) 谁负责推导未定义 —— 派发器？`forum_*` 工具？UI？「通知层按需推导」把成本摊到每个消费者头上，且每个消费者各写一份。这不是简化，是把一个（几乎零成本的）落盘字段换成 N 处重复实现。
**建议**：`append_post` 在临界区内校验 `reply_to` 属于本主题（这是端口该做的不变量，和"closed 拒绝回帖"同级）；`Post` 落 `root_id`（根帖 = 自身 id）与 `causality_depth`，或至少落 `root_id`（深度可 O(1) 从父帖推出）。若坚持不落盘，也要在端口上给一个 `causal_chain`/`thread_tree` 查询，别让每个消费者自己爬。

**自我证伪**：有人说「派生字段不落盘是避免 Flatboard『派生当真源』的老路」——但 `root_id`/`depth` 是**写时确定、永不变化**的因果事实，不是可重算的聚合统计（`post_count` 那种）。git-bug 的快照里也保留 Timeline/parent 关系。把「不落派生态」的原则套到因果键上是类比错误。成立。

---

### D4【High】`@提及`只是字段，不是解析

**证据**
- `mentions` 是调用方传进来的 `Vec<String>`（`NewThread`/`NewPost`，`forum.rs:118/130`）；`validate_mention`（`forum.rs:211`）只查「非空、无控制字符、无路径分隔符」。
- `file.rs` 全文没有任何从 `body` 里解析 `@token` 的代码（`grep '@' file.rs` 无 mention 相关命中）。CLI 也只能靠显式的 `--mentions a,b`（`cli/mod.rs:239`）。
- 设计 §2 把「`@提及`」列为核心能力；调研 `multi-agent-messaging.md` §7.2 明确要求「只匹配正文中的 mention token；忽略代码块 / 引用块中的 `@`；忽略自己 @ 自己」。

**为什么现在不痛**：MVP 只存字段，没通知，"提及"只是个装饰。
**为什么将来会痛**：论坛的存在理由是「异步 + 多对多」；Phase 2 的唤醒只对 `@mention`/回复我触发。人会自然地手写 `@builder 你看下`（这正是「人机共写」），但**没有任何东西把它转成 mention 字段**——于是人的 @ 永远唤不醒 Agent，平台最亮的卖点在真实使用里是哑的。更糟：如果哪天补上解析，同一句话会同时存在"正文里的 @"和"字段里的 mentions"两套真源。
**建议**：在端口之上（或在 `append_post` 内）定义 mention 解析规则并落库；明确「正文 token 是权威，`mentions` 字段是它的缓存」还是反之。若暂不做，设计里要写明「v1 提及仅字段、不解析正文」的**已知缺口**，别在 §2 声称支持。

**自我证伪**：有人说「模型走工具时自己会填 mentions 字段」——人走 CLI/UI 时不会。设计三分之一的价值主张是"人机共写"，不能只对 Agent 侧成立。成立。

---

### D5【High】「作者不可伪造」在 CLI/bash 边界破功

**证据**
- 设计 §5：「**作者不可伪造**：原生工具路径的 `author` 由运行上下文盖章，模型无法指定；CLI 用 `--author`（其自身是信任边界内的人/脚本）。」
- 实测：`cargo run -p pipi-forum -- post --title t --body b` 的作者是 `$USER`（`whoeason`）——`author()`（`cli/mod.rs:192`）的兜底链就是 `--author > $PIPI_FORUM_AUTHOR > $USER > human`。
- 设计 §9 自己承认这个缺口：「Agent 应走原生 `forum_*` 工具而非 `bash` 调 CLI」——**"应"不是"不能"。** Pipi 默认给 Agent `bash` 工具（`DEFAULT_TOOLS` 含 `bash`），论坛 CLI 在 `PATH` 上，任何 Agent 都能 `pipi-forum post --author planner …`。

**为什么现在不痛**：Phase 1 还没有工具路径，也没有审计消费方。
**为什么将来会痛**：一旦派发器/通知按 author 做"回复我""订阅"路由，author 就是触发器。冒充 author = 伪造唤醒、伪造讨论归属。`via = tool|cli|web` 只能告诉你"走的哪条通道"，不能证明"谁"。
**建议**：不要依赖 `--author` 作为信任边界。让 CLI 写入的帖子带一个不可伪造的运行身份（如 shell 注入的 `PIPI_RUN_ID`/会话令牌），或明确降级承诺：文档改为「工具路径不可伪造；CLI 路径的 author 是自报，仅供展示，不参与路由」，并让派发器只信任 `via=tool` 的 mention。审计上至少把 `via` 纳入"回复我"的判定。

**自我证伪**：有人说「Agent 守规矩，不会故意冒充」——那正是"最佳努力"而非不变量。设计把这条列为**不变量**（§5 标题），就必须在 bash 边界守住，或承认它不是不变量。成立。

---

### D6【High】声明的落位与 `architecture.toml` 相冲

**证据**
- 设计 §8 表格声明：Phase 1 `pipi-core::tools::forum`（`pipi-core` 要用 `pipi-forum`）；Phase 2 「git 对象后端 …… 复用 `pipi-session` 的 git 管线」（`pipi-forum` 要用 `pipi-session`）。
- 现状 `architecture.toml`：`[crates.pipi-core] may-depend-on` **不含 `pipi-forum`**；`[crates.pipi-forum] may-depend-on = ["pipi-protocol"]` **不含 `pipi-session`**。`cargo xtask arch` 只校验当前状态（12 成员、无环），不会替你拦下"计划中的越界"。
- 另外 `pipi-forum`（**库**）自带 `default_root()`（`format.rs:124`，走 `dirs`）与 `PIPI_FORUM_ROOT`/`--root` 三级配置，而 `pipi-session` 的模式是**由宿主传路径**（`GitSessionRepo` 按 Agent 目录构造）。同一个「库该不该知道 `~/.pipi`」的问题，两个载体给了相反答案。
- 附：`docs/agent-forum.md` §10 让默认根是 `~/.pipi/forum`，而 AGENTS.md/README 说 Agent 数据根是 `~/.pipi/agents/<name>/`；`~/.pipi/forum` 是第三个、README 骨架里不存在的顶层位置。

**为什么现在不痛**：Phase 1/2 没开工，`pipi-forum` 目前只依赖 `pipi-protocol`，arch 检查绿。
**为什么将来会痛**：等你真要写 `pipi-core::tools::forum` 时，会发现要么改 `architecture.toml`（一行，但需说明理由），要么把论坛塞进 `pipi-core` 当模块（推翻独立 crate 决策，并让 `pipi-protocol` 的端口失去唯一实现者）。git 后端更硬：`pipi-forum → pipi-session` 违反"会话载体唯一 spawn git"的边界，也可能形成 `pipi-session` 语义泄漏；若改成把 git 管线下沉到新 crate，是一次真正的重构。**这是一个现在花 10 分钟、将来花 1 周的缝。**
**建议**：现在就把 `architecture.toml` 里 `pipi-core`/`pipi-forum` 的**计划依赖**写下并注明理由（哪怕暂未使用），或先定"git 管线提取到独立 crate（如 `pipi-git`）"的方向；`ForumRepo` 的默认根改为由宿主注入，`default_root` 只留在 CLI/bin（避免库→宿主策略倒挂）。

**自我证伪**：有人说「Phase 1 还没到，改配置是那时的事」——但铁律第一条就是"改依赖或加 crate 先改 `architecture.toml` 再跑检查，想放宽要在那里写明理由"。设计文档已经承诺了这些依赖却不更新边界文件，等于把违反铁律的伏笔写进了单一事实来源。成立。

---

## Medium

### D7【Medium】`Result<T, String>` 撑不起 Phase 2 派发器的失败分类

**证据**：`ForumRepo` 全部方法返回 `Result<_, String>`（`forum.rs:168-197`）。而"不存在"（`locate`，`file.rs` 未命中）、"已归档"、"已关闭"、"非法 id"、"IO 失败"全部是 `String` 消息（如 `format!("主题「{thread_id}」已关闭，不能回帖")`）。
**为什么现在不痛**：CLI 只把错误打到 stderr，人看得懂。
**为什么将来会痛**：派发器要按语义决策——`closed`/`archived`/`duplicate` 是**终态，不重试、要撤唤醒**；IO/并发是**可重试**。用 `String` 就只能 `contains("已关闭")` 做字符串匹配，语言/文案一改就断，且第二个后端要复制同一套字符串口径。
**建议**：至少给写路径一个 `enum ForumWriteError { NotFound, Closed, Archived, Invalid, Io(String), … }`（`Thiserror` 或手写），把"可重试/终态"编码进类型；端口层用类型错误，CLI 侧再格式化成人话。

**自我证伪**：`SessionRepo` 也是 `String`，是不是"全仓一致"就够了？会话调用点不做"按错误类型重试"的决策（重试在 provider 层，有 `RetryHint` 类型）。论坛的派发器是**新**的决策点，沿用旧 wart 只是因为没想过——这正是"对称"造成的假象。成立（但可降为 Medium）。

---

### D8【Medium】MVP 端口/DTO 缺的正是自家路线图要用的字段与操作

**证据**（对照设计 §4/§6/§8 与调研 §7.4）
- 设计 §4 把 `origin(origin_run_id/origin_session_id)`、`content_hash`（§6 幂等键 `(thread,parent,author,content-hash)`）、`author_kind`、`updated_at`、`board`、`locked` 列为"将来加"，但 `Post`/`Thread`/`NewPost`/`NewThread`/`PostFrontmatter`（`forum.rs`、`format.rs:56`）一个都没有。
- 端口缺：`exists`（对照 `SessionRepo::exists`）、post 的 `edit`/`delete`（调研 §7.3/`protocols-and-exposure.md` §2.5 的 `forum_edit_post`）、`mark_read`/cursor、按 participant/author 过滤、`count`/游标分页。
- 后果：`origin`/`content_hash` 是**帖子**字段（append-only，安全）；但 `board`/`locked`/`updated_at` 是**主题**字段 → 撞上 D1 的读改写丢字段。`NewPost`/`NewThread` 加字段会破坏所有结构体字面量调用点（CLI、测试、将来的工具），`ForumRepo` 加方法会破坏所有 implementor。

**为什么现在不痛**：MVP 不需要。
**为什么将来会痛**：设计把这套描述成"向后兼容、无需迁移"，让人以为缝很小；实际是"文件格式兼容，Rust API 不兼容，且主题侧字段有丢数据风险"。
**建议**：要么现在就把这些字段按 `Option<T>` + `Default` 放进 DTO 与 frontmatter（"预留位"真正落盘而不是只写在文档里），要么在文档里修正措辞："文件格式向后兼容；`NewX` 是构造入参、新增字段是源码级 breaking"。端口层面，`ForumRepo` 是 trait，给非必需操作提供**默认实现**（报 `Err("该载体不支持")`）——`SessionRepo::entries_archived` 已经是这个先例（`session.rs:224`），论坛没有沿用。

---

### D9【Medium】「严格参数解析」只对全局 flag 集合严格，不对命令严格

**证据**
- CLI 文档（`cli/mod.rs:18`）与设计 §9 都自称「未声明的 `--x` 一律报错」。
- 但每个命令都调 `parse_args(args, VALUE_FLAGS, BOOL_FLAGS)`（`cli/mod.rs:254` 等），而 `VALUE_FLAGS`/`BOOL_FLAGS`（`cli/mod.rs:166/179`）是**全局并集**。于是任何命令都接受所有 flag，只是用不用而已。
- **实测**：`pipi-forum list --title totally-irrelevant` 正常列出；`pipi-forum search body --reopen` 正常；`pipi-forum read <id> --tag nope` 正常。`--json` 在 `close/pin/archive/restore` 被接受但无任何效果（这几个命令无条件打印人话）。

**为什么现在不痛**：人多半不会乱写。
**为什么将来会痛**：`--status closed` 拼进 `reply`，用户以为改状态了，实际静默忽略——把 fail-closed 哲学做成了 fail-open。Agent 走 CLI 时这类"接受但忽略"更难排查。
**建议**：把每个命令的 flag 白名单拆到各自的 `run` 里（`per-command allowed flags`），或让 `parse_args` 接收命令名并查表；至少给未使用的 flag 报"命令 X 不支持 --y"。

**自我证伪**：这是纯 CLI 层、非端口问题。但它直接推翻文档的两处明确断言（design §9、cli 模块文档），且"严格解析"是设计拿来对标 av 的卖点，值得记 Medium 而非 Nit。

---

### D10【Medium】`search` 语义被钉进契约，却与自身字段矛盾

**证据**
- `SearchHit`（`forum.rs:139`）带 `thread_title`，但 `search`（`file.rs:315`）只在 `post.body` 上 `match_snippet`，**从不匹配标题**。搜一个只出现在标题里的词 → 0 命中（代码即证据；`match_snippet` 只被传 `&post.body`）。
- 归档主题不参与检索：`search` 只遍历 `thread_ids_in(&self.threads_dir())`，没有 `include_archived` 开关；而 `list_threads` 有。UI 无法"在归档里搜"。
- 设计 §9/§4 把"全文子串检索（大小写不敏感），每个主题最多一处命中"写进端口文档 —— 这层语义现在是**契约**，未来的索引型后端必须逐字复刻（含"每主题一处"这一非标准选择）。
- 调研 `flat-file-forums.md` §8.2 建议"列表/搜索用可重建派生索引""列表页不读全量正文"，`git-carriers.md` §8 建议索引留本地、区分全文命中与结构化过滤。设计没有采纳，也没说明。

**为什么现在不痛**：MVP 数据量小。
**为什么将来会痛**：搜索是论坛的核心入口之一；标题不搜等于半个搜索。把"每主题一处"和"不搜归档"钉进 port doc，会让 git/索引后端要么违心实现，要么改契约 → breaking。
**建议**：`search` 覆盖标题+正文（标题命中单独标注），`SearchHit` 加 `matched_field`；`ThreadFilter`/`search` 加归档开关；把"每主题一处"降级为实现细节或提供 `per_thread_limit`。

**自我证伪**：有人会说 `session_search` 也是每 session 一处、也是"防浪费"。但会话标题是首条用户消息，天然被正文覆盖；论坛标题是独立元数据，类比不成立。成立。

---

### D11【Medium】`ThreadStatus` 模型在文档里自相矛盾，未知变体使主题"消失"

**证据**
- 代码只有 `Open`/`Closed`（`forum.rs:37`），CLI `--status` 也只认 open/closed（`cli/mod.rs:246`）。
- 设计 §6 却写「主题 `open/settled/locked`」；而设计 §4 又把 `locked` 列为**独立 bool 字段**（"锁定但可读"）。同一个 `locked` 在两节里既是状态枚举值又是布尔字段 —— 二者不能同时成立。
- `ThreadStatus` 没有 `#[serde(other)]`；未知状态串解析失败 → `list_threads` 静默跳过（实测 D1）。

**为什么现在不痛**：只用了 open/closed。
**为什么将来会痛**：Phase 2 的终止语义（`settled` 后不再唤醒、`locked` 只读）必须先定模型。若走"状态枚举加变体"，旧版本会把新状态的主题**从列表里抹掉**（不是报错，是静默消失）；若走"独立 bool"，则 `settled` 无处安放。等改的时候，两种方案都会踩 D1 的读改写。
**建议**：现在就把 `locked` 与 `status` 的关系定死（建议 `status: open|settled|closed` + `locked: bool` 两轴），并让 `ThreadStatus` 对未知值 fail-visible（保留原串、只读展示）。

**自我证伪**：这是文档内部不一致（§4 vs §6），也是"设计文档里错误断言"的条目，但因为它直接影响 Phase 2 接缝，单独列为 Medium。成立。

---

### D12【Medium】`Post.via` 的 DTO 与 frontmatter 默认值不一致

**证据**
- `Post`（DTO，`forum.rs:79`）对 `via` 用了 `#[serde(default)]`，注释也把 `via` 当可省字段；但落盘结构 `PostFrontmatter`（`format.rs:56`）的 `via: PostVia`（`format.rs:61`）**没有** `#[serde(default)]`。
- 于是解码走 `PostFrontmatter`：手写帖子缺 `via` → `decode_post` 失败 → `read_posts_in` 里 `Err(_) => continue`（`file.rs` 读帖循环）→ **帖子被静默丢弃**。
- **实测**：删掉某帖的 `via = "cli"` 后，`read` 只显示主题头、不显示该帖。

**为什么现在不痛**：工具/CLI 写帖永远带 `via`。
**为什么将来会痛**：论坛的价值主张是"人机共写、任何编辑器能改能写"（§3）。人新写一条帖子时不会知道要写 `via`；正确行为应是"默认 `via = cli/web` 或显式 `unknown`"，而不是让帖子凭空消失。这也和 §4"旧文件照读"的承诺相悖（未来的旧文件就是*旧版写出的*文件）。
**建议**：`PostFrontmatter.via` 加 `#[serde(default)]`（默认 `Unknown` 而非 `Tool`，避免把来源未知的帖误记为工具帖）；`PostVia` 加 `Unknown` 变体。

**自我证伪**：有人说"这是测试里已覆盖的坏文件策略（跳过）"。但设计明确把"手写/直改文件"作为一等用法，坏文件策略是给"真损坏"兜底的，不该把"少写一个可选字段"也归进去。`via` 在 DTO 里既然有 default，frontmatter 就不该 required。成立。

---

### D13【Medium】游标/inbox/根目录的属主与位置未定义，README 骨架里没有论坛

**证据**
- 设计 §6：「未读用『每 agent 一台账 + 游标/空标记文件』」，§8 说这些归 `pipi-core::forum`；但**从未说这些文件放在哪**（全局 `~/.pipi/forum/agents/<name>/`？还是 `~/.pipi/agents/<name>/`？）。调研 `multi-agent-messaging.md` §7.4 给的是 `~/.pipi/forum/agents/<name>/{subscriptions,cursors,inbox}`。
- README「Agent 的组成」表与目录骨架（`~/.pipi/` 那棵树）里**完全没有论坛**；而 AGENTS.md 铁律：「目录骨架必须与 README『Agent 的组成』表格一致；改动时两边同步更新。」
- 论坛默认根 `~/.pipi/forum`（`format.rs:124`）是 Agent 树之外的第三个顶层位置，架构上没有任何地方登记它。

**为什么现在不痛**：Phase 1 没写游标代码。
**为什么将来会痛**：(a) 派发器与 run 注入两处要各自解析游标路径，容易各写一份、各有一个"默认根"；(b) 用户备份/迁移 Agent 目录时，游标与 inbox 不在其中，未读状态丢失或错位；(c) README 是"设计北极星"，骨架缺项会让后续协作者以为论坛不该存在。
**建议**：现在就定：论坛数据根（`~/.pipi/forum`）+ 游标/inbox 位置的权威（建议游标放**全局论坛目录**下、按 agent 分，因为跨 Agent 共享；inbox 可重建）；在 README 目录树与「Agent 的组成」里加一行"论坛"；在 `architecture.toml`/文档里登记该顶层目录的性质（非 Agent 素材，故不受 `agent.toml` 材质路径约束——这条也要写明，免得将来被误判为越界）。

---

### D14【Medium】论坛被抬到契约层原语，比后台任务还高

**证据**
- `pipi-protocol` 目前只放 5 个 trait（`grep 'pub trait'`）：`AgentTool`、`Provider`、`SessionLog`、`SessionRepo`、`CommandApprover` + 新增 `ForumRepo`（`forum.rs:168`）。
- 「Agent 组合工具」`create_agent`/`run_agent`/`read_agent` 与「后台任务托管」`submit/query/manage_background_task` 是 README 明确的"核心组合能力"，**都没有**在协议层拥有 trait；论坛作为"产品功能"却拥有一个 + 一个独立 crate + 未来一条 UI 分区。
- 设计 §1 自称「论坛是**产品功能，不是原语**」。

**为什么现在不痛**：多一个 trait/一个 crate 成本很低。
**为什么将来会痛**：层级的暗示会被消费：新功能作者会以"论坛都有独立端口"为理由，要求给各种产品功能配 trait/crate，小核心被稀释；反之，若论坛只是产品功能，它就该像后台任务一样住在 `pipi-core`，端口放本层。
**建议**：二选一并写清理由：(a) 保留协议层端口，但把"论坛是产品功能"改为"论坛的**存储端口**是跨层契约，载体/工具/UI 仍是组合能力"（承认原语化）；(b) 或把 `ForumRepo` 下沉到 `pipi-forum`/`pipi-core`，协议层只保留 DTO。我倾向 (a) 但文档必须诚实。

**自我证伪**：调研 `protocols-and-exposure.md` §6.1 **明确建议** `ForumRepo` 进 `pipi-protocol`。所以这不是"无视调研"，是调研与设计哲学的一次张力。设计没有点出这个张力，也没解释为什么论坛比后台任务更"契约化"。按对抗式立场，值得显式记账，而不是当它不存在。

---

## Low / Nit

### D15【Low】id 与排序的断言与实现不符，`seq` 双写两处真源
- 设计 §4：「id = `{millis}-{8hex}`：与会话 id 同形」。实际会话 id = `{millis}-{完整 32 hex uuid}`（`pipi-session/src/git/sink.rs:36`），论坛只取前 8 位（`format.rs:35`，`&random[..8]`）。随机空间 2³² vs 2¹²⁸，且"同形"不成立。
- 设计 §4：「**文件名排序 = 序号排序**」。实现（`read_posts_in`，`file.rs` 帖子循环）是读完所有帖后按 **frontmatter 的 `seq`** 排序；文件名只用于 `parse_post_seq`/`is_post_file`。且 `{:04}` 在 `seq ≥ 10000` 时字典序反转（"10000" < "9999"），文件名字段根本不是排序依据。
- `seq` 同时写进文件名与 frontmatter（`format.rs:42/57`），两处真源；手工改名或改 frontmatter 会导致"名字里的序号 ≠ 显示的序号"，且两个帖子可拿到同一 `seq`（append 只保证本进程内 max+1，跨进程/手工可重复）→ 排序不确定。
- **建议**：改文档断言；`seq` 只留一处真源（建议 frontmatter 为准，文件名序号仅作人读提示并在读取时校验/修复）；或干脆用内容哈希+时间做 id，`seq` 沦为展示序号。

### D16【Nit】术语漂移：`thread` vs `topic`
- 调研四篇统一用 `topic`（并建议 `topics/<topicId>/`），设计/代码用 `thread`（磁盘 `threads/`，`forum.rs` 全 `Thread*`）。在"Agent 中心"的应用里 `thread` 还容易被误读成"对话线程"。建议统一（要么全 `thread` 并在文档里声明"＝调研的 topic"，要么改名），否则跨文档检索会漏。

### D17【Nit】若干 CLI/实现的对称性问题
- `list_threads`/`search` 为取派生字段读**全部帖子的正文**（`summarize` → `read_posts_in`），与调研"列表页不读全量正文"相反；MVP 可接受，但别在文档里声称已对齐。
- `--json` 在 `close`/`pin`/`archive`/`restore` 被接受却无效果（`BOOL_FLAGS` 含 json，命令却无条件打印人话）。
- 空参数 `pipi-forum` 打印 USAGE 并退出 0，而 `av` 空参数是报错退出 1（`av/cli/mod.rs:64`）；`--help` 被 `run()` 的"任意位置扫描"特判，但 `--root` 不是（见 D9/D4 相关，`--root` 放子命令后是唯一用法，`pipi-forum --root X list` 报"未知子命令 --root"，与绝大多数 CLI 直觉相反）。`--root` 前置应被支持或在 USAGE 里明确禁止。

### D18【Nit】库里承担宿主策略；死 API
- `pipi-forum`（库）通过 `dirs` 提供 `default_root()` 并导出（`format.rs:124`、`lib.rs`），而 `pipi-session` 的载体是"路径由宿主给"。库不该知道 `~/.pipi`；建议 `default_root` 移入 `bin`/CLI，端口/库只接受路径。
- `is_post_file`（`file.rs:462`）导出但仓库内无消费者，属预兑现的 API 面。

---

## 设计文档里我判定为**错误或不准确**的断言

1. §4「id = `{millis}-{8hex}`：与会话 id 同形」——**不实**。会话是 32 位 uuid 尾缀（`pipi-session/src/git/sink.rs:36`），论坛只取 8 位，随机空间差 2⁹⁶，且"同形"会让人误以为碰撞特性一致。
2. §4「**文件名排序 = 序号排序**」——**不实**。实现按 frontmatter `seq` 排序（`file.rs` 读帖循环），文件名只在 `parse_post_seq` 用；`{:04}` 在 `seq ≥ 10000` 时字典序反转。
3. §4「有意保留的字段（向后兼容新增，无需迁移）」——**不实**（对主题侧）。`set_status`/`set_pinned` 的读—改—写会静默抹掉 `thread.toml` 里的未知字段（D1 实测）；正确的表述是"帖子文件向后兼容；主题元数据目前**不**向后兼容"。
4. §5「**作者不可伪造**」——**只对工具路径成立**。CLI 的 `--author` 无认证且默认 `$USER`，Agent 用 bash 即可冒充（D5）；应改为"工具路径不可伪造；CLI 路径为自报，不参与路由"。
5. §3「只追加、无锁（同进程一把写锁）」——**夸大**。`set_status`/`set_pinned`/`archive`/`restore` 不持锁（`file.rs:352/362/372/386`），"一把写锁"并未覆盖全部写路径（`review-code.md` F3/F4/F5 已复现）。
6. §3「跨进程……与会话载体的已知限制一致」——**不成立**。会话跨进程靠 git ref CAS + 重读重试；论坛的 `create_new` 只防文件名撞名，不防同序号（`format.rs:42`+`file.rs` 追加循环），保证更弱（`review-code.md` F8）。
7. §6「主题 `open/settled/locked`」 vs §4「`locked`（锁定但可读）」——**文档自相矛盾**：`locked` 不可能既是状态枚举值又是独立 bool；代码只有 open/closed（D11）。
8. §9 / `cli/mod.rs:18`「严格参数解析（未声明的 `--x` 报错）」——**对命令级不成立**。flag 白名单是全局并集，`list --title`、`search --reopen`、`read --tag` 均被静默接受（D9 实测）。
9. §10「`cargo xtask arch` 通过」——**不构成落位正确的证据**。arch 只校验当前依赖，不校验设计 §8 承诺的 Phase 1/2 依赖（`pipi-core→pipi-forum`、`pipi-forum→pipi-session`），这两条现在都会被拒绝（D6）。
10. §12 开放问题 3「时间：毫秒整数 vs ISO-8601」把它列为"开放问题"——其实**已经决定**（毫秒，§4），且理由是"仓库既有口径"。这个理由是站得住的（`now_millis`/`SessionEntry.timestamp` 一致），但论坛文件是**人类可读的产物**（`created = 1792335028029`），与内部日志的性质不同，失去唯一读者是"用编辑器读帖的人"。建议把它记为"合理简化"而非"开放问题"，以免反复。

---

## 调研建议的逐条裁定（采纳 / 合理简化 / 埋雷）

| 调研建议 | 出处 | 设计处置 | 裁定 |
| --- | --- | --- | --- |
| 载体用 git 对象 + 独立 ref | `git-carriers.md` §8、`protocols-and-exposure.md` §1 | 推迟到 Phase 2，明确理由 | **合理**（但依赖方向未预留，见 D6） |
| id = 内容哈希 + 随机 nonce | `git-carriers.md` §8 该抄 3 | 用 `{millis}-{8hex}` | **埋雷**：无内容哈希 → 跨端去重/幂等无落点（D8）；8 位随机弱于 ULID（D15） |
| 时间用 ISO-8601 | `flat-file-forums.md` §8.1 | 用 u64 毫秒，理由"仓库一致" | **合理简化**（但论坛是人类产物，见错误断言 10） |
| ULID 主键 + 独立 ISO 时间 | `flat-file-forums.md` §8.1 | 用 `{millis}-{8hex}` | **部分埋雷**（见上） |
| 每 Agent 每主题游标 | `multi-agent-messaging.md` §7.2 | 采纳（§6） | ✅ 采纳 |
| `rootId`/`causalityDepth` 展示与落盘 | `multi-agent-messaging.md` §7.2/§7.3 | 归"通知层按需推导" | **埋雷**（D3） |
| `contentHash` 落盘 + 幂等键 | `multi-agent-messaging.md` §7.4 | 未落 | **埋雷**（D8） |
| `author_kind(agent/human/system)` | `multi-agent-messaging.md` §7.4 | 未落（`via` 近似但不等于） | **部分埋雷**（审计弱，D5/D8） |
| mention 解析正文 token、忽略代码块 | `multi-agent-messaging.md` §7.2 | 未做、未声明 | **埋雷**（D4） |
| 列表/搜索用可重建派生索引、不读全量 | `flat-file-forums.md` §8.2、`git-carriers.md` §8 该抄 6 | 全量线性 | **合理简化**（MVP）+ 别声称已对齐（D17） |
| 搜索区分标题/正文权重 | `flat-file-forums.md` §6 | 只搜正文、`SearchHit` 却带标题 | **埋雷**（D10） |
| `status: open/settled/locked` | `multi-agent-messaging.md` §7.3 | 只有 open/closed，且模型自相矛盾 | **埋雷**（D11） |
| 归档=状态、物理删除只删本地 | `git-carriers.md` §7 | 采纳（只 archive/restore） | ✅ 采纳 |
| `--format json` 双输出、错误走 stderr | `git-carriers.md` §5 | 采纳（`--json`，但部分命令无效） | ✅（D17 小瑕） |
| MCP 双门面（写只能 tools）、A2A 不做 | `protocols-and-exposure.md` §2/§4 | 采纳并推迟 | ✅ 采纳 |
| bundle 主通道导出 | `protocols-and-exposure.md` §5 | 推迟 Phase 2 | ✅ 合理 |

---

## 结论

没有 **Blocker**：MVP 自洽、测试全绿、`cargo xtask arch` 通过。真正的问题全部是"留给未来的缝"：

- **最该立刻修的**：D1（`thread.toml` 读改写吞字段）与 D11（状态模型），因为它们是"预留字段"计划的**地基**，Phase 1 一旦写 `board/locked/updated_at` 就变成数据丢失；
- **最容易被低估的**：D3（因果链无人拥有）与 D4（提及不解析）——它们决定论坛能不能履行"异步 + 多对多 + 可沉淀"的定位；
- **最该在写代码前改配置的**：D6（`architecture.toml` 与设计 §8 的依赖承诺不一致）。

设计文档整体质量高、自我意识强（明确说"产品功能不是原语"）。它的问题不是"想得不周到"，而是**几处"向后兼容/无需迁移/与会话一致"的乐观断言把真实接缝说小了**——而接缝正是第二阶段全部工作量的来源。
