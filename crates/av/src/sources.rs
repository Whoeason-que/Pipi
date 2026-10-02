//! 安装源：spec 解析、git 修订解析与浅取、来源内技能发现。
//!
//! 支持的 spec（对齐 skills.sh 生态习惯）：
//! - 本地目录：`./x`、`../x`、`/abs/x`、`~/x`（`.`/`..` 亦可）；
//! - GitHub 短名：`owner/repo[@ref][#子目录]`（`github:` 前缀等价）；
//! - GitLab 短名：`gitlab:owner/repo[@ref][#子目录]`；
//! - 完整 git URL：`https://…`、`git@host:path`、`ssh://…`（可带 `@ref`/`#子目录`）；
//! - GitHub/GitLab 网页 tree URL（复制粘贴即可用）：
//!   `https://github.com/o/r/tree/<ref>/<path>`、`https://gitlab.com/o/r/-/tree/<ref>/<path>`。
//!
//! 明确不支持（fail-closed 并给指引）：`skills.sh` 的网页/包直链（本轮不做
//! 归档下载）——改用 `owner/repo` 短名、git URL 或本地目录。
//!
//! git 一律 spawn 系统 `git`（只读操作 + 浅取到暂存目录），不引入 git2：
//! `git ls-remote` 定 ref → 精确 commit，`git fetch --depth 1` 取该 commit。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::paths;
use crate::skills::{self, SkillMeta};
use crate::store::SourceType;

/// 约定候选目录（按生态顺序：通用 `skills/` 优先，随后 dot 目录）。
const CANDIDATE_DIRS: &[&str] = &[
    "skills",
    "skills/.curated",
    "skills/.experimental",
    "skills/.system",
    ".agents/skills",
    ".claude/skills",
];

/// 约定候选目录向下的限深（`skills/` 内通常一层一个技能，允许一层分组）。
const CANDIDATE_DEPTH: usize = 3;
/// 全树兜底搜索的限深。
const FULL_SEARCH_DEPTH: usize = 5;

/// 递归时跳过的目录名（`.git` 与点目录另行处理）。
const SKIP_DIRS: &[&str] = &["node_modules", "dist", "build", "__pycache__", "target"];

/// 解析后的安装源。
#[derive(Debug, Clone, PartialEq)]
pub enum SourceSpec {
    Git {
        /// 仓库 URL（`https://…` / `git@host:path`）。
        url: String,
        /// 分支/标签/commit；None = 远端 HEAD。
        reference: Option<String>,
        /// 限定发现范围的来源内子路径。
        skill_path: Option<String>,
    },
    Local {
        /// 绝对、canonical 的本地目录。
        path: PathBuf,
    },
}

impl SourceSpec {
    /// 锁文件里的 source 字段。
    pub fn source_label(&self) -> String {
        match self {
            SourceSpec::Git { url, .. } => url.clone(),
            SourceSpec::Local { path } => path.to_string_lossy().into_owned(),
        }
    }

    pub fn source_type(&self) -> SourceType {
        match self {
            SourceSpec::Git { .. } => SourceType::Git,
            SourceSpec::Local { .. } => SourceType::Local,
        }
    }
}

fn is_local_path(raw: &str) -> bool {
    raw == "."
        || raw == ".."
        || raw == "~"
        || raw.starts_with("./")
        || raw.starts_with("../")
        || raw.starts_with('/')
        || raw.starts_with("~/")
        || raw.starts_with(".\\")
        || raw.starts_with("..\\")
        || raw.as_bytes().get(1) == Some(&b':') // Windows 盘符
}

/// 解析 spec；不做网络访问（本地路径在这里 canonicalize 并校验存在）。
pub fn parse_spec(raw: &str) -> Result<SourceSpec, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("安装源不能为空".into());
    }
    if raw == "skills.sh"
        || raw.starts_with("skills.sh/")
        || raw.starts_with("https://skills.sh/")
        || raw.starts_with("http://skills.sh/")
    {
        return Err(format!(
            "不支持 skills.sh 的网页/包直链（{raw}）：请改用 `owner/repo` 短名、git URL 或本地目录"
        ));
    }
    if is_local_path(raw) {
        let expanded = crate::discovery::expand_tilde(raw);
        let canonical = fs::canonicalize(&expanded)
            .map_err(|e| format!("本地来源不可用 {}: {e}", expanded.display()))?;
        if !canonical.is_dir() {
            return Err(format!("本地来源不是目录：{}", canonical.display()));
        }
        return Ok(SourceSpec::Local { path: canonical });
    }
    if let Some(rest) = raw.strip_prefix("github:") {
        return shorthand_github(rest);
    }
    if let Some(rest) = raw.strip_prefix("gitlab:") {
        return shorthand_gitlab(rest);
    }
    if let Some(spec) = parse_tree_url(raw)? {
        return Ok(spec);
    }
    if looks_like_git_url(raw) {
        return parse_git_url(raw, None);
    }
    // `owner/repo` 短名（skills.sh 生态约定；优先于同名相对目录）
    if is_owner_repo(raw) {
        return shorthand_github(raw);
    }
    Err(format!(
        "无法识别的安装源 {raw:?}：支持 `owner/repo[@ref][#子目录]`、`gitlab:owner/repo`、\
         git URL（https/ssh/git@）与本地目录（./、../、/、~/）"
    ))
}

fn shorthand_github(rest: &str) -> Result<SourceSpec, String> {
    let (owner_repo, reference, skill_path) = split_spec_suffix(rest)?;
    if !is_owner_repo(owner_repo) {
        return Err(format!(
            "GitHub 短名必须是 `owner/repo`（得到 {owner_repo:?}）"
        ));
    }
    Ok(SourceSpec::Git {
        url: format!("https://github.com/{owner_repo}"),
        reference,
        skill_path,
    })
}

fn shorthand_gitlab(rest: &str) -> Result<SourceSpec, String> {
    let (owner_repo, reference, skill_path) = split_spec_suffix(rest)?;
    if !is_owner_repo(owner_repo) {
        return Err(format!(
            "GitLab 短名必须是 `owner/repo`（得到 {owner_repo:?}）"
        ));
    }
    Ok(SourceSpec::Git {
        url: format!("https://gitlab.com/{owner_repo}"),
        reference,
        skill_path,
    })
}

/// 拆出 `[@ref][#子目录]` 后缀；ref 不允许含 `/`（避免与路径歧义）。
fn split_spec_suffix(raw: &str) -> Result<(&str, Option<String>, Option<String>), String> {
    let (head, skill_path) = match raw.split_once('#') {
        Some((head, sub)) => {
            let sub = sub.trim();
            if sub.is_empty() {
                return Err("`#` 后的子目录不能为空".into());
            }
            (head, Some(validate_relative_subpath(sub)?))
        }
        None => (raw, None),
    };
    let (head, reference) = match head.rsplit_once('@') {
        Some((head, reference)) => {
            let reference = reference.trim();
            if reference.is_empty() {
                return Err("`@` 后的 ref 不能为空".into());
            }
            if reference.contains('/') {
                return Err(format!(
                    "ref 不能包含 `/`（得到 {reference:?}）：请用分支/标签/commit，子目录写在 `#` 之后"
                ));
            }
            (head, Some(reference.to_string()))
        }
        None => (head, None),
    };
    let head = head.trim();
    if head.is_empty() {
        return Err("安装源缺少仓库部分".into());
    }
    Ok((head, reference, skill_path))
}

fn is_owner_repo(raw: &str) -> bool {
    let mut segments = raw.split('/');
    let (Some(owner), Some(repo), None) = (segments.next(), segments.next(), segments.next())
    else {
        return false;
    };
    !owner.is_empty()
        && !repo.is_empty()
        && owner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && repo
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn looks_like_git_url(raw: &str) -> bool {
    raw.starts_with("https://")
        || raw.starts_with("http://")
        || raw.starts_with("ssh://")
        || raw.starts_with("git://")
        || raw.starts_with("file://")
        || raw.starts_with("git@")
}

/// 完整 git URL：切出 `#子目录` 与尾部 `@ref`（用「最后一个 `/` 之后」判定，
/// 因此 `https://user@host/o/r` 的 userinfo 不会被当成 ref）。
fn parse_git_url(raw: &str, default_reference: Option<String>) -> Result<SourceSpec, String> {
    let (head, skill_path) = match raw.split_once('#') {
        Some((head, sub)) => {
            let sub = sub.trim();
            if sub.is_empty() {
                return Err("`#` 后的子目录不能为空".into());
            }
            (head, Some(validate_relative_subpath(sub)?))
        }
        None => (raw, None),
    };
    let last_at = head.rfind('@');
    let last_slash = head.rfind('/');
    let (url, reference) = match (last_at, last_slash) {
        (Some(at), Some(slash)) if at > slash => {
            let reference = head[at + 1..].trim();
            if reference.is_empty() {
                return Err("`@` 后的 ref 不能为空".into());
            }
            if reference.contains('@') {
                return Err(format!("无法识别该 git URL：{raw:?}"));
            }
            (
                head[..at].trim_end_matches('/').to_string(),
                Some(reference.to_string()),
            )
        }
        _ => (head.trim_end_matches('/').to_string(), default_reference),
    };
    if url.is_empty() {
        return Err(format!("无法识别的 git URL：{raw:?}"));
    }
    Ok(SourceSpec::Git {
        url,
        reference,
        skill_path,
    })
}

/// GitHub/GitLab 网页 tree URL → git 源（带 ref 与子目录）。
fn parse_tree_url(raw: &str) -> Result<Option<SourceSpec>, String> {
    let Some(rest) = raw
        .strip_prefix("https://github.com/")
        .or_else(|| raw.strip_prefix("http://github.com/"))
    else {
        if let Some(rest) = raw
            .strip_prefix("https://gitlab.com/")
            .or_else(|| raw.strip_prefix("http://gitlab.com/"))
        {
            return parse_gitlab_tree_url(raw, rest);
        }
        return Ok(None);
    };
    let mut segments = rest.splitn(6, '/');
    let (Some(owner), Some(repo), Some(kind)) = (segments.next(), segments.next(), segments.next())
    else {
        return Ok(None);
    };
    if kind != "tree" {
        return Ok(None);
    }
    let Some(reference) = segments.next().filter(|r| !r.is_empty()) else {
        return Err(format!("{raw} 缺少 ref（形如 /tree/<ref>/<子目录>）"));
    };
    let path = segments.collect::<Vec<_>>().join("/");
    let skill_path = if path.is_empty() {
        None
    } else {
        Some(validate_relative_subpath(&path)?)
    };
    Ok(Some(SourceSpec::Git {
        url: format!("https://github.com/{owner}/{repo}"),
        reference: Some(reference.to_string()),
        skill_path,
    }))
}

fn parse_gitlab_tree_url(raw: &str, rest: &str) -> Result<Option<SourceSpec>, String> {
    let Some((repo_part, tail)) = rest.split_once("/-/tree/") else {
        return Ok(None);
    };
    if !is_owner_repo(repo_part) {
        return Ok(None);
    }
    let mut segments = tail.splitn(2, '/');
    let Some(reference) = segments.next().filter(|r| !r.is_empty()) else {
        return Err(format!("{raw} 缺少 ref（形如 /-/tree/<ref>/<子目录>）"));
    };
    let path = segments.next().unwrap_or("").trim_matches('/');
    let skill_path = if path.is_empty() {
        None
    } else {
        Some(validate_relative_subpath(path)?)
    };
    Ok(Some(SourceSpec::Git {
        url: format!("https://gitlab.com/{repo_part}"),
        reference: Some(reference.to_string()),
        skill_path,
    }))
}

/// 子路径校验（fail-closed）：相对、不含 `..`、不以 `~` 开头。
fn validate_relative_subpath(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Err("子目录不能为空".into());
    }
    if Path::new(trimmed).is_absolute() || trimmed.starts_with('~') {
        return Err(format!("子目录必须是来源内的相对路径：{raw:?}"));
    }
    if trimmed
        .split('/')
        .any(|segment| segment == ".." || segment.is_empty() || segment == ".")
    {
        return Err(format!("子目录不能包含 `.`/`..`/空段：{raw:?}"));
    }
    Ok(trimmed.to_string())
}

// ============ git 解析与获取 ============

fn git_command() -> Command {
    let mut command = Command::new("git");
    // 非交互（失败即报错，避免挂起等待凭据输入）；不拉 LFS 实体
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.env("GIT_LFS_SKIP_SMUDGE", "1");
    command
}

fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut command = git_command();
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .output()
        .map_err(|e| format!("无法执行 git（需要系统 git）: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail = stderr.trim();
        let tail = if tail.len() > 500 {
            &tail[tail.len() - 500..]
        } else {
            tail
        };
        return Err(format!("git {} 失败: {tail}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 把一个 ref（分支/标签/commit；None = 远端 HEAD）解析为精确 commit。
pub fn resolve_revision(url: &str, reference: Option<&str>) -> Result<String, String> {
    if let Some(reference) = reference
        && reference.len() == 40
        && reference.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Ok(reference.to_string());
    }
    let pattern = reference.unwrap_or("HEAD");
    let output = run_git(&["ls-remote", url, pattern], None)?;
    let mut best: Option<(u8, String)> = None;
    for line in output.lines() {
        let Some((sha, refname)) = line.split_once('\t') else {
            continue;
        };
        let refname = refname.trim();
        // 精确分支/标签，或（annotated tag 的）peeled 行 → 最高优先级
        let exact = Some(refname.strip_prefix("refs/heads/")) == Some(reference)
            || Some(refname.strip_prefix("refs/tags/")) == Some(reference);
        let rank = if refname == "HEAD" {
            0
        } else if exact || refname.ends_with("^{}") {
            1
        } else {
            2
        };
        if sha.len() != 40 {
            continue;
        }
        if best.as_ref().is_none_or(|(best_rank, _)| rank < *best_rank) {
            best = Some((rank, sha.to_string()));
        }
    }
    best.map(|(_, sha)| sha)
        .ok_or_else(|| format!("无法在 {url} 解析 ref {pattern:?}（分支/标签/commit 不存在？）"))
}

/// 浅取到暂存目录的工作树；`Drop` 时自动清理。
#[derive(Debug)]
pub struct GitFetch {
    pub worktree: PathBuf,
    pub revision: String,
    pub reference: Option<String>,
}

impl Drop for GitFetch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.worktree);
    }
}

/// 解析 ref 并浅取对应 commit。
pub fn fetch(url: &str, reference: Option<&str>) -> Result<GitFetch, String> {
    let revision = resolve_revision(url, reference)?;
    fetch_at(url, reference, &revision)
}

/// 浅取指定 commit（`sync` 复现锁定的修订时用；不重新解析 ref）。
pub fn fetch_at(url: &str, reference: Option<&str>, revision: &str) -> Result<GitFetch, String> {
    if revision.len() != 40 || !revision.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "revision 必须是 40 位十六进制 commit：{revision:?}"
        ));
    }
    let staging = paths::staging_dir().ok_or("无法定位 av 家目录（~/.av）")?;
    fetch_at_in(&staging, url, reference, revision)
}

fn fetch_at_in(
    staging_root: &Path,
    url: &str,
    reference: Option<&str>,
    revision: &str,
) -> Result<GitFetch, String> {
    fs::create_dir_all(staging_root)
        .map_err(|e| format!("无法创建暂存目录 {}: {e}", staging_root.display()))?;
    let worktree = staging_root.join(format!(
        "git-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&worktree).map_err(|e| format!("无法创建暂存目录: {e}"))?;
    let cleanup = |error: String| {
        let _ = fs::remove_dir_all(&worktree);
        Err(error)
    };

    if let Err(e) = run_git(&["init", "--quiet"], Some(&worktree)) {
        return cleanup(e);
    }
    if let Err(e) = run_git(&["remote", "add", "origin", url], Some(&worktree)) {
        return cleanup(e);
    }
    // 首选：按精确 commit 浅取（GitHub 等支持 reachable SHA 的 fetch）
    if run_git(
        &["fetch", "--quiet", "--depth", "1", "origin", revision],
        Some(&worktree),
    )
    .is_ok()
    {
        if let Err(e) = run_git(
            &["checkout", "--quiet", "--detach", "FETCH_HEAD"],
            Some(&worktree),
        ) {
            return cleanup(e);
        }
        return Ok(GitFetch {
            worktree,
            revision: revision.to_string(),
            reference: reference.map(str::to_string),
        });
    }
    // 兜底：按 ref 浅取并复核 commit 一致（ref 在解析后移动即 fail-closed）
    let pattern = reference.unwrap_or("HEAD");
    if let Err(e) = run_git(
        &["fetch", "--quiet", "--depth", "1", "origin", pattern],
        Some(&worktree),
    ) {
        return cleanup(format!(
            "无法获取 {url} 的 {pattern:?}（commit {revision}）：{e}"
        ));
    }
    let head = match run_git(&["rev-parse", "FETCH_HEAD"], Some(&worktree)) {
        Ok(head) => head.trim().to_string(),
        Err(e) => return cleanup(e),
    };
    if head != revision {
        return cleanup(format!(
            "ref {pattern:?} 在解析后发生了移动（期望 {revision}，实际 {head}）；请重试"
        ));
    }
    if let Err(e) = run_git(
        &["checkout", "--quiet", "--detach", "FETCH_HEAD"],
        Some(&worktree),
    ) {
        return cleanup(e);
    }
    Ok(GitFetch {
        worktree,
        revision: revision.to_string(),
        reference: reference.map(str::to_string),
    })
}

// ============ 来源内技能发现 ============

/// 来源中发现的一个技能。
#[derive(Debug, Clone)]
pub struct SourceSkill {
    pub name: String,
    /// 技能目录（canonical）。
    pub dir: PathBuf,
    /// 相对来源根路径（`/` 分隔；来源根自身即技能时为 `.`）。
    pub relative: String,
    pub meta: SkillMeta,
}

/// 发现结果：合法技能 + 被跳过的目录（含原因，供 CLI 提示）。
#[derive(Debug, Default)]
pub struct Discovery {
    pub skills: Vec<SourceSkill>,
    pub skipped: Vec<(String, String)>,
}

/// 在来源根中发现技能：
/// 1. 显式子路径（`#子目录`）→ 只在该目录下找；
/// 2. 来源根自身含 `SKILL.md` → 单技能；
/// 3. 约定候选目录（`skills/`、`skills/.curated`、…、`.agents/skills`）；
/// 4. 兜底：全树递归（限深 [`FULL_SEARCH_DEPTH`]，跳过 `node_modules` 等）。
///
/// 找到 `SKILL.md` 的目录即技能根、不再下钻；同名技能先发现者赢
/// （与运行时的 `load_skill_sources` 一致）。
pub fn discover_source_skills(root: &Path, subpath: Option<&str>) -> Result<Discovery, String> {
    let root =
        fs::canonicalize(root).map_err(|e| format!("无法读取来源目录 {}: {e}", root.display()))?;
    let mut discovery = Discovery::default();
    let mut seen_dirs = HashSet::new();
    let mut seen_names = HashSet::new();

    if let Some(subpath) = subpath {
        let dir = root.join(subpath);
        let canonical = fs::canonicalize(&dir)
            .map_err(|e| format!("来源子目录不可用 {}: {e}", dir.display()))?;
        if !canonical.starts_with(&root) {
            return Err(format!(
                "来源子目录 {} 逃逸来源根，拒绝（fail-closed）",
                canonical.display()
            ));
        }
        collect_from_dir(
            &canonical,
            &root,
            FULL_SEARCH_DEPTH,
            &mut seen_dirs,
            &mut seen_names,
            &mut discovery,
        )?;
        return Ok(discovery);
    }

    if root.join("SKILL.md").is_file() {
        collect_from_dir(
            &root,
            &root,
            1,
            &mut seen_dirs,
            &mut seen_names,
            &mut discovery,
        )?;
    }
    for candidate in CANDIDATE_DIRS {
        let dir = root.join(candidate);
        if dir.is_dir() {
            collect_from_dir(
                &dir,
                &root,
                CANDIDATE_DEPTH,
                &mut seen_dirs,
                &mut seen_names,
                &mut discovery,
            )?;
        }
    }
    if discovery.skills.is_empty() {
        // 兜底全树：候选目录没命中时再扫（避免大仓库无谓遍历）
        collect_from_dir(
            &root,
            &root,
            FULL_SEARCH_DEPTH,
            &mut seen_dirs,
            &mut seen_names,
            &mut discovery,
        )?;
    }
    Ok(discovery)
}

fn collect_from_dir(
    dir: &Path,
    root: &Path,
    depth: usize,
    seen_dirs: &mut HashSet<PathBuf>,
    seen_names: &mut HashSet<String>,
    discovery: &mut Discovery,
) -> Result<(), String> {
    let canonical =
        fs::canonicalize(dir).map_err(|e| format!("无法读取目录 {}: {e}", dir.display()))?;
    if !canonical.starts_with(root) {
        return Err(format!(
            "目录 {} 逃逸来源根，拒绝（fail-closed）",
            canonical.display()
        ));
    }
    if !seen_dirs.insert(canonical.clone()) {
        return Ok(());
    }
    let relative = relative_label(root, &canonical);

    if canonical.join("SKILL.md").is_file() {
        match skills::read_skill_meta(&canonical) {
            Some(meta) if seen_names.insert(meta.name.clone()) => {
                discovery.skills.push(SourceSkill {
                    name: meta.name.clone(),
                    dir: canonical,
                    relative,
                    meta,
                });
            }
            Some(meta) => {
                // 同名先发现者赢：静默跳过（与 load_skill_sources 一致）
                let _ = meta;
            }
            None => discovery.skipped.push((
                relative,
                "SKILL.md 缺失 name/description 或名称形状非法".to_string(),
            )),
        }
        return Ok(());
    }

    if depth == 0 {
        return Ok(());
    }
    let entries = fs::read_dir(&canonical)
        .map_err(|e| format!("无法读取目录 {}: {e}", canonical.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("无法读取目录 {}: {e}", canonical.display()))?;
        paths.push(entry.path());
    }
    paths.sort();
    for path in paths {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') || SKIP_DIRS.contains(&name) {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        collect_from_dir(&path, root, depth - 1, seen_dirs, seen_names, discovery)?;
    }
    Ok(())
}

fn relative_label(root: &Path, dir: &Path) -> String {
    if dir == root {
        return ".".to_string();
    }
    dir.strip_prefix(root)
        .map(|relative| {
            relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_else(|_| dir.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "av-sources-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn spec_parsing_shorthands() {
        assert_eq!(
            parse_spec("vercel-labs/agent-skills").unwrap(),
            SourceSpec::Git {
                url: "https://github.com/vercel-labs/agent-skills".into(),
                reference: None,
                skill_path: None,
            }
        );
        assert_eq!(
            parse_spec("github:o/r@v1.2#skills/pdf").unwrap(),
            SourceSpec::Git {
                url: "https://github.com/o/r".into(),
                reference: Some("v1.2".into()),
                skill_path: Some("skills/pdf".into()),
            }
        );
        assert_eq!(
            parse_spec("gitlab:o/r@main").unwrap(),
            SourceSpec::Git {
                url: "https://gitlab.com/o/r".into(),
                reference: Some("main".into()),
                skill_path: None,
            }
        );
        // tree URL（GitHub / GitLab）
        assert_eq!(
            parse_spec("https://github.com/o/r/tree/v1/skills/pdf").unwrap(),
            SourceSpec::Git {
                url: "https://github.com/o/r".into(),
                reference: Some("v1".into()),
                skill_path: Some("skills/pdf".into()),
            }
        );
        assert_eq!(
            parse_spec("https://gitlab.com/o/r/-/tree/main/skills/pdf").unwrap(),
            SourceSpec::Git {
                url: "https://gitlab.com/o/r".into(),
                reference: Some("main".into()),
                skill_path: Some("skills/pdf".into()),
            }
        );
    }

    #[test]
    fn spec_parsing_git_urls() {
        assert_eq!(
            parse_spec("https://github.com/o/r.git@v2").unwrap(),
            SourceSpec::Git {
                url: "https://github.com/o/r.git".into(),
                reference: Some("v2".into()),
                skill_path: None,
            }
        );
        // userinfo 的 @ 不能被当成 ref
        assert_eq!(
            parse_spec("https://user@example.com/o/r").unwrap(),
            SourceSpec::Git {
                url: "https://user@example.com/o/r".into(),
                reference: None,
                skill_path: None,
            }
        );
        // scp 形式
        assert_eq!(
            parse_spec("git@github.com:o/r.git@v3").unwrap(),
            SourceSpec::Git {
                url: "git@github.com:o/r.git".into(),
                reference: Some("v3".into()),
                skill_path: None,
            }
        );
        assert_eq!(
            parse_spec("git@github.com:o/r.git").unwrap(),
            SourceSpec::Git {
                url: "git@github.com:o/r.git".into(),
                reference: None,
                skill_path: None,
            }
        );
        // ref 带 / 要拒绝
        assert!(parse_spec("owner/repo@feature/x").is_err());
    }

    #[test]
    fn spec_parsing_local_and_fail_closed() {
        let root = temp_dir("local");
        let spec = parse_spec(root.to_str().unwrap()).unwrap();
        assert_eq!(spec.source_type(), SourceType::Local);
        assert!(parse_spec("./definitely-not-here-xyz").is_err());

        for bad in [
            "",
            "skills.sh",
            "https://skills.sh/p/abc",
            "no-slashes",
            "a/b/c/d",
        ] {
            assert!(parse_spec(bad).is_err(), "应拒绝：{bad:?}");
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovery_finds_nested_and_convention_dirs() {
        let root = temp_dir("discovery");
        // 约定目录：skills/ 下两个技能 + 一层分组
        fs::create_dir_all(root.join("skills/pdf")).unwrap();
        fs::write(
            root.join("skills/pdf/SKILL.md"),
            "---\nname: pdf\ndescription: pdf skill\n---\nbody",
        )
        .unwrap();
        fs::create_dir_all(root.join("skills/group/writer")).unwrap();
        fs::write(
            root.join("skills/group/writer/SKILL.md"),
            "---\nname: writer\ndescription: writing\n---\nbody",
        )
        .unwrap();
        // 非技能目录（无 SKILL.md）不该被当技能
        fs::create_dir_all(root.join("skills/not-a-skill")).unwrap();
        fs::write(root.join("skills/not-a-skill/README.md"), "x").unwrap();

        let discovery = discover_source_skills(&root, None).unwrap();
        let names: Vec<&str> = discovery.skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["writer", "pdf"], "顺序：扫描到即记录");
        assert_eq!(discovery.skills[0].relative, "skills/group/writer");

        // 子路径限定
        let discovery = discover_source_skills(&root, Some("skills/pdf")).unwrap();
        assert_eq!(discovery.skills.len(), 1);
        assert_eq!(discovery.skills[0].name, "pdf");
        assert_eq!(discovery.skills[0].relative, "skills/pdf");

        // 逃逸子路径拒绝
        assert!(discover_source_skills(&root, Some("../outside")).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovery_root_is_skill_and_invalid_reported() {
        let root = temp_dir("root-skill");
        fs::write(
            root.join("SKILL.md"),
            "---\nname: solo\ndescription: solo skill\n---\nbody",
        )
        .unwrap();
        let discovery = discover_source_skills(&root, None).unwrap();
        assert_eq!(discovery.skills.len(), 1);
        assert_eq!(discovery.skills[0].relative, ".");

        // 兄弟目录里坏的 SKILL.md 进入 skipped
        fs::create_dir_all(root.join("broken")).unwrap();
        fs::write(root.join("broken/SKILL.md"), "---\nname: broken\n---\nbody").unwrap();
        let discovery = discover_source_skills(&root, None).unwrap();
        assert_eq!(discovery.skills.len(), 1, "根自身即技能，不再下钻");
        let _ = fs::remove_dir_all(root);
    }

    /// 建一个本地 git 仓库（含一个技能），返回 (仓库路径, HEAD commit)。
    fn make_repo(label: &str) -> (PathBuf, String) {
        let repo = temp_dir(label);
        let run = |args: &[&str]| {
            let output = git_command()
                .args(args)
                .current_dir(&repo)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "--quiet"]);
        fs::create_dir_all(repo.join("skills/pdf")).unwrap();
        fs::write(
            repo.join("skills/pdf/SKILL.md"),
            "---\nname: pdf\ndescription: pdf skill\n---\nbody",
        )
        .unwrap();
        run(&["add", "-A"]);
        run(&["commit", "--quiet", "-m", "init"]);
        run(&["tag", "v1"]);
        let head = run_git(&["rev-parse", "HEAD"], Some(&repo)).unwrap();
        (repo, head.trim().to_string())
    }

    #[test]
    fn resolve_and_fetch_local_git_repo() {
        let (repo, head) = make_repo("git-repo");
        let url = repo.to_string_lossy().into_owned();

        assert_eq!(resolve_revision(&url, None).unwrap(), head);
        assert_eq!(resolve_revision(&url, Some("v1")).unwrap(), head);
        assert_eq!(resolve_revision(&url, Some(&head)).unwrap(), head);

        let staging = temp_dir("git-staging");
        let fetch = fetch_at_in(&staging, &url, Some("v1"), &head).unwrap();
        assert_eq!(fetch.revision, head);
        assert!(fetch.worktree.join("skills/pdf/SKILL.md").is_file());
        let worktree = fetch.worktree.clone();
        drop(fetch);
        assert!(!worktree.exists(), "Drop 应清理暂存工作树");

        // 不存在的 ref fail-closed
        assert!(resolve_revision(&url, Some("nope")).is_err());
        let _ = fs::remove_dir_all(repo);
        let _ = fs::remove_dir_all(staging);
    }
}
