//! 一个 Agent 的会话仓库：新建走 git，旧 JSONL 会话按 id 分发。
//!
//! 载体切换（2026-10-13）后新会话一律写 git 分支；`sessions/*.jsonl` 只作为
//! **旧会话的兼容路径**保留 —— 未导入的旧会话照旧可读、可继续对话（同一个
//! 会话始终只有一个载体，不会出现半 git 半 jsonl 的会话）。要迁到 git 就跑
//! 一次导入（[`AgentSessionRepo::import_legacy_sessions`]）：逐条校验、幂等、
//! 原文件改名 `.jsonl.imported` 保留。
//!
//! 宿主侧的装配点（`pipi-core::runtime::store`）只认这一个类型。

use std::path::{Path, PathBuf};

use pipi_protocol::session::{
    SessionEntry, SessionImportFailure, SessionImportReport, SessionLog, SessionRepo,
    SessionSummary,
};

use crate::git::GitSessionRepo;
use crate::{JsonlSessionRepo, load_session};

/// 一个 Agent 的会话仓库：git 分支 + 旧 JSONL 文件。
pub struct AgentSessionRepo {
    git: GitSessionRepo,
    jsonl: JsonlSessionRepo,
}

impl AgentSessionRepo {
    /// 以 Agent 数据目录构造（`~/.pipi/agents/<name>`）。
    pub fn new(agent_dir: impl Into<PathBuf>) -> Self {
        let agent_dir = agent_dir.into();
        Self {
            git: GitSessionRepo::new(agent_dir.join("sessions.git")),
            jsonl: JsonlSessionRepo::new(agent_dir.join("sessions")),
        }
    }

    /// git 侧仓库（导入、探测与测试用）。
    pub fn git(&self) -> &GitSessionRepo {
        &self.git
    }

    /// 把该 Agent 的旧 JSONL 会话导入 git。
    ///
    /// 逐条校验（读回与原文一致才算成功）、幂等（已在 git 里且一致则跳过）、
    /// 原文件改名 `.jsonl.imported` 保留 —— 不改内容、不做隐式迁移。
    pub fn import_legacy_sessions(&self) -> SessionImportReport {
        let mut report = SessionImportReport::default();
        for (id, path, archived) in self.legacy_files() {
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
            match self.git.import(&id, &entries, archived) {
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
    fn legacy_files(&self) -> Vec<(String, PathBuf, bool)> {
        let mut files = Vec::new();
        // `archived` 标记来源：导入归档区的会话要落在 archive/<id>，不能搬回活跃区
        for (dir, archived) in [
            (self.jsonl.dir().to_path_buf(), false),
            (self.jsonl.dir().join(".archive"), true),
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

    /// 按会话当前所在的载体分发一次操作（git 优先）。
    fn dispatch<T>(
        &self,
        session_id: &str,
        on_git: impl FnOnce(&GitSessionRepo) -> Result<T, String>,
        on_jsonl: impl FnOnce(&JsonlSessionRepo) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.git.exists(session_id)? {
            on_git(&self.git)
        } else {
            on_jsonl(&self.jsonl)
        }
    }

    /// 该会话是否由 git 载体持有（活跃区或归档区都算）。
    fn git_holds(&self, session_id: &str) -> Result<bool, String> {
        if self.git.exists(session_id)? {
            return Ok(true);
        }
        Ok(self
            .git
            .list_archived()?
            .iter()
            .any(|summary| summary.id == session_id))
    }
}

impl SessionRepo for AgentSessionRepo {
    fn create(&self) -> Result<Box<dyn SessionLog>, String> {
        self.git.create()
    }

    fn open(&self, session_id: &str) -> Result<Box<dyn SessionLog>, String> {
        self.dispatch(
            session_id,
            |git| git.open(session_id),
            |jsonl| jsonl.open(session_id),
        )
    }

    fn exists(&self, session_id: &str) -> Result<bool, String> {
        // git 命中就不再碰旧目录；否则由 JSONL 侧回答（含符号链接的 fail-closed）
        if self.git.exists(session_id)? {
            return Ok(true);
        }
        self.jsonl.exists(session_id)
    }

    fn entries(&self, session_id: &str) -> Result<Vec<SessionEntry>, String> {
        self.dispatch(
            session_id,
            |git| git.entries(session_id),
            |jsonl| jsonl.entries(session_id),
        )
    }

    fn list(&self) -> Vec<SessionSummary> {
        merge_summaries(self.git.list(), self.jsonl.list())
    }

    fn list_archived(&self) -> Result<Vec<SessionSummary>, String> {
        Ok(merge_summaries(
            self.git.list_archived()?,
            self.jsonl.list_archived()?,
        ))
    }

    fn fork(
        &self,
        session_id: &str,
        up_to_entry_id: Option<&str>,
    ) -> Result<Box<dyn SessionLog>, String> {
        // 分叉保持原载体：git 会话分叉出新分支，旧 JSONL 会话分叉出新文件
        self.dispatch(
            session_id,
            |git| git.fork(session_id, up_to_entry_id),
            |jsonl| jsonl.fork(session_id, up_to_entry_id),
        )
    }

    fn archive(&self, session_id: &str) -> Result<(), String> {
        self.dispatch(
            session_id,
            |git| git.archive(session_id),
            |jsonl| jsonl.archive(session_id),
        )
    }

    fn restore(&self, session_id: &str) -> Result<(), String> {
        // 归档区里的会话不在活跃区，由 git_holds 一并探测
        if self.git_holds(session_id)? {
            return self.git.restore(session_id);
        }
        self.jsonl.restore(session_id)
    }

    fn delete(&self, session_id: &str) -> Result<(), String> {
        self.dispatch(
            session_id,
            |git| git.delete(session_id),
            |jsonl| jsonl.delete(session_id),
        )
    }

    fn delete_archived(&self, session_id: &str) -> Result<(), String> {
        if self.git_holds(session_id)? {
            return self.git.delete_archived(session_id);
        }
        self.jsonl.delete_archived(session_id)
    }
}

/// 合并两个载体的摘要：按 id 去重（git 优先），按最后活跃倒序。
fn merge_summaries(git: Vec<SessionSummary>, jsonl: Vec<SessionSummary>) -> Vec<SessionSummary> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for summary in git.into_iter().chain(jsonl) {
        if seen.insert(summary.id.clone()) {
            out.push(summary);
        }
    }
    out.sort_by_key(|summary| std::cmp::Reverse(summary.last_active));
    out
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

    #[test]
    fn summaries_dedupe_with_git_winning() {
        let summary = |id: &str, last_active: u64| SessionSummary {
            id: id.to_string(),
            title: id.to_string(),
            message_count: 1,
            started_at: 0,
            last_active,
            model: None,
        };
        let merged = merge_summaries(
            vec![summary("a", 10)],
            vec![summary("a", 20), summary("b", 5)],
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "a"); // last_active 倒序：a(10) 在 b(5) 前
        assert_eq!(merged[0].last_active, 10); // git 侧胜出（jsonl 的 a 是 20）
        assert_eq!(merged[1].id, "b");
    }

    #[test]
    fn new_sessions_go_to_git_and_legacy_files_import_once() {
        use pipi_protocol::Message;
        let dir = pipi_test_support::temp_dir("pipi-agent-repo");
        // 造一条旧 JSONL 会话（活跃区）与一条归档会话
        let legacy = crate::SessionWriter::create(&dir.join("sessions")).unwrap();
        let mut legacy = legacy;
        legacy
            .append_message(&Message::user_text("旧会话"))
            .unwrap();
        let legacy_id = legacy.session_id().to_string();
        drop(legacy);

        let repo = AgentSessionRepo::new(&dir);
        // 新建走 git
        let created = repo.create().unwrap();
        assert!(!created.is_temporary());
        let created_id = created.session_id().to_string();
        assert!(created_id.contains('-'));
        // 未导入的旧会话仍可读
        assert!(repo.exists(&legacy_id).unwrap());
        assert_eq!(repo.entries(&legacy_id).unwrap().len(), 1);

        // 导入：原文件改名、会话改由 git 提供、再跑一次幂等
        let report = repo.import_legacy_sessions();
        assert_eq!(report.imported, vec![legacy_id.clone()]);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(
            !dir.join("sessions")
                .join(format!("{legacy_id}.jsonl"))
                .exists()
        );
        assert!(
            dir.join("sessions")
                .join(format!("{legacy_id}.jsonl.imported"))
                .exists()
        );
        assert!(repo.git().exists(&legacy_id).unwrap());
        assert_eq!(repo.entries(&legacy_id).unwrap().len(), 1);
        let again = repo.import_legacy_sessions();
        assert!(again.imported.is_empty());
        assert!(again.failed.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
