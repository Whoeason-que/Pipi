//! macOS Seatbelt：生成 SBPL profile，交给系统 `/usr/bin/sandbox-exec` 执行。
//!
//! 这里不引入私有 FFI：`sandbox-exec` 是 Apple 自带的内核强制入口（虽然被
//! 标注 deprecated，但仍是唯一可用的公开路径）。profile 与 codex 的
//! base policy 同源，只保留 Agent 工具真正需要的放行项。
//!
//! 注意：本文件在 Linux 上不参与编译；只有 macOS 发布构建会验证它。

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::Command;

use pipi_protocol::permissions::SandboxPolicy;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// 生成 SBPL profile。`deny default` + 显式放行。
pub fn profile(policy: &SandboxPolicy) -> String {
    let mut policy_text = String::from(
        "(version 1)\n\
         (deny default)\n\
         ; 子进程继承同一 sandbox\n\
         (allow process-exec)\n\
         (allow process-fork)\n\
         (allow signal (target same-sandbox))\n\
         (allow process-info* (target same-sandbox))\n\
         ; 工具链需要的基础查询；不涉及文件内容\n\
         (allow sysctl-read)\n\
         (allow mach-lookup)\n\
         (allow ipc-posix-sem)\n\
         ; 默认全盘可读（与 Linux 的 read rule 对齐）\n\
         (allow file-read*)\n\
         (allow file-write-data\n\
           (require-all (path \"/dev/null\") (vnode-type CHARACTER-DEVICE)))\n",
    );

    for root in &policy.writable_roots {
        policy_text.push_str(&format!(
            "(allow file-write* (subpath \"{}\"))\n",
            sbpl_escape(&root.to_string_lossy())
        ));
    }

    if policy.network_access {
        policy_text.push_str("(allow network*)\n");
    } else {
        policy_text.push_str("(deny network*)\n");
    }
    policy_text
}

/// 用 `sandbox-exec` 重新 exec 目标命令（helper 自身不再返回）。
pub fn exec(policy: &SandboxPolicy, program: &OsString, args: &[OsString]) -> Result<(), String> {
    if !policy.is_enforced() {
        let error = Command::new(program).args(args).exec();
        return Err(format!(
            "failed to exec {}: {error}",
            program.to_string_lossy()
        ));
    }
    let profile = profile(policy);
    if let Some(root) = policy
        .writable_roots
        .iter()
        .find(|root| root.to_string_lossy().chars().any(char::is_control))
    {
        return Err(format!(
            "sandbox writable root contains control characters: {:?}",
            root
        ));
    }
    let error = Command::new(SANDBOX_EXEC)
        .arg("-p")
        .arg(profile)
        .arg(program)
        .args(args)
        .exec();
    Err(format!("failed to start sandbox-exec: {error}"))
}

fn sbpl_escape(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::permissions::SandboxMode;
    use std::path::Path;

    #[test]
    fn read_only_denies_writes_and_network() {
        let policy = SandboxPolicy::for_workspace(SandboxMode::ReadOnly, Path::new("/tmp/ws"));
        let profile = profile(&policy);
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(deny network*)"));
        assert!(!profile.contains("(allow file-write* (subpath"));
    }

    #[test]
    fn workspace_write_adds_roots_and_escapes_quotes() {
        let mut policy =
            SandboxPolicy::for_workspace(SandboxMode::WorkspaceWrite, Path::new("/tmp/ws"));
        policy.writable_roots = vec![Path::new("/tmp/a\"b").to_path_buf()];
        policy.network_access = true;
        let profile = profile(&policy);
        assert!(profile.contains("(allow file-write* (subpath \"/tmp/a\\\"b\"))"));
        assert!(profile.contains("(allow network*)"));
    }
}
