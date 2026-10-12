//! `rm -f` 家族在 `workspace-write` 沙箱下的唯一放行例外：只作用于工作区内
//! 字面量路径的直接调用（判据见 [`forced_rm_inside_workspace`]）。

use std::fs;
use std::path::Path;

use super::parse::split_segments;
use pipi_protocol::permissions::resolve_write_path;

/// `rm` 的 force 变体是否只作用于工作区内的字面量路径（清理构建 / 缓存产物的
/// 常规操作）。
///
/// 放行需同时满足：
/// - 直接调用 `rm`：`sudo` / `env` / `trap` / `bash -c` 包装一律不放行；
/// - 旗标只含 `-r` / `-R` / `-f` 的组合或 `--recursive` / `--force`；
/// - 至少一个目标，且每个目标都是字面量路径（不含 `~` `$` 反引号反斜杠与
///   通配符）—— 变量和 glob 无法静态判定删除范围；
/// - 每个目标经 canonical containment 解析后落在工作区内，且既不是工作区根、
///   也不是 git 仓库根（一次删掉整个仓库不允许）；
/// - 相对路径仅在命令内没有任何 `cd` / `pushd` / `popd` 时可放行：否则工具的
///   cwd 与解析基准不一致，只能用绝对路径表达。
///
/// 任一条件不满足返回 `false`，由调用方按「强制删除」拒绝（不可审批）。
pub(super) fn forced_rm_inside_workspace(argv: &[String], workspace: &Path, command: &str) -> bool {
    let Some(program) = argv.first().and_then(|arg| arg.rsplit('/').next()) else {
        return false;
    };
    if program != "rm" {
        return false;
    }
    let Ok(canonical_workspace) = fs::canonicalize(workspace) else {
        return false;
    };
    let relative_allowed = !command_changes_directory(command);

    let mut after_separator = false;
    let mut targets: Vec<&str> = Vec::new();
    for arg in &argv[1..] {
        if !after_separator && arg == "--" {
            after_separator = true;
            continue;
        }
        if !after_separator && arg.starts_with('-') {
            if !rm_flag_is_supported(arg) {
                return false;
            }
            continue;
        }
        targets.push(arg);
    }

    !targets.is_empty()
        && targets.iter().all(|target| {
            if !is_literal_path(target) {
                return false;
            }
            if !Path::new(target).is_absolute() && !relative_allowed {
                return false;
            }
            match resolve_write_path(workspace, target) {
                Ok(resolved) => resolved != canonical_workspace && !resolved.join(".git").exists(),
                Err(_) => false,
            }
        })
}

/// 命令内是否可能出现目录切换（`cd` / `pushd` / `popd`）。
///
/// 切段失败时按「可能切换」处理（fail-closed）；本条评估之前，无法解析的命令
/// 已经被拒绝，这里只是不引入更宽的假设。
fn command_changes_directory(command: &str) -> bool {
    match split_segments(command) {
        Ok(segments) => segments.iter().any(|seg| {
            matches!(
                seg.argv.first().map(String::as_str),
                Some("cd") | Some("pushd") | Some("popd")
            )
        }),
        Err(_) => true,
    }
}

/// `rm` 旗标白名单：只接受不改变「删什么」语义的旗标。
fn rm_flag_is_supported(flag: &str) -> bool {
    match flag {
        "--recursive" | "--force" => true,
        other => other.strip_prefix('-').is_some_and(|chars| {
            !chars.is_empty()
                && !chars.starts_with('-')
                && chars.chars().all(|c| matches!(c, 'r' | 'R' | 'f'))
        }),
    }
}

/// 字面量路径：含变量、通配符或 `~` 的路径无法静态判定，一律不放行。
fn is_literal_path(raw: &str) -> bool {
    !raw.is_empty()
        && !raw.starts_with('~')
        && !raw.contains(['*', '?', '[', ']', '{', '}', '$', '`', '\\'])
}
