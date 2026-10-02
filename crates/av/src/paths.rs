//! 路径布局的单一来源。
//!
//! - **av 家目录**（`AV_HOME`，默认 `~/.av`）：skills store 与索引 ——
//!   全局技能包安装在这里，与任何宿主无关；
//! - **Pipi 数据根**（`PIPI_HOME`，默认 `~/.pipi`）：agents / settings /
//!   cache 的骨架，供 pipi-core 等宿主复用（此前散落在多个模块）。
//!
//! 环境变量只接受显式覆盖值（不做 `~` 展开、不做相对路径解析）；未设置或
//! 为空串时回退默认目录。所有函数在无法定位主目录时返回 `None`。

use std::path::PathBuf;

/// av 家目录的覆盖变量。
pub const AV_HOME_ENV: &str = "AV_HOME";
/// Pipi 数据根的覆盖变量。
pub const PIPI_HOME_ENV: &str = "PIPI_HOME";

/// 纯函数：显式覆盖（非空）> 主目录下的默认目录名。
fn home_from(
    override_value: Option<String>,
    home: Option<PathBuf>,
    default_dir: &str,
) -> Option<PathBuf> {
    match override_value {
        Some(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => home.map(|home| home.join(default_dir)),
    }
}

fn env_override(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// av 家目录：`$AV_HOME` 或 `~/.av`。
pub fn av_home() -> Option<PathBuf> {
    home_from(env_override(AV_HOME_ENV), dirs::home_dir(), ".av")
}

/// 全局技能 store 根：`<av_home>/skills`。
pub fn skills_store() -> Option<PathBuf> {
    av_home().map(|home| home.join("skills"))
}

/// store 索引文件：`<av_home>/skills.lock`。
pub fn skills_index_path() -> Option<PathBuf> {
    av_home().map(|home| home.join("skills.lock"))
}

/// 安装暂存目录：`<av_home>/.tmp`（与 store 同盘，保证发布 rename 原子）。
pub fn staging_dir() -> Option<PathBuf> {
    av_home().map(|home| home.join(".tmp"))
}

/// Pipi 数据根：`$PIPI_HOME` 或 `~/.pipi`。
pub fn pipi_home() -> Option<PathBuf> {
    home_from(env_override(PIPI_HOME_ENV), dirs::home_dir(), ".pipi")
}

/// Agent 数据根：`<pipi_home>/agents`。
pub fn agents_root() -> Option<PathBuf> {
    pipi_home().map(|home| home.join("agents"))
}

/// 单个 Agent 目录：`<pipi_home>/agents/<name>`。
/// 名称合法性与 symlink 边界由调用方（pipi-core 的 `ensure_real_directory`）把关。
pub fn agent_dir(name: &str) -> Option<PathBuf> {
    agents_root().map(|root| root.join(name))
}

/// 模型目录缓存：`<pipi_home>/cache/models.json`。
pub fn catalog_cache_path() -> Option<PathBuf> {
    pipi_home().map(|home| home.join("cache").join("models.json"))
}

/// 设置文件：`<pipi_home>/settings.json`。
pub fn settings_path() -> Option<PathBuf> {
    pipi_home().map(|home| home.join("settings.json"))
}

#[cfg(test)]
mod tests {
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
}
