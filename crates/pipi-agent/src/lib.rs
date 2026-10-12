//! Pipi Agent 引擎 —— 从 [pi](https://github.com/earendil-works/pi) 移植的运行时内核。
//!
//! 这一层是**可复用、与宿主无关、不碰 Agent 注册表、不做网络目录**的部分：
//! 主循环、上下文预算、重试策略、用量统计、会话 JSONL 存储与压缩。它只依赖
//! 协议 / provider / 工具 / prompt 渲染这些基础 crate —— 对齐 pi 把
//! `harness/{runtime,session,compaction}` 收在同一个 `pi-agent-core` 包里的分组。
//!
//! | 模块 | 职责 | pi 对应 |
//! | --- | --- | --- |
//! | [`agent_loop`] | 主循环 + steering/follow-up + 工具批次执行 | `packages/agent/src/agent-loop.ts` |
//! | [`context`] | token 估算 / 裁剪 / 环境上下文 | `harness/compaction`（启发式部分） |
//! | [`compaction`] | 摘要替换旧轮次 + 旧工具输出清理 | `harness/compaction`（LLM 摘要） |
//! //! | [`retry`] | 可重试错误的退避决策 | （Pipi 新增） |
//! | [`stats`] | 会话统计（滚动窗口、命中率） | hermes 口径，见 README |
//!
//! 不属于这里的：Agent 定义与 av 契约、技能中心、模型目录、设置（`pipi-core` /
//! `pipi-skills` / `pipi-catalog`），会话槽、后台编排与宿主事件（`pipi-app`）。

pub mod agent_loop;
pub mod compaction;
pub mod context;
pub mod retry;
pub mod stats;

#[cfg(test)]
mod test_support;
