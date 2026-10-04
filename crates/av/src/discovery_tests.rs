use super::*;

/// 临时目录名：进程 id + 纳秒时钟，避免并行测试互相踩踏。
fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let dir = std::env::temp_dir().join(format!("av-{label}-{}-{}", std::process::id(), nanos));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_contract(dir: &Path, text: &str) {
    std::fs::write(dir.join(AGENT_TOML_FILENAME), text).unwrap();
}

#[test]
fn no_contract_anywhere_yields_empty_layers() {
    let dir = temp_dir("empty");
    let discovered = discover(&dir).unwrap();
    assert!(discovered.layers.is_empty());
    // 无项目根时 root 回退为 cwd
    let cwd = std::fs::canonicalize(&dir).unwrap();
    assert_eq!(discovered.root, cwd);
}

#[test]
fn nearest_wins_from_cwd() {
    let root = temp_dir("nearest");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let deep = root.join("a/b/c");
    std::fs::create_dir_all(&deep).unwrap();
    write_contract(&root, "schema = 1\n[env]\nset = { FROM = \"root\" }");
    write_contract(&deep, "schema = 1\n[env]\nset = { FROM = \"deep\" }");

    let discovered = discover(&deep).unwrap();
    assert_eq!(discovered.layers.len(), 1);
    assert_eq!(
        discovered.layers[0]
            .config
            .env
            .as_ref()
            .unwrap()
            .set
            .as_ref()
            .unwrap()["FROM"],
        "deep"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stops_search_at_project_root() {
    // 外层 repo 有 agent.toml，内层 repo（.git）没有：不得越界继承外层契约
    let outer = temp_dir("outer");
    std::fs::create_dir_all(outer.join(".git")).unwrap();
    write_contract(&outer, "schema = 1\n[env]\nset = { FROM = \"outer\" }");
    let inner = outer.join("sub");
    std::fs::create_dir_all(inner.join(".git")).unwrap();

    let discovered = discover(&inner).unwrap();
    assert!(discovered.layers.is_empty());
    std::fs::remove_dir_all(outer).unwrap();
}

#[test]
fn local_layer_loaded_with_higher_priority() {
    let dir = temp_dir("local");
    write_contract(&dir, "schema = 1\n[env]\nset = { K = \"main\" }");
    std::fs::write(
        dir.join(AGENT_LOCAL_TOML_FILENAME),
        "schema = 1\n[env]\nset = { K = \"local\" }",
    )
    .unwrap();

    let discovered = discover(&dir).unwrap();
    assert_eq!(discovered.layers.len(), 2);
    assert_eq!(discovered.layers[0].label, "agent.toml");
    assert_eq!(discovered.layers[1].label, "agent.local.toml");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invalid_contract_is_fail_closed() {
    let dir = temp_dir("invalid");
    write_contract(&dir, "schema = 9");
    assert!(discover(&dir).is_err());

    let dir = temp_dir("syntax");
    write_contract(&dir, "schema = = 1");
    assert!(discover(&dir).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn symlinked_contract_outside_dir_rejected() {
    let dir = temp_dir("symlink");
    let external = temp_dir("external");
    write_contract(&external, "schema = 1");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        external.join(AGENT_TOML_FILENAME),
        dir.join(AGENT_TOML_FILENAME),
    )
    .unwrap();
    #[cfg(windows)]
    std::fs::copy(
        external.join(AGENT_TOML_FILENAME),
        dir.join(AGENT_TOML_FILENAME),
    )
    .unwrap();

    #[cfg(unix)]
    assert!(discover(&dir).is_err());
    #[cfg(windows)]
    assert!(discover(&dir).is_ok());

    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(external).unwrap();
}

#[test]
fn explicit_contract_file_load() {
    let dir = temp_dir("explicit");
    let path = dir.join("agent.toml");
    std::fs::write(&path, "schema = 1\n[env]\nset = { K = \"v\" }").unwrap();

    let config = load_contract_file(&path).unwrap();
    assert!(config.env.is_some());
    // 未知段 fail-closed
    std::fs::write(&path, "schema = 1\n[unknown]\nx = 1").unwrap();
    assert!(load_contract_file(&path).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn tilde_and_relative_resolution() {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    assert_eq!(expand_tilde("~"), home);
    assert_eq!(expand_tilde("~/x"), home.join("x"));
    assert_eq!(expand_tilde("/abs/x"), PathBuf::from("/abs/x"));
    assert_eq!(expand_tilde("~user/x"), PathBuf::from("~user/x"));
    assert_eq!(
        resolve_path(Path::new("/base"), "rel"),
        PathBuf::from("/base/rel")
    );
    assert_eq!(resolve_path(Path::new("/base"), "~/x"), home.join("x"));
}
