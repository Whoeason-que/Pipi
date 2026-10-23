//! 工作区只读视图。桌面与 Web 共用路径边界、限额和版本身份，不触碰会话账本。

mod files;
mod git;
mod paths;

pub use files::{WorkspaceEntry, WorkspaceFile, WorkspaceListing, list_files, preview_file};
pub use git::{WorkspaceChange, WorkspaceChanges, WorkspaceDiff, WorkspaceRepoInfo, changes, diff};
