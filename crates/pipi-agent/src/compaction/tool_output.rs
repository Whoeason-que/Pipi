//! 旧工具输出清理（投影式）：把体积大的旧工具结果换成占位符。
//!
//! 这是最便宜的一档压缩 —— 不花 LLM 调用、不动对话结构（toolResult 仍在
//! 原位，配对不变量不受影响），只是把最占体积的部分抽走，从而把摘要往后推。
//! 对齐 Claude Code 的「先清旧工具输出，再摘要」与 Anthropic context editing
//! 的 `clear_tool_uses_*`（那边是服务端做同一件事）。
//!
//! 因为它是**确定性投影**（给定原始历史必得同一结果），所以不落盘：回放时
//! 重算即可，live 与重开自然一致。幂等由占位符前缀保证 —— 二次应用是空操作。

use super::{Plan, Preparation, Projection};
use pipi_protocol::{Message, ToolResultContent};

/// 占位符前缀：既是给模型看的说明，也是「已清理」的幂等标记。
pub const CLEARED_TOOL_RESULT_PREFIX: &str = "[工具输出已清理";

pub struct ToolOutputPrune;

impl Projection for ToolOutputPrune {
    fn name(&self) -> &'static str {
        "tool-output-prune"
    }

    fn applies(&self, prep: &Preparation<'_>) -> bool {
        prep.over_budget() && !clearable(prep).is_empty()
    }

    fn plan(&self, prep: &Preparation<'_>) -> Plan {
        Plan {
            rewrites: clearable(prep),
            ..Default::default()
        }
    }
}

/// 收集可清理的工具结果：体积够大、且不在「最近 N 个」里。
///
/// 保留最近若干个原文，是因为模型手头正在用的细节（刚读的文件、刚跑的命令
/// 输出）被抽走会立刻打断工作；越早的输出越可能已经沉淀进后续推理。
fn clearable(prep: &Preparation<'_>) -> Vec<(usize, Message)> {
    let keep = prep.budget.tool_output_keep;
    let min_chars = prep.budget.tool_output_min_chars;

    // 从后往前数：最近 keep 个工具结果不动
    let mut protected = 0usize;
    let mut protected_index = std::collections::HashSet::new();
    for (index, message) in prep.messages.iter().enumerate().rev() {
        if message.role() != "toolResult" {
            continue;
        }
        if protected < keep {
            protected += 1;
            protected_index.insert(index);
        }
    }

    let mut rewrites = Vec::new();
    for (index, message) in prep.messages.iter().enumerate() {
        if message.role() != "toolResult" || protected_index.contains(&index) {
            continue;
        }
        let Message::ToolResult {
            tool_name,
            content,
            is_error,
            ..
        } = message
        else {
            continue;
        };
        if content.iter().any(is_cleared) {
            continue; // 幂等：已经清理过的不再动
        }
        let chars: usize = content
            .iter()
            .map(|block| match block {
                ToolResultContent::Text { text } => text.chars().count(),
                ToolResultContent::Image { .. } => 0,
            })
            .sum();
        let images = content
            .iter()
            .filter(|block| matches!(block, ToolResultContent::Image { .. }))
            .count();
        if chars < min_chars && images == 0 {
            continue; // 太短，清掉省不下什么
        }
        rewrites.push((
            index,
            cleared_message(message, tool_name, chars, images, *is_error),
        ));
    }
    rewrites
}

fn is_cleared(block: &ToolResultContent) -> bool {
    match block {
        ToolResultContent::Text { text } => text.starts_with(CLEARED_TOOL_RESULT_PREFIX),
        ToolResultContent::Image { .. } => false,
    }
}

/// 原地替换内容但保留 id / 名字 / 是否失败 —— 配对与「哪些调用失败过」
/// 这些结构信息一律不动，模型仍然看得到「这里有一次工具调用、结果被清理了」。
fn cleared_message(
    original: &Message,
    tool_name: &str,
    chars: usize,
    images: usize,
    is_error: bool,
) -> Message {
    let Message::ToolResult {
        tool_call_id,
        timestamp,
        details,
        ..
    } = original
    else {
        return original.clone();
    };
    let mut note = format!("{CLEARED_TOOL_RESULT_PREFIX}：{tool_name}，原文 {chars} 字符");
    if images > 0 {
        note.push_str(&format!("、{images} 张图片"));
    }
    note.push_str("。需要时重新执行该调用]");
    Message::ToolResult {
        tool_call_id: tool_call_id.clone(),
        tool_name: tool_name.to_string(),
        content: vec![ToolResultContent::Text { text: note }],
        is_error,
        details: details.clone(),
        timestamp: *timestamp,
    }
}
