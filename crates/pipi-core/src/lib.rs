//! Pipi 核心 —— 从 [pi](https://github.com/earendil-works/pi) 移植的 agent 内核。
//!
//! 与 pi 的模块对应关系（移植时保持概念与 JSON 格式尽量对齐，便于对照上游修改）：
//!
//! | pi | pipi-core | 说明 |
//! | --- | --- | --- |
//! | `packages/ai` types | `pipi-protocol`（经 [`types`] 兼容导出） | 消息 / 内容块 / 流式事件协议 |
//! | `packages/ai` api adapters | `pipi-provider`（经 [`provider`] 兼容导出） | 仅移植 anthropic-messages 与 openai-completions 两个 |
//! | `packages/agent` agent-loop | [`agent_loop`] | 主循环 + steering/follow-up + 工具批次执行 |
//! | `packages/agent` harness/tools | `pipi-tools` + [`tools`] | 基础文件工具；核心保留 Agent 组合工具 |
//! | `packages/agent` harness/utils/truncate | `pipi-tools::truncate`（经 [`truncate`] 兼容导出） | 2000 行 / 50KB 截断规则 |
//! | `packages/agent` harness/session | [`session`] | 树状 JSONL 条目（append-only） |
//! | （Pipi 应用层） | `pipi-app::runtime` | 会话槽、后台编排与宿主事件；Tauri/Web 共用 |
//! | `packages/agent` harness prompt/resources | `pipi-harness`（经 [`harness`] 兼容导出） | 纯 prompt 渲染和项目上下文发现 |
//! | `packages/agent` compaction（启发式部分） | [`context`] | token 估算 / 裁剪 / 环境上下文 |
//! | `packages/agent` compaction（LLM 摘要替换） | [`compaction`] | 摘要替换旧轮次 + 保留近期轮次，落盘为 compaction 条目 |
//! | `packages/agent` skills（frontmatter + 索引） | [`skills`] | 渐进式披露：索引常驻、全文按需 read |
//! | （Pipi 新增） | `pipi-tools::permissions`（经 [`permissions`] 兼容导出） | bash 命令权限：白名单 / 黑名单 |
//! | （Pipi 新增） | [`agents`] | Agent 定义与注册表（一切皆文件） |
//!
//! 另有 codex 移植：[`permissions::safety`]（危险命令）、[`permissions`]
//! 的 SandboxMode、[`project_doc`]（AGENTS.md 发现），见 README「与 codex
//! 的关系」。
//!
//! 尚未移植（有意推迟，需要时再从上游搬运）：
//! hooks 全集、prepareNextTurn、多 provider 目录、图片工具等。

pub mod agent_loop;
pub mod agents;
pub mod catalog;
pub mod compaction;
pub mod context;
pub mod harness;
pub mod permissions;
pub mod project_doc;
pub mod provider;
pub mod retry;
pub mod session;

pub mod settings;
pub mod skills;
pub mod stats;
pub mod tools;
pub mod truncate;
pub mod types;

/// HOME 环境变量是进程级的：所有涉及 `~/.pipi` 的测试（改写 HOME 或依赖
/// 环境 HOME 下的 settings）都必须持有此锁串行执行，否则并行测试互相污染。
#[cfg(test)]
pub(crate) static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
