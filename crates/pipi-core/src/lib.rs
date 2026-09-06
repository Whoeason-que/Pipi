//! Pipi 核心 —— 从 [pi](https://github.com/earendil-works/pi) 移植的 agent 内核。
//!
//! 与 pi 的模块对应关系（移植时保持概念与 JSON 格式尽量对齐，便于对照上游修改）：
//!
//! | pi | pipi-core | 说明 |
//! | --- | --- | --- |
//! | `packages/ai` types | [`types`] | 消息 / 内容块 / 流式事件协议 |
//! | `packages/ai` api adapters | [`provider`] | 仅移植 anthropic-messages 与 openai-completions 两个 |
//! | `packages/agent` agent-loop | [`agent_loop`] | 主循环 + steering/follow-up + 工具批次执行 |
//! | `packages/agent` harness/tools | [`tools`] | read / write / edit / bash / memory |
//! | `packages/agent` harness/utils/truncate | [`truncate`] | 2000 行 / 50KB 截断规则 |
//! | `packages/agent` harness/session | [`session`] | 树状 JSONL 条目（append-only） |
//! | `packages/agent` compaction（启发式部分） | [`context`] | token 估算 / 裁剪 / 环境上下文 |
//! | `packages/agent` skills（frontmatter + 索引） | [`skills`] | 渐进式披露：索引常驻、全文按需 read |
//! | （Pipi 新增） | [`permissions`] | bash 命令权限：白名单 / 黑名单 |
//! | （Pipi 新增） | [`agents`] | Agent 定义与注册表（一切皆文件） |
//!
//! 另有 codex 移植：[`permissions::safety`]（危险命令）、[`permissions`]
//! 的 SandboxMode、[`project_doc`]（AGENTS.md 发现），见 README「与 codex
//! 的关系」。
//!
//! 尚未移植（有意推迟，需要时再从上游搬运）：
//! compaction 的 LLM 摘要替换（现用 [`context::prune_oldest`] 保底）、
//! hooks 全集、prepareNextTurn、多 provider 目录、图片工具等。

pub mod agent_loop;
pub mod agents;
pub mod context;
pub mod permissions;
pub mod project_doc;
pub mod provider;
pub mod session;


pub mod settings;
pub mod skills;
pub mod stats;
pub mod tools;
pub mod truncate;
pub mod types;
