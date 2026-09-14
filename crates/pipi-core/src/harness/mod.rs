//! 纯 harness 组件。

pub mod resources;
pub mod system_prompt;

pub use system_prompt::{
    build_system_prompt, BuildSystemPromptOptions, ContextFile, MemoryFileMeta, SkillMetadata,
};
