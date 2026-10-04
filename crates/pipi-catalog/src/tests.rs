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

/// 真实上游数据：`cargo test -p pipi-core -- --ignored`（默认不跑，避免依赖网络）。
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
            "kimi-for-coding",
            Api::AnthropicMessages,
            "https://api.kimi.com/coding/v1",
            false,
        ),
        (
            "minimax-cn",
            Api::AnthropicMessages,
            "https://api.minimaxi.com/anthropic/v1",
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
