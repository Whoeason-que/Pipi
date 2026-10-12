//! 压缩阈值契约。
//!
//! 只有「默认阈值 + 归一化规则」两个符号：清单（`agent.json` 的
//! `compactThresholdPercent`）与压缩预算（`pipi-context::Budget`）必须对同一个
//! 值达成一致，所以规则住在两边都依赖的契约层。压缩流水线本身在 `pipi-context`。

/// 默认自动压缩阈值（上下文占用达到窗口的这个百分比时压缩）。
pub const DEFAULT_THRESHOLD_PERCENT: u8 = 75;

/// 阈值百分比归一化：0 / >100 视为「没设过」，用默认值。
pub fn normalize_threshold_percent(percent: u8) -> u8 {
    if (1..=100).contains(&percent) {
        percent
    } else {
        DEFAULT_THRESHOLD_PERCENT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_falls_back_to_the_default() {
        assert_eq!(normalize_threshold_percent(0), DEFAULT_THRESHOLD_PERCENT);
        assert_eq!(normalize_threshold_percent(101), DEFAULT_THRESHOLD_PERCENT);
        assert_eq!(normalize_threshold_percent(1), 1);
        assert_eq!(normalize_threshold_percent(100), 100);
        assert_eq!(normalize_threshold_percent(75), 75);
    }
}
