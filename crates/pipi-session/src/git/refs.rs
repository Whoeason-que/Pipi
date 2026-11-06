//! 分支与 ref 生命周期：命名、建分支、归档/恢复/删除。
//!
//! 一个会话 = 一条 `refs/heads/session/<id>` 分支；归档 = 移到
//! `refs/heads/archive/<id>`（ref 迁移，零复制）；删除 = 删 ref +
//! `gc --prune=now`（否则独有对象在盘上仍可被 `git fsck` 找回）。

use std::collections::HashMap;

use pipi_protocol::session::validate_session_id;

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

/// 分叉来源元数据 ref 前缀：`refs/pipi/forks/<new-session-id>/<source-session-id>`。
///
/// 值指向新分支 tip 提交（对象本来就可从分支到达，不额外保活）。它不参与
/// `refs/heads/session|archive` 列表，也不改 entry / commit 格式 —— 只是给
/// 右栏分支树回答「这条分支从哪来」。删除会话时先删它再 `gc`，否则 ref 会
/// 把分支对象继续保活。
pub const FORK_PREFIX: &str = "refs/pipi/forks";

/// 记录一条分支的分叉来源。失败不影响分叉本身（调用方只 warn）。
pub fn write_fork_marker(
    exec: &GitExec,
    session_id: &str,
    source_id: &str,
    tip: &str,
) -> Result<(), String> {
    validate_session_id(session_id)?;
    validate_session_id(source_id)?;
    let name = format!("{FORK_PREFIX}/{session_id}/{source_id}");
    exec.run(&["update-ref", &name, tip]).map(|_| ())
}

/// 读出全部分叉来源：`new session id -> source session id`。
pub fn fork_parents(exec: &GitExec) -> Result<HashMap<String, String>, String> {
    let output = exec.run(&["for-each-ref", "--format=%(refname)", FORK_PREFIX])?;
    let mut parents = HashMap::new();
    for line in String::from_utf8_lossy(&output).lines() {
        let Some(rest) = line.trim().strip_prefix(FORK_PREFIX) else {
            continue;
        };
        let Some((session_id, source_id)) = rest.trim_start_matches('/').split_once('/') else {
            continue;
        };
        if validate_session_id(session_id).is_ok() && validate_session_id(source_id).is_ok() {
            parents.insert(session_id.to_string(), source_id.to_string());
        }
    }
    Ok(parents)
}

/// 删除某条会话的分叉来源 ref（删除会话时先调用，再 [`delete_branch`]）。
pub fn delete_fork_markers(exec: &GitExec, session_id: &str) -> Result<(), String> {
    validate_session_id(session_id)?;
    let pattern = format!("{FORK_PREFIX}/{session_id}");
    let output = exec.run(&["for-each-ref", "--format=%(refname)", &pattern])?;
    for line in String::from_utf8_lossy(&output).lines() {
        let name = line.trim();
        if !name.is_empty() {
            exec.run(&["update-ref", "-d", name])?;
        }
    }
    Ok(())
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
        assert_eq!(
            restore(&exec, "s1").unwrap_err(),
            "Session does not exist in archive"
        );
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
        assert_eq!(
            archive(&exec, "s1").unwrap_err(),
            "Session with the same ID already exists in archive"
        );
        // 活跃区已有同名会话时拒绝恢复
        assert_eq!(
            restore(&exec, "s1").unwrap_err(),
            "Session with the same ID already exists in active region"
        );

        delete_branch(&exec, &session_ref("s1")).unwrap();
        assert!(!exec.ref_exists(&session_ref("s1")).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fork_markers_round_trip_and_are_scoped_to_the_session() {
        let dir = pipi_test_support::temp_dir("pipi-git-forks");
        let exec = GitExec::new(dir.join("sessions.git"));
        exec.run(&["rev-parse", "refs/heads/main"]).unwrap();
        let tip = exec.resolve_ref("refs/heads/main").unwrap().unwrap();

        write_fork_marker(&exec, "new-1", "old-1", &tip).unwrap();
        assert_eq!(
            fork_parents(&exec)
                .unwrap()
                .get("new-1")
                .map(String::as_str),
            Some("old-1")
        );
        // 非法 id 不会写进 ref 命名空间
        assert!(write_fork_marker(&exec, "bad/id", "old-1", &tip).is_err());
        // 删除只影响目标会话的 marker
        write_fork_marker(&exec, "new-2", "new-1", &tip).unwrap();
        delete_fork_markers(&exec, "new-1").unwrap();
        let parents = fork_parents(&exec).unwrap();
        assert!(!parents.contains_key("new-1"));
        assert_eq!(parents.get("new-2").map(String::as_str), Some("new-1"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
