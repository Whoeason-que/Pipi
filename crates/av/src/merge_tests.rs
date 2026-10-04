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
    let high = layer(
        "agent.local.toml",
        "schema = 1\n[[requires]]\ncommand = \"rg\"",
    );

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
        "schema = 1\n[resources]\nmax-bytes = 100\n[resources.skills]\nsources = [\"agent-skills\"]\nonly = [\"a\"]\nuse = [\"low\"]",
    );
    let high = layer(
        "agent.local.toml",
        "schema = 1\n[resources.skills]\nexclude = [\"b\"]\nuse = [\"high\"]",
    );

    let merged = merge_layers(&[low, high]).unwrap();
    let resources = merged.resources.as_ref().unwrap();
    assert_eq!(resources.max_bytes, Some(100));
    let skills = resources.skills.as_ref().unwrap();
    assert_eq!(
        skills.sources.as_deref(),
        Some(&["agent-skills".to_string()][..])
    );
    assert_eq!(skills.only.as_deref(), Some(&["a".to_string()][..]));
    assert_eq!(skills.exclude.as_deref(), Some(&["b".to_string()][..]));
    // use 与其它数组一致：高层整体替换，缺席保留
    assert_eq!(
        skills.use_.as_deref(),
        Some(&["high".to_string()][..]),
        "高层 use 应整体替换低层"
    );
}

#[test]
fn use_absence_preserves_low_layer() {
    let low = layer(
        "agent.toml",
        "schema = 1\n[resources.skills]\nuse = [\"pdf\"]",
    );
    let high = layer("agent.local.toml", "schema = 1\n[resources]\nmax-bytes = 1");
    let merged = merge_layers(&[low, high]).unwrap();
    let skills = merged.resources.as_ref().unwrap().skills.as_ref().unwrap();
    assert_eq!(skills.use_.as_deref(), Some(&["pdf".to_string()][..]));
}

#[test]
fn empty_layers_yield_defaults() {
    let merged = merge_layers(&[]).unwrap();
    assert_eq!(merged.env.inherit, Inherit::All);
    assert!(merged.env.set.is_empty() && merged.requires.is_empty());
    assert!(merged.resources.is_none());
}
