use std::sync::{Arc, Weak};

use tokio::sync::Mutex;

use super::{
    RemoteAccessConfig, RemoteAccessControl, RemoteAccessStatus, storage, tunnel::TunnelHandle,
};
use crate::{
    runtime::RuntimeState,
    server::{self, ServerHandle, WebAssets},
};

struct State {
    config: RemoteAccessConfig,
    valid: bool,
    server: Option<ServerHandle>,
    tunnel: Option<TunnelHandle>,
    error: Option<String>,
}

impl State {
    async fn stop_all(&mut self) -> Result<(), String> {
        // 即使一项停止失败，也继续关闭另一项。
        let tunnel = match self.tunnel.take() {
            Some(tunnel) => tunnel.stop().await,
            None => Ok(()),
        };
        let server = match self.server.take() {
            Some(server) => server.stop().await,
            None => Ok(()),
        };
        tunnel.and(server)
    }

    /// 启停与令牌轮换共用失败回收、错误记录，避免留下半开的远程入口。
    async fn finish_transition(&mut self, result: Result<(), String>) -> Result<(), String> {
        if let Err(error) = result {
            let _ = self.stop_all().await;
            self.error = Some(error.clone());
            return Err(error);
        }
        self.error = None;
        Ok(())
    }
}

pub struct RemoteAccess {
    runtime: Weak<RuntimeState>,
    assets: WebAssets,
    state: Mutex<State>,
}

impl RemoteAccess {
    pub fn keep_alive(&self) -> bool {
        self.state
            .try_lock()
            .map(|state| state.server.as_ref().is_some_and(ServerHandle::running))
            .unwrap_or(true)
    }

    pub fn new(runtime: &Arc<RuntimeState>, assets: WebAssets) -> Arc<Self> {
        let (config, valid, error) = match storage::load_config() {
            Ok(config) => (config, true, None),
            Err(error) => (RemoteAccessConfig::default(), false, Some(error)),
        };
        Arc::new(Self {
            runtime: Arc::downgrade(runtime),
            assets,
            state: Mutex::new(State {
                config,
                valid,
                server: None,
                tunnel: None,
                error,
            }),
        })
    }

    pub async fn restore(&self) {
        let start = {
            let state = self.state.lock().await;
            state.valid && state.config.start_with_app
        };
        if start {
            let _ = self.set_enabled(true).await;
        }
    }

    async fn snapshot(state: &mut State) -> RemoteAccessStatus {
        let local_running = state.server.as_ref().is_some_and(ServerHandle::running);
        // 本地服务异常退出后不保留公网连接。
        if !local_running && let Some(tunnel) = state.tunnel.take() {
            let _ = tunnel.stop().await;
        }
        let (tunnel_running, tunnel_connected, tunnel_error) = match &mut state.tunnel {
            Some(tunnel) => tunnel.status().await,
            None => (false, false, None),
        };
        RemoteAccessStatus {
            config: state.config.clone(),
            local_running,
            tunnel_running,
            tunnel_connected,
            local_url: state
                .server
                .as_ref()
                .filter(|_| local_running)
                .map(|server| format!("http://{}", server.address())),
            connected_clients: state
                .server
                .as_ref()
                .filter(|_| local_running)
                .map_or(0, ServerHandle::connected_clients),
            last_error: state.error.clone().or(tunnel_error).or_else(|| {
                (state.server.is_some() && !local_running)
                    .then(|| "远程服务已意外停止，请重新启动".into())
            }),
        }
    }

    async fn start(
        &self,
        config: &RemoteAccessConfig,
        token: String,
    ) -> Result<ServerHandle, String> {
        let runtime = self.runtime.upgrade().ok_or("宿主已退出")?;
        server::start(
            runtime,
            ([127, 0, 0, 1], config.port).into(),
            Some(token),
            self.assets.clone(),
        )
        .await
    }

    /// 仅重启 HTTP/WS；令牌轮换无需重启现有 Tunnel。
    async fn restart_server(&self, state: &mut State, token: String) -> Result<(), String> {
        if let Some(server) = state.server.take() {
            let was_running = server.running();
            let stopped = server.stop().await;
            // 已异常结束的旧句柄不能阻止用户一次点击重新开启。
            if was_running {
                stopped?;
            }
        }
        state.server = Some(self.start(&state.config, token).await?);
        Ok(())
    }
}

#[async_trait::async_trait]
impl RemoteAccessControl for RemoteAccess {
    async fn status(&self) -> RemoteAccessStatus {
        Self::snapshot(&mut *self.state.lock().await).await
    }

    async fn configure(&self, config: RemoteAccessConfig) -> Result<RemoteAccessStatus, String> {
        config.validate()?;
        let mut state = self.state.lock().await;
        if state.server.as_ref().is_some_and(ServerHandle::running)
            && (state.config.tunnel_token_file != config.tunnel_token_file
                || (state.config.port != config.port && !state.config.tunnel_token_file.is_empty()))
        {
            return Err(
                "请先关闭远程访问再修改端口或 Tunnel 凭据；Cloudflare 的服务地址需与本机端口一致"
                    .into(),
            );
        }
        // 先绑定新端口，冲突时旧入口仍工作；持久化失败也关闭预备服务并保留旧配置。
        let replacement = if state.server.as_ref().is_some_and(ServerHandle::running)
            && state.config.port != config.port
        {
            match self.start(&config, storage::load_token()?).await {
                Ok(server) => Some(server),
                Err(error) => {
                    state.error = Some(error.clone());
                    return Err(error);
                }
            }
        } else {
            None
        };
        if let Err(error) = storage::save_config(&config) {
            if let Some(server) = replacement {
                let _ = server.stop().await;
            }
            state.error = Some(error.clone());
            return Err(error);
        }
        if let Some(replacement) = replacement
            && let Some(old) = state.server.replace(replacement)
        {
            let _ = old.stop().await;
        }
        state.config = config;
        state.valid = true;
        state.error = None;
        Ok(Self::snapshot(&mut state).await)
    }

    async fn set_enabled(&self, enabled: bool) -> Result<RemoteAccessStatus, String> {
        let mut state = self.state.lock().await;
        let result = if !enabled {
            state.stop_all().await
        } else {
            if !state.valid {
                return Err(state.error.clone().unwrap_or("请先保存有效配置".into()));
            }
            async {
                if !state.server.as_ref().is_some_and(ServerHandle::running) {
                    self.restart_server(&mut state, storage::load_token()?)
                        .await?;
                }
                let alive = if let Some(tunnel) = &mut state.tunnel {
                    tunnel.running()
                } else {
                    false
                };
                if !alive && !state.config.tunnel_token_file.is_empty() {
                    if let Some(old) = state.tunnel.take() {
                        let _ = old.stop().await;
                    }
                    state.tunnel =
                        Some(TunnelHandle::start(&state.config.tunnel_token_file).await?);
                } else if state.config.tunnel_token_file.is_empty()
                    && !state.config.public_url.is_empty()
                {
                    return Err(
                        "请配置 Tunnel 凭据文件；只填写公开地址不会建立公网连接".to_string()
                    );
                }
                Ok(())
            }
            .await
        };
        state.finish_transition(result).await?;
        Ok(Self::snapshot(&mut state).await)
    }

    async fn token(&self, rotate: bool) -> Result<String, String> {
        let mut state = self.state.lock().await;
        if !rotate {
            return storage::load_token();
        }
        let token = storage::new_token();
        storage::save_token(&token)?;
        let restart = state.server.as_ref().is_some_and(ServerHandle::running);
        let result = if restart {
            self.restart_server(&mut state, token.clone()).await
        } else {
            state.stop_all().await
        };
        state.finish_transition(result).await?;
        Ok(token)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::remote::test_support::{Environment, tunnel_fixture};
    use crate::server::WebAsset;
    use std::{fs, time::Duration};

    fn assets() -> WebAssets {
        WebAssets::Embedded(Arc::new(|_| {
            Some(WebAsset {
                bytes: b"<html>Pipi</html>".to_vec(),
                content_type: "text/html".into(),
            })
        }))
    }

    #[tokio::test]
    async fn app_restore_toggle_rotation_and_tunnel_failure_share_one_lifecycle() {
        let _lock = pipi_test_support::HOME_LOCK.lock().unwrap();
        let (root, _, token) = tunnel_fixture();
        let _env = Environment::new(&root);
        let runtime = Arc::new(RuntimeState::new(tokio::runtime::Handle::current()));
        let assets = assets();
        let remote = RemoteAccess::new(&runtime, assets.clone());
        remote.restore().await;
        assert!(!remote.status().await.local_running);
        let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);
        remote
            .configure(RemoteAccessConfig {
                port,
                start_with_app: true,
                public_url: "https://pipi.example.com".into(),
                tunnel_token_file: token.to_str().unwrap().into(),
            })
            .await
            .unwrap();
        assert!(!remote.status().await.local_running, "保存配置不隐式启动");
        remote.restore().await;
        let connecting = remote.status().await;
        assert!(connecting.local_running && connecting.tunnel_running);
        assert!(!connecting.tunnel_connected);
        fs::write(root.join("connected"), "").unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !remote.status().await.tunnel_connected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let pid = fs::read_to_string(root.join("pid")).unwrap();
        let probes: usize = fs::read_to_string(root.join("probes"))
            .unwrap()
            .parse()
            .unwrap();
        remote.set_enabled(true).await.unwrap();
        let next_probes: usize = fs::read_to_string(root.join("probes"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(next_probes, probes + 1, "启用只在返回快照时探测一次连接");
        assert_eq!(
            fs::read_to_string(root.join("pid")).unwrap(),
            pid,
            "重复开启不重复 spawn"
        );
        let old_token = remote.token(false).await.unwrap();
        assert_ne!(old_token, remote.token(true).await.unwrap());
        assert_eq!(
            fs::read_to_string(root.join("pid")).unwrap(),
            pid,
            "轮换令牌保留同一个 Tunnel"
        );
        assert!(remote.status().await.tunnel_connected);
        let mut changed = remote.status().await.config;
        changed.port = if port == 1421 { 1422 } else { 1421 };
        assert!(remote.configure(changed).await.is_err());
        assert_eq!(remote.status().await.config.port, port);
        fs::remove_file(root.join("connected")).unwrap();
        let disconnected = remote.status().await;
        assert!(!disconnected.tunnel_connected);
        assert!(disconnected.local_running && disconnected.tunnel_running);
        remote.set_enabled(false).await.unwrap();
        let stopped = remote.status().await;
        assert!(!stopped.local_running && !stopped.tunnel_running && !stopped.tunnel_connected);
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err()
        );

        // 后续启动恢复已保存的开关；进程退出不能伪装成远程已连接。
        fs::write(root.join("exit"), "").unwrap();
        let restored = RemoteAccess::new(&runtime, assets.clone());
        restored.restore().await;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let status = restored.status().await;
                if status.last_error.is_some() {
                    assert!(!status.tunnel_connected && !status.tunnel_running);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        restored.set_enabled(false).await.unwrap();
        let mut config = restored.status().await.config;
        config.start_with_app = false;
        restored.configure(config).await.unwrap();
        let off = RemoteAccess::new(&runtime, assets);
        off.restore().await;
        assert!(!off.status().await.local_running);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn rotation_restart_failure_reclaims_both_transports_and_keeps_error() {
        let _lock = pipi_test_support::HOME_LOCK.lock().unwrap();
        let (root, _, token) = tunnel_fixture();
        let _env = Environment::new(&root);
        let runtime = Arc::new(RuntimeState::new(tokio::runtime::Handle::current()));
        let remote = RemoteAccess::new(&runtime, assets());
        let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);
        remote
            .configure(RemoteAccessConfig {
                port,
                public_url: "https://pipi.example.com".into(),
                tunnel_token_file: token.to_str().unwrap().into(),
                ..Default::default()
            })
            .await
            .unwrap();
        fs::write(root.join("connected"), "").unwrap();
        remote.set_enabled(true).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !remote.status().await.tunnel_connected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let metrics = fs::read_to_string(root.join("metrics")).unwrap();
        let blocker = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        // 注入重启绑定失败，验证正常运行中的 Tunnel 也被回收。
        remote.state.lock().await.config.port = blocker.local_addr().unwrap().port();
        let error = remote.token(true).await.unwrap_err();
        let status = remote.status().await;
        assert!(!status.local_running && !status.tunnel_running && !status.tunnel_connected);
        assert_eq!(status.last_error.as_deref(), Some(error.as_str()));
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err()
        );
        assert!(tokio::net::TcpStream::connect(metrics).await.is_err());
        assert!(
            serde_json::to_value(status)
                .unwrap()
                .get("running")
                .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
