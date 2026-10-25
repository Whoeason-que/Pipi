//! 标准 Web Push：库负责 RFC8291 加密和 VAPID 签名，HTTP 复用 reqwest。
//! 仅在用户显式订阅时创建密钥与本地文件，接收端不依赖浏览器在线。
use std::{sync::Mutex, time::Duration};

use base64::{Engine, prelude::BASE64_URL_SAFE_NO_PAD};
use p256::{
    SecretKey,
    elliptic_curve::{rand_core::OsRng, sec1::ToEncodedPoint},
};
use serde::{Deserialize, Serialize};
use web_push::{ContentEncoding, SubscriptionInfo, VapidSignatureBuilder, WebPushMessageBuilder};

use super::NotificationEnvelope;

mod storage;
use storage::{Store, load, save};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushSubscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub origin: String,
}

impl PushSubscription {
    fn validate(&self) -> Result<(), String> {
        validate_endpoint(&self.endpoint)?;
        if self.origin.len() > 2048 || self.p256dh.len() != 87 || self.auth.len() != 22 {
            return Err("Invalid push subscription fields".into());
        }
        let origin = reqwest::Url::parse(&self.origin).map_err(|_| "Invalid push origin")?;
        if origin.scheme() != "https"
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.host_str() == Some("localhost")
        {
            return Err("Push origin must be an HTTPS origin".into());
        }
        let public = BASE64_URL_SAFE_NO_PAD
            .decode(&self.p256dh)
            .map_err(|_| "Invalid push public key")?;
        if public.len() != 65 || p256::PublicKey::from_sec1_bytes(&public).is_err() {
            return Err("Invalid push public key".into());
        }
        if BASE64_URL_SAFE_NO_PAD
            .decode(&self.auth)
            .map_err(|_| "Invalid push auth secret")?
            .len()
            != 16
        {
            return Err("Invalid push auth secret".into());
        }
        Ok(())
    }
}

fn validate_endpoint(endpoint: &str) -> Result<(), String> {
    if endpoint.len() > 4096 {
        return Err("Push endpoint is too long".into());
    }
    let url = reqwest::Url::parse(endpoint).map_err(|_| "Invalid push endpoint")?;
    let host = url.host_str().unwrap_or("");
    // 订阅来自远程客户端；只允许实际浏览器推送服务，拒绝任意 URL/内网 SSRF。
    let trusted = host.ends_with(".push.apple.com")
        || host == "fcm.googleapis.com"
        || host == "updates.push.services.mozilla.com";
    if !trusted
        || url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("Unsupported push service endpoint".into());
    }
    Ok(())
}

pub struct PushService {
    gate: Mutex<()>,
    // 启动时绑定数据根，异步投递不重新读取进程环境。
    path: Option<std::path::PathBuf>,
}

impl Default for PushService {
    fn default() -> Self {
        Self {
            gate: Mutex::new(()),
            path: pipi_venv::av::paths::push_store_path(),
        }
    }
}

impl PushService {
    pub fn public_key(&self) -> Result<String, String> {
        let _guard = self.gate.lock().map_err(|e| e.to_string())?;
        let store = match load(self.path.as_deref())? {
            Some(store) => store,
            None => {
                let secret = SecretKey::random(&mut OsRng);
                let store = Store {
                    private_key: BASE64_URL_SAFE_NO_PAD.encode(secret.to_bytes()),
                    subscriptions: vec![],
                };
                save(self.path.as_deref(), &store)?;
                store
            }
        };
        let bytes = BASE64_URL_SAFE_NO_PAD
            .decode(&store.private_key)
            .map_err(|_| "Invalid VAPID key")?;
        let secret = SecretKey::from_slice(&bytes).map_err(|_| "Invalid VAPID key")?;
        Ok(BASE64_URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes()))
    }

    pub fn subscribe(&self, subscription: PushSubscription) -> Result<(), String> {
        subscription.validate()?;
        let _guard = self.gate.lock().map_err(|e| e.to_string())?;
        let mut store = load(self.path.as_deref())?
            .ok_or("Enable notifications before registering a subscription")?;
        store
            .subscriptions
            .retain(|item| item.endpoint != subscription.endpoint);
        if store.subscriptions.len() >= 32 {
            return Err("Too many notification devices (maximum 32)".into());
        }
        store.subscriptions.push(subscription);
        save(self.path.as_deref(), &store)
    }

    pub fn contains(&self, endpoint: &str) -> Result<bool, String> {
        let _guard = self.gate.lock().map_err(|e| e.to_string())?;
        Ok(load(self.path.as_deref())?.is_some_and(|store| {
            store
                .subscriptions
                .iter()
                .any(|item| item.endpoint == endpoint)
        }))
    }

    pub fn unsubscribe(&self, endpoint: &str) -> Result<(), String> {
        let _guard = self.gate.lock().map_err(|e| e.to_string())?;
        if let Some(mut store) = load(self.path.as_deref())? {
            store.subscriptions.retain(|item| item.endpoint != endpoint);
            save(self.path.as_deref(), &store)?;
        }
        Ok(())
    }

    pub async fn deliver(
        &self,
        notice: NotificationEnvelope,
        endpoint: Option<String>,
    ) -> Result<(), String> {
        let store = {
            let _guard = self.gate.lock().map_err(|e| e.to_string())?;
            load(self.path.as_deref())?
        };
        let Some(store) = store else {
            return Err("Notifications are not configured".into());
        };
        let testing = endpoint.is_some();
        let subscriptions: Vec<_> = store
            .subscriptions
            .into_iter()
            .filter(|subscription| {
                endpoint
                    .as_ref()
                    .is_none_or(|value| value == &subscription.endpoint)
            })
            .collect();
        if subscriptions.is_empty() {
            return if testing {
                Err("This device is not subscribed".into())
            } else {
                Ok(())
            };
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| "Cannot create push HTTP client")?;
        let mut tasks = tokio::task::JoinSet::new();
        for subscription in subscriptions {
            let client = client.clone();
            let private_key = store.private_key.clone();
            let notice = notice.clone();
            tasks.spawn(async move {
                let result = send(&client, &private_key, &subscription, &notice).await;
                (subscription.endpoint, result)
            });
        }
        let mut failure = None;
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((endpoint, Ok(true))) => {
                    self.unsubscribe(&endpoint)?;
                    if testing {
                        failure = Some(
                            "Push subscription expired. Reconnect notifications on this device."
                                .into(),
                        );
                    }
                }
                Ok((_, Ok(false))) => {}
                Ok((_, Err(error))) => failure = Some(error),
                Err(_) => failure = Some("Push task failed".into()),
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

async fn send(
    client: &reqwest::Client,
    key: &str,
    subscription: &PushSubscription,
    notice: &NotificationEnvelope,
) -> Result<bool, String> {
    let message = encrypted_message(key, subscription, notice)?;
    let request = web_push::request_builder::build_request::<Vec<u8>>(message);
    let (parts, body) = request.into_parts();
    let mut request = client.post(parts.uri.to_string()).body(body);
    for (name, value) in &parts.headers {
        request = request.header(name.as_str(), value.as_bytes());
    }
    let response = request
        .send()
        .await
        .map_err(|_| "Push service could not be reached")?;
    match response.status().as_u16() {
        200..=299 => Ok(false),
        404 | 410 => Ok(true),
        status => Err(format!("Push service returned HTTP {status}")),
    }
}

fn encrypted_message(
    key: &str,
    subscription: &PushSubscription,
    notice: &NotificationEnvelope,
) -> Result<web_push::WebPushMessage, String> {
    subscription.validate()?;
    let info = SubscriptionInfo::new(
        &subscription.endpoint,
        &subscription.p256dh,
        &subscription.auth,
    );
    let mut signature =
        VapidSignatureBuilder::from_base64(key, &info).map_err(|e| e.to_string())?;
    signature.add_claim("sub", subscription.origin.clone());
    let signature = signature.build().map_err(|e| e.to_string())?;
    let content = serde_json::to_vec(notice).map_err(|e| e.to_string())?;
    let mut message = WebPushMessageBuilder::new(&info);
    message.set_ttl(3600);
    message.set_vapid_signature(signature);
    message.set_payload(ContentEncoding::Aes128Gcm, &content);
    message.build().map_err(|e| e.to_string())
}

pub(crate) fn start(
    runtime: &tokio::runtime::Handle,
    events: &crate::events::EventHub,
    service: std::sync::Arc<PushService>,
) {
    let mut notifications = events.subscribe_notifications();
    let settings_path = pipi_venv::av::paths::settings_path();
    runtime.spawn(async move {
        loop {
            match notifications.recv().await {
                Ok(notice) => {
                    if !settings_path
                        .as_deref()
                        .map(crate::settings::load_settings_from)
                        .unwrap_or_default()
                        .notifications
                        .allows(notice.kind)
                    {
                        continue;
                    }
                    if let Err(error) = service.deliver(notice, None).await {
                        // 尚未订阅是正常状态，不为它输出错误。
                        if error != "Notifications are not configured" {
                            eprintln!("pipi: web push: {error}");
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    eprintln!("pipi: missed {count} web push notifications")
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_cannot_target_local_services_or_redirect_hosts() {
        for endpoint in [
            "http://localhost/",
            "https://127.0.0.1/",
            "https://push.apple.com.evil.test/",
            "https://fcm.googleapis.com:8443/a",
            "https://user@fcm.googleapis.com/a",
        ] {
            assert!(validate_endpoint(endpoint).is_err(), "{endpoint}");
        }
        assert!(validate_endpoint("https://web.push.apple.com/token").is_ok());
        assert!(validate_endpoint("https://fcm.googleapis.com/fcm/send/token").is_ok());
    }

    #[test]
    fn payload_is_encrypted_and_vapid_signature_matches_published_key() {
        use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
        let secret = SecretKey::random(&mut OsRng);
        let public = secret.public_key().to_encoded_point(false);
        let subscription = PushSubscription {
            endpoint: "https://web.push.apple.com/token".into(),
            p256dh: BASE64_URL_SAFE_NO_PAD.encode(public.as_bytes()),
            auth: BASE64_URL_SAFE_NO_PAD.encode([7u8; 16]),
            origin: "https://pipi.example.com".into(),
        };
        let notice = NotificationEnvelope::new(
            "fixture",
            "session",
            1,
            super::super::NotificationKind::Completed,
        );
        let message = encrypted_message(
            &BASE64_URL_SAFE_NO_PAD.encode(secret.to_bytes()),
            &subscription,
            &notice,
        )
        .unwrap();
        let request = web_push::request_builder::build_request::<Vec<u8>>(message);
        assert_eq!(request.headers()["content-encoding"], "aes128gcm");
        assert_eq!(request.headers()["ttl"], "3600");
        assert!(
            !request
                .body()
                .windows(notice.body.len())
                .any(|bytes| bytes == notice.body.as_bytes())
        );
        let authorization = request.headers()["authorization"].to_str().unwrap();
        let jwt = authorization
            .strip_prefix("vapid t=")
            .unwrap()
            .split(',')
            .next()
            .unwrap();
        let parts: Vec<_> = jwt.split('.').collect();
        let claims: serde_json::Value =
            serde_json::from_slice(&BASE64_URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://web.push.apple.com");
        assert_eq!(claims["sub"], subscription.origin);
        let signature =
            Signature::from_slice(&BASE64_URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
        VerifyingKey::from_sec1_bytes(public.as_bytes())
            .unwrap()
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .unwrap();
    }
}
