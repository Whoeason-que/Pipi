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
    let legacy: Settings = serde_json::from_str(r#"{"theme":"dark","providers":[]}"#).unwrap();
    assert_eq!(legacy.retry, RetrySettings::default());
    let policy = legacy.retry.policy();
    assert_eq!(policy.max_attempts, 3);
    assert_eq!(policy.base_delay_ms, 1_000);
    assert_eq!(policy.max_delay_ms, 30_000);

    // 只写一项 → 其余取默认；序列化字段是 camelCase
    let partial: Settings =
        serde_json::from_str(r#"{"theme":"dark","providers":[],"retry":{"maxAttempts":1}}"#)
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
    // 改写进程环境前持锁：见 `set_env_var` 的安全约定
    let _guard = pipi_test_support::HOME_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let provider = ProviderConfig {
        id: "t".into(),
        name: "T".into(),
        api: Api::OpenAICompletions,
        base_url: "https://x".into(),
        env_key: Some("PIPI_TEST_KEY".into()),
        api_key: Some("plain".into()),
    };
    pipi_test_support::set_env_var("PIPI_TEST_KEY", "from-env");
    assert_eq!(provider.resolve_api_key().as_deref(), Some("from-env"));
    pipi_test_support::remove_env_var("PIPI_TEST_KEY");
    assert_eq!(provider.resolve_api_key().as_deref(), Some("plain"));
}

#[test]
fn save_and_load_roundtrip() {
    let dir = std::env::temp_dir().join(format!("pipi-settings-{}", pipi_agent::session::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    let settings = Settings {
        theme: Theme::Light,
        ..Settings::default()
    };
    let text = serde_json::to_string_pretty(&settings).unwrap();
    std::fs::write(&path, text + "\n").unwrap();
    let loaded: Settings = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(loaded.theme, Theme::Light);
}
