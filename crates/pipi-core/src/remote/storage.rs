use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;

use super::RemoteAccessConfig;
use pipi_venv::av::paths;

pub(super) fn load_config() -> Result<RemoteAccessConfig, String> {
    let path = paths::remote_settings_path().ok_or("无法定位 Pipi 数据目录")?;
    match fs::read_to_string(path) {
        Ok(text) => {
            let mut config: RemoteAccessConfig =
                serde_json::from_str(&text).map_err(|e| format!("远程访问配置无效：{e}"))?;
            // 兼容此前由 systemd 托管的部署，只复用凭据路径，不恢复系统服务。
            // 显式保存的空路径表示用户不让应用管理 Tunnel，不能再次自动补回。
            let legacy = dirs::config_dir().map(|root| root.join("pipi-web/tunnel.token"));
            if !config.public_url.is_empty()
                && !serde_json::from_str::<serde_json::Value>(&text)
                    .map_err(|e| e.to_string())?
                    .as_object()
                    .is_some_and(|value| value.contains_key("tunnelTokenFile"))
                && let Some(path) = legacy.filter(|path| path.is_file())
            {
                config.tunnel_token_file = path.to_string_lossy().into_owned();
            }
            config.validate()?;
            Ok(config)
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(RemoteAccessConfig::default()),
        Err(e) => Err(format!("读取远程访问配置失败：{e}")),
    }
}

pub(super) fn save_config(config: &RemoteAccessConfig) -> Result<(), String> {
    let path = paths::remote_settings_path().ok_or("无法定位 Pipi 数据目录")?;
    let bytes = serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?;
    private_write(&path, &bytes)
}

pub(super) fn load_token() -> Result<String, String> {
    let path = paths::remote_token_path().ok_or("无法定位 Pipi 数据目录")?;
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err("远程访问令牌必须是普通文件".into());
        }
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return rotate_token(),
        Err(e) => return Err(format!("读取远程访问令牌失败：{e}")),
    }
    let value = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let token = value.trim();
    if token.len() < 32
        || !token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        return Err("远程访问令牌无效，请在桌面设置中重新生成".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    Ok(token.to_string())
}

pub(super) fn new_token() -> String {
    // UUID v4 使用操作系统随机源；两个 UUID 提供 244 bit 随机量。
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub(super) fn save_token(token: &str) -> Result<(), String> {
    private_write(
        &paths::remote_token_path().ok_or("无法定位 Pipi 数据目录")?,
        token.as_bytes(),
    )
}

fn rotate_token() -> Result<String, String> {
    let token = new_token();
    save_token(&token)?;
    Ok(token)
}

fn private_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("文件路径无父目录")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = parent.join(format!(".remote-{}", uuid::Uuid::new_v4().simple()));
    let result = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result.map_err(|e| format!("保存远程访问配置失败：{e}"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::remote::test_support::{Environment, tunnel_fixture};

    #[test]
    fn legacy_credential_reference_is_reused_but_an_explicit_empty_path_stays_empty() {
        let _lock = pipi_test_support::HOME_LOCK.lock().unwrap();
        let (root, _, _) = tunnel_fixture();
        let _env = Environment::new(&root);
        let legacy = root.join("config/pipi-web/tunnel.token");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, "secret-never-read-by-migration").unwrap();
        let config_path = paths::remote_settings_path().unwrap();
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            r#"{"port":1421,"startWithApp":true,"publicUrl":"https://pipi.example.com"}"#,
        )
        .unwrap();
        assert_eq!(
            load_config().unwrap().tunnel_token_file,
            legacy.to_string_lossy()
        );
        let mut config = load_config().unwrap();
        config.tunnel_token_file.clear();
        save_config(&config).unwrap();
        assert!(load_config().unwrap().tunnel_token_file.is_empty());
        assert!(
            !fs::read_to_string(config_path)
                .unwrap()
                .contains("secret-never-read")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
