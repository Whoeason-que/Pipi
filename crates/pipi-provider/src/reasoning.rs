//! 将用户的思考强度映射到当前请求协议；不设置时不改变原有请求。
use pipi_protocol::{Api, Model, ReasoningEffort, StreamOptions};
use serde_json::{Value, json};

use crate::StreamError;

pub(super) fn parameters(
    model: &Model,
    options: &StreamOptions,
) -> Result<Option<Value>, StreamError> {
    let Some(effort) = model.reasoning_effort else {
        return Ok(None);
    };
    if model.api == Api::OpenAICompletions {
        return Ok(Some(json!({ "reasoning_effort": effort })));
    }
    if effort == ReasoningEffort::None {
        return Ok(Some(json!({ "thinking": { "type": "disabled" } })));
    }

    // Claude 4.6+ 使用 adaptive；旧版和其他 Messages 兼容模型仍走预算。
    // 只识别 Claude 的公开型号格式，不把协议本身当成 adaptive 能力证明。
    let id = model.id.rsplit('/').next().unwrap_or(&model.id);
    let parts: Vec<_> = id
        .strip_prefix("claude-")
        .unwrap_or("")
        .split(['-', '.'])
        .collect();
    let version = parts.get(1).and_then(|v| v.parse::<u32>().ok());
    let minor = parts
        .get(2)
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    let adaptive = matches!(parts.first(), Some(&"mythos" | &"fable"))
        || (matches!(parts.first(), Some(&"opus" | &"sonnet" | &"haiku"))
            && version.is_some_and(|major| major >= 5 || (major == 4 && minor >= 6)));
    if adaptive {
        // adaptive 没有 minimal；4.6 的最高档叫 max，后续型号才有 xhigh。
        let effort = match effort {
            ReasoningEffort::Minimal => ReasoningEffort::Low,
            ReasoningEffort::Xhigh if version == Some(4) && minor == 6 => ReasoningEffort::Max,
            other => other,
        };
        return Ok(Some(json!({
            "thinking": { "type": "adaptive" },
            "output_config": { "effort": effort },
        })));
    }

    // 与 pi 的默认预算一致，并在总输出上限内至少给正文留 1024 token。
    let budget = match effort {
        ReasoningEffort::Minimal => 1024,
        ReasoningEffort::Low => 2048,
        ReasoningEffort::Medium => 8192,
        ReasoningEffort::High | ReasoningEffort::Xhigh | ReasoningEffort::Max => 16384,
        ReasoningEffort::None => unreachable!(),
    };
    let max_tokens = options.max_tokens.unwrap_or(model.max_tokens);
    if max_tokens < 2048 {
        return Err(StreamError::terminal(
            "Extended thinking requires maxTokens of at least 2048 (1024 thinking + 1024 answer). Increase the model output limit or select default/off reasoning.",
        ));
    }
    Ok(Some(json!({ "thinking": {
        "type": "enabled",
        "budget_tokens": budget.min(max_tokens - 1024),
    } })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, api: Api, effort: Option<ReasoningEffort>) -> Model {
        Model {
            id: id.into(),
            name: String::new(),
            api,
            base_url: String::new(),
            max_tokens: 32768,
            context_window: 200000,
            reasoning_effort: effort,
        }
    }

    #[test]
    fn default_omits_parameters_and_openai_preserves_every_explicit_level() {
        for api in [Api::OpenAICompletions, Api::AnthropicMessages] {
            assert_eq!(
                parameters(&model("m", api, None), &StreamOptions::default()).unwrap(),
                None
            );
        }
        for effort in [
            ReasoningEffort::None,
            ReasoningEffort::Minimal,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::Xhigh,
            ReasoningEffort::Max,
        ] {
            let params = parameters(
                &model("m", Api::OpenAICompletions, Some(effort)),
                &StreamOptions::default(),
            )
            .unwrap()
            .unwrap();
            assert_eq!(params, json!({"reasoning_effort": effort}));
        }
    }

    #[test]
    fn claude_adaptive_uses_effort_and_legacy_uses_a_bounded_budget() {
        for (id, effort, expected) in [
            (
                "claude-sonnet-4-6",
                ReasoningEffort::Medium,
                json!({"thinking":{"type":"adaptive"},"output_config":{"effort":"medium"}}),
            ),
            (
                "anthropic/claude-opus-4.6",
                ReasoningEffort::Xhigh,
                json!({"thinking":{"type":"adaptive"},"output_config":{"effort":"max"}}),
            ),
            (
                "claude-opus-4-7",
                ReasoningEffort::Xhigh,
                json!({"thinking":{"type":"adaptive"},"output_config":{"effort":"xhigh"}}),
            ),
            (
                "claude-sonnet-5",
                ReasoningEffort::Minimal,
                json!({"thinking":{"type":"adaptive"},"output_config":{"effort":"low"}}),
            ),
            (
                "claude-sonnet-4-5",
                ReasoningEffort::High,
                json!({"thinking":{"type":"enabled","budget_tokens":16384}}),
            ),
            (
                "claude-sonnet-4-5",
                ReasoningEffort::None,
                json!({"thinking":{"type":"disabled"}}),
            ),
        ] {
            assert_eq!(
                parameters(
                    &model(id, Api::AnthropicMessages, Some(effort)),
                    &StreamOptions::default()
                )
                .unwrap(),
                Some(expected),
                "{id}"
            );
        }
        let model = model(
            "claude-3-7-sonnet",
            Api::AnthropicMessages,
            Some(ReasoningEffort::High),
        );
        let mut options = StreamOptions {
            max_tokens: Some(4096),
            ..Default::default()
        };
        assert_eq!(
            parameters(&model, &options).unwrap().unwrap()["thinking"]["budget_tokens"],
            3072
        );
        options.max_tokens = Some(1024);
        assert!(
            parameters(&model, &options)
                .unwrap_err()
                .message
                .contains("at least 2048")
        );
    }
}
