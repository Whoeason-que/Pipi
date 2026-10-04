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
pub use tool_output::{CLEARED_TOOL_RESULT_PREFIX, ToolOutputPrune};

use crate::context::{DEFAULT_RESERVE_TOKENS, estimate_context_tokens, prune_cut_index};
use pipi_protocol::{AbortSignal, Message, Model, StreamOptions, Usage};
use pipi_provider::Provider;

/// 保留近期原文轮次的 token 预算（对齐 pi 的 keepRecentTokens 默认值）。
pub const KEEP_RECENT_TOKENS: u64 = 20_000;

/// 摘要请求的最大输出预算（含模型的思考 token）。对齐 pi：最多使用预留空间的 80%。
pub const SUMMARY_MAX_TOKENS: u32 = (DEFAULT_RESERVE_TOKENS as u32) * 4 / 5;

/// 小窗口仍给摘要留出基本的输出空间；这不是大窗口的固定上限。
const MIN_SUMMARY_MAX_TOKENS: u32 = 1_024;

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
    /// 摘要请求的输出 token 上限。
    pub summary_max: u32,
    /// 可选的压缩后上下文目标（占压缩前估算 token 的百分比）；None 保持旧尾部预算。
    pub target_percent: Option<u8>,
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
        let threshold_percent = normalize_threshold_percent(threshold_percent);
        let trigger_tokens = context_window.saturating_mul(threshold_percent as u64) / 100;
        // keep_budget 会为摘要扣两倍预算；小窗口最多用剩余空间的 1/4，
        // 这样还有至少一半可用于保留近期原文（直到 keep_recent 上限）。
        let available = trigger_tokens.saturating_sub(DEFAULT_RESERVE_TOKENS);
        let summary_max =
            (available / 4).clamp(MIN_SUMMARY_MAX_TOKENS as u64, SUMMARY_MAX_TOKENS as u64) as u32;
        Budget {
            context_window,
            threshold_percent,
            reserve: DEFAULT_RESERVE_TOKENS,
            keep_recent: KEEP_RECENT_TOKENS,
            summary_max,
            target_percent: None,
            tool_output_keep: TOOL_OUTPUT_KEEP,
            tool_output_min_chars: TOOL_OUTPUT_MIN_CHARS,
        }
    }

    /// 只改变摘要后的目标，不改变自动压缩的触发线。
    pub fn with_target_percent(mut self, target_percent: Option<u8>) -> Budget {
        self.target_percent = target_percent.filter(|value| (1..=99).contains(value));
        self
    }

    /// 触发阈值：占用超过「窗口 × 百分比」才值得动用压缩。
    ///
    /// 这是压缩**唯一**的触发口径 —— 投影式（清理 / 硬裁）、替换式（摘要）
    /// 与手动压缩的预检都读它，不要再各算一套。
    pub fn trigger_tokens(&self) -> u64 {
        self.context_window
            .saturating_mul(self.threshold_percent as u64)
            / 100
    }

    /// 保留尾部的预算：先给摘要自身与模型回复留位置，再收敛到 `keep_recent` 以内。
    pub fn keep_budget(&self) -> u64 {
        self.max_tail_budget().min(self.keep_recent)
    }

    fn max_tail_budget(&self) -> u64 {
        self.trigger_tokens()
            .saturating_sub(self.reserve)
            .saturating_sub(self.summary_max as u64 * 2)
    }

    /// 本次摘要保留原文的预算。显式目标包含摘要，因此先扣摘要输出上限；
    /// 实际结果还会受 user 轮次边界和 token 估算误差影响。
    pub fn keep_budget_for(&self, tokens_before: u64) -> u64 {
        match self.target_percent {
            None => self.keep_budget(),
            Some(percent) => {
                let target_tokens = tokens_before.saturating_mul(percent as u64) / 100;
                target_tokens
                    .saturating_sub(self.summary_max as u64)
                    .min(self.max_tail_budget())
            }
        }
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
    /// 摘要调用失败重发策略（与对话共用同一份，见 [`crate::retry`]）。
    pub retry: crate::retry::RetryPolicy,
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
        if let Some(first) = messages.get(start)
            && first.role() == "toolResult"
        {
            return Err("保留起点落在工具结果上：会孤立 toolResult".into());
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
            pipi_protocol::ContentBlock::ToolCall { id, .. } => Some(id.as_str()),
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
            let pipi_protocol::ContentBlock::ToolCall {
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
mod tests;
