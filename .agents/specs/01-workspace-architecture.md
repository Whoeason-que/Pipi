# 01 - 工作区架构与分层规范

> **状态**：现行规范（生效日期：2026-10-24）  
> **适用范围**：Pipi 工作区全部 12 个 Crate、构建门禁及新增模块  
> **核心工具**：`cargo xtask arch`（分层与依赖方向机检）  
> **参考基准**：`architecture.toml`、`AGENTS.md`、`.zcode/plans/plan-sess_d6cc490e-648d-45e6-9d29-0fc8686f4b14.md`

---

## 1. 架构演进与终态布局

Pipi 是一个以 **Agent（而非会话）** 为中心的本地优先桌面应用。为了彻底杜绝代码腐化、循环依赖与职责越界，代码库经历了一次深度的系统级重构（从早期 15+ 混合 crate / 散乱模块重构为现在的 **12-Crate 现代工作区体系**）。

### 1.1 历史重构与废弃资产清单

在重构过程中，多个职责不清或命名陈旧的 crate 与模块已被彻底合并、拆解或消除，**严禁在任何新代码、配置或文档中重新引入以下废弃名词**：

| 已废弃/消除名词 | 历史角色 | 终态归属与重构方案 |
| :--- | :--- | :--- |
| `pipi-app` | 原宿主与状态管理混杂层 | 拆解并收敛至宿主层 `pipi-core` |
| `src-tauri` | 早期 Tauri 桌面外壳目录 | 规范化为标准 Cargo 工作区成员 `crates/pipi` |
| `pipi-server` | 独立 Web 服务可执行 crate | 降级为 `pipi-core` 内置的 `--features server` 宿主模式 |
| `pipi-catalog` | 模型目录缓存与抓取 | 收敛并入模型面 `pipi-provider::catalog` |
| `pipi-skills` / `pipi-harness` | 技能与提示词分散实现 | 与 `av` 统一收敛至环境域 `pipi-venv` |
| `pipi-error` | 跨 crate 集中式通用错误枚举 | 彻底消除；采用细粒度 `Result<T, String>` 与 per-crate 领域错误分类 |
| 跨 crate 全包导出 (`pub use pipi_xxx::*`) | 懒惰的转口重导出 | 彻底禁止；消费者必须直接依赖真实属主 crate |

---

### 1.2 现代 12-Crate 体系全景

当前工作区包含且仅包含 12 个成员 crate，形成清晰的单向分层结构：

```
                      ┌────────────────────────┐
                      │    crates/pipi         │  (Tauri 桌面薄壳)
                      └───────────┬────────────┘
                                  │
                                  ▼
                      ┌────────────────────────┐
                      │    pipi-core           │  (宿主层：命令表/运行时索引/调度/Server)
                      └───────────┬────────────┘
         ┌────────────────────────┼────────────────────────┐
         ▼                        ▼                        ▼
┌────────────────┐       ┌────────────────┐       ┌────────────────┐
│   pipi-agent   │       │   pipi-venv    │       │   pipi-tools   │
│  (L5单会话内核) │       │ (环境/契约/技能) │       │  (内置工具/权限) │
└────────┬───────┘       └────────┬───────┘       └────────┬───────┘
         │                        │                        │
         ├────────────────────────┼────────────────────────┤
         ▼                        ▼                        ▼
┌────────────────┐       ┌────────────────┐       ┌────────────────┐
│  pipi-context  │       │  pipi-session  │       │  pipi-provider │
│(上下文/压缩/重试)│      │(Git分支会话载体)│       │ (模型/Catalog) │
└────────┬───────┘       └────────┬───────┘       └────────┬───────┘
         │                        │                        │
         └────────────────────────┼────────────────────────┘
                                  ▼
                         ┌────────────────┐       ┌────────────────┐
                         │ pipi-protocol  │◄──────┤   pipi-forum   │
                         │  (契约与端口层)  │       │  (本地论坛载体) │
                         └────────────────┘       └────────────────┘

       [独立辅助支撑]
       - pipi-test-support : 测试环境隔离脚手架 (仅 dev-dependency)
       - xtask             : 仓库维护任务与依赖合规检查 (cargo xtask arch)
```

| Crate 名称 | 层次划分 | 核心职责 |
| :--- | :--- | :--- |
| **`pipi-protocol`** | 契约层 | DTO 与 Wire 形状、会话条目模型、存储端口（`SessionLog`/`SessionRepo`）、工具/权限/Provider 抽象端口，以及跨层共用的零依赖纯函数助手（fs 守卫、路径转义、ID 生成）。 |
| **`pipi-tools`** | 能力层 | 7 大内置工具实现（`bash`/`read`/`write`/`edit`/`glob`/`grep`/`memory`）及 bash 权限静态判定策略（切分、重定向检查、强制危险命令拦截）。 |
| **`pipi-provider`** | 模型面 | 基于 `rig` 的 HTTP/SSE 流式适配、错误类型严格分类（仅分类、不做重发）、models.dev 目录拉取与本地缓存。 |
| **`pipi-context`** | 策略层 | Token 预算、历史上下文滑动裁剪与配对修复（ToolCall/ToolResult 守恒）、Compaction 压缩流水线与重试退避算法。纯内存策略，不碰载体与网络。 |
| **`pipi-session`** | 会话载体 | 会话物理存储实现：以 `sessions.git` 裸仓库为唯一持久载体（一轮一个 commit）、旧 JSONL 幂等导入通道及设置工作台内存测试账本（`MemoryLog`）。 |
| **`pipi-forum`** | 论坛载体 | 纯文件的本地论坛实现（一个主题一个目录、一条帖子一个文件），实现 `ForumRepo` 端口并提供 `pipi-forum` CLI。 |
| **`pipi-venv`** | 环境域 | Agent 虚拟环境中心：清单与注册表、`agent.toml` 契约体系（`av` CLI + 全局技能 store）、提示词组装流水线。 |
| **`pipi-agent`** | 内核层 | L5 单会话运行内核：运行状态机、单轮独占守卫、abort/steering 调度、账本恢复与统计口径追踪。不碰具体网络与 `agent.toml`。 |
| **`pipi-core`** | 宿主层 | 多会话槽位索引与调度调度、单一公共命令表（`commands.rs`）、事件中心、后台任务编排，以及 `--features server` 提供的 Web 宿主能力。 |
| **`pipi`** | 外壳层 | Tauri 桌面宿主薄壳，仅做 IPC 消息解包与转发，严禁包含任何业务逻辑。 |
| **`pipi-test-support`** | 支撑层 | 进程级全局环境锁（`HOME_LOCK`）、安全改写 `HOME`/`AV_HOME` 的测试脚手架，仅作为内部 `dev-dependency`。 |
| **`xtask`** | 门禁层 | 维护工具，执行 `cargo xtask arch` 针对 `architecture.toml` 展开 AST 级依赖方向与合规性校验。 |

---

## 2. 分层原则与依赖白名单

工作区的依赖关系由根目录的 `architecture.toml` 严格规范。该文件采用 **显式白名单机制（Default-Deny）**：任何未登记的依赖关系均被视作非法并阻断构建。

### 2.1 依赖白名单规则矩阵

| 目标 Crate | 允许依赖的内部 Crate (`may-depend-on`) | 允许的内部开发依赖 (`dev-may-depend-on`) | 显式禁止依赖 (`forbid`) |
| :--- | :--- | :--- | :--- |
| **`pipi-protocol`** | *(无)* | `pipi-test-support` | `pipi-tools`, `pipi-core`, `pipi-agent`, `reqwest`, `rig` |
| **`pipi-context`** | `pipi-protocol` | `pipi-session` | *(无)* |
| **`pipi-session`** | `pipi-protocol` | `pipi-test-support` | *(无)* |
| **`pipi-forum`** | `pipi-protocol` | `pipi-test-support` | *(无)* |
| **`pipi-tools`** | `pipi-protocol` | `pipi-test-support` | *(无)* |
| **`pipi-provider`** | `pipi-protocol` | *(无)* | *(无)* |
| **`pipi-agent`** | `pipi-context`, `pipi-protocol` | `pipi-provider` | `reqwest`, `rig`, `tauri` |
| **`pipi-venv`** | `pipi-protocol`, `pipi-tools` | `pipi-test-support` | `reqwest`, `rig`, `tokio` |
| **`pipi-core`** | `pipi-agent`, `pipi-context`, `pipi-forum`, `pipi-protocol`, `pipi-provider`, `pipi-session`, `pipi-tools`, `pipi-venv` | `pipi-test-support` | `rig` |
| **`pipi`** | `pipi-agent`, `pipi-core`, `pipi-provider`, `pipi-protocol`, `pipi-venv` | *(无)* | *(无)* |
| **`pipi-test-support`**| *(无)* | *(无)* | *(无)* |
| **`xtask`** | *(无)* | *(无)* | *(无)* |

### 2.2 核心禁止规则与边界不变量

1. **`pipi-protocol` 绝对纯洁性**：
   - 不允许依赖任何其他工作区内部实现 crate。
   - 第三方异步运行时 `tokio` 仅允许开启 `features = ["sync"]`（用于 Provider 事件流的多生产者多消费者通道）；严禁引入 `tokio/rt`、`tokio/net` 或 `tokio/time`。
2. **`pipi-agent` 引擎层无网络、无 Tauri**：
   - 单会话内核必须保持平台无关与网络无关，Provider 仅以 `pipi-protocol` 抽象 trait 注入。生产依赖中严禁包含 `reqwest`、`rig` 或 `tauri`。
3. **`pipi-venv` 无重型网络运行时**：
   - Agent 契约与技能管理默认本地运行，生产依赖禁止 `reqwest`、`rig` 和 `tokio`。只有独立 CLI 二进制（`av`）在显式开启 `search` feature 时才启用微型 HTTP 客户端。
4. **进程 Spawn 物理隔离边界**：
   - 全工作区中，**`pipi-session` 是唯一允许为会话存储持久化 spawn `git` 子进程的 crate**。
   - `pipi-venv` 中的 `av` 模块仅为技能包远端拉取执行 git 操作，二者职责绝不交叉。内核与宿主严禁自行 spawn git。

---

## 3. 机器门禁检查机制：`cargo xtask arch`

为了防止人为疏忽导致的依赖渗漏，工作区将依赖拓扑的合法性固化为自动化机检工具 `cargo xtask arch`。

### 3.1 检查机理

`crates/xtask/src/arch.rs` 是一个纯函数化校验引擎：
1. **元数据提取**：扫描工作区根目录 `Cargo.toml` 以及各子 crate 的 `Cargo.toml`，解析其实际的 `[dependencies]`、`[dev-dependencies]`、`[build-dependencies]` 与 `[target.*.dependencies]`。
2. **规则对比**：逐一对比 `architecture.toml`：
   - `Unregistered`：存在于工作区但未在 `architecture.toml` 登记的 Crate。
   - `UnknownCrate`：在规则中登记但已不在工作区存在的残留项。
   - `Undeclared`：实际依赖的内部 crate 未被列入 `may-depend-on` 或 `dev-may-depend-on`。
   - `Forbidden`：点名禁用的第三方依赖出现在生产依赖树中。
   - `Cycle`：基于拓扑排序检查生产依赖图中是否存在循环引用（Cycle）。
3. **Fail-Closed 退出**：一旦检测到任何违规，控制台输出精准错误并以非零状态码退出，直接阻断本地提交与 CI 流水线。

### 3.2 开发者指引

- **新增工作区 Crate 或调整依赖关系时**：
  1. 首先修改根目录 `architecture.toml`，在对应段落添加白名单并阐述设计意图。
  2. 修改子 crate 的 `Cargo.toml`。
  3. 执行 `cargo xtask arch` 确认门禁通过。

---

## 4. 模块系统与反转口（Anti-Re-Export）规范

代码规范直接决定系统长期的可维护性。Pipi 在 Rust 模块组织与接口导出上贯彻极简与显式原则。

### 4.1 严禁跨 Crate 转口 Re-Export

- **规范要求**：类型、Trait 与逻辑函数一律由其**属主 crate** 直接导出。消费者必须在自身的 `Cargo.toml` 中显式引入属主 crate 并直接 `use`。
- **禁止反模式**：
  - 严禁在中间层（如 `pipi-core`）创建 `pub use pipi_tools::*`、`pub use pipi_protocol::types::*` 等“便利型”整包转口。
  - 杜绝门面伪装：外部调用方不得通过 `pipi_core::SessionSummary` 访问本属于 `pipi_protocol::session::SessionSummary` 的类型。
- **正当的局部门面**：单个 crate 内部的 `mod.rs` 使用 `pub use sub_module::Type;` 聚合 crate 自身对外公共 API 属于 Rust 标准设计，不在禁止之列。

### 4.2 模块代码规模与 ~500 行拆分规则

- **拆分阈值**：单个模块文件超过 **~500 行生产代码** 时，必须拆分为目录模块（如 `session.rs` 超过 500 行即拆解为 `session/mod.rs` 门面及若干专注于单一变更理由的子文件）。
- **行数统计口径**：文件底部的 `#[cfg(test)] mod tests` 测试代码**不计入**这 500 行限制。
- **目录内聚合**：`mod.rs` 仅负责模块级文档、`mod` 声明与必要的 `pub use` 门面暴露，具体业务实现按职责分散在兄弟文件中。

### 4.3 测试布局与测试脚手架隔离

Pipi 采用严格的三层测试架构，杜绝分散无序的测试文件：

```
crates/<crate>/
├── src/
│   ├── foo.rs               # 生产代码
│   │   └── #[cfg(test)]     # 单元测试：内联在被测文件底部
│   │       mod tests { ... }
│   ├── test_support.rs      # crate 内部跨模块共用的 Test Fixtures
│   └── lib.rs
└── tests/                   # 集成测试：黑盒回归，独立编译为外部 binary
    └── integration_test.rs
```

1. **单元测试内联化**：
   - 测哪个文件就内联在该文件底部（`#[cfg(test)] mod tests { use super::*; ... }`）。
   - 禁止在 `src/` 下创建如 `foo/tests.rs`、`tests_foo.rs` 等外部孤立文件，严禁使用 `#[path]` 黑魔法引入测试代码。
2. **跨模块共享夹具**：
   - 仅在单一 crate 内部共享的测试夹具，统一放置于 `src/test_support.rs`（声明为 `#[cfg(test)] mod test_support;`）。
3. **黑盒集成测试**：
   - 验证跨模块端到端生命周期的测试，统一放置于 `crates/<crate>/tests/*.rs`，仅调用公开的 `pub API`。
4. **全局环境安全改写规范**：
   - 针对 `HOME`、`AV_HOME`、`PIPI_HOME` 的环境改写，必须使用 `pipi-test-support` 提供的安全 API。
   - 任何改写与读取进程环境的测试用例，**必须首先获取全局唯一的 `pipi_test_support::HOME_LOCK`**，严禁并发污染。

---

## 5. 架构不变量核对清单

在提交任何架构改动前，开发者与评审者必须对照以下清单逐项核查：

- [ ] 是否运行了 `cargo xtask arch` 且结果为 0 违规、0 警告？
- [ ] 提交代码中是否杜绝了所有已废弃名称（`pipi-app`、`src-tauri`、`pipi-error` 等）？
- [ ] 生产依赖图谱中是否不存在任何形式的循环依赖（Cycle-free）？
- [ ] 是否未在任何 crate 中引入违规转口 `pub use other_crate::*`？
- [ ] 超过 500 行生产代码的文件是否已合理拆分为目录模块？
- [ ] 测试用例中的环境变量修改是否由 `HOME_LOCK` 严格串行化？
