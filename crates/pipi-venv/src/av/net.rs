//! skills.sh 搜索客户端（`GET /api/search`，无鉴权，公开接口）。
//!
//! 只做**搜索**：结果是 `owner/repo` 短名，直接喂给 `av skill install`
//! （安装走 git，不依赖 skills.sh 的直链/归档）。
//!
//! 该模块由 `search` feature 门控（默认开启）；pipi-core 以
//! `default-features = false` 依赖 av，桌面应用不会链入第二个 HTTP 栈。
//! 可用 `SKILLS_API_URL` 环境变量覆盖基址（测试/镜像）。

use std::time::Duration;

use serde::{Deserialize, Serialize};

const DEFAULT_BASE: &str = "https://skills.sh";
const SEARCH_LIMIT_MAX: u32 = 50;
const TIMEOUT_SECS: u64 = 20;

/// 一条搜索结果。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SkillSearchResult {
    pub name: String,
    /// `owner/repo`（可直接作为 `av skill install` 的来源）。
    pub source: String,
    pub installs: u64,
    /// 技能页链接（skills.sh）。
    pub url: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    skills: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    name: String,
    #[serde(default)]
    installs: u64,
    source: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    slug: Option<String>,
}

/// 调用 skills.sh 搜索接口；网络/解析失败即报错（fail-closed，不静默返回空）。
pub fn search_skills(query: &str, limit: u32) -> Result<Vec<SkillSearchResult>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("搜索关键词不能为空".into());
    }
    let limit = limit.clamp(1, SEARCH_LIMIT_MAX);
    let base = std::env::var("SKILLS_API_URL").unwrap_or_else(|_| DEFAULT_BASE.to_string());
    let url = format!(
        "{}/api/search?q={}&limit={}",
        base.trim_end_matches('/'),
        percent_encode(query),
        limit
    );
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(TIMEOUT_SECS)))
        .build();
    let agent: ureq::Agent = config.into();
    let mut response = agent
        .get(&url)
        .call()
        .map_err(|e| format!("skills.sh 搜索失败（{url}）：{e}"))?;
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("读取 skills.sh 响应失败：{e}"))?;
    let parsed: SearchResponse =
        serde_json::from_str(&body).map_err(|e| format!("解析 skills.sh 响应失败：{e}"))?;
    Ok(parsed
        .skills
        .into_iter()
        .map(|item| {
            let slug = item.slug.or(item.id).unwrap_or_default();
            let url = if slug.is_empty() {
                format!("{}/", base.trim_end_matches('/'))
            } else {
                format!("{}/{slug}", base.trim_end_matches('/'))
            };
            SkillSearchResult {
                name: item.name,
                source: item.source,
                installs: item.installs,
                url,
            }
        })
        .collect())
}

/// 最小百分号编码（unreserved 字符集外全部转义）；不引入 URL 依赖。
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let pass = byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~');
        if pass {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding() {
        assert_eq!(percent_encode("pdf"), "pdf");
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("中文"), "%E4%B8%AD%E6%96%87");
        assert_eq!(percent_encode("a/b?c"), "a%2Fb%3Fc");
    }

    #[test]
    fn empty_query_rejected() {
        assert!(search_skills("  ", 10).is_err());
    }

    #[test]
    fn response_parsing() {
        let body = r#"{"skills":[{"id":"o/r/pdf","name":"pdf","installs":123,"source":"o/r"}]}"#;
        let parsed: SearchResponse = serde_json::from_str(body).unwrap();
        let item = &parsed.skills[0];
        assert_eq!(item.name, "pdf");
        assert_eq!(item.source, "o/r");
        assert_eq!(item.id.as_deref(), Some("o/r/pdf"));

        // 缺字段时用默认值（installs 缺失不报错）
        let body = r#"{"skills":[{"name":"x","source":"a/b"}]}"#;
        let parsed: SearchResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.skills[0].installs, 0);
    }
}
