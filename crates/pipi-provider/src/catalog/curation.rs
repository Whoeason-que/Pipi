//! 收录范围与人工核实结果：**唯一**「Pipi 里能选到哪些提供商」的权威表。
//!
//! baseUrl 的来源只有三种：① 上游 `api` 字段；② 官方 SDK/文档核实后写死在这里；
//! ③ 本地运行时（手写条目）。**不要凭记忆填** —— 表里没有的提供商宁可漏收录。
//! 改这里等于改用户能选到的模型来源，改完跑 `cargo test -p pipi-provider catalog`。

use pipi_protocol::Api;

/// 分组顺序即 UI 顺序。
pub const GROUPS: [&str; 4] = ["聚合网关", "国内", "国际", "本地"];

/// CURATION 的一条：`pid` 指向上游 provider，其余字段是人工核实后的覆盖。
pub(super) struct Curated {
    pub(super) pid: &'static str,
    pub(super) id: Option<&'static str>,
    pub(super) name: Option<&'static str>,
    pub(super) group: &'static str,
    pub(super) api: Option<Api>,
    pub(super) base_url: Option<&'static str>,
    pub(super) base_url_note: Option<&'static str>,
    pub(super) env_key: Option<&'static str>,
    pub(super) doc: Option<&'static str>,
    pub(super) note: Option<&'static str>,
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
pub(super) struct ManualEntry {
    pub(super) id: &'static str,
    pub(super) name: &'static str,
    pub(super) base_url: &'static str,
    pub(super) env_key: &'static str,
    pub(super) doc: &'static str,
    pub(super) note: &'static str,
}

/// 收录范围与人工核实结果。改这里 = 改「Pipi 里能选到哪些提供商」。
///
/// baseUrl 的来源只有三种：① 上游 `api` 字段；② 官方 SDK/文档核实后写死在这里；
/// ③ 本地运行时（手写条目）。**不要凭记忆填**。
pub(super) const CURATION: &[Curated] = &[
    // ---- 聚合网关 ----
    curated!(
        "openrouter",
        "聚合网关",
        note = "聚合 300+ 模型；模型 ID 形如 anthropic/claude-sonnet-4.5"
    ),
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
    curated!(
        "opencode",
        "聚合网关",
        id = "opencode-zen",
        name = "OpenCode Zen"
    ),
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
    // 上游 2026-10 改名（kimi-for-coding → kimi-code-plan-cn）。官方文档核实：同一端点
    // 两种入口 —— OpenAI 兼容是 `.../coding/v1`（上游 api 字段给的就是它），Anthropic
    // 兼容是 `.../coding/`；故按上游 npm 收 openai-completions。
    curated!(
        "kimi-code-plan-cn",
        "国内",
        name = "Kimi For Coding",
        note = "官方文档：OpenAI 兼容入口是 .../coding/v1，Anthropic 兼容入口是 .../coding/"
    ),
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
    curated!(
        "xai",
        "国际",
        name = "xAI Grok",
        base_url = "https://api.x.ai/v1"
    ),
    curated!("groq", "国际", base_url = "https://api.groq.com/openai/v1"),
    curated!("mistral", "国际", base_url = "https://api.mistral.ai/v1"),
    curated!(
        "togetherai",
        "国际",
        base_url = "https://api.together.xyz/v1"
    ),
    curated!(
        "deepinfra",
        "国际",
        base_url = "https://api.deepinfra.com/v1/openai"
    ),
    curated!("cerebras", "国际", base_url = "https://api.cerebras.ai/v1"),
    curated!("fireworks-ai", "国际", name = "Fireworks AI"),
    curated!(
        "novita-ai",
        "国际",
        base_url_note =
            "官方 llms.txt 明写 OpenAI 兼容端点为 https://api.novita.ai/openai（无版本段）"
    ),
    curated!("nvidia", "国际", name = "NVIDIA NIM"),
    curated!(
        "huggingface",
        "国际",
        name = "Hugging Face Router",
        env_key = "HF_TOKEN"
    ),
    curated!("upstage", "国际"),
    curated!("inception", "国际"),
    curated!("chutes", "国际"),
    // ---- 本地（上游有 lmstudio，但没有可用的 baseUrl，故按手写条目处理） ----
];

pub(super) const MANUAL: &[ManualEntry] = &[
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

/// 上游 SDK（`npm` 字段）→ Pipi 支持的两种协议。provider 级与**模型级**
/// （`model.provider.npm`）共用这张表：表里没有的一律报错，模型级映射不到时
/// 跳过那个模型 —— 宁可漏收录，也不猜协议。
pub(super) fn protocol_of(npm: &str) -> Option<Api> {
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

// 测试里也用这两个宏造 fixture 条目；`curated!` 的展开体引用 `opt!`，
// 而 macro_rules 的宏名在**调用点**解析 —— 两个都要能被调用方看到。
#[cfg(test)]
pub(crate) use curated;
#[cfg(test)]
pub(crate) use opt;
