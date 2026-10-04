use super::*;
use crate::test_support::make_repo;

#[test]
fn spec_parsing_shorthands() {
    assert_eq!(
        parse_spec("vercel-labs/agent-skills").unwrap(),
        SourceSpec::Git {
            url: "https://github.com/vercel-labs/agent-skills".into(),
            reference: None,
            skill_path: None,
        }
    );
    assert_eq!(
        parse_spec("github:o/r@v1.2#skills/pdf").unwrap(),
        SourceSpec::Git {
            url: "https://github.com/o/r".into(),
            reference: Some("v1.2".into()),
            skill_path: Some("skills/pdf".into()),
        }
    );
    assert_eq!(
        parse_spec("gitlab:o/r@main").unwrap(),
        SourceSpec::Git {
            url: "https://gitlab.com/o/r".into(),
            reference: Some("main".into()),
            skill_path: None,
        }
    );
    // tree URL（GitHub / GitLab）
    assert_eq!(
        parse_spec("https://github.com/o/r/tree/v1/skills/pdf").unwrap(),
        SourceSpec::Git {
            url: "https://github.com/o/r".into(),
            reference: Some("v1".into()),
            skill_path: Some("skills/pdf".into()),
        }
    );
    assert_eq!(
        parse_spec("https://gitlab.com/o/r/-/tree/main/skills/pdf").unwrap(),
        SourceSpec::Git {
            url: "https://gitlab.com/o/r".into(),
            reference: Some("main".into()),
            skill_path: Some("skills/pdf".into()),
        }
    );
}

#[test]
fn spec_parsing_git_urls() {
    assert_eq!(
        parse_spec("https://github.com/o/r.git@v2").unwrap(),
        SourceSpec::Git {
            url: "https://github.com/o/r.git".into(),
            reference: Some("v2".into()),
            skill_path: None,
        }
    );
    // userinfo 的 @ 不能被当成 ref
    assert_eq!(
        parse_spec("https://user@example.com/o/r").unwrap(),
        SourceSpec::Git {
            url: "https://user@example.com/o/r".into(),
            reference: None,
            skill_path: None,
        }
    );
    // scp 形式
    assert_eq!(
        parse_spec("git@github.com:o/r.git@v3").unwrap(),
        SourceSpec::Git {
            url: "git@github.com:o/r.git".into(),
            reference: Some("v3".into()),
            skill_path: None,
        }
    );
    assert_eq!(
        parse_spec("git@github.com:o/r.git").unwrap(),
        SourceSpec::Git {
            url: "git@github.com:o/r.git".into(),
            reference: None,
            skill_path: None,
        }
    );
    // ref 带 / 要拒绝
    assert!(parse_spec("owner/repo@feature/x").is_err());
}

#[test]
fn spec_parsing_local_and_fail_closed() {
    let root = pipi_test_support::temp_dir("av-sources");
    let spec = parse_spec(root.to_str().unwrap()).unwrap();
    assert_eq!(spec.source_type(), SourceType::Local);
    assert!(parse_spec("./definitely-not-here-xyz").is_err());

    for bad in [
        "",
        "skills.sh",
        "https://skills.sh/p/abc",
        "no-slashes",
        "a/b/c/d",
    ] {
        assert!(parse_spec(bad).is_err(), "应拒绝：{bad:?}");
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn discovery_finds_nested_and_convention_dirs() {
    let root = pipi_test_support::temp_dir("av-sources");
    // 约定目录：skills/ 下两个技能 + 一层分组
    fs::create_dir_all(root.join("skills/pdf")).unwrap();
    fs::write(
        root.join("skills/pdf/SKILL.md"),
        "---\nname: pdf\ndescription: pdf skill\n---\nbody",
    )
    .unwrap();
    fs::create_dir_all(root.join("skills/group/writer")).unwrap();
    fs::write(
        root.join("skills/group/writer/SKILL.md"),
        "---\nname: writer\ndescription: writing\n---\nbody",
    )
    .unwrap();
    // 非技能目录（无 SKILL.md）不该被当技能
    fs::create_dir_all(root.join("skills/not-a-skill")).unwrap();
    fs::write(root.join("skills/not-a-skill/README.md"), "x").unwrap();

    let discovery = discover_source_skills(&root, None).unwrap();
    let names: Vec<&str> = discovery.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["writer", "pdf"], "顺序：扫描到即记录");
    assert_eq!(discovery.skills[0].relative, "skills/group/writer");

    // 子路径限定
    let discovery = discover_source_skills(&root, Some("skills/pdf")).unwrap();
    assert_eq!(discovery.skills.len(), 1);
    assert_eq!(discovery.skills[0].name, "pdf");
    assert_eq!(discovery.skills[0].relative, "skills/pdf");

    // 逃逸子路径拒绝
    assert!(discover_source_skills(&root, Some("../outside")).is_err());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn discovery_root_is_skill_and_invalid_reported() {
    let root = pipi_test_support::temp_dir("av-sources");
    fs::write(
        root.join("SKILL.md"),
        "---\nname: solo\ndescription: solo skill\n---\nbody",
    )
    .unwrap();
    let discovery = discover_source_skills(&root, None).unwrap();
    assert_eq!(discovery.skills.len(), 1);
    assert_eq!(discovery.skills[0].relative, ".");

    // 兄弟目录里坏的 SKILL.md 进入 skipped
    fs::create_dir_all(root.join("broken")).unwrap();
    fs::write(root.join("broken/SKILL.md"), "---\nname: broken\n---\nbody").unwrap();
    let discovery = discover_source_skills(&root, None).unwrap();
    assert_eq!(discovery.skills.len(), 1, "根自身即技能，不再下钻");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn resolve_and_fetch_local_git_repo() {
    let (repo, head) = make_repo("git-repo");
    let url = repo.to_string_lossy().into_owned();

    assert_eq!(resolve_revision(&url, None).unwrap(), head);
    assert_eq!(resolve_revision(&url, Some("v1")).unwrap(), head);
    assert_eq!(resolve_revision(&url, Some(&head)).unwrap(), head);

    let staging = pipi_test_support::temp_dir("av-sources");
    let fetch = fetch_at_in(&staging, &url, Some("v1"), &head).unwrap();
    assert_eq!(fetch.revision, head);
    assert!(fetch.worktree.join("skills/pdf/SKILL.md").is_file());
    let worktree = fetch.worktree.clone();
    drop(fetch);
    assert!(!worktree.exists(), "Drop 应清理暂存工作树");

    // 不存在的 ref fail-closed
    assert!(resolve_revision(&url, Some("nope")).is_err());
    let _ = fs::remove_dir_all(repo);
    let _ = fs::remove_dir_all(staging);
}
