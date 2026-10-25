use super::PushSubscription;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Store {
    pub private_key: String,
    pub subscriptions: Vec<PushSubscription>,
}

pub(super) fn load(path: Option<&std::path::Path>) -> Result<Option<Store>, String> {
    let path = path.ok_or("Cannot locate Pipi data directory")?;
    if !pipi_protocol::fs::ensure_real_file(path, "Push subscription store")? {
        return Ok(None);
    }
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > 256 * 1024 {
        return Err("Push subscription store is too large".into());
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let store: Store =
        serde_json::from_str(&text).map_err(|_| "Invalid push subscription store")?;
    if store.subscriptions.len() > 32 {
        return Err("Too many stored push subscriptions".into());
    }
    Ok(Some(store))
}

pub(super) fn save(path: Option<&std::path::Path>, store: &Store) -> Result<(), String> {
    let path = path.ok_or("Cannot locate Pipi data directory")?;
    pipi_protocol::fs::ensure_real_file(path, "Push subscription store")?;
    let parent = path.parent().ok_or("Missing Pipi data directory")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = parent.join(format!(".push-{}", uuid::Uuid::new_v4().simple()));
    let result = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(store)?)?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|e| format!("Cannot save push subscriptions: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::push::PushService;
    use pipi_test_support::{HOME_LOCK, remove_env_var, set_env_var, temp_dir};

    #[test]
    fn key_is_stable_private_and_corrupt_store_is_not_overwritten() {
        let _guard = HOME_LOCK.lock().unwrap();
        let dir = temp_dir("pipi-push");
        let previous = std::env::var_os("PIPI_HOME");
        set_env_var("PIPI_HOME", &dir);
        let service = PushService::default();
        let key = service.public_key().unwrap();
        assert_eq!(key, PushService::default().public_key().unwrap());
        let device = |suffix: &str| PushSubscription {
            endpoint: format!("https://web.push.apple.com/{suffix}"),
            p256dh: key.clone(),
            auth: base64::Engine::encode(&base64::prelude::BASE64_URL_SAFE_NO_PAD, [7u8; 16]),
            origin: "https://pipi.example.com".into(),
        };
        service.subscribe(device("one")).unwrap();
        service.subscribe(device("two")).unwrap();
        service.subscribe(device("one")).unwrap();
        assert_eq!(
            load(service.path.as_deref())
                .unwrap()
                .unwrap()
                .subscriptions
                .len(),
            2
        );
        service.unsubscribe(&device("one").endpoint).unwrap();
        assert!(!service.contains(&device("one").endpoint).unwrap());
        assert!(service.contains(&device("two").endpoint).unwrap());
        assert_eq!(key, service.public_key().unwrap());
        let mut invalid = device("invalid");
        invalid.auth = "bad".into();
        assert!(service.subscribe(invalid).is_err());
        assert!(service.contains(&device("two").endpoint).unwrap());
        let path = pipi_venv::av::paths::push_store_path().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::write(&path, "broken").unwrap();
        assert!(service.public_key().is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "broken");
        if let Some(value) = previous {
            set_env_var("PIPI_HOME", value);
        } else {
            remove_env_var("PIPI_HOME");
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
