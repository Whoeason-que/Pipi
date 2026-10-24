//! 正常退出：关闭运行入口，中止并等待会话收尾，最后重试尚未提交的缓冲。
use super::*;
use std::time::Duration;

impl RuntimeState {
    pub(super) fn ensure_not_shutting_down(&self) -> Result<(), String> {
        if self.shutting_down.load(Ordering::Acquire) {
            Err("Application is shutting down; quit again to retry saving".into())
        } else {
            Ok(())
        }
    }

    /// 失败后保留宿主与账本；再次调用会重试，绝不取消正在收尾的任务。
    pub async fn shutdown(&self) -> Result<(), String> {
        {
            // 与 send_prompt / compact_now 共用锁，禁止检查后又启动新一轮。
            let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
            self.shutting_down.store(true, Ordering::Release);
            for session in sessions.values() {
                session.engine.stop()?;
            }
        }
        tokio::time::timeout(Duration::from_secs(15), async {
            self.background.shutdown().await?;
            loop {
                let running = self
                    .sessions
                    .lock()
                    .map_err(|e| e.to_string())?
                    .values()
                    .try_fold(false, |running, session| {
                        Ok::<_, String>(running || session.engine.snapshot()?.running)
                    })?;
                if !running {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let sessions = self.sessions.clone();
            tokio::task::spawn_blocking(move || {
                let sessions = sessions.lock().map_err(|e| e.to_string())?;
                for (key, session) in sessions.iter() {
                    session.engine.commit_idle().map_err(|error| {
                        format!(
                            "Cannot save session {}/{}: {error}",
                            key.agent_name, key.session_id
                        )
                    })?;
                }
                Ok::<_, String>(())
            })
            .await
            .map_err(|error| format!("Cannot finish saving sessions: {error}"))?
        })
        .await
        .map_err(|_| "Stopping and saving sessions timed out; quit again to retry".to_string())?
    }
}
