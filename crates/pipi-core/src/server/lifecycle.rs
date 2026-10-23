use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use tokio::{net::TcpListener, sync::watch, task::JoinHandle};

use super::{AppState, WebAssets, build_app};
use crate::runtime::RuntimeState;

/// 停止 HTTP/WS 不释放运行时，也不取消 Agent 任务。
pub struct ServerHandle {
    addr: SocketAddr,
    shutdown: watch::Sender<bool>,
    task: Option<JoinHandle<Result<(), String>>>,
    clients: Arc<AtomicUsize>,
}

impl ServerHandle {
    pub fn address(&self) -> SocketAddr {
        self.addr
    }
    pub fn running(&self) -> bool {
        self.task.as_ref().is_some_and(|task| !task.is_finished())
    }
    pub fn connected_clients(&self) -> usize {
        self.clients.load(Ordering::Relaxed)
    }

    pub async fn stop(mut self) -> Result<(), String> {
        let _ = self.shutdown.send(true);
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
                Ok(result) => result.map_err(|e| e.to_string())?,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    Ok(())
                }
            }
        } else {
            Ok(())
        }
    }

    pub async fn wait(&mut self) -> Result<(), String> {
        match self.task.as_mut() {
            Some(task) => task.await.map_err(|e| e.to_string())?,
            None => Ok(()),
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

pub async fn start(
    runtime: Arc<RuntimeState>,
    addr: SocketAddr,
    token: Option<String>,
    assets: WebAssets,
) -> Result<ServerHandle, String> {
    if !addr.ip().is_loopback() && token.is_none() {
        return Err("Access token must be configured when listening on non-loopback addresses".into());
    }
    if token.as_deref().is_some_and(|token| {
        token.is_empty() || token.chars().any(|c| c.is_ascii_control() || c == ';')
    }) {
        return Err("Access token cannot be empty or contain control characters or semicolons".into());
    }
    if !assets.has_index() {
        return Err("Remote web assets missing, please run npm run build or reinstall full application".into());
    }
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| format!("Cannot listen on {addr}: {e}. Please check if port is occupied by another service"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let clients = Arc::new(AtomicUsize::new(0));
    let (shutdown, receiver) = watch::channel(false);
    let state = AppState {
        events: runtime.events().clone(),
        runtime,
        assets,
        auth_token: token,
        shutdown: receiver.clone(),
        clients: clients.clone(),
    };
    let task = tokio::spawn(async move {
        let mut receiver = receiver;
        axum::serve(listener, build_app(state))
            .with_graceful_shutdown(async move {
                if !*receiver.borrow() {
                    let _ = receiver.changed().await;
                }
            })
            .await
            .map_err(|e| format!("Remote service exited abnormally: {e}"))
    });
    Ok(ServerHandle {
        addr,
        shutdown,
        task: Some(task),
        clients,
    })
}
