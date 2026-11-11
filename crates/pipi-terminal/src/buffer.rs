//! Bounded replay buffer for PTY output.
//!
//! The buffer keeps the newest chunks up to a byte budget. Readers address it
//! with a chunk cursor (`after_seq`), which makes reads non-destructive and
//! replayable after a panel remount. When the buffer drops old chunks, the
//! next read reports `truncated` so the client can reset its screen instead of
//! rendering a gap as if it were contiguous.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::Notify;

use crate::types::TerminalChunk;

pub(crate) const DEFAULT_MAX_BUFFER_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const READ_CHUNK_BYTES: usize = 64 * 1024;

pub(crate) struct OutputBuffer {
    inner: Mutex<Output>,
    notify: Notify,
}

struct Output {
    chunks: VecDeque<TerminalChunk>,
    bytes: usize,
    next_seq: u64,
    capacity: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct BufferSlice {
    pub chunks: Vec<TerminalChunk>,
    pub next_seq: u64,
    pub truncated: bool,
}

impl OutputBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Output {
                chunks: VecDeque::new(),
                bytes: 0,
                next_seq: 0,
                capacity: capacity.max(1),
            }),
            notify: Notify::new(),
        }
    }

    pub fn push(&self, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        {
            let mut output = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            let seq = output.next_seq;
            output.next_seq += 1;
            output.bytes += data.len();
            output.chunks.push_back(TerminalChunk { seq, data });
            // Keep at least one chunk even when a single read exceeds the
            // budget; otherwise a huge paste could empty the buffer forever.
            while output.bytes > output.capacity && output.chunks.len() > 1 {
                if let Some(chunk) = output.chunks.pop_front() {
                    output.bytes -= chunk.data.len();
                }
            }
        }
        self.notify.notify_waiters();
    }

    /// Wake waiters after a status change (process exit or shutdown).
    pub fn wake(&self) {
        self.notify.notify_waiters();
    }

    pub fn read(&self, after_seq: u64, max_bytes: usize) -> BufferSlice {
        let output = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let first_seq = output
            .chunks
            .front()
            .map(|chunk| chunk.seq)
            .unwrap_or(output.next_seq);
        let truncated = after_seq < first_seq;
        let start_seq = after_seq.max(first_seq);
        let start = start_seq.saturating_sub(first_seq) as usize;
        if start >= output.chunks.len() {
            let next_seq = if truncated {
                output.next_seq
            } else {
                after_seq
            };
            return BufferSlice {
                chunks: Vec::new(),
                next_seq,
                truncated,
            };
        }

        let max_bytes = max_bytes.max(1);
        let mut chunks = Vec::new();
        let mut bytes = 0usize;
        let mut next_seq = after_seq;
        for chunk in output.chunks.iter().skip(start) {
            // Always return the first chunk, even if it alone exceeds the
            // requested budget; otherwise the cursor could never advance.
            if !chunks.is_empty() && bytes + chunk.data.len() > max_bytes {
                break;
            }
            bytes += chunk.data.len();
            next_seq = chunk.seq + 1;
            chunks.push(chunk.clone());
        }
        BufferSlice {
            chunks,
            next_seq,
            truncated,
        }
    }

    /// Wait until new output arrives or `wait_ms` elapses. The notification
    /// future is registered before the buffer is checked so a push racing the
    /// check cannot be missed.
    pub async fn read_async(&self, after_seq: u64, max_bytes: usize, wait_ms: u64) -> BufferSlice {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let slice = self.read(after_seq, max_bytes);
        if !slice.chunks.is_empty() || wait_ms == 0 {
            return slice;
        }
        let _ = tokio::time::timeout(Duration::from_millis(wait_ms), notified).await;
        self.read(after_seq, max_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(text: &str) -> Vec<u8> {
        text.as_bytes().to_vec()
    }

    #[test]
    fn cursor_reads_are_non_destructive_and_resume_from_next_seq() {
        let buffer = OutputBuffer::new(1024);
        buffer.push(data("one"));
        buffer.push(data("two"));

        let first = buffer.read(0, 1024);
        assert_eq!(first.next_seq, 2);
        assert!(!first.truncated);
        assert_eq!(first.chunks.len(), 2);
        assert_eq!(first.chunks[1].seq, 1);

        // Re-reading the same cursor returns the same bytes; another reader
        // must not steal output from the first one.
        assert_eq!(buffer.read(0, 1024).chunks, first.chunks);
        let second = buffer.read(1, 1024);
        assert_eq!(second.chunks.len(), 1);
        assert_eq!(second.chunks[0].data, data("two"));
        assert_eq!(second.next_seq, 2);

        let caught_up = buffer.read(2, 1024);
        assert!(caught_up.chunks.is_empty());
        assert_eq!(caught_up.next_seq, 2);
        assert!(!caught_up.truncated);
    }

    #[test]
    fn byte_budget_returns_complete_chunks_and_advances() {
        let buffer = OutputBuffer::new(1024);
        buffer.push(data("aaa"));
        buffer.push(data("bbb"));
        buffer.push(data("ccc"));

        let first = buffer.read(0, 4);
        assert_eq!(first.chunks.len(), 1);
        assert_eq!(first.chunks[0].data, data("aaa"));
        assert_eq!(first.next_seq, 1);

        let rest = buffer.read(first.next_seq, 1024);
        assert_eq!(rest.chunks.len(), 2);
        assert_eq!(rest.next_seq, 3);
    }

    #[test]
    fn dropped_chunks_report_truncation_and_skip_to_the_oldest_available() {
        let buffer = OutputBuffer::new(6);
        buffer.push(data("aaa"));
        buffer.push(data("bbb"));
        buffer.push(data("ccc"));

        let read = buffer.read(0, 1024);
        assert!(read.truncated);
        assert_eq!(read.chunks[0].data, data("bbb"));
        assert_eq!(read.chunks[0].seq, 1);
        assert_eq!(read.next_seq, 3);
    }

    #[test]
    fn empty_after_truncation_advances_to_the_end() {
        let buffer = OutputBuffer::new(3);
        buffer.push(data("aaa"));
        buffer.push(data("bbb")); // "aaa" is dropped, one chunk remains
        let read = buffer.read(0, 1024);
        assert!(read.truncated);
        assert_eq!(read.chunks[0].data, data("bbb"));
        assert_eq!(read.next_seq, 2);
    }

    #[test]
    fn one_oversized_chunk_is_still_returned_so_the_cursor_can_advance() {
        let buffer = OutputBuffer::new(1024);
        buffer.push(vec![7u8; 128]);
        let read = buffer.read(0, 1);
        assert_eq!(read.chunks.len(), 1);
        assert_eq!(read.next_seq, 1);
    }

    #[tokio::test]
    async fn long_poll_wakes_on_push() {
        let buffer = std::sync::Arc::new(OutputBuffer::new(1024));
        let writer = buffer.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            writer.push(data("late"));
        });
        let read = buffer.read_async(0, 1024, 500).await;
        assert_eq!(read.chunks.len(), 1);
        assert_eq!(read.chunks[0].data, data("late"));
    }

    #[tokio::test]
    async fn long_poll_times_out_without_output() {
        let buffer = OutputBuffer::new(1024);
        let started = std::time::Instant::now();
        let read = buffer.read_async(0, 1024, 30).await;
        assert!(read.chunks.is_empty());
        assert_eq!(read.next_seq, 0);
        assert!(started.elapsed() >= Duration::from_millis(20));
    }
}
