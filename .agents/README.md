# Pipi Agent 规范与工程知识库 (`.agents`)

本目录收敛 Pipi 仓库的核心设计规范、系统架构决策及历史演进方案。

---

## 目录结构

```
.agents/
├── README.md                                  # 本文件：目录导航与规范维护指引
├── specs/                                     # [当前有效] 核心架构规范与设计标准
│   ├── 01-workspace-architecture.md           # 12-crate 工作区架构、分层与依赖白名单
│   ├── 02-agent-contract-and-materials.md     # agent.toml 路径权威化与 Agent 素材加载规范
│   ├── 03-git-session-carrier.md              # Git 会话载体与提交点生命周期规范
│   ├── 04-av-skills-ecosystem.md              # av 技能包管理、全局 Store 与锁机制规范
│   ├── 05-retry-and-protocol-negotiation.md   # [合并] 请求重试机制与运行时协议协商规范
│   └── 06-context-compaction.md               # [合并] 静态化压缩管线、百分比阈值与手动压缩规范
└── archive/                                   # [历史归档] 早期单会话实施计划与阶段性方案
    ├── 2026-09-07_pipi-agent-hardening.md     # 早期单会话可靠性与安全基线（原 20260907_115354-...）
    ├── 2026-09-07_pi-harness-parity.md        # 早期 Pi harness 兼容基线（原 2026-09-07_225046-...）
    ├── 2026-09-14_compaction-and-glob-grep.md # 首版 LLM 压缩与 glob/grep 工具引入（原 plan-sess_dc4c2b8c）
    ├── 2026-09-21_compaction-percentage-and-manual.md # 压缩百分比阈值与立即压缩（原 plan-sess_c5af0335）
    ├── 2026-09-21_retry-mechanism.md          # 瞬时错误重试机制与分类（原 plan-sess_1e528311）
    ├── 2026-10-02_av-global-skills-store.md   # av 全局技能库与锁文件机制（原 plan-sess_aa20be20）
    ├── 2026-10-12_crate-split-phase1.md       # 工作区 10->13 crate 拆分草案（原 plan-sess_d08a369f）
    ├── 2026-10-12_crate-restructure-12crates.md # 终态 15->12 crate 结构重构计划（原 plan-sess_d6cc490e）
    ├── 2026-10-12_protocol-auto-negotiation.md # 协议 auto/manual 模式与 400 协商（原 plan-sess_d4a9c900）
    ├── 2026-10-13_git-session-carrier.md      # Git 会话载体全面切换实施（原 plan-sess_ab4582ae）
    ├── 2026-10-13_agent-toml-prompt-materials.md # 素材路径权威转移至 agent.toml（原 plan-sess_a5b856de）
    ├── 2026-10-13_refactor-p1-fixes.md        # 重构审查 P1 修复与机械项清理（原 plan-sess_dce287c7）
    └── 2026-10-13_pipi-error-dissolution.md   # pipi-error 消解与 send_prompt 身份确立（原 plan-sess_9bc81c7b）
```

---

## 核心设计规范 (Specs) 导览

| 规范文档 | 核心主题 | 关键设计与不变量 |
| :--- | :--- | :--- |
| **[`01-workspace-architecture.md`](specs/01-workspace-architecture.md)** | 12-crate 分层体系 | 严格单向依赖 DAG，`cargo xtask arch` 机器门禁，契约层 `pipi-protocol` 零业务逻辑，不设跨 crate 转口 re-export。 |
| **[`02-agent-contract-and-materials.md`](specs/02-agent-contract-and-materials.md)** | Agent 素材权威化 | `agent.toml`（av 规范）是指令/记忆/技能来源的唯一权威；三项必需显式声明；项目层绝对禁止声明 `[resources].memory`。 |
| **[`03-git-session-carrier.md`](specs/03-git-session-carrier.md)** | Git 会话载体 | `sessions.git` 裸仓库为唯一会话载体；一条 entry 一个文件，一轮一个 commit；严格受控的 6 处内核提交点；旧 JSONL 幂等导入。 |
| **[`04-av-skills-ecosystem.md`](specs/04-av-skills-ecosystem.md)** | av 技能包生态 | 全局 store 内容寻址不可变；`agent.lock` 权威解析；「安装 ≠ 启用」；会话启动离线只读；供应链沙箱安全。 |
| **[`05-retry-and-protocol-negotiation.md`](specs/05-retry-and-protocol-negotiation.md)** | 请求重试与协议协商 | 两层架构（provider 分类 + agent_loop 重发）；用户中止/已出正文不重试；400 协议不匹配单次翻转协商与进程内 memo。 |
| **[`06-context-compaction.md`](specs/06-context-compaction.md)** | 上下文压缩体系 | 投影式（纯函数不落盘）与替换式（落盘条目）严格分离；具象顺序调用；百分比阈值（默认 75%）与手动压缩（遵守分叉归档）。 |

---

## 历史归档索引 (Archive)

所有在开发过程中阶段性使用的会话实施计划（`plan-sess_*`）在完全落地并被架构吸收后，统一按「日期_语义主题」重命名归入 `archive/` 目录，供历史追溯与审计对照：

| 归档文件 | 原文件名 / 来源 | 落地提交 / 里程碑 | 说明 |
| :--- | :--- | :--- | :--- |
| **`2026-09-07_pipi-agent-hardening.md`** | `20260907_115354-pipi-agent-hardening.md` | 初期 Commit 系列 | 初期单会话 Agent 可靠性与安全闭环设计 |
| **`2026-09-07_pi-harness-parity.md`** | `2026-09-07_225046-pi-harness-parity.md` | 初期 Commit 系列 | Pi-compatible harness 第一阶段对齐 |
| **`2026-09-14_compaction-and-glob-grep.md`** | `plan-sess_dc4c2b8c-8a12-4023-8773-5f2575855e06.md` | `71bb02ac` 等 | 首版 LLM 摘要压缩与 glob/grep 检索工具 |
| **`2026-09-21_compaction-percentage-and-manual.md`** | `plan-sess_c5af0335-8aa0-47f5-992a-3562b74ec9d5.md` | `dae8aede` / `4e152448` | 百分比阈值触发与手动立即压缩 |
| **`2026-09-21_retry-mechanism.md`** | `plan-sess_1e528311-2ef0-41e5-9ed2-8ea74406aad4.md` | `4e152448` | 端到端请求重试与结构化错误分类 |
| **`2026-10-02_av-global-skills-store.md`** | `plan-sess_aa20be20-4705-4644-ad0c-9961f8d3c245.md` | `4e152448` | av 技能全局 Store、版本锁定与 CLI 系列 |
| **`2026-10-12_crate-split-phase1.md`** | `plan-sess_d08a369f-d650-4212-88af-bfac3498c549.md` | `e0e50e5f` 等 | 工作区 10->13 crate 拆分过渡草案 |
| **`2026-10-12_crate-restructure-12crates.md`** | `plan-sess_d6cc490e-648d-45e6-9d29-0fc8686f4b14.md` | `7f74760d` 等 | 终态 15->12 crate 结构重构方案 |
| **`2026-10-12_protocol-auto-negotiation.md`** | `plan-sess_d4a9c900-9011-4b65-bca4-f2faf3b712d3.md` | `03e5342f` | 模型协议 auto/manual 与 400 运行期协商 |
| **`2026-10-13_git-session-carrier.md`** | `plan-sess_ab4582ae-ee65-44ec-818f-6019c6446274.md` | `feat/git-session-store` | 会话载体全面切换为 Git bare 仓库与分支 |
| **`2026-10-13_agent-toml-prompt-materials.md`** | `plan-sess_a5b856de-f2d7-475b-8324-957dabfff6c9.md` | `166572b` 等 | Agent 侧提示词素材路径权威转移至 agent.toml |
| **`2026-10-13_refactor-p1-fixes.md`** | `plan-sess_dce287c7-8be7-4a99-85d2-c00b832ae8b6.md` | `bf897a3a` 等 | 符号链接 fail-closed、IPC 契约断言等 P1 修复 |
| **`2026-10-13_pipi-error-dissolution.md`** | `plan-sess_9bc81c7b-5e05-4c50-a838-1e4f45ada84e.md` | `90a0042` / `f04b382` | pipi-error 消除、send_prompt 身份确立、压缩管线静态化 |

---

## 规范维护与流转指南

1. **会话草稿与正式规范流转**：
   - AI Agent 或协作者在本地开发的临时会话计划（如 `.zcode/plans/plan-sess_*.md`）属于工作区草稿（`.gitignore` 已忽略）。
   - 跨模块或架构级改动落地后，应当将沉淀下的核心设计总结更新到 `.agents/specs/` 中，保持与最新代码库 1:1 对齐。
   - 临时性、阶段性计划在任务完成后移至 `.agents/archive/` 并使用语义化重命名。
2. **代码路径与命名严格同步**：
   - 编写与维护 specs 时，严格禁止使用已被合并或废弃的 crate 路径（如 `pipi-app`、`src-tauri`、`pipi-error` 等）。
   - 任何涉及 Cargo 依赖与分层的变更，必须以 `cargo xtask arch` 通过为硬性前提。
