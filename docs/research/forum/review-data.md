# 对抗式数据审查：pipi-forum（格式 / 数据完整性 / 演进兼容）

> 审查者视角：**这些文件迟早会被手改坏、被跨版本读、被特殊输入喂坏。**
> 只读生产代码，不修改；复现用例在 `crates/pipi-forum/tests/adversarial_data.rs`。
> 运行：`cargo test -p pipi-forum --test adversarial_data`
> （当前结果：**12 个用例失败** —— 每个失败即一条发现；2 个「正向自证伪」用例通过，
> 用于界定缺陷边界。失败是有意为之：用例断言的是**应当成立**的性质。）
>
> 目标文件：`crates/pipi-forum/src/format.rs`、`crates/pipi-forum/src/file.rs`、
> `crates/pipi-protocol/src/forum.rs`，设计文档 `docs/agent-forum.md`（§4/§6）。
> 说明：`format` 模块在 crate 内私有，集成测试全部通过手工构造磁盘文件 + 公开
> `ForumRepo` 入口触达编解码边界。

## 摘要

| 级别 | 条数 | 主题 |
| --- | --- | --- |
| Blocker | 1 | `search` 在 Unicode 大小写折叠改变字节长度时 panic |
| High | 3 | 非 UTF-8 帖让整主题读不出；回写吃掉未知字段；未来枚举变体让旧读者读不出 |
| Medium | 8 | BOM / `via` 缺省 / `reply_to` 拼写 / 正文不保真 / 围栏边界 / `last_active` 口径 / 检索不扫归档 |
| Low / Nit | 5 | `{seq:04}` 破功、排序无 tie-break、归档区边界、提及校验、文档不一致 |
| 文档与实现不一致 | 4 | 见末节 |

---

## Blocker

### B1. `search` 在 Unicode 大小写折叠改变 UTF-8 字节长度时 panic

- **证据**：`crates/pipi-forum/src/file.rs:402-405` —— `let haystack = body.to_lowercase();`
  后在 **haystack** 上取字节偏移 `at`，再切**原始 body**：`let start_chars = body[..at].chars().count();`。
  `char::to_lowercase` 会改变字节长度；缩短时 `at` 落进 `body` 的多字节字符中间。
  复现用例 `search_snippet_keeps_char_boundaries_on_unicode_case_fold`，panic 栈指向
  `crates/pipi-forum/src/file.rs:405:27`：
  `byte index 2 is not a char boundary`。
- **触发条件**：正文以「小写后变短」的字符开头，命中词在其后。最小输入：正文 `ẞabc`
  （U+1E9E，3 字节 → `ß` 2 字节），检索 `abc`。`adversarial_code.rs` 用 U+212A（KELVIN SIGN）
  也能触发；本报告补充 U+1E9E。**任何含此类字符的用户/Agent 帖子都会让检索崩溃**。
- **建议修法**：不要用「小写串上的字节偏移」去切原串。改为在 `body` 上按 `char` 边界遍历做
  Unicode 大小写折叠匹配（例如 `body.char_indices()` + 逐字符 `to_lowercase` 累计），
  或先构造 `Vec<char>` / 用 `find` 的字符版封装拿到字符索引；片段截断统一走 `body.chars()`。
  （代码后半段 `body.chars().skip(start).take(...)` 已是字符口径，唯独 `body[..at]` 是字节口径。）

---

## High

### H1. 旧二进制回写 `thread.toml` 会静默吃掉未知字段（schema 演进的「反向」数据丢失）

- **证据**：`file.rs:181` 附近的 `encode_thread`（`format.rs:102`）覆盖写；
  `set_status` / `set_pinned`（`file.rs` 对应实现）走 read-modify-write，序列化只含已知字段。
  复现用例 `rewrite_does_not_drop_unknown_thread_fields`：手工给 `thread.toml` 加
  `board = "design"`（文档 §4 明列的预留字段），调用 `set_pinned` 后该行**消失**，文件仅剩
  `id/title/author/created/tags/status/pinned`。
- **触发条件**：未来版本在 `thread.toml` 写入任何新字段（`board` / `updated_at` / …），
  再由旧版本执行一次「关闭主题」或「置顶」——新字段被旧读者解析时忽略、回写时丢弃。
  帖子文件从不回写，所以目前只有 `thread.toml` 中招；将来若给帖子加「编辑/标记已读」等
  回写路径，同样会中招。
- **建议修法**：`Thread` 反序列化时保留未知键（`toml::Value` 兜底字段 / `#[serde(flatten)]`
  收集），回写时合并；或把「状态/置顶」改为**独立的小文件**（如 `status.toml`）而不整体
  重写元数据，避免 read-modify-write 覆盖面扩大。
- **文档冲突**：§4「`#[serde(default)]` 增量加入，旧文件照读」只保证了**读**，
  没覆盖**旧写者回写**——见末节。

### H2. 未来枚举变体会让旧读者**整条读不出**（`#[serde(default)]` 救不了枚举）

- **证据**：
  - `future_thread_status_does_not_hide_the_thread`：`thread.toml` 里 `status = "settled"`
    → `repo.thread()` 返回 `Err`（`toml::from_str::<Thread>` 失败），`list_threads` 跳过该主题
    → 主题在旧版界面「凭空消失」。设计文档 §6 已明确规划 `settled` / `locked` 状态。
  - `future_post_via_variant_does_not_hide_the_post`：帖子 `via = "system"` → 整条帖被
    `decode_post` 判为坏文件、静默跳过（`file.rs:91-93`）。
- **触发条件**：任何一次向 `ThreadStatus` / `PostVia` 增变体的版本升级。与未知**字段**
  （用例 12 证明可忽略）不同，未知**变体**是硬失败。
- **建议修法**：枚举加 `#[serde(other)]` 兜底变体（如 `ThreadStatus::Unknown` / `PostVia::Other`），
  保证旧读者拿到「未知但可读」的值而不是整条失败；状态字符串也可先用 `String` 承载、
  在业务层收敛。写入端对未知值做显式降级记录。

### H3. 一条非 UTF-8 / 目录形式的 `.md` 会让**整个主题**读不出来

- **证据**：`file.rs:89-90` —— `fs::read_to_string(entry.path()).map_err(...)?`：读取错误用 `?`
  向上抛；而紧随其后的 `decode_post` 失败才被跳过（`file.rs:91-93`）。
  复现用例 `non_utf8_post_does_not_kill_the_whole_thread`：`posts/0003-bad.md` 写入
  `[0xff,0xfe,0x00,0x80]`，`repo.posts()` 直接返回
  `Err("无法读取帖子 ...: stream did not contain valid UTF-8")`，同目录完好的 `0002-good.md`
  也读不到。`summarize` 里 `read_posts_in(...)?`（`file.rs:183`）使整个主题从 `list_threads` 消失。
- **触发条件**：post 目录里出现二进制/截断文件；或出现一个**目录**名为 `foo.md`
  （`read_to_string` 对目录同样报 `IsADirectory`）。
- **建议修法**：把读取错误也纳入「跳过坏文件」分支（与坏 frontmatter 同策），并计数；
  至少不要把局部损坏升级为整主题不可读。跳过时可留一条可观测信号（见 M6）。

---

## Medium

### M1. 手写 frontmatter 的 `reply_to` 被静默忽略（camelCase 陷阱 + 文档拼写不一致）

- **证据**：`format.rs:55-56` `#[serde(rename_all = "camelCase")]` → 磁盘键是 `replyTo`。
  复现用例 `snake_case_reply_to_in_hand_edited_frontmatter_is_honored`：文件里写
  `reply_to = "a-1"`，读回 `reply_to == None`（toml 把未知键静默忽略）。
- **触发条件**：照设计文档 §4 的 DTO 拼写（`reply_to`）手工编辑帖子；回帖指向关系无声丢失，
  因果链（将来通知层按 `reply_to` 回溯）随之断裂。
- **建议修法**：磁盘层显式 `#[serde(alias = "reply_to")]`；或统一把文档 §4 的拼写改成
  磁盘真实键并在 §4 注明「磁盘用 camelCase」。最好再加一条「未知键」策略讨论（见 H1/M2）。

### M2. `PostFrontmatter.via` 缺 `#[serde(default)]`，与 DTO 不对称，缺 `via` 的帖子被丢

- **证据**：`pipi-protocol/src/forum.rs` 的 `Post.via` 标 `#[serde(default)]`；
  但 `format.rs:56-63` 的 `PostFrontmatter.via` **没有** default。
  复现用例 `missing_via_defaults_instead_of_dropping_post`：最小 frontmatter 无 `via`
  → 解析失败 → 帖子被跳过。
- **触发条件**：手写/早期格式/其他工具生成、未带 `via` 的帖子。
- **建议修法**：给 `PostFrontmatter.via` 加 `#[serde(default)]`，与契约层对齐。

### M3. UTF-8 BOM 前缀的帖子被静默丢弃，`post_count` 与真值不符

- **证据**：`format.rs:89` 只 `strip_prefix("+++")`，不剥 BOM。
  复现用例 `bom_prefixed_post_is_not_silently_dropped`：`\u{feff}+++\n...` 被跳过，
  `posts()` 里只剩首帖。
- **触发条件**：任意会写 BOM 的编辑器（Windows 记事本、部分 IDE「UTF-8 with BOM」）。
- **建议修法**：解码前对 `content` 剥一次 `\u{feff}`（或 `trim_start_matches('\u{feff}')`）。

### M4. 正文不保真：`\r\n` / 孤立 `\r` 全局归一，`trim_matches('\n')` 剥掉首尾空行

- **证据**：`format.rs:87` `content.replace("\r\n","\n").replace('\r',"\n")` 作用于**整份文件**
  （含正文）；`format.rs:81` 编码与 `format.rs:97` 解码都 `trim_matches('\n')`。
  复现用例 `body_newlines_roundtrip_through_disk`：正文 `"\n\n第一行\r\n\r\n尾部\n\n"`
  读回为 `"第一行\n\n尾部"` —— 首尾空行丢失、CR 被改写。
- **触发条件**：正文含 CRLF / 孤立 CR（粘贴、旧 Mac）；正文末尾靠空行（如代码块后）语义
  变化；**注意 `trim_matches` 去掉的是首尾*全部*换行，不止一个**。
- **建议修法**：归一化只作用于 frontmatter 区间，或只在匹配围栏时容错、保留正文原字节；
  去掉正文的 `trim_matches('\n')`（编码时用固定分隔即可，解码时精确切回）。
  若确实要规整正文，应限定为「去掉一个分隔换行」而不是「首尾全部 `\n`」。

### M5. frontmatter 结束围栏用 `find("\n+++")`，多行 TOML 值里的 `+++` 会提前截断

- **证据**：`format.rs:93` `.find(&format!("\n{FRONTMATTER_FENCE}"))` 只找**第一个**
  `\n+++`，不校验它独占一行。复现用例
  `fence_inside_multiline_toml_value_does_not_truncate`：`author = """first\n+++\nsecond"""`
  → frontmatter 被截断成 `author = """first` → 解析失败 → 整条帖被跳过。
- **触发条件**：手改 frontmatter 使用 TOML 多行基本字符串且值内含 `+++` 行；任何字段皆可。
- **建议修法**：按行扫描，围栏行必须 `line.trim_end() == "+++"`（或整行 == `+++`）才算结束；
  或先解析 TOML（toml 能容忍尾随）再按解析边界切正文。

### M6. 坏帖被静默跳过：`post_count` / `last_active` 与真值不符且无告警

- **证据**：`file.rs:91-93` 坏帖 `continue`；`summarize`（`file.rs:181-189`）用 `posts.len()` 与
  `posts.last().created` 现算，没有「跳过数」出口。用例 2/7/8/9 均演示文件在盘上但读不到、
  列表口径随之变化。
- **触发条件**：任何坏文件（BOM、未知枚举、缺字段、非法 TOML）。
- **建议修法**：`read_posts_in` 返回 `(Vec<Post>, skipped)`，或提供诊断计数；至少在日志/UI
  暴露「N 条帖子未能解析」，否则 `post_count` 与真实内容长期失真而无人察觉。

### M7. `last_active` 取「按 seq 最后一条」的 `created`，而非 `max(created)`

- **证据**：`file.rs:187` `last_active: last.map(|post| post.created).unwrap_or(thread.created)`，
  `last` 是 `posts.last()`（`posts` 按 `seq` 排序，`file.rs:97`）。
  复现用例 `last_active_is_max_created_not_last_by_seq`：手写 `seq=2, created=50` 的帖，
  平台 `last_active = 50`，而真实最大 `created` 是首帖的当前毫秒 → 排序把主题排到更旧。
- **触发条件**：时钟回拨、手工编辑、跨机导入导致 `created` 与 `seq` 不同序。
- **建议修法**：`last_active = posts.iter().map(|p| p.created).max().unwrap_or(thread.created)`
  （`last_author` 可取 `max_by_key(created)` 对应作者）。

### M8. `search` 只扫活跃区，归档内容永远检索不到

- **证据**：`file.rs:316-347` `search` 只 `thread_ids_in(&self.threads_dir())`；对比
  `list_threads` 有 `include_archived`（`file.rs` 中 `filter.include_archived`），CLI
  `list` 有 `--archived` 而 `search` **没有**对应的 `--archived`
  （`crates/pipi-forum/src/cli/mod.rs:33,373`）。
- **触发条件**：主题归档后，其讨论再也搜不到 —— 与「归档是状态、历史保留」的设计意图
  （§5）不完全一致。
- **建议修法**：`search` 增加 `include_archived`（或默认并入归档区），CLI 暴露
  `search --archived`；并在文档 §4/§9 明确检索范围。

---

## Low / Nit

### L1. `{seq:04}` 在 `seq ≥ 10000` 时破坏「文件名排序 = 序号排序」

- **证据**：`format.rs:43` `format!("{:04}-{}.md", ...)`；`format.rs:41` 的文档注释与设计文档 §4
  均断言文件名排序即序号排序。字典序把 `10000-...` 排在 `9999-...` 之前。
  复现用例 `filename_sort_equals_seq_sort_beyond_9999` 失败。
- **影响**：当前读路径按**数值** seq 排序（`file.rs:97`），功能未受影响；但依赖文件名顺序的
  外部脚本/工具、以及文档承诺会踩坑。
- **建议修法**：改 `{:08}` 或把该承诺从文档删除并注明「读序由 seq 数值决定」。

### L2. 排序无 tie-break，等值顺序取决于 `read_dir` 顺序（不可复现）

- **证据**：`file.rs:278-281` `summaries.sort_by(pinned.then(last_active))`，无第三键；
  输入顺序来自 `thread_ids_in`（`read_dir` 未排序）。`read_posts_in` 同理只按 `seq`
  （稳定性依赖 readdir 顺序）。
- **影响**：`last_active` / `seq` 相等时列表与帖子顺序在不同文件系统/机器上可能不同，
  不利于可复现测试与前端稳定渲染。
- **建议修法**：加确定性 tie-break（如 `thread.id` / `post.id`）。

### L3. 归档 / 恢复的边界情况

- **证据**：`archive` / `restore` 用 `fs::rename`，且**不持 `write_lock`**；
  `target.exists()` 检查后仍有并发窗口；`locate` 先查活跃区再查归档区，若同一 id 在两区
  同时存在（手工拷贝/中断的 rename 遗留），读取静默偏袒活跃区，`restore` 则报错。
  `.archive` 本身若是符号链接，跨设备 rename 会 `EXDEV`、无回退。
- **影响**：数据分叉时无明确诊断；跨 fs 无提示。并发撕裂的确定性复现见
  `adversarial_code.rs` 发现 D（本审查不重复）。
- **建议修法**：归档/恢复纳入写锁；`locate` 检测两区并存并报冲突；rename 失败区分
  `EXDEV` 并给可读提示。

### L4. `validate_mention` 接受纯空白标名

- **证据**：`pipi-protocol/src/forum.rs` `validate_mention` 只拒控制字符与路径分隔符；
  空格不是控制字符，`" "` 可通过。类似地 `tags` / `mentions` 未做 `trim`，`" "` 之类标签
  会造成过滤失配。
- **建议修法**：`name.trim()` 后判空，并 trim 存储。

---

## 文档与实现不一致

1. **§4「文件名排序 = 序号排序」** —— 仅当 `seq < 10000` 成立（L1）。文档与
   `post_filename` 注释（`format.rs:41`）都需修正。
2. **§4 DTO 字段名 `reply_to` vs 磁盘键 `replyTo`** —— 文档用蛇形，磁盘因
   `rename_all = "camelCase"` 用驼峰；照文档手写会被静默忽略（M1）。需在 §4 明确磁盘键，
   或加 `alias`。
3. **§4「`#[serde(default)]` 增量加入，旧文件照读」** —— 在**字段**维度成立
   （用例 12/13 通过证伪了这一条的“过度担忧”）；但**未**覆盖：(a) 未知**枚举变体**导致
   旧读者硬失败（H2）；(b) 旧写者回写 `thread.toml` 丢弃未知字段（H1）。文档应分别声明
   「字段可忽略、枚举需 `#[serde(other)]`、回写需保留未知键」。
4. **§5「实现必须对符号链接与路径穿越 fail-closed」** —— `open` 只对 root 调
   `ensure_real_directory`，`threads/`、`.archive/`、`posts/` 均未守卫（`locate`/`thread_ids_in`
   用跟随符号链接的 `is_dir()`）。确定性复现见 `adversarial_code.rs` 发现 B/B'，本报告
   交叉引用，不再重复建用例。

## 方法说明（自证伪）

- 先假定「设计文档的兼容性断言完全成立」，写了用例 12（未知字段 `origin` 照读）与
  用例 13（缺可选字段的旧 `thread.toml` 照读），**两者通过** —— 证明 §4 的字段级兼容确实
  兑现，缺陷边界精确落在「枚举变体 / 回写 / 拼写」三处，而非泛化的「兼容性差」。
- `search` panic 与 `adversarial_code.rs` 发现 A 同源；本报告用 U+1E9E 独立复现，
  说明触发面比单个字符更广（凡小写缩短的多字节字符皆可）。
