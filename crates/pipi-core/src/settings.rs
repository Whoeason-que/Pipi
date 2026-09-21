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
