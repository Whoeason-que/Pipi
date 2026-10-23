## 交付方式

**在新分支 `feat/git-session-store` 上实施**（从当前 `main` 的 `bf7ed1a` 起），按逻辑边界拆多个 commit 落在该分支上，**不碰 main、不 push**，完成后由你决定合并。工作区现有 4 个未提交文件（上一轮审查里 `validate_session_id`/`kept_start` 去重的收尾）先原样提交为该分支的第一个 commit。

## 目标

把会话载体从 JSONL 换成 git 分支（设计已拍板、实测已完成，见 `docs/git-session-store.md`）：git 实现落在**已有的** `crates/pipi-session`，不新建 crate；`commit_point` 是唯一需要新接的线。

**三项默认决策**（你没答，按前几轮对话意图定，可在批准时推翻）：
1. **git 是唯一写入载体**：新建会话一律 git，不加 `agent.json.sessionStore` 开关（JSONL 写入实现保留，仅用于旧会话）。
2. **旧数据走显式导入**：新增导入命令 + 最小 UI 入口；逐条校验、幂等、原文件改名 `.jsonl.imported` 保留。未导入的旧会话仍可正常打开与继续对话（按会话 id 分发到 JSONL），不产生半 git 半 jsonl 的会话。
3. **范围**：P2（载体）+ 运行时接线 + 装配切换 + 导入 + P5 文档；不含 P4（前端导出/bundle 入口）。

## 数据模型（照文档 §3，已实测）

- 影子 bare 仓库 `~/.pipi/agents/<n>/sessions.git`（`git init --bare --initial-branch=main` + 一条 main 锚提交）；refs：`session/<id>`、`archive/<id>`、`main`。
- **O2**：一轮一个 commit（`fast-import` 一次写 N 个 blob + 1 个 commit，parent = 当前 tip）。
- tree：每条 entry 一个文件 `e/<3位分片>/<seq:05>-<kind>-<id8>.json`，内容 = 今天落盘那一行 JSON 原文；路径零填充 seq → 按文件名排序即对话顺序。
- commit message：该轮首条 user 消息的一行摘要（截 72），body 带 `Turn: <n>` trailer（按 commit_point 计数递增，不按 user entry 数；创建提交不带 Turn）。
- author `<role> <role@pipi.local>`、committer `Pipi <pipi@local>`、日期取 entry timestamp。

## 代码落点

```
crates/pipi-session/src/
  git/mod.rs     门面：GitSessionRepo::new(agent_dir) + impl SessionRepo + 缺 git fail-closed
  git/exec.rs    进程调用：git_command/run_git（照抄 av/sources.rs 的 env 加固与错误文案）、
                 repo 级 Mutex<()>（兼作 gc 与写者互斥）、probe（git --version）、gc 节流
  git/layout.rs  tree 路径编解码 + commit message 组装（写读两侧共用）
  git/sink.rs    GitSink: impl SessionLog —— 内存缓冲、fast-import 组装、commit_point
  git/reader.rs  ls-tree -r + cat-file --batch → entries；summary 四字段
  git/refs.rs    建仓与 main 锚、分支命名、update-ref、fork/archive/restore/delete
  store.rs       组合载体：create 走 git；其余按会话 id 分发 git/JSONL；list 合并去重
```

约束：`architecture.toml` 已写明 pipi-session 是唯一为会话存储 spawn git 的 crate；内部依赖只能 `pipi-protocol`；不引 git2、不 spawn 别的进程；文件按 500 行规则拆。

## 实施步骤（每步保持全绿）

**1. 分支与收尾**：建 `feat/git-session-store`，先把工作区 4 个未提交文件提交（去重收尾）。
**2. git/exec.rs + layout.rs**（内联单测：参数组装、路径编解码、message 截断、env 加固）。
**3. git/refs.rs + git/sink.rs**：建仓幂等、main 锚、create/open 返回 sink；缓冲语义与 `SessionWriter` 完全一致（含 `append_env` 幂等、`message_ids` 走 `pipi_protocol::session::kept_start`）；commit_point 组装 fast-import 流，失败不清缓冲。
**4. git/reader.rs**：`entries()` = ls-tree + 一次 cat-file --batch；`list()` = 一次 for-each-ref + 每分支 ls-tree（数 message 算 message_count）+ 一次 cat-file 取 title 与 model（display_name），last_active 取 tip commit 时间。
**5. git/mod.rs**：`impl SessionRepo` 全部方法；`exists()` fail-closed；缺 git 明确报错不回落；delete = 删 ref + `gc --prune=now`；gc = commit_point 末尾节流的 `gc --auto`。
**6. store.rs 组合载体**：create 恒走 git；其余按 id 先查 git ref、否则回落 JSONL；list 合并去重（git 优先）；临时会话不变。
**7. 黑盒测试**：`tests/git_backend.rs`（文档 §9 的 1–5、7、8）+ `tests/backend_conformance.rs`（同一套场景对两个后端跑同一批断言）。
**8. 运行时接线（pipi-agent）**：见下表；异步路径用 `spawn_blocking`，一次性生命周期路径同步调用（≤5ms）。
**9. core 黑盒测试去载体化**（先做，仍跑 JSONL）：`concurrent_sessions.rs:292-298`、`session_repair.rs`、`compaction_runtime.rs` 里直接读 `sessions/*.jsonl` 的断言改为经 `SessionRepo` 读。
**10. 装配切换**：`runtime/store.rs::session_repo` 返回组合载体；`tools/agent.rs:508` 收敛到同一处。
**11. 导入命令**：`import_sessions`（逐条读 JSONL → 写 git → 读回逐字节比对 → 改名 `.jsonl.imported`，幂等）+ 命令表/前端兜底表/`ipc-wire.json` + Agent 设置页「导入旧会话」按钮。
**12. 文档与规则（P5）**：AGENTS.md、README（会话行/架构图/与 pi 的关系表）、`architecture.toml` 注释、`docs/git-session-store.md` 状态改「已实施」。
**13. 验证**：`cargo test -p pipi-session` / `-p pipi-agent` / `-p pipi-core`（含 `--features server`）、`npm run test:frontend`、`cargo xtask arch`、`cargo clippy --workspace --all-targets`，最后手工 dogfood：Q 跑一轮后用 `git log`/`git show` 浏览。

## 运行时提交点（步骤 8 明细）

| 点 | 位置 | 说明 |
| --- | --- | --- |
| 轮末 | `pipi-agent/src/session/run.rs` harvest 返回与 abort 终态之后、`run_compaction` 之前（约 :263/:264） | 必须在压缩前、`drop(guard)`（:125）与 AgentEnd（:133）之前；async → spawn_blocking |
| 超时 | `run.rs` 超时分支提前 return 之前（约 :227） | child 超时终态也要落 |
| 启动失败 | `run.rs::fail_start` 返回前（约 :97-104） | 文档表未列，但今天这两条即时落盘，git 下不提交就丢 |
| 压缩条目 | `compaction.rs` 分叉路径（:202 与 :208 之间，对 `forked` 提交）与原地路径（:167 后） | 分叉必须在 `mem::replace` 换账本之前提交 |
| 会话创建 | 宿主 `create_session_object`（slots.rs:50-58）与 child（child.rs:98-107）创建后各提交一次 | 空会话也可见 |
| 手动切模型 | `state.rs:256` append_model_change 之后 | 空闲时切模型入史 |
| 旧会话尾部修复 | `state.rs:153-156`，仅当追加了条目 | 只对旧 JSONL 会话有意义 |

**不做**：`prepare` 后不提交（O2 明确否掉「输入即提交」，崩溃丢进行中的一轮是已知代价）；临时会话不提交；`fork_session` 无需提交。

## 已知取舍（会写进文档）

- `session_kernel.rs:221`「prepare 后条目已可见」是 MemoryLog 替身行为，git 语义变为轮末持久；内核测试补断言：**commit_point 在 guard 释放与压缩之前被调用**。
- `entries()` 的「含非活跃路径」在 git 分支上不存在；已实测现有 16 个会话 100% 线性，等价性成立。
- `fork_session(upToEntryId)` 前端恒传 null；git 侧实现为「从 tip 往回找含该 entry 的 commit」，不做索引。
- `list()` 每分支 2 次 spawn（20 会话约 100ms），比今天扫 25MB 文件更快。
- gc 用节流的 `gc --auto`（同步、锁内、≥60s 一次）；裸仓库默认不写 reflog，删除紧跟 `gc --prune=now`；日后开启 `core.logAllRefUpdates` 必须补 `reflog expire`。