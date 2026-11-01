//! Agent 级统计：跨会话汇总用量、成本、工具与活动趋势。
//!
//! 数据源只有一个 —— 会话载体里的 `SessionEntry`（git 分支上的活跃路径）。
//! 这里不写账本、不落缓存文件：统计是读侧投影，会话条目才是真相。会话很多时
//! 通过 [`MAX_SESSIONS`] 限制单次扫描量，并在响应里明确标记 `sessionsTruncated`，
//! 不用截断后的数字假装全量。
//!
//! 口径与 `pipi_agent::stats` 保持一致：`input` 只计未命中缓存的提示词 token，
//! 提示词总量 = `input + cache_read + cache_write`；成本按 models.dev 的每百万
//! token 单价估算，没有可信价格时返回 `cost: None` 与 `unpricedCalls`，不编造 0。

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::runtime::agent_repo;
use pipi_protocol::Message;
use pipi_protocol::session::{
    EntryKind, SessionEntry, SessionRepo, SessionSummary, active_path, background_result_body,
    summary_text,
};
use pipi_venv::agents;

mod pricing;
use pricing::Pricing;

/// 单次最多扫描的会话数（按最近活跃倒序）。
pub const MAX_SESSIONS: usize = 300;
const DAY_MS: u64 = 86_400_000;

/// 一次 Agent 级统计请求的完整响应。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStats {
    pub agent_name: String,
    /// 统计生成时间（unix 毫秒）。
    pub generated_at: u64,
    /// `None` = 全部历史；`Some(n)` = 最近 n 天。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_days: Option<u32>,
    pub totals: StatsTotals,
    /// 活跃日，按时间升序（只含有活动的日期；前端按需补零）。
    pub days: Vec<DayStats>,
    /// 按调用次数倒序。
    pub models: Vec<ModelStats>,
    /// 按调用次数倒序。
    pub tools: Vec<ToolStats>,
    /// 按最后活跃时间倒序。
    pub sessions: Vec<SessionStatsRow>,
    /// 范围内会话数超过 [`MAX_SESSIONS`] 时为 true。
    pub sessions_truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsTotals {
    pub sessions: u64,
    /// 真实用户输入（排除压缩摘要与后台任务通知）。
    pub user_messages: u64,
    /// 助手调用 + 摘要压缩调用。
    pub assistant_calls: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// input + output + cache_read + cache_write。
    pub tokens: u64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    pub compactions: u64,
    pub assistant_errors: u64,
    pub aborted: u64,
    /// 助手调用累计耗时（模型生成时间，不含工具）。
    pub generation_ms: u64,
    /// 按 models.dev 单价估算的 USD 成本；没有任何可信价格时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// 没有可信价格、未计入 `cost` 的调用数。
    pub unpriced_calls: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayStats {
    /// 当天 UTC 零点（unix 毫秒）；前端按本地时区展示日期。
    pub day_start: u64,
    pub user_messages: u64,
    pub assistant_calls: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub tokens: u64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    pub assistant_errors: u64,
    /// 当天有活动的会话数（按会话去重）。
    pub sessions: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    pub unpriced_calls: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStats {
    pub model: String,
    pub calls: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub tokens: u64,
    pub errors: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    pub unpriced_calls: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStats {
    pub name: String,
    pub calls: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatsRow {
    pub id: String,
    pub title: String,
    pub started_at: u64,
    pub last_active: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub user_messages: u64,
    pub assistant_calls: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub tokens: u64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    pub compactions: u64,
    pub assistant_errors: u64,
    pub aborted: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    pub unpriced_calls: u64,
}

/// 扫描一个 Agent 的活跃会话并聚合。`range_days = None` 表示全部历史。
pub fn agent_stats(agent_name: &str, range_days: Option<u32>) -> Result<AgentStats, String> {
    let def = agents::load_agent(agent_name)?;
    let repo = agent_repo(&def)?;
    let now = pipi_protocol::now_millis();
    let since = range_days.map(|days| now.saturating_sub(u64::from(days).saturating_mul(DAY_MS)));

    let mut aggregator = Aggregator::new(agent_name, range_days, now);

    let summaries = repo.list();
    let mut scanned = 0usize;
    for summary in &summaries {
        if since.is_some_and(|start| summary.last_active < start) {
            continue;
        }
        if scanned >= MAX_SESSIONS {
            aggregator.stats.sessions_truncated = true;
            break;
        }
        scanned += 1;
        match repo.entries(&summary.id) {
            Ok(entries) => aggregator.add_session(summary, &entries, since),
            // 单个会话坏掉不影响整页统计；跳过并在 stderr 留下线索。
            Err(error) => eprintln!(
                "pipi: 统计跳过会话 {}（{}）：{error}",
                summary.id, agent_name
            ),
        }
    }

    Ok(aggregator.finish())
}

struct Aggregator {
    stats: AgentStats,
    days: BTreeMap<u64, DayStats>,
    day_sessions: BTreeMap<u64, HashSet<String>>,
    models: HashMap<String, ModelStats>,
    tools: HashMap<String, ToolStats>,
    pricing: Pricing,
}

impl Aggregator {
    fn new(agent_name: &str, range_days: Option<u32>, now: u64) -> Self {
        Self {
            stats: AgentStats {
                agent_name: agent_name.to_string(),
                generated_at: now,
                range_days,
                ..Default::default()
            },
            days: BTreeMap::new(),
            day_sessions: BTreeMap::new(),
            models: HashMap::new(),
            tools: HashMap::new(),
            pricing: Pricing::load(),
        }
    }

    fn add_session(
        &mut self,
        summary: &SessionSummary,
        entries: &[SessionEntry],
        since: Option<u64>,
    ) {
        let mut row = SessionStatsRow {
            id: summary.id.clone(),
            title: summary.title.clone(),
            started_at: summary.started_at,
            last_active: summary.last_active,
            model: summary.model.clone(),
            ..Default::default()
        };
        // 摘要压缩条目本身不记模型；用同一条会话里最近一次助手调用的模型估价。
        let mut last_model: Option<String> = summary.model.clone();
        let mut touched = false;

        for entry in active_path(entries) {
            if since.is_some_and(|start| entry.timestamp < start) {
                continue;
            }
            let day_start = entry.timestamp / DAY_MS * DAY_MS;
            let mut recorded = false;
            match &entry.kind {
                EntryKind::Message { message } => match message {
                    Message::User { .. } => {
                        // 压缩摘要与后台任务通知在回放里也是 User 消息，但不是用户输入。
                        if summary_text(message).is_none()
                            && background_result_body(message).is_none()
                        {
                            row.user_messages += 1;
                            self.stats.totals.user_messages += 1;
                            self.days.entry(day_start).or_default().user_messages += 1;
                            recorded = true;
                            touched = true;
                        }
                    }
                    Message::Assistant {
                        model,
                        usage,
                        stop_reason,
                        duration_ms,
                        ..
                    } => {
                        if !model.is_empty() {
                            last_model = Some(model.clone());
                            row.model = Some(model.clone());
                        }
                        let pricing_model = if model.is_empty() {
                            last_model.as_deref().unwrap_or("")
                        } else {
                            model.as_str()
                        };
                        let call_cost = self.pricing.lookup(pricing_model).map(|price| {
                            price.estimate(
                                usage.input,
                                usage.output,
                                usage.cache_read,
                                usage.cache_write,
                            )
                        });
                        row.add_call(usage, call_cost);
                        self.stats.totals.add_call(usage, call_cost);
                        self.days
                            .entry(day_start)
                            .or_default()
                            .add_call(usage, call_cost);
                        if !model.is_empty() {
                            self.models
                                .entry(model.clone())
                                .or_insert_with(|| ModelStats {
                                    model: model.clone(),
                                    ..Default::default()
                                })
                                .add_call(usage, call_cost);
                        }
                        if let Some(ms) = duration_ms {
                            self.stats.totals.generation_ms += ms;
                        }
                        if stop_reason == &pipi_protocol::StopReason::Error {
                            row.assistant_errors += 1;
                            self.stats.totals.assistant_errors += 1;
                            self.days.entry(day_start).or_default().assistant_errors += 1;
                            if let Some(model) = self.models.get_mut(model) {
                                model.errors += 1;
                            }
                        }
                        if stop_reason == &pipi_protocol::StopReason::Aborted {
                            row.aborted += 1;
                            self.stats.totals.aborted += 1;
                        }
                        recorded = true;
                        touched = true;
                    }
                    Message::ToolResult {
                        tool_name,
                        is_error,
                        ..
                    } => {
                        row.tool_calls += 1;
                        self.stats.totals.tool_calls += 1;
                        self.days.entry(day_start).or_default().tool_calls += 1;
                        let tool =
                            self.tools
                                .entry(tool_name.clone())
                                .or_insert_with(|| ToolStats {
                                    name: tool_name.clone(),
                                    ..Default::default()
                                });
                        tool.calls += 1;
                        if *is_error {
                            row.tool_errors += 1;
                            self.stats.totals.tool_errors += 1;
                            self.days.entry(day_start).or_default().tool_errors += 1;
                            tool.errors += 1;
                        }
                        recorded = true;
                        touched = true;
                    }
                },
                EntryKind::Compaction { usage, .. } => {
                    row.compactions += 1;
                    self.stats.totals.compactions += 1;
                    if let Some(usage) = usage {
                        let model = last_model.as_deref().unwrap_or("");
                        let call_cost = self.pricing.lookup(model).map(|price| {
                            price.estimate(
                                usage.input,
                                usage.output,
                                usage.cache_read,
                                usage.cache_write,
                            )
                        });
                        row.add_call(usage, call_cost);
                        self.stats.totals.add_call(usage, call_cost);
                        self.days
                            .entry(day_start)
                            .or_default()
                            .add_call(usage, call_cost);
                        if !model.is_empty() {
                            self.models
                                .entry(model.to_string())
                                .or_insert_with(|| ModelStats {
                                    model: model.to_string(),
                                    ..Default::default()
                                })
                                .add_call(usage, call_cost);
                        }
                    }
                    recorded = true;
                    touched = true;
                }
                _ => {}
            }
            if recorded {
                self.day_sessions
                    .entry(day_start)
                    .or_default()
                    .insert(summary.id.clone());
            }
        }

        if touched {
            self.stats.totals.sessions += 1;
            self.stats.sessions.push(row);
        }
    }

    fn finish(mut self) -> AgentStats {
        for (day_start, mut day) in self.days {
            day.tokens = token_total(day.input, day.output, day.cache_read, day.cache_write);
            day.sessions = self.day_sessions.get(&day_start).map_or(0, HashSet::len) as u64;
            self.stats.days.push(DayStats { day_start, ..day });
        }
        let mut models = self.models.into_values().collect::<Vec<_>>();
        models.sort_by(|a, b| {
            b.calls
                .cmp(&a.calls)
                .then(b.tokens.cmp(&a.tokens))
                .then(a.model.cmp(&b.model))
        });
        let mut tools = self.tools.into_values().collect::<Vec<_>>();
        tools.sort_by(|a, b| b.calls.cmp(&a.calls).then(a.name.cmp(&b.name)));
        let mut sessions = std::mem::take(&mut self.stats.sessions);
        sessions.sort_by_key(|session| std::cmp::Reverse(session.last_active));
        self.stats.models = models;
        self.stats.tools = tools;
        self.stats.sessions = sessions;
        self.stats.totals.tokens = token_total(
            self.stats.totals.input,
            self.stats.totals.output,
            self.stats.totals.cache_read,
            self.stats.totals.cache_write,
        );
        self.stats
    }
}

fn token_total(input: u64, output: u64, cache_read: u64, cache_write: u64) -> u64 {
    input + output + cache_read + cache_write
}

trait AddUsage {
    fn add_call(&mut self, usage: &pipi_protocol::Usage, cost: Option<f64>);
}

impl AddUsage for StatsTotals {
    fn add_call(&mut self, usage: &pipi_protocol::Usage, cost: Option<f64>) {
        self.assistant_calls += 1;
        self.input += usage.input;
        self.output += usage.output;
        self.cache_read += usage.cache_read;
        self.cache_write += usage.cache_write;
        self.tokens = token_total(self.input, self.output, self.cache_read, self.cache_write);
        if let Some(value) = cost {
            self.cost = Some(round_cost(self.cost.unwrap_or(0.0) + value));
        } else {
            self.unpriced_calls += 1;
        }
    }
}

impl AddUsage for DayStats {
    fn add_call(&mut self, usage: &pipi_protocol::Usage, cost: Option<f64>) {
        self.assistant_calls += 1;
        self.input += usage.input;
        self.output += usage.output;
        self.cache_read += usage.cache_read;
        self.cache_write += usage.cache_write;
        self.tokens = token_total(self.input, self.output, self.cache_read, self.cache_write);
        if let Some(value) = cost {
            self.cost = Some(round_cost(self.cost.unwrap_or(0.0) + value));
        } else {
            self.unpriced_calls += 1;
        }
    }
}

impl AddUsage for ModelStats {
    fn add_call(&mut self, usage: &pipi_protocol::Usage, cost: Option<f64>) {
        self.calls += 1;
        self.input += usage.input;
        self.output += usage.output;
        self.cache_read += usage.cache_read;
        self.cache_write += usage.cache_write;
        self.tokens = token_total(self.input, self.output, self.cache_read, self.cache_write);
        if let Some(value) = cost {
            self.cost = Some(round_cost(self.cost.unwrap_or(0.0) + value));
        } else {
            self.unpriced_calls += 1;
        }
    }
}

impl SessionStatsRow {
    fn add_call(&mut self, usage: &pipi_protocol::Usage, cost: Option<f64>) {
        self.assistant_calls += 1;
        self.input += usage.input;
        self.output += usage.output;
        self.cache_read += usage.cache_read;
        self.cache_write += usage.cache_write;
        self.tokens = token_total(self.input, self.output, self.cache_read, self.cache_write);
        if let Some(value) = cost {
            self.cost = Some(round_cost(self.cost.unwrap_or(0.0) + value));
        } else {
            self.unpriced_calls += 1;
        }
    }
}

fn round_cost(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipi_protocol::{StopReason, Usage};
    use pipi_provider::catalog::ModelCost;

    fn summary(id: &str, model: &str) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            title: "test".into(),
            message_count: 0,
            started_at: 0,
            last_active: 10_000,
            model: Some(model.into()),
        }
    }

    fn assistant(model: &str, usage: Usage, stop: StopReason, timestamp: u64) -> SessionEntry {
        SessionEntry {
            id: format!("a-{timestamp}"),
            parent_id: None,
            seq: timestamp,
            timestamp,
            kind: EntryKind::Message {
                message: Message::Assistant {
                    content: Vec::new(),
                    api: "openai-completions".into(),
                    provider: "test".into(),
                    model: model.into(),
                    usage,
                    stop_reason: stop,
                    error_message: None,
                    error_kind: None,
                    timestamp,
                    duration_ms: Some(1_000),
                },
            },
        }
    }

    fn tool(name: &str, is_error: bool, timestamp: u64) -> SessionEntry {
        SessionEntry {
            id: format!("t-{timestamp}"),
            parent_id: None,
            seq: timestamp,
            timestamp,
            kind: EntryKind::Message {
                message: Message::ToolResult {
                    tool_call_id: "call".into(),
                    tool_name: name.into(),
                    content: Vec::new(),
                    is_error,
                    details: None,
                    timestamp,
                },
            },
        }
    }

    fn chain(mut entries: Vec<SessionEntry>) -> Vec<SessionEntry> {
        for index in 1..entries.len() {
            entries[index].parent_id = Some(entries[index - 1].id.clone());
        }
        entries
    }

    fn user(text: &str, timestamp: u64) -> SessionEntry {
        SessionEntry {
            id: format!("u-{timestamp}"),
            parent_id: None,
            seq: timestamp,
            timestamp,
            kind: EntryKind::Message {
                message: Message::user_text(text),
            },
        }
    }

    #[test]
    fn wire_fields_are_camel_case() {
        let value = serde_json::to_value(AgentStats::default()).expect("serialize");
        assert!(value.get("agentName").is_some());
        assert!(value.get("sessionsTruncated").is_some());
        assert!(value.get("totals").is_some());
        let totals = serde_json::to_value(StatsTotals::default()).expect("serialize");
        assert!(totals.get("assistantCalls").is_some());
        assert!(totals.get("cacheRead").is_some());
        assert!(totals.get("unpricedCalls").is_some());
    }

    #[test]
    fn aggregates_usage_cost_tools_and_days() {
        let mut aggregator = Aggregator::new("a", Some(30), 1_000_000);
        aggregator.pricing.by_id.insert(
            "m".into(),
            ModelCost {
                input: 1.0,
                output: 2.0,
                cache_read: 0.1,
                cache_write: 0.0,
            },
        );
        let entries = chain(vec![
            user("hello", 1_000),
            assistant(
                "m",
                Usage {
                    input: 1_000_000,
                    output: 500_000,
                    cache_read: 2_000_000,
                    cache_write: 0,
                    total_tokens: 3_500_000,
                },
                StopReason::Stop,
                2_000,
            ),
            tool("read", false, 3_000),
            tool("bash", true, 4_000),
        ]);
        aggregator.add_session(&summary("s1", "m"), &entries, None);
        let stats = aggregator.finish();

        assert_eq!(stats.totals.sessions, 1);
        assert_eq!(stats.totals.user_messages, 1);
        assert_eq!(stats.totals.assistant_calls, 1);
        assert_eq!(stats.totals.input, 1_000_000);
        assert_eq!(stats.totals.output, 500_000);
        assert_eq!(stats.totals.cache_read, 2_000_000);
        assert_eq!(stats.totals.tool_calls, 2);
        assert_eq!(stats.totals.tool_errors, 1);
        // 1*1 + 0.5*2 + 2*0.1 = 2.2 USD
        assert_eq!(stats.totals.cost, Some(2.2));
        assert_eq!(stats.totals.unpriced_calls, 0);
        assert_eq!(stats.sessions[0].tool_errors, 1);
        assert_eq!(stats.tools[0].name, "bash");
        assert_eq!(stats.days.len(), 1);
        assert_eq!(stats.days[0].assistant_calls, 1);
    }

    #[test]
    fn unknown_price_is_reported_not_zeroed() {
        let mut aggregator = Aggregator::new("a", None, 1_000_000);
        let entries = chain(vec![assistant(
            "unknown",
            Usage {
                input: 100,
                output: 50,
                cache_read: 0,
                cache_write: 0,
                total_tokens: 150,
            },
            StopReason::Stop,
            2_000,
        )]);
        aggregator.add_session(&summary("s1", "unknown"), &entries, None);
        let stats = aggregator.finish();
        assert_eq!(stats.totals.cost, None);
        assert_eq!(stats.totals.unpriced_calls, 1);
        assert_eq!(stats.models[0].cost, None);
    }

    #[test]
    fn range_filters_entries_and_sessions() {
        let mut aggregator = Aggregator::new("a", Some(1), 2 * DAY_MS);
        let entries = chain(vec![
            user("old", DAY_MS / 2),
            assistant(
                "m",
                Usage {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                    total_tokens: 15,
                },
                StopReason::Stop,
                DAY_MS / 2,
            ),
            user("new", 2 * DAY_MS),
        ]);
        aggregator.add_session(&summary("s1", "m"), &entries, Some(DAY_MS + 1));
        let stats = aggregator.finish();
        assert_eq!(stats.totals.assistant_calls, 0);
        assert_eq!(stats.totals.user_messages, 1);
        assert_eq!(stats.sessions.len(), 1);
    }

    #[test]
    fn compaction_usage_is_counted_as_a_call() {
        let mut aggregator = Aggregator::new("a", None, 1_000_000);
        let entries = chain(vec![
            assistant(
                "m",
                Usage {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                    total_tokens: 15,
                },
                StopReason::Stop,
                1_000,
            ),
            SessionEntry {
                id: "c-1".into(),
                parent_id: None,
                seq: 2,
                timestamp: 2_000,
                kind: EntryKind::Compaction {
                    summary: "summary".into(),
                    keep_from_entry: String::new(),
                    source_tip: String::new(),
                    strategy: "test".into(),
                    usage: Some(Usage {
                        input: 20,
                        output: 5,
                        cache_read: 0,
                        cache_write: 0,
                        total_tokens: 25,
                    }),
                },
            },
        ]);
        aggregator.add_session(&summary("s1", "m"), &entries, None);
        let stats = aggregator.finish();
        assert_eq!(stats.totals.compactions, 1);
        assert_eq!(stats.totals.assistant_calls, 2);
        assert_eq!(stats.totals.input, 30);
    }

    #[test]
    fn compaction_summary_is_not_a_user_message() {
        let entries = vec![SessionEntry {
            id: "c-1".into(),
            parent_id: None,
            seq: 1,
            timestamp: 1_000,
            kind: EntryKind::Compaction {
                summary: "summary".into(),
                keep_from_entry: String::new(),
                source_tip: String::new(),
                strategy: "test".into(),
                usage: None,
            },
        }];
        let mut aggregator = Aggregator::new("a", None, 1_000_000);
        aggregator.add_session(&summary("s1", "m"), &entries, None);
        let stats = aggregator.finish();
        assert_eq!(stats.totals.user_messages, 0);
        assert_eq!(stats.totals.compactions, 1);
    }

    #[test]
    fn cost_rounds_and_accumulates() {
        let mut totals = StatsTotals::default();
        totals.add_call(
            &Usage {
                input: 1,
                output: 1,
                cache_read: 0,
                cache_write: 0,
                total_tokens: 2,
            },
            Some(0.0000001),
        );
        totals.add_call(
            &Usage {
                input: 1,
                output: 1,
                cache_read: 0,
                cache_write: 0,
                total_tokens: 2,
            },
            Some(0.0000002),
        );
        assert_eq!(totals.cost, Some(0.000000));
        assert_eq!(totals.assistant_calls, 2);
    }
}
