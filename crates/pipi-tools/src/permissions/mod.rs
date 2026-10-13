//! bash 命令权限与沙箱策略（`pipi-tools` 的安全边界）。
//!
//! 权限是 Pipi 新增的（pi 靠扩展/审批机制，codex 靠 OS 级沙箱）。Pipi 的
//! 分层设计，按顺序评估，任何一层拒绝即拒绝（宁可拒绝不可放行）：
//!
//! 1. **切分**：引号感知的词法扫描 + shlex 分词，复合命令按 `&&` `||` `;`
//!    `|` 换行 `&` 拆段；引号内的连接符不拆段；无法安全解析 → 拒绝。
//! 2. **白/黑名单**：`denylist` 命中即拒；`allowlist` 必须逐段命中。
//!    Allowlist 未命中的**非危险**段落是唯一可交互审批的类型（审批通道由
//!    宿主实现 `CommandApprover`）：用户拒绝 / 超时 / 中止 / 通道缺失一律拒绝。
//! 3. **危险命令**（移植自 codex，见 [`safety`]）：`rm -f` 家族、
//!    sudo/env/trap/bash-c 包装器 —— 非 `danger-full-access` 下拒绝。
//!    `rm -f` 家族的唯一例外是「只作用于工作区内的字面量路径、且不是 git
//!    仓库根」的直接调用（见 `forced_rm_inside_workspace`），用于清理构建
//!    与缓存产物；包装调用、变量 / 通配符目标一律落回拒绝。
//! 4. **沙箱**：`workspace-write` 下拒绝写入文件系统的重定向（`>` `>>`
//!    `2>文件` `&>` `>|` `<>` `>&词`）与从文件读入的重定向（`<文件` `<(cmd)`）；
//!    文件写入请使用具备 canonical containment 的 `write` / `edit` 工具。
//!    丢弃 / 合并输出（`2>/dev/null`、`2>&1`、`>&-`）与 heredoc / herestring
//!    既不落盘也不读文件，一律放行 —— 见 `check_write_redirect`。
//!
//! 局限（诚实声明）：这是用户态的粗粒度闸门，拦不住所有逃逸路径（如
//! `tee`、`python -c "open(p,'w')"`、工具进程自身写文件）。bash 在
//! `workspace-write` 下拒绝写入重定向，是因为仅做执行前路径检查无法消除
//! 「命令先创建 symlink、再重定向写入」的 TOCTOU 窗口；codex 的做法是
//! OS 级沙箱（Landlock/Seatbelt），那是 Pipi 的后续工作。**因此这条规则的
//! 价值是「工作区写入必须走 write / edit 的 canonical containment」，不是
//! 防逃逸**——把不含写入的构造一并拦掉只会白付摩擦成本（实测：一个真实
//! 会话里 91 次拒绝中只有 1 次是真实文件写入）。
//!
//! 权限**数据**（`PermissionsConfig` / `SandboxMode` / 工具名清单）与沙箱
//! 模式的 kebab-case 取值在 `pipi_protocol::permissions`（清单与引擎都要读）；
//! 本模块只放策略实现。

mod safety;

mod config;
mod parse;
mod redirect;
mod rm;

pub use config::BashPolicy;
pub use parse::{matches_entry, split_segments};
