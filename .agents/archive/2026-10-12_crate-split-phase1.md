# Pipi Rust workspace 结构重整（crate 拆分）计划

## 一、调研结论（现状 + 上游证据）

**现状**（`cargo` 成员 10 个，Rust 共 36.2k 行，crate 级 DAG 已无环）：

| crate | LOC | 问题 |
| --- | ---: | --- |
| `pipi-core` | 12,773（占 35%） | **纯引擎与 IO/网络服务混在一个 crate** |
| `pipi-app` | 7,961 | 有工作区里仅有的两处**模块环** |
| `av` / `pipi-tools` | 6,665 / 4,549 | 边界清楚，不动 |
| `pipi-protocol` / `pipi-harness` / `pipi-provider` / `pipi-error` | 538 / 912 / 1,249 / 134 | 已是干净的叶子/协议层，不动 |

`pipi-core` 内部按 IO 审计分成两类：
- **纯**（无文件/网络/子进程）：`agent_loop`(1,776)、`context`(461)、`retry`(173)、`stats`(391)、`compaction/`(1,686)、`tools/background`(523，仅一处 canonicalize)
- **IO/网络**：`agents`(2,250，std::fs + av)、`session`(1,141，JSONL 文件)、`settings`(363)、`skill_center`(1,156，fs + av 阻塞/联网)、`catalog`(1,178，reqwest 拉 models.dev)、`tools/agent`(659，经 session 读盘)

**上游路径级证据**：
1. **pi** `packages/agent`(pi-agent-core) 把 `harness/{runtime,session,compaction,tools}` 放在同一个包 —— 「循环 + 会话存储 + 压缩」本来就是一个单元；宿主 `pi-coding-agent` 在其上。
2. **codex** `AGENTS.md`：「resist adding code to codex-core… **introduce a new crate**」；`protocol/README.md`：「minimal dependencies, no material business logic」；会话持久化独立成 `rollout/`、`thread-store/`；技能 `skills/`、模型目录 `models-manager/` 各自成 crate；测试支持独立成 `core_test_support`/`app_test_support`；宿主统一走 `app-server` 家族（协议 / 传输 / 守护 / 客户端分家）。
3. **zcode** `architecture-policy.yaml` 定义 `layers` / `layerOrder` / `publicEntrypoints` / `forbidCycles` / `forbidDeepImports`，由 `scripts/architecture/*.mjs` **机检**（module-dependency、deep-import、cycle、layer-direction、domain-io、max-file-lines，带 baseline 指纹、只查变更集）；`packages/services` 是唯一应用服务层，桌面/HTTP/stdio/守护四个宿主共用 `createLocalServices`；`core` 只依赖 `contracts` 端口，**只有 `adapters` 碰 `node:*`**。
4. **opencode** V2 把 `core`(会话+工具) / `server`(HTTP) / `protocol`(wire) / `llm`(provider) 分成独立包 —— 与 pipi 现状最接近的对照。

## 二、目标结构（中等方案：10 → 13 crates）

```
叶子      av        pipi-error      pipi-harness
协议/基础  pipi-protocol → pipi-tools, pipi-provider
引擎(新)   pipi-agent   = agent_loop, context, retry, stats,
                          session, compaction          ~4.9k
                          (无 av、无网络、不碰 Agent 注册表)
服务(新)   pipi-skills  = skill_center                 ~1.2k
          pipi-catalog = catalog (models.dev + 缓存)   ~1.2k
领域       pipi-core    = agents, settings, tools/*    ~3.8k
应用       pipi-app     → {pipi-agent, pipi-core, pipi-skills, pipi-catalog}
宿主       src-tauri / pipi-server（薄壳，不变）
```

要点：
- `pipi-agent` 对齐 pi 的 `pi-agent-core` 分组；`pipi-skills` / `pipi-catalog` 对齐 codex 的 `skills/`、`models-manager/` 与 zcode 的 adapters/services 分离 —— 把**联网/阻塞**从领域 crate 里挪走。
- **不拆** `compaction` 成独立 crate（pi 与 zcode 都把它留在 domain 内）。
- 跨 crate 依赖在 `Cargo.toml` 直连属主（沿用「不设转口 re-export」约定）。

## 三、实施阶段

**Phase 0 — 基线与测试脚手架**
- 记录基线：`cargo test --workspace` 全绿、各 crate 行数、`cargo build` 冷/热耗时。
- 新增 dev-only crate `pipi-test-support`：承载 `HOME_LOCK` / `set_env_var` / `remove_env_var`（现 `pipi-core/src/lib.rs:43-61`，拆 crate 后 `pub(crate)` 不再跨 crate），各 crate 以 dev-dependency 使用（先例：codex 的 `core_test_support`）。

**Phase 1 — 拆 `pipi-agent`**
- 移动 `agent_loop.rs`、`context.rs`、`retry.rs`、`stats.rs`、`session.rs`、`compaction/` 及对应 `tests/{compaction_corpus,retry_runtime,provider_usage}.rs`。
- 解除唯一阻塞：`session.rs:404-409` 依赖 `agents::{ensure_real_file, ensure_real_directory, ARCHIVE_DIR}` → 前两者移到 `pipi-tools::fs`（该 crate 已有 `fs_atomic`），`ARCHIVE_DIR` 由 session 自己持有。
- 依赖：`pipi-protocol`、`pipi-provider`、`pipi-tools`、`pipi-harness`、`pipi-error`、`serde`、`tokio`、`futures-util`。
- 更新调用点：`pipi-app` 67 处、`pipi-core` 内部（agents→compaction 2 个常量、tools/agent→session）、两个壳 10 处、集成测试 11 处。

**Phase 2 — 拆 `pipi-skills` 与 `pipi-catalog`**
- `pipi-skills` ← `skill_center.rs`；`agents::collect_contract_layers`（现 `pub(crate)`，`agents.rs:844`）升为 `pub`。
- `pipi-catalog` ← `catalog.rs`（自带 reqwest，是 `pipi-core` 里唯一的直连网络）。
- 两者消费方只有 `pipi-app` 与两个壳（`SERVER/main.rs`、`TAURI/commands.rs`）。

**Phase 3 — 修 `pipi-app` 的两处模块环**（后续任何 app 层拆分的前置）
- `approval → runtime`：只用到 `RuntimeEvent` / `EventEmitter`（`approval.rs:17`）→ 把这两个类型提到 `pipi-app::events`（叶子模块），`runtime` 与 `approval` 都依赖它。
- `background → runtime`：只调用 `run_agent_once_inner_with_sink` + `CHILD_RUN_TIMEOUT_SECS`（`background.rs:978,982`）→ 改为构造时注入（闭包/参数），消除反向依赖。

**Phase 4 — 宿主命令表收敛**（可推迟；本轮若时间不够先不做）
- `pipi-app::commands`：每条命令一个 serde 参数结构 + 唯一 dispatch；`src-tauri` 的 47 个 `#[tauri::command]` 变成 1 行转发，`pipi-server` 用同一批结构反序列化替换手写 `required_string/optional_value`。
- 现有 `tests/fixtures/ipc-wire.json` + `ipc-contract.test.ts` 继续作为契约兜底，**wire 字段不许变**。

**Phase 5 — 机械护栏**
- 新增 `cargo xtask arch`（`crates/xtask`，workspace 成员）：读取声明式 `architecture.toml`，检查 ① 内部依赖方向白名单 ② 无环 ③ 纯 crate 禁用项（如 `pipi-agent` 不许依赖 `av`/`reqwest`、`pipi-protocol` 不许依赖 `tokio::net`）。违规非零退出，带反例测试。
- 行数预算 ratchet 暂不做（可后续加）。

**Phase 6 — 文档同步**
- `README.md` 架构图与「与 pi/codex 的关系」表补新 crate 映射；`AGENTS.md` crate 树补上缺失的 `pipi-server`、写清依赖方向表与「改依赖先跑 `cargo xtask arch`」；`docs/complexity-optimization.md` 修正 39→47 命令的漂移。

## 四、验证口径（每阶段结束都跑）

`cargo test -p <改动的 crate>` → 收尾跑 `cargo test --workspace`、`cargo check --workspace`、`cd src-tauri && cargo clippy`、`npm run test:frontend`、`npx tsc --noEmit`、`cargo xtask arch`、`cargo fmt --all --check`、`git diff --check`。
验收重点：IPC/wire 契约回归必须全绿（拆分不许改任何 JSON 字段）；`pipi-app/tests/{concurrent_sessions,compaction_runtime,session_repair,agent_composition}.rs` 行为不变。

## 五、风险与不做的事

- **公开 API 迁移**：机械更新约 150 个调用点；`pipi-core` 的 `pub(crate)` 只有 `collect_contract_layers` 一处需升 `pub`。
- **编译时间**：增量编译会变快（改 `skill_center` 不再重编引擎），冷编译可能略慢；不宣称提速。
- **不做**：不动 provider/protocol/tools/harness 边界；不改 JSONL/会话格式；不改 IPC 字段；不引入数据库；不拆 compaction；不动前端目录结构。
- 分支：在当前 `feat/skill-center-ui` 之上新建 `refactor/crate-split`；**不自行 push**，提交按逻辑边界拆分（沿用你的习惯）。