# 会话记录 git 化设计

日期：2026-10-12。状态：**设计待确认，未实施**。

目标（用户 2026-10-12 拍板）：**对话记录以 git 为载体，能用标准 git 工具浏览/分发；
压缩对话时新建分支。** 不动工作区文件（那是另一个话题），不用户项目的 `.git`。

实测脚本：`/tmp/pipi_git_bench.py`（代价与去重）、`/tmp/pipi_git_layout.py`（三种布局与
工具体验、bundle 往返）。以下所有数字来自本机真实最大会话：
`~/.pipi/agents/Q/sessions/1790247474035-dd72cf…jsonl`，1472 条 / 4.88 MB / 平均 3315 B 每条 / 57 轮。

## 1. 边界

做：

- 会话的**持久化载体**从「一个 JSONL 文件」变为「一个 git 分支」。
- 一条 entry = 一个 commit；commit 摘要 = 一行人类可读文本；tree 里放该条 entry 的
  **原文 JSON 行**（与今天落盘的那一行逐字节相同）。
- 压缩 = 新建分支（零复制），旧线保留可回溯。
- 归档 = ref 前缀迁移；分叉 = 从某个 entry 的 commit 起建分支（共享对象，零复制）。
- 分发 = `git bundle` / `git push` / `git fetch`。

不做：

- 不写用户项目的 `.git`（影子仓库，见 §3）。
- 不改 `SessionEntry` / `EntryKind` 的 JSON 结构、不改 IPC/DTO、不改 session id 形态。
- 不做隐式迁移：既有 `sessions/*.jsonl` 继续按 JSONL 读，导入是显式命令（§10 P3）。
- 不把 workspace 文件快照进 git（opencode 那套；与本设计无关）。

## 2. 实测依据

### 2.1 布局对比（同为「一条 entry 一个 commit」，差异在文件粒度）

| 布局 | 导入 1472 条 | gc 后占盘 | 相对原文 | 结论 |
| --- | --- | --- | --- | --- |
| 现状 JSONL（append+flush） | 2 ms | 4.88 MB | 1.00× | 基线 |
| message 即 payload、tree 为空 | 134 ms | 2.43 MB | 0.50× | 淘汰：`git log -p`/`show` 无内容，浏览体验为空 |
| **B1 一条 entry 一个文件** | **401 ms** | **3.01 MB** | **0.62×** | **采用** |
| B2 一轮一个文件 | 718 ms | 3.51 MB | 0.72× | 备选：树更好读，但要向后端引入「轮」这一产品语义 |
| B3 一会话一份整文件 | 13 580 ms | 20.55 MB | 4.21× | 淘汰：每个 commit 重写整份大 blob，delta 窗口撑不住，写入是大 O(会话长度²) |

B1 与 B2 都实测**逐条还原 1472/1472 字节级一致**。

### 2.2 其它实测

| 操作 | 实测 |
| --- | --- |
| 逐条追加（一条一次 `fast-import` spawn） | 2.8 ms/条（一轮 30 条约 83 ms；冷启动 1.8 ms） |
| 全量读回（`ls-tree -r` + `cat-file --batch`） | 31–56 ms / 1472 对象 |
| `git log --oneline` / `for-each-ref` / `log --all` | 1 / 1.9 / 11 ms |
| 压缩＝建分支（`update-ref`） | 0.7–1.6 ms，**0 字节增量** |
| 现压缩分叉（parse+serialize+写+归档原件） | 27 ms，落盘 9.76 MB |
| `git bundle create` → 接收方 `fetch` | 7.9 ms / 2.61 MB → 92.6 ms |
| 删除会话（删 ref + `gc --prune=now`） | 1 ms；独有对象**确实不可读**（已验证） |
| 跨会话行级内容去重 | **0.0%**（16.0 MB 全部 vs 16.0 MB 唯一） |

去重为 0 的原因：每条 entry 都带唯一 uuid + seq + timestamp，相同内容不会产生相同字节。
所以 git 的内容寻址**不省空间**；0.62× 的占盘优势来自对象压缩与 pack delta。
真正受益于对象复用的是 **压缩/分叉时被复制的保留段**（同一批行 → 同一批 blob，零额外存储）。

### 2.3 标准 git 工具能看到的（实测输出）

裸仓库里 `git log --oneline` 直接就是对话（真实输出片段）：

```text
c66f14a custom
8656130 compaction: llm-summarize
c0bdc25 assistant: 已提交（`9ebf7e7`），`+187 / −9`，7 个文件。 ## 做了什么 **判定规则**（fail-
099abb7 toolResult: 9ebf7e7 feat(live): 识别"过期且无价值"的持仓并忽略，不再锁死试点 crates/poly-
e579529 toolResult: 10 0 === 用真实数据复核判定规则（只读，复刻 dead_position）=== 判定为死仓（会被剔除、
```

`git log --graph --all --decorate`（压缩产生的两条线一目了然）：

```text
* c66f14a (HEAD -> main, archive/1790247474035-dd72cf…, session/1790247474035-dd72cf…, session/compacted-demo) custom
```

其余已验证：`git log -p -n1` 给出该条 entry 的内容（421 字节）；`git show <branch>:<path>`
取单条 entry；`git archive main e` 导出整棵树（52.7 ms / 6.02 MB tar）；
`git diff main~5 main` =「5 files changed, 5 insertions(+)」；接收方 `git fetch <bundle>` 后
`git log --oneline` 可读、`git show <branch>:<path>` 可取出单条 entry。

轮级浏览（在 entry 级 commit 之上，靠 `Turn: <n>` trailer，已实测）：

```bash
git log --oneline --grep='^user:'                      # 只看我说了什么
git log --oneline --grep='Turn: 12'                    # 只看第 12 轮（含该轮的工具调用）
git log --format='%s  [%(trailers:key=Turn,valueonly)]' # 每行带轮号
```

## 3. 数据模型

影子仓库（**bare**，绝不碰用户项目 `.git`）：

```text
~/.pipi/agents/<name>/sessions.git/          # git init --bare --initial-branch=main
  refs/heads/session/<session-id>            # 一条会话线（压缩后是新分支）
  refs/heads/archive/<session-id>            # 归档（= 今天 sessions/.archive/ 的等价物）
  refs/heads/main                            # 自述 commit，兼作 HEAD 锚，让裸仓库 git log 可用
```

- **session id 不变**：仍是 `{millis}-{uuid}`，即分支名后缀。列表的 startedAt 仍取
  millis 前缀，下游（`read_agent_output`、后台任务 owner、供应商缓存路由键）契约不动。
- 一个 commit = 一条 entry：
  - tree：新增 `e/<3 位分片>/<seq:05>-<kind>-<id8>.json`，文件内容 = 今天那一行 JSON 原文。
    路径里的零填充 seq 让「按文件名排序 = 对话顺序」，读回不必依赖 DAG 拓扑。
  - message：一行摘要，`user: …` / `assistant: …` / `toolResult/<name>: …` / `compaction: …` /
    `model_change` / `env: …`（截断 72 字符，供 `git log --oneline` 阅读）；
    body 里带 `Turn: <n>` trailer（轮号），用于轮级浏览（见 §2.3 实测）。
- **粒度**（易混；用户 2026-10-12 追问，**待拍板**）：一个对话 = 一个 session id = 一条
  `session/<id>` 分支。「一个用户输入一个 commit」= 一轮一个 commit，但 commit 创建即不可变，
  而用户输入在轮首、该轮产物（assistant/toolResult）在轮末才齐，所以只有三种可实现形态：

  | 形态 | commit 数（实测样本） | `log --oneline` | 崩溃丢什么 | 分叉精度 |
  | --- | --- | --- | --- | --- |
  | O1 每条 entry 一个 commit | 1472 | 1472 行（靠 `Turn:` trailer 做轮级视图） | 至多 1 条 | 任意 entry |
  | O2 轮末一个 commit（含用户输入） | 56 | 56 行，一行 = 一轮 | 整轮（实测中位 14 条/49 KB，最大 118 条/0.44 MB，其中 63 条工具输出） | 轮边界 |
  | O3 轮首提交用户输入 + 轮末提交该轮产物 | 112 | 112 行（输入/结果成对） | 该轮产物，保住用户输入 | 轮边界（+输入/结果之间） |

  实测样本（Q agent 1472 条 / 56 轮）：每轮条目数中位 14、均值 26、最大 118；
  每轮字节中位 49 KB、均值 85 KB、最大 0.44 MB。

  采用 **O2（轮末一个 commit，含用户输入）**：用户 2026-10-12 选择「一个用户输入一个
  commit + 可 revert 最近一个用户输入」；revert 点必须落在 commit 边界上，O2 让它精确到
  `tip^`。O1 只在将来要 entry 级回溯时再考虑——entry 级分叉目前没有消费方
  （`fork_session(upToEntryId)` 的前端恒传 `null`，压缩也不需要 entry 级分叉）。
  实现上把「提交点」做成运行时决定：store 只提供 `append(entry)` 与 `commit_point()`，
  由 runtime 决定提交时机，不写死在存储层；O2 的崩溃代价（丢进行中的一轮，实测中位
  14 条/49 KB、最大 118 条/0.44 MB）可用「长轮内每 N 条一次 checkpoint」这个旋钮收紧，
  默认关（会往 log 里引入半轮）。

  **被否的第四种形态**（用户 2026-10-12 提出、我反对）：提交时机挪到「下一次用户输入到来时」
  （最近一轮保持未提交，于是 revert 就是丢弃工作副本）。三条反对理由：
  ① **持久化倒退**——四家上游全是「先持久化、revert 是显式操作」：codex
  `thread-store/src/local/revert_thread.rs` 头部注释「Old rollouts stay intact. The new file
  references the retained prefix, and the only mutable cutover is the existing SQLite
  rollout-path pointer」；hermes `hermes_state_rewind.py` 头部「Rewind is a persisted-history
  operation: the durable transcript is the authority」；opencode `session/revert.ts` 先把
  revert 点写进 session 行、`cleanup` 才 `removeMessage` 删尾部行。把最近一轮放在持久化之外，
  崩溃/强杀丢掉的正是用户刚看完的那一轮。
  ② **新会话在仓库里是空的**——会话创建只写 env/model_change，首条用户消息要等「下一次输入」
  才提交，单轮会话在 git 里 0 commit，列表与浏览都看不到；要补救就得再加「创建/关闭」两个
  提交点，那「一个用户输入提交上一次」已不成立。
  ③ **正常关窗也丢**——发一轮、看完、关窗，没有下一次输入，该轮永不提交（强杀更补不了）。

  结构性副产物：O2 下「悬挂工具调用」在落盘层不可能出现（每个 commit 边界都是完整轮），
  今天的 `settle_unanswered_tail` 从常态兜底退化为旧 JSONL 会话的兼容路径；
  轮内 steer 消息随该轮 settle 一起提交（或按需单独提交）。
  - author：`<role> <role@pipi.local>`；committer：`Pipi <pipi@local>`；日期取 entry 的
    `timestamp`（git 只到秒，毫秒精度仍由 payload 承载 → `git shortlog -sn` 可按角色/模型统计）。
- `seq` / `parentId` / `timestamp` 原样保留在 payload 里，与 commit 父链互为校验
  （可加断言测试：父链 == parentId 链）。
- 会话标题不能取根 commit 的 subject：实测会话首条常是 `env` 记账条目。标题取
  「首个 `user` 条目」，从分支 tip 的树里按 seq 读前若干条 entry（每次几毫秒）。

## 4. 读写路径

写（O2：条目先进内存缓冲，轮末一次提交）：

```text
SessionLog::append(entry)      → 只进内存（tip/seq/message_ids 记账照旧）
SessionLog::commit_point()     → 把缓冲的 N 条组装成 fast-import 输入
                                 （N 个 blob + 1 个 commit(from 当前 tip)）
                               → spawn 一次 git fast-import
```

- **提交点（「轮末」）的判定**——权威定义在 `run_turn`
  （`crates/pipi-core/src/runtime/execution.rs`，`run_agent_loop` 外层的
  steering 收割循环）：外层循环反复 `run_agent_loop`，每轮之后
  `steering.drain()`，**插话队列排空或 abort 置位即返回**。所以「轮末」= 一次 `run_turn`
  返回，不是「模型停止输出」，也不是「assistant 消息结束」。
- 落点：`pipi-core/src/runtime/execution.rs` 的 spawn 块里，
  `run_turn(...).await` 之后、`run_compaction` 之前（压缩可能 fork 新分支，压缩条目应当
  成为新分支的首个 commit），且**必须在 `running.finish(run_token)` 之前完成**——否则
  「running 已释放」不等于「历史已落盘」，前端收到 AgentEnd 后立刻发的新一轮会与提交竞争
  同一分支。
- 汇聚到这条路径的结束方式：正常结束、用户点停止（abort 置位）、provider 报错；
  child 会话同理，其超时在 `child.rs:266-280`（`tokio::time::timeout` →
  `append_child_timeout_terminal`）后收尾。
- **steer 插话不结束轮**：队列非空就继续 `run_agent_loop`，所以一轮可能含多条 user entry，
  Turn trailer 的轮号必须按 `run_turn` 次数递增，不能按 user entry 数。
- **进程被杀**（强杀/崩溃）：spawn 的 task 随进程消失，提交点不执行 → 这一轮不落盘
  （O2 的已知代价，见 §3）。
- 提交耗时（实测写入吞吐 0.27 ms/条）：一轮中位 14 条/49 KB ≈ 6 ms；最大轮 118 条/0.44 MB
  ≈ 35 ms。提交点在轮末、不在交互路径上，相对其后的压缩可忽略。
- 实现上必须用异步/阻塞线程池跑 git 进程，不能在 async 上下文里同步 spawn。

轮外的提交点（少一个就会出问题）：

| 提交点 | 代码位置 | 不提交的后果 |
| --- | --- | --- |
| 会话创建 | `slots/mod.rs:95-101`（写 model_change）、`execution/mod.rs:282-283`（env 记账） | 单轮会话在 git 里 0 commit，列表/浏览都看不到——与 §3 否掉的「下次输入才提交」是同一个毛病 |
| 压缩条目 | 轮末之后的 `run_compaction`；空闲时的手动 `compact_now` | 压缩记录不入史，回放按旧历史 |
| 手动切模型 | `queries.rs::set_session_model`（空闲时追加 model_change） | 模型变更不入史 |
| 旧会话尾部修复 | `execution/mod.rs:95-124`（`settle_unanswered_tail`） | 只对旧 JSONL 会话需要；git 后端每个 commit 边界都是完整轮，不会出现悬挂尾部 |
- 读：`ls-tree -r <branch>` 拿路径与 blob id（顺序即对话顺序）→ 一次
  `cat-file --batch` 流式取内容 → 逐行 serde 成 `SessionEntry` → 之后的
  `active_path` / `replay` / `rebuild_messages` / 统计全部复用，零改动。
- 列表：`for-each-ref refs/heads/session|archive` 拿 id 与 tip 时间；标题按上一节取。
- 行格式与容错策略不变：坏行照旧跳过（`load_session` 的逐行解析逻辑平移即可）。

## 5. 生命周期

- **压缩＝新建分支**（用户要点）：新分支从压缩点起，`update-ref` 1.1 ms、零复制；
  压缩条目照旧带 `summary` / `keepFromEntry` / `sourceTip`；旧线留在 `session/<old-id>`
  （或按设置转 `archive/<old-id>`）。今天「fork 到新文件 + 归档原件」的 9.76 MB 落盘消失；
  保留段的对象被两条线共享，不重复存储。
- **分叉**（`fork_session(upToEntryId)`）：新分支从目标 entry 的 commit 起，零复制。
- **revert 最近一个用户输入**（= 市面标准的 `/undo`、`/rewind` 的会话层那一半）：
  与压缩、分叉**同一机制**——从「该轮之前的 commit」（O2 下就是 `tip^`，O(1) 不需搜索）
  新建分支。两条 `update-ref`（≈2 ms、零字节）：先把被撤销的线存成
  `archive/<id>-reverted-<ts>`（可回溯），再把 `session/<id>` 移回该 commit；之后的轮
  挂在这个 commit 上，被撤销的线在 `--graph` 里表现为兄弟分支。
  对照上游：opencode 是删除尾部消息行（不可回溯）、hermes 是软删、codex 是新文件引用保留
  前缀 + 指针切换——git 版在**零复制 + 可回溯**两点上都更好。
  注意与「工作区文件 revert」是两件事：把 agent 改过的文件恢复原状需要文件快照
  （opencode 影子仓库 `write-tree`、zcode per-file artifact），本设计不含，需另行决定。
- **归档/恢复**：ref 在 `session/` 与 `archive/` 前缀间迁移（`update-ref` + 删旧 ref）。
- **删除**：删 ref → `gc --prune=now`（实测 1 ms，独有对象不可读）。bare 仓库默认不写
  reflog（实测无 `logs/` 目录），所以不需要 `reflog expire`；若日后开启
  `core.logAllRefUpdates`，删除路径必须补 `reflog expire --expire=now --all`，
  否则被删会话会被 reflog 兜住。注意：与其它线共享的保留段**不会**被清掉（仍被引用），
  这是共享存储的必然结果。
- **gc**：收在空闲时执行，且与写者互斥（repo 级锁）。`fast-import` 的小 pack 会被
  `fastimport.unpackLimit`（默认 100）拆成松散对象，逐条 spawn 会产生较多松散对象，需要定期
  `git gc`/`repack`。参考 opencode 的 `gc --prune=7.days` 策略。
- **并发**：会话与分支一一对应、单写者（现有 `Session.writer` 互斥），不同会话改不同 ref，
  ref 锁互不冲突；对象写入本身安全。唯一需要串行的是 gc 与写者。

## 6. 分发与分享

- 单会话：`git bundle create <id>.bundle refs/heads/session/<id>`（实测 2.61 MB / 7.9 ms），
  接收方 `git fetch <bundle>` 后即可 `git log` / `git show`（实测可用）。
- 整 Agent：`git push` 到远端即可得到该 Agent 全部会话线；`git log --all --graph` 是会话浏览器。
- 人类可读的分享（非 git 用户）：仍需要一个「渲染成 markdown」的导出命令，本设计不覆盖。

## 7. 代码落点与依赖边界

> 2026-10-12 更新：拆分布局（三个 workspace）已回退，本设计改在**单 workspace**
> 布局下实施；同日完成的结构重构（15 → 12 个成员）已把契约层定在
> `crates/pipi-protocol`、载体定在 `crates/pipi-session`、宿主并成 `crates/pipi-core`。
> 第一步已落地，见 §10。

- 契约层（`crates/pipi-protocol::session`）：
  - `SessionLog`（身份 + `append_*` + `tip`/`seq`/`message_ids` 记账 + `commit_point`）与
    `SessionRepo`（create/open/exists/entries/list/list_archived/fork/archive/restore/delete/
    delete_archived）两个接口，与条目格式一起定义在 `crates/pipi-protocol/src/session.rs`。
  - `SessionEntry` / `EntryKind` / `replay` / `active_path` 与行级容错随格式一起进契约层，
    行为不变；JSONL 实现 = `SessionWriter`（`crates/pipi-session/src/ledger.rs`，
    `impl SessionLog`）+ `JsonlSessionRepo`（`crates/pipi-session/src/repo.rs`，
    `impl SessionRepo`）。
  - 判据：既有测试**全绿不改断言**（唯一例外见 §10 记录）。
- 宿主侧：
  - 载体侧 `crates/pipi-session`（已存在，今天装 JSONL）：git 实现按职责分文件
    （sink / reader / refs / gc / probe），遵守约 500 行拆分约定；只依赖契约层，
    不依赖宿主 `pipi-core`。
  - git 可用性探测仿 `av::requires::probe_version`（`git --version`）。
  - 装配点：`pipi-core/src/runtime/store.rs`（唯一选载体的地方，已落地）按 per-agent 配置
    选后端；`agent.json` 新增 `sessionStore: "jsonl" | "git"`，**默认 `jsonl`**。
  - `pipi-core/src/tools/agent.rs` 的 `read_agent_output` 已改为走 `SessionRepo`
    （该处注释标了载体装配点）。
- `architecture.toml` 登记新 crate 与依赖方向（单 workspace 下同一份规则文件），跑
  `cargo xtask arch`。
- 缺 git：配置为 `git` 且探测失败 → **fail-closed** 并给可执行提示，不静默回落成 JSONL
  （否则同一 Agent 出现两种载体，列表/归档语义会混乱）。

## 8. 兼容与风险

兼容（已实测）：

- 行格式零变更：payload 就是今天那一行，1472/1472 逐字节还原。
- 旧会话零迁移：默认后端仍是 JSONL，`sessions/*.jsonl` 与 `.archive/*.jsonl` 照旧可读；
  列表可同时覆盖两种载体（按 id 合并）。
- IPC/DTO/前端不需要改；session id 形态不变。

风险：

- **读取路径新增外部依赖**：以前读会话只碰文件，现在要 spawn git。git 不可用时 git 载体
  的会话读不出来（缓解：默认 JSONL、显式导出命令、payload 仍是明文 JSON，`git cat-file` 可取）。
- **删除语义**：删 ref 后对象在 gc 前仍在盘上（`git fsck` 类手段可找回），删除必须紧接着
  `gc --prune=now`。
- **松散对象/包数量**：逐条 spawn 会产生大量松散对象，必须有 gc 策略与 repo 级互斥。
- **写入延迟**：2.8 ms/条落在 emitter 路径上，必须异步/阻塞线程池。
- **仓库膨胀无法按会话回收**：共享保留段只有在所有引用它的线都删除后才回收。

## 9. 验收口径

1. 等价性：同一 entry 序列分别经 JSONL 后端与 git 后端，读回的行逐字节一致（含 compaction
   /env/model_change），且两后端下 `replay` 结果相同。
2. 压缩＝分支：压缩后新分支的父提交 == 压缩前 tip；旧线仍可读；`log --all --graph` 能看到两条线；
   落盘增量为 0（新分支不复制保留段对象）。
3. 分叉：`fork_session(upToEntryId)` 后两条线各自追加互不影响，共享前缀对象只存一份。
4. 归档/恢复/删除：ref 迁移正确；删除后独有对象 `cat-file -e` 失败。
5. 分发：`bundle` → `fetch` → 接收方 `log`/`show` 与源一致。
6. 缺 git：探测失败时启动会话报明确错误，不产生半 git 半 jsonl 的会话。
7. 并发：同 Agent 两个会话并行跑，两条分支内容互不串（对齐现有 `concurrent_sessions.rs` 口径）。
8. 崩溃：写入中途被杀，重启后能读到已提交条目；悬挂工具调用仍由 `settle_unanswered_tail`
   在打开时补齐。
9. 现有测试：JSONL 后端下全部既有断言不变通过。

## 10. 实施阶段

| 阶段 | 内容 | 完成判据 | 状态 |
| --- | --- | --- | --- |
| P1 | 内核抽取 `SessionLog`/`SessionRepo`，JSONL 后端为现有实现 | 全部既有测试不改断言通过 | **已完成（2026-10-12）** |
| P2 | `crates/pipi-session` 的 git 载体：sink/reader/refs/gc/probe + 内联单测 + tests/ 黑盒 | §9 的 1–5、7、8 通过 | 待做 |
| P3 | 装配与配置（`agent.json` 的 `sessionStore`）+ 显式导入命令（逐条校验、原文件保留） | 导入后校验一致且幂等 | 待做 |
| P4 | 前端：仓库路径提示、导出/bundle 入口（可选） | 手动验证 | 待做 |
| P5 | 规则与文档：`AGENTS.md` 会话格式条款加「容器可为 git 分支、行格式不变」，更新本文件状态 | `cargo xtask arch` 通过 | 待做 |

### P1 落地记录（2026-10-12）

- `crates/pipi-agent/src/session.rs`（1139 行）先拆成 `session/{mod,history,ledger,repo}.rs`，
  （按「模块超 ~500 行生产代码拆目录」规则；内联测试跟着被测代码走）。
- 接口定义在 `session/mod.rs`；`SessionWriter` 实现 `SessionLog`，`JsonlSessionRepo`
  实现 `SessionRepo`；`fork_session` / `archive_session_file` / `list_session_summaries`
  仍是 JSONL 实现的公开函数（黑盒测试在用），但运行时不直接调用它们。
- 运行时（`crates/pipi-core/src/runtime`）只依赖两个接口：`Session.writer` 类型改为
  `Arc<Mutex<Box<dyn SessionLog>>>`（`Box` 是为了压缩分叉能就地换账本），
  归档/恢复/删除/分叉/列表全部走 `SessionRepo`；载体选择集中在
  `runtime/store.rs::session_repo`。
- 验证：`cargo test --workspace --locked` 387 passed / 0 failed（1 ignored 需
  `SPARROW_SESSION_CORPUS`）；`cargo fmt --all --check` 通过；`cargo xtask arch` 通过。
- 唯一改动的断言：删掉 `assert!(ledger.persistent_path().is_none())` —— `persistent_path`
  是 JSONL 载体细节、已不在接口上；同测试的 `assert!(ledger.is_temporary())` 保留，
  语义等价。其余改动均为机械适配。
