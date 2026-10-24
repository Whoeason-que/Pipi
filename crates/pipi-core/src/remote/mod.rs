//! 桌面远程入口的配置、管理端口与宿主进程所有权。
use serde::{Deserialize, Serialize};

mod host;
pub use host::HostLease;
#[cfg(feature = "server")]
mod service;
#[cfg(feature = "server")]
mod storage;
#[cfg(all(test, feature = "server", unix))]
mod test_support;
#[cfg(feature = "server")]
mod tunnel;
#[cfg(feature = "server")]
pub use service::RemoteAccess;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteAccessConfig {
    pub port: u16,
    pub start_with_app: bool,
    pub public_url: String,
    /// 只保存凭据文件路径，秘密值不进入 DTO。
    #[serde(default)]
    pub tunnel_token_file: String,
}

impl Default for RemoteAccessConfig {
    fn default() -> Self {
        Self {
            port: 1421,
            start_with_app: false,
            public_url: String::new(),
            tunnel_token_file: String::new(),
        }
    }
}

impl RemoteAccessConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("Port must be between 1 and 65535".into());
        }
        if !self.tunnel_token_file.is_empty() {
            if !std::path::Path::new(&self.tunnel_token_file).is_absolute()
                || self.tunnel_token_file.chars().any(char::is_control)
            {
                return Err("Tunnel credentials file must use an absolute path".into());
            }
            if self.public_url.is_empty() {
                return Err(
                    "Please specify a public access URL when using Cloudflare Tunnel".into(),
                );
            }
        }
        if !self.public_url.is_empty() {
            let authority = self
                .public_url
                .strip_prefix("https://")
                .ok_or("Public URL must use HTTPS")?;
            if authority.is_empty()
                || authority.contains(['/', '?', '#', '@', '\\'])
                || authority
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control())
            {
                return Err("Public URL must be an HTTPS domain without path, username, or query parameters".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAccessStatus {
    pub config: RemoteAccessConfig,
    pub local_running: bool,
    pub tunnel_running: bool,
    pub tunnel_connected: bool,
    pub local_url: Option<String>,
    pub connected_clients: usize,
    pub last_error: Option<String>,
}

/// 来源由 transport 指定，永不从 JSON 参数读取。
#[derive(Clone, Copy)]
pub enum CommandSource {
    Desktop,
    Web,
}

impl CommandSource {
    pub fn require_desktop(self) -> Result<(), String> {
        match self {
            Self::Desktop => Ok(()),
            Self::Web => {
                Err("Remote access management is only allowed in the desktop application".into())
            }
        }
    }
}

#[async_trait::async_trait]
pub trait RemoteAccessControl: Send + Sync {
    async fn status(&self) -> RemoteAccessStatus;
    async fn configure(&self, config: RemoteAccessConfig) -> Result<RemoteAccessStatus, String>;
    async fn set_enabled(&self, enabled: bool) -> Result<RemoteAccessStatus, String>;
    async fn token(&self, rotate: bool) -> Result<String, String>;
}
