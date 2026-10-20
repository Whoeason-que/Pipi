//! 本机错误诊断：只记录响应元数据与流计数，不记录密钥、请求正文或模型输出。
use super::*;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub(super) struct RequestTrace(Arc<Mutex<Trace>>);

#[derive(Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Trace {
    api: Option<&'static str>,
    status: Option<u16>,
    request_id: Option<String>,
    cf_ray: Option<String>,
    text_chars: usize,
    thinking_chars: usize,
    completed_tool_calls: usize,
    unknown_frames: usize,
}

fn header_id(headers: &HeaderMap, name: &str) -> Option<String> {
    let value = headers.get(name)?.to_str().ok()?;
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte)))
    .then(|| value.to_string())
}

impl HttpMiddleware for RequestTrace {
    fn after_response<'a>(
        &'a self,
        _method: &'a http::Method,
        _uri: &'a http::Uri,
        status: http::StatusCode,
        headers: &'a HeaderMap,
    ) -> rig::wasm_compat::WasmBoxedFuture<'a, rig::http_client::Result<()>> {
        Box::pin(async move {
            if let Ok(mut trace) = self.0.lock() {
                trace.status = Some(status.as_u16());
                trace.request_id =
                    header_id(headers, "x-request-id").or_else(|| header_id(headers, "request-id"));
                trace.cf_ray = header_id(headers, "cf-ray");
            }
            Ok(())
        })
    }
}

impl RequestTrace {
    pub(super) fn set_api(&self, api: Api) {
        if let Ok(mut trace) = self.0.lock() {
            trace.api = Some(api.as_str());
        }
    }

    pub(super) fn event(&self, item: &RigItem<RigStreamEvent>) {
        if let Ok(mut trace) = self.0.lock() {
            match item {
                RigItem::Event(RigStreamEvent::Text { text, .. }) => {
                    trace.text_chars += text.chars().count()
                }
                RigItem::Event(RigStreamEvent::Reasoning { text, .. }) => {
                    trace.thinking_chars += text.chars().count()
                }
                RigItem::Event(RigStreamEvent::End {
                    content: AssistantContent::ToolCall(_),
                    ..
                }) => trace.completed_tool_calls += 1,
                RigItem::Unknown(_) => trace.unknown_frames += 1,
                _ => {}
            }
        }
    }

    pub(super) fn report(&self, model: &Model, error: &StreamError, started: Instant) {
        if let Ok(trace) = self.0.lock() {
            // URL 只取主机名，剥离用户名、密码、路径与 query 中可能出现的凭据。
            let host = reqwest::Url::parse(&model.base_url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_string));
            eprintln!(
                "pipi: 模型请求失败 {}",
                json!({
                    "model": model.id, "host": host, "elapsedMs": started.elapsed().as_millis(),
                    "retryable": error.retry.is_some(), "protocolMismatch": error.protocol_mismatch,
                    "response": &*trace,
                })
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_allowlisted_bounded_response_ids_are_kept() {
        let trace = RequestTrace::default();
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("x-request-id", "req-123"),
            ("cf-ray", "abc-SJC"),
            ("authorization", "Bearer secret"),
            ("set-cookie", "session=secret"),
        ] {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        trace
            .after_response(
                &http::Method::POST,
                &"https://example.test".parse().unwrap(),
                http::StatusCode::BAD_REQUEST,
                &headers,
            )
            .await
            .unwrap();
        let saved = serde_json::to_value(&*trace.0.lock().unwrap()).unwrap();
        assert_eq!(saved["status"], 400);
        assert_eq!(saved["requestId"], "req-123");
        assert_eq!(saved["cfRay"], "abc-SJC");
        assert!(!saved.to_string().contains("secret"));
        for invalid in ["x".repeat(129), "id with spaces".into()] {
            headers.insert("x-request-id", invalid.parse().unwrap());
            assert_eq!(header_id(&headers, "x-request-id"), None);
        }
    }
}
