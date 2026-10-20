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
            return Err("端口必须在 1–65535 之间".into());
        }
        if !self.tunnel_token_file.is_empty() {
            if !std::path::Path::new(&self.tunnel_token_file).is_absolute()
                || self.tunnel_token_file.chars().any(char::is_control)
            {
                return Err("Tunnel 凭据文件必须使用绝对路径".into());
            }
            if self.public_url.is_empty() {
                return Err("使用 Cloudflare Tunnel 时请填写公开访问地址".into());
            }
        }
        if !self.public_url.is_empty() {
            let authority = self
                .public_url
                .strip_prefix("https://")
                .ok_or("公开地址必须使用 HTTPS")?;
            if authority.is_empty()
                || authority.contains(['/', '?', '#', '@', '\\'])
                || authority
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control())
            {
                return Err("公开地址应为 HTTPS 域名，不包含路径、账号或查询参数".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAccessStatus {
    pub config: RemoteAccessConfig,
    pub running: bool,
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
            Self::Web => Err("远程访问管理仅允许在桌面应用中操作".into()),
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
