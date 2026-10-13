//! Pipi Agent 引擎 —— 从 [pi](https://github.com/earendil-works/pi) 移植的运行时内核。
//!
//! 这一层是**可复用、与宿主无关、不碰 Agent 注册表、不做网络目录**的部分：
//! L5 单会话生命周期、主循环、账本恢复与用量统计；上下文预算、压缩与重试
//! 算法在 `pipi-context`，单会话压缩编排在本层，会话载体在
//! `pipi-session`，具体 provider 只在测试里出现（生产依赖是协议层的 `Provider`
//! 端口）—— 对齐 pi 把 `harness/{runtime,session,compaction}` 收在同一个
//! `pi-agent-core` 包里的分组。
//!
//! | 模块 | 职责 | pi 对应 |
//! | --- | --- | --- |
//! | [`agent_loop`] | 主循环 + steering/follow-up + 工具批次执行 | `packages/agent/src/agent-loop.ts` |
//! | [`session`] | 单会话运行、恢复、记账、压缩与换号；能力均由宿主注入 | harness/runtime + session 生命周期 |
//! | [`stats`] | 会话统计（滚动窗口、命中率） | hermes 口径，见 README |
//!
//! 不属于这里的：Agent 定义与 av 契约、技能中心、提示词装配（`pipi-venv`），设置与
//! 模型目录（`pipi-core` / `pipi-provider::catalog`），会话槽、后台编排与宿主事件（`pipi-core`）。

pub mod agent_loop;
pub mod session;
pub mod stats;
