//! git 进程调用：本 crate 里唯一 spawn git 的地方。
//!
//! 影子仓库是 bare 仓库，全部操作显式走 `--git-dir <repo>`，与调用方的 cwd
//! 和进程环境（`GIT_DIR` / `GIT_WORK_TREE` 等）无关；`GIT_TERMINAL_PROMPT=0`
//! 保证不会挂在凭据输入上。仓库级互斥锁也在这里：写者与 gc 共用一个锁，
//! 满足「gc 与写者串行」（见 `docs/git-session-store.md` §5）。
//!
//! 进程调用是同步阻塞的（与 `av` 的 git 调用同一口径）：调用点负责别在
//! async 上下文里直接调用 —— 运行时的提交点在阻塞线程池上跑，见
//! `pipi-agent` 的 `session::commit`。

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use pipi_protocol::now_millis;

/// 两次 `gc --auto` 之间的最小间隔（毫秒）。
const GC_INTERVAL_MILLIS: u64 = 60_000;
/// 错误信息里保留的 stderr 尾部字节数（与 av 的 `run_git` 同一口径）。
const STDERR_TAIL_BYTES: usize = 500;

/// 一个 Agent 的影子仓库：路径 + 串行化锁 + gc 节流状态。
pub struct GitExec {
    repo_dir: PathBuf,
    /// 仓库级互斥：写者（fast-import）与 gc 都经过它。
    lock: Mutex<()>,
    /// 上次尝试 gc 的毫秒时间戳（0 = 从未）。
    last_gc: AtomicU64,
}

impl GitExec {
    pub fn new(repo_dir: PathBuf) -> Self {
        Self {
            repo_dir,
            lock: Mutex::new(()),
            last_gc: AtomicU64::new(0),
        }
    }

    /// 带 `--git-dir` 的基础命令：与 cwd、`GIT_DIR` 等环境变量解耦。
    fn command(&self) -> Command {
        let mut command = Command::new("git");
        command
            .arg("--git-dir")
            .arg(&self.repo_dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_LFS_SKIP_SMUDGE", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
            .stdin(Stdio::null());
        command
    }

    /// 取仓库锁并保证仓库已初始化，然后执行 `f`（`f` 内不得再次取锁）。
    fn with_lock<T>(&self, f: impl FnOnce(&Self) -> Result<T, String>) -> Result<T, String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "Session git repository lock poisoned".to_string())?;
        self.ensure_initialized()?;
        f(self)
    }

    /// 在仓库里跑一条 git 命令，返回 stdout 原始字节。
    pub fn run(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        self.with_lock(|exec| exec.run_raw(args))
    }

    /// 确保仓库可用：幂等初始化（缺 git 或仓库目录被换成符号链接都当场报错）。
    pub fn ensure_ready(&self) -> Result<(), String> {
        self.with_lock(|_| Ok(()))
    }

    /// 同 [`GitExec::run`]，但从 stdin 喂入输入（fast-import / cat-file --batch）。
    pub fn run_with_stdin(&self, args: &[&str], input: Vec<u8>) -> Result<Vec<u8>, String> {
        self.with_lock(|exec| exec.run_raw_with_stdin(args, input))
    }

    fn run_raw(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let output = self
            .command()
            .args(args)
            .output()
            .map_err(|error| format!("无法执行 git（需要系统 git）: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git {} 失败: {}",
                args.join(" "),
                tail(&String::from_utf8_lossy(&output.stderr))
            ));
        }
        Ok(output.stdout)
    }

    fn run_raw_with_stdin(&self, args: &[&str], input: Vec<u8>) -> Result<Vec<u8>, String> {
        let mut child = self
            .command()
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("无法执行 git（需要系统 git）: {error}"))?;
        let mut stdin = child.stdin.take().expect("stdin 已管道化");
        // 输入可能大于管道缓冲，必须另开线程写，否则与子进程的 stdout 互等。
        let writer = std::thread::spawn(move || stdin.write_all(&input));
        let output = child
            .wait_with_output()
            .map_err(|error| format!("等待 git 退出失败: {error}"))?;
        let write_result = writer
            .join()
            .map_err(|_| "git stdin 写入线程崩溃".to_string())?;
        if !output.status.success() {
            return Err(format!(
                "git {} 失败: {}",
                args.join(" "),
                tail(&String::from_utf8_lossy(&output.stderr))
            ));
        }
        write_result.map_err(|error| format!("写入 git stdin 失败: {error}"))?;
        Ok(output.stdout)
    }

    /// 解析 ref 指向的提交；ref 不存在返回 `None`。
    pub fn resolve_ref(&self, ref_name: &str) -> Result<Option<String>, String> {
        self.with_lock(|exec| exec.resolve_ref_raw(ref_name))
    }

    /// [`GitExec::resolve_ref`] 的不取锁版本（调用点必须已经持锁）。
    fn resolve_ref_raw(&self, ref_name: &str) -> Result<Option<String>, String> {
        let output = self
            .command()
            .args(["rev-parse", "--verify", "--quiet", ref_name])
            .output()
            .map_err(|error| format!("Cannot execute git (system git is required): {error}"))?;
        match output.status.code() {
            Some(0) => Ok(Some(
                String::from_utf8_lossy(&output.stdout).trim().to_string(),
            )),
            // --quiet：不存在时退出码 1、无输出
            Some(1) => Ok(None),
            _ => Err(format!(
                "git rev-parse {ref_name} failed: {}",
                tail(&String::from_utf8_lossy(&output.stderr))
            )),
        }
    }

    pub fn ref_exists(&self, ref_name: &str) -> Result<bool, String> {
        Ok(self.resolve_ref(ref_name)?.is_some())
    }

    /// 一次 `fast-import` 写入若干 blob 与一个 commit（流以 `done` 收尾）。
    pub fn fast_import(&self, stream: Vec<u8>) -> Result<(), String> {
        self.run_with_stdin(&["fast-import", "--quiet", "--done"], stream)
            .map(|_| ())
    }

    /// 提交之后的节流 gc（`gc --auto` 无事可做时只有毫秒级开销）。
    ///
    /// 自己取仓库锁（调用点必须已经放掉锁）；失败忽略 —— gc 是维护动作，
    /// 不该让一轮对话失败。
    pub fn gc_auto(&self) {
        if !self.gc_due() {
            return;
        }
        let Ok(_guard) = self.lock.lock() else {
            return;
        };
        if !self.gc_due() {
            return;
        }
        self.last_gc.store(now_millis(), Ordering::Relaxed);
        let _ = self.run_raw(&["gc", "--auto", "--quiet"]);
    }

    /// 删除会话后立即回收独有对象（`gc --prune=now`，实测 1 ms）。
    pub fn gc_prune_now(&self) -> Result<(), String> {
        self.run(&["gc", "--prune=now", "--quiet"]).map(|_| ())
    }

    fn gc_due(&self) -> bool {
        let last = self.last_gc.load(Ordering::Relaxed);
        last == 0 || now_millis().saturating_sub(last) >= GC_INTERVAL_MILLIS
    }

    /// 幂等初始化：bare 仓库 + `main` 锚提交。
    ///
    /// 锚提交让裸仓库的 `git log` 可用（否则 HEAD 指向的 main 没有提交）。
    /// 顺带显式关掉 reflog：裸仓库默认不写，显式写死是为了防用户全局配置
    /// 打开它 —— 删除路径的简化（删 ref + `gc --prune=now`，不做 reflog
    /// expire）依赖这条。
    fn ensure_initialized(&self) -> Result<(), String> {
        // 快路径：HEAD 在就是已初始化（免一次 spawn）
        if self.repo_dir.join("HEAD").is_file() {
            return Ok(());
        }
        if let Some(parent) = self.repo_dir.parent() {
            pipi_protocol::fs::ensure_real_directory(parent, "Agent directory")?;
        }
        if !pipi_protocol::fs::ensure_real_directory(&self.repo_dir, "Session git repository")? {
            let output = Command::new("git")
                .args(["init", "--bare", "--quiet", "--initial-branch=main"])
                .arg(&self.repo_dir)
                .env("GIT_TERMINAL_PROMPT", "0")
                .stdin(Stdio::null())
                .output()
                .map_err(|error| format!("Cannot execute git (system git is required): {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "git init failed: {}",
                    tail(&String::from_utf8_lossy(&output.stderr))
                ));
            }
        }
        self.run_raw(&["config", "core.logAllRefUpdates", "false"])?;
        if self.resolve_ref_raw("refs/heads/main")?.is_none() {
            self.run_raw_with_stdin(
                &["fast-import", "--quiet", "--done"],
                anchor_stream(now_millis() / 1000),
            )?;
        }
        Ok(())
    }
}

/// `main` 锚提交：一条空提交，只为了让裸仓库的 `git log` 有东西可读。
fn anchor_stream(timestamp_secs: u64) -> Vec<u8> {
    let message = "Pipi 会话仓库\n";
    let mut stream = String::new();
    stream.push_str("commit refs/heads/main\n");
    stream.push_str(&format!(
        "committer Pipi <pipi@local> {timestamp_secs} +0000\n"
    ));
    stream.push_str(&format!("data {}\n", message.len()));
    stream.push_str(message);
    stream.push_str("done\n");
    stream.into_bytes()
}

/// 错误信息里的 stderr 尾部（trim 后截尾 500 字节）。
fn tail(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.len() <= STDERR_TAIL_BYTES {
        return trimmed.to_string();
    }
    let mut cut = trimmed.len() - STDERR_TAIL_BYTES;
    while !trimmed.is_char_boundary(cut) {
        cut += 1;
    }
    trimmed[cut..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_end_and_respects_char_boundaries() {
        assert_eq!(tail("  boom  "), "boom");
        let long = "字".repeat(400);
        let cut = tail(&long);
        assert!(cut.len() <= STDERR_TAIL_BYTES);
        assert!(long.ends_with(&cut));
    }

    #[test]
    fn ensure_initialized_is_idempotent() {
        let dir = pipi_test_support::temp_dir("pipi-git-exec");
        let exec = GitExec::new(dir.join("sessions.git"));
        // 任何操作都会顺带初始化（幂等），锚提交让裸仓库的 git log 可用
        let anchor = exec.resolve_ref("refs/heads/main").unwrap();
        assert!(anchor.is_some());
        exec.ensure_initialized().unwrap();
        assert_eq!(exec.resolve_ref("refs/heads/main").unwrap(), anchor);
        // 显式初始化同样幂等
        exec.ensure_ready().unwrap();
        assert_eq!(exec.resolve_ref("refs/heads/main").unwrap(), anchor);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn symlinked_repo_dir_is_rejected() {
        let outside = pipi_test_support::temp_dir("pipi-git-outside");
        let parent = pipi_test_support::temp_dir("pipi-git-parent");
        let link = parent.join("sessions.git");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let exec = GitExec::new(link);
        let error = exec.resolve_ref("refs/heads/main").unwrap_err();
        assert!(
            error.contains("cannot be a symbolic link") || error.contains("不能是符号链接"),
            "{error}"
        );
        std::fs::remove_dir_all(&parent).ok();
        std::fs::remove_dir_all(&outside).ok();
    }
}
