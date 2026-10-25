//! require 模式的黑盒回归：helper 缺失时必须拒绝执行，而不是回退到无沙箱。
//!
//! 单独一个测试二进制：环境变量改写不受其它用例并发影响。

use pipi_protocol::permissions::{SandboxMode, SandboxPolicy};
use pipi_sandbox::Enforcement;

#[test]
fn require_without_helper_fails_closed() {
    let _guard = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    pipi_test_support::set_env_var("PIPI_SANDBOX", "require");
    pipi_test_support::set_env_var("PIPI_SANDBOX_BIN", "/nonexistent/pipi-sandbox");

    let policy =
        SandboxPolicy::for_workspace(SandboxMode::WorkspaceWrite, std::path::Path::new("/tmp"));
    let error = pipi_sandbox::enforcement(&policy).unwrap_err();
    assert!(error.contains("helper"), "{error}");
    assert!(pipi_sandbox::tokio_command(&policy, "true", std::iter::empty::<&str>()).is_err());
    assert_eq!(
        pipi_sandbox::enforcement(&SandboxPolicy::for_workspace(
            SandboxMode::DangerFullAccess,
            std::path::Path::new("/tmp"),
        ))
        .unwrap(),
        Enforcement::Legacy,
        "danger-full-access 是显式逃生门，不要求 helper"
    );

    pipi_test_support::remove_env_var("PIPI_SANDBOX");
    pipi_test_support::remove_env_var("PIPI_SANDBOX_BIN");
}
