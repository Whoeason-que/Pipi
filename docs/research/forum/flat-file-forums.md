# 纯文件论坛数据模型调研（面向 Pipi 的「Agent 本地论坛」）

> 调研对象：**Flatboard**（主）、`vertex101/flatboard`、`git-bug`（补充）。
> 结论按调研指令的 8 个问题组织，最后给出 Pipi 可执行的最小字段集与布局建议。
> 证据路径均相对仓库根 `/home/whoeason/Desktop/code/Pipi`。

---

## 0. 调研范围与一个必须先说明的事实

指令假定 `reference/flatboard/` 是「纯 JSON、无 SQL 的轻量论坛」。实际浅克隆下来的是
Flatboard **5.x 的 landing 仓库**，只有 `README.md`、图片与 `latest_version`，
**没有任何源码**（`reference/flatboard/README.md`，`git log` 只有一个提交链，`git log --all --name-only` 显示仓库历史上从未提交过 PHP 源码；见 `reference/flatboard/.git`）。

因此本报告做了两件事：

1. Flatboard 主对象的源码取自其官方 **release 附件**（`github.com/Fred89/Flatboard/releases/tag/4.0` 的 `flatboard_latest.zip`），
   解压到 `reference/flatboard-src-4.0/flatboard/`。**这是 Flatboard 最后一个纯文件（前一代）架构**：
   README 明确写「Flatboard 4 is the last supported version of the previous architecture」，
   而 5.x 才改成 MVC + JSON/SQLite。也就是说我们拿到的是**真正的纯文件版 Flatboard**，代价是它用
   PHP 数组而非 JSON 落盘（见第 2 节，这一点本身就是一个重要教训）。
2. 补充样本（各 1 个）：`vertex101/flatboard`（Node.js，**纯 JSON** 无数据库论坛，
   浅克隆于 `reference/vertex101-flatboard/`）与 `git-bug`（**git 对象为真源、SQLite 仅作 cache/index**，
   仓库中已有 `reference/git-bug/`）。这两者刚好补上 Flatboard 缺失的「JSON」与「分布式 git 持久化」两块。

> 复现：`curl -L .../releases/download/4.0/flatboard_latest.zip`（外层再套一层 `flatboard.zip`）。
> 主样本版本号见 `reference/flatboard-src-4.0/flatboard/init.php:15`（`VERSION='4.0'`）。

### 三个样本速览

| | Flatboard 4.0（主） | vertex101/flatboard | git-bug |
| --- | --- | --- | --- |
| 持久化 | 一实体一文件，**PHP 数组** `.dat.php` | 一实体一文件，**纯 JSON** | git blob/commit/tree（JSON 操作），SQLite 仅索引 |
| 布局 | `data/<type>/<id>.dat.php` 扁平分类型 | `projects/<p>/boards/<b>/threads/<t>/posts/<n>.json` 分层 | `refs/bugs/<id>` 提交 DAG |
| 写入语义 | 就地覆盖整文件 | 就地覆盖整文件 | append-only 操作，状态可重算 |
| 并发 | `LOCK_EX` + 静态 cache | 无锁 | Lamport 时钟 + DAG 合并 |
| 语言 | PHP 8 | Node.js/EJS | Go |

---

## 1. 领域模型（board / thread / post / user / tag / status）

### 1.1 Flatboard 4.0（`reference/flatboard-src-4.0/flatboard/`）

实体只有 4 类，全部走同一个 KV：`config`（含 `forumOrder`、`tags` 等「单例配置」）、
`forum`（版块）、`topic`（主题帖）、`reply`（回复）；`plugin` 是插件状态。证据：`lib/Flatdb.lib.php`
的 `readEntry/saveEntry`，以及散落的 `saveEntry('topic'|'reply'|'forum'|'config'|'plugin', ...)`。

**topic（主题帖）字段**（`add.php:25-36` 创建、`edit.php` 修改）：
```php
ip, role, tag, title, mail, content, view(int), forum(父版块 id),
reply(数组: 回复 id 列表), locked(bool), trip(显示名/身份), 
lastedit?(字符串), authoredit?(编辑者身份)
```
**reply（回复）字段**（`add.php:112-118`）：
```php
ip, role, content, topic(父主题 id), trip, mail, lastedit?, authoredit?
```
> 注意：**没有显式的 createdAt/updatedAt 字段**，时间信息完全编码在 id 里（见第 3 节）。

**forum（版块）字段**（`add.php:155-161`）：
```php
name, info(简介), font_icon, badge_color,
topic(数组: 主题 id 列表), pinnedTopic(数组: 置顶主题 id)
// private_forum 字段存在但被注释掉（add.php:159）
```

**tag**：不是独立实体，而是 `config/tags` 单例文件里的 map `id => {title, replace, hidden_tag, id}`（`config.php:413-424`），topic 上只存一个 `tag` 字符串（`add.php:27`）。

**status（置顶/锁定/精华）**：
- **锁定** = `topic.locked`（bool，`add.php:34`、`edit.php:29`）；
- **置顶** = `forum.pinnedTopic[id]`（不在 topic 上，`edit.php:47`）；
- **精华/已解决** 在核心不存在：`lang/*.php` 里有 `'solved'` 文案（`lang/en-US.php:265`）但代码无对应字段，属 Pro/插件能力。
  另有一个 `pinned_homepage` 文案（`lang/en-US.php:182`）也只是首页语义。

**user**：没有用户实体文件。身份是一个 **trip**（见第 4 节），角色放在 session，创建帖子时**冗余复制**一份到 `topic.role`/`reply.role`（`add.php:26,113`）。

### 1.2 vertex101/flatboard（纯 JSON）

层级模型更完整：`project > board > thread > post`，还有独立 `user` 实体。

- **user**（`lib/store.js` `saveUser`）：`{username, role, password(bcrypt), postCount, avatar}`，
  另有目录索引 `data/users/_index.json`。角色 `owner / mod / contributor / member`（`lib/auth.js`）。
- **board**（`store.js` `saveProject`）：`{id, title, description, threadCount, postCount, lastPostAt, lastPostBy}`——**冗余聚合统计**。
- **thread**（`store.js:createThread`）：`{id(nanoid), title, authorUsername, createdAt, lastPostAt, lastPostBy, postCount, locked, pinned, tags[], status}`。
- **post**（`store.js:createPost`）：`{id(int seq), authorUsername, createdAt, editedAt, body(Markdown), isOP}`。
- **status 是 board 级状态机**：`routes/threads.js` 顶部 `BOARD_STATUSES`——`bugs: open/confirmed/fixed/wontfix`，`features: open/planned/shipped`。即同一「主题」在不同版块有不同状态枚举。

### 1.3 git-bug

不使用「当前状态」模型，而是 **操作日志（operation-based CRDT）+ 可重算快照**：

- 编译后的快照字段（`entities/bug/snapshot.go:17-29`）：`Status, Title, Comments[], Labels[], Author, Actors[], Participants[], CreateTime, EditTime, Timeline[]`。
- 编辑动作是操作类型：`Create / SetTitle / AddComment / EditComment / SetStatus / LabelChange / SetMetadata`（`entities/bug/op_*.go`）。
- 状态只有 `OpenStatus/ClosedStatus`（`op_set_status.go`）。
- 每条操作的公共字段 `type, author, timestamp, (Lamport) time, nonce`（`doc/design/data-model.md`）。
- **id = hash(首个操作)**，内容寻址（`doc/design/data-model.md` "Entities and Operation's ID"）。

---

## 2. 存储布局

### 2.1 Flatboard 4.0 —— 按「类型」扁平分目录，一个实体一个文件

```
data/
  config/config.dat.php        # 站点配置单例
  config/forumOrder.dat.php    # 版块顺序（数组）
  config/tags.dat.php          # 标签表
  topic/<id>.dat.php           # 一主题一个文件
  reply/<id>.dat.php           # 一回复一个文件
  forum/<slug>.dat.php         # 一版块一个文件（slug = safeString(name)）
  plugin/<name>.dat.php
  ban/blocklist.txt            # 封禁名单，纯文本一行一个 IP/CIDR
  rate_limits/global_<md5(ip)>.json
  key.php                      # 管理员口令哈希
  index.php                    # "<?php exit;?>" 防目录浏览
uploads/  lang/  lib/  plugin/  theme/
```
证据：`init.php:38-41`（`DATA_DIR`、`BAN_FILE`），`lib/install.file:77-90`（安装时创建 `topic/reply/forum/plugin/config` 目录）。

**落盘格式不是 JSON**：`saveEntry` 写 `"<?php exit;?>\n" . var_export($data, true)`（`Flatdb.lib.php:121-131`），
读取时用 `file_get_contents(..., offset=14)` 跳过 14 字节的 `<?php exit;?>` 前缀（`Flatdb.lib.php:103`）——
本质是 PHP 数组字面量，靠 `eval` 解析，`<?php exit;?>` 只是防止直接访问时执行。
**这是最需要避免的「伪 flat-file」**（详见第 7 节）。

**列举/分页不读全量**：`listEntry(type)` = `readdir(data/<type>)` 只取文件名（`Flatdb.lib.php:168-181`），
`readEntry` 才读单文件；并有一层 5 秒 TTL 的静态缓存 `$readCache/$listCache`（`Flatdb.lib.php:21,39`）。
**代价**：`forum.topic[]` 与 `topic.reply[]` 是「父持子 id 列表」，所以**每次发帖/回复都要重写父文件**
（`add.php:39-40,121-122`），写放大 + git 冲突面很大。

### 2.2 vertex101 —— 分层目录 + 冗余统计

```
data/users/<username>.json  +  data/users/_index.json
data/projects/<pid>/project.json
data/projects/<pid>/boards/<bid>/board.json
data/projects/<pid>/boards/<bid>/threads/<tid>/thread.json
data/projects/<pid>/boards/<bid>/threads/<tid>/posts/<n>.json   # n = 1,2,3...
```
证据：`reference/vertex101-flatboard/lib/store.js` 的 path helper（`userPath/projectPath/boardPath/threadPath/postPath`）与 README「File structure」。
JSON 以 `spaces: 2` 落盘（`fs.outputJSON(..., { spaces: 2 })`），人可直接读改、可直接 git diff。
`posts/<n>.json` 用**递增整数**做 id，因此「按文件名排序即时间序」，列表页天然可只读少量文件。

### 2.3 git-bug —— 一切都进 git 对象，文件系统不出现业务文件

每个实体一个 `refs/<namespace>/<id>`；每次编辑 = 一个新 `Tree`（含 `/ops` blob、`/media` blob、`create-clock-*`/`edit-clock-*` 空 blob）挂在新 `Commit` 上，
多次编辑构成 commit 链，并发编辑合并成 DAG（`doc/design/data-model.md`）。
SQLite 只出现在 `.git/git-bug/index-v1.db`（`repository/compose.go:63`、`repository/index_sqlite.go:18-37`），
明确定位为**可重建的索引/缓存**，删掉不影响数据。

---

## 3. id 与时间

### Flatboard
- **id 自带时间**：`newEntry()` = `date('Y-m-dHis') . substr(uniqid(), -5)`（`Flatdb.lib.php:199-201`），
  例如 `2021-11-24153312ab3f9`。**排序靠 id**：`forum` 列表用 `array_reverse(pinnedTopic)`（`view.php:146`），
  回复用 `asort()`（`view.php:44`），首页最新用 `rsort(array_merge(topic..., reply...))`（`index.php`）。
- **时间解析**：`Util::toDate()` 取 id 前 16 字符按 `YYYY-MM-DDHHMMSS` 解析（`lib/Utils.lib.php` `toDate`）；
  「最后回复时间」= 最后一条 reply 的 id（`view.php:168-173`）。**没有独立时间戳字段**。
- 编辑时间才另存字符串：`lastedit = date('Y-m-d H:i:s')`（`edit.php:25,133`）。
- 版块 id 是名字的 slug（`Parser::safeString`，`add.php:162`）；tag 有独立 `newEntry()` id（`config.php:420`）。

### vertex101
- thread id = `nanoid(10)`；post id = `max(现有 id)+1`（`store.js:createPost`）。
- 时间一律 ISO-8601：`new Date().toISOString()`；排序按 `lastPostAt`、`createdAt`。

### git-bug
- 内容寻址 id = `hash(json(op))`，人类显示截断 7 字符；分布式排序不用物理时钟，而用 **Lamport 逻辑时钟**
  并遵守 DAG 因果（`doc/design/data-model.md` "Time is unreliable"）。

> 结论：Flatboard 的「id 即时间」是省字段、天然排序的巧思，但**牺牲了可读性与精度**（秒级 + 5 位随机，且格式不通用）。
> vertex101 的「ISO 时间戳 + 单调 seq」更稳。git-bug 的「物理时间只做展示、排序用逻辑时钟」值得在有并发/合并需求时借鉴。

---

## 4. 权限与身份

### Flatboard：session 角色 + trip 身份
- 角色只有两级：`admin`（`User::$ROLE_ADMIN`）与 `worker`（版主，`User::$ROLE_WORKER`），
  另有**匿名/访客**（无 session 也可发帖，只多一个 captcha）。证据 `lib/User.lib.php:25-28,106-140`；`User::login`（225/240 行）设置 `$_SESSION['role']`。
- **身份 = trip**：`HTMLForm::trip(name, id)` = 昵称（去符号）+ `@` + 盐哈希后 8 位；名为空时退化为 id 后 8 位（`lib/HTMLForm.lib.php` `trip`）。
  即「可选密码的轻量身份」：不注册也能发言，同一个 trip 即可认领自己的帖子。
- **两条鉴权路径**：
  1. 操作时读 **session**（`User::isAdmin/isWorker`）；
  2. 判定「是不是作者」用 trip 哈希比对条目里的 `trip`（`User::isAuthor`，`lib/User.lib.php:315-341`）。
- **谁能做什么**（`lib/EntryLink.lib.php`）：管理自己的 topic/reply = `isWorker() || isAuthor(...)`（138-152 行）；
  版块/标签/插件/封禁 = `isAdmin()`；**锁定/置顶/移动版块只有 worker/admin**（`edit.php:28-49`）。
- 帖子内冗余 `role` 字段，仅用于展示样式（`add.php:26`）。
- 封禁：`data/ban/blocklist.txt` 纯文本 IP/CIDR 列表；限流：`GlobalRequestLimiter` 按 IP 写 `rate_limits/global_<md5>.json`（`init.php:119`）。

### vertex101
`lib/auth.js` 给出清晰的能力中间件：`requireLogin / requireOwner / requireMod / requireContributor`；
注册的第一个账号自动成为 **owner**（README）。身份存在 session，帖子只存 `authorUsername`（引用 user 文件）。

### git-bug
作者是**独立的 identity 实体**（`entities/identity`），身份有多个 `Version` 且操作带签名，可跨机器 adopt/推送；比 Flatboard 的「trip 字符串」健壮得多。

---

## 5. 编辑 / 删除语义

### Flatboard：**就地改写 + 硬删，无审计**
- 编辑 = 整文件覆盖（`saveEntry` 即 `file_put_contents(..., LOCK_EX)`，`Flatdb.lib.php:121-129`），
  只补 `lastedit` 与 `authoredit` 两个字段，**没有历史版本**（`edit.php:25-26,133-134`）。
  权限：作者或 worker/admin（`edit.php:12,127`）。
- 删除 = `unlink` 硬删（`Flatdb.lib.php:147-160`），并且：
  - 删主题**级联删所有回复**（`delete.php:20-30`）；
  - 删回复时从 `topic.reply[]` 摘除（`delete.php:48-54`）；
  - 删版块级联删全部主题+回复（`delete.php:71-90`）。
  **没有墓碑、没有回收站、没有操作日志**；删错了只能靠备份。
- `topic.view` 在**每次浏览时 +1 并立即 saveEntry**（`view.php:35`）——「读」变成「写」。

### vertex101
`editPost` 就地改 `body` 并记录 `editedAt`（`store.js`），同样**无版本历史**；
`deletePost`/`deleteThread` 用 `fs.remove` 硬删，删掉最后一帖会顺带删主题（`store.js`）。

### git-bug
编辑/删除都是**追加操作**：`SetTitle/EditComment/SetStatus/...` 依次 `Apply` 到快照，
旧值仍在历史里；同名/同字段的编辑靠 Lamport 时钟与 DAG 合并确定最终值。这是「可审计、可合并、可回溯」的范本。

---

## 6. 搜索 / 分页 / 未读

### 搜索
- Flatboard：**全量线性扫描**。`search.php` 遍历 `listEntry('topic')`，逐条 `readEntry`，
  打分 `title=4~5 > tag=3 > author=2 > content=1`，命中上限 100 条，内容只截前 500 字符（`search.php:30-104`）。
  **没有只搜回复**、没有索引、没有增量。
- vertex101：`store.js:search()` 遍历所有 project→board→thread→post 做 `includes`，返回按 `lastPostAt` 排序，**同样全量**。
- git-bug：独立 SQLite FTS 索引（`repository/index_sqlite.go`），但它是**可重建的 cache**，不是真源。

### 分页
- Flatboard：在**内存数组**上分页。`Paginate::countPage/viewPage/pid`（`lib/Pagination.lib.php:70-107`），
  数据源是 `forum.topic[]` / `topic.reply[]`；页大小 `config.ItemByPage`（默认 8，`lib/install.file:148`），
  无限滚动由前端 ias 插件实现（`plugin/core/core.plg.php`）。
- vertex101：列表页一次性 `getThreads()` 读全部 thread.json 再排序，无服务端分页（小规模自用假设）。

### 未读 / 最近活动
- Flatboard 核心**没有已读/未读**；首页只有「最新 topic+reply」聚合（`index.php`）。
  **未读/收藏由官方 Notification 插件用「每用户一台账目录 + 空标记文件」实现**：
  `plugin/Notification/db/userList/<trip>/{readList,stared,hidden}/<entryId>`（证据：`Downloads/Notification.zip` 内 `Notification.plg.php`；
  来源 `github.com/XHiddenProjects/Flatboard`）。空文件存在与否即状态——极简、可 git、可手工清理。
- 「提及」核心也没有；官方 `Mention.zip` 只是前端正则把 `@name` 染色（`Mention/js/mention.js`），**不产生任何数据结构**。
- vertex101：`getRecentActivity()` 扫全量后按 `post.createdAt` 截断，供 RSS；无未读。
- git-bug：靠 SQLite 缓存做列表/查询，无「未读」概念。

---

## 7. 与「一切皆文件」的契合度

**天然契合（值得继承）**
- 一实体一文件、纯文本：可手改、可 `git diff`、可同步（Flatboard 的 `.dat.php` 内容可读，vertex101 的 JSON 完全可读）。
- **列举只看文件名**：`listEntry` 只 `readdir`（`Flatdb.lib.php:168`），`posts/<n>.json` 数字命名让「分页/取最新」不必读全量。
- 时间编码进 id：省字段、天然有序（Flatboard）、`_index.json` 目录索引（vertex101）。
- 用户级状态用空标记文件：未读/收藏/隐藏各一串空文件，零解析成本、天然 git 友好（Notification 插件）。
- git-bug 更进一步：真源是 git 对象，SQLite 只是 `.git/git-bug/index-v1.db` 里可删可重建的索引——这正是 Pipi 会话「git 分支为唯一载体」的同构做法。

**已知取舍 / 必须避开的坑**
- **私有格式**：Flatboard 的 `<?php exit;?>` + `var_export` + `eval` 是反面教材——不可跨语言、不可安全解析、不能给非 PHP 工具读写。Pipi 必须用 JSON。
- **父持子列表导致写放大**：`forum.topic[]`、`topic.reply[]` 每次发帖都重写整个父文件（`add.php:39-40,121-122`），并发下必然冲突。
- **读操作触发写**：`view ++` 每次都落盘（`view.php:35`）。
- **硬删 + 无版本**：删主题/版块级联 unlink（`delete.php`），编辑就地覆盖——无审计、无撤销，和「可 git 追溯」的目标相悖。
- **全量线性搜索**：数据一多就 O(N)，且不看回复正文。
- **秒级时间 id + 5 位随机**：高并发下有碰撞风险；跨语言也不通用。
- **角色冗余在条目里**：`topic.role` 记录发帖时的角色，之后权限变更不会回改，只能当展示用。

---

## 8. 给 Pipi 的结论

### 8.1 主题帖 / 回复的**最小字段集**建议

**thread**（一主题一目录一文件）：
```jsonc
{
  "id":        "01J8...ULID",   // 时间有序、跨语言、可做文件名
  "board":     "general",       // 版块 id
  "title":     "…",
  "author":    "agent-alice",   // Agent 名 / handle
  "createdAt": "2026-10-18T09:00:00Z",   // ISO-8601 UTC
  "updatedAt": "2026-10-18T10:30:00Z",   // 最后活动（回复/编辑），供列表排序
  "tags":      ["design"],
  "status":    "open",          // 可扩展状态机："open"|"resolved"|"closed"
  "pinned":    false,
  "locked":    false,
  "postCount": 3,               // 冗余聚合（读优化，需可重算）
  "lastPostBy":"agent-bob"
}
```
**post**（回复；一帖一文件，序号做文件名）：
```jsonc
{
  "id":        "0003",          // 目录内单调序号（或独立 ULID）
  "thread":    "01J8...ULID",
  "author":    "agent-bob",
  "createdAt": "2026-10-18T10:30:00Z",
  "body":      "…Markdown 正文…",
  "parentId":  "0001",          // 可选：回复某楼，支撑引用/树
  "editedAt":  null             // 就地编辑时更新；若要审计则改用追加修订
}
```
> 关键取舍：**时间用独立 ISO 字段 + id 用 ULID**（兼顾 Flatboard 的「id 即序」与规范可读，避免其私有时间 id 的坑）；
> **`postCount/lastPostBy/updatedAt` 视为派生**，与 git-bug 一样「可从源文件重算」，不写进权威状态（可放 cache）。

### 8.2 目录布局建议（与 Pipi「git 分支 + 一文件一 entity」一致）
```
<agent-home>/forum/                 # 或仓库内 .pipi/forum/，随工作区走
  boards/<board>/board.json         # 版块元数据（name/desc/tags 白名单）
  boards/<board>/threads/<threadId>/
      thread.json                   # 上表
      posts/0001.json               # 首帖 = 主题正文
      posts/0002.json               # 回复
  index/posts.idx.json              # 可选、可重建、可 gitignore 的派生索引（列表/搜索）
  users/<agent>.json                # 可选：角色、关注/订阅
  read/<agent>/<threadId>           # 空标记文件：已读（抄 Notification 插件）
```
- **列表页不读全量正文**：只读 `thread.json` 头 + `index/*.idx.json`（可重建）。
- **搜索**：先做「可重建的派生索引」（扫 thread/post 生成），失败/丢失时退化为线性扫描；不要把索引当真源，也**别让搜索成为唯一真源**。
- **未读/订阅**：沿用「每 agent 一台账目录 + 空标记文件」（`read/`、`watch/`），天然 git 友好、零解析。
- **写入并发**：避免父持子列表（不要 `board.threadIds[]` 每帖重写）；用目录 = 天然的集合，排序靠 `updatedAt` 派生索引。

### 8.3 该抄 / 该避

| 该抄（来自） | 该避（来自） |
| --- | --- |
| 一实体一文件、纯文本，可手改可 git diff（三者） | 私有格式 `.dat.php`/`eval`（Flatboard `Flatdb.lib.php:121`） |
| 按文件名列举、序号命名 `posts/0001.json` 天然分页（vertex101 / Flatboard `listEntry`） | 父文件里存子 id 数组，发帖重写整文件（Flatboard `add.php:39`） |
| id 编码时间以省字段排序（Flatboard `newEntry`）→ 改良为 ULID + 独立 ISO 时间 | 秒级时间戳 + 5 位随机 id、无 dedup 保证（Flatboard） |
| 列表/未读用「每用户空标记文件」台账（Flatboard Notification 插件） | 浏览即写 `view++`（Flatboard `view.php:35`） |
| 冗余聚合字段只当派生、可重算（vertex101 `board.json` 统计） | 冗余字段当真源、就地覆盖无审计（vertex101/Flatboard） |
| 就地编辑记 `editedAt`（vertex101） | 硬删 + 级联 unlink、无墓碑无回收站（Flatboard `delete.php`） |
| 状态机按版块类型定义（vertex101 `BOARD_STATUSES`） | 核心无「提及/订阅」，靠前端正则占位（Flatboard Mention 插件） |
| **操作日志 + 可重算快照，真源 append-only**（git-bug `doc/design/data-model.md`） | —— |
| **SQLite 仅作可重建索引/cache**（git-bug `repository/index_sqlite.go`） | 把 SQLite/索引当唯一真源 |
| **逻辑时钟定序、合并而非覆盖**（git-bug，适用于多 Actor 并发） | 用物理时钟的多方排序 |

**一句话**：Pipi 的论坛应把它当成「一组 git 友好、可重算的 JSON 文档」——
主题/回复一文件一实体、id 用 ULID、时间独立成字段、父子关系靠目录而非父文件数组、
未读用标记文件、搜索/列表用可重建派生索引；**编辑要么就地 + 审计字段，要么按 git-bug 走 append-only 操作日志**，
但都**不要**重蹈 Flatboard 的私有格式、写放大、读触发写与硬删覆写。

---

### 附：证据索引
- Flatboard 4.0 源码：`reference/flatboard-src-4.0/flatboard/`
  （`init.php`、`lib/Flatdb.lib.php`、`lib/User.lib.php`、`lib/HTMLForm.lib.php`、`lib/Pagination.lib.php`、
  `lib/EntryLink.lib.php`、`add.php`、`edit.php`、`delete.php`、`view.php`、`index.php`、`search.php`、
  `config.php`、`lib/install.file`、`plugin/core/core.plg.php`、`lang/en-US.php`）
- 上游 landing 仓库（无源码）：`reference/flatboard/README.md`
- vertex101/flatboard：`reference/vertex101-flatboard/{README.md,lib/store.js,lib/auth.js,routes/threads.js}`
- git-bug：`reference/git-bug/doc/design/data-model.md`、`entities/bug/{snapshot.go,op_*.go}`、
  `repository/{compose.go,index_sqlite.go}`
- Flatboard 官方插件（未读/收藏/提及）：GitHub `XHiddenProjects/Flatboard` 的 `Downloads/Notification.zip`、`Downloads/Mention.zip`
