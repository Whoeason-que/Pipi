//! 模型目录：运行时从 models.dev 拉取 + 本地缓存。
//!
//! 为什么不再有生成物：模型清单、上下文窗口、推理标记每周都在变，手写/生成都要人去
//! 对账。上游 [models.dev](https://models.dev)（MIT）就是 opencode 用的那份目录，
//! 我们直接消费它，只保留两件**不能外包**的东西：
//!
//! 1. [`CURATION`]：收录哪些 provider、归到哪一组、显示成什么名字；
//! 2. baseUrl 与协议的**人工核实**结果 —— 上游给的是 AI SDK 语义的 baseUrl，
//!    未必满足 rig 的拼装要求（例：上游 DeepSeek 的 api 是 `https://api.deepseek.com`，
//!    没有版本段，而 rig 的 OpenAI 客户端会往后拼路径）。
//!
//! 缓存：`~/.pipi/cache/models.json`（我们的 wire 结构，自带 `fetchedAt`）。
//! 拉取失败时用缓存顶（`stale=true`）；没缓存就报错，由前端退化成手填模型 ID。

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use pipi_protocol::Api;

mod curation;

pub use curation::GROUPS;
use curation::{CURATION, Curated, MANUAL, ManualEntry, protocol_of};
#[cfg(test)]
use curation::{curated, opt};

/// 上游目录。
const REMOTE: &str = "https://models.dev/api.json";
/// 缓存的保鲜期：超过就尝试刷新（刷新失败仍用缓存）。
const TTL_SECS: i64 = 24 * 60 * 60;
/// 拉取超时。冷启动没缓存时用户要等这么久，别设太长；
/// 超时/失败会落到缓存或「手填」退化路径（`resolve_catalog`）。
const FETCH_TIMEOUT_SECS: u64 = 10;
/// 生产目录至少要有这么多家 —— 少了说明上游结构变了或 CURATION 被改坏。
const MIN_PROVIDERS: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: String,
    pub name: String,
    /// 上下文窗口（上游 limit.context）；缺失表示上游没给。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    /// 单次最大输出（上游 limit.output）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
    #[serde(default)]
    pub reasoning: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogProvider {
    pub id: String,
    pub name: String,
    pub api: Api,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_key: Option<String>,
    pub group: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 端点不含版本段时的官方出处说明（OpenAI 兼容协议的例外）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url_note: Option<String>,
    /// 本地运行时：端点用 http:// 即可，不参与 https / 版本段校验。
    #[serde(default)]
    pub local: bool,
    pub models: Vec<CatalogModel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalog {
    /// 上游快照时间（unix 秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<i64>,
    /// `models.dev` = 本次拉到；`cache` = 用本地缓存。
    pub source: String,
    /// 缓存已过期且本次刷新失败。
    #[serde(default)]
    pub stale: bool,
    pub providers: Vec<CatalogProvider>,
}

/// 本地运行时端点：http:// 也算合法，不参与 https / 版本段校验。
fn is_local_endpoint(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://localhost")
        || lower.starts_with("http://127.0.0.1")
        || lower.starts_with("http://[::1]")
}

fn strip_slash(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

/// OpenAI 兼容端点必须自带版本段（rig 会往后拼路径）：`/v1`、`/v1beta`、
/// `/compatible-mode/v1`、`/api/paas/v4` 都算；例外必须在 CURATION 里写 baseUrlNote。
fn has_version_segment(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while let Some(pos) = lower[i..].find("/v") {
        let mut j = i + pos + 2;
        let digits_start = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > digits_start {
            while j < bytes.len() && bytes[j].is_ascii_lowercase() {
                j += 1;
            }
            if j == bytes.len() || bytes[j] == b'/' {
                return true;
            }
        }
        i = i + pos + 2;
    }
    false
}

pub fn cache_path() -> Option<PathBuf> {
    av::paths::catalog_cache_path()
}

/// 缓存的默认刷新策略（纯函数，便于单测）。
fn is_fresh(fetched_at: i64, now: i64) -> bool {
    now >= fetched_at && now.saturating_sub(fetched_at) < TTL_SECS
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn read_cache() -> Option<ModelCatalog> {
    let text = std::fs::read_to_string(cache_path()?).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_cache(catalog: &ModelCatalog) -> Result<(), String> {
    let path = cache_path().ok_or("无法定位 ~/.pipi")?;
    let text = serde_json::to_string(catalog).map_err(|e| format!("序列化目录失败: {e}"))?;
    // 原子替换：进程中途崩溃也不会留下截断的缓存
    // （截断的 models.json 会被 read_cache 当作「没有缓存」，下次冷启动又要拉 4.6MB）。
    // tempfile 的唯一命名也避免了并发刷新时互相覆盖同一个 .tmp。
    pipi_tools::fs::write_atomic(&path, text).map_err(|e| format!("写入目录缓存失败: {e}"))
}

fn validate_endpoint(
    pid: &str,
    api: Api,
    base_url: &str,
    base_url_note: Option<&str>,
) -> Result<(), String> {
    if base_url.trim().is_empty() {
        return Err(format!("{pid} 的 baseUrl 为空"));
    }
    if is_local_endpoint(base_url) {
        return Ok(());
    }
    if !base_url.starts_with("https://") {
        return Err(format!(
            "{pid} 的端点既不是本地地址也不是 https：{base_url}"
        ));
    }
    if api == Api::OpenAICompletions && !has_version_segment(base_url) && base_url_note.is_none() {
        return Err(format!(
            "{pid} 的端点 {base_url} 不含版本段；若已核实官方文档确实如此，请补 baseUrlNote 说明来源"
        ));
    }
    Ok(())
}

/// 收录范围的不变量（与旧测试同义）：id 唯一、(协议, 端点) 唯一、分组合法、
/// 非本地必须有模型、envKey 形态（数字开头的例外必须带 note 说明）。
fn assert_invariants(catalog: &ModelCatalog, min_providers: usize) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    let mut endpoints = std::collections::HashSet::new();
    for provider in &catalog.providers {
        if !ids.insert(provider.id.clone()) {
            return Err(format!("provider id 重复：{}", provider.id));
        }
        let key = format!(
            "{}|{}",
            provider.api.as_str(),
            provider.base_url.to_ascii_lowercase()
        );
        if !endpoints.insert(key) {
            return Err(format!(
                "{} 的 (协议, 端点) 与另一家重复，模型列表会错配",
                provider.id
            ));
        }
        if !GROUPS.contains(&provider.group.as_str()) {
            return Err(format!(
                "{} 的分组 {} 不在 GROUPS 里",
                provider.id, provider.group
            ));
        }
        if !provider.local && provider.models.is_empty() {
            return Err(format!(
                "{} 没有任何支持工具调用的模型，应移出 CURATION",
                provider.id
            ));
        }
        if let Some(env_key) = &provider.env_key {
            let first_ok = env_key
                .chars()
                .next()
                .map(|ch| ch.is_ascii_alphanumeric())
                .unwrap_or(false);
            let all_ok = env_key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
            if !(first_ok && all_ok) && provider.note.is_none() {
                return Err(format!(
                    "{} 的 envKey 形态异常（{env_key}），需在 CURATION 里写 note 说明",
                    provider.id
                ));
            }
        }
    }
    if catalog.providers.len() < min_providers {
        return Err(format!(
            "收录的 provider 只有 {} 家（少于 {min_providers}），疑似上游结构变化",
            catalog.providers.len()
        ));
    }
    Ok(())
}

/// 上游 provider → 模型清单：只收 `tool_call` 为真的模型（不能工具调用的模型在
/// Pipi 里没有意义），**不再截断**（搜索交给 UI），保持上游顺序。
fn models_of(provider: &serde_json::Value) -> Vec<CatalogModel> {
    let Some(models) = provider.get("models").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let mut list = Vec::new();
    for model in models.values() {
        if model.get("tool_call").and_then(|v| v.as_bool()) != Some(true) {
            continue;
        }
        let Some(id) = model.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let limit = model.get("limit");
        list.push(CatalogModel {
            id: id.to_string(),
            name: model
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(id)
                .to_string(),
            context: limit
                .and_then(|l| l.get("context"))
                .and_then(|v| v.as_u64()),
            output: limit.and_then(|l| l.get("output")).and_then(|v| v.as_u64()),
            reasoning: model.get("reasoning").and_then(|v| v.as_bool()) == Some(true),
        });
    }
    list
}

fn build_with(
    curation: &[Curated],
    manual: &[ManualEntry],
    upstream: &serde_json::Value,
    fetched_at: i64,
    min_providers: usize,
) -> Result<ModelCatalog, String> {
    let map = upstream
        .as_object()
        .ok_or("上游目录不是对象（models.dev/api.json 结构变了？）")?;
    let mut providers: Vec<CatalogProvider> = Vec::new();
    // 上游漂移（改名、换协议、模型清零）只跳过这一家并记录，不拖垮整个目录；
    // 我们自己的表写错（端点不合规等）仍然硬失败 —— 那是必须当场修的 bug。
    let mut skipped: Vec<String> = Vec::new();

    for entry in curation {
        let provider = match map.get(entry.pid) {
            Some(provider) => provider,
            None => {
                skipped.push(format!(
                    "{} 已不在 models.dev 目录（上游改名或下线）",
                    entry.pid
                ));
                continue;
            }
        };
        let npm = provider.get("npm").and_then(|v| v.as_str()).unwrap_or("");
        let api = match entry.api.or_else(|| protocol_of(npm)) {
            Some(api) => api,
            None => {
                skipped.push(format!(
                    "{} 的协议 {npm} 不受支持（Pipi 只支持 anthropic-messages / openai-completions）",
                    entry.pid
                ));
                continue;
            }
        };
        let base_url = match entry.base_url.map(str::to_string).or_else(|| {
            provider
                .get("api")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }) {
            Some(base_url) => strip_slash(&base_url),
            None => {
                skipped.push(format!(
                    "{} 既没有 baseUrl 覆盖也没有上游 api 字段",
                    entry.pid
                ));
                continue;
            }
        };
        // 我们表里的端点必须自己合规，这是硬错误
        validate_endpoint(entry.pid, api, &base_url, entry.base_url_note)?;
        let models = models_of(provider);
        if models.is_empty() && !is_local_endpoint(&base_url) {
            skipped.push(format!("{} 在上游没有任何支持工具调用的模型", entry.pid));
            continue;
        }

        let env_key = entry
            .env_key
            .map(str::to_string)
            .or_else(|| {
                provider
                    .get("env")
                    .and_then(|v| v.as_array())
                    .and_then(|list| list.first())
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .filter(|key| !key.trim().is_empty());

        providers.push(CatalogProvider {
            id: entry.id.unwrap_or(entry.pid).to_string(),
            name: entry
                .name
                .map(str::to_string)
                .or_else(|| {
                    provider
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| entry.pid.to_string()),
            api,
            local: is_local_endpoint(&base_url),
            base_url,
            env_key,
            group: entry.group.to_string(),
            doc: entry.doc.map(str::to_string).or_else(|| {
                provider
                    .get("doc")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            }),
            note: entry.note.map(str::to_string),
            base_url_note: entry.base_url_note.map(str::to_string),
            models,
        });
    }

    if !skipped.is_empty() {
        eprintln!(
            "pipi: 目录里有 {} 家被跳过（上游变化，需人工核对 CURATION）：\n  - {}",
            skipped.len(),
            skipped.join("\n  - ")
        );
    }

    for item in manual {
        let base_url = strip_slash(item.base_url);
        validate_endpoint(item.id, Api::OpenAICompletions, &base_url, None)?;
        providers.push(CatalogProvider {
            id: item.id.to_string(),
            name: item.name.to_string(),
            api: Api::OpenAICompletions,
            base_url,
            env_key: Some(item.env_key.to_string()),
            group: "本地".to_string(),
            doc: Some(item.doc.to_string()),
            note: Some(item.note.to_string()),
            base_url_note: None,
            local: true,
            models: Vec::new(),
        });
    }

    let catalog = ModelCatalog {
        fetched_at: Some(fetched_at),
        source: "models.dev".to_string(),
        stale: false,
        providers,
    };
    assert_invariants(&catalog, min_providers)?;
    Ok(catalog)
}

/// 把上游 JSON 与 CURATION 合成我们的目录。纯函数（不联网），便于测试。
pub fn build_catalog(
    upstream: &serde_json::Value,
    fetched_at: i64,
) -> Result<ModelCatalog, String> {
    build_with(CURATION, MANUAL, upstream, fetched_at, MIN_PROVIDERS)
}

/// 共享 HTTP 客户端：连接池与 TLS 会话复用（原来每次拉取都新建 Client）。
/// 构建失败会连错误一起缓存：初始化失败的成因（TLS 后端缺失等）不会自愈。
fn http_client() -> Result<reqwest::Client, String> {
    static CLIENT: std::sync::OnceLock<Result<reqwest::Client, String>> =
        std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
                .build()
                .map_err(|e| format!("HTTP 客户端初始化失败: {e}"))
        })
        .clone()
}

async fn fetch_upstream() -> Result<serde_json::Value, String> {
    let client = http_client()?;
    let response = client
        .get(REMOTE)
        .send()
        .await
        .map_err(|e| format!("拉取 {REMOTE} 失败: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("拉取 {REMOTE} 失败：HTTP {}", response.status()));
    }
    response
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("解析上游目录失败: {e}"))
}

/// 回落决策（纯函数）：刷新成功就用新结果；刷新失败退回缓存并标 stale；
/// 两者都没有则把「退化为手填」写进错误里。
fn resolve_catalog(
    cached: Option<ModelCatalog>,
    fetched: Result<ModelCatalog, String>,
) -> Result<ModelCatalog, String> {
    match fetched {
        Ok(catalog) => Ok(catalog),
        Err(error) => match cached {
            Some(mut catalog) => {
                catalog.source = "cache".to_string();
                catalog.stale = true;
                Ok(catalog)
            }
            None => Err(format!("{error}（且没有本地缓存，请退化为手填模型 ID）")),
        },
    }
}

/// 目录刷新的单飞锁：桌面壳与 Web 服务可能同时触发刷新（冷启动 + 用户点刷新），
/// 没有它每个调用方都会各拉一份 4.6MB。持锁期间后来者看到的是刚写好的缓存。
static CATALOG_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 取目录：缓存优先（未过期直接用），刷新失败退回缓存，两者都没有则报错。
///
/// `source` 语义：`models.dev` = 本次联网刷新的结果；`cache` = 读的是本地缓存
/// （此时 `stale=true` 表示缓存已过期且本次刷新失败）。
pub async fn load_catalog(refresh: bool) -> Result<ModelCatalog, String> {
    let _single_flight = CATALOG_LOCK.lock().await;
    let cached = read_cache();
    if !refresh
        && let Some(catalog) = &cached
        && catalog
            .fetched_at
            .map(|at| is_fresh(at, now_secs()))
            .unwrap_or(false)
    {
        let mut hit = catalog.clone();
        hit.source = "cache".to_string();
        hit.stale = false;
        return Ok(hit);
    }
    let fetched = fetch_upstream()
        .await
        .and_then(|raw| build_catalog(&raw, now_secs()));
    let resolved = resolve_catalog(cached, fetched)?;
    if resolved.source == "models.dev"
        && let Err(error) = write_cache(&resolved)
    {
        eprintln!("pipi: 写入模型目录缓存失败：{error}");
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests;
