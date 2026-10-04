//! 权限配置与 bash 命令总闸：切分 → 白/黑名单 → 危险命令 → 沙箱策略。

use serde::{Deserialize, Serialize};

use super::parse::{matches_entry, split_segments};
use super::redirect::{RedirectCheck, check_write_redirect};
use super::rm::forced_rm_inside_workspace;
use super::safety::{DangerousCommandMatch, dangerous_command_match};
use super::types::{BashAssessment, BashMode, BashPermissions, DEFAULT_TOOLS, SandboxMode};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsConfig {
    /// 启用的工具名列表（`KNOWN_TOOLS` 的子集）。
    #[serde(default = "default_tools")]
    pub tools: Vec<String>,
    #[serde(default)]
    pub bash: BashPermissions,
    /// 沙箱策略（codex 的 SandboxMode）。缺省 DangerFullAccess 以兼容旧文件。
    #[serde(default)]
    pub sandbox: SandboxMode,
}

pub(super) fn default_tools() -> Vec<String> {
    DEFAULT_TOOLS.iter().map(|s| s.to_string()).collect()
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        PermissionsConfig {
            tools: default_tools(),
            bash: BashPermissions::default(),
            sandbox: SandboxMode::default(),
        }
    }
}

impl PermissionsConfig {
    pub fn tool_enabled(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t == name)
    }

    /// bash 命令总闸：切分 → 白/黑名单 → 危险命令 → 沙箱策略。
    pub fn assess_bash(&self, command: &str, _workspace: &std::path::Path) -> Result<(), String> {
        match self.assess_bash_classified(command, _workspace) {
            BashAssessment::Allowed => Ok(()),
            BashAssessment::HardDenied(message) => Err(message),
            BashAssessment::NeedsApproval { missing } => Err(format!(
                "命令「{}」不在白名单中（允许：{}）",
                missing.join(" && "),
                self.bash.commands.join(", ")
            )),
        }
    }

    /// bash 命令分类评估。只有「Allowlist 模式下白名单未命中、且不命中危险
    /// 规则」的段落可以交给交互审批救回；黑名单命中、危险命令、沙箱约束
    /// 一律硬拒 —— 宁可拒绝不可放行。
    pub fn assess_bash_classified(
        &self,
        command: &str,
        _workspace: &std::path::Path,
    ) -> BashAssessment {
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
                    && forced_rm_inside_workspace(&seg.argv, _workspace, command);
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
