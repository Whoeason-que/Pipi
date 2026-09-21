//! 全局设置：`~/.pipi/settings.json`。
//!
//! 一切皆文件 —— 主题与模型提供商配置也是普通文件，可手改、可同步。
//! 提供商的形态对齐 codex 的 `model_providers` / pi 的 providers 配置：
//! id、API 协议（anthropic-messages / openai-completions）、baseUrl，
//! 密钥支持环境变量引用（envKey，推荐）或明文（apiKey，本机自担）。

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::Api;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    #[serde(rename = "dark")]
    Dark,
    #[serde(rename = "light")]
    Light,
}

/// 模型提供商。Agent 通过 `agent.json` 的 `provider` 字段绑定其中一家。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// 稳定标识（slug），Agent 绑定与展示用。
    pub id: String,
    pub name: String,
    pub api: Api,
    /// 例如 https://api.anthropic.com 或 https://api.openai.com/v1
    pub base_url: String,
    /// 环境变量名（推荐）：存在时优先于 api_key。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_key: Option<String>,
    /// 明文密钥（本机自担；设置文件在用户目录下）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

impl ProviderConfig {
    /// 解析实际可用的 API key：环境变量优先，回退明文。
    pub fn resolve_api_key(&self) -> Option<String> {
        if let Some(env_key) = &self.env_key {
            if let Ok(value) = std::env::var(env_key) {
                if !value.trim().is_empty() {
                    return Some(value);
                }
            }
        }
        self.api_key.clone().filter(|k| !k.trim().is_empty())
    }

    /// 同 [`Self::resolve_api_key`]，但环境来源是 av 契约解析出的 resolved env
    /// （一处解析，多处消费：bash 子进程 / provider key / 未来 MCP）。
    pub fn resolve_api_key_in(
        &self,
        env: &std::collections::BTreeMap<String, String>,
    ) -> Option<String> {
        if let Some(env_key) = &self.env_key {
            if let Some(value) = env.get(env_key) {
                if !value.trim().is_empty() {
                    return Some(value.clone());
                }
            }
        }
        self.api_key.clone().filter(|k| !k.trim().is_empty())
    }
}

/// 上下文压缩的行为开关。
///
/// 默认「分叉 + 归档」：压缩不再原地改写当前会话，而是把压缩结果写进一个
/// 分叉出的新会话，原会话作为**完整记录**保留（可选移入归档）。
/// 关掉 `fork_before_compact` 即回到原地压缩。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSettings {
    /// 压缩前分叉出新会话（原会话保留为完整记录）。
    #[serde(default = "default_true")]
    pub fork_before_compact: bool,
    /// 分叉后把原会话移入归档（`sessions/.archive/`）。
    #[serde(default = "default_true")]
    pub archive_original: bool,
}

fn default_true() -> bool {
    true
}

impl Default for CompactionSettings {
    fn default() -> Self {
        CompactionSettings {
            fork_before_compact: true,
            archive_original: true,
        }
    }
}

/// 请求失败重发策略（可重试错误的分类见 [`crate::retry`]）。
///
/// 每次取值都夹到安全区间：配置可能来自旧文件或手工编辑，越界值不该让重试变成
/// 「几乎无限重试」或「永不重试」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrySettings {
    /// 总尝试次数（含首次）：1 = 不重试，上限 5。
    #[serde(default = "default_retry_max_attempts")]
    pub max_attempts: u32,
    /// 退避基数（毫秒）：第 n 次失败后等 `base * 2^(n-1)`。
    #[serde(default = "default_retry_base_delay_ms")]
    pub base_delay_ms: u64,
    /// 退避上限（毫秒）；服务端要求的等待超过它时直接放弃并说明原因。
    #[serde(default = "default_retry_max_delay_ms")]
    pub max_delay_ms: u64,
}

fn default_retry_max_attempts() -> u32 {
    crate::retry::RetryPolicy::default().max_attempts
}

fn default_retry_base_delay_ms() -> u64 {
    crate::retry::RetryPolicy::default().base_delay_ms
}

fn default_retry_max_delay_ms() -> u64 {
    crate::retry::RetryPolicy::default().max_delay_ms
}

impl Default for RetrySettings {
    fn default() -> Self {
        let policy = crate::retry::RetryPolicy::default();
        RetrySettings {
            max_attempts: policy.max_attempts,
            base_delay_ms: policy.base_delay_ms,
            max_delay_ms: policy.max_delay_ms,
        }
    }
}

impl RetrySettings {
    /// 夹到安全区间后转成运行时策略。
    pub fn policy(&self) -> crate::retry::RetryPolicy {
        let base_delay_ms = self.base_delay_ms.clamp(100, 10_000);
        let max_delay_ms = self.max_delay_ms.clamp(1_000, 120_000).max(base_delay_ms);
        crate::retry::RetryPolicy {
            max_attempts: self.max_attempts.clamp(1, 5),
            base_delay_ms,
            max_delay_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme: Theme,
    pub providers: Vec<ProviderConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider_id: Option<String>,
    /// 压缩行为（字段级 default：旧 settings.json 缺这一段也能读）。
    #[serde(default)]
    pub compaction: CompactionSettings,
    /// 请求失败重发策略（字段级 default：旧 settings.json 缺这一段也能读）。
    #[serde(default)]
    pub retry: RetrySettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: Theme::Dark,
            providers: vec![
                ProviderConfig {
                    id: "anthropic".into(),
                    name: "Anthropic".into(),
                    api: Api::AnthropicMessages,
                    base_url: "https://api.anthropic.com".into(),
                    env_key: Some("ANTHROPIC_API_KEY".into()),
                    api_key: None,
                },
                ProviderConfig {
                    id: "openai".into(),
                    name: "OpenAI".into(),
                    api: Api::OpenAICompletions,
                    base_url: "https://api.openai.com/v1".into(),
                    env_key: Some("OPENAI_API_KEY".into()),
                    api_key: None,
                },
            ],
            default_provider_id: None,
            compaction: CompactionSettings::default(),
            retry: RetrySettings::default(),
        }
    }
}

/// `~/.pipi/settings.json`
pub fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".pipi").join("settings.json"))
}

/// 读取设置；文件缺失或损坏时返回默认值（坏文件不拖垮应用）。
pub fn load_settings() -> Settings {
    let Some(path) = settings_path() else {
        return Settings::default();
    };
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(settings) => settings,
            Err(e) => {
                eprintln!("pipi: settings.json 格式错误，使用默认设置: {e}");
                Settings::default()
            }
        },
        Err(_) => Settings::default(),
    }
}

/// 保存设置（pretty JSON + 换行，与 agent.json 一致）。
pub fn save_settings(settings: &Settings) -> Result<(), String> {
    let path = settings_path().ok_or_else(|| "无法定位用户主目录".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(&path, text + "\n").map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_serde_lowercase() {
        assert_eq!(serde_json::to_string(&Theme::Dark).unwrap(), r#""dark""#);
        assert_eq!(serde_json::to_string(&Theme::Light).unwrap(), r#""light""#);
        let t: Theme = serde_json::from_str(r#""light""#).unwrap();
        assert_eq!(t, Theme::Light);
    }

    #[test]
    fn roundtrip_and_legacy_default() {
        let settings = Settings::default();
        let json = serde_json::to_string_pretty(&settings).unwrap();
        assert!(json.contains("anthropic-messages"));
        assert!(json.contains("\"theme\": \"dark\""));
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, settings);

        // 旧/手写文件缺省字段也能读
        let minimal = r#"{"theme":"light","providers":[]}"#;
        let back: Settings = serde_json::from_str(minimal).unwrap();
        assert_eq!(back.theme, Theme::Light);
        assert!(back.providers.is_empty());
        assert_eq!(back.default_provider_id, None);
        // 压缩段缺失 → 默认「分叉 + 归档」都开
        assert_eq!(back.compaction, CompactionSettings::default());
        assert!(back.compaction.fork_before_compact);
        assert!(back.compaction.archive_original);
    }

    #[test]
    fn compaction_settings_partial_object_keeps_defaults() {
        // 只写了 archiveOriginal：forkBeforeCompact 取默认 true
        let text = r#"{"theme":"dark","providers":[],"compaction":{"archiveOriginal":false}}"#;
        let settings: Settings = serde_json::from_str(text).unwrap();
        assert!(!settings.compaction.archive_original);
        assert!(settings.compaction.fork_before_compact);
        // 往返后字段名是 camelCase
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"forkBeforeCompact\":true"), "{json}");
        assert!(json.contains("\"archiveOriginal\":false"), "{json}");
    }

    #[test]
    fn retry_settings_partial_object_keeps_defaults() {
        // 旧 settings.json 完全没有 retry 段 → 全默认（3 次尝试 / 1s / 30s）
        let legacy: Settings =
            serde_json::from_str(r#"{"theme":"dark","providers":[]}"#).unwrap();
        assert_eq!(legacy.retry, RetrySettings::default());
        let policy = legacy.retry.policy();
        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.base_delay_ms, 1_000);
        assert_eq!(policy.max_delay_ms, 30_000);

        // 只写一项 → 其余取默认；序列化字段是 camelCase
        let partial: Settings = serde_json::from_str(
            r#"{"theme":"dark","providers":[],"retry":{"maxAttempts":1}}"#,
        )
        .unwrap();
        assert_eq!(partial.retry.max_attempts, 1);
        assert_eq!(partial.retry.base_delay_ms, 1_000);
        let json = serde_json::to_string(&partial).unwrap();
        assert!(json.contains("\"maxAttempts\":1"), "{json}");
        assert!(json.contains("\"baseDelayMs\":1000"), "{json}");
    }

    #[test]
    fn retry_settings_are_clamped() {
        // 越界值不应变成「几乎无限重试」或「永不重试」
        let wild = RetrySettings {
            max_attempts: 99,
            base_delay_ms: 0,
            max_delay_ms: 10_000_000,
        }
        .policy();
        assert_eq!(wild.max_attempts, 5);
        assert_eq!(wild.base_delay_ms, 100);
        assert_eq!(wild.max_delay_ms, 120_000);

        let zero = RetrySettings {
            max_attempts: 0,
            base_delay_ms: 0,
            max_delay_ms: 0,
        }
        .policy();
        assert_eq!(zero.max_attempts, 1, "0 次尝试没有意义，夹到「不重试」");
        assert_eq!(zero.base_delay_ms, 100);
        assert_eq!(zero.max_delay_ms, 1_000);

        // 上限不得低于基数（否则退避序列会自我矛盾）
        let inverted = RetrySettings {
            max_attempts: 3,
            base_delay_ms: 10_000,
            max_delay_ms: 1_000,
        }
        .policy();
        assert_eq!(inverted.max_delay_ms, 10_000);
    }

    #[test]
    fn api_key_env_precedence() {
        let provider = ProviderConfig {
            id: "t".into(),
            name: "T".into(),
            api: Api::OpenAICompletions,
            base_url: "https://x".into(),
            env_key: Some("PIPI_TEST_KEY".into()),
            api_key: Some("plain".into()),
        };
        std::env::set_var("PIPI_TEST_KEY", "from-env");
        assert_eq!(provider.resolve_api_key().as_deref(), Some("from-env"));
        std::env::remove_var("PIPI_TEST_KEY");
        assert_eq!(provider.resolve_api_key().as_deref(), Some("plain"));
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pipi-settings-{}", crate::session::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let settings = Settings {
            theme: Theme::Light,
            ..Settings::default()
        };
        let text = serde_json::to_string_pretty(&settings).unwrap();
        std::fs::write(&path, text + "\n").unwrap();
        let loaded: Settings =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.theme, Theme::Light);
    }
}
