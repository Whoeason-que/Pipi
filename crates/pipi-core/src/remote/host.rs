use pipi_venv::av::paths;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

/// 持有文件描述符即持有宿主所有权，崩溃后由 OS 自动释放，不删除锁文件。
pub struct HostLease {
    _file: File,
}

impl HostLease {
    pub fn acquire() -> Result<Self, String> {
        Self::at(&paths::host_lock_path().ok_or("Cannot locate Pipi data directory")?)
    }

    fn at(path: &Path) -> Result<Self, String> {
        fs::create_dir_all(path.parent().ok_or("Invalid lock path")?).map_err(|e| e.to_string())?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| format!("Failed to open host lock: {e}"))?;
        file.try_lock().map_err(|_| {
            "Another Pipi host is already running in this data directory, please exit the other desktop app or standalone service first".to_string()
        })?;
        file.set_len(0).map_err(|e| e.to_string())?;
        write!(file, "{}", std::process::id()).map_err(|e| e.to_string())?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_lease_is_exclusive_and_released_on_drop() {
        let dir = pipi_test_support::temp_dir("host-lease");
        let path = dir.join("host.lock");
        let lease = HostLease::at(&path).unwrap();
        assert!(HostLease::at(&path).is_err());
        drop(lease);
        assert!(HostLease::at(&path).is_ok());
        let _ = fs::remove_dir_all(dir);
    }
}
