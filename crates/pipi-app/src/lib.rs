//! Pipi 的应用服务层。
//!
//! 领域逻辑在 `pipi-core` / `pipi-agent` / `pipi-skills` / `pipi-provider::catalog`；
//! 这里把它们组装为带会话槽、后台运行和宿主事件的应用服务，Tauri 与 Web
//! server 共用这一层，因此不会各自复制 Agent 调度逻辑或改变 IPC event 的载荷。
//!
//! 模块依赖方向：`events` 是叶子（宿主事件协议），`approval` 与 `runtime` 都
//! 依赖它；`background` 通过 [`background::ChildAgentRunner`] 端口拿到 child
//! 运行能力，由 `runtime` 注入 —— 三者之间没有环。

pub mod approval;
pub mod background;
pub mod commands;
pub mod events;
pub mod runtime;
