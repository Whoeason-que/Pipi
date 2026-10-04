use super::*;

#[test]
fn override_wins_over_home_default() {
    assert_eq!(
        home_from(
            Some("/custom".into()),
            Some(PathBuf::from("/home/u")),
            ".av"
        ),
        Some(PathBuf::from("/custom"))
    );
    // 空串视为未设置
    assert_eq!(
        home_from(Some(String::new()), Some(PathBuf::from("/home/u")), ".av"),
        Some(PathBuf::from("/home/u/.av"))
    );
    assert_eq!(
        home_from(None, Some(PathBuf::from("/home/u")), ".pipi"),
        Some(PathBuf::from("/home/u/.pipi"))
    );
    // 两者都不可得
    assert_eq!(home_from(None, None, ".av"), None);
}

#[test]
fn derived_paths_are_consistent() {
    let Some(home) = av_home() else {
        return; // 无主目录的环境跳过
    };
    assert_eq!(skills_store(), Some(home.join("skills")));
    assert_eq!(skills_index_path(), Some(home.join("skills.lock")));
    assert_eq!(staging_dir(), Some(home.join(".tmp")));
    let Some(pipi) = pipi_home() else { return };
    assert_eq!(agents_root(), Some(pipi.join("agents")));
    assert_eq!(agent_dir("foo"), Some(pipi.join("agents").join("foo")));
    assert_eq!(
        catalog_cache_path(),
        Some(pipi.join("cache").join("models.json"))
    );
    assert_eq!(settings_path(), Some(pipi.join("settings.json")));
}
