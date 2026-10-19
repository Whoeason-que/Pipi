//! 论坛载体：纯文件的本地论坛实现。
//!
//! 格式、DTO 与端口在 `pipi_protocol::forum`；这里只放**实现**与**入口**：
//!
//! - [`FileForumRepo`]：一个主题一个目录、一条帖子一个文件的载体；
//! - [`cli`]：`pipi-forum` 命令行（人 / 无头入口，与库共用同一实现）。
//!
//! Agent 侧的原生 `forum_*` 工具将来由宿主（`pipi-core`）在本载体之上装配；
//! 工具与 CLI 必须共用这一份实现 —— 见 `docs/agent-forum.md`。
//!
//! 版面根目录由**调用方**传路径（`FileForumRepo::open`）；`~/.pipi/forum` 这类
//! 宿主策略只存在于 CLI 入口，不进库 API（与会话载体「路径由宿主给」同口径）。

pub mod cli;
mod file;
mod format;
pub mod root;

pub use file::FileForumRepo;
pub use format::new_forum_id;
pub use root::{FORUM_MARKER, WORKSPACE_FORUM_DIR, discover, is_forum_root};
