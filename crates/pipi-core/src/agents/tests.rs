use super::*;

#[test]
fn agent_directory_rejects_path_traversal_and_separators() {
    assert!(agent_dir("safe-agent_1").is_some());
    for invalid in ["", ".", "..", "../escape", "nested/name", "nested\\name"] {
        assert!(
            agent_dir(invalid).is_none(),
            "accepted invalid agent name: {invalid}"
        );
    }
}

#[test]
fn manifest_name_must_match_containing_agent_directory() {
    assert!(manifest_name_matches_directory("safe-agent", "safe-agent"));
    assert!(!manifest_name_matches_directory(
        "safe-agent",
        "other-agent"
    ));
    assert!(!manifest_name_matches_directory("../escape", "../escape"));
}

#[cfg(unix)]
#[test]
fn agent_directory_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;

    let _guard = HOME_LOCK.lock().unwrap();
    let home = std::env::temp_dir().join(format!(
        "pipi-agent-link-home-{}",
        pipi_agent::session::new_id()
    ));
    let external = std::env::temp_dir().join(format!(
        "pipi-agent-link-external-{}",
        pipi_agent::session::new_id()
    ));
    let linked = home.join(".pipi/agents/linked-agent");
    std::fs::create_dir_all(home.join(".pipi/agents")).unwrap();
    std::fs::create_dir_all(&external).unwrap();
    symlink(&external, &linked).unwrap();

    let previous_home = std::env::var("HOME").unwrap();
    pipi_test_support::set_env_var("HOME", &home);
    let def = AgentDefinition {
        name: "linked-agent".into(),
        description: String::new(),
        model: String::new(),
        provider: None,
        workspace: None,
        permissions: PermissionsConfig::default(),
        mcp_servers: Vec::new(),
        subagent: false,
        compact_threshold_percent: 75,
        compact_target_percent: None,
    };

    assert!(agent_dir("linked-agent").is_none());
    assert!(load_agent("linked-agent").is_err());
    assert!(save_agent(&def).is_err());
    assert!(ensure_agent_dir("linked-agent").is_err());
    assert!(list_agents().unwrap().is_empty());

    pipi_test_support::set_env_var("HOME", previous_home);
    std::fs::remove_dir_all(&home).unwrap();
    std::fs::remove_dir_all(&external).unwrap();
}

#[cfg(unix)]
#[test]
fn agent_root_symlink_is_rejected() {
    use std::os::unix::fs::symlink;

    let _guard = HOME_LOCK.lock().unwrap();
    let home = std::env::temp_dir().join(format!(
        "pipi-agent-root-link-home-{}",
        pipi_agent::session::new_id()
    ));
    let external = std::env::temp_dir().join(format!(
        "pipi-agent-root-link-external-{}",
        pipi_agent::session::new_id()
    ));
    std::fs::create_dir_all(home.join(".pipi")).unwrap();
    std::fs::create_dir_all(&external).unwrap();
    symlink(&external, home.join(".pipi/agents")).unwrap();

    let previous_home = std::env::var("HOME").unwrap();
    pipi_test_support::set_env_var("HOME", &home);
    assert!(list_agents().is_err());
    assert!(ensure_agent_dir("root-link-agent").is_err());
    pipi_test_support::set_env_var("HOME", previous_home);

    std::fs::remove_dir_all(&home).unwrap();
    std::fs::remove_dir_all(&external).unwrap();
}

#[cfg(unix)]
#[test]
fn agent_manifest_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;

    let _guard = HOME_LOCK.lock().unwrap();
    let home = std::env::temp_dir().join(format!(
        "pipi-agent-manifest-link-home-{}",
        pipi_agent::session::new_id()
    ));
    let external = std::env::temp_dir().join(format!(
        "pipi-agent-manifest-link-external-{}",
        pipi_agent::session::new_id()
    ));
    let agent = home.join(".pipi/agents/manifest-link-agent");
    let external_manifest = external.join("agent.json");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::create_dir_all(&external).unwrap();
    let def = AgentDefinition {
        name: "manifest-link-agent".into(),
        description: "external".into(),
        model: String::new(),
        provider: None,
        workspace: None,
        permissions: PermissionsConfig::default(),
        mcp_servers: Vec::new(),
        subagent: false,
        compact_threshold_percent: 75,
        compact_target_percent: None,
    };
    std::fs::write(
        &external_manifest,
        serde_json::to_string_pretty(&def).unwrap() + "\n",
    )
    .unwrap();
    symlink(&external_manifest, agent.join("agent.json")).unwrap();

    let previous_home = std::env::var("HOME").unwrap();
    pipi_test_support::set_env_var("HOME", &home);
    assert!(load_agent("manifest-link-agent").is_err());
    assert!(save_agent(&def).is_err());
    assert!(list_agents().unwrap().is_empty());
    pipi_test_support::set_env_var("HOME", previous_home);

    let external_contents = std::fs::read_to_string(&external_manifest).unwrap();
    assert!(external_contents.contains("external"));
    std::fs::remove_dir_all(&home).unwrap();
    std::fs::remove_dir_all(&external).unwrap();
}

#[test]
fn create_defaults_to_workspace_write_sandbox() {
    let _guard = HOME_LOCK.lock().unwrap();
    let name = format!("pipi-default-sandbox-{}", pipi_agent::session::new_id());
    let def = create_agent(&name, "", None, None, None, None).unwrap();
    assert_eq!(
        def.permissions.sandbox,
        pipi_tools::permissions::SandboxMode::WorkspaceWrite
    );
    assert!(!def.subagent);
    let _ = fs::remove_dir_all(agent_dir(&name).unwrap());
}

#[test]
fn subagent_defaults_and_persists_changes() {
    let _guard = HOME_LOCK.lock().unwrap();
    let legacy: AgentDefinition =
        serde_json::from_str(r#"{"name":"legacy","permissions":{"tools":["read"]}}"#).unwrap();
    assert!(!legacy.subagent);

    let explicit: AgentDefinition = serde_json::from_str(
        r#"{"name":"explicit","subagent":true,"permissions":{"tools":["read"]}}"#,
    )
    .unwrap();
    assert!(explicit.subagent);
    let json = serde_json::to_string(&explicit).unwrap();
    assert!(json.contains("\"subagent\":true"), "{json}");

    let name = format!("pipi-subagent-roundtrip-{}", pipi_agent::session::new_id());
    let mut def = create_agent(&name, "", None, None, None, None).unwrap();
    assert!(!def.subagent);

    def.subagent = true;
    save_agent(&def).unwrap();
    assert!(load_agent(&name).unwrap().subagent);

    def.subagent = false;
    save_agent(&def).unwrap();
    assert!(!load_agent(&name).unwrap().subagent);
    let _ = fs::remove_dir_all(agent_dir(&name).unwrap());
}

#[test]
fn system_prompt_includes_wire_tool_descriptions() {
    let def = AgentDefinition {
        name: "__pipi_harness_wire_test__".into(),
        description: String::new(),
        model: String::new(),
        provider: None,
        workspace: Some(
            std::env::temp_dir()
                .join("pipi-harness-wire-test-does-not-write")
                .to_string_lossy()
                .into_owned(),
        ),
        permissions: PermissionsConfig {
            tools: vec!["read".into()],
            ..Default::default()
        },
        mcp_servers: Vec::new(),
        subagent: false,
        compact_threshold_percent: 75,
        compact_target_percent: None,
    };
    let tools = [pipi_protocol::Tool {
        name: "read".into(),
        description: "Read files".into(),
        parameters: serde_json::json!({}),
    }];

    let prompt = build_system_prompt_with_tools(&def, &tools).unwrap();

    assert!(prompt.contains("- read: Read files"));
}

/// av 契约端到端：项目层 agent.toml 的声明替换约定发现（instructions）、
/// skills 按名过滤、env 解析注入 AV_*，以及包含约束 fail-closed。
#[test]
fn agent_contract_drives_instructions_skills_and_env() {
    let root = std::env::temp_dir().join(format!(
        "pipi-contract-e2e-{}",
        pipi_agent::session::new_id()
    ));
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join("AGENTS.md"), "CONVENTION-AGENTS-ROOT-RULES").unwrap();
    fs::write(root.join("PROMPT.md"), "DECLARED-PROMPT-BODY").unwrap();

    let skills_dir = root.join(".pi").join("skills");
    fs::create_dir_all(skills_dir.join("wanted")).unwrap();
    fs::write(
        skills_dir.join("wanted").join("SKILL.md"),
        "---\nname: wanted\ndescription: wanted skill\n---\nbody",
    )
    .unwrap();
    fs::create_dir_all(skills_dir.join("noisy")).unwrap();
    fs::write(
        skills_dir.join("noisy").join("SKILL.md"),
        "---\nname: noisy\ndescription: noisy skill\n---\nbody",
    )
    .unwrap();

    fs::write(
        root.join("agent.toml"),
        r#"
schema = 1

[env]
set = { CONTRACT_E2E_MARKER = "on" }

[resources]
instructions = ["PROMPT.md"]

[resources.skills]
only = ["wanted"]
"#,
    )
    .unwrap();

    let def = AgentDefinition {
        name: "__pipi_contract_e2e__".into(),
        description: String::new(),
        model: String::new(),
        provider: None,
        workspace: Some(root.to_string_lossy().into_owned()),
        permissions: PermissionsConfig {
            tools: vec!["read".into(), "bash".into()],
            ..Default::default()
        },
        mcp_servers: Vec::new(),
        subagent: false,
        compact_threshold_percent: 75,
        compact_target_percent: None,
    };

    // 声明替换约定发现；skills 按名过滤
    let prompt = build_system_prompt_with_tools(&def, &[]).unwrap();
    assert!(prompt.contains("DECLARED-PROMPT-BODY"));
    assert!(!prompt.contains("CONVENTION-AGENTS-ROOT-RULES"));
    assert!(prompt.contains("<name>wanted</name>"));
    assert!(!prompt.contains("noisy"));

    // env：声明 set + AV_* 注入 + 默认继承 all
    let (ctx, _) = build_tool_context(
        &def,
        Some("sess-1".into()),
        pipi_protocol::AbortSignal::new(),
    )
    .unwrap();
    assert_eq!(
        ctx.resolved_env
            .get("CONTRACT_E2E_MARKER")
            .map(String::as_str),
        Some("on")
    );
    assert_eq!(
        ctx.resolved_env.get("AV_AGENT").map(String::as_str),
        Some("__pipi_contract_e2e__")
    );
    assert_eq!(
        ctx.resolved_env.get("AV_SESSION").map(String::as_str),
        Some("sess-1")
    );
    assert!(ctx.resolved_env.contains_key("PATH"));

    // 逃逸包含根 fail-closed
    let outside = std::env::temp_dir().join(format!(
        "pipi-contract-e2e-outside-{}.md",
        pipi_agent::session::new_id()
    ));
    fs::write(&outside, "outside").unwrap();
    fs::write(
        root.join("agent.toml"),
        format!(
            "schema = 1\n\n[resources]\ninstructions = [\"../{}\"]\n",
            outside.file_name().unwrap().to_string_lossy()
        ),
    )
    .unwrap();
    let err = build_system_prompt_with_tools(&def, &[]).unwrap_err();
    assert!(err.contains("fail-closed"), "{err}");

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(outside);
}

/// 项目层契约可以用 `[resources.skills] sources` 指向仓库内的技能包目录
/// （多仓库工作区用 `.agents/skills` 这种既有约定），不必改造成 `.pi/skills`。
#[test]
fn project_contract_skills_sources_point_into_the_repo() {
    let root = std::env::temp_dir().join(format!(
        "pipi-skills-sources-{}",
        pipi_agent::session::new_id()
    ));
    fs::create_dir_all(root.join(".git")).unwrap();
    let pack = root.join(".agents").join("skills").join("workspace-guide");
    fs::create_dir_all(&pack).unwrap();
    fs::write(
        pack.join("SKILL.md"),
        "---\nname: workspace-guide\ndescription: repo skill pack\n---\nbody",
    )
    .unwrap();
    fs::write(
        root.join("agent.toml"),
        "schema = 1\n\n[resources.skills]\nsources = [\".agents/skills\"]\n",
    )
    .unwrap();

    let def = AgentDefinition {
        name: "__pipi_skills_sources__".into(),
        description: String::new(),
        model: String::new(),
        provider: None,
        workspace: Some(root.to_string_lossy().into_owned()),
        permissions: PermissionsConfig {
            tools: vec!["read".into(), "bash".into()],
            ..Default::default()
        },
        mcp_servers: Vec::new(),
        subagent: false,
        compact_threshold_percent: 75,
        compact_target_percent: None,
    };

    let prompt = build_system_prompt_with_tools(&def, &[]).unwrap();
    assert!(prompt.contains("<name>workspace-guide</name>"), "{prompt}");
    assert!(prompt.contains("repo skill pack"), "{prompt}");

    let _ = fs::remove_dir_all(root);
}

/// 声明启用的 store 技能（av `[resources.skills].use` + `agent.lock`）：
/// 安装后进入系统提示与 read_roots；锁/内容缺失即 fail-closed 并提示 `av skill sync`。
#[test]
fn declared_store_skills_are_injected_and_missing_fails_closed() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = std::env::temp_dir().join(format!(
        "pipi-store-skills-home-{}",
        pipi_agent::session::new_id()
    ));
    fs::create_dir_all(&home).unwrap();
    let previous_home = std::env::var("HOME").unwrap();
    let previous_av = std::env::var("AV_HOME").ok();
    pipi_test_support::set_env_var("HOME", &home);
    // 避免外部覆盖影响 store 位置（av 家目录 = $AV_HOME 或 ~/.av）
    pipi_test_support::remove_env_var("AV_HOME");

    // 1) 真实安装管线装进 store（与 `av skill install` 同一实现）
    let source = home.join("source").join("pdf");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: pdf\ndescription: STORE-DECLARED-PDF\n---\nbody",
    )
    .unwrap();
    let spec = av::sources::SourceSpec::Local {
        path: fs::canonicalize(&source).unwrap(),
    };
    let report =
        av::install::install_source(&spec, &av::install::Selection::All, false, &mut |_| {})
            .unwrap();
    let entry = report.installed[0].entry.clone();

    // 2) 项目契约：声明 use + 同层 agent.lock
    let root = home.join("project");
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(
        root.join("agent.toml"),
        "schema = 1\n\n[resources.skills]\nuse = [\"pdf\"]\n",
    )
    .unwrap();
    let mut lock = av::store::SkillLock::empty();
    lock.set_entry(entry.clone());
    let lock_path = root.join("agent.lock");
    lock.write(&lock_path).unwrap();

    let def = AgentDefinition {
        name: "__pipi_store_skills__".into(),
        description: String::new(),
        model: String::new(),
        provider: None,
        workspace: Some(root.to_string_lossy().into_owned()),
        permissions: PermissionsConfig {
            tools: vec!["read".into(), "bash".into()],
            ..Default::default()
        },
        mcp_servers: Vec::new(),
        subagent: false,
        compact_threshold_percent: 75,
        compact_target_percent: None,
    };

    // 3) 提示词注入 + 内容目录进受信任读取根
    let prompt = build_system_prompt_with_tools(&def, &[]).unwrap();
    assert!(prompt.contains("<name>pdf</name>"), "{prompt}");
    assert!(prompt.contains("STORE-DECLARED-PDF"), "{prompt}");
    let (ctx, _) = build_tool_context(&def, None, pipi_protocol::AbortSignal::new()).unwrap();
    let content_dir_name = av::store::content_dir_name(&entry.content_hash).unwrap();
    assert!(
        ctx.read_roots.iter().any(|path| {
            path.file_name().and_then(|name| name.to_str()) == Some(content_dir_name)
        }),
        "store 技能目录应在 read_roots：{:?}",
        ctx.read_roots
    );

    // 4) store 内容缺失：两处路径都应 fail-closed 且带 av skill sync 提示
    av::store::remove_version("pdf", &entry.content_hash).unwrap();
    let error = build_system_prompt_with_tools(&def, &[]).unwrap_err();
    assert!(error.contains("av skill sync"), "{error}");
    let error = build_tool_context(&def, None, pipi_protocol::AbortSignal::new())
        .err()
        .expect("内容缺失应 fail-closed");
    assert!(error.contains("av skill sync"), "{error}");

    // 5) 缺锁文件：fail-closed
    fs::remove_file(&lock_path).unwrap();
    let error = build_system_prompt_with_tools(&def, &[]).unwrap_err();
    assert!(error.contains("agent.lock"), "{error}");

    pipi_test_support::set_env_var("HOME", previous_home);
    match previous_av {
        Some(value) => pipi_test_support::set_env_var("AV_HOME", value),
        None => pipi_test_support::remove_env_var("AV_HOME"),
    }
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn system_prompt_injects_memory_index_with_summaries() {
    let _guard = HOME_LOCK.lock().unwrap();
    let name = format!("memory-index-{}", pipi_agent::session::new_id());
    let def = create_agent(&name, "", None, None, None, None).unwrap();
    let memory_dir = def.memory_dir().unwrap();
    fs::write(
        memory_dir.join("user-prefs.md"),
        "# 用户偏好\n\n简洁回答，中文优先。\n",
    )
    .unwrap();

    let prompt = build_system_prompt_with_tools(&def, &[]).unwrap();
    assert!(prompt.contains("<persistent_memory>"));
    assert!(prompt.contains("<path>"));
    assert!(prompt.contains(&memory_dir.join("user-prefs.md").display().to_string()));
    assert!(prompt.contains("用户偏好"));
    // 渐进披露：只注入摘要，不注入正文
    assert!(prompt.contains("简洁回答") || !prompt.contains("中文优先"));

    // 空 memory 时零打扰
    fs::remove_file(memory_dir.join("user-prefs.md")).unwrap();
    let prompt = build_system_prompt_with_tools(&def, &[]).unwrap();
    assert!(!prompt.contains("<persistent_memory>"));

    let _ = fs::remove_dir_all(agent_dir(&name).unwrap());
}

#[test]
fn tool_context_grants_read_access_to_memory_dir() {
    let _guard = HOME_LOCK.lock().unwrap();
    let name = format!("memory-read-{}", pipi_agent::session::new_id());
    let def = create_agent(&name, "", None, None, None, None).unwrap();
    let memory_dir = def.memory_dir().unwrap();
    fs::write(memory_dir.join("note.md"), "hello").unwrap();

    let (ctx, _) = build_tool_context(&def, None, pipi_protocol::AbortSignal::new()).unwrap();
    let read_root = ctx
        .read_roots
        .iter()
        .find(|root| root.ends_with("memory"))
        .expect("memory dir should be a trusted read root");

    // read 工具路径解析能通过 memory 根
    let resolved = pipi_tools::resolve_read_path(
        &ctx.workspace,
        &ctx.read_roots,
        &memory_dir.join("note.md").display().to_string(),
    )
    .unwrap();
    assert!(resolved.starts_with(read_root));

    let _ = fs::remove_dir_all(agent_dir(&name).unwrap());
}

#[cfg(unix)]
#[test]
fn build_tool_context_rejects_skills_symlink_outside_agent_dir() {
    use std::os::unix::fs::symlink;

    let _guard = HOME_LOCK.lock().unwrap();
    let home = std::env::temp_dir().join(format!(
        "pipi-skills-link-home-{}",
        pipi_agent::session::new_id()
    ));
    let external = std::env::temp_dir().join(format!(
        "pipi-skills-link-external-{}",
        pipi_agent::session::new_id()
    ));
    let agent_dir = home.join(".pipi/agents/skills-link-agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(&external).unwrap();
    symlink(&external, agent_dir.join("skills")).unwrap();

    let previous_home = std::env::var("HOME").unwrap();
    pipi_test_support::set_env_var("HOME", &home);
    let def = AgentDefinition {
        name: "skills-link-agent".into(),
        description: String::new(),
        model: String::new(),
        provider: None,
        workspace: None,
        permissions: PermissionsConfig::default(),
        mcp_servers: Vec::new(),
        subagent: false,
        compact_threshold_percent: 75,
        compact_target_percent: None,
    };

    let result = build_tool_context(&def, None, pipi_protocol::AbortSignal::new());
    pipi_test_support::set_env_var("HOME", previous_home);
    std::fs::remove_dir_all(&home).unwrap();
    std::fs::remove_dir_all(&external).unwrap();

    let error = match result {
        Ok(_) => panic!("skills symlink outside the agent must fail closed"),
        Err(error) => error,
    };
    assert!(error.contains("skills"), "unexpected error: {error}");
}

#[test]
fn tilde_expansion() {
    // 读取 HOME 前先拿锁，避免与其他测试的 set_var 竞争
    let _guard = HOME_LOCK.lock().unwrap();
    let home = dirs::home_dir().expect("home dir");
    assert_eq!(expand_tilde("~/foo"), home.join("foo").to_string_lossy());
    assert_eq!(expand_tilde("~"), home.to_string_lossy());
    assert_eq!(expand_tilde("/abs/path"), "/abs/path");
    assert_eq!(expand_tilde("rel/path"), "rel/path");
}

#[test]
fn agent_file_editor_is_restricted_to_agents_md_and_memory() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = std::env::temp_dir().join(format!("pipi-edit-{}", pipi_agent::session::new_id()));
    std::fs::create_dir_all(&home).unwrap();
    let prev = std::env::var("HOME").unwrap();
    pipi_test_support::set_env_var("HOME", &home);
    create_agent("editor", "d", None, None, None, None).unwrap();

    // AGENTS.md 可读写
    write_agent_file("editor", "AGENTS.md", "# 新指令\n").unwrap();
    assert_eq!(
        read_agent_file("editor", "AGENTS.md").unwrap(),
        "# 新指令\n"
    );

    // memory 新文件可创建并读写
    write_agent_file("editor", "memory/user-prefs.md", "偏好： concise\n").unwrap();
    assert!(
        read_agent_file("editor", "memory/user-prefs.md")
            .unwrap()
            .contains("偏好")
    );

    // 拒绝：绝对路径、逃逸、agent.json、其他扩展名、过深路径
    assert!(read_agent_file("editor", "/etc/passwd").is_err());
    assert!(read_agent_file("editor", "../escape.md").is_err());
    assert!(write_agent_file("editor", "agent.json", "{}").is_err());
    assert!(write_agent_file("editor", "memory/notes.txt", "x").is_err());
    assert!(write_agent_file("editor", "skills/tool/SKILL.md", "x").is_err());
    assert!(write_agent_file("editor", "memory/a/b/c/d.md", "x").is_err());
    assert!(read_agent_file("editor", "memory/missing.md").is_err());
    // AGENTS.md 不允许通过 write 新建（已被 create_agent 管理）
    std::fs::remove_file(home.join(".pipi/agents/editor/AGENTS.md")).unwrap();
    assert!(write_agent_file("editor", "AGENTS.md", "x").is_err());

    pipi_test_support::set_env_var("HOME", prev);
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn create_rejects_bad_names_and_tools() {
    assert!(create_agent("", "d", None, None, None, None).is_err());
    assert!(create_agent("bad name", "d", None, None, None, None).is_err());
    assert!(create_agent("../escape", "d", None, None, None, None).is_err());
    assert!(
        create_agent(
            "x",
            "d",
            None,
            Some(PermissionsConfig {
                tools: vec!["nuclear".into()],
                bash: Default::default(),
                sandbox: Default::default(),
            }),
            None,
            None,
        )
        .is_err()
    );
    assert!(
        create_agent(
            "x",
            "d",
            None,
            Some(PermissionsConfig {
                tools: vec![],
                bash: Default::default(),
                sandbox: Default::default(),
            }),
            None,
            None,
        )
        .is_err()
    );
}

#[test]
fn validate_definition_rejects_bad_tools_workspace_and_mcp() {
    let base = || AgentDefinition {
        name: "ok-name".into(),
        description: String::new(),
        model: String::new(),
        provider: None,
        workspace: None,
        permissions: PermissionsConfig::default(),
        mcp_servers: Vec::new(),
        subagent: false,
        compact_threshold_percent: 75,
        compact_target_percent: None,
    };

    // 未知工具 / 空工具名单
    let mut def = base();
    def.permissions.tools = vec!["nuclear".into()];
    assert!(validate_definition(&def).is_err());
    def.permissions.tools = vec![];
    assert!(validate_definition(&def).is_err());
    // 相对工作目录拒绝，绝对路径通过
    def.permissions.tools = vec!["read".into()];
    def.workspace = Some("relative/path".into());
    assert!(validate_definition(&def).is_err());
    def.workspace = Some("/absolute/path".into());
    assert!(validate_definition(&def).is_ok());
    // 名称非法
    def.workspace = None;
    def.name = "bad name".into();
    assert!(validate_definition(&def).is_err());
    def.name = "ok-name".into();
    assert!(validate_definition(&def).is_ok());
    // MCP 名称重复拒绝
    let server = |name: &str| McpServerConfig {
        name: name.into(),
        command: "cmd".into(),
        args: Vec::new(),
        env: Default::default(),
        enabled: true,
    };
    def.mcp_servers = vec![server("a"), server("a")];
    assert!(validate_definition(&def).is_err());
    def.mcp_servers = vec![server("a"), server("b")];
    assert!(validate_definition(&def).is_ok());
}

// HOME 环境变量是进程级的：涉及 ~/.pipi 的测试互斥串行。
// 锁本体在 crate 根（lib.rs），runtime 等其他模块的 HOME 测试共享同一把。
use pipi_test_support::HOME_LOCK;

#[test]
fn create_writes_skeleton_with_workspace_and_model() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = std::env::temp_dir().join(format!("pipi-home-{}", pipi_agent::session::new_id()));
    std::fs::create_dir_all(&home).unwrap();
    let prev = std::env::var("HOME").unwrap();
    pipi_test_support::set_env_var("HOME", &home);

    // 默认骨架：目录齐备、agent.json 落盘权限、未指定 workspace 时建默认目录
    let def = create_agent(
        "tester",
        "测试用",
        None,
        Some(PermissionsConfig {
            tools: vec!["read".into(), "bash".into()],
            bash: pipi_tools::permissions::BashPermissions {
                mode: pipi_tools::permissions::BashMode::Allowlist,
                commands: vec!["git".into()],
            },
            sandbox: Default::default(),
        }),
        None,
        None,
    )
    .unwrap();
    let dir = home.join(".pipi/agents/tester");
    for file in ["agent.json", "AGENTS.md"] {
        assert!(dir.join(file).is_file(), "{file} 未创建");
    }
    for sub in ["skills", "memory", "jobs", "sessions", "workspace"] {
        assert!(dir.join(sub).is_dir(), "{sub}/ 未创建");
    }
    let text = std::fs::read_to_string(dir.join("agent.json")).unwrap();
    assert!(text.contains("\"allowlist\""));
    assert!(text.contains("\"tools\": ["));
    assert_eq!(def.resolve_workspace().unwrap(), dir.join("workspace"));
    // 重复创建被拒
    assert!(create_agent("tester", "", None, None, None, None).is_err());

    // 显式 workspace：不建默认目录，解析结果就是声明的路径
    let ws = std::env::temp_dir().join(format!("pipi-ws-{}", pipi_agent::session::new_id()));
    let def = create_agent("ws-agent", "", Some(ws.to_str().unwrap()), None, None, None).unwrap();
    assert_eq!(def.workspace.as_deref(), Some(ws.to_str().unwrap()));
    assert!(!home.join(".pipi/agents/ws-agent/workspace").exists());
    assert_eq!(def.resolve_workspace().unwrap(), ws);

    // 默认模型写盘并可读回
    let provider = Model {
        id: "claude-3-7-sonnet-20250219".into(),
        name: "Claude 3.7 Sonnet".into(),
        api: pipi_protocol::Api::AnthropicMessages,
        base_url: "https://api.anthropic.com".into(),
        max_tokens: 8192,
        context_window: 200000,
    };
    let def = create_agent(
        "model-agent",
        "测试默认模型",
        None,
        None,
        Some("claude-3-7-sonnet-20250219"),
        Some(provider.clone()),
    )
    .unwrap();
    assert_eq!(def.model, "claude-3-7-sonnet-20250219");
    assert_eq!(def.provider, Some(provider));
    let loaded = load_agent("model-agent").unwrap();
    assert_eq!(loaded.model, "claude-3-7-sonnet-20250219");
    assert!(loaded.provider.is_some());

    pipi_test_support::set_env_var("HOME", &prev);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn compact_threshold_defaults_and_bounds() {
    let minimal = r#"{"name":"t","permissions":{"tools":["read"]}}"#;
    let def: AgentDefinition = serde_json::from_str(minimal).unwrap();
    assert_eq!(def.compact_threshold_percent, 75);
    assert_eq!(def.compact_threshold_percent(), 75);

    // 显式值生效（含边界）
    let custom = r#"{"name":"t","permissions":{"tools":["read"]},"compactThresholdPercent":40}"#;
    let def: AgentDefinition = serde_json::from_str(custom).unwrap();
    assert_eq!(def.compact_threshold_percent(), 40);

    // 越界 → 退回默认（手改过的文件不该让压缩失效或不可预期）
    for value in [0u8, 101, 200] {
        let text = format!(
            r#"{{"name":"t","permissions":{{"tools":["read"]}},"compactThresholdPercent":{value}}}"#
        );
        let def: AgentDefinition = serde_json::from_str(&text).unwrap();
        assert_eq!(
            def.compact_threshold_percent(),
            75,
            "值 {value} 应兜底成默认"
        );
    }

    // 序列化键名与前端类型对齐（此刻 def 是上面那个 40 的）
    let json = serde_json::to_string(&def).unwrap();
    assert!(json.contains("\"compactThresholdPercent\":40"), "{json}");
}

#[test]
fn compact_target_is_optional_and_uses_camel_case() {
    let minimal = r#"{"name":"t","permissions":{"tools":["read"]}}"#;
    let def: AgentDefinition = serde_json::from_str(minimal).unwrap();
    assert_eq!(def.compact_target_percent(), None);
    assert!(
        !serde_json::to_string(&def)
            .unwrap()
            .contains("compactTargetPercent")
    );

    let custom = r#"{"name":"t","permissions":{"tools":["read"]},"compactTargetPercent":20}"#;
    let def: AgentDefinition = serde_json::from_str(custom).unwrap();
    assert_eq!(def.compact_target_percent(), Some(20));
    assert!(
        serde_json::to_string(&def)
            .unwrap()
            .contains("\"compactTargetPercent\":20")
    );

    for value in [0, 100, 200] {
        let text = format!(r#"{{"name":"t","compactTargetPercent":{value}}}"#);
        let def: AgentDefinition = serde_json::from_str(&text).unwrap();
        assert_eq!(def.compact_target_percent(), None);
    }
}

#[test]
fn seeds_default_agent_when_missing_and_never_overwrites() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home =
        std::env::temp_dir().join(format!("pipi-home-seed-{}", pipi_agent::session::new_id()));
    std::fs::create_dir_all(&home).unwrap();
    let prev = std::env::var("HOME").unwrap();
    pipi_test_support::set_env_var("HOME", &home);

    // 目录缺失 → 播种；骨架与 README「Agent 的组成」一致
    let created = ensure_default_agent()
        .unwrap()
        .expect("首次调用应播种默认 Agent");
    assert_eq!(created.name, DEFAULT_AGENT_NAME);
    let dir = home.join(".pipi").join("agents").join(DEFAULT_AGENT_NAME);
    for entry in [
        "agent.json",
        "AGENTS.md",
        "skills",
        "memory",
        "jobs",
        "sessions",
        "workspace",
    ] {
        assert!(dir.join(entry).exists(), "{entry} 应存在");
    }
    // 默认权限：全部基础工具 + 受限沙箱（PermissionsConfig::default 的沙箱是 DangerFullAccess，不能被沿用）
    assert_eq!(
        created.permissions.sandbox,
        pipi_tools::permissions::SandboxMode::WorkspaceWrite
    );
    assert!(created.permissions.tools.iter().any(|tool| tool == "bash"));
    assert!(
        created.provider.is_none(),
        "播种的默认 Agent 不应凭空绑定模型"
    );

    // 已存在 → 不再创建，且不覆盖用户改动
    std::fs::write(dir.join("AGENTS.md"), "# Pipi\n\n用户改过的说明\n").unwrap();
    assert!(ensure_default_agent().unwrap().is_none());
    let agents = list_agents_bootstrapped().unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, DEFAULT_AGENT_NAME);
    assert!(
        std::fs::read_to_string(dir.join("AGENTS.md"))
            .unwrap()
            .contains("用户改过的说明"),
        "播种不得覆盖用户对 AGENTS.md 的改动"
    );

    pipi_test_support::set_env_var("HOME", &prev);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn bootstrap_keeps_existing_agents_and_backfills_default() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home =
        std::env::temp_dir().join(format!("pipi-home-fill-{}", pipi_agent::session::new_id()));
    std::fs::create_dir_all(&home).unwrap();
    let prev = std::env::var("HOME").unwrap();
    pipi_test_support::set_env_var("HOME", &home);

    create_agent("helper", "已有的 Agent", None, None, None, None).unwrap();
    assert_eq!(
        list_agents().unwrap().len(),
        1,
        "list_agents 本身不应有副作用"
    );

    // 有其它 Agent 但没有默认 Agent → 保留原样并补上默认 Agent
    let agents = list_agents_bootstrapped().unwrap();
    let names: Vec<String> = agents.iter().map(|agent| agent.name.clone()).collect();
    assert!(names.contains(&"helper".to_string()));
    assert!(names.contains(&DEFAULT_AGENT_NAME.to_string()));
    assert_eq!(names.len(), 2);
    // 幂等
    assert!(ensure_default_agent().unwrap().is_none());
    assert_eq!(list_agents().unwrap().len(), 2);

    pipi_test_support::set_env_var("HOME", &prev);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn agent_archive_restore_delete_lifecycle() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home =
        std::env::temp_dir().join(format!("pipi-home-arch-{}", pipi_agent::session::new_id()));
    std::fs::create_dir_all(&home).unwrap();
    let prev = std::env::var("HOME").unwrap();
    pipi_test_support::set_env_var("HOME", &home);

    let name = "arch-lifecycle";
    create_agent(name, "归档生命周期", None, None, None, None).unwrap();
    assert!(list_agents().unwrap().iter().any(|a| a.name == name));
    assert!(list_archived_agents().unwrap().is_empty());

    // 归档：活跃列表消失、归档列表出现、目录真的移动了
    archive_agent(name).unwrap();
    assert!(!list_agents().unwrap().iter().any(|a| a.name == name));
    assert!(
        list_archived_agents()
            .unwrap()
            .iter()
            .any(|a| a.name == name)
    );
    let dir = agents_dir().unwrap();
    assert!(dir.join(ARCHIVE_DIR).join(name).is_dir());
    assert!(!dir.join(name).exists());

    // 重复归档同一名字的 Agent → 该名字现在已不在活跃区 → 报错
    assert!(archive_agent(name).is_err());

    // 恢复：回到活跃区
    restore_agent(name).unwrap();
    assert!(list_agents().unwrap().iter().any(|a| a.name == name));
    assert!(
        !list_archived_agents()
            .unwrap()
            .iter()
            .any(|a| a.name == name)
    );

    // 恢复一个不存在的归档 Agent → 报错
    assert!(restore_agent("no-such-archived-agent").is_err());

    // 删除（活跃区）
    delete_agent(name).unwrap();
    assert!(!list_agents().unwrap().iter().any(|a| a.name == name));

    // 归档区删除
    archive_agent("arch-lifecycle-2").unwrap_or_else(|_| {
        create_agent("arch-lifecycle-2", "归档删除", None, None, None, None).unwrap();
        archive_agent("arch-lifecycle-2").unwrap();
    });
    // 上一步用 create+archive 保证该名字存在
    delete_archived_agent("arch-lifecycle-2").unwrap();
    assert!(
        !list_archived_agents()
            .unwrap()
            .iter()
            .any(|a| a.name == "arch-lifecycle-2")
    );

    // 删除不存在的 Agent → 报错
    assert!(delete_agent("no-such-agent").is_err());

    pipi_test_support::set_env_var("HOME", &prev);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn archive_scan_skips_dot_dir_with_manifest() {
    // 归档区里即使意外出现 agent.json，活跃列表也不得扫到（点目录硬跳过）
    let _guard = HOME_LOCK.lock().unwrap();
    let home =
        std::env::temp_dir().join(format!("pipi-home-dot-{}", pipi_agent::session::new_id()));
    std::fs::create_dir_all(&home).unwrap();
    let prev = std::env::var("HOME").unwrap();
    pipi_test_support::set_env_var("HOME", &home);

    let name = "dot-dir-agent";
    create_agent(name, "点目录防漏", None, None, None, None).unwrap();
    archive_agent(name).unwrap();
    // 往归档区里塞一个「坏掉」的 agent.json（不该影响活跃列表）
    let archive_agent_dir = agents_dir().unwrap().join(ARCHIVE_DIR).join(name);
    std::fs::write(archive_agent_dir.join("agent.json"), "{ broken json").unwrap();
    assert!(!list_agents().unwrap().iter().any(|a| a.name == name));

    pipi_test_support::set_env_var("HOME", &prev);
    let _ = std::fs::remove_dir_all(&home);
}
