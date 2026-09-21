//! 上下文压缩：策略 + 流水线。
//!
//! 设计要点（对齐 pi 的 `preparation` 与 codex 的 compaction 生命周期思路）：
//!
//! - **触发**由 runtime 决定（预算 + turn 边界），策略只回答「我能不能压」；
//! - **策略产出 [`Plan`]，不直接改历史** —— 于是不变量能统一校验、UI 能观测、
//!   落盘能回放，三件事都只依赖这一层；
//! - 策略分两类，这条线决定扩展成本：
//!   - [`Projection`]（投影式）：**可从原始历史重算** —— 纯函数、确定性、
//!     不落盘，只在组装请求时应用（`transform_context`）。加一档不动会话格式。
//!   - [`Replacement`]（替换式）：信息已丢失（摘要），必须落盘成 compaction
//!     条目并在回放时重建。
//!
//! 两类刻意用两个 trait：投影式是同步纯函数，替换式要发请求（async）且改变
//! 持久状态 —— 合成一个 trait 只会逼出「假的 async」，或让投影式拿到不该有的
//! 写权限。

mod llm;
mod tool_output;

pub use llm::LlmSummarize;
pub use tool_output::{ToolOutputPrune, CLEARED_TOOL_RESULT_PREFIX};

use crate::context::{estimate_context_tokens, prune_cut_index, DEFAULT_RESERVE_TOKENS};
use crate::provider::Provider;
use crate::types::{AbortSignal, Message, Model, StreamOptions, Usage};

/// 保留近期原文轮次的 token 预算（对齐 pi 的 keepRecentTokens 默认值）。
pub const KEEP_RECENT_TOKENS: u64 = 20_000;

/// 摘要正文的 token 上限（约 4000 字；防摘要失控，无需等于 reserve 全额）。
pub const SUMMARY_MAX_TOKENS: u32 = 1_024;

/// 旧工具输出清理：最近这么多个工具输出保持原样（还在用的细节别动）。
pub const TOOL_OUTPUT_KEEP: usize = 6;

/// 小于这个字符数的工具输出不值得清理 —— 占位符本身也要花 token。
pub const TOOL_OUTPUT_MIN_CHARS: usize = 400;

/// 阈值百分比归一化：0 / >100 视为「没设过」，用默认值。
pub fn normalize_threshold_percent(percent: u8) -> u8 {
    if (1..=100).contains(&percent) {
        percent
    } else {
        DEFAULT_THRESHOLD_PERCENT
    }
}

/// 压缩预算。默认值集中在这里，从模型的上下文窗口推导；调参只改这一处。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    /// 模型上下文窗口；0 表示未知（不启用压缩）。
    pub context_window: u64,
    /// 自动压缩阈值：上下文占用达到窗口的这个百分比时触发（1–100）。
    /// 越界值在 [`Budget::from_window`] 里归一化，所以这里始终是合法值。
    pub threshold_percent: u8,
    /// 给模型回复预留的空间。
    pub reserve: u64,
    /// 保留近期原文轮次的 token 预算。
    pub keep_recent: u64,
    /// 摘要正文的 token 上限。
    pub summary_max: u32,
    /// 保留最近几个工具输出的原文。
    pub tool_output_keep: usize,
    /// 小于这个字符数的工具输出不值得清理。
    pub tool_output_min_chars: usize,
}

/// 默认自动压缩阈值（上下文占用达到窗口的这个百分比时压缩）。
pub const DEFAULT_THRESHOLD_PERCENT: u8 = 75;

impl Budget {
    /// `threshold_percent` 越界（0 或 >100）时退回默认值 —— 手改过的
    /// agent.json 不该让压缩失效或变得不可预期。
    pub fn from_window(context_window: u64, threshold_percent: u8) -> Budget {
        Budget {
            context_window,
            threshold_percent: normalize_threshold_percent(threshold_percent),
            reserve: DEFAULT_RESERVE_TOKENS,
            keep_recent: KEEP_RECENT_TOKENS,
            summary_max: SUMMARY_MAX_TOKENS,
            tool_output_keep: TOOL_OUTPUT_KEEP,
            tool_output_min_chars: TOOL_OUTPUT_MIN_CHARS,
        }
    }

    /// 触发阈值：占用超过「窗口 × 百分比」才值得动用压缩。
    ///
    /// 这是压缩**唯一**的触发口径 —— 投影式（清理 / 硬裁）、替换式（摘要）
    /// 与手动压缩的预检都读它，不要再各算一套。
    pub fn trigger_tokens(&self) -> u64 {
        self.context_window.saturating_mul(self.threshold_percent as u64) / 100
    }

    /// 保留尾部的预算：先给摘要自身与模型回复留位置，再收敛到 `keep_recent` 以内。
    pub fn keep_budget(&self) -> u64 {
        self.trigger_tokens()
            .saturating_sub(self.reserve)
            .saturating_sub(self.summary_max as u64 * 2)
            .min(self.keep_recent)
    }
}

/// 一次压缩的只读输入（pi 的 preparation：策略只看到「压什么、留多少」）。
pub struct Preparation<'a> {
    pub messages: &'a [Message],
    pub budget: Budget,
}

impl<'a> Preparation<'a> {
    pub fn new(messages: &'a [Message], budget: Budget) -> Self {
        Preparation { messages, budget }
    }

    /// 当前历史的 token 估算（各策略的触发判断共用）。
    pub fn tokens(&self) -> u64 {
        estimate_context_tokens(self.messages)
    }

    /// 是否已超触发阈值。
    pub fn over_budget(&self) -> bool {
        self.tokens() > self.budget.trigger_tokens()
    }

    /// 历史首条若是上次注入的摘要，取出正文 —— 重复压缩时交回给模型，
    /// 避免措辞与事实逐轮漂移（pi 的 previousSummary 语义）。
    pub fn previous_summary(&self) -> Option<&str> {
        self.messages.first().and_then(crate::session::summary_text)
    }
}

/// 替换式策略的执行环境（投影式忽略它）。
pub struct StrategyEnv<'a> {
    pub provider: &'a dyn Provider,
    pub model: &'a Model,
    pub options: &'a StreamOptions,
    pub abort: AbortSignal,
}

/// 策略产出的「改写计划」。
///
/// 策略不直接改历史：不变量统一校验、UI 可观测、落盘可回放都依赖这一层。
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// 就地改写（消息下标 → 新消息）。投影式用它，长度不变。
    pub rewrites: Vec<(usize, Message)>,
    /// 丢弃此下标之前的消息（替换式用它）。`None` = 不丢。
    pub keep_from: Option<usize>,
    /// 注入历史头部的消息（摘要消息）。
    pub injections: Vec<Message>,
    /// 生成摘要那次调用的用量（计入会话账本）。
    pub usage: Option<Usage>,
}

impl Plan {
    pub fn is_noop(&self) -> bool {
        self.rewrites.is_empty() && self.keep_from.is_none() && self.injections.is_empty()
    }

    /// 应用计划。**调用前必须过 [`validate`]**（就地改写不可与保留/注入混用，
    /// 否则下标语义会错位）。
    pub fn apply(&self, messages: &[Message]) -> Vec<Message> {
        let start = self.keep_from.unwrap_or(0).min(messages.len());
        let mut out: Vec<Message> = self.injections.clone();
        out.extend(messages[start..].iter().cloned());
        for (index, replacement) in &self.rewrites {
            if let Some(slot) = out.get_mut(*index) {
                *slot = replacement.clone();
            }
        }
        out
    }
}

/// 计划校验：压缩的硬不变量只在这一处检查，任何策略都绕不过（fail-closed）。
pub fn validate(messages: &[Message], plan: &Plan) -> Result<(), String> {
    if !plan.rewrites.is_empty() && (plan.keep_from.is_some() || !plan.injections.is_empty()) {
        return Err("计划同时使用就地改写与保留/注入：下标语义会错位".into());
    }
    for (index, replacement) in &plan.rewrites {
        let Some(original) = messages.get(*index) else {
            return Err(format!("就地改写下标越界：{index}"));
        };
        if original.role() != replacement.role() {
            return Err(format!(
                "就地改写不得改变消息角色：{} → {}",
                original.role(),
                replacement.role()
            ));
        }
    }
    if let Some(start) = plan.keep_from {
        if start > messages.len() {
            return Err(format!("保留起点越界：{start}"));
        }
        // 保留段的第一条不能是工具结果 —— 它的 assistant tool_call 已被摘要
        // 带走，端点会按协议拒绝（与 prune_oldest 同一不变量）。
        if let Some(first) = messages.get(start) {
            if first.role() == "toolResult" {
                return Err("保留起点落在工具结果上：会孤立 toolResult".into());
            }
        }
    }
    Ok(())
}

/// 应用后的历史不得比应用前**更破损**。
///
/// 用「破损度」而不是「是否合法」来判：历史里本来就可能有中段悬挂（被中断
/// 的运行留下无主调用，发送前才由 `context::repair_tool_pairing` 兜底），
/// 那种破损不是压缩造成的 —— 若要求输出必须绝对合法，一档压缩会因为别人的
/// 旧伤被整档禁用。只要求「不新增破损」。
pub fn validate_result(before: &[Message], after: &[Message]) -> Result<(), String> {
    let (missing_before, orphan_before) = pairing_damage(before);
    let (missing_after, orphan_after) = pairing_damage(after);
    if missing_after > missing_before || orphan_after > orphan_before {
        return Err(format!(
            "压缩后的历史破坏了工具配对不变量：无主调用 {missing_before}→{missing_after}、\
             孤儿结果 {orphan_before}→{orphan_after}"
        ));
    }
    Ok(())
}

/// 破损度：没有结果回答的 tool_call 数、找不到对应调用的孤儿 tool_result 数。
fn pairing_damage(messages: &[Message]) -> (usize, usize) {
    use std::collections::HashSet;
    let calls: HashSet<&str> = messages
        .iter()
        .flat_map(|message| message.tool_calls())
        .filter_map(|block| match block {
            crate::types::ContentBlock::ToolCall { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let results: HashSet<&str> = messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult { tool_call_id, .. } => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    (
        calls.difference(&results).count(),
        results.difference(&calls).count(),
    )
}

/// 投影式策略：纯函数（同步、确定性），可从原始历史重算 —— 因此**不落盘**。
pub trait Projection: Send + Sync {
    fn name(&self) -> &'static str;
    /// 前置条件；不满足则跳过（不算失败）。
    fn applies(&self, prep: &Preparation<'_>) -> bool;
    fn plan(&self, prep: &Preparation<'_>) -> Plan;
}

/// 替换式策略的异步返回类型（`Send`，可跨 await 持有）。
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// 替换式策略：信息已丢失，必须落盘成 compaction 条目并在回放时重建。
///
/// 手写 boxed future 而不用 `#[async_trait]`：这里的入参带借用（`Preparation` /
/// `StrategyEnv` 都有生命周期），宏展开出的签名与实现很难对齐，显式写出更省事。
pub trait Replacement: Send + Sync {
    fn name(&self) -> &'static str;
    fn applies(&self, prep: &Preparation<'_>) -> bool;
    fn run<'a>(
        &'a self,
        prep: &'a Preparation<'a>,
        env: &'a StrategyEnv<'a>,
    ) -> BoxFuture<'a, Result<Plan, String>>;
}

/// 硬裁剪（投影式，保底档）：超预算时从最旧的完整轮次开始丢弃。
pub struct BoundaryPrune;

impl Projection for BoundaryPrune {
    fn name(&self) -> &'static str {
        "boundary-prune"
    }

    fn applies(&self, prep: &Preparation<'_>) -> bool {
        prep.over_budget()
    }

    fn plan(&self, prep: &Preparation<'_>) -> Plan {
        Plan {
            // 硬裁是最后一档：裁到触发线为止（裁完不该马上又触发压缩）
            keep_from: prune_cut_index(prep.messages, prep.budget.trigger_tokens()),
            ..Default::default()
        }
    }
}

/// 投影式流水线：按成本从低到高（清理旧工具输出 → 硬裁）。
pub fn projections() -> Vec<Box<dyn Projection>> {
    vec![Box::new(ToolOutputPrune), Box::new(BoundaryPrune)]
}

/// 替换式流水线：目前只有摘要；未来按成本排序追加（例如服务端压缩）。
pub fn replacements() -> Vec<Box<dyn Replacement>> {
    vec![Box::new(LlmSummarize)]
}

/// 应用投影式流水线（组装请求时调用）。
///
/// 结果**不落盘**：回放时用同一份逻辑重算，于是 live 与重开自然一致 ——
/// 这是「可重算就不持久化」这条规则的直接收益。每档都过校验，任何一档
/// 出问题只跳过它自己，不影响其余档与会话。
pub fn project(messages: Vec<Message>, budget: Budget) -> Vec<Message> {
    if budget.context_window == 0 {
        return messages;
    }
    let mut current = messages;
    for strategy in projections() {
        let plan = {
            let prep = Preparation::new(&current, budget);
            if !strategy.applies(&prep) {
                continue;
            }
            strategy.plan(&prep)
        };
        if plan.is_noop() {
            continue;
        }
        if let Err(error) = validate(&current, &plan) {
            eprintln!(
                "pipi: 投影式压缩「{}」计划不合法，已跳过: {error}",
                strategy.name()
            );
            continue;
        }
        let next = plan.apply(&current);
        if let Err(error) = validate_result(&current, &next) {
            eprintln!(
                "pipi: 投影式压缩「{}」结果不合法，已跳过: {error}",
                strategy.name()
            );
            continue;
        }
        current = next;
    }
    current
}

/// 替换式压缩的结果（runtime 据此落盘 compaction 条目）。
#[derive(Debug, Clone)]
pub struct Compacted {
    /// 压缩后的完整消息历史（注入的消息在前，保留的原文在后）。
    pub messages: Vec<Message>,
    /// 被摘要替换掉的消息条数。
    pub replaced: usize,
    /// 产出它的策略名（诊断与 UI）。
    pub strategy: &'static str,
    /// 保留区间在**输入历史**里的起点下标 —— runtime 换算成条目 id 落盘
    /// （`keep_from_entry`），回放时才留得住这段原文。
    pub keep_from: Option<usize>,
    /// 摘要调用的用量（计入会话账本）。
    pub usage: Option<Usage>,
}

/// 跑一次替换式压缩：返回可落盘的结果。
///
/// 「何时压」不在这里 —— 自动路径由 runtime 用 [`needs_compaction`] 判过阈值，
/// 手动路径是「用户点了就压」；本函数只负责「怎么压」。所以这里不检查阈值，
/// 只要求**有东西可压**（历史太长、至少能切出一段旧轮次），否则返回可读错误
/// （手动路径会把它显示给用户）。
pub async fn compact(
    messages: &[Message],
    budget: Budget,
    env: &StrategyEnv<'_>,
) -> Result<Compacted, String> {
    if messages.is_empty() {
        return Err("空上下文无需压缩".into());
    }
    let prep = Preparation::new(messages, budget);
    for strategy in replacements() {
        if !strategy.applies(&prep) {
            continue;
        }
        let plan = strategy.run(&prep, env).await?;
        if plan.is_noop() {
            continue;
        }
        validate(messages, &plan)?;
        let out = plan.apply(messages);
        validate_result(messages, &out)?;
        return Ok(Compacted {
            replaced: plan.keep_from.unwrap_or(messages.len()),
            messages: out,
            strategy: strategy.name(),
            keep_from: plan.keep_from,
            usage: plan.usage,
        });
    }
    Err("没有可用的压缩策略".into())
}

/// 压缩决策：上下文是否达到触发线（供 runtime 在 turn 边界检查）。
///
/// 手动压缩不读这个函数 —— 手动就是「现在压，不管阈值」。
pub fn needs_compaction(messages: &[Message], budget: Budget) -> bool {
    if budget.context_window == 0 {
        return false;
    }
    estimate_context_tokens(messages) > budget.trigger_tokens()
}

/// 从被摘要的消息中提取文件操作（read → 读取清单，write/edit → 修改清单）。
/// glob 的 `path` 参数是搜索根而非文件，bash 参数不可靠，都跳过。
fn file_operations(messages: &[Message]) -> (Vec<String>, Vec<String>) {
    use std::collections::BTreeSet;
    let mut read = BTreeSet::new();
    let mut modified = BTreeSet::new();
    for message in messages {
        let Message::Assistant { content, .. } = message else {
            continue;
        };
        for block in content {
            let crate::types::ContentBlock::ToolCall {
                name, arguments, ..
            } = block
            else {
                continue;
            };
            let Some(path) = arguments["path"].as_str() else {
                continue;
            };
            match name.as_str() {
                "read" => {
                    read.insert(path.to_string());
                }
                "write" | "edit" => {
                    modified.insert(path.to_string());
                }
                _ => {}
            }
        }
    }
    (read.into_iter().collect(), modified.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::estimate_tokens;
    use crate::types::{ContentBlock, StopReason};

    pub(super) fn assistant_text(text: &str) -> Message {
        Message::Assistant {
            content: vec![ContentBlock::Text { text: text.into() }],
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            usage: Default::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
            duration_ms: None,
        }
    }

    pub(super) fn tool_result(text: &str) -> Message {
        Message::ToolResult {
            tool_call_id: "t1".into(),
            tool_name: "bash".into(),
            content: vec![crate::types::ToolResultContent::Text { text: text.into() }],
            is_error: false,
            details: None,
            timestamp: 0,
        }
    }

    fn tool_call(name: &str, path: &str) -> Message {
        Message::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: "c1".into(),
                name: name.into(),
                arguments: serde_json::json!({ "path": path }),
            }],
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            usage: Default::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 0,
            duration_ms: None,
        }
    }

    #[test]
    fn file_operations_extracts_read_and_modified_paths() {
        let messages = vec![
            Message::user_text("go"),
            tool_call("read", "b.rs"),
            tool_call("read", "a.rs"),
            tool_call("write", "a.rs"),
            tool_call("edit", "c.rs"),
            tool_call("bash", "irrelevant"),
            tool_result("done"),
        ];
        let (read, modified) = file_operations(&messages);
        assert_eq!(read, vec!["a.rs".to_string(), "b.rs".to_string()]);
        assert_eq!(modified, vec!["a.rs".to_string(), "c.rs".to_string()]);
    }

    #[test]
    fn budget_derives_threshold_from_window_percent() {
        // 默认 75%：20 万窗口 → 15 万触发
        let budget = Budget::from_window(200_000, DEFAULT_THRESHOLD_PERCENT);
        assert_eq!(budget.trigger_tokens(), 150_000);
        assert_eq!(budget.keep_budget(), KEEP_RECENT_TOKENS);
        // 百分比可调：50% 更早压
        assert_eq!(Budget::from_window(200_000, 50).trigger_tokens(), 100_000);
        // 窗口小：保留预算被触发线压住，不会超过可用空间
        let small = Budget::from_window(30_000, 75);
        assert_eq!(
            small.keep_budget(),
            22_500 - DEFAULT_RESERVE_TOKENS - SUMMARY_MAX_TOKENS as u64 * 2
        );
        // 越界百分比退回默认（手改 agent.json 不该让压缩失效）
        assert_eq!(Budget::from_window(200_000, 0).threshold_percent, 75);
        assert_eq!(Budget::from_window(200_000, 200).threshold_percent, 75);
        assert_eq!(normalize_threshold_percent(1), 1);
        assert_eq!(normalize_threshold_percent(100), 100);
        assert_eq!(estimate_tokens(&Message::user_text("abcdefgh")), 2);
    }

    #[test]
    fn needs_compaction_respects_zero_window() {
        assert!(!needs_compaction(
            &[Message::user_text("hi")],
            Budget::from_window(0, 75)
        ));
        assert!(needs_compaction(
            &[Message::user_text("x".repeat(100_000))],
            Budget::from_window(10_000, 75)
        ));
    }

    #[test]
    fn validate_rejects_illegal_plans() {
        let messages = vec![
            Message::user_text("hi"),
            assistant_text("ok"),
            tool_result("body"),
        ];
        // 下标越界
        assert!(validate(
            &messages,
            &Plan {
                rewrites: vec![(9, tool_result("x"))],
                ..Default::default()
            }
        )
        .is_err());
        // 就地改写不得改角色
        assert!(validate(
            &messages,
            &Plan {
                rewrites: vec![(2, assistant_text("x"))],
                ..Default::default()
            }
        )
        .is_err());
        // 就地改写与保留混用（下标会错位）
        assert!(validate(
            &messages,
            &Plan {
                rewrites: vec![(2, tool_result("x"))],
                keep_from: Some(1),
                ..Default::default()
            }
        )
        .is_err());
        // 保留起点落在工具结果上
        assert!(validate(
            &messages,
            &Plan {
                keep_from: Some(2),
                ..Default::default()
            }
        )
        .is_err());
        // 合法：只保留尾部
        assert!(validate(
            &messages,
            &Plan {
                keep_from: Some(1),
                ..Default::default()
            }
        )
        .is_ok());
    }

    #[test]
    fn plan_apply_keeps_tail_and_injects_summary() {
        let messages = vec![
            Message::user_text("old"),
            assistant_text("old answer"),
            Message::user_text("new"),
        ];
        let plan = Plan {
            keep_from: Some(2),
            injections: vec![crate::session::summary_message("旧摘要")],
            ..Default::default()
        };
        let out = plan.apply(&messages);
        assert_eq!(out.len(), 2);
        assert_eq!(crate::session::summary_text(&out[0]), Some("旧摘要"));
        assert_eq!(out[1].role(), "user");
        validate_result(&messages, &out).unwrap();
    }

    #[test]
    fn validate_result_only_rejects_new_damage() {
        // 输入本来就有的破损不算在压缩头上（中段悬挂由发送前兜底）
        let orphan = |id: &str| Message::ToolResult {
            tool_call_id: id.into(),
            tool_name: "bash".into(),
            content: vec![crate::types::ToolResultContent::Text { text: "x".into() }],
            is_error: false,
            details: None,
            timestamp: 0,
        };
        let already_broken = vec![Message::user_text("hi"), orphan("o1")];
        assert!(validate_result(&already_broken, &already_broken.clone()).is_ok());
        // 新增孤儿结果 → 拒绝
        let mut worse = already_broken.clone();
        worse.push(orphan("o2"));
        assert!(validate_result(&already_broken, &worse).is_err());
        // 新增无主调用 → 拒绝
        let mut missing = vec![Message::user_text("hi")];
        missing.push(assistant_call("t9"));
        assert!(validate_result(&[Message::user_text("hi")], &missing).is_err());
        let healthy = vec![
            Message::user_text("hi"),
            Message::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "t1".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({}),
                }],
                api: String::new(),
                provider: String::new(),
                model: String::new(),
                usage: Default::default(),
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: 0,
                duration_ms: None,
            },
            tool_result("ok"),
        ];
        assert!(validate_result(&healthy, &healthy.clone()).is_ok());
        // 保留段切在工具结果上会孤立它 → 拒绝
        let cut = vec![healthy[2].clone()];
        assert!(validate_result(&healthy, &cut).is_err());
    }

    fn assistant_call(id: &str) -> Message {
        Message::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: id.into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            }],
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            usage: Default::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 0,
            duration_ms: None,
        }
    }

    #[test]
    fn projection_pipeline_is_noop_without_pressure() {
        let messages = vec![
            Message::user_text("hi"),
            assistant_text("ok"),
            tool_result(&"x".repeat(20_000)),
        ];
        // 窗口足够大：既不清理也不裁剪（细节保留到真需要时）
        assert_eq!(project(messages.clone(), Budget::from_window(1_000_000, DEFAULT_THRESHOLD_PERCENT)), messages);
    }

    #[test]
    fn projection_pipeline_clears_tool_outputs_before_hard_pruning() {
        // 多轮历史：每轮一个很大的工具输出，整体超预算
        let mut messages = Vec::new();
        for turn in 0..8 {
            messages.push(Message::user_text(format!("问题 {turn}")));
            messages.push(assistant_text(&"思考".repeat(50)));
            messages.push(tool_result(&"输出".repeat(8_000)));
        }
        // 阈值从实测估算值反推（CJK 按字节算，别口算）：设在「清掉最旧 2 个
        // 工具输出就落回阈值内」的位置，于是只有清理档生效、硬裁不该触发
        let total = estimate_context_tokens(&messages);
        let per_result = estimate_tokens(&messages[2]);
        let cleared = 8 - TOOL_OUTPUT_KEEP;
        let trigger = total - per_result * cleared as u64 + 100;
        // 触发线直接用窗口表达（百分比 100 = 触发线即窗口），与百分比口径解耦
        let budget = Budget {
            context_window: trigger,
            threshold_percent: 100,
            ..Budget::from_window(trigger, 100)
        };
        assert!(total > budget.trigger_tokens(), "总 {total} 应超阈值");
        let after = project(messages, budget);
        // 结构不变：条数一致、工具结果仍在原位（配对不受影响）
        assert_eq!(after.len(), 24);
        validate_result(&after, &after.clone()).unwrap();
        let tool_outputs: Vec<&Message> =
            after.iter().filter(|m| m.role() == "toolResult").collect();
        assert_eq!(tool_outputs.len(), 8);
        // 前 8-KEEP 个被清理，最近 KEEP 个保留原文
        for message in &tool_outputs[..8 - TOOL_OUTPUT_KEEP] {
            let Message::ToolResult { content, .. } = message else {
                unreachable!()
            };
            let crate::types::ToolResultContent::Text { text } = &content[0] else {
                unreachable!()
            };
            assert!(
                text.starts_with(CLEARED_TOOL_RESULT_PREFIX),
                "旧输出应被清理: {text}"
            );
        }
        let Message::ToolResult { content, .. } = tool_outputs[7] else {
            unreachable!()
        };
        let crate::types::ToolResultContent::Text { text } = &content[0] else {
            unreachable!()
        };
        assert!(
            !text.starts_with(CLEARED_TOOL_RESULT_PREFIX),
            "最近的输出应保留原文"
        );
    }

    #[test]
    fn projection_is_idempotent() {
        let mut messages = Vec::new();
        for turn in 0..8 {
            messages.push(Message::user_text(format!("问题 {turn}")));
            messages.push(tool_result(&"输出".repeat(8_000)));
        }
        let total = estimate_context_tokens(&messages);
        let per_result = estimate_tokens(&messages[1]);
        let trigger = total - per_result * 4 + 100;
        let budget = Budget {
            context_window: trigger,
            threshold_percent: 100,
            ..Budget::from_window(trigger, 100)
        };
        let once = project(messages, budget);
        let twice = project(once.clone(), budget);
        assert_eq!(once, twice, "投影必须可重复应用（回放时重算不会变形）");
    }
}
