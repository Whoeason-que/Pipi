//! 沙箱生效时，旧的命令文本策略必须让位给内核边界。
//!
//! 这个测试只验证「门」：用一个存在的占位 helper 让 `will_enforce` 为真，
//! 断言黑名单里的 `rm -rf /` 不再被用户态策略拒绝。真实的内核行为由
//! `pipi-sandbox` 的黑盒测试覆盖。

use std::collections::BTreeMap;
use std::sync::Arc;

use pipi_protocol::AbortSignal;
use pipi_protocol::permissions::{BashMode, BashPermissions, PermissionsConfig, SandboxMode};
use pipi_protocol::tools::ToolContext;
use pipi_tools::permissions::check_bash_command;

#[test]
fn sandbox_enforcement_bypasses_command_text_policy() {
    let _guard = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = pipi_test_support::temp_dir("pipi-sandbox-gate");
    let helper = root.join("pipi-sandbox");
    std::fs::write(&helper, "").unwrap();
    pipi_test_support::set_env_var("PIPI_SANDBOX", "require");
    pipi_test_support::set_env_var("PIPI_SANDBOX_BIN", &helper);

    let ctx = ToolContext {
        workspace: root.clone(),
        memory_dir: None,
        read_roots: Vec::new(),
        permissions: Arc::new(PermissionsConfig {
            tools: vec!["bash".into()],
            bash: BashPermissions {
                mode: BashMode::Denylist,
                commands: vec!["rm".into()],
            },
            sandbox: SandboxMode::WorkspaceWrite,
        }),
        sandbox: SandboxMode::WorkspaceWrite,
        resolved_env: Arc::new(BTreeMap::new()),
        abort: AbortSignal::new(),
        approver: None,
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        // 旧策略会 HardDenied；沙箱生效时这里放行，交给内核。
        check_bash_command(&ctx, "rm -rf /")
            .await
            .expect("kernel sandbox is the boundary");

        // danger-full-access 不使用 helper，回退到旧策略（黑名单仍然生效）。
        let mut full = ctx.clone();
        full.sandbox = SandboxMode::DangerFullAccess;
        assert!(check_bash_command(&full, "rm -rf /").await.is_err());
    });

    pipi_test_support::remove_env_var("PIPI_SANDBOX");
    pipi_test_support::remove_env_var("PIPI_SANDBOX_BIN");
    let _ = std::fs::remove_dir_all(root);
}
