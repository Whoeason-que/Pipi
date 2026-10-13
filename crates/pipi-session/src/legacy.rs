//! 旧 JSONL 会话的显式导入（逃生通道，见 `docs/git-session-store.md`）。
//!
//! 单轨化后 git 分支是唯一会话载体，运行时不再直接读 `sessions/*.jsonl`；
//! 未迁移的机器先跑一次导入，把旧会话迁进 git 分支。宿主命令 `import_sessions`
//! （Agent 工作台「文件」页）与无壳工具
//! `cargo run -p pipi-session --example import_legacy_sessions` 走同一份实现：
//! 逐条校验（读回与原文一致才算成功）、幂等（已在 git 里且一致则跳过）、原文件
//! 改名 `.jsonl.imported` 保留 —— 不改内容、不做隐式迁移。

use std::path::{Path, PathBuf};

use pipi_protocol::session::{SessionImportFailure, SessionImportReport};

use crate::git::GitSessionRepo;
use crate::load_session;

/// 把 `sessions/`（含 `.archive/`）里的旧会话导入 git 分支。
///
/// `archived` 来源要落在 `archive/<id>`，不把归档会话搬回活跃区。
pub fn import_legacy_sessions(git: &GitSessionRepo, legacy_dir: &Path) -> SessionImportReport {
    let mut report = SessionImportReport::default();
    for (id, path, archived) in legacy_files(legacy_dir) {
        let entries = match load_session(&path) {
            Ok(entries) => entries,
            Err(error) => {
                report.failed.push(SessionImportFailure {
                    session_id: id,
                    reason: error,
                });
                continue;
            }
        };
        match git.import(&id, &entries, archived) {
            Ok(true) => {
                if let Err(error) = mark_imported(&path) {
                    report.failed.push(SessionImportFailure {
                        session_id: id,
                        reason: error,
                    });
                    continue;
                }
                report.imported.push(id);
            }
            Ok(false) => report.skipped.push(id),
            Err(error) => report.failed.push(SessionImportFailure {
                session_id: id,
                reason: error,
            }),
        }
    }
    report
}

/// 枚举旧载体里的会话文件（活跃区 + 归档区）。
fn legacy_files(sessions_dir: &Path) -> Vec<(String, PathBuf, bool)> {
    let mut files = Vec::new();
    // `archived` 标记来源：导入归档区的会话要落在 archive/<id>，不能搬回活跃区
    for (dir, archived) in [
        (sessions_dir.to_path_buf(), false),
        (sessions_dir.join(".archive"), true),
    ] {
        if !matches!(
            pipi_protocol::fs::ensure_real_directory(&dir, "旧会话目录"),
            Ok(true)
        ) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            files.push((id.to_string(), path, archived));
        }
    }
    files.sort();
    files
}

/// 导入成功后把原文件改名 `.jsonl.imported`（字节保留，退出会话列表）。
fn mark_imported(path: &Path) -> Result<(), String> {
    let target = path.with_file_name(format!(
        "{}.imported",
        path.file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "无法解析旧会话文件名".to_string())?
    ));
    std::fs::rename(path, &target).map_err(|error| format!("改名旧会话文件失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::Message;
    use pipi_protocol::session::SessionRepo;

    /// 直接写一条旧格式的 JSONL 会话（导入读的就是这一行 JSON）。
    fn seed_legacy(dir: &Path, id: &str, first_message: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let message = Message::user_text(first_message);
        let entry = pipi_protocol::session::SessionEntry {
            id: pipi_protocol::new_id(),
            parent_id: None,
            seq: 0,
            timestamp: message.timestamp(),
            kind: pipi_protocol::session::EntryKind::Message { message },
        };
        let path = dir.join(format!("{id}.jsonl"));
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&entry).unwrap()),
        )
        .unwrap();
        path
    }

    #[test]
    fn legacy_sessions_import_once_and_keep_their_region() {
        let agent_dir = pipi_test_support::temp_dir("pipi-legacy-import");
        let sessions = agent_dir.join("sessions");
        let active_id = format!(
            "{}-{}",
            pipi_protocol::now_millis(),
            pipi_protocol::new_id()
        );
        let archived_id = format!(
            "{}-{}",
            pipi_protocol::now_millis(),
            pipi_protocol::new_id()
        );
        seed_legacy(&sessions, &active_id, "活跃的旧会话");
        let archived = sessions.join(".archive");
        seed_legacy(&archived, &archived_id, "归档的旧会话");

        let git = GitSessionRepo::new(agent_dir.join("sessions.git"));
        let report = import_legacy_sessions(&git, &sessions);
        let mut imported = report.imported.clone();
        imported.sort();
        let mut expected = vec![active_id.clone(), archived_id.clone()];
        expected.sort();
        assert_eq!(imported, expected, "{report:?}");
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        // 活跃区与归档区各归各位
        assert!(git.exists(&active_id).unwrap());
        assert!(!git.exists(&archived_id).unwrap());
        assert!(
            git.list_archived()
                .unwrap()
                .iter()
                .any(|summary| summary.id == archived_id)
        );
        assert_eq!(git.entries(&active_id).unwrap().len(), 1);
        // 原文件改名保留，不再被枚举
        assert!(!sessions.join(format!("{active_id}.jsonl")).exists());
        assert!(
            sessions
                .join(format!("{active_id}.jsonl.imported"))
                .is_file()
        );
        // 幂等：再跑一次不重复导入
        let again = import_legacy_sessions(&git, &sessions);
        assert!(again.imported.is_empty(), "{again:?}");
        assert!(again.skipped.is_empty(), "{again:?}");
        assert!(again.failed.is_empty(), "{again:?}");
        std::fs::remove_dir_all(&agent_dir).ok();
    }

    #[test]
    fn ids_already_in_git_with_different_content_fail_instead_of_overwriting() {
        let agent_dir = pipi_test_support::temp_dir("pipi-legacy-conflict");
        let sessions = agent_dir.join("sessions");
        let git = GitSessionRepo::new(agent_dir.join("sessions.git"));

        // 先在 git 里造一条会话，再把一个**同 id 但内容不同**的旧文件放回磁盘
        let mut sink = git.create().unwrap();
        let created_id = sink.session_id().to_string();
        sink.append_message(&Message::user_text("git 里的版本"))
            .unwrap();
        sink.commit_point().unwrap();
        drop(sink);
        seed_legacy(&sessions, &created_id, "旧文件里的另一个版本");

        let report = import_legacy_sessions(&git, &sessions);
        assert!(report.imported.is_empty(), "{report:?}");
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert!(report.failed[0].reason.contains("内容不同"), "{report:?}");
        // git 里的原内容未被改动
        assert_eq!(git.entries(&created_id).unwrap().len(), 1);
        std::fs::remove_dir_all(&agent_dir).ok();
    }
}
