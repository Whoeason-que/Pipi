//! 统计的 token / 成本累加口径：所有派生结构共用同一套实现，避免前端或各分支各算一遍。

use super::{DayStats, ModelStats, SessionStatsRow, StatsTotals};

pub(super) fn token_total(input: u64, output: u64, cache_read: u64, cache_write: u64) -> u64 {
    input + output + cache_read + cache_write
}

pub(super) trait AddUsage {
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
    pub(super) fn add_call(&mut self, usage: &pipi_protocol::Usage, cost: Option<f64>) {
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
