//! 同一文件的写操作串行化。
//!
//! 运行时会把同一批里的非 bash 工具**并行**执行（见 `agent_loop/tools.rs`），
//! 同一轮里对同一个文件的两个 `edit` / `write` 会互相覆盖：两个调用都读到旧
//! 内容，后写的那个把先写的静默抹掉。这里按**规范化路径**给写操作排队，
//! 与上游 pi 的 `withFileMutationQueue` 同一语义。
//!
//! 只在进程内生效（桌面应用同一宿主）；跨进程的并发写不在工具层解决。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

fn registry() -> &'static Mutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 同一个文件的不同写法（相对/绝对、经符号链接目录）落到同一个键。
/// 目标尚不存在时（`write` 新建文件）退化为「父目录 canonical + 文件名」。
pub fn lock_key(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    match (absolute.parent(), absolute.file_name()) {
        (Some(parent), Some(name)) => std::fs::canonicalize(parent)
            .map(|parent| parent.join(name))
            .unwrap_or(absolute),
        _ => absolute,
    }
}

/// 某个文件的写锁；guard 存活期间同一文件的 `edit` / `write` 串行执行。
pub struct FileLock {
    guard: Option<OwnedMutexGuard<()>>,
    key: PathBuf,
    mutex: Arc<AsyncMutex<()>>,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // 先释放 guard 再清表项：guard 还在时移除，会让新来的写者拿到一把全新
        // 的锁，与当前持有者并发。清表项只在「没有等待者」时发生（strong_count
        // == 2：表里一份 + 自己一份），避免长会话编辑大量文件后无界增长。
        drop(self.guard.take());
        let Ok(mut registry) = registry().lock() else {
            return;
        };
        if let Some(entry) = registry.get(&self.key)
            && Arc::ptr_eq(entry, &self.mutex)
            && Arc::strong_count(entry) == 2
        {
            registry.remove(&self.key);
        }
    }
}

/// 取得某个文件的写锁；返回值必须持有到文件写入完成。
pub async fn lock(path: &Path) -> FileLock {
    let key = lock_key(path);
    let mutex = {
        let mut registry = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        registry
            .entry(key.clone())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    };
    let guard = mutex.clone().lock_owned().await;
    FileLock {
        guard: Some(guard),
        key,
        mutex,
    }
}

/// Stop waiting without performing a queued mutation after cancellation.
pub async fn lock_cancellable(
    path: &Path,
    abort: &pipi_protocol::AbortSignal,
) -> Result<FileLock, String> {
    let token = abort.token();
    tokio::select! {
        biased;
        _ = token.cancelled() => Err("Operation aborted".into()),
        guard = lock(path) => Ok(guard),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn same_file_is_serialized() {
        let dir = pipi_test_support::temp_dir("file-queue");
        let path = dir.join("a.txt");
        tokio::fs::write(&path, "x").await.unwrap();

        let first = lock(&path).await;
        let second_path = path.clone();
        let task = tokio::spawn(async move {
            let _guard = lock(&second_path).await;
            42
        });
        // 另一条任务必须先等第一个 guard 释放。
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!task.is_finished());

        drop(first);
        let value = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("释放后应立即拿到锁")
            .unwrap();
        assert_eq!(value, 42);

        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn different_files_do_not_block_each_other() {
        let dir = pipi_test_support::temp_dir("file-queue-diff");
        let first = dir.join("a.txt");
        let second = dir.join("b.txt");
        tokio::fs::write(&first, "a").await.unwrap();
        tokio::fs::write(&second, "b").await.unwrap();

        let _held = lock(&first).await;
        // 不同文件不应等待。
        let _other = tokio::time::timeout(Duration::from_secs(1), lock(&second))
            .await
            .expect("不同文件不应互相阻塞");

        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn lock_is_released_and_registry_is_cleaned_up() {
        let dir = pipi_test_support::temp_dir("file-queue-clean");
        let path = dir.join("a.txt");
        tokio::fs::write(&path, "x").await.unwrap();
        let key = lock_key(&path);

        {
            let _guard = lock(&path).await;
            assert!(registry().lock().unwrap().contains_key(&key));
        }
        assert!(!registry().lock().unwrap().contains_key(&key));

        std::fs::remove_dir_all(dir).ok();
    }
}
