//! JSONL 会话仓库：一个 Agent 的 `sessions/` 目录即仓库。
//!
//! 归档 = 移动到 `sessions/.archive/<id>.jsonl`；删除 = 删文件。目录与文件的
//! 符号链接防护在这里（rename/remove 不能沿链接波及外部文件）。
use std::path::{Path, PathBuf};

use pipi_protocol::session::{
    ARCHIVE_DIR, EntryKind, SessionEntry, SessionLog, SessionRepo, SessionSummary,
    active_model_from_entries, active_path, validate_session_id,
};

use super::{SessionWriter, load_session};

/// [`SessionRepo`] 的 JSONL 实现。
pub struct JsonlSessionRepo {
    dir: PathBuf,
}

impl JsonlSessionRepo {
    /// 以 Agent 的 `sessions/` 目录构造。目录可以尚不存在（`create` 会建）。
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// 会话目录（组合载体枚举旧会话文件时用）。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 会话文件路径。目录与文件任何一环是符号链接都拒绝（fail-closed：
    /// 链接会让读写逃出数据树）；文件不存在按「没有会话」处理，由调用方决定
    /// 报错还是创建。这是全部目录级操作的唯一入口 —— 读、写、分叉、归档、
    /// 删除都经它取路径，守卫只有这一处。
    fn session_path(&self, session_id: &str) -> Result<PathBuf, String> {
        validate_session_id(session_id)?;
        pipi_protocol::fs::ensure_real_directory(&self.dir, "sessions 目录")?;
        let path = self.dir.join(format!("{session_id}.jsonl"));
        pipi_protocol::fs::ensure_real_file(&path, "会话文件")?;
        Ok(path)
    }

    /// 归档区目录；不存在时返回 None。
    fn archive_dir(&self) -> Result<Option<PathBuf>, String> {
        let dir = self.dir.join(ARCHIVE_DIR);
        match pipi_protocol::fs::ensure_real_directory(&dir, "归档目录")? {
            true => Ok(Some(dir)),
            false => Ok(None),
        }
    }
}

impl SessionRepo for JsonlSessionRepo {
    fn create(&self) -> Result<Box<dyn SessionLog>, String> {
        Ok(Box::new(
            SessionWriter::create(&self.dir).map_err(|error| error.to_string())?,
        ))
    }

    fn open(&self, session_id: &str) -> Result<Box<dyn SessionLog>, String> {
        let path = self.session_path(session_id)?;
        if !path.is_file() {
            return Err("会话不存在".into());
        }
        Ok(Box::new(
            SessionWriter::open(&path).map_err(|error| error.to_string())?,
        ))
    }

    fn exists(&self, session_id: &str) -> Result<bool, String> {
        // session_path 已经拒绝符号链接（Err）；走到这里要么是真文件，要么不存在
        Ok(self.session_path(session_id)?.is_file())
    }

    fn entries(&self, session_id: &str) -> Result<Vec<SessionEntry>, String> {
        let path = self.session_path(session_id)?;
        // 与 git 载体同口径的错误文案（conformance 套件盯这条）
        if !path.is_file() {
            return Err("会话不存在".into());
        }
        load_session(&path)
    }

    fn list(&self) -> Vec<SessionSummary> {
        list_session_summaries(&self.dir)
    }

    fn list_archived(&self) -> Result<Vec<SessionSummary>, String> {
        match self.archive_dir()? {
            Some(dir) => Ok(list_session_summaries(&dir)),
            None => Ok(Vec::new()),
        }
    }

    fn fork(
        &self,
        session_id: &str,
        up_to_entry_id: Option<&str>,
    ) -> Result<Box<dyn SessionLog>, String> {
        let source = self.session_path(session_id)?;
        Ok(Box::new(fork_session(&source, &self.dir, up_to_entry_id)?))
    }

    fn archive(&self, session_id: &str) -> Result<(), String> {
        archive_session_file(&self.dir, session_id).map(|_| ())
    }

    fn restore(&self, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let Some(archive) = self.archive_dir()? else {
            return Err("归档区没有该会话".into());
        };
        let source = archive.join(format!("{session_id}.jsonl"));
        if !pipi_protocol::fs::ensure_real_file(&source, "会话文件")? {
            return Err("归档区没有该会话".into());
        }
        let target = self.session_path(session_id)?;
        if target.exists() {
            return Err("活跃区已存在同名会话".into());
        }
        std::fs::rename(&source, &target).map_err(|error| format!("恢复会话失败: {error}"))
    }

    fn delete(&self, session_id: &str) -> Result<(), String> {
        let path = self.session_path(session_id)?;
        if !pipi_protocol::fs::ensure_real_file(&path, "会话文件")? {
            return Err("会话不存在".into());
        }
        std::fs::remove_file(&path).map_err(|error| format!("删除会话失败: {error}"))
    }

    fn delete_archived(&self, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let Some(archive) = self.archive_dir()? else {
            return Err("归档区没有该会话".into());
        };
        let path = archive.join(format!("{session_id}.jsonl"));
        if !pipi_protocol::fs::ensure_real_file(&path, "会话文件")? {
            return Err("归档区没有该会话".into());
        }
        std::fs::remove_file(&path).map_err(|error| format!("删除归档会话失败: {error}"))
    }
}

/// 从源会话中分叉出一个新会话文件：
/// 拷贝从根到目标 entry（若为 None 则到当前 tip）的活跃路径条目到新会话文件中。
/// 返回新建且 tip 指向分支末尾的 SessionWriter。
pub fn fork_session(
    source_session_path: &Path,
    sessions_dir: &Path,
    up_to_entry_id: Option<&str>,
) -> Result<SessionWriter, String> {
    let entries = load_session(source_session_path)?;
    if entries.is_empty() {
        return SessionWriter::create(sessions_dir).map_err(|e| e.to_string());
    }
    let active = if let Some(target_id) = up_to_entry_id {
        let by_id: std::collections::HashMap<&str, &SessionEntry> =
            entries.iter().map(|e| (e.id.as_str(), e)).collect();
        let mut path = Vec::new();
        let mut cursor = Some(target_id);
        while let Some(id) = cursor {
            if let Some(entry) = by_id.get(id) {
                path.push((*entry).clone());
                cursor = entry.parent_id.as_deref();
            } else {
                break;
            }
        }
        path.reverse();
        path
    } else {
        active_path(&entries).into_iter().cloned().collect()
    };

    let mut writer = SessionWriter::create(sessions_dir).map_err(|e| e.to_string())?;
    for entry in &active {
        writer.write_entry(entry).map_err(|e| e.to_string())?;
    }
    // 活跃路径上已有环境记账：置位，避免分叉出的会话再记一条（append_env 幂等）
    if active
        .iter()
        .any(|entry| matches!(entry.kind, EntryKind::Env { .. }))
    {
        writer.mark_env_recorded();
    }
    Ok(writer)
}

/// 把会话文件移入归档区（`sessions/.archive/<id>.jsonl`），返回新路径。
///
/// 纯移动、无标记（文件即真相，见 [`ARCHIVE_DIR`]）。自由函数形式是为了
/// 让运行任务（压缩换会话后归档原文件）也能调用 —— `RuntimeState::archive_session`
/// 在其上加了「会话正打开时拒绝」的占用检查。
pub fn archive_session_file(sessions_dir: &Path, session_id: &str) -> Result<PathBuf, String> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("非法会话 ID".into());
    }
    let source = sessions_dir.join(format!("{session_id}.jsonl"));
    if !pipi_protocol::fs::ensure_real_file(&source, "会话文件")? {
        return Err("会话不存在".into());
    }
    let archive_dir = sessions_dir.join(ARCHIVE_DIR);
    // 归档目录：不存在则创建；存在必须是真实目录（防符号链接）
    if !pipi_protocol::fs::ensure_real_directory(&archive_dir, "归档目录")? {
        std::fs::create_dir_all(&archive_dir).map_err(|e| format!("无法创建归档目录: {e}"))?;
    }
    let target = archive_dir.join(format!("{session_id}.jsonl"));
    if target.exists() {
        return Err("归档区已存在同名会话".into());
    }
    std::fs::rename(&source, &target).map_err(|e| format!("归档会话失败: {e}"))?;
    Ok(target)
}

/// 扫描会话目录，按最后活跃时间倒序返回摘要。
/// 坏行在 load_session 里跳过，不拖垮列表。
pub fn list_session_summaries(sessions_dir: &Path) -> Vec<SessionSummary> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let id = stem.to_string();
        let started_at = stem
            .split('-')
            .next()
            .and_then(|m| m.parse::<u64>().ok())
            .unwrap_or(0);
        let Ok(file_entries) = load_session(&path) else {
            continue;
        };
        let title = crate::title_from_entries(file_entries.iter());
        let message_count = file_entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Message { .. }))
            .count();
        let last_active = file_entries.last().map(|e| e.timestamp).unwrap_or(0);
        let model = active_model_from_entries(&file_entries)
            .map(|m| m.display_name().to_string())
            .or_else(|| crate::latest_assistant_model(file_entries.iter()));
        out.push(SessionSummary {
            id,
            title,
            message_count,
            started_at,
            last_active,
            model,
        });
    }
    out.sort_by_key(|summary| std::cmp::Reverse(summary.last_active));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::Message;
    use pipi_protocol::session::EnvDeclared;

    /// 断言被拒绝并取出错误消息。用 match 而非 `unwrap_err`：`Result<Box<dyn
    /// SessionLog>, _>` 的 Ok 侧没有 `Debug`，解不开。
    fn rejection<T>(what: &str, result: Result<T, String>) -> String {
        match result {
            Err(error) => error,
            Ok(_) => panic!("{what} 必须拒绝符号链接"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_session_file_is_rejected() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("hi")).unwrap();
        let real = writer.path().to_path_buf();
        drop(writer);

        // 指向真实会话文件的链接：任何目录级操作都必须拒绝，绝不跟随
        let link = dir.join("linked.jsonl");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let repo = JsonlSessionRepo::new(dir.clone());
        let errors = [
            rejection("exists", repo.exists("linked")),
            rejection("entries", repo.entries("linked")),
            rejection("open", repo.open("linked")),
            rejection("fork", repo.fork("linked", None)),
        ];
        for error in errors {
            assert!(error.contains("不能是符号链接"), "{error}");
        }

        // 真实文件照常可用；缺失的 id 仍是「不存在」而不是报错
        let real_id = real.file_stem().unwrap().to_str().unwrap();
        assert!(repo.exists(real_id).unwrap());
        assert!(!repo.exists("missing").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn broken_session_symlink_is_rejected_instead_of_treated_as_missing() {
        let dir = pipi_test_support::temp_dir("pipi-broken-session-link");
        std::os::unix::fs::symlink(dir.join("missing.jsonl"), dir.join("broken.jsonl")).unwrap();
        let repo = JsonlSessionRepo::new(dir);
        for error in [
            rejection("exists", repo.exists("broken")),
            rejection("entries", repo.entries("broken")),
            rejection("open", repo.open("broken")),
            rejection("fork", repo.fork("broken", None)),
        ] {
            assert!(error.contains("不能是符号链接"), "{error}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_sessions_dir_is_rejected() {
        let outside = pipi_test_support::temp_dir("pipi-test");
        let parent = pipi_test_support::temp_dir("pipi-test");
        let link = parent.join("sessions");
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        let error = rejection(
            "sessions 目录",
            JsonlSessionRepo::new(link).exists("anything"),
        );
        assert!(error.contains("不能是符号链接"), "{error}");
    }

    #[test]
    fn summaries_list_titles_and_order() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut w1 = SessionWriter::create(&dir).unwrap();
        w1.append_message(&Message::user_text("第一条会话的标题"))
            .unwrap();
        w1.append_message(&Message::user_text("second")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut w2 = SessionWriter::create(&dir).unwrap();
        w2.append_message(&Message::user_text("更新会话")).unwrap();

        let summaries = list_session_summaries(&dir);
        assert_eq!(summaries.len(), 2);
        // 按最后活跃倒序：w2 在前
        assert_eq!(summaries[0].title, "更新会话");
        assert_eq!(summaries[1].title, "第一条会话的标题");
        assert_eq!(summaries[1].message_count, 2);
        assert!(summaries[1].started_at > 0);
    }

    #[test]
    fn archive_session_file_moves_and_refuses_conflicts() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("hi")).unwrap();
        let path = writer.path().to_path_buf();
        let id = path.file_stem().unwrap().to_str().unwrap().to_string();
        drop(writer);

        let archived = archive_session_file(&dir, &id).unwrap();
        assert!(!path.exists(), "原文件应当已经移走");
        assert!(archived.is_file(), "归档区应当有该文件");
        assert_eq!(
            archived.file_name().unwrap().to_str().unwrap(),
            format!("{id}.jsonl")
        );
        // 归档后活跃区查不到
        assert!(!dir.join(format!("{id}.jsonl")).exists());

        // 重复归档：活跃区已无此会话
        assert!(archive_session_file(&dir, &id).is_err());
        // 非法 id
        assert!(archive_session_file(&dir, "../evil").is_err());
        // 目标已存在时拒绝（把文件放回去再归档一次）
        std::fs::rename(&archived, &path).unwrap();
        std::fs::write(&archived, "{}\n").unwrap();
        let error = archive_session_file(&dir, &id).unwrap_err();
        assert!(error.contains("已存在"), "{error}");
    }

    #[test]
    fn fork_marks_env_recorded_when_path_already_has_env_entry() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();
        writer.append_message(&Message::user_text("hi")).unwrap();
        writer
            .append_env(vec![EnvDeclared {
                key: "AV_AGENT".into(),
                source: "runtime".into(),
            }])
            .unwrap();
        let source_path = writer.path().to_path_buf();
        drop(writer);

        let mut forked = fork_session(&source_path, &dir, None).unwrap();
        // 活跃路径已带 Env 条目 → 分叉出的会话不应再记一条
        assert!(
            forked
                .append_env(vec![EnvDeclared {
                    key: "AV_AGENT".into(),
                    source: "runtime".into(),
                }])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn fork_session_copies_active_history() {
        let dir = pipi_test_support::temp_dir("pipi-test");
        let mut writer = SessionWriter::create(&dir).unwrap();
        let id1 = writer.append_message(&Message::user_text("msg 1")).unwrap();
        let _id2 = writer.append_message(&Message::user_text("msg 2")).unwrap();
        let source_path = writer.path().to_path_buf();
        drop(writer);

        // 分叉到 id1
        let mut forked = fork_session(&source_path, &dir, Some(&id1)).unwrap();
        assert_eq!(forked.tip_id(), Some(id1.as_str()));
        let id3 = forked
            .append_message(&Message::user_text("msg 3 branch"))
            .unwrap();
        let forked_entries = load_session(forked.path()).unwrap();
        assert_eq!(forked_entries.len(), 2);
        assert_eq!(forked_entries[1].parent_id.as_deref(), Some(id1.as_str()));
        assert_eq!(forked_entries[1].id, id3);
    }
}
