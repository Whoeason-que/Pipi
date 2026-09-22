//! 运行期协议补充与 `pipi-protocol` 的兼容 re-export。
//!
//! 持久化/IPC 数据类型已迁入 `pipi-protocol`；这里保留 Abort、流事件和请求
//! 选项等只在 core/provider/harness 内部有意义的运行期控制类型。旧调用点继续
//! 使用 `pipi_core::types::*`，避免一次性破坏外部 crate。

pub use pipi_protocol::{
    now_millis, AbortSignal, Api, ContentBlock, Context, ErrorEnvelope, Message, Model, StopReason,
    StreamEvent, StreamOptions, Tool, ToolResultContent, Usage,
};

#[cfg(test)]
mod tests {
    use super::AbortSignal;

    #[test]
    fn abort_signal_can_be_reset_for_next_turn() {
        let signal = AbortSignal::new();
        signal.abort();
        assert!(signal.is_aborted());

        signal.reset();

        assert!(!signal.is_aborted());
    }
}
