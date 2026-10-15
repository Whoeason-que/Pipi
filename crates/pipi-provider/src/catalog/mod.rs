//! 模型目录：运行时从 models.dev 拉取 + 本地缓存。
//!
//! 为什么不再有生成物：模型清单、上下文窗口、推理标记每周都在变，手写/生成都要人去
//! 对账。上游 [models.dev](https://models.dev)（MIT）就是 opencode 用的那份目录，
//! 我们直接消费它，只保留两件**不能外包**的东西：
//!
//! 1. [`CURATION`]：收录哪些 provider、归到哪一组、显示成什么名字；
//! 2. baseUrl 与 provider 级协议的**人工核实**结果 —— 上游给的是 AI SDK 语义的
//!    baseUrl，未必满足 rig 的拼装要求（例：上游 DeepSeek 的 api 是
//!    `https://api.deepseek.com`，没有版本段，而 rig 的 OpenAI 客户端会往后拼路径）。
//!
//! 协议有两层：provider 级（上面的核实结果）与**模型级**覆盖 —— 上游
//! `model.provider.npm` 与 provider 级不同时，说明「这个模型只走另一套 wire」
//! （例：opencode zen/go 的 Claude 系只有 Anthropic Messages，其余模型只有
//! OpenAI 兼容）。模型级映射不到 Pipi 支持的两种协议（如 `@ai-sdk/google`）的
//! 模型直接跳过：宁可漏收录，也不猜协议。
//!
//! 缓存：`~/.pipi/cache/models.json`（我们的 wire 结构，自带 `fetchedAt`）。
//! 拉取失败时用缓存顶（`stale=true`）；没缓存就报错，由前端退化成手填模型 ID。
//!
//! **缓存路径由宿主注入**（`load_catalog(refresh, cache_path)`）：路径布局是
//! `pipi_venv::av::paths` 的职责，provider 层不该反向依赖它。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use pipi_protocol::Api;

mod curation;

use curation::{CURATION, Curated, GROUPS, MANUAL, ManualEntry, protocol_of};
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
    /// 模型级协议覆盖（上游 `model.provider.npm` 与 provider 级不同时填写）。
    /// `None` = 与 provider 级相同，按 provider 的协议请求。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<Api>,
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

fn write_cache(cache_path: Option<&Path>, catalog: &ModelCatalog) -> Result<(), String> {
    let path = cache_path.ok_or("无法定位 ~/.pipi")?;
    let text = serde_json::to_string(catalog).map_err(|e| format!("序列化目录失败: {e}"))?;
    // 原子替换：进程中途崩溃也不会留下截断的缓存
    // （截断的 models.json 会被 cached_catalog 当作「没有缓存」，下次冷启动又要拉 4.6MB）。
    // tempfile 的唯一命名也避免了并发刷新时互相覆盖同一个 .tmp。
    pipi_protocol::fs::write_atomic(path, text).map_err(|e| format!("写入目录缓存失败: {e}"))
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
///
/// 协议取两层：模型级 `provider.npm` 优先（与 provider 级不同时记进
/// [`CatalogModel::api`]）；上游说这个模型只走 Pipi 不支持的协议（如
/// `@ai-sdk/google`）时跳过该模型 —— 宁可漏收录，也不猜协议。
fn models_of(provider: &serde_json::Value, provider_api: Api) -> Vec<CatalogModel> {
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
        let api = match model
            .get("provider")
            .and_then(|v| v.get("npm"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|npm| !npm.is_empty())
        {
            None => None,
            Some(npm) => match protocol_of(npm) {
                Some(model_api) if model_api != provider_api => Some(model_api),
                Some(_) => None,
                None => continue,
            },
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
            api,
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
        let models = models_of(provider, api);
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
pub(crate) fn build_catalog(
    upstream: &serde_json::Value,
    fetched_at: i64,
) -> Result<ModelCatalog, String> {
    build_with(CURATION, MANUAL, upstream, fetched_at, MIN_PROVIDERS)
}

// ---------------------------------------------------------------------------
// 协议解析：宿主在发起请求前用这三件套把「绑定的协议」修正成「该模型该用的协议」
// ---------------------------------------------------------------------------

/// 只读本地缓存（**不联网、不判保鲜期**）：宿主在 run 前解析模型级协议用。
/// 需要刷新走 [`load_catalog`]；没有缓存返回 `None`（调用方按原协议继续）。
pub fn cached_catalog(cache_path: Option<&Path>) -> Option<ModelCatalog> {
    let text = std::fs::read_to_string(cache_path?).ok()?;
    serde_json::from_str(&text).ok()
}

/// 端点归一化键：去尾斜杠 + 忽略大小写（上游与用户手填的写法可能不同）。
/// 同时折叠 OpenCode 官方端点 (/zen/) 与 models.dev / 控制台推理端点 (/inference/)，
/// 确保两套等价路径均能稳定命中同一条目录记录。
/// provider 层的协议记忆（[`crate::learned_protocol`]）用同一个口径。
pub(crate) fn endpoint_key(url: &str) -> String {
    let lower = strip_slash(url).to_ascii_lowercase();
    if lower.starts_with("https://opencode.ai/inference/") {
        lower.replacen("/inference/", "/zen/", 1)
    } else {
        lower
    }
}

/// 规范化端点地址：去除尾斜杠并转为小写。
pub fn normalize_endpoint(url: &str) -> String {
    endpoint_key(url)
}

/// 目录里同一端点的全部条目（同一端点可能有多条：协议不同，如 deepseek 与
/// deepseek-anthropic）。`declared` 命中协议的那条优先 —— 模型级覆盖是相对
/// 它自己的 provider 说的。
fn providers_at<'a>(
    catalog: &'a ModelCatalog,
    base_url: &str,
    declared: Api,
) -> Vec<&'a CatalogProvider> {
    let key = endpoint_key(base_url);
    let mut hits: Vec<_> = catalog
        .providers
        .iter()
        .filter(|p| endpoint_key(&p.base_url) == key)
        .collect();
    hits.sort_by_key(|p| p.api != declared);
    hits
}

/// 该模型在目录里的模型级协议（`None` = 上游没给覆盖）。
///
/// 只看**最贴近的那条端点条目**（同端点多条时按声明协议选）：跨条目拼答案会把
/// 两家的话混在一起。目录里根本没收录这个模型时同样返回 `None` —— 调用方保持
/// 绑定里的协议（覆盖用户手填的目录外模型与本地运行时）。
pub(crate) fn model_api_override(
    catalog: &ModelCatalog,
    base_url: &str,
    model_id: &str,
    declared: Api,
) -> Option<Api> {
    let provider = providers_at(catalog, base_url, declared)
        .into_iter()
        .next()?;
    provider
        .models
        .iter()
        .find(|model| model.id == model_id)
        .and_then(|model| model.api)
}

/// 端点在目录里的 provider 级协议（跟随上游 / CURATION 的更新）。
pub(crate) fn catalog_provider_api(
    catalog: &ModelCatalog,
    base_url: &str,
    declared: Api,
) -> Option<Api> {
    providers_at(catalog, base_url, declared)
        .first()
        .map(|provider| provider.api)
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
pub async fn load_catalog(
    refresh: bool,
    cache_path: Option<PathBuf>,
) -> Result<ModelCatalog, String> {
    let _single_flight = CATALOG_LOCK.lock().await;
    let cached = cached_catalog(cache_path.as_deref());
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
        && let Err(error) = write_cache(cache_path.as_deref(), &resolved)
    {
        eprintln!("pipi: 写入模型目录缓存失败：{error}");
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn upstream_fixture() -> serde_json::Value {
        json!({
            "deepseek": {
                "id": "deepseek", "name": "DeepSeek", "npm": "@ai-sdk/openai-compatible",
                "api": "https://api.deepseek.com", "env": ["DEEPSEEK_API_KEY"],
                "models": {
                    "deepseek-chat": {
                        "id": "deepseek-chat", "name": "DeepSeek Chat", "tool_call": true,
                        "reasoning": false, "limit": { "context": 128000, "output": 8192 }
                    },
                    "deepseek-reasoner": {
                        "id": "deepseek-reasoner", "name": "DeepSeek Reasoner", "tool_call": true,
                        "reasoning": true, "limit": { "context": 128000, "output": 65536 }
                    },
                    "deepseek-no-tools": { "id": "deepseek-no-tools", "tool_call": false }
                }
            },
            "anthropic": {
                "id": "anthropic", "name": "Anthropic", "npm": "@ai-sdk/anthropic",
                "env": ["ANTHROPIC_API_KEY"],
                "models": {
                    "claude-sonnet-4-6": {
                        "id": "claude-sonnet-4-6", "name": "Claude Sonnet 4.6", "tool_call": true,
                        "reasoning": true, "limit": { "context": 1000000, "output": 128000 }
                    }
                }
            },
            "gateway": {
                "id": "gateway", "name": "Gateway", "npm": "@ai-sdk/openai-compatible",
                "api": "https://gateway.example.com/v1", "env": ["GATEWAY_API_KEY"],
                "models": {
                    "mixed-chat": { "id": "mixed-chat", "name": "Mixed Chat", "tool_call": true },
                    "mixed-claude": {
                        "id": "mixed-claude", "name": "Mixed Claude", "tool_call": true,
                        "provider": { "npm": "@ai-sdk/anthropic" }
                    },
                    "mixed-same": {
                        "id": "mixed-same", "name": "Mixed Same", "tool_call": true,
                        "provider": { "npm": "@ai-sdk/openai-compatible" }
                    },
                    "mixed-google": {
                        "id": "mixed-google", "name": "Mixed Google", "tool_call": true,
                        "provider": { "npm": "@ai-sdk/google" }
                    }
                }
            }
        })
    }

    /// 测试用的收录子集：只覆盖 fixture 里出现过的 provider（生产表另有不变量测试）。
    /// gateway 收了两条同端点的条目（协议不同），用来钉住「同端点优先按声明协议反查」。
    const TEST_CURATION: &[Curated] = &[
        curated!("deepseek", "国内", base_url = "https://api.deepseek.com/v1"),
        curated!(
            "deepseek",
            "国内",
            id = "deepseek-anthropic",
            name = "DeepSeek（Anthropic 兼容）",
            api = Api::AnthropicMessages,
            base_url = "https://api.deepseek.com/anthropic"
        ),
        curated!("anthropic", "国际", base_url = "https://api.anthropic.com"),
        curated!("gateway", "国际"),
        curated!(
            "gateway",
            "国际",
            id = "gateway-anthropic",
            name = "Gateway（Anthropic 兼容）",
            api = Api::AnthropicMessages
        ),
    ];

    const TEST_MANUAL: &[ManualEntry] = &[ManualEntry {
        id: "ollama",
        name: "Ollama（本地）",
        base_url: "http://localhost:11434/v1",
        env_key: "OLLAMA_API_KEY",
        doc: "https://docs.ollama.com/api/openai-compatibility",
        note: "本地服务：密钥填任意非空值",
    }];

    fn fixture_catalog() -> ModelCatalog {
        build_with(TEST_CURATION, TEST_MANUAL, &upstream_fixture(), 0, 3).expect("应能构建目录")
    }

    fn find<'a>(catalog: &'a ModelCatalog, id: &str) -> &'a CatalogProvider {
        catalog
            .providers
            .iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("provider 缺失：{id}"))
    }

    #[test]
    fn curation_覆盖上游端点并只收工具模型() {
        let catalog = fixture_catalog();
        let deepseek = find(&catalog, "deepseek");
        assert_eq!(deepseek.api, Api::OpenAICompletions);
        // 上游给的是无版本段的 https://api.deepseek.com，必须被 CURATION 覆盖
        assert_eq!(deepseek.base_url, "https://api.deepseek.com/v1");
        assert_eq!(deepseek.env_key.as_deref(), Some("DEEPSEEK_API_KEY"));
        assert_eq!(deepseek.group, "国内");
        let ids: Vec<_> = deepseek.models.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"deepseek-chat"));
        assert!(ids.contains(&"deepseek-reasoner"));
        assert!(
            !ids.contains(&"deepseek-no-tools"),
            "不能工具调用的模型不入列"
        );
        let reasoner = deepseek
            .models
            .iter()
            .find(|m| m.id == "deepseek-reasoner")
            .unwrap();
        assert_eq!(reasoner.context, Some(128000));
        assert_eq!(reasoner.output, Some(65536));
        assert!(reasoner.reasoning);
        assert!(!find(&catalog, "deepseek").local);
    }

    #[test]
    fn 同一家的第二种协议是独立条目() {
        let catalog = fixture_catalog();
        let compat = find(&catalog, "deepseek-anthropic");
        assert_eq!(compat.api, Api::AnthropicMessages);
        assert_eq!(compat.base_url, "https://api.deepseek.com/anthropic");
        // Anthropic 兼容端点提供同一批模型，所以模型清单与 openai 那条一致
        let plain = find(&catalog, "deepseek");
        assert_eq!(compat.models, plain.models);
        // 但 (协议, 端点) 不同，不会被不变量判成重复
        assert_ne!(compat.base_url, plain.base_url);
    }

    #[test]
    fn 模型级协议覆盖只在与_provider_级不同时入列() {
        let catalog = fixture_catalog();
        let gateway = find(&catalog, "gateway");
        assert_eq!(gateway.api, Api::OpenAICompletions);

        let claude = gateway
            .models
            .iter()
            .find(|m| m.id == "mixed-claude")
            .unwrap();
        assert_eq!(
            claude.api,
            Some(Api::AnthropicMessages),
            "上游模型级 npm 与 provider 级不同：记成模型级覆盖"
        );
        // 无覆盖的模型不写字段（前端/缓存里的形状保持最小）
        let chat_json = serde_json::to_string(
            gateway
                .models
                .iter()
                .find(|m| m.id == "mixed-chat")
                .unwrap(),
        )
        .unwrap();
        assert!(!chat_json.contains("\"api\""), "{chat_json}");

        let same = gateway
            .models
            .iter()
            .find(|m| m.id == "mixed-same")
            .unwrap();
        assert_eq!(same.api, None, "与 provider 级相同不必重复记录");
        assert!(
            !gateway.models.iter().any(|m| m.id == "mixed-google"),
            "上游说这个模型走 Pipi 不支持的协议：跳过，不猜"
        );
    }

    #[test]
    fn 有效协议按模型级覆盖解析且端点归一化() {
        let catalog = fixture_catalog();
        let endpoint = "https://gateway.example.com/v1";

        assert_eq!(
            model_api_override(&catalog, endpoint, "mixed-claude", Api::OpenAICompletions),
            Some(Api::AnthropicMessages)
        );
        assert_eq!(
            model_api_override(&catalog, endpoint, "mixed-chat", Api::OpenAICompletions),
            None
        );
        assert_eq!(
            model_api_override(&catalog, endpoint, "no-such-model", Api::OpenAICompletions),
            None,
            "目录外的模型（手填 ID / 本地运行时）保持绑定里的协议"
        );
        // 尾斜杠与大小写不影响命中
        assert_eq!(
            model_api_override(
                &catalog,
                "https://Gateway.Example.com/v1/",
                "mixed-claude",
                Api::OpenAICompletions
            ),
            Some(Api::AnthropicMessages)
        );
        // 同端点两条条目：按声明协议选中的那条先看（mixed-claude 在 anthropic 条目里
        // 无需覆盖，因为它的 provider 级协议已经是 anthropic）
        assert_eq!(
            model_api_override(&catalog, endpoint, "mixed-claude", Api::AnthropicMessages),
            None
        );
        assert_eq!(
            catalog_provider_api(&catalog, endpoint, Api::OpenAICompletions),
            Some(Api::OpenAICompletions)
        );
        assert_eq!(
            catalog_provider_api(&catalog, endpoint, Api::AnthropicMessages),
            Some(Api::AnthropicMessages)
        );
        assert_eq!(
            catalog_provider_api(
                &catalog,
                "https://unknown.example.com/v1",
                Api::OpenAICompletions
            ),
            None
        );
    }

    #[test]
    fn opencode端点别名折叠_inference与zen互为别名() {
        assert_eq!(
            endpoint_key("https://opencode.ai/inference/go/v1"),
            endpoint_key("https://opencode.ai/zen/go/v1")
        );
        assert_eq!(
            endpoint_key("https://opencode.ai/inference/v1/"),
            endpoint_key("https://opencode.ai/zen/v1")
        );
    }

    #[test]
    fn 本地运行时是手写条目且不参与版本段校验() {
        let catalog = fixture_catalog();
        let ollama = find(&catalog, "ollama");
        assert_eq!(ollama.base_url, "http://localhost:11434/v1");
        assert_eq!(ollama.group, "本地");
        assert!(ollama.local);
        assert!(ollama.models.is_empty(), "本地运行时模型名由用户手填");
    }

    #[test]
    fn 不带版本段的_openai_兼容端点必须报错() {
        let err = validate_endpoint(
            "deepseek",
            Api::OpenAICompletions,
            "https://api.deepseek.com",
            None,
        )
        .unwrap_err();
        assert!(err.contains("版本段"), "{err}");
        // 有 baseUrlNote 说明出处则放行（novita 的实测形态）
        assert!(
            validate_endpoint(
                "novita-ai",
                Api::OpenAICompletions,
                "https://api.novita.ai/openai",
                Some("官方 llms.txt")
            )
            .is_ok()
        );
        // anthropic 协议不看版本段
        assert!(
            validate_endpoint(
                "anthropic",
                Api::AnthropicMessages,
                "https://api.anthropic.com",
                None
            )
            .is_ok()
        );
    }

    #[test]
    fn 非本地端点必须是_https() {
        assert!(
            validate_endpoint(
                "x",
                Api::OpenAICompletions,
                "http://api.example.com/v1",
                None
            )
            .is_err()
        );
        assert!(
            validate_endpoint(
                "x",
                Api::OpenAICompletions,
                "https://api.example.com/v1",
                None
            )
            .is_ok()
        );
    }

    #[test]
    fn 版本段识别覆盖常见写法() {
        for ok in [
            "https://api.openai.com/v1",
            "https://generativelanguage.googleapis.com/v1beta",
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "https://open.bigmodel.cn/api/paas/v4",
            "https://api.x.ai/v1",
        ] {
            assert!(has_version_segment(ok), "{ok} 应识别为含版本段");
        }
        for bad in [
            "https://api.deepseek.com",
            "https://api.novita.ai/openai",
            "https://x.com/v",
            "https://x.com/version1",
        ] {
            assert!(!has_version_segment(bad), "{bad} 不应识别为含版本段");
        }
    }

    #[test]
    fn 上游漂移只跳过那一家_其余照常可用() {
        // 协议不再受支持 → 跳过该家，但不影响别人
        let mut upstream = upstream_fixture();
        upstream["deepseek"]["npm"] = json!("@ai-sdk/google");
        let curation = &[
            curated!("deepseek", "国内"),
            curated!("anthropic", "国际", base_url = "https://api.anthropic.com"),
        ];
        let catalog = build_with(curation, &[], &upstream, 0, 1).expect("不该整体失败");
        assert_eq!(catalog.providers.len(), 1, "只有 anthropic 应留下");
        assert_eq!(catalog.providers[0].id, "anthropic");

        // provider 从上游消失 → 同样只跳过
        let curation = &[
            curated!("not-there", "国内"),
            curated!("anthropic", "国际", base_url = "https://api.anthropic.com"),
        ];
        let catalog = build_with(curation, &[], &upstream_fixture(), 0, 1).expect("不该整体失败");
        assert_eq!(catalog.providers.len(), 1);

        // 上游把工具模型清零 → 跳过（本地运行时除外，见下一个测试）
        let empty = json!({
            "google": { "id": "google", "name": "Google", "npm": "@ai-sdk/openai",
                        "models": { "x": { "id": "x", "tool_call": false } } },
            "anthropic": upstream_fixture()["anthropic"].clone(),
        });
        let curation = &[
            curated!(
                "google",
                "国际",
                base_url = "https://generativelanguage.googleapis.com/v1beta/openai"
            ),
            curated!("anthropic", "国际", base_url = "https://api.anthropic.com"),
        ];
        let catalog = build_with(curation, &[], &empty, 0, 1).expect("不该整体失败");
        assert_eq!(catalog.providers.len(), 1);
        assert_eq!(catalog.providers[0].id, "anthropic");
    }

    #[test]
    fn 自家表写错仍然硬失败() {
        // 端点不含版本段且无 baseUrlNote → 必须当场失败，不能静默跳过
        let curation = &[
            curated!("deepseek", "国内", base_url = "https://api.deepseek.com"),
            curated!("anthropic", "国际", base_url = "https://api.anthropic.com"),
        ];
        let err = build_with(curation, &[], &upstream_fixture(), 0, 1).unwrap_err();
        assert!(err.contains("版本段"), "{err}");

        // 分组写错 → 不变量拦截
        let curation = &[curated!(
            "deepseek",
            "海外",
            base_url = "https://api.deepseek.com/v1"
        )];
        let err = build_with(curation, &[], &upstream_fixture(), 0, 1).unwrap_err();
        assert!(err.contains("不在 GROUPS"), "{err}");
    }

    #[test]
    fn 不变量能挡住重复端点与分组写错() {
        // (协议, 端点) 重复：把 anthropic 也指到 deepseek 的 OpenAI 兼容端点
        let dup = &[
            curated!("deepseek", "国内", base_url = "https://api.deepseek.com/v1"),
            curated!(
                "anthropic",
                "国内",
                api = Api::OpenAICompletions,
                base_url = "https://api.deepseek.com/v1"
            ),
        ];
        let err = build_with(dup, &[], &upstream_fixture(), 0, 1).unwrap_err();
        assert!(err.contains("重复"), "{err}");

        // 分组写错
        let bad_group = &[curated!(
            "deepseek",
            "海外",
            base_url = "https://api.deepseek.com/v1"
        )];
        let err = build_with(bad_group, &[], &upstream_fixture(), 0, 1).unwrap_err();
        assert!(err.contains("不在 GROUPS"), "{err}");
    }

    #[test]
    fn 缓存保鲜期按_ttl_判断() {
        assert!(is_fresh(1_000, 1_000));
        assert!(is_fresh(1_000, 1_000 + TTL_SECS - 1));
        assert!(!is_fresh(1_000, 1_000 + TTL_SECS));
        assert!(!is_fresh(1_000, 999), "时钟回拨视为过期");
    }

    #[test]
    fn 离线回落_有缓存就用缓存并标_stale() {
        let fresh = fixture_catalog();
        let resolved = resolve_catalog(Some(fresh.clone()), Err("断网".into())).unwrap();
        assert_eq!(resolved.source, "cache");
        assert!(resolved.stale);
        assert_eq!(resolved.providers, fresh.providers, "内容必须原样可用");

        // 刷新成功时以新结果为准（不标 stale）
        let resolved = resolve_catalog(Some(fresh.clone()), Ok(fresh.clone())).unwrap();
        assert_eq!(resolved.source, "models.dev");
        assert!(!resolved.stale);
    }

    #[test]
    fn 离线回落_没有任何缓存时提示退化为手填() {
        let err = resolve_catalog(None, Err("拉取失败".into())).unwrap_err();
        assert!(err.contains("拉取失败"), "{err}");
        assert!(err.contains("手填"), "前端要靠这句判断退化路径：{err}");
    }

    #[test]
    fn wire_结构保持驼峰字段命名() {
        let catalog = fixture_catalog();
        let text = serde_json::to_string(&catalog).unwrap();
        assert!(text.contains("\"baseUrl\""), "前端按 camelCase 读：{text}");
        assert!(text.contains("\"fetchedAt\""));
        let parsed: ModelCatalog = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, catalog);
    }

    /// 真实上游数据：`cargo test -p pipi-provider -- --ignored`（默认不跑，避免依赖网络）。
    #[tokio::test]
    #[ignore]
    async fn 真实上游目录可构建且满足不变量() {
        let raw = fetch_upstream().await.expect("应能拉到 models.dev");
        let catalog = build_catalog(&raw, now_secs()).expect("应能构建目录");
        assert!(catalog.providers.len() >= MIN_PROVIDERS);
        assert!(
            catalog
                .providers
                .iter()
                .all(|p| !p.models.is_empty() || p.local)
        );

        // 钉住几家关键 provider 的协议与端点：上游静默漂移（改名/换协议/换端点）时
        // 这个测试会红，而不是让用户少看到几家。
        let pinned: &[(&str, Api, &str, bool)] = &[
            (
                "deepseek",
                Api::OpenAICompletions,
                "https://api.deepseek.com/v1",
                false,
            ),
            (
                "deepseek-anthropic",
                Api::AnthropicMessages,
                "https://api.deepseek.com/anthropic",
                false,
            ),
            (
                "anthropic",
                Api::AnthropicMessages,
                "https://api.anthropic.com",
                false,
            ),
            (
                "openai",
                Api::OpenAICompletions,
                "https://api.openai.com/v1",
                false,
            ),
            (
                "google",
                Api::OpenAICompletions,
                "https://generativelanguage.googleapis.com/v1beta/openai",
                false,
            ),
            (
                "kimi-code-plan-cn",
                Api::OpenAICompletions,
                "https://api.kimi.com/coding/v1",
                false,
            ),
            (
                "minimax-cn",
                Api::AnthropicMessages,
                "https://api.minimax.cn/anthropic/v1",
                false,
            ),
            (
                "zhipuai",
                Api::OpenAICompletions,
                "https://open.bigmodel.cn/api/paas/v4",
                false,
            ),
            (
                "novita-ai",
                Api::OpenAICompletions,
                "https://api.novita.ai/openai",
                false,
            ),
            (
                "ollama",
                Api::OpenAICompletions,
                "http://localhost:11434/v1",
                true,
            ),
            (
                "vllm",
                Api::OpenAICompletions,
                "http://localhost:8000/v1",
                true,
            ),
            (
                "lmstudio",
                Api::OpenAICompletions,
                "http://127.0.0.1:1234/v1",
                true,
            ),
            (
                "opencode-zen",
                Api::OpenAICompletions,
                "https://opencode.ai/zen/v1",
                false,
            ),
            (
                "opencode-go",
                Api::OpenAICompletions,
                "https://opencode.ai/zen/go/v1",
                false,
            ),
        ];
        for (id, api, base_url, local) in pinned {
            let provider = catalog
                .providers
                .iter()
                .find(|p| p.id == *id)
                .unwrap_or_else(|| panic!("{id} 不在目录里（被跳过或改名了？）"));
            assert_eq!(provider.api, *api, "{id} 的协议变了");
            assert_eq!(&provider.base_url, base_url, "{id} 的端点变了");
            assert_eq!(provider.local, *local, "{id} 的本地标记变了");
        }
        // 无版本段的例外必须有出处说明
        let novita = catalog
            .providers
            .iter()
            .find(|p| p.id == "novita-ai")
            .unwrap();
        assert!(
            novita.base_url_note.is_some(),
            "novita-ai 的无版本段端点必须带 baseUrlNote"
        );
    }
}
