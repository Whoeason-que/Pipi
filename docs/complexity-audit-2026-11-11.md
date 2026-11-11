# 复杂度审计（2026-11-11）

8 路只读并行审计，覆盖全部 14 个 crate 与前端 26.6k 行。目标：找出可删除 / 可统一的不必要设计、
可优化的写法，且不破坏核心功能。基线 commit `575ce6e`。

审计基线（子代理实测）：前端 `npm run test:frontend` 189/189、`tsc --noEmit` 干净；
`pipi-protocol/-session/-provider` 测试全绿；`pipi-tools/-sandbox/-forum/-mcp/-terminal`
`cargo check --all-targets` 无告警、`cargo xtask arch` 通过。**`cargo test -p pipi-venv` 在本机为红**，
根因是 0.1 的真实缺陷（不是环境噪音）。

标注：✓ = 主代理已人工复核；其余为子代理带 `rg` 证据的结论。

前两轮已落地 / 已否决的项（`abstraction-cleanup-2026-10-13`、`complexity-cleanup-2026-11-01`）
本轮没有重复提出；见 §5。

---

## 0. 先修：真实缺陷与潜在缺陷

### 0.1 Agent 自己的 `agent.toml` 被当作「项目层」二次加载，默认 workspace 直接失败 ✓

- 位置：`crates/pipi-venv/src/agents/contract.rs:193-217`（Agent 层）
  + `crates/pipi-venv/src/av/discovery.rs:65-113`（向上发现项目层）
  + `crates/pipi-venv/src/av/merge.rs:42-55`（非 Agent 层声明 memory 即拒绝）
- 现象：Agent 默认 workspace 是 `<agent>/workspace`，向上发现会命中**同一个**
  `agent.toml`；它以项目层身份再参与合并，`[resources].memory` 触发 fail-closed。
  本机 `~/.git` 是空目录时可直接复现：`av check /home/whoeason/.pipi/agents/Pipi` → exit 1。
  单线程跑 `pipi-venv` 测试时，首个 panic 还会毒化 `HOME_LOCK`，导致 23 个 `PoisonError` 级联。
- 修法：`collect_contract_layers` 发现项目层后按 canonical path 去重
  （排除 `layer.path == agent_layer.path` 的层），约 +3 行；补一条
  「workspace 位于 Agent 目录内且祖先有 `.git`」的回归测试。
- 不变量：fail-closed 规则不变，只是不再把同一个文件加载两次。
- 验证：`cargo test -p pipi-venv`、`av check <agent>`、`cargo xtask arch`。

### 0.2 ChatView 每次重挂都会抹掉右栏开关偏好 ✓

- 位置：`src/chat/ChatView.tsx:307-309`（挂载即 `setInspectorOpen(isTest && !narrow)`）
  对照 `:140-147`（初始化读 `pipi-inspector-open`）与 `:149-156`（注释明确要求跨重挂保留）。
- 后果：session 模式下 `isTest=false`，effect 在挂载后无条件写 `false`；随后的持久化 effect
  把 `"0"` 写回 storage —— 不只是本次关掉，用户的偏好被永久清除。App 用
  `key={current.name}-{chatKey}` 在切会话时重挂，所以从仓库树点分支必现。
- 修法：宽屏下不覆盖持久化值（首次挂载跳过，或仅在 `narrow` / `isTest` 时设置默认）；
  先确认产品期望是「记住用户选择」还是「宽屏强制展开」。
- 验证：`tsc --noEmit` + 打开右栏 → 切会话 → 仍开；窄屏进入应收起。

### 0.3 SkillsView 第二个 effect 会在挂载时把刚打开的右栏关掉 ✓

- 位置：`src/skills/SkillsView.tsx:46-49`。`:47` 在 `selected` 存在时 `setOpen(true)`；
  `:49` 的 effect 依赖数组 `[]`，挂载时无条件 `setOpen(false)`。同一次提交按顺序执行，后者获胜。
- 后果：`skillSelected` 在 App 中跨视图保留，从技能页离开再回来时右栏被关。
- 修法：合并为 `useEffect(() => rail.setOpen(Boolean(selected)), [selected])`（-1 行）。

### 0.4 search 的 `GroupGuard` 漏 `disarm`，可能对已回收 PID 再 `killpg`

- 位置：`crates/pipi-tools/src/search.rs:10-17`、`:111-118`；对照 `crates/pipi-tools/src/bash.rs:37-55`。
- 风险：`status?` 早退时进程组已 `kill` + `wait()`，守卫仍 armed，Drop 会再次 `killpg`
  ——`bash.rs:34-36` 注释描述的 PID 复用误杀风险。建议把 `GroupGuard` 提到
  `pipi-tools/src/process.rs` 共用，并在 kill+wait 后立即 `disarm()`。

### 0.5 `GitExec` 的「仓库级锁」实际是实例级，gc 与写者不互斥（已知缺口，仍未修）

- 位置：`crates/pipi-session/src/git/exec.rs:26-31`、`:65-72`；
  `GitSessionRepo::new` 每次新建 `Arc::new(GitExec::new(...))`（`git/mod.rs:50-54`）；
  宿主 6+ 个构造点（`runtime/store.rs:17-22`、`slots.rs:31/197`、`lifecycle.rs:185`）。
- 后果：同 Agent 两条并发会话各持不同 `Mutex`，`fast-import` 可与另一实例的
  `gc --prune=now` 并发；文档承诺的 repo 级互斥不成立。与既有记忆
  `pipi-git-gc-writer-gap` 一致。
- 修法：进程级 `Arc<GitExec>` 注册表（`OnceLock<Mutex<HashMap<PathBuf, Weak<GitExec>>>>`，
  键用绝对化路径），`Mutex<()>` 可进一步改 `RwLock<()>`（普通命令读、gc 写）。
  约 +15 行，中风险；新增「同一路径两个 repo 共享 exec」单测。

### 0.6 `GitExec` 初始化快路径绕过仓库目录符号链接守卫（安全收紧）

- 位置：`crates/pipi-session/src/git/exec.rs:203-206`：`if repo_dir.join("HEAD").is_file() { return Ok(()); }`
  在 `:211` 的 `ensure_real_directory` 之前返回；`is_file()` 会跟随符号链接。
  现有测试只把链接指向空目录（无 HEAD），因此没覆盖「链接指向已有仓库」。
- 修法：快路径前先 `ensure_real_directory`（多一次 `symlink_metadata`），补一条用例。

---

## 1. 批次 A：零 / 低风险删除与收窄

只改可见性或删无消费者代码，不改行为。

| 域 | 项 | 位置 | 净行 |
| --- | --- | --- | ---: |
| protocol | `GitSink` 从 `pub use` 移除（crate 外零引用） | `pipi-session/src/lib.rs:155` | -1 |
| protocol | `background_result_text/tagged_text` 改私有；`parse_mentions` 改私有 | `protocol/src/session.rs:505/537`、`forum.rs:285` | 0 |
| protocol | `GitExec` 的 4 个 pub 方法降 `pub(super)` | `session/src/git/exec.rs:80/164/173/188` | 0 |
| core | 删只被测试用的 `ApprovalBroker::resolve`（或改私有） | `approvals.rs:145-152` | -8 |
| core | `restore_agent` 的 `runtime` 参数未用 | `commands.rs:1187-1190` | -3 |
| core | 测试通知信封两处重复构造 → `NotificationEnvelope::test()` | `events.rs:195-205`、`commands.rs:471-479` | -6 |
| core | `Theme::as_str` 改私有；6 个 Args 去掉多余 `Serialize` | `settings.rs:23-30`、`commands.rs:507+` | 0 |
| core | 删纯转发的 `wake_session`（唯一调用点直调 `queue_background_results`） | `runtime/background_wake.rs:151/160-169` | -10 |
| core | 删 `JobHandle` 的 4 个纯转发方法（`job_id/belongs_to/is_active/snapshot`） | `background/mod.rs:102-125` | -12 |
| core | 删 `server::AppState.events` 字段（与 `runtime.events()` 同一份，测试已分叉） | `server/mod.rs:42/92-99/312/395` | -5 |
| core | `pub(crate) mod background;`（crate 外零消费者） | `lib.rs:31` | 0 |
| core | `McpManager::call` 只测试用且与 `call_scoped` 重复；删重复文档行 | `mcp/manager.rs:298-303` | -12 |
| core | `session_search` / `stats` 的 `pub const` 改私有 | `tools/session_search.rs:24-30`、`stats/mod.rs:32` | 0 |
| agent | 删 `CompactMode::as_str`（零调用） | `context/src/compaction/mod.rs:68-75` | -8 |
| agent | `run_compaction` 返回 `Result<(), String>`（bool 无消费者） | `agent/src/session/compaction.rs:74-81` | 0 |
| agent | 内联单调用 `drive_compaction` | `agent/src/session/run.rs:258-274` | -9 |
| agent | 一批只在本 crate 用的 `pub` 降可见性 | `context/src/{context.rs,retry.rs,stats.rs}` | 0 |
| venv | 删未使用的 `tempfile` 依赖 | `pipi-venv/Cargo.toml:29` | -1 |
| venv | 删只写不读字段：`SkillContent.total_bytes`、`InstalledSkill.dir/meta`、`Discovered.root` | `av/store.rs:331`、`av/install.rs:34/36`、`av/discovery.rs:48` | -14 |
| venv | 删死 re-export `prompt::SkillMeta` | `prompt/mod.rs:15` | -1 |
| venv | 删 `store::ensure_real_dir`（纯改名转发）、`notebook_path` 方法、`NEW_CONTRACT_SKELETON` 别名 | `av/store.rs:742`、`agents/registry.rs:38-42`、`skills/enable.rs:14` | -10 |
| tools | 删 `McpClient::ping`、`TerminalManager::default`、`ContentPart::ResourceText.mime_type` | `pipi-mcp/src/client.rs:289`、`pipi-terminal/src/manager.rs:28`、`pipi-mcp/src/protocol.rs:52` | -15 |
| tools | 删 Windows 不可达分支；`MAX_TERMINALS_PER_WORKSPACE` 收回 crate 内 | `pipi-sandbox/src/lib.rs:54-58`、`pipi-terminal/src/lib.rs:18` | -5 |
| tools | `ServerInfo/server_info()` 只有集成测试消费：删除或接进诊断（二选一） | `pipi-mcp/src/{protocol.rs:57-63,client.rs:293-295,652-664}` | -25 |
| 前端 | 删零引用 `IconArchive` / `IconTrash` | `src/icons.tsx:307-328` | -22 |
| 前端 | 删 22 个 en + 15 行 zh 零引用 i18n 键（含 `settings.archiveOriginal*`） | `src/i18n/locales/en/*`、`src/i18n/zh.ts` | -37 |
| 前端 | 删 `hasLoadedRef`、`AgentWorkbench` 死重置块、`LineCount`、`findMessages` | `forum-state.tsx:67/103`、`AgentWorkbench.tsx:204-217`、`WorkspaceSidebar.tsx:441`、`chat/find.ts:33` | -30 |
| 前端 | 死 CSS：`.archive-toggle-label`、`.diff-row.del .diff-sign`、`--glow-ok/--glow-warn/--bg-fade` | `agent-navigation.css:383`、`files.css:36`、`tokens.css:59-64/156-162` | -12 |
| 前端 | 收窄过度导出（`RIGHT_RAIL_BREAKPOINT`、`SESSION_STATUS_KEY`、`TerminalChunk` 等）与 `RightRail` 的 `icon/className` | `src/app/right-rail.tsx:8/77/92` 等 | -6 |

合计约 **-300 行**，全部可编译期暴露遗漏；无行为变化。

---

## 2. 批次 B：统一重复实现

| 域 | 重复 | 建议 | 净行 | 风险 |
| --- | --- | --- | ---: | --- |
| core | `HostSession` 字面量构造 4 处（load/create/test/fork） | `HostSession::new(...)` | -20 | 低 |
| core | 子运行进度转发 3 份（`child.rs:172/446`、`ephemeral.rs:122`） | `forward_progress(...)` | -34 | 低 |
| core | child / ephemeral 的模型标签与 `RunRequest` 装配 | `configured_model_label` + `run_prepared_session` | -35 | 中 |
| core | 会话/Agent 的归档与删除两对近复制；临时会话占用检查 3 次 | `run_session_file_op` / `remove_agent` / `ensure_temporary_idle`（闭包必须留在 sessions 锁内） | -25 | 低-中 |
| core | 普通运行与手动压缩的预算/策略/重键/spawn 装配 | 4 个 helper | -18 | 低 |
| core | `create_agent` 与 `run_agent` 的继承工具过滤逐字重复 | `select_inherited_tools` | -28 | 低 |
| core | `submit` 与 `submit_agent` 的任务登记块 | `new_job_paths` + `register_job` | -22 | 低 |
| core | shell / child Agent 终态收尾 16 行重复 | `persist_finished`（把「先落盘再置 finished」收成一处） | -12 | 低 |
| core | stats 的计价 + 四处累加两组 | `record_call` | -16 | 低 |
| core+protocol | `mcp/tool.rs:66-73` 的 `truncate_chars` 是协议层同实现的副本 | 直接用 `pipi_protocol::text::truncate_chars` | -8 | 零 |
| core+forum | `snippet_around` 两份逐字重复 | 提到 `pipi_protocol::text` | -18 | 低 |
| 全仓 | 字节→hex 至少 8 处 | `pipi_protocol` 加 `hex(&[u8])` | -20 | 低 |
| core | 审批审计 SHA-256 hex 两处 | `arguments_sha256` | -4 | 低 |
| core | `ProviderConfig` 两种 key 解析同构 | `resolve_api_key_with(lookup)` | -8 | 低 |
| core | `SessionInfo` 对 `SessionModes` 锁两次、建两个 key | `snapshot(&key) -> (mode, revision)` | -5 | 低 |
| protocol | `exec.rs:251-262` 的 `tail` 与 `text::tail_at_char_boundary` 重复 | 复用协议层 | -9 | 低 |
| protocol | `same_entries` / `journal::recover` 用 JSON 字符串比较条目 | `SessionEntry: PartialEq`，直接 `==` | -5 | 低 |
| protocol | `layout::entry_role` 与 `is_user_entry` 各自维护 user 判定 | `entry_role` 复用 `is_user_entry` | -4 | 低 |
| agent | `summarize` / `call_plain` 重试主循环几乎逐行重复 | `drive_plain(..., retry_truncated)`（保留文案与截断语义） | -18 | 中低 |
| agent | `find_keep_start` 与 `prune_cut_index` 是镜像算法 | `cut_index(messages, budget, fallback)` | -10 | 中 |
| agent | `record_question/approval` 同一「空闲追加 + 提交」模板 | `append_idle_entry` | -10 | 低 |
| agent | 工具输出字符/图片统计两处；编辑压缩三段 index→id 转换 | `content_stats` / `ids_at` | -14 | 低 |
| venv | `agent_material` 与 `load_declared_context_files` 的「读受包含文件」块重复 | `read_contained_text`（错误文案保持子串） | -20 | 低 |
| venv | `scan_agents` 与 `migrate_tool_lists` 各写一遍目录遍历 | `iter_agent_manifests` | -10 | 低 |
| venv | `merge_skill_scopes` / `merge_store_skills` 同一去重 | `merge_unique_skills` | -8 | 零 |
| venv | `civil_from_days` 两份、`short_hash` 两份、`lock_path_for` 两份 | 各收一处 | -30 | 低 |
| tools | `ToolContext` 测试夹具 8 份 | `src/test_support.rs` | -70 | 低 |
| tools | write/edit 的沙箱路径 + confined 判定重复 | 私有 `mutation_target(ctx, path) -> (PathBuf, bool)`（不要上移协议层） | -12 | 低 |
| tools | bash/search 的 `GroupGuard` 两份 | 提到 `process.rs`（同时修 0.4） | -6 | 低 |
| tools | bash spill 创建两份；edit 行切分扫描两份 | `spill_file` / `line_spans` 派生 | -20 | 低 |
| terminal | 同步/异步 teardown 阶梯两份 | 三个小步 + 各自 sleep | -15 | 低 |
| 前端 | dev-mock 的 session info 形状 4 份、stats 2 份 | `demoSessionInfo` / `DEMO_SESSION_STATS` | -25 | 低 |
| 前端 | `chat-runtime` toolResult 合并两份、工具条目 upsert 三份 | `mergeToolResultEntry` / `upsertEntry` | -16 | 低 |
| 前端 | `session-runtime` 手写 `\u0000`、`browser-sessions` 另造复合键 | 走 `session-key.ts` | -4 | 零 |
| 前端 | `platform.ts` 是全仓最后一处裸 localStorage try/catch | 走 `storage.ts`（补 `deleteStored`） | -5 | 低 |
| 前端 | 论坛作者徽章、Git 文件行、空态/加载态各写多份 | 同文件抽组件 / 提升 `RightRailEmpty` | -40~-70 | 中（视觉） |

合计约 **-550 行**（与批次 A 不重叠），其中约一半是纯抽取。

---

## 3. 批次 C：性能（有量级收益）

| # | 问题 | 位置 | 改法 | 影响 |
| --- | --- | --- | --- | --- |
| C1 | 每次 append 克隆整个缓冲 + 全量重写恢复文件 + 两次 entry 深拷贝，且在 async 线程上 fsync | `pipi-session/src/git/sink.rs:88-95`、`journal.rs:63-75`；调用点 `agent/src/session/events.rs:54-66` | 先 push 再写 `&self.buffer`，失败 `pop()`；`append_entry` 用 `push_owned`。净 +3，零行为变化 | 每个 MessageEnd 从 O(n²) 降到 O(n)；一轮 118 条时省掉大量深拷贝与磁盘写 |
| C2 | `cached_catalog` 每次命中深拷贝约 4.6 MB 目录 ✓ | `pipi-provider/src/catalog/mod.rs:488-511`；调用点 `runtime/model.rs:89-92`、`stats/pricing.rs:16` | 返回 `Arc<ModelCatalog>` | 每次 run / 每次统计省一次 4.6 MB 拷贝 |
| C3 | 列表路径克隆全部 message `TreePath`；partial-only 会话触发完整读 | `pipi-session/src/git/reader.rs:92-97`、`journal.rs:96-107`、`mod.rs:164-176` | 用引用 + 拆 `has_entries` | 1000 条消息约省 2000 次分配 |
| C4 | stats 页在 GTK 主线程串行 spawn git（最多 300 会话） | `stats/mod.rs:162-211` → `commands.rs:1146-1148` → `crates/pipi/src/chat.rs:242-247` | `agent_stats` 改 async + `spawn_blocking`（同目录 `list_icon_themes` 已有先例） | 消除统计页卡 UI |
| C5 | 后台任务每个输出 chunk 重新 open 两个文件 | `background/persist.rs:46-71`，调用点 `shell.rs:92-109` | 复用句柄。**需先确认归档/删除时移动目录的交互**（与压缩归档同款 fd 坑） | chatty 服务少大量 syscall |
| C6 | `load_persisted_snapshot` 每次列表全量解析 JSONL | `background/persist.rs:73-135`、`mod.rs:340-364` | 从尾部有界窗口读 | 长任务日志 MB 级时列表变快 |
| C7 | `edit` 每次 edit 全量归一化（N × 16 MiB NFKC） | `tools/edit.rs:107-114/275/284/287` | `apply_edits` 开头归一化一次 base | O(N×文件) → O(文件 + N×old_text) |
| C8 | `edit` 16 MiB 同步读在 async 线程；`memory list` 同步递归读目录 | `tools/edit.rs:482-493`、`memory.rs:66-75` | `spawn_blocking`（同文件写路径已这么做） | 不阻塞 tokio worker |
| C9 | 隐藏的 Inspector 仍对全部 entries 做多轮全量扫描 | `src/chat/Inspector.tsx:169-213`、`SessionFilesPanel.tsx:11` | memo 增加 `tab/hidden` 条件；files 面板 inactive 返回 null | 长会话流式时每帧少约 5 次 O(n) 扫描 |
| C10 | `agent_end` 归约 O(m×n) 且每次比较重建消息投影 | `src/chat-runtime.ts:759-774`、`:414-425`、`:676/708` | 投影提前求值一次；`activeIndex` 提到 reduce 外 | 轮末少一次可见卡顿 |
| C11 | MCP legacy 清理每轮执行一次（一次性迁移） | `mcp/manager.rs:171`、`cache.rs:107-149` | 每 Agent 每进程只清理一次 | 每轮少一次目录扫描 + 反序列化 |
| C12 | 子运行/后台每 100ms 无条件快照 128 KiB tail | `tools/bash.rs:332-346/165-167` | `OutputBuffer.version` 未变则跳过 | 长命令空转时省分配 |
| C13 | `dayLabel` 每次 new `Intl.DateTimeFormat`（单次渲染最多 ~92 次） | `src/stats/format.ts:69-75` | 按 locale 缓存 | 统计页渲染 |
| C14 | `WorkspaceRail` 的 memo 依赖每次新建的对象，拖拽栏宽即重算 | `src/files/WorkspaceRail.tsx:38`、`WorkspaceView.tsx:33` | 依赖 `patch` 或 memo `source` | 拖拽流畅度 |

---

## 4. 批次 D：需拍板（契约 / 产品语义）

1. **`CompactionSettings.archive_original` 已成纯兼容字段**：`settings.rs:125-132` 明确宿主忽略，
   两条路径都硬编码 `false`（`runtime/execution.rs:460`、`runtime/compaction.rs:42`）；
   前端仅 `types.ts` / dev-mock / fixture 保留。删除会改 settings wire 形状与 fixture；
   保留则建议 `skip_serializing_if` 并删死 i18n 键。
2. **`TurnStart` / `TurnEnd` 是否继续跨 IPC**：前端是 no-op（`chat-runtime.ts:1006-1007`），
   `TurnEnd` 携带整条 assistant 消息 + 全部工具结果，每轮重复序列化。
   保守解是 `session/events.rs:14` 过滤掉二者（+1 行），但会改变宿主可见事件面；
   与上轮「保留 TurnEnd 载荷」的决定相邻，需重新确认。
3. **前端只声明未调用的命令**：`query_background_tasks`、`manage_background_task`
   （另有已知保留的 `session_messages/session_stats/forum_search`）——属 Web API 产品面。
4. **dev-mock 有 15 个命令落进 `default: return null`**，其中 `set_session_permission_mode`、
   `fork_session` 在 demo UI 可见并会抛错；`open_session` 还缺 `questions/questionsRevision/approvals`
   三个必填字段。建议补 mock + `satisfies` 真 DTO + 契约测试校验 case 覆盖。
5. **`query(include_completed)` 里对全终态任务的 250ms sleep** ✓：从代码可证明不改变返回数据，
   但可能是刻意的反空转节流，删前确认意图。
6. **时间格式化统一**：论坛 `formatRelativeTime` 硬编码英文，统一后会跟随 locale（中文界面文案变化）。
7. **弹窗统一**：`ModelSelectModal` / `InstallSkillModal` / RemoteAccess 旋转确认 / `confirm()` 四套
   → Radix `Dialog` 壳；会影响焦点与视觉。
8. **空态统一**：把 `RightRailEmpty` 提升为 `EmptyState`，迁移 8 处 `.empty-state`；标题标签/间距可能微变。
9. **暗色 quota 卡片被亮色规则误伤**：`src/styles/features/messages.css:494-495` 第二个选择器漏了
   `:root[data-theme="light"]` 前缀；修好后暗色徽标会略变亮。
10. **`App.tsx:113-120` 的归档可见性自相矛盾**：filter 使 `showArchivedSessions=true` 时
    归档会话的 fork 后代仍进入 overview/search/stats；需确认期望。
11. **`pipi-terminal` 缺 `dev-may-depend-on = ["pipi-test-support"]`**，导致测试手写 temp dir；
    加白名单还是写明「刻意零测试依赖」。
12. **`av skill remove --purge` 不做跨 Agent 声明检查**（面板会拒绝）；有意放宽还是遗漏。

---

## 5. 已核实「不该改」（含前两轮否决 + 本轮复核）

- 命令表 80 项与 `dispatch` / `src/ipc.ts` / Tauri `generate_handler!` 完全一致且顺序受测试锁定；
  9 个重复 Args 结构体不合并（契约测试按名字反查）。
- 事件 envelope 三字段不抽公共结构；`HostSession` 的 queue/questions/permission 不合成大对象；
  `SessionModes` 独立于 `HostSession`。
- questions 与 approvals 两套状态机不合并（持久账本 vs oneshot rendezvous + 审计）。
- 压缩：投影不落盘 / 替换落盘、`validate`/`validate_result`、`Plan` 双形状、两档模式、
  分叉+归档、原地替换 writer、`session-switched` 用旧 id —— 全部保留。
- 重试：provider 只分类、重发只在 agent_loop、中止不可重试、已输出正文不重放、
  协议协商是唯一例外 —— 保留。
- 统计口径：窗口 N=10、`Usage::prompt_tokens()` 单一读法、数据不足省略。
  唯一建议是修正 `last` 更新条件（错误/中止轮次不应把占用归零，`pipi-agent/src/stats.rs:113-118`，
  +1 行，与 `context::last_usage_anchor` 对齐）。
- `settle_unanswered_tail` 是活路径（git 载体下仍需配对修复）。
- 端口层（`Provider` / `SessionRepo` / `SessionLog` / `ToolGate` / `ForumRepo`）不删；
  `ApprovalBroker` 的 `Arc<dyn Fn>` 与 `AgentRunner` 是测试缝。
- 沙箱是唯一执行边界；`resolve_read_path/resolve_write_path` 是 UX containment；
  `secure_file` 的 handle-relative `openat` 不能并进路径 API。
- MCP：启用即授权、`parallelCalls` 默认 false、2000 行/50 KiB 截断、64 KiB structuredContent 上限。
- 终端：工作区级、2 MiB 环形缓冲 + seq、`setsid` 整组信号、不沙箱、不走 EventHub。
- `BackgroundTaskManager::submit` 的 `accepting` 读锁跨整个函数（保护 shutdown 的登记窗口）。
- workspace 与 pipi-session 两套 git 封装不合并（async 只读预览 vs 同步 bare-repo + 仓库锁）。
- `pipi-context` 单包测试缺 tokio `rt` 的问题已不存在（dev-deps 已含）。
- `cached_catalog` 的 path+mtime+len 记忆化与 `load_catalog` 的单飞持锁刷新保留。
- 前端 `SessionRuntime.replay` 与 `SessionController.pending` 是两个生命周期的缓冲，不是两套 reducer；
  `drafts/awaitingIdentity` 兜底保留（replay 在 `agent_end` 会截断中间帧）。
- `RightRailEmpty` / `SidebarSearch` / `FileGlyph` 动态类名 / i18n 动态拼接键 / xterm 与 diffs 的
  shadow DOM 类 —— 均不是死代码。
- 大文件（`commands.rs` 1702、`AgentWorkbench.tsx` 735 等）本轮不拆：纯搬移 churn 大，
  与清理混做会掩盖回归；需要单独一轮。

---

## 6. 建议实施顺序

1. **PR-1（缺陷）**：0.1 venv 重复层加载 + 0.2/0.3 右栏 effect + 0.4 `GroupGuard` disarm
   + 0.6 符号链接守卫。每项带回归测试。
2. **PR-2（零风险删除）**：批次 A，按 crate 分 3~4 个 commit；验证 `cargo check/clippy/test`
   + `tsc` + 前端测试。
3. **PR-3（core 去重）**：`HostSession::new`、`forward_progress`、生命周期/压缩装配、stats 累加。
4. **PR-4（tools/venv 去重 + 测试夹具）**：`ToolContext` 夹具、mutation 助手、venv 读取块与遍历。
5. **PR-5（性能）**：C1（sink 克隆）、C2（Arc 目录）、C4（stats 线程）、C7/C8（edit）、C9/C10（前端）。
   先做零语义变化的两项，再评估 C5/C6。
6. **PR-6（契约与文档）**：`archive_original` 决策、`ipc-wire.json` 补 11 类结果、dev-mock 类型收紧、
   AGENTS.md 压缩描述与模块表（`AGENTS.md:274-275` 与代码相反；`lib.rs:7-17` 缺
   approvals/remote/notifications/forum/stats/workspace/icon_themes）、`docs/git-session-store.md`
   的「一条 entry 一个 commit」与 §8 旧结论。

验证矩阵（每个 PR 至少）：`cargo test -p <受影响 crate>`、`cargo test -p pipi-core`、
涉及 wire 时 `npm run test:frontend` + `npx tsc --noEmit`、依赖变更时 `cargo xtask arch`、
收尾 `cargo clippy --workspace --all-targets`；本机测试需按 `pipi-local-verification-gotchas` 隔离 HOME。

---

## 7. 未决疑点

1. `~/.git` 空目录触发 0.1 是本机环境特例还是普遍路径（默认 workspace 在 Agent 目录内，
   任何在祖先有 `.git` 的机器都会命中）——修法相同，但影响面需要确认。
2. `query` 250ms sleep 的原始意图（见 §4.5）。
3. `ServerInfo` / `McpClient::ping` 是否在路线图上（决定删还是接线）。
4. `SkillVersion.reference` / `AgentSkills.declaredByPath` / `agentContractExists` 是否为后续 UI 预留
   ——当前零消费者，删除需同步 wire fixture。
5. `GitExec` 共享注册表的路径归一化边界（符号链接 vs canonical）需要一次实测。
