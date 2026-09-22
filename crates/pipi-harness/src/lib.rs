//! Pipi 的纯 harness 层。
//!
//! 这里只做项目上下文发现和系统提示词渲染：不读 Agent 注册表、不执行工具，
//! 也不依赖运行时或 provider。宿主负责把配置、工具说明和已读取的资源传进来。

use std::path::{Path, PathBuf};

pub mod resources;
pub mod system_prompt;

pub use system_prompt::{
    build_system_prompt, BuildSystemPromptOptions, ContextFile, MemoryFileMeta, SkillMetadata,
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

/// 将文件系统路径安全地渲染进 prompt：规范化分隔符、可见化控制字符并
/// 转义 XML 特殊字符，避免路径破坏上下文标签或注入额外行。
pub fn escape_path_for_prompt(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let mut sanitized = String::with_capacity(normalized.len());
    for character in normalized.chars() {
        match character {
            '\n' => sanitized.push_str("\\n"),
            '\r' => sanitized.push_str("\\r"),
            '\t' => sanitized.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write;
                write!(sanitized, "\\u{{{:x}}}", character as u32).unwrap();
            }
            character => sanitized.push(character),
        }
    }
    sanitized
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::escape_path_for_prompt;

    #[test]
    fn path_escaping_preserves_prompt_structure() {
        assert_eq!(
            escape_path_for_prompt("C:\\work\n<unsafe>&\"'"),
            "C:/work\\n&lt;unsafe&gt;&amp;&quot;&apos;"
        );
    }
}
