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

use crate::types::Api;

/// 上游目录。
const REMOTE: &str = "https://models.dev/api.json";
/// 缓存的保鲜期：超过就尝试刷新（刷新失败仍用缓存）。
const TTL_SECS: i64 = 24 * 60 * 60;
/// 拉取超时。冷启动没缓存时用户要等这么久，别设太长；
/// 超时/失败会落到缓存或「手填」退化路径（`resolve_catalog`）。
const FETCH_TIMEOUT_SECS: u64 = 10;
/// 生产目录至少要有这么多家 —— 少了说明上游结构变了或 CURATION 被改坏。
const MIN_PROVIDERS: usize = 20;

/// 分组顺序即 UI 顺序。
pub const GROUPS: [&str; 4] = ["聚合网关", "国内", "国际", "本地"];

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

/// CURATION 的一条：`pid` 指向上游 provider，其余字段是人工核实后的覆盖。
struct Curated {
    pid: &'static str,
    id: Option<&'static str>,
    name: Option<&'static str>,
    group: &'static str,
    api: Option<Api>,
    base_url: Option<&'static str>,
    base_url_note: Option<&'static str>,
    env_key: Option<&'static str>,
    doc: Option<&'static str>,
    note: Option<&'static str>,
}

/// 写表用：只列要覆盖的字段（const 上下文不能用结构体更新语法）。
/// **键必须按顺序**：id → name → api → base_url → base_url_note → env_key → doc → note
/// （可以整段跳过，但不能回退；越序会报 `no rules expected`）。
macro_rules! opt {
    () => {
        None
    };
    ($value:expr) => {
        Some($value)
    };
}

macro_rules! curated {
    ($pid:expr, $group:expr
        $(, id = $id:expr)?
        $(, name = $name:expr)?
        $(, api = $api:expr)?
        $(, base_url = $base_url:expr)?
        $(, base_url_note = $base_url_note:expr)?
        $(, env_key = $env_key:expr)?
        $(, doc = $doc:expr)?
        $(, note = $note:expr)?
    ) => {
        Curated {
            pid: $pid,
            group: $group,
            id: opt!($($id)?),
            name: opt!($($name)?),
            api: opt!($($api)?),
            base_url: opt!($($base_url)?),
            base_url_note: opt!($($base_url_note)?),
            env_key: opt!($($env_key)?),
            doc: opt!($($doc)?),
            note: opt!($($note)?),
        }
    };
}

/// 手写条目：上游没有这些（本地运行时），模型名由用户在本地服务里看，故列空表。
struct ManualEntry {
    id: &'static str,
    name: &'static str,
    base_url: &'static str,
    env_key: &'static str,
    doc: &'static str,
    note: &'static str,
}

/// 收录范围与人工核实结果。改这里 = 改「Pipi 里能选到哪些提供商」。
///
/// baseUrl 的来源只有三种：① 上游 `api` 字段；② 官方 SDK/文档核实后写死在这里；
/// ③ 本地运行时（手写条目）。**不要凭记忆填**。
const CURATION: &[Curated] = &[
    // ---- 聚合网关 ----
    curated!("openrouter", "聚合网关", note = "聚合 300+ 模型；模型 ID 形如 anthropic/claude-sonnet-4.5"),
    curated!("requesty", "聚合网关"),
    // 上游没有 baseUrl → 按官方文档（/v1/chat/completions）核实
    curated!(
        "vercel",
        "聚合网关",
        base_url = "https://ai-gateway.vercel.sh/v1",
        note = "AI Gateway（OpenAI 兼容端点）"
    ),
    curated!("llmgateway", "聚合网关"),
    curated!("poe", "聚合网关"),
    curated!("opencode", "聚合网关", id = "opencode-zen", name = "OpenCode Zen"),
    curated!("opencode-go", "聚合网关", name = "OpenCode Go"),
    curated!(
        "302ai",
        "聚合网关",
        name = "302.AI",
        note = "目录里的环境变量名以数字开头，shell 不便设置，建议直接在表单里填明文密钥"
    ),
    // ---- 国内 ----
    curated!("deepseek", "国内", base_url = "https://api.deepseek.com/v1"),
    curated!(
        "deepseek",
        "国内",
        id = "deepseek-anthropic",
        name = "DeepSeek（Anthropic 兼容）",
        api = Api::AnthropicMessages,
        base_url = "https://api.deepseek.com/anthropic",
        note = "官方 Anthropic 兼容端点，可跑 Claude Code 类客户端"
    ),
    curated!("moonshotai-cn", "国内", name = "Moonshot / Kimi（中国）"),
    curated!("moonshotai", "国内", name = "Moonshot / Kimi（国际）"),
    curated!("kimi-for-coding", "国内", name = "Kimi For Coding"),
    curated!("zhipuai", "国内", name = "智谱 GLM（开放平台）"),
    curated!("zai", "国内", name = "Z.ai（GLM 国际）"),
    curated!("minimax-cn", "国内", name = "MiniMax（中国）"),
    curated!("alibaba-cn", "国内", name = "阿里云百炼 / Qwen（中国）"),
    curated!("alibaba", "国内", name = "Aliyun DashScope（国际）"),
    curated!("siliconflow-cn", "国内", name = "硅基流动 SiliconFlow"),
    curated!("qiniu-ai", "国内", name = "七牛云 AI"),
    curated!(
        "volcengine",
        "国内",
        name = "火山方舟 Volcengine",
        note = "模型 ID 用方舟的 endpoint id（ep-…）或模型名"
    ),
    curated!("modelscope", "国内", name = "魔搭 ModelScope"),
    // ---- 国际 ----
    curated!("anthropic", "国际", base_url = "https://api.anthropic.com"),
    curated!("openai", "国际", base_url = "https://api.openai.com/v1"),
    curated!(
        "google",
        "国际",
        name = "Google Gemini",
        api = Api::OpenAICompletions,
        base_url = "https://generativelanguage.googleapis.com/v1beta/openai",
        env_key = "GEMINI_API_KEY",
        note = "走 Gemini 的 OpenAI 兼容端点（原生协议不支持）"
    ),
    curated!("xai", "国际", name = "xAI Grok", base_url = "https://api.x.ai/v1"),
    curated!("groq", "国际", base_url = "https://api.groq.com/openai/v1"),
    curated!("mistral", "国际", base_url = "https://api.mistral.ai/v1"),
    curated!("togetherai", "国际", base_url = "https://api.together.xyz/v1"),
    curated!("deepinfra", "国际", base_url = "https://api.deepinfra.com/v1/openai"),
    curated!("cerebras", "国际", base_url = "https://api.cerebras.ai/v1"),
    curated!("fireworks-ai", "国际", name = "Fireworks AI"),
    curated!(
        "novita-ai",
        "国际",
        base_url_note = "官方 llms.txt 明写 OpenAI 兼容端点为 https://api.novita.ai/openai（无版本段）"
    ),
    curated!("nvidia", "国际", name = "NVIDIA NIM"),
    curated!("huggingface", "国际", name = "Hugging Face Router", env_key = "HF_TOKEN"),
    curated!("upstage", "国际"),
    curated!("inception", "国际"),
    curated!("chutes", "国际"),
    // ---- 本地（上游有 lmstudio，但没有可用的 baseUrl，故按手写条目处理） ----
];

const MANUAL: &[ManualEntry] = &[
    ManualEntry {
        // 端点与旧生成物一致（老用户 settings 里保存的就是 127.0.0.1），
        // 换写法会让既有绑定匹配不上；matchProvider 另做本地主机别名归一兜底。
        id: "lmstudio",
        name: "LM Studio（本地）",
        base_url: "http://127.0.0.1:1234/v1",
        env_key: "LMSTUDIO_API_KEY",
        doc: "https://lmstudio.ai/docs/app/api/endpoints/openai",
        note: "本地服务：密钥填任意非空值；模型名以 LM Studio 已加载模型为准",
    },
    ManualEntry {
        id: "ollama",
        name: "Ollama（本地）",
        base_url: "http://localhost:11434/v1",
        env_key: "OLLAMA_API_KEY",
        doc: "https://docs.ollama.com/api/openai-compatibility",
        note: "本地服务：密钥填任意非空值（如 ollama）；模型名以 `ollama list` 为准",
    },
    ManualEntry {
        id: "llamacpp",
        name: "llama.cpp server（本地）",
        base_url: "http://localhost:8080/v1",
        env_key: "LLAMACPP_API_KEY",
        doc: "https://github.com/ggml-org/llama.cpp/tree/master/tools/server",
        note: "本地服务：密钥填任意非空值；模型名以 --model / --alias 指定值为准",
    },
    ManualEntry {
        id: "vllm",
        name: "vLLM（本地）",
        base_url: "http://localhost:8000/v1",
        env_key: "VLLM_API_KEY",
        doc: "https://docs.vllm.ai/en/latest/serving/openai_compatible_server.html",
        note: "本地服务：密钥填任意非空值；模型名以 --served-model-name 为准",
    },
];

/// 上游 SDK（`npm` 字段）→ Pipi 支持的两种协议。表里没有的一律报错：
/// 宁可漏收录，也不猜协议。
fn protocol_of(npm: &str) -> Option<Api> {
    match npm {
        "@ai-sdk/anthropic" => Some(Api::AnthropicMessages),
        "@ai-sdk/openai"
        | "@ai-sdk/openai-compatible"
        | "@openrouter/ai-sdk-provider"
        | "@ai-sdk/xai"
        | "@ai-sdk/groq"
        | "@ai-sdk/mistral"
        | "@ai-sdk/togetherai"
        | "@ai-sdk/deepinfra"
        | "@ai-sdk/cerebras"
        | "@ai-sdk/gateway" => Some(Api::OpenAICompletions),
        _ => None,
    }
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
    dirs::home_dir().map(|home| home.join(".pipi").join("cache").join("models.json"))
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
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建缓存目录失败: {e}"))?;
    }
    let text = serde_json::to_string(catalog).map_err(|e| format!("序列化目录失败: {e}"))?;
    // 先写临时文件再原子替换：进程中途崩溃也不会留下截断的缓存
    // （截断的 models.json 会被 read_cache 当作「没有缓存」，下次冷启动又要拉 4.6MB）。
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写入目录缓存失败: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("替换目录缓存失败: {e}")
    })
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
        return Err(format!("{pid} 的端点既不是本地地址也不是 https：{base_url}"));
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
            return Err(format!("{} 的分组 {} 不在 GROUPS 里", provider.id, provider.group));
        }
        if !provider.local && provider.models.is_empty() {
            return Err(format!("{} 没有任何支持工具调用的模型，应移出 CURATION", provider.id));
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
            context: limit.and_then(|l| l.get("context")).and_then(|v| v.as_u64()),
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
                skipped.push(format!("{} 已不在 models.dev 目录（上游改名或下线）", entry.pid));
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
        let base_url = match entry
            .base_url
            .map(str::to_string)
            .or_else(|| provider.get("api").and_then(|v| v.as_str()).map(str::to_string))
        {
            Some(base_url) => strip_slash(&base_url),
            None => {
                skipped.push(format!("{} 既没有 baseUrl 覆盖也没有上游 api 字段", entry.pid));
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
                .or_else(|| provider.get("name").and_then(|v| v.as_str()).map(str::to_string))
                .unwrap_or_else(|| entry.pid.to_string()),
            api,
            local: is_local_endpoint(&base_url),
            base_url,
            env_key,
            group: entry.group.to_string(),
            doc: entry
                .doc
                .map(str::to_string)
                .or_else(|| provider.get("doc").and_then(|v| v.as_str()).map(str::to_string)),
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
pub fn build_catalog(upstream: &serde_json::Value, fetched_at: i64) -> Result<ModelCatalog, String> {
    build_with(CURATION, MANUAL, upstream, fetched_at, MIN_PROVIDERS)
}

async fn fetch_upstream() -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("HTTP 客户端初始化失败: {e}"))?;
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

/// 取目录：缓存优先（未过期直接用），刷新失败退回缓存，两者都没有则报错。
///
/// `source` 语义：`models.dev` = 本次联网刷新的结果；`cache` = 读的是本地缓存
/// （此时 `stale=true` 表示缓存已过期且本次刷新失败）。
pub async fn load_catalog(refresh: bool) -> Result<ModelCatalog, String> {
    let cached = read_cache();
    if !refresh {
        if let Some(catalog) = &cached {
            if catalog
                .fetched_at
                .map(|at| is_fresh(at, now_secs()))
                .unwrap_or(false)
            {
                let mut hit = catalog.clone();
                hit.source = "cache".to_string();
                hit.stale = false;
                return Ok(hit);
            }
        }
    }
    let fetched = fetch_upstream()
        .await
        .and_then(|raw| build_catalog(&raw, now_secs()));
    let resolved = resolve_catalog(cached, fetched)?;
    if resolved.source == "models.dev" {
        if let Err(error) = write_cache(&resolved) {
            eprintln!("pipi: 写入模型目录缓存失败：{error}");
        }
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
            }
        })
    }

    /// 测试用的收录子集：只覆盖 fixture 里出现过的 provider（生产表另有不变量测试）。
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
        assert!(!ids.contains(&"deepseek-no-tools"), "不能工具调用的模型不入列");
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
        assert!(validate_endpoint(
            "novita-ai",
            Api::OpenAICompletions,
            "https://api.novita.ai/openai",
            Some("官方 llms.txt")
        )
        .is_ok());
        // anthropic 协议不看版本段
        assert!(validate_endpoint("anthropic", Api::AnthropicMessages, "https://api.anthropic.com", None).is_ok());
    }

    #[test]
    fn 非本地端点必须是_https() {
        assert!(
            validate_endpoint("x", Api::OpenAICompletions, "http://api.example.com/v1", None).is_err()
        );
        assert!(validate_endpoint("x", Api::OpenAICompletions, "https://api.example.com/v1", None).is_ok());
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
            curated!("google", "国际", base_url = "https://generativelanguage.googleapis.com/v1beta/openai"),
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
        let curation = &[curated!("deepseek", "海外", base_url = "https://api.deepseek.com/v1")];
        let err = build_with(curation, &[], &upstream_fixture(), 0, 1).unwrap_err();
        assert!(err.contains("不在 GROUPS"), "{err}");
    }

    #[test]
    fn 不变量能挡住重复端点() {
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
        let bad_group = &[curated!("deepseek", "海外", base_url = "https://api.deepseek.com/v1")];
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

    /// 真实上游数据：`cargo test -p pipi-core -- --ignored`（默认不跑，避免依赖网络）。
    #[tokio::test]
    #[ignore]
    async fn 真实上游目录可构建且满足不变量() {
        let raw = fetch_upstream().await.expect("应能拉到 models.dev");
        let catalog = build_catalog(&raw, now_secs()).expect("应能构建目录");
        assert!(catalog.providers.len() >= MIN_PROVIDERS);
        assert!(catalog
            .providers
            .iter()
            .all(|p| !p.models.is_empty() || p.local));

        // 钉住几家关键 provider 的协议与端点：上游静默漂移（改名/换协议/换端点）时
        // 这个测试会红，而不是让用户少看到几家。
        let pinned: &[(&str, Api, &str, bool)] = &[
            ("deepseek", Api::OpenAICompletions, "https://api.deepseek.com/v1", false),
            ("deepseek-anthropic", Api::AnthropicMessages, "https://api.deepseek.com/anthropic", false),
            ("anthropic", Api::AnthropicMessages, "https://api.anthropic.com", false),
            ("openai", Api::OpenAICompletions, "https://api.openai.com/v1", false),
            (
                "google",
                Api::OpenAICompletions,
                "https://generativelanguage.googleapis.com/v1beta/openai",
                false,
            ),
            ("kimi-for-coding", Api::AnthropicMessages, "https://api.kimi.com/coding/v1", false),
            ("minimax-cn", Api::AnthropicMessages, "https://api.minimaxi.com/anthropic/v1", false),
            ("zhipuai", Api::OpenAICompletions, "https://open.bigmodel.cn/api/paas/v4", false),
            ("novita-ai", Api::OpenAICompletions, "https://api.novita.ai/openai", false),
            ("ollama", Api::OpenAICompletions, "http://localhost:11434/v1", true),
            ("vllm", Api::OpenAICompletions, "http://localhost:8000/v1", true),
            ("lmstudio", Api::OpenAICompletions, "http://127.0.0.1:1234/v1", true),
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
        let novita = catalog.providers.iter().find(|p| p.id == "novita-ai").unwrap();
        assert!(novita.base_url_note.is_some(), "novita-ai 的无版本段端点必须带 baseUrlNote");
    }
}
