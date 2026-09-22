//! `pipi-tools` 的 bash 工具。移植自 `packages/agent/src/harness/tools/bash.ts` 的核心：
//! 合并 stdout/stderr、tail 截断（2000 行 / 50KB）、可选超时；在其上叠加
//! Pipi 的命令权限检查（allowlist / denylist），并流式回报部分输出。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::{AgentTool, ToolContext, ToolOutput};
use crate::truncate::{format_size, truncate_tail, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
use crate::types::ToolResultContent;

pub struct BashTool;

const MAX_TIMEOUT_SECONDS: f64 = 2_147_483_647.0 / 1000.0;

fn validate_timeout(timeout: Option<f64>) -> Result<Option<u64>, String> {
    match timeout {
        None => Ok(None),
        Some(t) => {
            if !t.is_finite() || t <= 0.0 {
                return Err("Invalid timeout: must be a finite number of seconds".into());
            }
            if t > MAX_TIMEOUT_SECONDS {
                return Err(format!(
                    "Invalid timeout: maximum is {MAX_TIMEOUT_SECONDS} seconds"
                ));
            }
            Ok(Some(t.ceil() as u64))
        }
    }
}

#[async_trait]
impl AgentTool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> String {
        format!(
            "Execute a bash command in the current working directory. Returns combined stdout and stderr. \
             In workspace-write mode, write redirections (`> file`, `>> file`, `2> file`, `&> file`) are rejected — \
             use the write / edit tools for file writes — and file-input redirections (`< file`, `<(cmd)`) are rejected; \
             discarding or merging output (`2>/dev/null`, `2>&1`), pipes and heredocs (`<<EOF`) are allowed. \
             For long-running work prefer one bounded foreground run with trimmed output, e.g. \
             `timeout 900 cargo test -p x 2>&1 | tail -40`; avoid detaching a job and polling it with sleep. \
             Output is truncated to last {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). \
             Optionally provide a timeout in seconds.",
            DEFAULT_MAX_BYTES / 1024
        )
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Bash command to execute" },
                "timeout": { "type": "number", "description": "Timeout in seconds (optional, no default timeout)" }
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
        let command = args["command"].as_str().ok_or("缺少 command")?;
        let timeout = validate_timeout(args["timeout"].as_f64())?;

        // Pipi：命令权限检查（切分 → 白/黑名单 → 危险命令 → 沙箱重定向）。
        // 只有 Allowlist 未命中的非危险命令可以走交互审批救回；其余硬拒。
        match ctx.permissions.assess_bash_classified(command, &ctx.workspace) {
            crate::permissions::BashAssessment::Allowed => {}
            crate::permissions::BashAssessment::HardDenied(message) => return Err(message),
            crate::permissions::BashAssessment::NeedsApproval { .. } => match &ctx.approver {
                Some(approver) => approver.approve(command).await?,
                None => {
                    return Err(
                        "命令不在白名单中，且当前会话没有可用的交互审批通道（请在 Agent 的命令权限中加入该命令，或调整沙箱设置）"
                            .into(),
                    )
                }
            },
        }

        tokio::fs::create_dir_all(&ctx.workspace)
            .await
            .map_err(|e| format!("工作目录不可用: {e}"))?;

        let mut child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&ctx.workspace)
            // 环境整体来自 av 契约解析结果（会话启动时已定，含继承策略与 AV_* 注入）
            .env_clear()
            .envs(ctx.resolved_env.iter())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("无法启动 bash: {e}"))?;

        let mut stdout = child.stdout.take().expect("stdout piped");
        let mut stderr = child.stderr.take().expect("stderr piped");
        let shared: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

        let shared_out = shared.clone();
        let t_out = tokio::spawn(async move {
            let mut buf = [0u8; 8192];
            loop {
                match stdout.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        shared_out
                            .lock()
                            .unwrap()
                            .push_str(&String::from_utf8_lossy(&buf[..n]));
                    }
                }
            }
        });
        let shared_err = shared.clone();
        let t_err = tokio::spawn(async move {
            let mut buf = [0u8; 8192];
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        shared_err
                            .lock()
                            .unwrap()
                            .push_str(&String::from_utf8_lossy(&buf[..n]));
                    }
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
                let s = shared.lock().unwrap();
                truncate_tail(&s, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES).content
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
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if let Some(t) = timeout {
                        if started.elapsed() >= Duration::from_secs(t) {
                            timed_out = true;
                            let _ = child.kill().await;
                            break child.wait().await;
                        }
                    }
                    if ctx.abort.is_aborted() {
                        aborted = true;
                        let _ = child.kill().await;
                        break child.wait().await;
                    }
                }
            }
        };
        let _ = t_out.await;
        let _ = t_err.await;

        let full = shared.lock().unwrap().clone();
        let truncation = truncate_tail(&full, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        let mut output_text = truncation.content.clone();
        let mut full_output_path: Option<String> = None;
        if truncation.truncated {
            // pi 的 spill 行为：完整输出写入临时文件，路径附在提示里
            let spill =
                std::env::temp_dir().join(format!("pipi-bash-{}.txt", crate::session::new_id()));
            if tokio::fs::write(&spill, &full).await.is_ok() {
                full_output_path = Some(spill.to_string_lossy().into_owned());
            }
            let total = truncation.total_lines;
            let start_line = total - truncation.output_lines + 1;
            let end_line = total;
            let spill_note = full_output_path
                .as_deref()
                .map(|p| format!(" Full output: {p}."))
                .unwrap_or_default();
            if truncation.last_line_partial {
                output_text.push_str(&format!(
                    "\n\n[Showing last {} of line {end_line} (line is {}).{spill_note}]",
                    format_size(truncation.output_bytes),
                    format_size(truncation.total_bytes)
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
        let details = if truncation.truncated {
            Some(json!({
                "truncation": { "truncated": true, "truncatedBy": truncation.truncated_by },
                "fullOutputPath": full_output_path,
            }))
        } else {
            None
        };

        if timed_out {
            return Err(match timeout {
                Some(t) => format!("{output_text}\n\nCommand timed out after {t} seconds"),
                None => "Command timed out".into(),
            });
        }
        if aborted {
            return Err(if output_text.is_empty() {
                "Command aborted".into()
            } else {
                format!("{output_text}\n\nCommand aborted")
            });
        }
        let status = status.map_err(|e| format!("等待命令结束失败: {e}"))?;
        if let Some(code) = status.code() {
            if code != 0 {
                return Err(format!("{output_text}\n\nCommand exited with code {code}"));
            }
        }

        Ok(ToolOutput {
            content: vec![ToolResultContent::Text {
                text: if output_text.is_empty() {
                    "(no output)".to_string()
                } else {
                    output_text
                },
            }],
            details,
            terminate: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_validation() {
        assert!(validate_timeout(None).unwrap().is_none());
        assert!(validate_timeout(Some(10.0)).unwrap() == Some(10));
        assert!(validate_timeout(Some(0.0)).is_err());
        assert!(validate_timeout(Some(-1.0)).is_err());
        assert!(validate_timeout(Some(f64::NAN)).is_err());
        assert!(validate_timeout(Some(MAX_TIMEOUT_SECONDS + 1.0)).is_err());
    }

    /// av 契约接线：子进程环境完全来自 resolved_env（env_clear + envs）——
    /// 声明外的变量（含 PATH）不可见。
    #[tokio::test]
    async fn bash_spawns_with_resolved_env_only() {
        let workspace =
            std::env::temp_dir().join(format!("pipi-bash-env-{}", crate::session::new_id()));
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        let resolved =
            std::collections::BTreeMap::from([("AV_MARKER".to_string(), "from-av".to_string())]);
        let ctx = ToolContext {
            workspace,
            memory_dir: None,
            read_roots: Vec::new(),
            permissions: std::sync::Arc::new(crate::permissions::PermissionsConfig::default()),
            sandbox: crate::permissions::SandboxMode::DangerFullAccess,
            resolved_env: std::sync::Arc::new(resolved),
            abort: crate::types::AbortSignal::new(),
            approver: None,
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
}
