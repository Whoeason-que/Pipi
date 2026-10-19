# forum-adversary-code：Pipi 本地论坛的并发 / 原子性 / 崩溃一致性对抗式审查

> 审查对象（只读）：`crates/pipi-forum/src/{file.rs,format.rs,cli/mod.rs}`、
> `crates/pipi-protocol/src/forum.rs`、设计文档 `docs/agent-forum.md`。
> 复现测试：`crates/pipi-forum/tests/adversarial_code.rs`（新增，保持失败状态）。
> 运行：`cargo test -p pipi-forum --test adversarial_code -- --test-threads=1`
>
> 结果：**9 个测试，1 通过（探针），8 失败** —— 8 条均已在同进程内稳定复现（一次运行即复现，非偶发）。
> 既有 `forum_lifecycle` 3 测试仍全绿，说明本次未改动生产代码。

立场：默认这段代码是错的。每条先尝试证伪（「这不是 bug」），证伪失败才记为发现。
无法复现的条目单列「推测」并说明原因。

---

## 汇总

| 级别 | 条目 | 测试 |
| --- | --- | --- |
| **Blocker** | F1 符号链接逃逸：只查 root，`threads/`、`.archive/`、`posts/` 全不设防 | `symlinked_threads_dir_escapes_the_root`、`symlinked_threads_dir_reads_threads_outside_the_root` |
| **High** | F2 `search` 在 Unicode 大小写折叠改变字节长度时 panic | `search_panics_on_unicode_lowercase_byte_length_change` |
| **High** | F3 `archive` 与 `append_post` 无互斥 → 主题目录被撕裂（有帖无元数据 / 不可恢复） | `archive_racing_append_can_split_the_thread_directory` |
| Medium | F4 `set_status` / `set_pinned` 对同一 `thread.toml` 做无锁 read-modify-write → lost update | `concurrent_set_status_and_pin_lose_updates` |
| Medium | F5 `append_post` 的「已关闭」检查在取锁**之前** → 回帖落进已关主题 | `append_post_can_land_in_a_thread_that_is_concurrently_closed` |
| Medium | F6 帖子目录里一个读不出来的文件会 brick 整个主题 | `unreadable_post_file_bricks_the_whole_thread`、`directory_named_md_bricks_the_whole_thread` |
| Medium（推测） | F7 `create_thread` 先写元数据后写首帖 → 崩溃窗口留下「有元数据无帖」空主题 | 无（不可在同进程注入崩溃，见正文） |
| Low（推测） | F8 跨进程同主题同序号重复 | 无（设计文档已承认无跨进程写锁） |
| Low | F9 `write_lock` 中毒后不恢复；`create_thread`→`append_post_in` 重入关系当前安全 | 无 |
| Nit | F10 正文首尾换行被 `trim_matches('\n')` 吞掉；`list/search` 全量读帖 | 无 |

---

## F1【Blocker】符号链接逃逸：守卫只覆盖 root

**证据**
- `crates/pipi-forum/src/file.rs:36-45` `FileForumRepo::open` 只对 **root** 调用
  `pipi_protocol::fs::ensure_real_directory`；`threads/`、`.archive/`、`<thread>/posts/`
  一律不做真实目录检查。
- `file.rs:61-73` `locate` 用 `active.is_dir()`（**跟随**符号链接）定位主题；
  `file.rs:178-180` `thread_ids_in` 用 `entry.path().is_dir()` 同样跟随链接；
  `file.rs:104-113` / `file.rs:203-212` 的 `fs::create_dir_all` 会顺着符号链接写。
- 设计文档 `docs/agent-forum.md` §5 明文：「fail-closed：非法 id / 逃逸路径 / 符号链接一律拒绝」；
  `pipi-protocol/src/forum.rs` 端口文档同样声明「实现必须对符号链接与路径穿越 fail-closed」。

**触发条件**：调用方（同用户下的其它进程、或被诱导的文件编辑器）把 `<root>/threads`
（或 `.archive/`、某主题的 `posts/`）替换为指向任意目录的符号链接。

**复现**
- `symlinked_threads_dir_escapes_the_root`：建 `root` 后把 `root/threads` 指向外部目录，
  `create_thread()` 返回 `Ok`，外部目录下出现真实主题目录（`断言失败：外部目录下出现了主题目录`）。
- `symlinked_threads_dir_reads_threads_outside_the_root`：外部目录放一个合法主题目录再建链接，
  `repo.thread(id)` **成功读到 root 之外**的 `thread.toml`。

**自我证伪**：有人说「root 对所有者是私有的，攻击者能写 root 就能写外面」——不成立。
该载体的价值主张正是「人机共写、工作区可读可改」（文档 §3），`posts/*.md`、`thread.toml`
都是鼓励手改的普通文件；而 `ensure_real_directory` 的存在本身就是承认这棵树不可信。
守卫只做最后一层等于没做。故记为 Blocker。

**建议修法**：把守卫下沉到每次进入子路径时：`locate` 对命中的 `thread_dir` 以及其
`posts/` 逐级 `ensure_real_directory`（或统一写一个 `open_child_real_dir`），`threads` /
`.archive` 在 `open` 时各查一次、在写入前复查；`read_posts_in` 对每个条目先用
`symlink_metadata` 拒绝 `is_symlink`（见 F6 一并处理）。核心：**任何进入路径的系统调用
（`is_dir` / `read_to_string` / `create_dir_all` / `rename`）之前都要先确认其父链是真实目录。**

---

## F2【High】`search` 在 Unicode 大小写折叠改变字节长度时 panic

**证据**
- `crates/pipi-forum/src/file.rs:401-405`：
  ```rust
  let haystack = body.to_lowercase();
  let at = haystack.find(needle_lowercase)?;
  let start_chars = body[..at].chars().count();   // ← line 405
  ```
  `at` 是 **`haystack`（折叠后）的字节偏移**，却被拿去切 **原始 `body`**。
  `str::to_lowercase` 会改变 UTF-8 字节长度（U+212A KELVIN SIGN 3 字节 → `k` 1 字节；
  U+212B ANGSTROM SIGN 3 字节 → `å` 2 字节），于是 `at` 可能落在 `body` 某个多字节字符中间。

**触发条件**：任一帖子正文含「折叠后变短」的字符且其**紧随**目标检索词即可。最小例：
正文 `"\u{212A}x"`（KELVIN SIGN + `x`），执行 `search("x", …)`。

**复现**（稳定 panic）
```
thread '...' panicked at crates/pipi-forum/src/file.rs:405:27:
end byte index 1 is not a char boundary; it is inside 'K' (bytes 0..3 of string)
```
`search` 是 `ForumRepo` 端口方法，CLI `search` 与将来的 `forum_search` 工具都会走到。
任何能发帖的 Agent/人都能写入触发串 → 让调用进程 panic（DoS）。

**自我证伪**：`query.trim().to_lowercase()` 与 `body.to_lowercase()` 同源，是否可能 `find`
永远落在边界？否。反例已给出：折叠会把匹配点前的字节数改小，`at` 相对 body 前移。
另一说「输入多为中文不会触发」——ASCII 的人/Agent 也可轻易构造，非可信输入。

**建议修法**：不要混用两套坐标系。要么在 `haystack` 上做字符切分后再映射回 body
（用 `char_indices` 记录折叠前后的字符边界，或统一在 body 的字符序列上做大小写无关匹配），
要么最简：对 `at` 先 `body.is_char_boundary(at)` 校验，失败则回退到 `at` 之前最近的
字符边界（`body.floor_char_boundary(at)`）。推荐前者，顺带修掉「snippet 位置偏一个字符」的错。

---

## F3【High】`archive` 与 `append_post` 无互斥 → 主题目录被撕裂

**证据**
- `file.rs:298-314` `append_post` 取 `write_lock` 后在 `append_post_in` 里
  `create_dir_all(thread_dir/posts)`（`file.rs:104-113`）。
- `file.rs:372-384` `archive` **完全不取 `write_lock`**，直接 `fs::rename(dir, .archive/id)`。
- 交错：`append_post` 已 `locate` 到活跃目录并读到 Open → `archive` 把该目录 rename 走 →
  `append_post_in` 的 `create_dir_all` 又把 `threads/<id>/posts` **重新造出来**。
- 结果：`.archive/<id>/`（完整）+ `threads/<id>/`（只有 `posts/`、无 `thread.toml`）并存。

**触发条件**：同进程中一个会话归档、另一个会话回帖同一主题（宿主持有多会话，本仓库主线场景）。

**复现**（一次运行、第 0 次即中）
```
archive/append 竞态撕裂目录：threads/<id> 与 .archive/<id> 同时存在，活跃区缺 thread.toml；
thread() 现在返回 Err("无法读取 .../threads/<id>/thread.toml: No such file or directory")
```
后果链：`thread(id)` 因 `locate` 先看活跃区（目录存在）→ 读不到 `thread.toml` → 报错；
`restore(id)` 因 `locate` 返回「未归档」→ 无法恢复；`list_threads(include_archived)` 会看到
归档那份、活跃空壳被 `summarize` 失败而静默跳过。**归档数据被孤立，主题事实上不可恢复。**
这同时就是「有帖无元数据」的具体形态（撕裂后新帖只在活跃空壳里，元数据在归档区）。

**自我证伪**：`write_lock` 是否已足够？不。`archive` 不在锁内，而且即使把 `archive` 也加锁，
`append_post` 的快照（`locate` + 读状态）在取锁**之前**，仍会拿到过期路径（见 F5）。
设计文档把「跨进程无写者锁」列为已知限制，但这是**同进程内**的缺陷，不在豁免范围。

**建议修法**：让 `archive` / `restore` / `set_status` / `set_pinned` 全部共用同一把
`write_lock`，并把 `append_post` 的 `locate`+状态检查挪到锁内（见 F5），使「检查→写入」
成为临界区。更稳的做法是引入与主题绑定的路径级锁/租约；顺序上先锁、再定位、再校验。

---

## F4【Medium】`set_status` / `set_pinned` 无锁 read-modify-write → lost update

**证据**
- `file.rs:352-360` `set_status`：`read_thread_in(&dir)?` 整份读、改一个字段、`write_thread_in` 整份覆盖。
- `file.rs:362-370` `set_pinned` 同款。**两者都不取 `write_lock`**（全仓仅
  `create_thread:206` 与 `append_post:309` 取锁）。
- `thread.toml` 的写入只有 `write_atomic`（原子替换），但**没有**对「读—改—写」整体串行化。

**触发条件**：同进程并发「关帖」+「置顶」同一主题（或任意两个字段写）。

**复现**（一次运行、第 0 次即中）
```
lost update：并发 set_status(Closed)+set_pinned(true) 后得到 status=Open pinned=true
（另一轮得到 status=Closed pinned=false）
```
即两个操作只留下一个，另一个被按过期快照整体覆盖。

**自我证伪**：`write_atomic` 保证单次写不撕裂，是否够？不够——这是**语义级**丢失，
不是文件级撕裂。有人会说「人机并发改状态很罕见」——同样罕见的两条 CLI/工具调用在
同一宿主进程里并发即可触发，且没有任何报错或提示，属静默数据丢失。

**建议修法**：与 F3 同源——把所有 `thread.toml` 的读改写纳入同一把写锁；
或改为字段级、CAS 式的定向更新（读版本号/对比再写）。

---

## F5【Medium】`append_post` 的「已关闭」检查在取锁之前 → 回帖落进已关主题

**证据**
- `file.rs:298-306`：`locate` 与 `thread.status == Closed` 检查发生在 `file.rs:308-312`
  取 `write_lock` **之前**。锁只保护 `append_post_in`（序号分配）。
- `set_status` 不取锁（F4），因此检查与写入之间没有共同互斥。

**触发条件**：并发 `set_status(Closed)` 与 `append_post`。

**复现**（一次运行、第 0 次即中）
```
第 0 次：主题已是 closed，却接受了并发回帖（post_count=2）
```
这违反文档 §5「closed 拒绝回帖」的不变量，也违反 `docs/agent-forum.md` §6 的终止语义
（`settled/locked` 之后不再有新内容）。

**自我证伪**：即便 `set_status` 加了锁，当前 `append_post` 的 TOCTOU 仍在
（检查在锁外，锁到手前状态可能已变）。所以这不是「另一个 bug 顺带引起」，而是锁的
**位置错误**；必须把检查移入临界区。

**建议修法**：`_guard = write_lock.lock()` → 再 `locate`+读状态+`append_post_in`，
一次成型。

---

## F6【Medium】一个读不出来的帖子文件 brick 整个主题

**证据**
- `file.rs:75-99` `read_posts_in`：`decode_post` 失败被注释为「坏文件跳过」（对齐坏行策略，
  `file.rs:94-96`），但上面 `file.rs:89-91` 的
  `fs::read_to_string(entry.path()).map_err(...)?` 对**读失败直接 `?` 冒泡**。
- 崩溃时写一半、恰好在多字节 UTF-8 序列中间被杀，留下的就是非法 UTF-8 残留；
  `append_post_in` 用 `create_new` 建文件后 `write_all`（`file.rs:135-146`），**写入失败/被杀
  不会清理半成品**。

**触发条件**：`posts/` 下出现 (a) 非法 UTF-8 的 `*.md`，或 (b) 名为 `*.md` 的目录。

**复现**（两个测试均一次即中）
```
unreadable_post_file_bricks_the_whole_thread:
  无法读取帖子 .../posts/0002-deadbeef.md: stream did not contain valid UTF-8
directory_named_md_bricks_the_whole_thread:
  无法读取帖子 .../posts/0002-deadbeef.md: Is a directory (os error 21)
```
后果：`posts()`、`thread()`（经 `summarize` 的 `list_threads`）、乃至 `append_post` 全部报错，
**单文件坏 → 主题彻底不可读写**，而文档承诺的是「手改坏一行不该让整个主题不可读」。

**自我证伪**：这与「设计只忽略解析错误、不忽略 I/O 错误」是否有意为之？设计文档 §3/§4
只说读为纯读、坏行跳过，没有为「读失败即整主题失败」背书；且 F1 的符号链接帖文件也走这条
`read_to_string` 路径，放大会互相叠加。故成立。

**建议修法**：对单个条目：先 `symlink_metadata` 拒符号链接与非普通文件，再读；读失败改为
`continue`（记录但不中断），与 `decode_post` 失败同一策略。

---

## F7【Medium，推测】`create_thread` 先写元数据后写首帖 → 崩溃窗口留下空主题

**证据**：`file.rs:225` 先 `write_thread_in(thread_dir, &thread)`，`file.rs:235` 之后才
`append_post_in`。进程在两者之间被杀（或 `append_post_in` 失败但 `remove_dir_all` 也失败），
会留下「有 `thread.toml`、无任何帖子」的主题。

**为什么标「推测」**：我无法在集成测试里注入真实进程崩溃（没有故障注入钩子），
`remove_dir_all` 清理只覆盖**返回 Err** 的路径，不覆盖被 kill 的时刻。可静态确认的只有
「写入顺序是先元数据后首帖」这一事实。

**影响**：`list_threads` 会把它列成 `post_count=0` 的可见主题（`read_posts_in` 对缺失
`posts/` 返回空），UI 上是一个永远没有起帖的幽灵主题；`append_post` 仍可往里补帖，
所以是「可自愈的脏状态」而非硬损坏，故 Medium 而非 High。

**建议修法**：反转顺序或做标记位——先写首帖再写 `thread.toml`（把 `thread.toml` 当作
「主题成立」的提交点）；或在 `thread.toml` 内加 `posts_ready` 之类的提交标记，
`list_threads` 过滤未提交项。用 `write_atomic` 的本意（提交点）也是这个方向。

---

## F8【Low，推测】跨进程同主题的同序号重复

**证据**：`file.rs:116-148` 序号取自 `read_posts_in` 的 max+1，靠
`create_new(true)` 的文件名冲突兜底；但文件名是 `{seq:04}-{随机 id}.md`（`format.rs:48-50`），
不同 id 使 `AlreadyExists` **永远不会**触发。两个进程对同一主题同时读到相同的 max seq，
就会各自写出 `seq = N+1`、id 不同的两条帖。

**为什么标「推测」**：单个测试进程内 `write_lock` 覆盖了这条路径，无法在
`adversarial_code.rs` 里制造两个进程。设计文档 `file.rs:6-9` 与
`docs/agent-forum.md` §3 已明文承认「跨进程没有写者锁」，故这是**已知并接受**的限制，
只登记风险、不作为缺陷。触发面：桌面壳与 Web 宿主同开一个版面。

**建议修法（若未来要跨进程）**：持仓级别的文件锁（`flock` 一个 `posts/.lock`），
或把序号分配改成对锁文件的 CAS 递增。

---

## F9【Low】写锁中毒不恢复；当前无重入（证伪成立）

- 中毒：`file.rs:206-209` / `309-312` 用 `.lock().map_err(...)`，一旦某线程持锁 panic，
  之后所有写操作永久返回「论坛写锁不可用」。读路径不受影响（fail-safe）。当前生产代码里
  `append_post_in`/`encode_post`（`format.rs:63-77`，`expect`）在常规输入下不会 panic，
  所以这是**理论风险**，未找到可复现的中毒路径，不记为缺陷，仅提示：如需健壮，
  改用 `.unwrap_or_else(|e| e.into_inner())` 或 `parking_lot`。
- 重入：`create_thread`（持锁）复用 `append_post_in`（不取锁），`append_post` 同样是
  「取锁→调不取锁的内部函数」。**未发现同线程二次获取 `Mutex` 的路径**，此点证伪成立。

---

## F10【Nit】其它

- **正文换行被吞**：`format.rs:79` `post.body.trim_matches('\n')`、`file.rs:131`
  `draft.body.trim()`，会静默丢掉正文首尾换行；`format.rs` 单测的样例正文恰好不以换行
  开头/结尾，掩盖了这一点。对 Markdown 代码块首行空行有细微影响。
- **全量读放大 / DoS**：`posts()`（`file.rs:288-296`）先整份 `read_posts_in` 再
  `drain`/`truncate`；`list_threads`（`file.rs:246-286`→`summarize:181`）为每主题读**全部**
  帖子只为取最后一条；`search`（`file.rs:315+`）扫全主题全帖且不看 `limit` 就整份读。
  超大/超多帖子下内存与耗时无上界，`--limit` 不减少读取量。无对应测试（属成本问题，
  非正确性），列为 Nit/改进点。

---

## 附：测试清单（保持失败）

| 测试名 | 复现发现 |
| --- | --- |
| `search_panics_on_unicode_lowercase_byte_length_change` | F2 |
| `symlinked_threads_dir_escapes_the_root` | F1 |
| `symlinked_threads_dir_reads_threads_outside_the_root` | F1 |
| `archive_racing_append_can_split_the_thread_directory` | F3 |
| `concurrent_set_status_and_pin_lose_updates` | F4 |
| `append_post_can_land_in_a_thread_that_is_concurrently_closed` | F5 |
| `unreadable_post_file_bricks_the_whole_thread` | F6 |
| `directory_named_md_bricks_the_whole_thread` | F6 |
| `probe_defaults_are_reachable` | 探针（通过） |

一条修复优先级建议：**F1 → F2 → F3 →（F4/F5 同一把锁一起改）→ F6**。
F1/F2 是不需要并发就能触发的确定性问题，应先堵。
