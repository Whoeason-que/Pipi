//! bash 命令总闸（实现侧）：切分 → 白/黑名单 → 危险命令 → 沙箱策略。
//!
//! `PermissionsConfig` 本体（数据）在 `pipi_protocol::permissions` —— 清单与
//! 引擎都要读它。这里用扩展 trait 给它挂上评估逻辑：数据属契约，策略属实现。

use std::path::Path;
use std::sync::Once;

use pipi_protocol::permissions::{BashAssessment, BashMode, PermissionsConfig, SandboxMode};

use super::parse::{matches_entry, split_segments};
use super::redirect::{RedirectCheck, check_write_redirect};
use super::rm::forced_rm_inside_workspace;
use super::safety::{DangerousCommandMatch, dangerous_command_match};

/// bash 命令评估入口。调用点 `use pipi_tools::permissions::BashPolicy;` 即可。
pub trait BashPolicy {
    /// bash 命令分类评估。只有「Allowlist 模式下白名单未命中、且不命中危险
    /// 规则」的段落可以交给交互审批救回；黑名单命中、危险命令、沙箱约束
    /// 一律硬拒 —— 宁可拒绝不可放行。
    fn assess_bash_classified(&self, command: &str, workspace: &Path) -> BashAssessment;
}

impl BashPolicy for PermissionsConfig {
    fn assess_bash_classified(&self, command: &str, workspace: &Path) -> BashAssessment {
        if self.sandbox == SandboxMode::ReadOnly {
            return BashAssessment::HardDenied(
                "Sandbox policy is read-only: command execution is not allowed (adjust the Agent's sandbox settings if execution is needed)".into(),
            );
        }
        if self.sandbox == SandboxMode::WorkspaceWrite {
            match check_write_redirect(command) {
                RedirectCheck::Ok => {}
                RedirectCheck::Write(token) => {
                    return BashAssessment::HardDenied(format!(
                        "Sandbox policy is workspace-write: write redirection \"{token}\" denied; use write / edit tools for workspace writes. \
                         2>/dev/null and 2>&1 can be used to discard or merge output; pipes and heredocs are unrestricted"
                    ));
                }
                RedirectCheck::Read(token) => {
                    return BashAssessment::HardDenied(format!(
                        "Sandbox policy is workspace-write: input redirection \"{token}\" denied; use read tool \
                         for file reading (with workspace containment check); heredocs (<<EOF) can be used to feed stdin"
                    ));
                }
                RedirectCheck::Unparseable(reason) => {
                    return BashAssessment::HardDenied(format!(
                        "Sandbox policy is workspace-write: {reason}, unable to statically analyze redirection in command, denied"
                    ));
                }
            }
        }

        let segments = match split_segments(command) {
            Ok(segments) => segments,
            Err(error) => return BashAssessment::HardDenied(error),
        };

        let mut missing: Vec<String> = Vec::new();
        for seg in &segments {
            match self.bash.mode {
                BashMode::AllowAll => {}
                BashMode::Allowlist => {
                    if !self
                        .bash
                        .commands
                        .iter()
                        .any(|c| matches_entry(c, &seg.text))
                    {
                        // 危险段落即使等用户批准也不放行
                        if self.sandbox != SandboxMode::DangerFullAccess
                            && dangerous_command_match(&seg.argv).is_some()
                        {
                            return BashAssessment::HardDenied(format!(
                                "\"{}\" matches dangerous command rules, denied under {} sandbox (cannot be approved)",
                                seg.text,
                                self.sandbox.as_str()
                            ));
                        }
                        missing.push(seg.text.clone());
                        continue;
                    }
                }
                BashMode::Denylist => {
                    if self
                        .bash
                        .commands
                        .iter()
                        .any(|c| matches_entry(c, &seg.text))
                    {
                        return BashAssessment::HardDenied(format!(
                            "Command \"{}\" is forbidden by denylist",
                            seg.text
                        ));
                    }
                }
            }
            if self.sandbox != SandboxMode::DangerFullAccess
                && let Some(matched) = dangerous_command_match(&seg.argv)
            {
                // 工作区内的字面量 `rm -f`：清理构建 / 缓存产物的常规操作，
                // 放行；其余强制删除（含包装调用、越界或不可解析的目标）照旧拒绝。
                let workspace_contained = matches!(matched, DangerousCommandMatch::ForcedRm)
                    && forced_rm_inside_workspace(&seg.argv, workspace, command);
                if !workspace_contained {
                    return BashAssessment::HardDenied(match matched {
                        DangerousCommandMatch::ForcedRm => format!(
                            "\"{}\" contains forced deletion (rm -f family), denied under {} sandbox (only literal absolute paths \
                             inside workspace and not at repository root are allowed; targets containing variables / wildcards, and \
                             relative paths when the command contains cd / pushd cannot be approved)",
                            seg.text,
                            self.sandbox.as_str()
                        ),
                        DangerousCommandMatch::Other => format!(
                            "\"{}\" matches dangerous command rules, denied under {} sandbox",
                            seg.text,
                            self.sandbox.as_str()
                        ),
                    });
                }
            }
        }

        if missing.is_empty() {
            BashAssessment::Allowed
        } else {
            BashAssessment::NeedsApproval { missing }
        }
    }
}

/// 统一检查待执行的 bash 命令。
///
/// **沙箱生效时这里直接放行**：Landlock/Seatbelt 已经在内核里限制了文件写与
/// 网络，命令文本（`$()`、`eval`、管道喂给 shell……）不再参与安全判定。旧的
/// 白/黑名单、危险命令与重定向检查只在没有沙箱可用时作为回退路径运行，
/// 并通过交互审批处理 allowlist 未命中的段落。
pub async fn check_bash_command(
    ctx: &pipi_protocol::tools::ToolContext,
    command: &str,
) -> Result<(), String> {
    if pipi_sandbox::will_enforce(&ctx.sandbox_policy()) {
        // 迁移期提示：沙箱接管后，旧的 allowlist/denylist 不再执行。
        if ctx.permissions.bash.mode != BashMode::AllowAll
            || !ctx.permissions.bash.commands.is_empty()
        {
            static NOTICE: Once = Once::new();
            NOTICE.call_once(|| {
                eprintln!(
                    "pipi: OS sandbox is active; permissions.bash allowlist/denylist are only enforced when PIPI_SANDBOX=off or the helper is unavailable"
                );
            });
        }
        return Ok(());
    }

    match ctx.permissions.assess_bash_classified(command, &ctx.workspace) {
        BashAssessment::Allowed => Ok(()),
        BashAssessment::HardDenied(message) => Err(message),
        BashAssessment::NeedsApproval { .. } => match &ctx.approver {
            Some(approver) => approver.approve(command).await,
            None => Err(
                "Command is not in allowlist, and no interactive approval channel is available for current session (please add this command to Agent command permissions, or adjust sandbox settings)"
                    .into(),
            ),
        },
    }
}
