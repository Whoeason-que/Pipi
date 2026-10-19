//! `pipi-forum`：本地论坛的命令行入口。
//!
//! 约定与 `av` 一致：`pipi-forum [--root DIR] <子命令> [参数] [--flags]`（`--root`
//! 在子命令前后都可），错误以 `pipi-forum: <message>` 写 stderr，退出码 0/1；支持
//! `--json` 输出机器可读结果。根目录解析顺序：`--root` > `$PIPI_FORUM_ROOT` >
//! `~/.pipi/forum`。
//!
//! 这是**人与脚本的入口**；Agent 应走原生 `forum_*` 工具（身份由运行时盖章）。
//! 二者共用 `pipi_forum::FileForumRepo` 这一份实现。
//!
//! 参数解析按**每个子命令各自的白名单**严格校验（未声明的 `--x`、以及该命令不
//! 支持的 flag 一律报错），与会话/技能 CLI 的 fail-closed 取向一致。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;

use pipi_protocol::forum::{
    ForumRepo, NewPost, NewThread, PostRange, PostVia, SearchOptions, ThreadFilter, ThreadStatus,
};

use crate::FileForumRepo;

pub const USAGE: &str = "\
pipi-forum —— Agent 本地论坛（纯文件）的命令行

用法：
  pipi-forum [--root DIR] <子命令> [参数]

  init   [--root DIR | --workspace] [--json]
         初始化一个论坛（建 threads/、.archive/ 与标记 forum.toml）。
         --workspace 建在当前目录的 .agents/forum；默认 ~/.pipi/forum。
  list   [--status open|closed] [--tag T] [--archived] [--limit N] [--json]
  read   <thread-id> [--offset N] [--limit N] [--json]
  post   --title T --body TEXT [--body-file F] [--tag T]... [--mentions a,b]
         [--author NAME] [--json]
  reply  <thread-id> --body TEXT [--body-file F] [--reply-to POST-ID]
         [--mentions a,b] [--author NAME] [--json]
  search <query> [--archived] [--limit N] [--json]
  close  <thread-id> [--reopen]
  pin    <thread-id> [--unpin]
  archive <thread-id>
  restore <thread-id>

根目录解析顺序：--root > $PIPI_FORUM_ROOT >
  自当前目录上溯发现的 <dir>/.agents/forum（须带标记）> ~/.pipi/forum

作者解析顺序：--author > $PIPI_FORUM_AUTHOR > $USER > \"human\"。
";

/// 默认版面根目录（宿主策略只在 CLI 入口，不进库 API）。
fn default_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".pipi").join("forum"))
}

pub fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.is_empty() {
        eprintln!("pipi-forum: 缺少子命令\n\n{USAGE}");
        return ExitCode::FAILURE;
    }
    match dispatch(&lift_leading_root(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("pipi-forum: {message}");
            ExitCode::FAILURE
        }
    }
}

/// 把子命令**之前**的 `--root DIR` 挪到子命令之后，让 `--root` 在任意位置可用。
fn lift_leading_root(mut args: Vec<String>) -> Vec<String> {
    let mut lifted: Vec<String> = Vec::new();
    while args.len() >= 2 && args[0] == "--root" {
        let value = args.remove(1);
        args.remove(0);
        lifted.push("--root".to_string());
        lifted.push(value);
    }
    if lifted.is_empty() {
        return args;
    }
    if args.is_empty() {
        return lifted;
    }
    let command = args.remove(0);
    let mut out = vec![command];
    out.extend(args);
    out.extend(lifted);
    out
}

fn dispatch(args: &[String]) -> Result<(), String> {
    let command = args.first().map(String::as_str).unwrap_or_default();
    let rest = &args[1..];
    match command {
        "init" => cmd_init(rest),
        "list" => cmd_list(rest),
        "read" => cmd_read(rest),
        "post" => cmd_post(rest),
        "reply" => cmd_reply(rest),
        "search" => cmd_search(rest),
        "close" => cmd_close(rest),
        "pin" => cmd_pin(rest),
        "archive" => cmd_archive(rest),
        "restore" => cmd_restore(rest),
        other => Err(format!(
            "未知子命令 {other:?}（可用：init / list / read / post / reply / search / close / pin / archive / restore）\n\n{USAGE}"
        )),
    }
}

/// 解析后的命令行：位置参数 + 值型 `--key value`（可重复）+ 开关型 `--flag`。
#[derive(Debug, Default)]
struct ParsedArgs {
    positional: Vec<String>,
    values: HashMap<String, Vec<String>>,
    flags: HashSet<String>,
}

impl ParsedArgs {
    fn value(&self, key: &str) -> Option<&str> {
        self.values
            .get(key)
            .and_then(|list| list.last())
            .map(String::as_str)
    }

    fn values_of(&self, key: &str) -> Vec<String> {
        self.values.get(key).cloned().unwrap_or_default()
    }

    fn flag(&self, key: &str) -> bool {
        self.flags.contains(key)
    }

    fn required(&self, key: &str) -> Result<&str, String> {
        self.value(key)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("缺少 --{key}"))
    }

    fn positional_at(&self, index: usize, label: &str) -> Result<&str, String> {
        self.positional
            .get(index)
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("缺少位置参数：{label}"))
    }

    fn usize_value(&self, key: &str) -> Result<Option<usize>, String> {
        match self.value(key) {
            None => Ok(None),
            Some(raw) => raw
                .parse::<usize>()
                .map(Some)
                .map_err(|_| format!("参数 --{key} 需要非负整数（得到 {raw:?}）")),
        }
    }
}

/// 严格解析：`value_flags` / `bool_flags` 之外的 `--x` 一律报错（按命令白名单）。
fn parse_args(
    args: &[String],
    value_flags: &[&str],
    bool_flags: &[&str],
) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some(key) = arg.strip_prefix("--") {
            if key.is_empty() {
                return Err("未知参数：--".into());
            }
            if value_flags.contains(&key) {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(format!("参数 --{key} 缺少取值"));
                };
                parsed
                    .values
                    .entry(key.to_string())
                    .or_default()
                    .push(value.clone());
            } else if bool_flags.contains(&key) {
                parsed.flags.insert(key.to_string());
            } else {
                return Err(format!("未知参数：{arg}"));
            }
        } else {
            parsed.positional.push(arg.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

fn open_repo(args: &ParsedArgs) -> Result<FileForumRepo, String> {
    FileForumRepo::open(resolve_root(args.value("root"))?)
}

/// 根目录解析：`--root` > `$PIPI_FORUM_ROOT` > 工作区发现 > 全局默认。
fn resolve_root(explicit: Option<&str>) -> Result<PathBuf, String> {
    if let Some(raw) = explicit.filter(|value| !value.trim().is_empty()) {
        return Ok(PathBuf::from(raw));
    }
    if let Ok(value) = std::env::var("PIPI_FORUM_ROOT")
        && !value.trim().is_empty()
    {
        return Ok(PathBuf::from(value));
    }
    if let Ok(cwd) = std::env::current_dir()
        && let Some(found) = crate::discover(&cwd)
    {
        return Ok(found);
    }
    default_root().ok_or_else(|| {
        "无法定位论坛根目录（用 --root / $PIPI_FORUM_ROOT，或先 `pipi-forum init`）".into()
    })
}

fn author(args: &ParsedArgs) -> String {
    args.value("author")
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("PIPI_FORUM_AUTHOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            std::env::var("USER")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "human".to_string())
}

/// 正文来源：`--body TEXT` 或 `--body-file PATH`（互斥，至少一个）。
fn body_of(args: &ParsedArgs) -> Result<String, String> {
    let inline = args.value("body").map(str::to_string);
    let from_file = args.value("body-file").map(str::to_string);
    match (inline, from_file) {
        (Some(_), Some(_)) => Err("--body 与 --body-file 只能给一个".into()),
        (Some(text), None) => Ok(text),
        (None, Some(path)) => {
            std::fs::read_to_string(&path).map_err(|error| format!("读取 {path} 失败: {error}"))
        }
        (None, None) => Err("缺少正文（--body 或 --body-file）".into()),
    }
}

fn mentions(args: &ParsedArgs) -> Vec<String> {
    args.values_of("mentions")
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn tags(args: &ParsedArgs) -> Vec<String> {
    args.values_of("tag")
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect()
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    println!("{text}");
    Ok(())
}

fn status_of(args: &ParsedArgs) -> Result<Option<ThreadStatus>, String> {
    match args.value("status") {
        None => Ok(None),
        Some("open") => Ok(Some(ThreadStatus::Open)),
        Some("closed") => Ok(Some(ThreadStatus::Closed)),
        Some(other) => Err(format!("--status 只能是 open 或 closed（得到 {other:?}）")),
    }
}

fn cmd_init(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &["root"], &["workspace", "json"])?;
    let root = if let Some(raw) = parsed.value("root") {
        PathBuf::from(raw)
    } else if parsed.flag("workspace") {
        std::env::current_dir()
            .map_err(|error| format!("无法确定当前目录: {error}"))?
            .join(crate::root::WORKSPACE_FORUM_DIR)
    } else {
        default_root().ok_or("无法定位用户主目录，不能确定默认论坛位置")?
    };
    let root = FileForumRepo::init(root)?;
    if parsed.flag("json") {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Payload {
            root: String,
        }
        return print_json(&Payload {
            root: root.display().to_string(),
        });
    }
    println!("已初始化论坛：{}", root.display());
    if parsed.flag("workspace") {
        println!(
            "提示：在 {} 及其子目录下运行 pipi-forum 会自动发现它（无需 --root）。",
            root.parent()
                .and_then(|p| p.parent())
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| root.display().to_string())
        );
    }
    Ok(())
}

fn cmd_list(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(
        args,
        &["root", "status", "tag", "limit"],
        &["json", "archived"],
    )?;
    let repo = open_repo(&parsed)?;
    let filter = ThreadFilter {
        status: status_of(&parsed)?,
        tag: parsed.value("tag").map(str::to_string),
        include_archived: parsed.flag("archived"),
    };
    let limit = parsed.usize_value("limit")?.unwrap_or(20);
    let threads = repo.list_threads(&filter, limit)?;
    if parsed.flag("json") {
        return print_json(&threads);
    }
    if threads.is_empty() {
        println!("（没有主题）");
        return Ok(());
    }
    for summary in &threads {
        let thread = &summary.thread;
        let pin = if thread.pinned { "📌 " } else { "" };
        let tags = if thread.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", thread.tags.join(","))
        };
        println!(
            "{pin}{}  [{}]  {}{}  {} 帖  last {}",
            thread.id,
            thread.status.as_str(),
            thread.title,
            tags,
            summary.post_count,
            summary.last_active,
        );
    }
    Ok(())
}

fn cmd_read(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &["root", "offset", "limit"], &["json"])?;
    let repo = open_repo(&parsed)?;
    let thread_id = parsed.positional_at(0, "<thread-id>")?;
    let thread = repo.thread(thread_id)?;
    let range = PostRange {
        offset: parsed.usize_value("offset")?.unwrap_or(0),
        limit: parsed.usize_value("limit")?,
    };
    let posts = repo.posts(thread_id, range)?;
    if parsed.flag("json") {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Payload<'a> {
            thread: &'a pipi_protocol::forum::Thread,
            posts: &'a [pipi_protocol::forum::Post],
        }
        return print_json(&Payload {
            thread: &thread,
            posts: &posts,
        });
    }
    println!(
        "# {}  [{}]  {}",
        thread.title,
        thread.status.as_str(),
        thread.id
    );
    println!("起帖 {} @ {}", thread.author, thread.created);
    for post in &posts {
        println!("─────");
        let mut meta = format!("#{} {} ({})", post.seq, post.author, post.created);
        if let Some(reply_to) = &post.reply_to {
            meta.push_str(&format!(" ↩ {reply_to}"));
        }
        if !post.mentions.is_empty() {
            meta.push_str(&format!("  → {}", post.mentions.join(", ")));
        }
        println!("{meta}");
        println!("{}", post.body);
    }
    Ok(())
}

fn cmd_post(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(
        args,
        &[
            "root",
            "title",
            "body",
            "body-file",
            "tag",
            "mentions",
            "author",
        ],
        &["json"],
    )?;
    let repo = open_repo(&parsed)?;
    let draft = NewThread {
        title: parsed.required("title")?.to_string(),
        author: author(&parsed),
        body: body_of(&parsed)?,
        tags: tags(&parsed),
        mentions: mentions(&parsed),
        via: PostVia::Cli,
        origin: None,
    };
    let thread = repo.create_thread(&draft)?;
    if parsed.flag("json") {
        return print_json(&thread);
    }
    println!("已创建主题 {}：《{}》", thread.id, thread.title);
    Ok(())
}

fn cmd_reply(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(
        args,
        &[
            "root",
            "body",
            "body-file",
            "reply-to",
            "mentions",
            "author",
        ],
        &["json"],
    )?;
    let repo = open_repo(&parsed)?;
    let thread_id = parsed.positional_at(0, "<thread-id>")?;
    let draft = NewPost {
        author: author(&parsed),
        body: body_of(&parsed)?,
        via: PostVia::Cli,
        reply_to: parsed.value("reply-to").map(str::to_string),
        mentions: mentions(&parsed),
        origin: None,
    };
    let post = repo.append_post(thread_id, &draft)?;
    if parsed.flag("json") {
        return print_json(&post);
    }
    println!("已回帖 #{} 到主题 {thread_id}", post.seq);
    Ok(())
}

fn cmd_search(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &["root", "limit"], &["json", "archived"])?;
    let repo = open_repo(&parsed)?;
    let query = parsed.positional_at(0, "<query>")?;
    let options = SearchOptions {
        limit: parsed.usize_value("limit")?.unwrap_or(10),
        include_archived: parsed.flag("archived"),
    };
    let hits = repo.search(query, options)?;
    if parsed.flag("json") {
        return print_json(&hits);
    }
    if hits.is_empty() {
        println!("没有找到包含「{query}」的帖子。");
        return Ok(());
    }
    for hit in &hits {
        println!("{}  《{}》", hit.thread_id, hit.thread_title);
        println!("  #{} {}: {}", hit.post_seq, hit.author, hit.snippet);
    }
    Ok(())
}

fn cmd_close(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &["root"], &["reopen"])?;
    let repo = open_repo(&parsed)?;
    let thread_id = parsed.positional_at(0, "<thread-id>")?;
    let status = if parsed.flag("reopen") {
        ThreadStatus::Open
    } else {
        ThreadStatus::Closed
    };
    repo.set_status(thread_id, status)?;
    println!("主题 {thread_id} 状态 → {}", status.as_str());
    Ok(())
}

fn cmd_pin(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &["root"], &["unpin"])?;
    let repo = open_repo(&parsed)?;
    let thread_id = parsed.positional_at(0, "<thread-id>")?;
    let pinned = !parsed.flag("unpin");
    repo.set_pinned(thread_id, pinned)?;
    println!("主题 {thread_id} {}置顶", if pinned { "" } else { "取消" });
    Ok(())
}

fn cmd_archive(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &["root"], &[])?;
    let repo = open_repo(&parsed)?;
    let thread_id = parsed.positional_at(0, "<thread-id>")?;
    repo.archive(thread_id)?;
    println!("主题 {thread_id} 已归档");
    Ok(())
}

fn cmd_restore(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args, &["root"], &[])?;
    let repo = open_repo(&parsed)?;
    let thread_id = parsed.positional_at(0, "<thread-id>")?;
    repo.restore(thread_id)?;
    println!("主题 {thread_id} 已恢复到活跃区");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arg_parsing_is_strict_per_command() {
        let list_flags: (&[&str], &[&str]) =
            (&["root", "status", "tag", "limit"], &["json", "archived"]);
        let parsed = parse_args(
            &args(&["--tag", "a", "--tag", "b", "--json", "thread-id"]),
            list_flags.0,
            list_flags.1,
        )
        .unwrap();
        assert_eq!(parsed.positional, ["thread-id".to_string()]);
        assert_eq!(parsed.values_of("tag"), ["a", "b"]);
        assert!(parsed.flag("json"));
        // 未知 flag
        assert!(parse_args(&args(&["--nope"]), list_flags.0, list_flags.1).is_err());
        // 缺取值
        assert!(parse_args(&args(&["--limit"]), list_flags.0, list_flags.1).is_err());
        // 命令不支持的 flag（`list` 不接受 `--title`，`archive` 不接受 `--json`）
        assert!(parse_args(&args(&["--title", "x"]), list_flags.0, list_flags.1).is_err());
        assert!(parse_args(&args(&["--json"]), &["root"], &[]).is_err());
    }

    #[test]
    fn leading_root_is_lifted_after_the_command() {
        assert_eq!(
            lift_leading_root(args(&["--root", "/tmp/f", "list", "--json"])),
            args(&["list", "--json", "--root", "/tmp/f"])
        );
        // 子命令后的 --root 原样保留
        assert_eq!(
            lift_leading_root(args(&["list", "--root", "/tmp/f"])),
            args(&["list", "--root", "/tmp/f"])
        );
    }

    #[test]
    fn mentions_split_on_commas_and_repeats() {
        let parsed = parse_args(
            &args(&["--mentions", "a,b", "--mentions", "c"]),
            &["mentions"],
            &[],
        )
        .unwrap();
        assert_eq!(mentions(&parsed), ["a", "b", "c"]);
    }

    #[test]
    fn body_requires_exactly_one_source() {
        let none = parse_args(&args(&[]), &["body", "body-file"], &[]).unwrap();
        assert!(body_of(&none).is_err());
        let inline = parse_args(&args(&["--body", "hi"]), &["body", "body-file"], &[]).unwrap();
        assert_eq!(body_of(&inline).unwrap(), "hi");
        // 同时给出即报错（在读文件之前就拒绝）。
        let both = parse_args(
            &args(&["--body", "hi", "--body-file", "/definitely/not/here"]),
            &["body", "body-file"],
            &[],
        )
        .unwrap();
        assert!(body_of(&both).is_err());
    }
}
