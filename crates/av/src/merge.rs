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
mod tests {
    use super::*;
    use crate::schema::AgentToml;

    fn layer(label: &str, text: &str) -> Layer {
        let config: AgentToml = toml::from_str(text).unwrap();
        config.validate().unwrap();
        Layer {
            label: label.to_string(),
            path: std::path::PathBuf::from(label),
            config,
        }
    }

    #[test]
    fn scalar_overrides_and_absence_preserves() {
        let low = layer(
            "agent.toml",
            "schema = 1\n[env]\ninherit = \"none\"\nignore = [\"A_*\"]",
        );
        // local 层未声明 inherit/ignore：不得把 none 重置回 all
        let high = layer("agent.local.toml", "schema = 1\n[env]\nset = { K = \"v\" }");

        let merged = merge_layers(&[low, high]).unwrap();
        assert_eq!(merged.env.inherit, Inherit::None);
        assert_eq!(merged.env.ignore, vec!["A_*".to_string()]);
        assert_eq!(merged.env.set["K"], "v");
    }

    #[test]
    fn maps_merge_per_key_arrays_replace() {
        let low = layer(
            "agent.toml",
            "schema = 1\n[env]\nset = { A = \"low\", B = \"low\" }\npath-prepend = [\"/low\"]",
        );
        let high = layer(
            "agent.local.toml",
            "schema = 1\n[env]\nset = { B = \"high\" }\npath-prepend = [\"/high\"]",
        );

        let merged = merge_layers(&[low, high]).unwrap();
        assert_eq!(merged.env.set["A"], "low");
        assert_eq!(merged.env.set["B"], "high");
        // 数组整体替换：低层的 /low 不保留
        assert_eq!(merged.env.path_prepend, vec!["/high".to_string()]);
    }

    #[test]
    fn requires_replace_wholesale() {
        let low = layer(
            "agent.toml",
            "schema = 1\n[[requires]]\ncommand = \"git\"\n[[requires]]\ncommand = \"node\"",
        );
        let high = layer("agent.local.toml", "schema = 1\n[[requires]]\ncommand = \"rg\"");

        let merged = merge_layers(&[low, high]).unwrap();
        assert_eq!(merged.requires.len(), 1);
        assert_eq!(merged.requires[0].command, "rg");
    }

    #[test]
    fn absence_of_requires_preserves_low_layer() {
        let low = layer(
            "agent.toml",
            "schema = 1\n[[requires]]\ncommand = \"git\"\nversion = \">=2\"",
        );
        let high = layer("agent.local.toml", "schema = 1");

        let merged = merge_layers(&[low, high]).unwrap();
        assert_eq!(merged.requires.len(), 1);
        assert_eq!(merged.requires[0].version.as_deref(), Some(">=2"));
    }

    #[test]
    fn resources_merge_field_wise() {
        let low = layer(
            "agent.toml",
            "schema = 1\n[resources]\nmax-bytes = 100\n[resources.skills]\nsources = [\"agent-skills\"]\nonly = [\"a\"]",
        );
        let high = layer(
            "agent.local.toml",
            "schema = 1\n[resources.skills]\nexclude = [\"b\"]",
        );

        let merged = merge_layers(&[low, high]).unwrap();
        let resources = merged.resources.as_ref().unwrap();
        assert_eq!(resources.max_bytes, Some(100));
        let skills = resources.skills.as_ref().unwrap();
        assert_eq!(skills.sources.as_deref(), Some(&["agent-skills".to_string()][..]));
        assert_eq!(skills.only.as_deref(), Some(&["a".to_string()][..]));
        assert_eq!(skills.exclude.as_deref(), Some(&["b".to_string()][..]));
    }

    #[test]
    fn empty_layers_yield_defaults() {
        let merged = merge_layers(&[]).unwrap();
        assert_eq!(merged.env.inherit, Inherit::All);
        assert!(merged.env.set.is_empty() && merged.requires.is_empty());
        assert!(merged.resources.is_none());
    }
}
