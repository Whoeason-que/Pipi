//! 协作式中止信号。
//!
//! 它没有持久化或 IPC 语义，但必须由 provider、工具与宿主共享，因而住在契约层。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

/// 协作式中止信号。它没有持久化或 IPC 语义，但必须由 provider、harness 与
/// 工具共享，因而和消息协议一起放在最底层的公共 crate。
///
/// 内部是 `tokio_util` 的 `CancellationToken`：等待方在 `abort()` 时被立刻唤醒，
/// 不再靠定时轮询（旧实现的 `wait_aborted` 每 50ms 醒一次，停止按钮与工具超时
/// 都带上了这段延迟，多会话并发时还会叠加周期性唤醒）。`reset()` 为下一轮换一个
/// 新 token —— 所有克隆共享同一状态，与旧的原子布尔语义一致。
#[derive(Clone, Default)]
pub struct AbortSignal(Arc<std::sync::Mutex<CancellationToken>>);

impl AbortSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn abort(&self) {
        self.token().cancel();
    }

    pub fn reset(&self) {
        *self.0.lock().expect("abort signal poisoned") = CancellationToken::new();
    }

    pub fn is_aborted(&self) -> bool {
        self.token().is_cancelled()
    }

    pub async fn wait_aborted(&self) {
        self.token().cancelled().await;
    }

    fn token(&self) -> CancellationToken {
        self.0.lock().expect("abort signal poisoned").clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abort_signal_can_be_reused_for_the_next_turn() {
        let signal = AbortSignal::new();
        signal.abort();
        assert!(signal.is_aborted());
        signal.reset();
        assert!(!signal.is_aborted());
    }

    #[tokio::test]
    async fn wait_aborted_wakes_up_on_abort() {
        let signal = AbortSignal::new();
        let waiter = signal.clone();
        let task = tokio::spawn(async move { waiter.wait_aborted().await });
        // 给等待方一个进入 await 的机会，再中止：旧实现要等下一次 50ms 轮询才醒
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        signal.abort();
        tokio::time::timeout(std::time::Duration::from_millis(200), task)
            .await
            .expect("abort 后等待方应立刻被唤醒")
            .expect("等待任务不应 panic");
    }

    #[tokio::test]
    async fn wait_aborted_returns_immediately_when_already_aborted() {
        let signal = AbortSignal::new();
        signal.abort();
        // 已经中止的信号不应再等一个轮询周期
        tokio::time::timeout(std::time::Duration::from_millis(20), signal.wait_aborted())
            .await
            .expect("已中止的等待应立刻返回");
    }

    #[tokio::test]
    async fn reset_makes_the_next_turn_abortable_again() {
        let signal = AbortSignal::new();
        signal.abort();
        signal.reset();
        let waiter = signal.clone();
        let task = tokio::spawn(async move { waiter.wait_aborted().await });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        signal.abort();
        tokio::time::timeout(std::time::Duration::from_millis(200), task)
            .await
            .expect("reset 之后的新一轮仍应能中止")
            .expect("等待任务不应 panic");
    }
}
