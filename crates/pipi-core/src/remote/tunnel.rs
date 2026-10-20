//! 应用拥有的 cloudflared 子进程；只通过凭据文件传密钥，连接状态来自 /ready。
use std::{
    net::SocketAddr,
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
};

pub(super) struct TunnelHandle {
    child: Child,
    metrics: SocketAddr,
    started: Instant,
}

impl TunnelHandle {
    pub async fn start(token_file: &str) -> Result<Self, String> {
        let binary = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|dir| {
                dir.join(if cfg!(windows) {
                    "cloudflared.exe"
                } else {
                    "cloudflared"
                })
            })
            .chain(
                [
                    "/usr/local/bin/cloudflared",
                    "/opt/homebrew/bin/cloudflared",
                    "/usr/bin/cloudflared",
                ]
                .map(Into::into),
            )
            .find(|path| path.is_file())
            .ok_or("未找到 cloudflared，请先安装 Cloudflare Tunnel 客户端")?;
        Self::spawn(&binary, Path::new(token_file)).await
    }

    async fn spawn(binary: &Path, token_file: &Path) -> Result<Self, String> {
        let meta = tokio::fs::symlink_metadata(token_file)
            .await
            .map_err(|_| "无法读取 Tunnel 凭据文件，请检查文件路径和权限")?;
        if !meta.is_file() || meta.len() == 0 {
            return Err("Tunnel 凭据必须是非空普通文件".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err("Tunnel 凭据文件权限过宽，请设为 0600".into());
            }
        }
        // 随机回环端口只供应用探测连接状态，不对外暴露管理接口。
        let reservation = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| e.to_string())?;
        let metrics = reservation.local_addr().map_err(|e| e.to_string())?;
        drop(reservation);
        let child = Command::new(binary)
            .args([
                "--no-autoupdate",
                "tunnel",
                "--metrics",
                &metrics.to_string(),
                "--loglevel",
                "error",
                "run",
                "--token-file",
            ])
            .arg(token_file)
            // 环境令牌优先于 --token-file；不得继承另一条 Tunnel 或调试日志配置。
            .env_clear()
            .envs(std::env::vars_os().filter(|(key, _)| {
                let key = key.to_string_lossy();
                !key.starts_with("TUNNEL_") && !key.starts_with("CF_")
            }))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("无法启动 cloudflared：{e}"))?;
        Ok(Self {
            child,
            metrics,
            started: Instant::now(),
        })
    }

    pub async fn status(&mut self) -> (bool, bool, Option<String>) {
        match self.child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => {
                return (
                    false,
                    false,
                    Some(format!(
                        "Cloudflare Tunnel 已退出（{status}），请检查凭据并重新启动远程访问"
                    )),
                );
            }
            Err(_) => return (false, false, Some("无法读取 Cloudflare Tunnel 状态".into())),
        }
        let connected = ready(self.metrics).await;
        let error = (!connected && self.started.elapsed() > Duration::from_secs(30))
            .then(|| "Tunnel 尚未连接 Cloudflare，请检查网络和凭据；客户端正在重连".into());
        (true, connected, error)
    }

    pub async fn stop(mut self) -> Result<(), String> {
        self.child
            .kill()
            .await
            .map_err(|e| format!("停止 Cloudflare Tunnel 失败：{e}"))
    }
}

async fn ready(addr: SocketAddr) -> bool {
    tokio::time::timeout(Duration::from_millis(500), async {
        let mut stream = TcpStream::connect(addr).await?;
        stream
            .write_all(
                format!("GET /ready HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await?;
        // 只读有界状态行，不缓存 cloudflared 的配置、指标或任何秘密值。
        let mut line = Vec::new();
        use tokio::io::AsyncReadExt;
        BufReader::new(stream)
            .take(128)
            .read_until(b'\n', &mut line)
            .await?;
        Ok::<_, std::io::Error>(
            line.starts_with(b"HTTP/1.1 200 ") || line.starts_with(b"HTTP/1.0 200 "),
        )
    })
    .await
    .is_ok_and(|result| result.unwrap_or(false))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::remote::test_support::tunnel_fixture as fixture;
    use std::{fs, os::unix::fs::PermissionsExt};

    async fn wait_connected(handle: &mut TunnelHandle) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !handle.status().await.1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn readiness_tracks_connection_loss_and_stop_reaps_owned_process() {
        let _lock = pipi_test_support::HOME_LOCK.lock().unwrap();
        let (root, binary, token) = fixture();
        let mut handle = TunnelHandle::spawn(&binary, &token).await.unwrap();
        let metrics = handle.metrics;
        assert_eq!(handle.status().await, (true, false, None));
        fs::write(root.join("connected"), "").unwrap();
        wait_connected(&mut handle).await;
        fs::remove_file(root.join("connected")).unwrap();
        assert_eq!(handle.status().await, (true, false, None));
        handle.started = Instant::now() - Duration::from_secs(31);
        assert!(handle.status().await.2.unwrap().contains("重连"));
        handle.stop().await.unwrap();
        assert!(!ready(metrics).await);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn drop_kills_process_and_early_exit_is_not_reported_as_connected() {
        let _lock = pipi_test_support::HOME_LOCK.lock().unwrap();
        let (root, binary, token) = fixture();
        fs::write(root.join("connected"), "").unwrap();
        let mut handle = TunnelHandle::spawn(&binary, &token).await.unwrap();
        wait_connected(&mut handle).await;
        let metrics = handle.metrics;
        drop(handle);
        tokio::time::timeout(Duration::from_secs(3), async {
            while ready(metrics).await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        fs::write(root.join("exit"), "").unwrap();
        let mut handle = TunnelHandle::spawn(&binary, &token).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let status = handle.status().await;
                if !status.0 {
                    assert!(!status.1);
                    assert!(status.2.unwrap().contains("23"));
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        handle.stop().await.unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn invalid_or_public_credentials_are_rejected_before_spawn() {
        let _lock = pipi_test_support::HOME_LOCK.lock().unwrap();
        let (root, binary, token) = fixture();
        fs::set_permissions(&token, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            TunnelHandle::spawn(&binary, &token)
                .await
                .err()
                .unwrap()
                .contains("0600")
        );
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&token, "").unwrap();
        assert!(TunnelHandle::spawn(&binary, &token).await.is_err());
        assert!(!root.join("pid").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
