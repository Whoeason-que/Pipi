//! models.dev 价格表：只用于统计的成本估算，不参与请求协议解析。

use std::collections::HashMap;

use pipi_provider::catalog::{ModelCost, cached_catalog};

/// models.dev 价格表：按模型 id 找，找不到再按显示名找。
#[derive(Default)]
pub(super) struct Pricing {
    pub(super) by_id: HashMap<String, ModelCost>,
    pub(super) by_name: HashMap<String, ModelCost>,
}

impl Pricing {
    pub(super) fn load() -> Self {
        let Some(catalog) = cached_catalog(pipi_venv::av::paths::catalog_cache_path().as_deref())
        else {
            return Self::default();
        };
        let mut pricing = Self::default();
        for provider in &catalog.providers {
            for model in &provider.models {
                let Some(cost) = &model.cost else {
                    continue;
                };
                pricing
                    .by_id
                    .entry(model.id.to_ascii_lowercase())
                    .or_insert_with(|| cost.clone());
                pricing
                    .by_name
                    .entry(model.name.to_ascii_lowercase())
                    .or_insert_with(|| cost.clone());
            }
        }
        pricing
    }

    pub(super) fn lookup(&self, model: &str) -> Option<&ModelCost> {
        if model.is_empty() {
            return None;
        }
        let key = model.to_ascii_lowercase();
        self.by_id.get(&key).or_else(|| self.by_name.get(&key))
    }
}
