//! 运行环境块：把工作目录、沙箱策略与日期渲染进系统提示。
//!
//! 移植自 codex `core/src/context/environment_context.rs` 的思路（上游作为独立
//! user fragment 注入，Pipi 拼进系统提示）。

use std::path::Path;

use pipi_protocol::permissions::SandboxMode;
use pipi_protocol::text::escape_path_for_prompt;

/// 运行环境上下文块（codex 思路，简化渲染）。
pub fn environment_context(workspace: &Path, sandbox: &SandboxMode) -> String {
    let cwd = escape_path_for_prompt(&workspace.display().to_string());
    format!(
        "<environment_context>\n<cwd>{cwd}</cwd>\n<sandbox>{}</sandbox>\n<platform>{}</platform>\n<date>{}</date>\n</environment_context>",
        sandbox.as_str(),
        std::env::consts::OS,
        today(),
    )
}

/// ISO 日期（YYYY-MM-DD），无 chrono 依赖的 civil 算法。
fn today() -> String {
    let days = pipi_protocol::ids::now_secs() / 86_400;
    let (year, month, day) = crate::av::store::civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_context_renders_tags_and_escapes_hostile_workspace_path() {
        let ctx = environment_context(Path::new("/home/u/proj"), &SandboxMode::WorkspaceWrite);
        assert!(ctx.contains("<environment_context>"));
        assert!(ctx.contains("<cwd>/home/u/proj</cwd>"));
        assert!(ctx.contains("<sandbox>workspace-write</sandbox>"));
        assert!(ctx.contains("<date>20"));

        // 工作目录来自用户/仓库，必须转义，不能破坏 XML 结构
        let hostile = environment_context(
            Path::new("/tmp/a<&>\"\'\nnext"),
            &SandboxMode::WorkspaceWrite,
        );
        assert!(hostile.contains("<cwd>/tmp/a&lt;&amp;&gt;&quot;&apos;\\nnext</cwd>"));
        assert!(!hostile.contains("<cwd>/tmp/a<&>\"\'"));
    }
}
