use super::*;

fn parse(text: &str) -> Result<AgentToml, toml::de::Error> {
    toml::from_str(text)
}

#[test]
fn minimal_valid() {
    let config = parse("schema = 1").unwrap();
    assert_eq!(config.schema, 1);
    assert!(config.env.is_none() && config.requires.is_none() && config.resources.is_none());
    config.validate().unwrap();
}

#[test]
fn full_valid() {
    let config = parse(
        r#"
schema = 1

[env]
inherit = "core"
ignore = ["AWS_*", "OPENAI_API_KEY"]
set = { RUST_BACKTRACE = "1" }
path-prepend = ["/opt/homebrew/bin", "~/.local/bin"]

[env.secrets]
GITHUB_TOKEN = { file = "~/.pipi/secrets/gh.token" }
CI_TOKEN = { env = "CI_TOKEN" }

[[requires]]
command = "git"

[[requires]]
command = "node"
version = ">=20"

[resources]
instructions = ["AGENTS.md"]
max-bytes = 16384
memory = "memory"

[resources.skills]
sources = [".pi/skills"]
only = ["git-safety", "review-*"]
exclude = ["experimental-*"]
"#,
    )
    .unwrap();
    config.validate().unwrap();
    let env = config.env.unwrap();
    assert_eq!(env.inherit, Some(Inherit::Core));
    assert_eq!(env.ignore.as_ref().unwrap().len(), 2);
    assert_eq!(env.path_prepend.as_ref().unwrap().len(), 2);
    assert_eq!(env.secrets.as_ref().unwrap().len(), 2);
    assert_eq!(config.requires.as_ref().unwrap().len(), 2);
    assert_eq!(
        config.requires.as_ref().unwrap()[1].version.as_deref(),
        Some(">=20")
    );
    let resources = config.resources.unwrap();
    assert_eq!(resources.max_bytes, Some(16384));
    assert_eq!(
        resources.skills.unwrap().only.as_deref(),
        Some(&["git-safety".to_string(), "review-*".to_string()][..])
    );
}

#[test]
fn unknown_top_level_section_rejected() {
    // 项目层白名单：model/agent/permissions/tools/mcp 段都是未知键
    for text in [
        "schema = 1\n[agent]\nname = \"x\"",
        "schema = 1\n[model]\nprovider = \"anthropic\"",
        "schema = 1\n[permissions]\nsandbox = \"read-only\"",
        "schema = 1\n[tools]\nenabled = [\"bash\"]",
        "schema = 1\n[mcp.foo]\ncommand = \"npx\"",
    ] {
        assert!(parse(text).is_err(), "应拒绝：{text}");
    }
}

#[test]
fn unknown_field_in_section_rejected() {
    let err = parse("schema = 1\n[env]\ninheritx = \"all\"").unwrap_err();
    assert!(err.to_string().contains("unknown field"), "{err}");
}

#[test]
fn wrong_schema_rejected() {
    let config = parse("schema = 2").unwrap();
    assert!(config.validate().is_err());
}

#[test]
fn reserved_namespace_rejected() {
    let config = parse("schema = 1\n[env]\nset = { AV_AGENT = \"x\" }").unwrap();
    assert!(config.validate().is_err());
    let config = parse("schema = 1\n[env]\nsecrets = { AV_X = { env = \"Y\" } }").unwrap();
    assert!(config.validate().is_err());
    let config = parse("schema = 1\n[env]\nignore = [\"AV_*\"]").unwrap();
    assert!(config.validate().is_err());
    let config = parse("schema = 1\n[env]\nset = { AV = \"x\" }").unwrap();
    assert!(config.validate().is_err());
}

#[test]
fn invalid_env_key_shape_rejected() {
    let config = parse("schema = 1\n[env]\nset = { \"A=B\" = \"x\" }").unwrap();
    assert!(config.validate().is_err());
    let config = parse("schema = 1\n[env]\nset = { \"\" = \"x\" }").unwrap();
    assert!(config.validate().is_err());
}

#[test]
fn secret_ref_requires_exactly_one_form() {
    assert!(parse("schema = 1\n[env.secrets]\nK = { env = \"A\", file = \"b\" }").is_err());
    assert!(parse("schema = 1\n[env.secrets]\nK = {}").is_err());
    parse("schema = 1\n[env.secrets]\nK = { env = \"A\" }").unwrap();
    parse("schema = 1\n[env.secrets]\nK = { file = \"b\" }").unwrap();
}

#[test]
fn kebab_case_field_names() {
    assert!(parse("schema = 1\n[env]\npathPrepend = [\"/x\"]").is_err());
    assert!(parse("schema = 1\n[env]\npath-prepEND = [\"/x\"]").is_err());
    parse("schema = 1\n[env]\npath-prepend = [\"/x\"]").unwrap();
    assert!(parse("schema = 1\n[resources]\nmaxBytes = 1").is_err());
}

#[test]
fn skills_use_parses_and_validates() {
    let config = parse("schema = 1\n[resources.skills]\nuse = [\"pdf\", \"git-safety\"]").unwrap();
    config.validate().unwrap();
    assert_eq!(
        config.resources.unwrap().skills.unwrap().use_.as_deref(),
        Some(&["pdf".to_string(), "git-safety".to_string()][..])
    );

    // 空列表 = 显式禁用，合法
    let config = parse("schema = 1\n[resources.skills]\nuse = []").unwrap();
    config.validate().unwrap();
    assert_eq!(
        config.resources.unwrap().skills.unwrap().use_,
        Some(Vec::new())
    );

    // 形状非法 / 重复：fail-closed
    for text in [
        "schema = 1\n[resources.skills]\nuse = [\"PDF\"]",
        "schema = 1\n[resources.skills]\nuse = [\"a_b\"]",
        "schema = 1\n[resources.skills]\nuse = [\"pdf\", \"pdf\"]",
    ] {
        let config = parse(text).unwrap();
        assert!(config.validate().is_err(), "应拒绝：{text}");
    }
}
