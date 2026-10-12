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
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    // Howard Hinnant 的 civil_from_days
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
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
