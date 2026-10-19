use std::sync::{Arc, Weak};

use tokio::sync::Mutex;

use super::{RemoteAccessConfig, RemoteAccessControl, RemoteAccessStatus, storage};
use crate::{
    runtime::RuntimeState,
    server::{self, ServerHandle, WebAssets},
};

struct State {
    config: RemoteAccessConfig,
    valid: bool,
    server: Option<ServerHandle>,
    error: Option<String>,
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

    fn snapshot(state: &State) -> RemoteAccessStatus {
        let running = state.server.as_ref().is_some_and(ServerHandle::running);
        RemoteAccessStatus {
            config: state.config.clone(),
            running,
            local_url: state
                .server
                .as_ref()
                .filter(|_| running)
                .map(|server| format!("http://{}", server.address())),
            connected_clients: state
                .server
                .as_ref()
                .filter(|_| running)
                .map_or(0, ServerHandle::connected_clients),
            last_error: state.error.clone().or_else(|| {
                (state.server.is_some() && !running)
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
}

#[async_trait::async_trait]
impl RemoteAccessControl for RemoteAccess {
    async fn status(&self) -> RemoteAccessStatus {
        Self::snapshot(&*self.state.lock().await)
    }

    async fn configure(&self, config: RemoteAccessConfig) -> Result<RemoteAccessStatus, String> {
        config.validate()?;
        let mut state = self.state.lock().await;
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
        if let Some(replacement) = replacement {
            if let Some(old) = state.server.replace(replacement) {
                let _ = old.stop().await;
            }
        }
        state.config = config;
        state.valid = true;
        state.error = None;
        Ok(Self::snapshot(&state))
    }

    async fn set_enabled(&self, enabled: bool) -> Result<RemoteAccessStatus, String> {
        let mut state = self.state.lock().await;
        if !enabled {
            if let Some(server) = state.server.take() {
                server.stop().await?;
            }
            state.error = None;
        } else if !state.server.as_ref().is_some_and(ServerHandle::running) {
            if !state.valid {
                return Err(state.error.clone().unwrap_or("请先保存有效配置".into()));
            }
            if let Some(old) = state.server.take() {
                let _ = old.stop().await;
            }
            let result = match storage::load_token() {
                Ok(token) => self.start(&state.config, token).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(server) => {
                    state.server = Some(server);
                    state.error = None;
                }
                Err(error) => {
                    state.error = Some(error.clone());
                    return Err(error);
                }
            }
        }
        Ok(Self::snapshot(&state))
    }

    async fn token(&self, rotate: bool) -> Result<String, String> {
        let mut state = self.state.lock().await;
        if !rotate {
            return storage::load_token();
        }
        let token = storage::new_token();
        storage::save_token(&token)?;
        let restart = state.server.as_ref().is_some_and(ServerHandle::running);
        if let Some(server) = state.server.take() {
            server.stop().await?;
        }
        if restart {
            match self.start(&state.config, token.clone()).await {
                Ok(server) => {
                    state.server = Some(server);
                    state.error = None;
                }
                Err(error) => {
                    state.error = Some(error.clone());
                    return Err(error);
                }
            }
        }
        Ok(token)
    }
}
