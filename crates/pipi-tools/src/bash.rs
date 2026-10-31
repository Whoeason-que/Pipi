//! `pipi-tools` 的 bash 工具。移植自 `packages/agent/src/harness/tools/bash.ts` 的核心：
//! 合并 stdout/stderr、tail 截断（2000 行 / 50KB）、超时（默认 900s）；在其上叠加
//! OS 沙箱（`pipi-sandbox` helper），并流式回报部分输出。
//!
//! 输出在内存里只保留按行对齐的尾部，完整内容在首次溢出时转写临时文件；
//! 这样超长输出不会撑爆内存，也不会在 8KB 块边界撕裂多字节 UTF-8。

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, format_size, truncate_tail};
use pipi_protocol::ToolResultContent;
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput, resolve_path};

pub struct BashTool;

/// 超时 / 中止时终止整棵进程树（`killpg` 覆盖孙进程），并回收直接子进程。
async fn terminate_child_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id()
        && let Err(error) = crate::process::kill_process_group(pid)
    {
        eprintln!("pipi: {error}");
    }
    // 非 unix 平台没有进程组；未取到 pid 时也退回直接杀子进程
    let _ = child.start_kill();
}

/// 兜底：工具 future 在跑到收尾之前被 drop（任务取消、运行时关闭）时，
/// 整组仍在跑的孙进程会失去约束，这里补一次组终止。正常路径必须 `disarm()`，
/// 否则已回收的 PID 可能被系统复用，误杀无关进程组。
struct GroupGuard(Option<u32>);

impl GroupGuard {
    fn new(pid: Option<u32>) -> Self {
        Self(pid)
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = crate::process::kill_process_group(pid);
        }
    }
}

/// 没有显式 `timeout` 时的上限：避免 `sleep` / 交互式命令把一轮运行挂死。
/// 900s 覆盖历史会话里最常见的长任务（`timeout 900 cargo test`）；更长的任务
/// 显式传 `timeout`，描述里也提示了「命令里的 `timeout N` 要同步传」。
const DEFAULT_TIMEOUT_SECONDS: u64 = 900;
const MAX_TIMEOUT_SECONDS: f64 = 2_147_483_647.0 / 1000.0;
/// 内存里保留的输出尾部上限。显示窗口是 2000 行 / 50KB，128KB 足够覆盖，
/// 超出部分转写临时文件。
const OUTPUT_TAIL_BYTES: usize = 128 * 1024;

fn parse_timeout_arg(value: &Value) -> Result<Option<f64>, String> {
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_f64()
        .map(Some)
        .ok_or_else(|| "timeout must be a number of seconds".into())
}

fn resolve_timeout(timeout: Option<f64>) -> Result<u64, String> {
    match timeout {
        None => Ok(DEFAULT_TIMEOUT_SECONDS),
        Some(t) => {
            if !t.is_finite() || t <= 0.0 {
                return Err("Invalid timeout: must be a finite number of seconds".into());
            }
            if t > MAX_TIMEOUT_SECONDS {
                return Err(format!(
                    "Invalid timeout: maximum is {MAX_TIMEOUT_SECONDS} seconds"
                ));
            }
            Ok(t.ceil() as u64)
        }
    }
}

/// 流式输出缓冲：内存只保留按行对齐的尾部，完整内容在首次溢出时转写临时
/// 文件。`yes` 或超长构建日志不会撑爆内存，多字节 UTF-8 也不会在块边界被
/// 拆坏。
struct OutputBuffer {
    tail: Vec<u8>,
    newlines: usize,
    total_bytes: usize,
    spill: Option<std::fs::File>,
    spill_path: Option<PathBuf>,
}

impl OutputBuffer {
    fn new() -> Self {
        Self {
            tail: Vec::new(),
            newlines: 0,
            total_bytes: 0,
            spill: None,
            spill_path: None,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        self.total_bytes += chunk.len();
        self.newlines += chunk.iter().filter(|byte| **byte == b'\n').count();
        if let Some(file) = &mut self.spill {
            let _ = file.write_all(chunk);
        }
        self.tail.extend_from_slice(chunk);
        if self.tail.len() > OUTPUT_TAIL_BYTES {
            self.ensure_spill();
            self.trim_tail();
        }
    }

    /// 首次溢出：把「到目前为止的全部内容」写进临时文件，之后的块直接追加。
    fn ensure_spill(&mut self) {
        if self.spill.is_some() {
            return;
        }
        let Ok(mut file) = tempfile::Builder::new()
            .prefix("pipi-bash-")
            .suffix(".txt")
            .tempfile()
        else {
            return;
        };
        if file.write_all(&self.tail).is_err() || file.flush().is_err() {
            return;
        }
        if let Ok((file, path)) = file.keep() {
            self.spill = Some(file);
            self.spill_path = Some(path);
        }
    }

    /// 按行边界裁掉头部；单行自身超过上限时退化为按字节裁
    /// （`truncate_tail` 会标记 `last_line_partial`）。
    fn trim_tail(&mut self) {
        if self.tail.len() <= OUTPUT_TAIL_BYTES {
            return;
        }
        let cut = self.tail.len() - OUTPUT_TAIL_BYTES;
        match self.tail[cut..].iter().position(|byte| *byte == b'\n') {
            Some(offset) => self.tail.drain(..cut + offset + 1),
            None => self.tail.drain(..cut),
        };
    }

    fn snapshot(&self) -> String {
        String::from_utf8_lossy(&self.tail).into_owned()
    }

    /// 全部输出（含已被裁掉的部分）的逻辑行数，与 `truncate::count_lines` 同口径。
    fn total_lines(&self) -> usize {
        if self.total_bytes == 0 {
            0
        } else if self.tail.last() == Some(&b'\n') {
            self.newlines
        } else {
            self.newlines + 1
        }
    }

    fn finish(mut self) -> (String, Option<PathBuf>) {
        if let Some(mut file) = self.spill.take() {
            let _ = file.flush();
        }
        let text = String::from_utf8_lossy(&self.tail).into_owned();
        (text, self.spill_path)
    }
}

/// 展示级截断时把完整输出写进临时文件（内存尾部本身没有溢出）。
fn write_spill(contents: &str) -> Option<PathBuf> {
    let mut file = tempfile::Builder::new()
        .prefix("pipi-bash-")
        .suffix(".txt")
        .tempfile()
        .ok()?;
    file.write_all(contents.as_bytes()).ok()?;
    file.flush().ok()?;
    file.keep().ok().map(|(_file, path)| path)
}

#[async_trait]
impl AgentTool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> String {
        format!(
            "Execute a bash command and return combined stdout and stderr. \
             Runs in the workspace by default; pass cwd to run in a subdirectory. \
             Under the workspace-write sandbox, writes outside the workspace are rejected \
             and network access is off by default; use the write / edit tools for file edits. \
             For long-running work prefer one bounded foreground run with trimmed output, e.g. \
             `cargo test -p x 2>&1 | tail -40`; avoid detaching a job and polling it with sleep. \
             Defaults to a {DEFAULT_TIMEOUT_SECONDS}-second timeout; pass timeout to extend \
             (if the command itself uses `timeout N`, also pass timeout >= N). \
             Output is truncated to the last {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit \
             first); the full output path is included in the truncation notice. \
             For search/list output, pipe through head so the first matches survive truncation.",
            DEFAULT_MAX_BYTES / 1024
        )
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Bash command to execute" },
                "cwd": {
                    "type": "string",
                    "description": "Working directory (relative to the workspace, or absolute). Defaults to the workspace."
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in seconds (default 900; pass a larger value for long builds or backtests)"
                }
            },
            "required": ["command"]
        })
    }

    fn requires_sequential(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &Value,
        on_update: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let command = args["command"].as_str().ok_or("Missing command")?;
        let timeout = resolve_timeout(parse_timeout_arg(&args["timeout"])?)?;
        let timeout_is_default = args["timeout"].is_null();

        tokio::fs::create_dir_all(&ctx.workspace)
            .await
            .map_err(|e| format!("Workspace directory unavailable: {e}"))?;

        // cwd 默认工作目录；相对路径按工作目录解析。是否可写由 OS 沙箱决定，
        // 这里只做「存在且是目录」的 UX 校验（否则 bash 会以难读的方式失败）。
        let cwd = match args["cwd"].as_str().map(str::trim).filter(|value| !value.is_empty()) {
            Some(raw) => {
                let path = resolve_path(&ctx.workspace, raw);
                let metadata = tokio::fs::metadata(&path)
                    .await
                    .map_err(|e| format!("Working directory is not accessible: {raw}. {e}"))?;
                if !metadata.is_dir() {
                    return Err(format!("Working directory is not a directory: {raw}"));
                }
                path
            }
            None => ctx.workspace.clone(),
        };

        let mut builder =
            pipi_sandbox::tokio_command(&ctx.sandbox_policy(), "bash", ["-c", command])?;
        builder
            .current_dir(&cwd)
            // 环境整体来自 av 契约解析结果（会话启动时已定，含继承策略与 AV_* 注入）
            .env_clear()
            .envs(ctx.resolved_env.iter())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // 单独进程组：超时 / 中止时给整组发信号。只杀直接子进程会留下孙进程，
        // 它们继续握着 stdout/stderr 管道，读取任务等不到 EOF（工具表现为卡住）。
        #[cfg(unix)]
        builder.process_group(0);
        let mut child = builder
            .spawn()
            .map_err(|e| format!("Failed to spawn bash: {e}"))?;
        let mut group = GroupGuard::new(child.id());

        let mut stdout = child.stdout.take().expect("stdout piped");
        let mut stderr = child.stderr.take().expect("stderr piped");
        let shared: Arc<Mutex<OutputBuffer>> = Arc::new(Mutex::new(OutputBuffer::new()));

        let shared_out = shared.clone();
        let t_out = tokio::spawn(async move {
            let mut buf = [0u8; 8192];
            loop {
                match stdout.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => shared_out.lock().unwrap().push(&buf[..n]),
                }
            }
        });
        let shared_err = shared.clone();
        let t_err = tokio::spawn(async move {
            let mut buf = [0u8; 8192];
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => shared_err.lock().unwrap().push(&buf[..n]),
                }
            }
        });

        let started = Instant::now();
        let mut timed_out = false;
        let mut aborted = false;
        let mut last_sent = String::new();
        let mut last_sent_at = Instant::now() - Duration::from_secs(1);

        let status = loop {
            // 快照 + 节流回报部分输出
            let snapshot = {
                let buffer = shared.lock().unwrap();
                truncate_tail(&buffer.snapshot(), DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES).content
            };
            if snapshot != last_sent && last_sent_at.elapsed() >= Duration::from_millis(150) {
                last_sent = snapshot.clone();
                last_sent_at = Instant::now();
                on_update(ToolOutput {
                    content: vec![ToolResultContent::Text { text: snapshot }],
                    details: None,
                    terminate: false,
                });
            }

            tokio::select! {
                st = child.wait() => break st,
                // 中止是事件驱动的（AbortSignal 内部是 CancellationToken）：
                // 停止按钮不再等下一次轮询
                _ = ctx.abort.wait_aborted() => {
                    aborted = true;
                    terminate_child_tree(&mut child).await;
                    break child.wait().await;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if started.elapsed() >= Duration::from_secs(timeout) {
                        timed_out = true;
                        terminate_child_tree(&mut child).await;
                        break child.wait().await;
                    }
                }
            }
        };
        // 子进程已回收：解除兜底守卫，避免 PID 复用后误杀无关进程组
        group.disarm();
        let _ = t_out.await;
        let _ = t_err.await;

        // 取出缓冲：内存里只有尾部，完整输出在 spill 文件里（如果溢出过）。
        let buffer = std::mem::replace(&mut *shared.lock().unwrap(), OutputBuffer::new());
        let total_bytes = buffer.total_bytes;
        let total_lines = buffer.total_lines();
        let (full, buffer_spill) = buffer.finish();
        let mut truncation = truncate_tail(&full, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        if total_bytes > full.len() {
            // 内存尾部被裁过：完整输出只在 spill 里，显示的一定是截断后的尾部。
            truncation.truncated = true;
            if truncation.truncated_by.is_none() {
                truncation.truncated_by = Some("bytes");
            }
        }
        truncation.total_bytes = total_bytes;
        truncation.total_lines = total_lines;
        let mut output_text = truncation.content.clone();
        let full_output_path = if truncation.truncated {
            buffer_spill
                .or_else(|| write_spill(&full))
                .map(|path| path.to_string_lossy().into_owned())
        } else {
            None
        };
        if truncation.truncated {
            let total = truncation.total_lines;
            let start_line = total.saturating_sub(truncation.output_lines) + 1;
            let end_line = total;
            let spill_note = full_output_path
                .as_deref()
                .map(|p| format!(" Full output: {p}."))
                .unwrap_or_default();
            if truncation.last_line_partial {
                // 提示里的「line is X」必须是最后一行的字节数，而不是整段输出。
                let last_line_bytes = full.rsplit('\n').next().map(str::len).unwrap_or(0);
                output_text.push_str(&format!(
                    "\n\n[Showing last {} of line {end_line} (line is {}).{spill_note}]",
                    format_size(truncation.output_bytes),
                    format_size(last_line_bytes)
                ));
            } else if truncation.truncated_by == Some("lines") {
                output_text.push_str(&format!(
                    "\n\n[Showing lines {start_line}-{end_line} of {total}.{spill_note}]"
                ));
            } else {
                output_text.push_str(&format!(
                    "\n\n[Showing lines {start_line}-{end_line} of {total} ({} limit).{spill_note}]",
                    format_size(DEFAULT_MAX_BYTES)
                ));
            }
        }

        if timed_out {
            let hint = if timeout_is_default {
                " (default timeout; pass timeout to extend)"
            } else {
                ""
            };
            return Err(if output_text.is_empty() {
                format!("Command timed out after {timeout} seconds{hint}")
            } else {
                format!("{output_text}\n\nCommand timed out after {timeout} seconds{hint}")
            });
        }
        if aborted {
            return Err(if output_text.is_empty() {
                "Command aborted".into()
            } else {
                format!("{output_text}\n\nCommand aborted")
            });
        }
        let status = status.map_err(|e| format!("Failed to wait for command completion: {e}"))?;
        // 信号退出（SIGKILL/OOM/段错误）没有 exit code：旧实现会当成成功返回
        // "(no output)"。按 shell 惯例映射成 128 + signal 并报错。
        #[cfg(unix)]
        let signal = {
            use std::os::unix::process::ExitStatusExt;
            status.signal()
        };
        #[cfg(not(unix))]
        let signal: Option<i32> = None;
        if let Some(signal) = signal {
            return Err(format!(
                "{output_text}\n\nCommand terminated by signal {signal} (exit code {})",
                128 + signal
            ));
        }
        let exit_code = status.code().unwrap_or(0);
        if exit_code != 0 {
            return Err(format!("{output_text}\n\nCommand exited with code {exit_code}"));
        }

        let details = json!({
            "exitCode": exit_code,
            "truncation": truncation.truncated.then(|| json!({
                "truncated": true,
                "truncatedBy": truncation.truncated_by,
            })),
            "fullOutputPath": full_output_path,
        });

        Ok(ToolOutput {
            content: vec![ToolResultContent::Text {
                text: if output_text.is_empty() {
                    "(no output)".to_string()
                } else {
                    output_text
                },
            }],
            details: Some(details),
            terminate: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_resolution_and_validation() {
        assert_eq!(resolve_timeout(None).unwrap(), DEFAULT_TIMEOUT_SECONDS);
        assert_eq!(resolve_timeout(Some(10.0)).unwrap(), 10);
        assert!(resolve_timeout(Some(0.0)).is_err());
        assert!(resolve_timeout(Some(-1.0)).is_err());
        assert!(resolve_timeout(Some(f64::NAN)).is_err());
        assert!(resolve_timeout(Some(MAX_TIMEOUT_SECONDS + 1.0)).is_err());
        // 字符串形式的 timeout 不能被静默当成「没传」（那会退化成默认值）。
        assert!(parse_timeout_arg(&serde_json::json!("30")).is_err());
        assert_eq!(parse_timeout_arg(&serde_json::json!(null)).unwrap(), None);
        assert_eq!(
            parse_timeout_arg(&serde_json::json!(30)).unwrap(),
            Some(30.0)
        );
    }

    #[test]
    fn output_buffer_keeps_a_bounded_tail_and_spills_the_full_output() {
        let mut buffer = OutputBuffer::new();
        for line in 0..30_000u32 {
            buffer.push(format!("line {line}\n").as_bytes());
        }
        assert!(buffer.total_bytes > OUTPUT_TAIL_BYTES);
        assert_eq!(buffer.total_lines(), 30_000);
        assert!(buffer.tail.len() <= OUTPUT_TAIL_BYTES + 16);
        let spill = buffer
            .spill_path
            .clone()
            .expect("overflow must write a spill file");

        let (text, path) = buffer.finish();
        assert_eq!(path.as_deref(), Some(spill.as_path()));
        assert!(text.starts_with("line "), "tail must start on a line boundary");
        assert!(text.ends_with("line 29999\n"));

        let full = std::fs::read_to_string(&spill).unwrap();
        assert!(full.starts_with("line 0\n"));
        assert!(full.ends_with("line 29999\n"));
        assert_eq!(full.lines().count(), 30_000);
        std::fs::remove_file(spill).ok();
    }

    #[test]
    fn output_buffer_decodes_multibyte_chunk_boundaries() {
        let bytes = "中文输出".as_bytes();
        let mut buffer = OutputBuffer::new();
        buffer.push(&bytes[..1]);
        buffer.push(&bytes[1..4]);
        buffer.push(&bytes[4..]);
        assert_eq!(buffer.snapshot(), "中文输出");
        assert_eq!(buffer.total_lines(), 1);
    }

    /// av 契约接线：子进程环境完全来自 resolved_env（env_clear + envs）——
    /// 声明外的变量（含 PATH）不可见。
    #[tokio::test]
    async fn bash_spawns_with_resolved_env_only() {
        let workspace =
            std::env::temp_dir().join(format!("pipi-bash-env-{}", pipi_test_support::new_id()));
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        let resolved =
            std::collections::BTreeMap::from([("AV_MARKER".to_string(), "from-av".to_string())]);
        let ctx = ToolContext {
            workspace,
            memory_dir: None,
            notebook_path: None,
            read_roots: Vec::new(),
            permissions: std::sync::Arc::new(
                pipi_protocol::permissions::PermissionsConfig::default(),
            ),
            sandbox: pipi_protocol::permissions::SandboxMode::DangerFullAccess,
            resolved_env: std::sync::Arc::new(resolved),
            abort: pipi_protocol::AbortSignal::new(),
        };
        let output = BashTool
            .execute(
                &ctx,
                // bash 在 PATH 缺失时会自行合成默认 PATH，故用自定义变量名验证隔离
                &serde_json::json!({ "command": "echo \"$AV_MARKER:${PIPI_AV_TEST_LEAK:-absent}\"" }),
                &|_| {},
            )
            .await
            .unwrap();
        let text = match &output.content[0] {
            ToolResultContent::Text { text } => text.clone(),
            _ => panic!("expected text output"),
        };
        assert_eq!(text.trim(), "from-av:absent");
    }

    /// 建一个最小 ToolContext（bash 用）。
    fn test_context(
        workspace: std::path::PathBuf,
        abort: pipi_protocol::AbortSignal,
    ) -> ToolContext {
        ToolContext {
            workspace,
            memory_dir: None,
            notebook_path: None,
            read_roots: Vec::new(),
            permissions: std::sync::Arc::new(
                pipi_protocol::permissions::PermissionsConfig::default(),
            ),
            sandbox: pipi_protocol::permissions::SandboxMode::DangerFullAccess,
            resolved_env: std::sync::Arc::new(std::collections::BTreeMap::new()),
            abort,
        }
    }

    fn test_workspace(tag: &str) -> std::path::PathBuf {
        let workspace =
            std::env::temp_dir().join(format!("pipi-bash-{tag}-{}", pipi_test_support::new_id()));
        std::fs::create_dir_all(&workspace).unwrap();
        workspace
    }

    /// 进程是否已经终止（消失，或在等 init 回收的僵尸态）。
    #[cfg(unix)]
    fn process_terminated(pid: i32) -> bool {
        #[cfg(target_os = "linux")]
        {
            match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                // `pid (comm) state ...`：comm 里可能有 ')'，取最后一个 ')' 之后的部分
                Ok(stat) => stat
                    .rsplit(')')
                    .next()
                    .map(|rest| rest.trim_start().starts_with('Z'))
                    .unwrap_or(false),
                Err(_) => true,
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            use nix::sys::signal::kill;
            use nix::unistd::Pid;
            kill(Pid::from_raw(pid), None).is_err()
        }
    }

    #[cfg(unix)]
    async fn assert_process_terminated(pid: i32) {
        for _ in 0..150 {
            if process_terminated(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("孙进程 {pid} 仍在运行：进程组没有被完整终止");
    }

    fn read_pid_file(workspace: &std::path::Path) -> i32 {
        let text = std::fs::read_to_string(workspace.join("bg.pid")).expect("后台进程 pid 文件");
        text.trim().parse().expect("pid")
    }

    fn output_text(output: &ToolOutput) -> String {
        match &output.content[0] {
            ToolResultContent::Text { text } => text.clone(),
            _ => panic!("expected text output"),
        }
    }

    /// `cwd` 让命令在子目录里跑；默认仍是 workspace。
    #[tokio::test]
    async fn cwd_runs_in_the_requested_directory() {
        let workspace = test_workspace("cwd");
        let nested = workspace.join("nested");
        tokio::fs::create_dir_all(&nested).await.unwrap();
        let ctx = test_context(workspace.clone(), pipi_protocol::AbortSignal::new());

        let output = BashTool
            .execute(
                &ctx,
                &serde_json::json!({ "command": "pwd", "cwd": "nested" }),
                &|_| {},
            )
            .await
            .unwrap();
        let expected = std::fs::canonicalize(&nested).unwrap();
        assert_eq!(output_text(&output).trim(), expected.to_string_lossy());

        std::fs::remove_dir_all(&workspace).ok();
    }

    #[tokio::test]
    async fn cwd_must_exist_and_be_a_directory() {
        let workspace = test_workspace("cwd-invalid");
        let ctx = test_context(workspace.clone(), pipi_protocol::AbortSignal::new());

        let missing = BashTool
            .execute(
                &ctx,
                &serde_json::json!({ "command": "pwd", "cwd": "missing" }),
                &|_| {},
            )
            .await;
        assert!(missing.is_err());

        let file = workspace.join("file.txt");
        tokio::fs::write(&file, "x").await.unwrap();
        let not_dir = BashTool
            .execute(
                &ctx,
                &serde_json::json!({ "command": "pwd", "cwd": "file.txt" }),
                &|_| {},
            )
            .await;
        assert!(not_dir.is_err());

        std::fs::remove_dir_all(&workspace).ok();
    }

    /// 被信号杀掉的命令没有 exit code：必须报错，不能当成 "(no output)" 成功。
    #[cfg(unix)]
    #[tokio::test]
    async fn signal_death_is_reported_as_an_error() {
        let workspace = test_workspace("signal");
        let ctx = test_context(workspace.clone(), pipi_protocol::AbortSignal::new());

        let result = BashTool
            .execute(&ctx, &serde_json::json!({ "command": "kill -9 $$" }), &|_| {})
            .await;
        let error = result.expect_err("signal death must not be reported as success");
        assert!(error.contains("terminated by signal 9"), "{error}");
        assert!(error.contains("137"), "{error}");

        std::fs::remove_dir_all(&workspace).ok();
    }

    /// 超长输出：内存里只留尾部，完整内容进 spill 文件；提示里带路径与真实总行数。
    #[tokio::test]
    async fn long_output_is_truncated_and_spilled() {
        let workspace = test_workspace("spill");
        let ctx = test_context(workspace.clone(), pipi_protocol::AbortSignal::new());

        let output = BashTool
            .execute(&ctx, &serde_json::json!({ "command": "seq 1 40000" }), &|_| {})
            .await
            .unwrap();
        let text = output_text(&output);
        assert!(
            text.contains("Showing lines 38001-40000 of 40000"),
            "unexpected notice: {text}"
        );
        let details = output.details.as_ref().expect("details");
        assert_eq!(details["exitCode"], 0);
        assert_eq!(details["truncation"]["truncated"], true);
        let spill = details["fullOutputPath"]
            .as_str()
            .expect("spill path in details");
        let full = std::fs::read_to_string(spill).unwrap();
        assert!(full.starts_with("1\n"), "spill must contain the beginning");
        assert!(full.ends_with("40000\n"));
        assert_eq!(full.lines().count(), 40_000);

        std::fs::remove_file(spill).ok();
        std::fs::remove_dir_all(&workspace).ok();
    }

    /// 超时不能只杀直接子进程：后台孙进程继续握着 stdout/stderr 时，
    /// 读取任务等不到 EOF（工具卡死），孙进程还留在系统里。
    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_terminates_the_whole_process_tree() {
        let workspace = test_workspace("timeout-tree");
        let ctx = test_context(workspace.clone(), pipi_protocol::AbortSignal::new());
        let result = BashTool
            .execute(
                &ctx,
                &serde_json::json!({
                    "command": "sleep 30 & echo $! > bg.pid; wait",
                    "timeout": 1
                }),
                &|_| {},
            )
            .await;
        let error = result.expect_err("超时应报错");
        assert!(error.contains("timed out"), "错误信息应说明超时: {error}");
        assert_process_terminated(read_pid_file(&workspace)).await;
        std::fs::remove_dir_all(&workspace).ok();
    }

    /// 中止（停止按钮）同样终止整棵树，并且立刻返回 ——
    /// 旧实现要等下一次 100ms 轮询，且会被孙进程拖住。
    #[cfg(unix)]
    #[tokio::test]
    async fn abort_terminates_the_whole_process_tree() {
        let workspace = test_workspace("abort-tree");
        let abort = pipi_protocol::AbortSignal::new();
        let task = tokio::spawn({
            let workspace = workspace.clone();
            let abort = abort.clone();
            async move {
                let ctx = test_context(workspace, abort);
                BashTool
                    .execute(
                        &ctx,
                        &serde_json::json!({ "command": "sleep 30 & echo $! > bg.pid; wait" }),
                        &|_| {},
                    )
                    .await
            }
        });
        // 等命令真正跑起来（pid 文件是 shell 写的）
        tokio::time::sleep(Duration::from_millis(400)).await;
        let started = Instant::now();
        abort.abort();
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("中止后应立即返回，不能被孙进程拖住")
            .expect("任务不应 panic");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "中止收尾不应耗时过久"
        );
        let error = result.expect_err("中止应报错");
        assert!(error.contains("aborted"), "错误信息应说明中止: {error}");
        assert_process_terminated(read_pid_file(&workspace)).await;
        std::fs::remove_dir_all(&workspace).ok();
    }
}
