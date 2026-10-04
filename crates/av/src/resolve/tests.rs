use super::*;
use crate::test_support::layer;

fn process_env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn no_layers_equals_process_env_plus_runtime() {
    let process = process_env(&[("HOME", "/home/u"), ("SECRET_SHOULD_PASS", "x")]);
    let runtime = process_env(&[("AV", "1"), ("AV_AGENT", "demo")]);

    let resolved = resolve_env(&[], &process, &runtime).unwrap();
    assert_eq!(resolved.vars["HOME"], "/home/u");
    assert_eq!(resolved.vars["AV_AGENT"], "demo");
    assert!(resolved.provenance["HOME"] == PROCESS_SOURCE);
    assert_eq!(resolved.provenance["AV_AGENT"], RUNTIME_SOURCE);
    assert!(resolved.secret_keys.is_empty());
}

#[test]
fn inherit_none_discards_process_env() {
    let process = process_env(&[("HOME", "/home/u"), ("KEEP", "x")]);
    let runtime = process_env(&[]);
    let layers = [layer(
        "agent.toml",
        "schema = 1\n[env]\ninherit = \"none\"\nset = { K = \"v\" }",
    )];

    let resolved = resolve_env(&layers, &process, &runtime).unwrap();
    assert!(!resolved.vars.contains_key("HOME"));
    assert!(!resolved.vars.contains_key("KEEP"));
    assert_eq!(resolved.vars["K"], "v");
}

#[test]
fn inherit_core_keeps_only_core_keys() {
    let process = process_env(&[
        ("PATH", "/usr/bin"),
        ("HOME", "/home/u"),
        ("AWS_SECRET", "nope"),
        ("TERM", "xterm"),
    ]);
    let runtime = process_env(&[]);
    let layers = [layer("agent.toml", "schema = 1\n[env]\ninherit = \"core\"")];

    let resolved = resolve_env(&layers, &process, &runtime).unwrap();
    assert!(resolved.vars.contains_key("PATH"));
    assert!(resolved.vars.contains_key("HOME"));
    assert!(resolved.vars.contains_key("TERM"));
    assert!(!resolved.vars.contains_key("AWS_SECRET"));
}

#[test]
fn ignore_removes_inherited_but_not_set() {
    let process = process_env(&[("AWS_KEY", "inherited"), ("AWS_KEY2", "inherited2")]);
    let runtime = process_env(&[]);
    let layers = [layer(
        "agent.toml",
        "schema = 1\n[env]\nignore = [\"AWS_*\"]\nset = { AWS_KEY = \"declared\" }",
    )];

    let resolved = resolve_env(&layers, &process, &runtime).unwrap();
    assert!(!resolved.vars.contains_key("AWS_KEY2"));
    assert_eq!(resolved.vars["AWS_KEY"], "declared");
    assert!(resolved.provenance["AWS_KEY"].starts_with("agent"));
}

#[test]
fn path_prepend_prepends_to_existing() {
    let process = process_env(&[("PATH", "/usr/bin")]);
    let runtime = process_env(&[]);
    let layers = [layer(
        "agent.toml",
        "schema = 1\n[env]\npath-prepend = [\"/opt/tools\"]",
    )];

    let resolved = resolve_env(&layers, &process, &runtime).unwrap();
    assert_eq!(resolved.vars["PATH"], "/opt/tools:/usr/bin");
}

#[test]
fn path_prepend_creates_path_when_none_inherited() {
    let process = process_env(&[]);
    let runtime = process_env(&[]);
    let layers = [layer(
        "agent.toml",
        "schema = 1\n[env]\ninherit = \"none\"\npath-prepend = [\"/opt/tools\"]",
    )];

    let resolved = resolve_env(&layers, &process, &runtime).unwrap();
    assert_eq!(resolved.vars["PATH"], "/opt/tools");
}

#[test]
fn secret_env_ref_reads_original_process_env() {
    // 引用键被 ignore 掉也不影响：秘密引用绕过 inherit/ignore
    let process = process_env(&[("CI_TOKEN", "tok-123")]);
    let runtime = process_env(&[]);
    let layers = [layer(
        "agent.toml",
        "schema = 1\n[env]\nignore = [\"CI_TOKEN\"]\nsecrets = { GH = { env = \"CI_TOKEN\" } }",
    )];

    let resolved = resolve_env(&layers, &process, &runtime).unwrap();
    assert_eq!(resolved.vars["GH"], "tok-123");
    assert!(resolved.secret_keys.contains("GH"));
    assert_eq!(resolved.redacted()["GH"], "[redacted]");
}

#[test]
fn secret_env_ref_missing_fails_closed() {
    let process = process_env(&[]);
    let runtime = process_env(&[]);
    let layers = [layer(
        "agent.toml",
        "schema = 1\n[env]\nsecrets = { GH = { env = \"MISSING\" } }",
    )];

    assert!(resolve_env(&layers, &process, &runtime).is_err());
}

#[test]
fn secret_file_ref_reads_trimmed_content() {
    let dir = pipi_test_support::temp_dir("av-resolve");
    let file = dir.join("token.txt");
    std::fs::write(&file, "tok-456\n\n").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let process = process_env(&[]);
    let runtime = process_env(&[]);
    let layers = [layer(
        "agent.toml",
        &format!(
            "schema = 1\n[env]\nsecrets = {{ GH = {{ file = {:?} }} }}",
            file.to_string_lossy()
        ),
    )];

    let resolved = resolve_env(&layers, &process, &runtime).unwrap();
    assert_eq!(resolved.vars["GH"], "tok-456");
    assert!(resolved.secret_keys.contains("GH"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn secret_file_with_wide_permissions_fails_closed() {
    use std::os::unix::fs::PermissionsExt;

    let dir = pipi_test_support::temp_dir("av-resolve");
    let file = dir.join("token");
    std::fs::write(&file, "tok\n").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();

    let process = process_env(&[]);
    let runtime = process_env(&[]);
    let layers = [layer(
        "agent.toml",
        &format!(
            "schema = 1\n[env]\nsecrets = {{ GH = {{ file = {:?} }} }}",
            file.to_string_lossy()
        ),
    )];

    let err = resolve_env(&layers, &process, &runtime).unwrap_err();
    assert!(err.contains("权限过宽"), "{err}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn runtime_vars_override_everything() {
    // 声明文件不得写 AV_*（schema 拒绝）；这里验证进程环境里的同名变量
    // 也被运行时注入最终覆盖
    let process = process_env(&[("AV_AGENT", "from-shell"), ("PATH", "/usr/bin")]);
    let runtime = process_env(&[("AV_AGENT", "host"), ("AV", "1")]);

    let resolved = resolve_env(&[], &process, &runtime).unwrap();
    assert_eq!(resolved.vars["AV_AGENT"], "host");
    assert_eq!(resolved.provenance["AV_AGENT"], RUNTIME_SOURCE);
}

#[test]
fn provenance_tracks_layers() {
    let process = process_env(&[]);
    let runtime = process_env(&[]);
    let layers = [
        layer("agent.toml", "schema = 1\n[env]\nset = { A = \"1\" }"),
        layer("agent.local.toml", "schema = 1\n[env]\nset = { A = \"2\" }"),
    ];

    let resolved = resolve_env(&layers, &process, &runtime).unwrap();
    assert_eq!(resolved.vars["A"], "2");
    assert_eq!(resolved.provenance["A"], "agent.local.toml");
}

#[test]
fn discovery_integration() {
    // 走真实 discover 路径：临时 repo 内 agent.toml + agent.local.toml
    let dir = pipi_test_support::temp_dir("av-resolve");
    std::fs::write(
        dir.join("agent.toml"),
        "schema = 1\n[env]\ninherit = \"none\"\nset = { FROM = \"main\" }",
    )
    .unwrap();
    std::fs::write(
        dir.join("agent.local.toml"),
        "schema = 1\n[env]\nset = { FROM = \"local\" }",
    )
    .unwrap();

    let discovered = crate::discover(&dir).unwrap();
    let resolved = resolve_env(&discovered.layers, &process_env(&[]), &process_env(&[])).unwrap();
    assert_eq!(resolved.vars["FROM"], "local");
    assert_eq!(resolved.provenance["FROM"], "agent.local.toml");
    std::fs::remove_dir_all(dir).unwrap();
}
