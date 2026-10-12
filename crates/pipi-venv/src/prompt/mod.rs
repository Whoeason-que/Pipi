//! Pipi 的纯 harness 层。
//!
//! 这里只做项目上下文发现和系统提示词渲染：不读 Agent 注册表、不执行工具，
//! 也不依赖运行时或 provider。宿主负责把配置、工具说明和已读取的资源传进来。

use std::path::{Path, PathBuf};

pub mod environment;
pub mod resources;
pub mod system_prompt;

pub use environment::environment_context;
pub use system_prompt::{
    BuildSystemPromptOptions, ContextFile, MemoryFileMeta, SkillMetadata, build_system_prompt,
};

/// 单个项目文档的默认字节预算（对齐 codex 的 project_doc_max_bytes）。
pub const DEFAULT_PROJECT_DOC_MAX_BYTES: usize = 16 * 1024;

/// 在没有项目根时，资源 loader 只扫描 cwd；有项目根时从根到 cwd 逐层扫描。
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cursor = start;
    loop {
        if cursor.join(".git").exists() {
            return Some(cursor.to_path_buf());
        }
        cursor = cursor.parent()?;
    }
}
