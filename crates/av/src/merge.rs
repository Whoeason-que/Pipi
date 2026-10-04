//! 分层合并：标量高层覆盖、映射按键合并、数组整体替换。
//!
//! 合并语义（可预测优先）：
//! - 标量（inherit 等）：高层显式声明覆盖低层，缺席保留低层；
//! - 映射（set/secrets/skills 过滤器）：按键合并，同键高层覆盖低层；
//! - 数组（ignore / path-prepend / requires）：高层整体替换低层，不拼接。

use std::collections::BTreeMap;

use crate::discovery::Layer;
use crate::schema::{Inherit, RequiresEntry, Resources, SecretRef, SkillsFilter};

/// 合并后的有效配置。
#[derive(Debug, Clone)]
pub struct Merged {
    pub env: MergedEnv,
    pub requires: Vec<RequiresEntry>,
    pub resources: Option<Resources>,
}

#[derive(Debug, Clone)]
pub struct MergedEnv {
    /// 缺省 [`Inherit::All`]。
    pub inherit: Inherit,
    pub ignore: Vec<String>,
    pub set: BTreeMap<String, String>,
    pub path_prepend: Vec<String>,
    pub secrets: BTreeMap<String, SecretRef>,
}

/// `layers` 自低到高优先级。
pub fn merge_layers(layers: &[Layer]) -> Result<Merged, String> {
    for layer in layers {
        layer
            .config
            .validate()
            .map_err(|e| format!("{} 校验失败：{e}", layer.label))?;
    }

    let mut env = MergedEnv {
        inherit: Inherit::All,
        ignore: Vec::new(),
        set: BTreeMap::new(),
        path_prepend: Vec::new(),
        secrets: BTreeMap::new(),
    };
    let mut requires: Vec<RequiresEntry> = Vec::new();
    let mut resources: Option<Resources> = None;

    for layer in layers {
        if let Some(config_env) = &layer.config.env {
            if let Some(inherit) = &config_env.inherit {
                env.inherit = inherit.clone();
            }
            if let Some(ignore) = &config_env.ignore {
                env.ignore = ignore.clone();
            }
            if let Some(set) = &config_env.set {
                env.set.extend(set.clone());
            }
            if let Some(prepend) = &config_env.path_prepend {
                env.path_prepend = prepend.clone();
            }
            if let Some(secrets) = &config_env.secrets {
                env.secrets.extend(secrets.clone());
            }
        }
        if let Some(layer_requires) = &layer.config.requires {
            requires = layer_requires.clone();
        }
        if let Some(res) = &layer.config.resources {
            let slot = resources.get_or_insert_with(Resources::default);
            if res.instructions.is_some() {
                slot.instructions = res.instructions.clone();
            }
            if let Some(max_bytes) = res.max_bytes {
                slot.max_bytes = Some(max_bytes);
            }
            if let Some(memory) = &res.memory {
                slot.memory = Some(memory.clone());
            }
            if let Some(skills) = &res.skills {
                let target = slot.skills.get_or_insert_with(SkillsFilter::default);
                if let Some(sources) = &skills.sources {
                    target.sources = Some(sources.clone());
                }
                if let Some(use_names) = &skills.use_ {
                    target.use_ = Some(use_names.clone());
                }
                if let Some(only) = &skills.only {
                    target.only = Some(only.clone());
                }
                if let Some(exclude) = &skills.exclude {
                    target.exclude = Some(exclude.clone());
                }
            }
        }
    }

    Ok(Merged {
        env,
        requires,
        resources,
    })
}

#[cfg(test)]
mod tests;
