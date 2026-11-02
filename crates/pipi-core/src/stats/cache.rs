//! `agent_stats` 的进程内缓存：会话摘要指纹与 models.dev 价格指纹共同失效。
//!
//! 聚合本身要逐个会话读 git tree；页面切范围、手动刷新时不应重复付出这份成本。
//! 缓存只是加速，不改变「会话条目是唯一真相」：摘要（id / 最后活跃 / 条数 /
//! 标题 / 模型）任何一项变化都会换 key。

use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use pipi_protocol::session::SessionSummary;

use super::AgentStats;

const MAX_CACHE_ENTRIES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct CacheKey {
    /// `sessions.git` 路径：不同 Agent/不同数据根不会串缓存。
    pub(super) repo: PathBuf,
    pub(super) range_days: Option<u32>,
    /// 会话摘要（id / 最后活跃 / 条数 / 标题 / 模型）的指纹：append-only 载体
    /// 只要追加了条目，这几个值至少有一个会变。
    pub(super) sessions: u64,
    /// models.dev 缓存文件的 mtime + 长度：目录刷新后成本要重新估算。
    pub(super) pricing: u64,
}

impl CacheKey {
    pub(super) fn new(
        repo: PathBuf,
        range_days: Option<u32>,
        summaries: &[SessionSummary],
    ) -> Self {
        Self {
            repo,
            range_days,
            sessions: summaries_fingerprint(summaries),
            pricing: pricing_fingerprint(),
        }
    }
}

pub(super) fn summaries_fingerprint(summaries: &[SessionSummary]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for summary in summaries {
        summary.id.hash(&mut hasher);
        summary.last_active.hash(&mut hasher);
        summary.message_count.hash(&mut hasher);
        summary.started_at.hash(&mut hasher);
        summary.title.hash(&mut hasher);
        summary.model.hash(&mut hasher);
    }
    hasher.finish()
}

fn pricing_fingerprint() -> u64 {
    let Some(path) = pipi_venv::av::paths::catalog_cache_path() else {
        return 0;
    };
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    let mut hasher = DefaultHasher::new();
    meta.len().hash(&mut hasher);
    if let Ok(modified) = meta.modified()
        && let Ok(elapsed) = modified.duration_since(UNIX_EPOCH)
    {
        elapsed.as_nanos().hash(&mut hasher);
    }
    hasher.finish()
}

#[derive(Default)]
struct StatsCache {
    entries: VecDeque<(CacheKey, AgentStats)>,
}

fn stats_cache() -> &'static Mutex<StatsCache> {
    static CACHE: OnceLock<Mutex<StatsCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(StatsCache::default()))
}

pub(super) fn cache_lookup(key: &CacheKey) -> Option<AgentStats> {
    let mut cache = stats_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = cache.entries.iter().position(|(item, _)| item == key)?;
    let entry = cache.entries.remove(index)?;
    let stats = entry.1.clone();
    cache.entries.push_front(entry);
    Some(stats)
}

pub(super) fn cache_store(key: CacheKey, stats: AgentStats) {
    let mut cache = stats_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(index) = cache.entries.iter().position(|(item, _)| item == &key) {
        cache.entries.remove(index);
    }
    cache.entries.push_front((key, stats));
    while cache.entries.len() > MAX_CACHE_ENTRIES {
        cache.entries.pop_back();
    }
}
