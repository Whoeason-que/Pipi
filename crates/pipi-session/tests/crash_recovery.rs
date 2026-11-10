use pipi_protocol::Message;
use pipi_protocol::session::SessionRepo;
use pipi_session::GitSessionRepo;
use std::time::{Duration, Instant};

#[test]
fn crash_child() {
    let Some(path) = std::env::var_os("PIPI_TEST_CRASH_REPOSITORY") else {
        return;
    };
    let id = std::env::var("PIPI_TEST_CRASH_SESSION").unwrap();
    let repo = GitSessionRepo::new(path.into());
    let mut log = repo.open(&id).unwrap();
    log.append_message(&Message::user_text("recover this message"))
        .unwrap();
    log.checkpoint_partial(&Message::assistant_text("partial response", "mock"))
        .unwrap();
    std::fs::write(std::env::var_os("PIPI_TEST_CRASH_READY").unwrap(), b"ready").unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn killed_process_recovers_entries_and_stream_snapshot() {
    let dir = pipi_test_support::temp_dir("pipi-crash-process");
    let path = dir.join("sessions.git");
    let repo = GitSessionRepo::new(path.clone());
    let mut log = repo.create().unwrap();
    let id = log.session_id().to_string();
    log.commit_point().unwrap();
    drop(log);
    let ready = dir.join("ready");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_child", "--nocapture"])
        .env("PIPI_TEST_CRASH_REPOSITORY", &path)
        .env("PIPI_TEST_CRASH_SESSION", &id)
        .env("PIPI_TEST_CRASH_READY", &ready)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() && Instant::now() < deadline {
        assert!(
            child.try_wait().unwrap().is_none(),
            "child exited before writing recovery"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.exists(), "child did not finish writing recovery");
    child.kill().unwrap();
    child.wait().unwrap();
    let (mut log, entries) = repo.open_with_entries(&id).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(log.has_recovery());
    assert!(
        serde_json::to_string(&log.recovered_partial().unwrap())
            .unwrap()
            .contains("partial response")
    );
    assert_eq!(repo.history(&id).unwrap().len(), 0);
    log.append_message(&Message::assistant_text("interrupted", "mock"))
        .unwrap();
    log.commit_point().unwrap();
    assert_eq!(repo.history(&id).unwrap().len(), 1);
    assert_eq!(repo.entries(&id).unwrap().len(), 2);
    assert!(!repo.open(&id).unwrap().has_recovery());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn cleanup_interrupted_after_commit_does_not_duplicate_entries() {
    let dir = pipi_test_support::temp_dir("pipi-crash-cleanup");
    let path = dir.join("sessions.git");
    let repo = GitSessionRepo::new(path.clone());
    let mut log = repo.create().unwrap();
    let id = log.session_id().to_string();
    log.commit_point().unwrap();
    log.append_message(&Message::user_text("once")).unwrap();
    let journal = path.join("pipi-recovery").join(format!("{id}.jsonl"));
    let saved = std::fs::read(&journal).unwrap();
    log.commit_point().unwrap();
    std::fs::write(&journal, saved).unwrap();
    let (mut restored, entries) = repo.open_with_entries(&id).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(!restored.has_recovery());
    restored.commit_point().unwrap();
    assert_eq!(repo.history(&id).unwrap().len(), 1);
    assert_eq!(repo.entries(&id).unwrap().len(), 1);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn conflicting_recovery_file_is_rejected_without_overwriting_history() {
    let dir = pipi_test_support::temp_dir("pipi-crash-conflict");
    let path = dir.join("sessions.git");
    let repo = GitSessionRepo::new(path.clone());
    let mut log = repo.create().unwrap();
    let id = log.session_id().to_string();
    log.commit_point().unwrap();
    log.append_message(&Message::user_text("original")).unwrap();
    let journal = path.join("pipi-recovery").join(format!("{id}.jsonl"));
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
    value["parentId"] = serde_json::json!("different-parent");
    std::fs::write(&journal, format!("{value}\n")).unwrap();
    assert!(repo.open(&id).is_err());
    assert_eq!(repo.history(&id).unwrap().len(), 0);
    assert!(journal.exists());
    std::fs::remove_dir_all(dir).ok();
}
