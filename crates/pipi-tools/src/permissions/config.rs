//! bash 命令总闸（实现侧）：切分 → 白/黑名单 → 危险命令 → 沙箱策略。
//!
//! `PermissionsConfig` 本体（数据）在 `pipi_protocol::permissions` —— 清单与
//! 引擎都要读它。这里用扩展 trait 给它挂上评估逻辑：数据属契约，策略属实现。

use std::path::Path;

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
                "沙箱策略为 read-only：不允许执行命令（需要执行请调整 Agent 的沙箱设置）".into(),
            );
        }
        if self.sandbox == SandboxMode::WorkspaceWrite {
            match check_write_redirect(command) {
                RedirectCheck::Ok => {}
                RedirectCheck::Write(token) => {
                    return BashAssessment::HardDenied(format!(
                        "沙箱策略为 workspace-write：拒绝写入重定向「{token}」；工作区内写入请用 write / edit 工具。\
                         丢弃或合并输出可以用 2>/dev/null、2>&1，管道与 heredoc 不受限制"
                    ));
                }
                RedirectCheck::Read(token) => {
                    return BashAssessment::HardDenied(format!(
                        "沙箱策略为 workspace-write：拒绝输入重定向「{token}」；读文件请用 read 工具\
                         （带工作区包含检查），把脚本喂给 stdin 可以用 heredoc（<<EOF）"
                    ));
                }
                RedirectCheck::Unparseable(reason) => {
                    return BashAssessment::HardDenied(format!(
                        "沙箱策略为 workspace-write：{reason}，无法静态分析命令中的重定向，已拒绝"
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
                                "「{}」命中危险命令规则，在 {} 沙箱下被拒绝（不可审批）",
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
                            "命令「{}」被黑名单禁止",
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
                            "「{}」包含强制删除（rm -f 家族），在 {} 沙箱下被拒绝（仅放行工作区内、\
                                 非仓库根的绝对路径字面量；含变量 / 通配符的目标，以及命令内有 cd / pushd \
                                 时的相对路径都不可审批）",
                            seg.text,
                            self.sandbox.as_str()
                        ),
                        DangerousCommandMatch::Other => format!(
                            "「{}」命中危险命令规则，在 {} 沙箱下被拒绝",
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

/// 统一检查待执行的 bash 命令权限与审批通道。
///
/// 命令权限检查（切分 → 白/黑名单 → 危险命令 → 沙箱重定向）。
/// 只有 Allowlist 未命中的非危险命令可以走交互审批救回；其余硬拒。
pub async fn check_bash_command(
    ctx: &pipi_protocol::tools::ToolContext,
    command: &str,
) -> Result<(), String> {
    match ctx.permissions.assess_bash_classified(command, &ctx.workspace) {
        BashAssessment::Allowed => Ok(()),
        BashAssessment::HardDenied(message) => Err(message),
        BashAssessment::NeedsApproval { .. } => match &ctx.approver {
            Some(approver) => approver.approve(command).await,
            None => Err(
                "命令不在白名单中，且当前会话没有可用的交互审批通道（请在 Agent 的命令权限中加入该命令，或调整沙箱设置）"
                    .into(),
            ),
        },
    }
}
