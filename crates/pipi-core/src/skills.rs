//! 技能（Skills）索引 —— 实现已下沉到独立标准 `av::skills`，
//! 这里保留兼容 re-export，宿主与既有调用点无需改动。
//!
//! 规则见 `crates/av/src/skills.rs` 的模块文档：只有名称与描述常驻上下文，
//! 模型用 read 工具按需读取 SKILL.md 全文。

pub use av::skills::{filter_skills_by_name, load_skill_sources, parse_frontmatter, SkillMeta};
