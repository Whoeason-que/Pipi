//! Workspace search for plan mode. Runs a fixed system ripgrep invocation
//! without a shell, configuration, user environment, or writable sandbox.
use async_trait::async_trait;
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::process::GroupGuard;

const MAX_OUTPUT: u64 = crate::truncate::DEFAULT_MAX_BYTES as u64;

pub struct SearchWorkspaceTool;

#[async_trait]
impl AgentTool for SearchWorkspaceTool {
    fn name(&self) -> &str {
        "search_workspace"
    }
    fn description(&self) -> String {
        format!(
            "Search workspace text with ripgrep, respecting ignore files. Results are capped at {} KiB.",
            MAX_OUTPUT / 1024
        )
    }
    fn parameters(&self) -> Value {
        json!({"type":"object", "additionalProperties":false, "properties":{"pattern":{"type":"string","description":"Regex pattern to search for."}}, "required":["pattern"]})
    }
    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &Value,
        _: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let pattern = args["pattern"].as_str().ok_or("Missing search pattern")?;
        if pattern.is_empty() || pattern.len() > 4096 {
            return Err("Search pattern must contain 1 to 4096 bytes".into());
        }
        let rg = ["/usr/bin/rg", "/usr/local/bin/rg", "/opt/homebrew/bin/rg"]
            .into_iter()
            .find(|path| std::path::Path::new(path).is_file())
            .ok_or("Workspace search requires ripgrep in a standard system location")?;
        let mut policy = pipi_protocol::permissions::SandboxPolicy::for_workspace(
            pipi_protocol::permissions::SandboxMode::ReadOnly,
            &ctx.workspace,
        );
        policy.allow_env_overrides = false;
        let env = std::collections::BTreeMap::new();
        let mut command = pipi_sandbox::tokio_agent_command(
            &policy,
            rg,
            [
                "--no-config",
                "--hidden",
                "--glob",
                "!.git/**",
                "--line-number",
                "--no-heading",
                "--color=never",
                "--max-columns=1000",
                "--max-columns-preview",
                "--",
                pattern,
                ".",
            ],
            &env,
        )?;
        command
            .current_dir(&ctx.workspace)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|e| format!("Cannot start workspace search: {e}"))?;
        let pid = child.id();
        let mut group = GroupGuard::new(pid);
        let mut output = Vec::new();
        let mut stdout = child
            .stdout
            .take()
            .ok_or("Workspace search stdout is unavailable")?
            .take(MAX_OUTPUT + 1);
        let result = tokio::select! {
            _ = ctx.abort.wait_aborted() => Err("Workspace search aborted".to_string()),
            result = tokio::time::timeout(std::time::Duration::from_secs(15), stdout.read_to_end(&mut output)) => match result {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(error)) => Err(format!("Cannot read workspace search output: {error}")),
                Err(_) => Err("Workspace search timed out".to_string()),
            }
        };
        let truncated = output.len() as u64 > MAX_OUTPUT;
        if result.is_err() || truncated {
            if let Some(pid) = pid {
                let _ = crate::process::kill_process_group(pid);
            }
            let _ = child.start_kill();
        }
        let status = tokio::select! {
            result = child.wait() => result.map_err(|e| format!("Cannot reap workspace search: {e}")),
            _ = ctx.abort.wait_aborted() => Err("Workspace search aborted".into()),
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => Err("Workspace search did not exit".into()),
        };
        if status.is_err() {
            if let Some(pid) = pid {
                let _ = crate::process::kill_process_group(pid);
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        // 成功与失败路径都已 wait() 回收子进程；此后守卫必须解除，避免 PID
        // 复用后误杀无关进程组。
        group.disarm();
        let status = status?;
        result?;
        if !truncated && !status.success() && status.code() != Some(1) {
            return Err(format!("Workspace search failed: {status}"));
        }
        output.truncate(MAX_OUTPUT as usize);
        let mut text = String::from_utf8_lossy(&output).into_owned();
        if truncated {
            text.push_str(&format!(
                "\n[Results truncated at {} KiB]",
                MAX_OUTPUT / 1024
            ));
        }
        if text.is_empty() {
            text.push_str("No matches found.");
        }
        Ok(ToolOutput::text(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::{AbortSignal, ToolResultContent};
    use std::sync::Arc;

    #[tokio::test]
    async fn search_respects_ignores_finds_hidden_files_and_does_not_interpret_flags() {
        let workspace = pipi_test_support::temp_dir("plan-search");
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&workspace)
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(workspace.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(workspace.join(".hidden"), "match --help\n").unwrap();
        std::fs::write(workspace.join("ignored.txt"), "match --help\n").unwrap();
        let ctx = ToolContext {
            workspace: workspace.clone(),
            memory_dir: None,
            notebook_path: None,
            read_roots: vec![],
            permissions: Arc::new(Default::default()),
            sandbox: pipi_protocol::permissions::SandboxMode::ReadOnly,
            sandbox_policy_override: None,
            resolved_env: Arc::new(Default::default()),
            abort: AbortSignal::new(),
        };
        let result = SearchWorkspaceTool
            .execute(&ctx, &json!({"pattern":"--help"}), &|_| {})
            .await;
        match result {
            Ok(output) => {
                let text = output
                    .content
                    .iter()
                    .find_map(|part| match part {
                        ToolResultContent::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .unwrap();
                assert!(text.contains(".hidden") && text.contains("--help"));
                assert!(!text.contains("ignored.txt"));
            }
            Err(error) if error.contains("helper") || error.contains("ripgrep") => {
                // Environments without the helper or system ripgrep must fail closed.
                assert!(!workspace.join("unexpected").exists());
            }
            Err(error) => panic!("workspace search failed: {error}"),
        }
        std::fs::remove_dir_all(workspace).unwrap();
    }
}
