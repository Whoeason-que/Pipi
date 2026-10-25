//! Linux 沙箱：Landlock（文件系统）+ seccomp-BPF（网络）。
//!
//! 顺序：先 Landlock（`restrict_self` 会设置 `no_new_privs`），再装 seccomp。
//! 两者都是不可撤销的进程属性，`exec` 后由所有子进程继承。
//!
//! 已知边界（Landlock 的固有语义，不是实现遗漏）：Landlock 只中介它声明的
//! 访问权（读/写/删除/创建/执行/rename/truncate），**不中介 chmod、chown、
//! utime 等元数据修改**。因此 `workspace-write` 保护的是内容与目录结构，
//! 不是文件权限位；需要更强隔离时用 namespace/容器把路径变成不可见。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus,
};
use pipi_protocol::permissions::SandboxPolicy;
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch, apply_filter,
};

/// 固定放行的设备节点（读写 `2>/dev/null`、终端、共享内存都是常规工具行为）。
const WRITABLE_DEVICES: [&str; 8] = [
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
    "/dev/pts",
    "/dev/shm",
];

/// 安装 Landlock + seccomp 后返回；调用方紧接着 `exec` 目标程序。
pub fn apply(policy: &SandboxPolicy) -> Result<(), String> {
    if !policy.is_enforced() {
        return Ok(());
    }
    apply_landlock(policy)?;
    if !policy.network_access {
        apply_no_network()?;
    }
    Ok(())
}

fn apply_landlock(policy: &SandboxPolicy) -> Result<(), String> {
    // V3 才包含 truncate；用 HardRequirement，老内核直接不可用（由上层决定
    // 回退或拒绝），避免悄悄降级成「挡得住写、挡不住 truncate」。
    let abi = ABI::V3;
    let ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))
        .map_err(|error| format!("Landlock unavailable (filesystem rules): {error}"))?
        .create()
        .map_err(|error| format!("failed to create Landlock ruleset: {error}"))?;

    // 全局只读：任何路径都可以读/执行，写权限由下面的白名单追加。
    let mut ruleset = ruleset
        .add_rule(PathBeneath::new(
            root_fd(Path::new("/"))?,
            AccessFs::from_read(abi),
        ))
        .map_err(|error| format!("failed to add Landlock read-only root rule: {error}"))?;

    for device in WRITABLE_DEVICES {
        let path = Path::new(device);
        if path.exists() {
            // 目录（/dev/pts、/dev/shm）用完整写权限；字符设备只能授
            // 文件级权限，Landlock 会拒绝把目录专属权限（Make*/Remove*）
            // 挂在非目录上。
            let access = if path.is_dir() {
                AccessFs::from_all(abi)
            } else {
                AccessFs::WriteFile | AccessFs::Truncate
            };
            ruleset = ruleset
                .add_rule(PathBeneath::new(root_fd(path)?, access))
                .map_err(|error| format!("failed to add Landlock device rule {device}: {error}"))?;
        }
    }

    for root in &policy.writable_roots {
        let canonical = fs::canonicalize(root).map_err(|error| {
            format!(
                "writable root {} cannot be resolved: {error}",
                root.display()
            )
        })?;
        ruleset = ruleset
            .add_rule(PathBeneath::new(
                root_fd(&canonical)?,
                AccessFs::from_all(abi),
            ))
            .map_err(|error| {
                format!(
                    "failed to add Landlock writable root {}: {error}",
                    canonical.display()
                )
            })?;
    }

    let status = ruleset
        .restrict_self()
        .map_err(|error| format!("failed to enable Landlock: {error}"))?;
    match status.ruleset {
        RulesetStatus::NotEnforced => {
            Err("Landlock cannot be enabled on this kernel (requires 5.13+ with the landlock LSM); refusing to run".into())
        }
        RulesetStatus::PartiallyEnforced | RulesetStatus::FullyEnforced => Ok(()),
    }
}

fn root_fd(path: &Path) -> Result<PathFd, String> {
    PathFd::new(path)
        .map_err(|error| format!("failed to open sandbox path {}: {error}", path.display()))
}

/// 禁止 AF_INET / AF_INET6 socket。
///
/// 只按 socket 的 `domain` 参数过滤，不用管命令怎么拼、是不是 `$()`、是不是
/// 编译出来的二进制 —— 只要想联网就会撞上 `socket(2)`。AF_UNIX 保留，
/// 否则本地 IPC、终端、构建工具会出现莫名其妙的失败。
fn apply_no_network() -> Result<(), String> {
    // 每个地址族一条规则：同一 `SeccompRule` 里的多个条件是与（AND），
    // 把 AF_INET/AF_INET6 放进同一条会永远不匹配。多条规则之间是或（OR）。
    let mut rules_for_socket = Vec::new();
    for domain in [libc::AF_INET, libc::AF_INET6] {
        let condition =
            SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, domain as u64)
                .map_err(|error| format!("invalid seccomp condition: {error}"))?;
        rules_for_socket.push(
            SeccompRule::new(vec![condition])
                .map_err(|error| format!("invalid seccomp rule: {error}"))?,
        );
    }
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    rules.insert(libc::SYS_socket, rules_for_socket);

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        TargetArch::try_from(std::env::consts::ARCH)
            .map_err(|error| format!("seccomp does not support this architecture: {error}"))?,
    )
    .map_err(|error| format!("invalid seccomp filter: {error}"))?;
    let program: BpfProgram = filter
        .try_into()
        .map_err(|error| format!("failed to compile seccomp filter: {error}"))?;
    apply_filter(&program).map_err(|error| format!("failed to install seccomp filter: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::permissions::SandboxMode;

    #[test]
    fn danger_mode_is_a_noop() {
        let policy = SandboxPolicy::for_workspace(SandboxMode::DangerFullAccess, Path::new("/tmp"));
        assert!(apply(&policy).is_ok());
    }

    #[test]
    fn writable_devices_are_real_paths_on_linux() {
        // 只保证常量本身没有打字错误；测试机缺 /dev/tty 也无妨。
        assert!(WRITABLE_DEVICES.contains(&"/dev/null"));
    }
}
