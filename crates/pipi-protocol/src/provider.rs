//! Provider 端口：发起流式请求、拿到事件流。
//!
//! 实现（rig 的 HTTP/SSE 适配与错误分类）在 `pipi-provider`；引擎与压缩流水线
//! 只依赖这个 trait 与 [`EventStream`]，不依赖任何 provider 实现 crate。
//!
//! `EventStream` 是 tokio mpsc 的接收端，所以契约层允许 tokio 的 **`sync`**
//! 特性（只要通道）；不得引入 `rt` / `net` / `time` —— 那会把运行时拖进协议层。

use tokio::sync::mpsc;

use crate::{AbortSignal, Context, Model, StreamEvent, StreamOptions};

/// provider 流事件接收端。channel 关闭（`recv()` 返回 None）表示请求被中止或
/// 异常断开。
pub type EventStream = mpsc::Receiver<StreamEvent>;

/// 一次模型调用的入口。实现只负责「发请求、把结果写成事件流」，不重试 ——
/// 是否重发由调用方（agent loop / 压缩流水线）按 `RetryHint` 决定。
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// 发起流式请求并立即返回事件接收端。HTTP/协议错误以 `StreamEvent::Error`
    /// 终止；正常结束以 `StreamEvent::Done` 终止。channel 关闭（None）表示
    /// 请求被中止或异常断开。
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
        abort: AbortSignal,
    ) -> EventStream;
}
