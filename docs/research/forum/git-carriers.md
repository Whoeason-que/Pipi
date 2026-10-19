# git 载体调研：git-bug 与 git-appraise 对 Pipi 本地论坛的启示

> 研究对象（只读）：`reference/git-bug`（Go，distributed bug tracker）、`reference/git-appraise`（Go，distributed code review）。
> 引用格式 `ref/<project>/<path>`。结论都指到源码文件。

## 0. 一句话对照

| 维度 | git-bug | git-appraise |
| --- | --- | --- |
| 载体 | 每个实体一条 **ref**（`refs/bugs/<id>`），commit 链 + tree/blob | **git-notes**（`refs/notes/devtools/*`），一行一条 JSON |
| 数据单元 | `Operation`（细粒度編集）→ commit 一个 `OperationPack` | 一条 note = 一个 JSON 对象（request/comment/ci/analysis） |
| 顺序 | Lamport 时钟 + DAG 拓扑 + 字典序兜底 | 时间戳字符串排序，「后者覆盖前者」 |
| 合并 | CAS 更新 ref + 分叉时造 merge commit | `git notes merge -s cat_sort_uniq`（行级并集） |
| 索引 | SQLite FTS5（词干 + CJK），excerpt 缓存 | 无，每次全量扫 notes |
| UI | CLI + TUI + GraphQL/Web，共用 cache 层 | 仅 CLI，Web 是独立项目 |

---

## 1. 数据放在哪

### git-bug：git 对象 + ref，本地辅助状态藏在 `.git/`
- 领域数据全在 git 对象里：`refs/bugs/<id>` 指向一条 commit 链，commit 的 tree 里有 `ops` blob（JSON 操作数组）、`edit-clock-N` / `create-clock-N`（时钟，指向空 blob）——`ref/entity/dag/operation_pack.go` 的 `Write`；namespace 定义见 `ref/entities/bug/bug.go`（`Namespace="bugs"`）、ref 命名见 `ref/repository/common.go`（`refs/<namespace>/<key>`）。
- 设计动机与代价写在官方文档：`ref/doc/design/data-model.md`（「Entities are stored in git objects」「Time is unreliable」）。
- **非领域数据**（可重建的派生态）放在 `.git/git-bug/`（namespace 常量见 `ref/commands/execenv/env.go`，组装见 `ref/repository/compose.go`）：Lamport 时钟 `clocks/`（`ref/repository/clocks_persisted.go`）、SQLite 全文索引 `index-v1.db`（`ref/repository/index_sqlite.go`）。
- 代价：可读性差（要 `git cat-file`）；好处是 push/pull/GC/签名全部复用 git；索引放本地（不入 git），所以跨机器要重建。

### git-appraise：git-notes，一行一条 JSON
- 全部数据在 notes ref：`refs/notes/devtools/reviews`（`ref/review/request/request.go` 的 `Ref`）、`.../discuss`（`ref/review/comment/comment.go`）、`.../ci`、`.../analyses`；归档 ref `refs/devtools/archives/reviews`（`ref/review/review.go` 的 `archiveRef`）。总览见 `ref/README.md` 的 Metadata 一节。
- 写入 = `git notes --ref <ref> append -m '<json>' <revision>`，见 `ref/repository/git.go` 的 `AppendNote`。
- 代价：notes 明文一行一对象，可直接 `git cat-file` 读，但**只能 append，不能改/删**（用「新 note 覆盖旧 note」表达编辑，见 §3）；notes 与代码 commit 绑定，`git-filter`/rebase 会牵连。

> **对 Pipi 的启示**：领域数据进 git 对象是可取的（与现有 session 载体一致、可 push/GC/签名）；但把「可重建的索引/时钟」放本地目录（git-bug 的做法）比把索引也提交进 git 更合适。

---

## 2. 领域模型：一次操作在存储层是什么单位

### git-bug：操作日志（operation-log）+ 编译出快照
- 领域 = 一串不可变 `Operation`。类型枚举见 `ref/entities/bug/operation.go`：`CreateOp / SetTitleOp / AddCommentOp / SetStatusOp / LabelChangeOp / EditCommentOp / SetMetadataOp`。
- **一次操作 = 一个 Operation**：改状态是一个 `SetStatusOperation`（`ref/entities/bug/op_set_status.go`），加评论是一个 `AddCommentOperation`（`ref/entities/bug/op_add_comment.go`）。
- 存储单位是 `OperationPack`：同作者的一批操作打包成一个 blob（`ref/entity/dag/operation_pack.go`，`MarshalJSON` 只序列化 `{author, ops}`，时钟/版本写在 tree entry 名里）。commit 时会把连续同作者的操作切块，每块一个 commit（`ref/entity/dag/entity.go` 的 `commitOperationsLocked`）。
- 阅读时把操作按序 `Apply` 到空 `Snapshot` 上得到当前状态（`ref/entities/bug/bug.go` 的 `Compile`、`ref/entities/bug/snapshot.go`）——即「事件溯源 / operation-based CRDT 的编译」。
- 领域字段：thread = bug（`Title/Status/Comments/Labels/Author/Actors/Participants/Timeline`），label 是 `[]Label`（`ref/entities/common/label.go`），status 是枚举 `Open/Closed`（`ref/entities/common/status.go`）。
- 身份模型：`Identity` 是多版本序列（`ref/entities/identity/identity.go`、`version.go`），带 PGP 公钥（`ref/entities/identity/key.go`），独立 namespace `identities`；操作里只存 author id。

### git-appraise：直接存「对象」，不存操作
- 存储对象就是语义对象：Review **Request**（`ref/review/request/request.go`）、**Comment**（`ref/review/comment/comment.go`）。没有 compare-and-swap 的「op」层。
- 状态不是存储字段，而是**推导**出来：`IsOpen/IsAbandoned/Submitted`（`ref/review/review.go`），显示状态串在 `ref/commands/output/output.go` 的 `getStatusString`。resolved 由 comment 树聚合（`updateThreadsStatus`）。
- 编辑 = 追加一条带 `original` 字段指向旧 comment 的新 comment；回复 = 带 `parent` 字段（`ref/review/comment/comment.go`，schema `ref/schema/comment.json`）。
- 没有独立 label；review 的「标签」等价物是 reviewers 列表和 CI/analysis 报告。

> **对 Pipi 的启示**：论坛条目（thread/post/回复/状态变更）用「不可变操作 + 编译快照」的 git-bug 模型，比 git-appraise 的「直接存最终对象、靠追加覆盖」更适合需要改标题/改状态/删除/归档的场景。

---

## 3. id 与顺序

### git-bug
- **id = 内容哈希（sha256）+ 强制随机 nonce**：`ref/entity/id.go` 的 `DeriveId`；nonce 由 `ref/entity/dag/operation.go` 的 `makeNonce(20)` 生成并在 `Validate` 里强制长度 20–64。注释明确解释了为什么需要 nonce（同秒同作者的两个简单操作否则会撞 id）。实体 id = 其**第一个操作**的 id（`entity.Id()`）。
- 对外可前缀匹配、展示取前 7 位（`ref/entity/id.go` 的 `Human`），歧义报错。
- 复合 id：`bug/comment` 用交错编码 `CombineIds` 让 bug 前缀与 comment 前缀都能命中（`ref/entity/id_interleaved.go`）。
- **顺序 = Lamport 逻辑时钟 + DAG 拓扑 + 字典序兜底**：`ref/entity/dag/entity.go` 的 `readSince`：先 Kahn 拓扑排序校验 DAG（只能一个叶、merge 不能带 ops），校验父时钟严格小于子时钟且不许跳太远（防时钟回滚攻击），最后按 `EditTime` 排序、并列时按 pack 的 id 字典序——完全脱离 DAG 拓扑。时钟持久化在 `.git/git-bug/clocks/`（`ref/repository/clocks_persisted.go`）。
- 分片/布局：一个实体 → 一个 ref → 一条 commit 链；**每个操作 pack 一个 tree/commit**，不是工作区目录树。

### git-appraise
- **id = sha1(序列化 JSON)**（`ref/review/comment/comment.go` 的 `Hash`），评论/请求的定位靠 `parent`/`original` 引用哈希。
- review 的「主键」是它所注解的 **git commit hash**（notes 的注解目标）。
- 顺序：时间戳字符串，序列化时零填充到 ≥10 位以保证字典序 = 时间序（`comment.serialize`）；请求排序取「时间戳最大者」为当前请求（`ref/review/review.go` 的 `requestsByTimestamp`；README 明说多个 request 时最后一个生效）。

> **对 Pipi 的启示**：用「内容哈希 + nonce」而非递增 id，天然幂等、可跨端去重；顺序不要信墙上时钟（git-appraise 就是反例，只适合单点 append），要学 git-bug 的 Lamport + 拓扑 + 确定性兜底。

---

## 4. 并发与合并

### git-bug
- **写入 = CAS 更新 ref**：`UpdateRef(namespace, key, old, commit)` 只有 ref 仍指向预期旧值时才能前进，否则 `ErrRefChanged`（接口 `ref/repository/repo.go`，实现 `ref/repository/gogit_refs.go`：自己按 git 的 `.lock` 协议做原子 rename）。失败后 `Entity.Repair` 重读+重试（`ref/entity/dag/entity.go`）。
- **无锁全局写**：并发的两个写者各自追加 commit，push 是非 fast-forward（refspec 无 `+`，见 `ref/repository/gogit.go` 的 `PushRefs`），远端拒绝覆盖。
- **分叉合并 = 造一个空 ops 的 merge commit**（场景 5，`ref/entity/dag/entity_actions.go` 的 `merge`），把两条分支汇成一个 DAG，时间线不丢；Lamport 时钟保证读出来顺序确定。
- 不支持 CRDT 式的字段级合并——冲突靠「两条操作都保留 + 确定性排序」化解，不靠「谁说最后话」。

### git-appraise
- **合并 = git notes 的行级并集**：`git notes --ref ... merge <remote> -s cat_sort_uniq`（`ref/repository/git.go` 的 `MergeNotes`，常数 `cat_sort_uniq`；`PullNotes` 同）。
- 前提：每条 note 是一个 JSON 且恰好一行、无内嵌换行，才能安全地按行 sort+uniq 合并（README Metadata 一节说明）。
- 语义冲突（两个人同时改同一 review 的 request）用「时间戳大者胜」解决——可能静默丢弃一方。

> **对 Pipi 的启示**：`cat_sort_uniq` 只适合「纯追加的评论流」，不适合改标题/改状态这类有覆盖语义的编辑（会静默丢写），所以论坛的写入要像 git-bug 一样做 CAS + 操作日志；跨端同步若需要，分叉用「保留两边 + 确定性排序」而不是「后者覆盖」。

---

## 5. CLI 设计

### git-bug（cobra，`ref/commands/root.go`）
- 命令分组：Entities（`bug` / `user` / `label`）、Interactive（`termui` / `webui`）、Remote（`pull`/`push`/`bridge`）。
- 约定（`ref/doc/design/cli-convention.md`）：`xxx` 列举 / `xxx new` 新建 / `xxx rm` 删 / `xxx show [ID]` 查看（无 ID 用「隐式选择」）/ `xxx select|deselect`。
- 参数：`bug` 支持查询语言（`status:open sort:edit`）、flag 与自然语言混用（`ref/commands/bug/bug.go`）；查询语法文档 `ref/doc/usage/query-language.md`，解析器 `ref/query/parser.go`。
- **人可读 + 机器可读**：`-f/--format` 取 `default|plain|id|json|org-mode`（`ref/commands/bug/bug.go`），JSON DTO 在 `ref/commands/cmdjson/bug.go`（`BugSnapshot` / `BugExcerpt`）。
- 错误与退出码：`RunE` 返回 error，`SilenceUsage=true`（`ref/commands/root.go`）；进程层 `os.Exit(1)`（`ref/main.go`）。错误消息经 `env.Err` 走 stderr（`ref/commands/execenv/env.go`）。
- UI 关系：**CLI/TUI/Web 都走同一个 `cache` 层**（`ref/doc/design/architecture.md`「the commands package will not directly use the bug or repository package」）；TUI 在 `ref/termui/`，Web = GraphQL API（`ref/commands/webui.go`、`ref/api/graphql/`）+ Svelte 前端（`ref/webui/`）。

### git-appraise（手写命令表，`ref/commands/commands.go`）
- 子命令：`request / list / show / comment / accept / reject / abandon / rebase / submit / pull / push`。
- 输出：默认人可读模板（`ref/commands/output/output.go`），`-json` 打印 pretty JSON（`ref/commands/list.go`、`show.go`）。
- 退出：`fmt.Println(err)` 到 **stdout**（不是 stderr）后 `os.Exit(1)`（`ref/git-appraise/git-appraise.go`）——这点比 git-bug 粗糙。
- 无 TUI；Web 是外部的 `git-appraise-web`。

> **对 Pipi 的启示**：命令名按 git-bug 约定（`xxx`/`new`/`rm`/`show`/`select`）；**每条列举/查看命令都要有 `--format json`**，DTO 与前端类型同源；错误走 stderr、非零退出。UI 与 CLI 共用一层领域服务，别让 UI 绕过。

---

## 6. 搜索与列举

### git-bug
- **有索引**：SQLite FTS5，每 namespace+版本一张 documents 表（`ref/repository/index_sqlite.go` 的 schema 注释）。分词在 `ref/repository/index_text.go`：英文 Porter2 词干 + 停用词，CJK 用二元组（`数据库`→`数据 据库`）。
- 列举/过滤走 **excerpt 缓存**（缓存索引里的摘要 JSON），不是每次扫 git：`ref/cache/subcache.go`（`Load`/`sync`）、`ref/cache/bug_subcache.go`。
- `Query` 先拿索引做全文命中，再用 `compileMatcher` 对 excerpt 做结构化过滤（状态/作者/标签/标题/参与者），最后排序（`ref/cache/bug_subcache.go` 的 `Query`、`ref/cache/filter.go`）。
- 索引是本地派生态，可重建；`builtFrom` 记录每个 excerpt 的基准 commit，用于增量 sync（`ref/cache/subcache.go`）。

### git-appraise
- **无索引**：`review.ListAll` → `unsortedListAll` → `repo.GetAllNotes` 每次全量扫 notes（`ref/review/review.go`、`ref/repository/git.go` 的 `GetAllNotes`）。
- 为性能把「每 commit 一次 git 调用」压成 3 次 `git cat-file --batch`（`GetAllNotes` 注释详述）。过滤/排序在内存里做（`ListOpen`、按时间戳排）。

> **对 Pipi 的启示**：列举不要每次扫 git；索引放本地（可重建，不进 git），且要区分「全文命中」与「结构化过滤」两层，与 git-bug 一致。

---

## 7. 生命周期

### git-bug
- 编辑：追加新 Operation（改标题 `op_set_title.go`、改状态 `op_set_status.go`、改评论 `op_edit_comment.go`），历史不丢。
- 删除：`bug rm` → `Remove` 删本地 ref **和所有 remote 的 tracking ref**（`ref/entity/dag/entity_actions.go` 的 `Remove`/`RemoveAll`，命令 `ref/commands/bug/bug_rm.go`）。
- 关闭：状态操作 `SetStatusOp(Closed)`（`ref/entities/bug/op_set_status.go`），不是物理删除。
- 归档/清空：`wipe` 清掉所有实体 ref + git config + 本地缓存（`ref/commands/wipe.go`）。
- 导入/导出/同步：`pull`/`push`（`ref/commands/pull.go`、`push.go`，走 ref）+ `bridge`（GitHub/GitLab/Jira 等，`ref/commands/bridge/`、`ref/bridge/`）——外部导入导出是桥，不是 bundle。

### git-appraise
- 编辑：追加带 `original` 的 note；评论树在读取时重建（`ref/review/review.go` 的 `buildCommentThreads`）。
- 关闭/接受/拒绝：追加 comment 或改 request（`accept.go`/`reject.go`），状态是推导的。
- 放弃：`abandon` 把 request 的 targetRef 置空（`ref/review/review.go` 的 `IsAbandoned`）。
- 提交：`submit` 做 merge/rebase（`ref/commands/submit.go`）。
- 归档：rebase 前把旧 head 加进 `refs/devtools/archives/reviews` 防止 GC（`ref/review/review.go` 的 `Rebase`、`ref/repository/git.go` 的 `ArchiveRef`）。

> **对 Pipi 的启示**：关闭/归档应是**状态**而非物理删除（保留可回放历史）；物理删除只做「本地 ref + tracking ref」（git-bug 的 `Remove`）；导出优先复用 git（push 特定 ref 即可），必要时再做外部桥。

---

## 8. 给 Pipi 的结论

### 该抄
1. **领域数据进 git 对象、用独立 ref namespace**（`refs/forum/<thread>`），与现有 session 载体一致：可 push/GC/签名，且不污染工作区。证据：`ref/entity/dag/operation_pack.go`、`ref/repository/common.go`。
2. **不可变操作日志 + 编译快照**（operation-log / event sourcing）：一次操作（发帖/回复/改标题/改状态/加标签）= 一个不可变记录，读时 `Compile` 出当前状态。证据：`ref/entities/bug/operation.go`、`ref/entities/bug/snapshot.go`。
3. **id = 内容哈希(sha256) + 随机 nonce**，天然幂等可去重、支持前缀引用。证据：`ref/entity/id.go`、`ref/entity/dag/operation.go`。
4. **顺序用 Lamport 时钟 + 拓扑校验 + 确定性兜底排序**，不信任墙上时钟。证据：`ref/entity/dag/entity.go` 的 `readSince`。
5. **写入用 CAS（compare-and-swap ref）**，冲突时重读重试；分叉保留两边而非覆盖。证据：`ref/repository/gogit_refs.go` 的 `UpdateRef`、`ref/entity/dag/entity_actions.go` 的 `merge`。
6. **本地可重建索引 + excerpt 缓存**，列举/搜索不扫 git；索引与领域数据分离。证据：`ref/repository/index_sqlite.go`、`ref/cache/subcache.go`。
7. **CLI 约定 + `--format json` 双输出**、UI 与 CLI 共用同层领域服务。证据：`ref/doc/design/cli-convention.md`、`ref/commands/cmdjson/bug.go`、`ref/doc/design/architecture.md`。

### 不该抄
1. **git-appraise 的 git-notes + `cat_sort_uniq` 做结构化编辑**：只适合纯追加的评论流，改标题/改状态会静默丢写（`ref/repository/git.go` 的 `MergeNotes`；`ref/review/review.go` 的「时间戳大者胜」）。
2. **git-appraise 无索引、每次全量扫 notes**：数据一多就慢（`ref/repository/git.go` 的 `GetAllNotes` 本身就是为掩盖这点才用 `cat-file --batch`）。
3. **git-bug 的 merge commit + 上一百万级时钟跳变防护 + 跨端 DAG 合并**：对「本地优先、单机为主、Agent 之间异步讨论」的论坛是过度设计；除非论坛真要跨机器乱序同步，否则别引入 DAG merge 复杂度（`ref/entity/dag/entity_actions.go` 场景 5、`entity.go` 的时钟攻击防护）。
4. **把索引/时钟提交进 git**：它们是派生态，提交会制造无谓 diff 和合并冲突（git-bug 也把它们放 `.git/git-bug/`，`ref/repository/compose.go`）。
5. **错误打到 stdout / 无机器可读输出**：git-appraise 的 `fmt.Println(err); os.Exit(1)`（`ref/git-appraise/git-appraise.go`）是反例。

### 存疑
1. **工作区可读 vs 藏进 ref**：git-bug/git-appraise 都藏进 ref，牺牲可读性换干净的 diff。Pipi 的 session 载体已经是「一轮一 commit、一条 entry 一个文件（blob 内容就是那一行 JSON）」，**建议论坛沿用同一模式**：数据进 git 对象（ref namespace），但**每个条目一个文件**，并额外提供一个只读的「工作区投影 / `forum show` 渲染」满足可读性与 diff/grep 友好；是否真的把投影 checkout 到工作区，等有具体阅读/编辑需求再定。
2. **是否复用 session 载体还是新开 ref namespace**：倾向新开 namespace（如 `refs/forum/*`），避免论坛写入触发会话分支的重建/压缩逻辑；待与 `pipi-session` 的 commit 时机对齐后再定。
3. **是否需要跨机器同步**：若论坛只在单机多 Agent 间用，则 Lamport/DAG 可简化成「单调序号 + commit 顺序」；若要跨机器，则保留 §8 该抄第 4 条。
4. **身份模型**：git-bug 的 PGP 签名身份较重；Pipi 可先用「Agent 名 + sessionId + runId」的轻身份（与现有事件 envelope 一致），签名留作可选。

### 推荐一句话
**载体选 git 对象 + 独立 ref（学 git-bug），不用 git-notes 的 append-覆盖（避 git-appraise）；工作区给只读投影，索引留本地。**
