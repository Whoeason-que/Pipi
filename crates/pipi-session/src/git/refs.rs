//! 分支与 ref 生命周期：命名、建分支、归档/恢复/删除。
//!
//! 一个会话 = 一条 `refs/heads/session/<id>` 分支；归档 = 移到
//! `refs/heads/archive/<id>`（ref 迁移，零复制）；删除 = 删 ref +
//! `gc --prune=now`（否则独有对象在盘上仍可被 `git fsck` 找回）。

use super::exec::GitExec;

/// 活跃区分支前缀。
pub const SESSION_PREFIX: &str = "session";
/// 归档区分支前缀（与 JSONL 的 `.archive` 目录等价）。
pub const ARCHIVE_PREFIX: &str = "archive";

pub fn session_ref(session_id: &str) -> String {
    format!("refs/heads/{SESSION_PREFIX}/{session_id}")
}

pub fn archive_ref(session_id: &str) -> String {
    format!("refs/heads/{ARCHIVE_PREFIX}/{session_id}")
}

/// 从某个已有提交起建一条新分支（零复制；压缩、分叉、恢复共用）。
pub fn branch_at(exec: &GitExec, ref_name: &str, commit: &str) -> Result<(), String> {
    exec.run(&["update-ref", ref_name, commit]).map(|_| ())
}

/// 归档：`session/<id>` → `archive/<id>`。先建新 ref 再删旧 ref。
pub fn archive(exec: &GitExec, session_id: &str) -> Result<(), String> {
    let source = session_ref(session_id);
    let Some(tip) = exec.resolve_ref(&source)? else {
        return Err("Session does not exist".into());
    };
    let target = archive_ref(session_id);
    if exec.resolve_ref(&target)?.is_some() {
        return Err("Session with the same ID already exists in archive".into());
    }
    branch_at(exec, &target, &tip)?;
    exec.run(&["update-ref", "-d", &source]).map(|_| ())
}

/// 恢复：`archive/<id>` → `session/<id>`。
pub fn restore(exec: &GitExec, session_id: &str) -> Result<(), String> {
    let source = archive_ref(session_id);
    let Some(tip) = exec.resolve_ref(&source)? else {
        return Err("Session does not exist in archive".into());
    };
    let target = session_ref(session_id);
    if exec.resolve_ref(&target)?.is_some() {
        return Err("Session with the same ID already exists in active region".into());
    }
    branch_at(exec, &target, &tip)?;
    exec.run(&["update-ref", "-d", &source]).map(|_| ())
}

/// 彻底删除一条分支，并立刻回收只被它引用的对象。
pub fn delete_branch(exec: &GitExec, ref_name: &str) -> Result<(), String> {
    exec.run(&["update-ref", "-d", ref_name])?;
    // 与其它线共享的保留段不会被清掉（仍被引用），这是共享存储的必然结果。
    exec.gc_prune_now()
}

/// 枚举某个前缀下的分支 id（不读内容，只要名字）。
pub fn list_ids(exec: &GitExec, prefix: &str) -> Result<Vec<String>, String> {
    let pattern = format!("refs/heads/{prefix}");
    let output = exec.run(&["for-each-ref", "--format=%(refname:short)", &pattern])?;
    let head = format!("{prefix}/");
    let mut ids = Vec::new();
    for line in String::from_utf8_lossy(&output).lines() {
        let line = line.trim();
        if let Some(id) = line.strip_prefix(&head)
            && !id.is_empty()
        {
            ids.push(id.to_string());
        }
    }
    ids.sort();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_names_are_prefixed_and_ids_stay_path_safe() {
        assert_eq!(session_ref("173-abc"), "refs/heads/session/173-abc");
        assert_eq!(archive_ref("173-abc"), "refs/heads/archive/173-abc");
    }

    #[test]
    fn branch_lifecycle_moves_refs_and_refuses_conflicts() {
        let dir = pipi_test_support::temp_dir("pipi-git-refs");
        let exec = GitExec::new(dir.join("sessions.git"));
        exec.run(&["rev-parse", "refs/heads/main"]).unwrap();
        let tip = exec.resolve_ref("refs/heads/main").unwrap().unwrap();

        // 空仓库里归档/恢复报「不存在」；删不存在的 ref 是幂等空操作
        assert_eq!(archive(&exec, "s1").unwrap_err(), "Session does not exist");
        assert_eq!(restore(&exec, "s1").unwrap_err(), "Session does not exist in archive");
        assert!(delete_branch(&exec, &session_ref("s1")).is_ok());

        branch_at(&exec, &session_ref("s1"), &tip).unwrap();
        assert!(exec.ref_exists(&session_ref("s1")).unwrap());
        assert_eq!(list_ids(&exec, SESSION_PREFIX).unwrap(), vec!["s1"]);

        archive(&exec, "s1").unwrap();
        assert!(!exec.ref_exists(&session_ref("s1")).unwrap());
        assert!(exec.ref_exists(&archive_ref("s1")).unwrap());
        assert_eq!(list_ids(&exec, ARCHIVE_PREFIX).unwrap(), vec!["s1"]);
        // 归档区已有同名会话时拒绝再归档
        branch_at(&exec, &session_ref("s1"), &tip).unwrap();
        assert_eq!(archive(&exec, "s1").unwrap_err(), "Session with the same ID already exists in archive");
        // 活跃区已有同名会话时拒绝恢复
        assert_eq!(restore(&exec, "s1").unwrap_err(), "Session with the same ID already exists in active region");

        delete_branch(&exec, &session_ref("s1")).unwrap();
        assert!(!exec.ref_exists(&session_ref("s1")).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }
}
