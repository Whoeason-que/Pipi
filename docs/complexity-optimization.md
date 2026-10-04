# Pipi 复杂度优化实施记录

本轮以 `5c0537c` 为基线，完成六个阶段的优化。目标是收敛状态所有权、隔离修改范围、减少重复实现，并用行为回归保护现有产品契约。

## 基线与结果

| 对象 | 基线 | 本轮结果 | 修改时需要理解的范围 |
| --- | ---: | ---: | --- |
| `src/App.tsx` | 3,220 行 | 1,366 行 | 导航、Agent/会话列表与全局订阅；配置表单由各自组件拥有 |
| `src/Chat.tsx` | 2,158 行 | 503 行 | 输入、滚动与视图交互；运行投影和身份由控制器拥有 |
| `pipi-app/src/runtime.rs` | 2,707 行 | `runtime/mod.rs` 293 行 | 公共入口与会话状态；执行、会话操作、压缩和事件各在私有模块 |
| 前端行为与契约测试 | 27 项 | 46 项 | 增加身份、并发、重键、水合、停止和 IPC 回归 |
| `pipi-app` 测试 | 37 项 | 40 项 | 增加两项后台任务回归与一项跨端 wire 回归 |

这些是物理行数，包含注释；入口文件缩小主要来自职责迁移。前端 `src/` 的 TS/TSX 总量由 8,834 行变为 8,832 行，不能把文件拆分当成等量删除。后台任务生产代码约由 1,357 行降至 1,336 行，新增回归测试使整个文件反而稍长。

工作区仍为 10 个 Cargo 成员。没有新增前端依赖、数据库或持久化格式，39 个命令的名称和 Rust wire 字段保持现有契约。

> 后续变更（不属于本轮）：技能中心一轮新增 8 个命令，现为 47 个；crate 拆分一轮后工作区为 15 个成员，分层与依赖方向见根目录 `architecture.toml`（`cargo xtask arch` 检查）。

## 六个阶段

### 1. 建立基线与行为护栏

- 记录入口文件规模、Cargo 依赖边界、39 个桌面/Web 命令与构建结果。
- 在修改前执行应用层、核心层和前端回归。
- 前端测试命令关闭 Node 的文件级测试隔离，使结果按实际测试用例计数，基线为 27 项。

### 2. 拆分界面职责

- Agent 工作台、创建表单、文件编辑器放入 `src/agents/`。
- 全局设置与供应商编辑器放入 `src/settings/`。
- 聊天时间线、工具详情、检查器和模型选择器放入 `src/chat/`。
- 确认弹窗、空状态与主题逻辑分别由共享组件和 `src/theme.ts` 拥有。
- 创建与编辑 Agent 共用工具、bash 模式、沙箱选择器及权限字段转换。

迁移沿用现有布局与交互，不引入通用表单框架或新的样式体系。

### 3. 收敛前端会话状态

`src/session-runtime.ts` 拥有全局六类宿主事件订阅、会话索引和短期事件回放。`src/session-controller.ts` 拥有单条会话的水合、run 身份、发送、停止、审批、模型与重键。`ChatView` 通过 `useSyncExternalStore` 订阅快照。

- 先订阅，再水合；加载期间按到达顺序合并事件。
- 旧 run 的完成、错误和审批不能作用于新 run；旧水合响应不能替换新快照。
- 缺失或不匹配的会话身份拒绝发送。
- 压缩切换 ID 时重键同一个控制器，并接收新 ID 下提前到达的完成事件。
- 停止轮询绑定发起时的 run，迟到的旧轮询不能结束下一轮。
- 视图卸载只解除订阅，不停止后端运行；同一 Agent 的多条会话保持隔离。
- 32 项缓存限制只针对可释放的空闲投影；运行中的会话不受并发数量限制。
- App 对会话列表与运行状态请求做响应顺序检查，按 Agent 合并列表刷新结果。

界面回归发现了重试提示参与历史匹配计数、导致用户消息重复的问题。本轮将重试提示保留为瞬时条目，匹配持久化消息时排除系统提示，并保留已经渲染的消息 key、工具参数与详情。未落盘的中止、工具与重试提示以其后已确认消息为位置锚点，反复水合时不会移到最新回复之后。新增回归覆盖重复文本、水合去重、消息顺序以及多次水合后的详情定位。

后端 `RuntimeState`、`session_infos` 与 JSONL 仍分别是运行、运行列表与历史的权威；前端控制器只维护投影。

### 4. 拆分 Rust 运行时

公共调用继续经过 `pipi_app::runtime`，子模块保持私有：

| 模块 | 职责 |
| --- | --- |
| `runtime/mod.rs` | 公共入口、运行状态、守卫与会话槽类型 |
| `runtime/slots.rs` | 会话加载、创建、查询、模型、分叉与归档操作 |
| `runtime/execution.rs` | 正式运行、child Agent 运行与启动准备 |
| `runtime/compaction.rs` | 手动/自动压缩、writer 切换及重键 |
| `runtime/events.rs` | 宿主事件、统计与会话变更通知 |
| `runtime/tests.rs` | 运行时单元回归 |

正式运行与 child Agent 共用声明式环境账本追加、流式调用选项构造。child Agent 的单层组合、基础工具集和无交互审批约束继续保留。

按会话守卫、压缩时共享 running/abort/steering、旧 ID 的切换事件、新 ID 的后续事件、关闭旧 writer 后归档等顺序不变量均由现有回归保护。provider 重试、权限判定及核心压缩算法没有改变。

### 5. 统一后台任务公共状态

shell 与 child Agent 共用 `JobState` 的快照、输出裁剪、游标、终态结果和完成时间处理。类型专属部分仍由各自任务拥有，例如 shell PID/进程终止和 child Agent 中止信号。

共用等待函数先注册完成通知，再检查状态，避免检查与等待之间漏掉完成事件。新增回归覆盖两种任务的空输出、512 KiB 限额、多字节字符裁剪、增量游标、终态结果、完成通知与超时。

### 6. 固化 IPC 与 DTO 契约

- `src/ipc.ts` 集中描述全部 39 个命令的参数与返回值（现为 47 个），以及六类宿主事件。
- UI 调用经过类型化命令边界，不再由调用者自行填写 `invoke<T>(string)` 的结果类型。
- 前端契约测试核对类型映射、桌面声明与注册、Web 路由和参数要求；命令拼写、参数类型、返回类型和必需会话身份的偏差会失败。
- `tests/fixtures/ipc-wire.json` 由 Rust 序列化回归与 TypeScript DTO 校验共同使用，覆盖设置、Agent、模型、消息、统计、会话、后台任务及六类事件。
- 前端可选字段对齐 Rust 的省略规则；Rust 持久化字段未改名。

README 与 AGENTS.md 同步记录状态所有权、私有模块和必跑契约测试。

## 验证结果

| 检查 | 结果与范围 |
| --- | --- |
| `npm run test:frontend` | 最终版本 46 项通过 |
| `npm run build` | 最终版本 TypeScript 检查和生产构建通过 |
| `cargo test -p pipi-app` | 行为修改完成后 40 项通过，包含并发、压缩、停止、后台任务与 wire 回归 |
| `cargo test -p pipi-core` | 修改前核心基线 137 项通过，1 项联网目录测试忽略；本轮未修改核心源码 |
| `cargo check --workspace` | 最终 Rust 源码通过 |
| `cargo clippy -p pipi-app -p pipi-server --all-targets` | 完成；存在原有参数数量与测试环境锁跨 await 警告 |
| `git diff --check` | 通过 |
| 浏览器交互回归 | 演示模式下核对设置、供应商、Agent 配置、历史/工具渲染、发送、停止、停止后再运行、运行中切换会话及返回后的历史合并 |

应用层 40 项测试通过后，Rust 仅进行了格式整理和私有可见性收敛，随后工作区检查与 all-targets Clippy 均完成。最后一次额外的 Rust 全量复跑因自动审批检查额度用尽而未执行，不能将其记为再次通过。需要绑定本地端口的 mock 集成测试此前已在获准环境中执行。

生产构建 JS 为 779.80 kB（gzip 243.85 kB），基线为 777.48 kB（gzip 242.12 kB）；CSS 仍为 56.73 kB（gzip 10.87 kB）。原有大于 500 kB 的 chunk 警告仍在。本轮改善代码职责与回归能力，不宣称首屏速度或内存使用量得到提升。

浏览器验证使用开发 mock 数据，没有真实模型请求，也没有改写用户 Agent 数据。尚未执行桌面安装包构建或真实供应商调用；这两项不计入本轮离线验证。

## 2026-10-04 模块与测试布局

crate 拆分（见上）之后又做了一轮文件级整理，不改行为、不改任何 wire 字段：

- **目录化的大模块**（按改动理由分文件，`mod.rs` 只留文档 + `mod` 声明 + `pub use` 门面）：
  `pipi-core/src/agents/`（types / paths / registry / archive / files / contract /
  tool_context / prompt，2,229 → 10 个文件，最大 314 行）、
  `pipi-app/src/background/`（service / shell / agent / state / persist，1,788 → 7 个文件）、
  `pipi-tools/src/permissions/`（types / config / parse / redirect / rm / paths / safety，1,464 → 8 个文件）、
  `pipi-skills/src/`（model / library / agent_skills / enable / install）。
- **测试搬出生产文件**：34 个文件里的 `#[cfg(test)] mod tests` 移到同目录
  `tests.rs`（目录模块）或 `<name>_tests.rs`（单文件模块，`#[path]` 声明），
  共搬走约 9,600 行测试代码；`cargo test --workspace` 的用例集合逐一比对不变。
- **合并**：`pipi-tools` 的 `fs.rs` 与 `fs_atomic.rs`（两个 40 行左右、名字撞车的
  模块）合并为一个 `fs` 模块（原子写入 + 数据树守卫）。
- **测试精简**：合并 13 个同函数、同形态的碎测试（provider 的 5 个 usage 扣除用例
  → 1 个表驱动；`truncate` 的 8 个 head/tail 用例 → 3 个；`create_agent` 的 3 个
  → 1 个；context 的 env 渲染 2 个 → 1 个；compaction 的 `find_keep_start` 2 个 → 1 个），
  断言一条未删。安全判据（沙箱重定向、`rm -f`、schema fail-closed）、并发、压缩
  不变量、wire 契约的表驱动测试全部保留。
- **删除兼容层**（Rust + 前端）：`pipi_tools::{types, session}` 两个转口模块、
  `pipi_tools::resolve_write_path` 转发、`pipi_app::runtime::SessionSummary`
  再导出、`src-tauri` 的 `ChatState` 别名；前端删掉「无 envelope 事件」兼容分支
  （`normalizeAgentEvent` / `normalizeStatsPayload` 及 `meta: null` 路径），
  载荷类型收紧为 envelope 本身。刻意保留的是**数据兼容**（读旧会话文件的
  serde alias、旧 agent.json 的宽松默认值），删了会丢用户历史。
- 验证：`cargo test --workspace` 全部通过、`cargo clippy --workspace --all-targets`
  无新告警、`cargo fmt --all --check` 干净、`cargo xtask arch` 通过。

## 2026-10-05 测试集中存放与冗余清理

- 单元测试从生产文件里搬出（50 个文件、约 1.03 万行），放在**被测模块自己的目录**下：
  `src/foo.rs` → `src/foo/tests.rs`、`src/foo/mod.rs` → `src/foo/tests.rs`、
  crate 根 → `src/tests.rs`，生产文件里只写 `#[cfg(test)] mod tests;`。
  这是 Rust 的默认解析路径，全仓 **0 处 `#[path]`**；真正的集成测试仍在
  `crates/<crate>/tests/*.rs`（独立 crate，无需声明）。跨文件 fixture 收进 crate 根的
  `src/test_support.rs`。
- 测试助手去重：`temp_dir` / `temp_root` 共 9 份实现 → 1 份
  （`pipi_test_support::temp_dir`，各 crate 以 dev-dependency 使用）；`av` 的
  `layer` 3→1、`write_skill` 2→1、`make_repo` 2→1；`pipi-agent` 的 `tool_result`
  4→1、`assistant_text` 2→1。合并时按各调用点取并集，测试用例集合逐一比对不变。
- 删除的死代码与未用依赖：`av::store::record_index`（`record_index_entries` 的
  死包装）、`pipi-tools` 的 `uuid` 依赖（随兼容层一起失效）、`pipi-agent` 未使用的
  `tempfile` dev-dependency；`architecture.toml` 补登新增的 dev 白名单
  （av / pipi-agent / pipi-harness / pipi-tools 允许 dev 依赖 pipi-test-support）。
- 验证：`cargo test --workspace` 384 项全通过、`cargo xtask arch` 通过、
  `npm run test:frontend` 46/46、`npx tsc --noEmit` 干净。

## 后续维护规则

1. 会话投影行为进入控制器或纯 reducer，组件只处理 UI；宿主事件订阅进入 runtime。
2. 新命令同步更新 IPC 映射、桌面/Web 壳与共享 wire fixture，运行前端及应用层回归。
3. 运行时公共 API 保持经过 `runtime`；内部模块不成为跨 crate 调用入口。
4. 继续以身份、并发、停止、压缩恢复和持久化契约验收重构，入口行数只作为辅助指标。

本轮代码与实施记录一起纳入版本管理。
