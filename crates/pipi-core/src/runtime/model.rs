//! 环境与模型的产品解析。
use super::*;
use crate::settings::ApiMode;

pub(super) fn append_declared_env(
    engine: &AgentSession,
    resolved_env: &pipi_venv::av::resolve::ResolvedEnv,
) -> Result<(), String> {
    let declared = resolved_env
        .provenance
        .iter()
        .filter(|(_, source)| source.as_str() != pipi_venv::av::resolve::PROCESS_SOURCE)
        .map(|(key, source)| pipi_protocol::session::EnvDeclared {
            key: key.clone(),
            source: source.clone(),
        })
        .collect();
    engine
        .record_env(declared)
        .map_err(|error| format!("无法写入环境记账: {error}"))
}

pub(super) fn run_stream_options(
    api_key: String,
    model: &Model,
    session_id: &str,
    auto_protocol: bool,
) -> StreamOptions {
    StreamOptions {
        auto_protocol,
        api_key: Some(api_key),
        temperature: None,
        max_tokens: Some(model.max_tokens),
        timeout_secs: 300,
        session_id: Some(session_id.to_string()),
    }
}

/// `resolve_model` 的结果：可用的模型（协议已按设置与目录解析）+ 密钥 + 协议模式。
#[derive(Debug)]
pub(super) struct ResolvedModel {
    pub(super) model: Model,
    pub(super) api_key: String,
    /// 协议是否由系统管理（提供商条目的 `api_mode == auto`）：agent loop 据此
    /// 决定端点回报「不支持当前协议」时要不要换协议再试一次。
    pub(super) auto_protocol: bool,
}

pub(super) fn resolve_model(
    def: &AgentDefinition,
    session_model: Option<&Model>,
    env: Option<&std::collections::BTreeMap<String, String>>,
) -> Result<ResolvedModel, String> {
    let target = session_model
        .cloned()
        .or_else(|| def.provider.clone())
        .ok_or_else(|| {
            "该 Agent 还未配置默认模型，且当前会话未选择模型（请在 Agent 详情页绑定，或在会话顶部选择模型）".to_string()
        })?;
    let settings = load_settings();
    // 同一端点可能被声明成两种协议（模型级覆盖 / 网关双协议）：先精确匹配
    // (协议, 端点)，再按端点兜底 —— 密钥是按端点配的，不该因为协议选择而丢。
    let entry = settings
        .providers
        .iter()
        .find(|p| p.api == target.api && p.base_url == target.base_url)
        .or_else(|| {
            settings
                .providers
                .iter()
                .find(|p| p.base_url == target.base_url)
        });
    let api_key = entry
        .and_then(|p| match env {
            // 与 bash 子进程消费同一份 resolved env；无契约上下文时回退进程环境
            Some(env) => p.resolve_api_key_in(env),
            None => p.resolve_api_key(),
        })
        .ok_or_else(|| {
            format!(
                "提供商 {} 未配置 API 密钥（设置 → 模型提供商）",
                target.base_url
            )
        })?;
    // 协议：auto（默认）按「运行期实证 → 目录模型级覆盖 → 目录 provider 级 →
    // 绑定声明」解析；manual（用户钉死）原样使用声明。
    let auto_protocol = entry.map(|p| p.api_mode == ApiMode::Auto).unwrap_or(true);
    let api = if auto_protocol {
        let catalog = pipi_provider::catalog::cached_catalog(
            pipi_venv::av::paths::catalog_cache_path().as_deref(),
        );
        pipi_provider::effective_api(catalog.as_ref(), &target.base_url, &target.id, target.api)
    } else {
        target.api
    };
    Ok(ResolvedModel {
        model: Model {
            id: target.id.clone(),
            name: target.name.clone(),
            api,
            base_url: target.base_url.clone(),
            max_tokens: if target.max_tokens == 0 {
                8192
            } else {
                target.max_tokens
            },
            context_window: target.context_window,
        },
        api_key,
        auto_protocol,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_model_prefers_session_model_over_agent_default() {
        // 该测试依赖「HOME 下没有 settings.json → 默认 provider 表」，
        // 且 HOME 是进程级状态：持全局锁 + 指向空目录，保证确定性。
        let _guard = pipi_test_support::HOME_LOCK.lock().unwrap();
        let previous_home = std::env::var("HOME").unwrap_or_default();
        let empty_home = pipi_test_support::temp_dir("pipi-resolve-model-home");
        std::fs::create_dir_all(empty_home.join(".pipi")).unwrap();
        pipi_test_support::set_env_var("HOME", &empty_home);

        let default_model = Model {
            id: "gpt-4o".into(),
            name: "GPT-4o".into(),
            api: pipi_protocol::Api::OpenAICompletions,
            base_url: "https://api.openai.com/v1".into(),
            max_tokens: 4096,
            context_window: 128000,
        };
        let session_model = Model {
            id: "claude-sonnet-4-5".into(),
            name: "Claude Sonnet".into(),
            api: pipi_protocol::Api::AnthropicMessages,
            base_url: "https://api.anthropic.com".into(),
            max_tokens: 8192,
            context_window: 200000,
        };

        let def = AgentDefinition {
            name: "test-agent".into(),
            description: String::new(),
            model: "gpt-4o".into(),
            provider: Some(default_model.clone()),
            workspace: None,
            permissions: Default::default(),
            mcp_servers: Vec::new(),
            subagent: false,
            compact_threshold_percent: 75,
            compact_target_percent: None,
        };

        // 未配置 key 时的校验
        pipi_test_support::remove_env_var("ANTHROPIC_API_KEY");
        pipi_test_support::remove_env_var("OPENAI_API_KEY");

        // 回退默认模型
        let err = super::resolve_model(&def, None, None).unwrap_err();
        assert!(err.contains("未配置 API 密钥"));

        // 优先会话覆盖模型
        let err = super::resolve_model(&def, Some(&session_model), None).unwrap_err();
        assert!(err.contains("api.anthropic.com"));

        // 配上 key 后成功解析
        pipi_test_support::set_env_var("ANTHROPIC_API_KEY", "test-key");
        let resolved = super::resolve_model(&def, Some(&session_model), None).unwrap();
        assert_eq!(resolved.model.id, "claude-sonnet-4-5");
        assert_eq!(resolved.api_key, "test-key");
        // 没配目录缓存 + auto：协议保持绑定里的声明
        assert!(resolved.auto_protocol);
        assert_eq!(resolved.model.api, pipi_protocol::Api::AnthropicMessages);
        pipi_test_support::remove_env_var("ANTHROPIC_API_KEY");
        pipi_test_support::set_env_var("HOME", &previous_home);
        let _ = std::fs::remove_dir_all(&empty_home);
    }
}
