use super::*;

fn make_skill(dir: &Path, name: &str, content: &str) {
    let d = dir.join(name);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("SKILL.md"), content).unwrap();
}

#[test]
fn frontmatter_parsing() {
    let (fm, body) =
        parse_frontmatter("---\nname: git-safety\ndescription: \"安全用 git\"\n---\n\n# 正文\n");
    assert_eq!(fm.len(), 2);
    assert_eq!(fm[0], ("name".into(), "git-safety".into()));
    assert_eq!(fm[1], ("description".into(), "安全用 git".into()));
    assert!(body.starts_with("# 正文"));
    // 无 frontmatter
    let (fm, body) = parse_frontmatter("# 普通文档\n");
    assert!(fm.is_empty());
    assert_eq!(body, "# 普通文档");
    // 未闭合
    let (fm, _) = parse_frontmatter("---\nname: x\n");
    assert!(fm.is_empty());
}

#[test]
fn skill_name_shape() {
    assert!(valid_skill_name("pdf"));
    assert!(valid_skill_name("git-safety-2"));
    assert!(!valid_skill_name(""));
    assert!(!valid_skill_name("PDF"));
    assert!(!valid_skill_name("-x"));
    assert!(!valid_skill_name("x-"));
    assert!(!valid_skill_name("a--b"));
    assert!(!valid_skill_name(&"a".repeat(65)));
}

#[test]
fn loads_sources_in_order_and_falls_back_to_nested_discovery() {
    let root = pipi_test_support::temp_dir("av-skills");
    let agent = root.join("agent");
    let global = agent.join("skills");
    let workspace = root.join("workspace");
    let project = workspace.join(".pi/skills/team/nested");
    fs::create_dir_all(&global).unwrap();
    fs::create_dir_all(&project).unwrap();
    make_skill(
        &global,
        "global",
        "---\nname: global\ndescription: global skill\n---\nbody",
    );
    fs::write(
        project.join("SKILL.md"),
        "---\nname: nested\ndescription: nested project skill\n---\nbody",
    )
    .unwrap();

    let skills = load_skill_sources(&[global.clone(), workspace.join(".pi/skills")], &root);

    assert_eq!(
        skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["global", "nested"]
    );
    assert!(skills[1].path.ends_with("SKILL.md"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn declared_skill_directory_stops_nested_discovery() {
    let root = pipi_test_support::temp_dir("av-skills");
    let workspace = root.join("workspace");
    let skill = workspace.join(".pi/skills/root-skill");
    let nested = skill.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: root-skill\ndescription: root skill\n---\nbody",
    )
    .unwrap();
    fs::write(
        nested.join("SKILL.md"),
        "---\nname: nested-skill\ndescription: nested skill\n---\nbody",
    )
    .unwrap();

    let skills = load_skill_sources(&[workspace.join(".pi/skills")], &workspace);

    assert_eq!(
        skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["root-skill"]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parses_disable_model_invocation_metadata() {
    let root = pipi_test_support::temp_dir("av-skills");
    let workspace = root.join("workspace");
    let project = workspace.join(".pi/skills");
    fs::create_dir_all(&project).unwrap();
    make_skill(
        &project,
        "disabled",
        "---\nname: disabled\ndescription: explicit only\ndisable-model-invocation: true\n---\nbody",
    );

    let skills = load_skill_sources(std::slice::from_ref(&project), &workspace);

    assert_eq!(skills.len(), 1);
    assert!(skills[0].disable_model_invocation);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn earlier_source_wins_over_later_duplicate_name() {
    let root = pipi_test_support::temp_dir("av-skills");
    let agent = root.join("agent");
    let global = agent.join("skills");
    let workspace = root.join("workspace");
    let project = workspace.join(".pi/skills");
    fs::create_dir_all(&global).unwrap();
    fs::create_dir_all(&project).unwrap();
    make_skill(
        &global,
        "global-shared",
        "---\nname: shared\ndescription: global\n---\nbody",
    );
    make_skill(
        &project,
        "project-shared",
        "---\nname: shared\ndescription: project\n---\nbody",
    );

    let skills = load_skill_sources(&[global.clone(), project.clone()], &root);

    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].description.as_deref(), Some("global"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_skill_metadata_is_omitted() {
    let root = pipi_test_support::temp_dir("av-skills");
    let workspace = root.join("workspace");
    let project = workspace.join(".pi/skills");
    fs::create_dir_all(&project).unwrap();
    make_skill(
        &project,
        "missing-description",
        "---\nname: missing-description\n---\nbody",
    );
    make_skill(
        &project,
        "invalid-name",
        "---\nname: Invalid_Name\ndescription: invalid\n---\nbody",
    );
    make_skill(
        &project,
        "valid-name",
        "---\nname: valid-name\ndescription: valid\n---\nbody",
    );

    let skills = load_skill_sources(std::slice::from_ref(&project), &workspace);

    assert_eq!(
        skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["valid-name"]
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(any(unix, windows))]
#[test]
fn rejects_skill_roots_outside_containment_root() {
    let root = pipi_test_support::temp_dir("av-skills");
    let agent = root.join("agent");
    let external = root.join("external");
    fs::create_dir_all(&agent).unwrap();
    fs::create_dir_all(&external).unwrap();
    make_skill(
        &external,
        "outside",
        "---\nname: outside\ndescription: outside\n---\nbody",
    );

    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, agent.join("skills")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&external, agent.join("skills")).unwrap();

    let skills = load_skill_sources(&[agent.join("skills")], &agent);

    assert!(skills.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn load_skill_sources_scans_with_dedupe() {
    let root = pipi_test_support::temp_dir("av-skills");
    let a = root.join("pack-a");
    let b = root.join("pack-b");
    make_skill(
        &a,
        "alpha",
        "---\nname: alpha\ndescription: from a\n---\nbody",
    );
    make_skill(
        &b,
        "beta",
        "---\nname: beta\ndescription: from b\n---\nbody",
    );
    make_skill(
        &b,
        "alpha-dup",
        "---\nname: alpha\ndescription: dup\n---\nbody",
    );

    let skills = load_skill_sources(&[a.clone(), b.clone()], &root);
    assert_eq!(skills.len(), 2, "同名技能先发现者赢：{:?}", skills);
    assert_eq!(skills[0].name, "alpha");
    assert_eq!(skills[0].description.as_deref(), Some("from a"));

    // source 逃逸包含根：拒绝加载
    let outside = pipi_test_support::temp_dir("av-skills");
    make_skill(
        &outside,
        "escaped",
        "---\nname: escaped\ndescription: x\n---\nbody",
    );
    let skills = load_skill_sources(std::slice::from_ref(&outside), &root);
    assert!(skills.is_empty());
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(outside);
}

#[test]
fn filter_skills_only_and_exclude() {
    let make = |name: &str| SkillMeta {
        name: name.to_string(),
        description: None,
        disable_model_invocation: false,
        path: PathBuf::from(format!("/x/{name}/SKILL.md")),
    };
    let skills = vec![
        make("git-safety"),
        make("review-pr"),
        make("experimental-x"),
    ];

    // exclude 优先
    let filtered =
        filter_skills_by_name(skills.clone(), None, Some(&["experimental-*".into()])).unwrap();
    assert_eq!(
        filtered.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        vec!["git-safety", "review-pr"]
    );
    // only 白名单
    let filtered = filter_skills_by_name(skills.clone(), Some(&["git-*".into()]), None).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "git-safety");
    // 同时命中：exclude 赢
    let filtered = filter_skills_by_name(
        skills,
        Some(&["git-*".into()]),
        Some(&["git-safety".into()]),
    )
    .unwrap();
    assert!(filtered.is_empty());
    // 模式非法 fail-closed
    assert!(filter_skills_by_name(vec![], None, Some(&["[".into()])).is_err());
}

#[test]
fn filter_reason_explains_why_a_name_is_dropped() {
    // 未被过滤
    assert_eq!(skill_name_filter_reason("pdf", None, None).unwrap(), None);
    assert_eq!(
        skill_name_filter_reason("git-safety", Some(&["git-*".into()]), None).unwrap(),
        None
    );
    // exclude 命中
    let reason = skill_name_filter_reason("experimental-x", None, Some(&["experimental-*".into()]))
        .unwrap()
        .unwrap();
    assert!(
        reason.contains("exclude") && reason.contains("experimental-*"),
        "{reason}"
    );
    // 不在 only 内
    let reason = skill_name_filter_reason("pdf", Some(&["git-*".into()]), None)
        .unwrap()
        .unwrap();
    assert!(
        reason.contains("only") && reason.contains("git-*"),
        "{reason}"
    );
    // exclude 优先于 only
    let reason = skill_name_filter_reason(
        "git-safety",
        Some(&["git-*".into()]),
        Some(&["git-safety".into()]),
    )
    .unwrap()
    .unwrap();
    assert!(reason.contains("exclude"), "{reason}");
    // 模式非法的口径与 filter_skills_by_name 一致
    assert!(skill_name_filter_reason("pdf", None, Some(&["[".into()])).is_err());
}
