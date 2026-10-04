use super::*;
use crate::test_support::{make_repo, noop_log, write_skill};
use std::fs;

/// 安装类测试会改 `AV_HOME`（进程级环境变量），串行执行避免互相踩踏。
/// 也是下面 `unsafe` 的依据：edition 2024 起 `std::env::set_var` 是 unsafe
/// （环境块不是线程安全的），持锁后改动互斥。
static AV_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct AvHomeGuard {
    previous: Option<String>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl AvHomeGuard {
    fn set(dir: &Path) -> Self {
        let lock = AV_HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var("AV_HOME").ok();
        // SAFETY: 持 AV_HOME_LOCK，同一测试二进制内的 AV_HOME 改动互斥
        unsafe { std::env::set_var("AV_HOME", dir) };
        Self {
            previous,
            _lock: lock,
        }
    }
}

impl Drop for AvHomeGuard {
    fn drop(&mut self) {
        // SAFETY: `_lock` 在 drop 体之后才释放，此处仍持 AV_HOME_LOCK
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var("AV_HOME", value),
                None => std::env::remove_var("AV_HOME"),
            }
        }
    }
}

fn setup_av_home(label: &str) -> (PathBuf, AvHomeGuard) {
    let home = pipi_test_support::temp_dir(&format!("av-install-{label}"));
    let guard = AvHomeGuard::set(&home);
    (home, guard)
}

#[test]
fn install_local_source_selects_and_records() {
    let (home, _guard) = setup_av_home("local-install");
    let source = home.join("source");
    write_skill(&source, "pdf", "body");
    write_skill(&source, "writer", "body");

    let spec = SourceSpec::Local {
        path: fs::canonicalize(&source).unwrap(),
    };
    let report = install_source(&spec, &Selection::All, false, &mut noop_log()).unwrap();
    assert_eq!(report.installed.len(), 2);
    assert!(report.installed.iter().all(|skill| !skill.unchanged));
    let index = store::read_index().unwrap();
    assert_eq!(index.skills.len(), 2);
    assert!(
        index
            .skills
            .iter()
            .all(|entry| entry.installed_at.is_some())
    );
    for skill in &report.installed {
        assert!(skill.dir.join("SKILL.md").is_file());
        assert_eq!(
            entry_for(&index, &skill.entry.name).content_hash,
            skill.entry.content_hash
        );
    }

    // 幂等：同内容重装 → unchanged，installed-at 保留
    let installed_at = entry_for(&index, "pdf").installed_at.clone();
    let report = install_source(&spec, &Selection::All, false, &mut noop_log()).unwrap();
    assert!(report.installed.iter().all(|skill| skill.unchanged));
    let index = store::read_index().unwrap();
    assert_eq!(entry_for(&index, "pdf").installed_at, installed_at);

    // 显式选择
    let report = install_source(
        &spec,
        &Selection::Only(vec!["pdf".into()]),
        false,
        &mut noop_log(),
    )
    .unwrap();
    assert_eq!(report.installed.len(), 1);
    let error = install_source(
        &spec,
        &Selection::Only(vec!["missing".into()]),
        false,
        &mut noop_log(),
    )
    .unwrap_err();
    assert!(error.contains("可用") && error.contains("pdf"), "{error}");

    // dry-run 不落盘、不改索引
    let before = fs::read_dir(home.join("skills").join("pdf"))
        .unwrap()
        .count();
    let report = install_source(&spec, &Selection::All, true, &mut noop_log()).unwrap();
    assert_eq!(report.installed.len(), 2);
    let after = fs::read_dir(home.join("skills").join("pdf"))
        .unwrap()
        .count();
    assert_eq!(before, after);

    let _ = fs::remove_dir_all(home);
}

fn entry_for<'a>(index: &'a store::SkillLock, name: &str) -> &'a SkillLockEntry {
    index.get(name).unwrap()
}

fn repo_commit(repo: &Path, message: &str) {
    let run = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
    };
    run(&["add", "-A"]);
    run(&["commit", "--quiet", "-m", message]);
}

#[test]
fn install_git_source_materialize_and_refresh() {
    let (home, _guard) = setup_av_home("git-install");
    let (repo, _head) = make_repo("git-repo");
    let spec = SourceSpec::Git {
        url: repo.to_string_lossy().into_owned(),
        reference: Some("HEAD".into()),
        skill_path: None,
    };

    let report = install_source(&spec, &Selection::All, false, &mut noop_log()).unwrap();
    assert_eq!(report.installed.len(), 1);
    let entry = report.installed[0].entry.clone();
    assert_eq!(entry.source_type, SourceType::Git);
    assert_eq!(entry.revision.as_deref().map(str::len), Some(40));
    assert_eq!(entry.skill_path.as_deref(), Some("skills/pdf"));

    // 从锁记录可复现（删掉 store 内容后 materialize）
    store::remove_version(&entry.name, &entry.content_hash).unwrap();
    let dir = materialize_entry(&entry, &mut noop_log()).unwrap();
    assert!(dir.join("SKILL.md").is_file());

    // 来源新增提交：锁记录仍复现旧内容（修订被钉住）
    write_skill(&repo.join("skills"), "pdf", "v2");
    repo_commit(&repo, "v2");
    let dir_again = materialize_entry(&entry, &mut noop_log()).unwrap();
    assert_eq!(dir_again, dir, "materialize 只认锁住的修订");
    assert_eq!(
        store::hash_skill_dir(&dir_again).unwrap(),
        entry.content_hash
    );

    // update：解析到新修订与新内容
    let updated = refresh_entry(&entry, &mut noop_log()).unwrap().unwrap();
    assert_ne!(updated.revision, entry.revision);
    assert_ne!(updated.content_hash, entry.content_hash);
    assert!(updated.updated_at.is_some());
    let index = store::read_index().unwrap();
    assert!(
        index
            .get_version(&updated.name, &updated.content_hash)
            .is_some()
    );

    // 再 refresh：已是最新
    assert!(refresh_entry(&updated, &mut noop_log()).unwrap().is_none());

    // 篡改锁哈希 → materialize 拒绝（fail-closed）
    let mut tampered = updated.clone();
    tampered.content_hash = format!("sha256:{}", "0".repeat(64));
    let error = materialize_entry(&tampered, &mut noop_log()).unwrap_err();
    assert!(error.contains("拒绝安装"), "{error}");

    let _ = fs::remove_dir_all(home);
    let _ = fs::remove_dir_all(repo);
}

#[test]
fn tag_reference_pins_revision() {
    let (home, _guard) = setup_av_home("tag-pin");
    let (repo, _head) = make_repo("tag-repo");
    let spec = SourceSpec::Git {
        url: repo.to_string_lossy().into_owned(),
        reference: Some("v1".into()),
        skill_path: None,
    };
    let report = install_source(&spec, &Selection::All, false, &mut noop_log()).unwrap();
    let entry = report.installed[0].entry.clone();

    // 新提交不影响 tag 解析：update 判定为「已是最新」
    write_skill(&repo.join("skills"), "pdf", "v2");
    repo_commit(&repo, "v2");
    assert!(refresh_entry(&entry, &mut noop_log()).unwrap().is_none());

    let _ = fs::remove_dir_all(home);
    let _ = fs::remove_dir_all(repo);
}

#[test]
fn drifted_store_content_is_detected_and_repairable() {
    let (home, _guard) = setup_av_home("drift");
    let source = home.join("source");
    write_skill(&source, "pdf", "body");
    let spec = SourceSpec::Local {
        path: fs::canonicalize(&source).unwrap(),
    };
    let report = install_source(&spec, &Selection::All, false, &mut noop_log()).unwrap();
    let entry = report.installed[0].entry.clone();

    assert_eq!(store::verify_entry(&entry).unwrap(), VerifyOutcome::Ok);
    fs::write(report.installed[0].dir.join("SKILL.md"), "tampered").unwrap();
    assert!(matches!(
        store::verify_entry(&entry).unwrap(),
        VerifyOutcome::Mismatch { .. }
    ));
    remove_drifted(&entry).unwrap();
    assert_eq!(store::verify_entry(&entry).unwrap(), VerifyOutcome::Missing);
    // 修复后可重新物化
    materialize_entry(&entry, &mut noop_log()).unwrap();
    assert_eq!(store::verify_entry(&entry).unwrap(), VerifyOutcome::Ok);

    let _ = fs::remove_dir_all(home);
}
