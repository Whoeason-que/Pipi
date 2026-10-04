use super::*;
use pipi_protocol::BackgroundTaskStatus;

#[test]
fn rejects_shell_detachment_but_allows_quoted_ampersand_and_and() {
    assert!(validate_managed_command("cargo test && echo ok").is_ok());
    assert!(validate_managed_command("echo '&'").is_ok());
    assert!(validate_managed_command("sleep 10 &").is_err());
    assert!(validate_managed_command("nohup cargo test").is_err());
}

#[test]
fn parses_limits() {
    let args = json!({"waitMs": 12, "limit": 3});
    assert_eq!(parse_u64(&args, "waitMs").unwrap(), Some(12));
    assert_eq!(parse_usize(&args, "limit").unwrap(), Some(3));
}

#[test]
fn status_active_only_covers_live_states() {
    assert!(BackgroundTaskStatus::Queued.is_active());
    assert!(BackgroundTaskStatus::Running.is_active());
    assert!(!BackgroundTaskStatus::Completed.is_active());
    assert!(!BackgroundTaskStatus::Orphaned.is_active());
}
