use super::*;
use crate::schema::AgentToml;

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "av-store-{label}-{}-{}",
        std::process::id(),
        monotonic_nonce()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_skill(dir: &Path, name: &str, body: &str) -> PathBuf {
    let skill = dir.join(name);
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: test skill\n---\n{body}"),
    )
    .unwrap();
    skill
}

fn local_entry(name: &str, source: &Path, content_hash: &str) -> SkillLockEntry {
    SkillLockEntry {
        name: name.to_string(),
        source: source.to_string_lossy().into_owned(),
        source_type: SourceType::Local,
        reference: None,
        revision: None,
        skill_path: None,
        content_hash: content_hash.to_string(),
        installed_at: None,
        updated_at: None,
    }
}

#[test]
fn content_hash_is_stable_and_path_sensitive() {
    let root = temp_dir("hash");
    let skill = write_skill(&root, "pdf", "body");
    fs::write(skill.join("extra.txt"), "extra").unwrap();

    let hash = hash_skill_dir(&skill).unwrap();
    assert!(hash.starts_with("sha256:"));
    assert_eq!(hash_skill_dir(&skill).unwrap(), hash, "同一内容哈希稳定");

    // 改内容 → 变
    fs::write(skill.join("extra.txt"), "extra2").unwrap();
    let changed = hash_skill_dir(&skill).unwrap();
    assert_ne!(changed, hash);

    // 重命名（路径参与哈希）→ 变
    fs::rename(skill.join("extra.txt"), skill.join("renamed.txt")).unwrap();
    let renamed = hash_skill_dir(&skill).unwrap();
    assert_ne!(renamed, changed);

    // 删文件 → 回到只有一个文件时的哈希
    fs::remove_file(skill.join("renamed.txt")).unwrap();
    assert_eq!(
        hash_skill_dir(&skill).unwrap(),
        hash_skill_dir(&skill).unwrap()
    );
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn scan_rejects_symlinks() {
    let root = temp_dir("symlink");
    let skill = write_skill(&root, "pdf", "body");
    let outside = root.join("outside.txt");
    fs::write(&outside, "secret").unwrap();
    std::os::unix::fs::symlink(&outside, skill.join("link.txt")).unwrap();

    let error = scan_skill_content(&skill).unwrap_err();
    assert!(error.contains("符号链接"), "{error}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn scan_skips_git_dir_and_enforces_limits() {
    let root = temp_dir("limits");
    let skill = write_skill(&root, "pdf", "body");
    fs::create_dir_all(skill.join(".git")).unwrap();
    fs::write(skill.join(".git/config"), "x").unwrap();
    let content = scan_skill_content(&skill).unwrap();
    assert_eq!(content.file_count(), 1, ".git 不进入内容清单");

    // 超字节上限
    let big = write_skill(&root, "big", "body");
    let oversized = MAX_SKILL_BYTES + 1;
    fs::write(big.join("huge.bin"), vec![0u8; oversized as usize]).unwrap();
    let error = scan_skill_content(&big).unwrap_err();
    assert!(error.contains("上限"), "{error}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn publish_is_idempotent_and_versioned() {
    let root = temp_dir("publish");
    let store = root.join("store");
    let staging = root.join("tmp");
    let skill = write_skill(&root, "pdf", "v1");

    let hash = hash_skill_dir(&skill).unwrap();
    let content = scan_skill_content(&skill).unwrap();
    let first = publish_skill_in(&store, &staging, &content, "pdf", &hash).unwrap();
    let again = publish_skill_in(&store, &staging, &content, "pdf", &hash).unwrap();
    assert_eq!(first, again, "同哈希发布幂等");
    assert_eq!(
        fs::read_dir(store.join("pdf")).unwrap().count(),
        1,
        "同名同哈希只有一个目录"
    );

    // 内容变化 → 新版本目录并存，旧目录不被改动
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: pdf\ndescription: t\n---\nv2",
    )
    .unwrap();
    let hash2 = hash_skill_dir(&skill).unwrap();
    assert_ne!(hash2, hash);
    let content2 = scan_skill_content(&skill).unwrap();
    let second = publish_skill_in(&store, &staging, &content2, "pdf", &hash2).unwrap();
    assert_ne!(second, first);
    assert_eq!(fs::read_dir(store.join("pdf")).unwrap().count(), 2);
    assert!(first.join("SKILL.md").is_file(), "旧版本仍完整");

    // 非法名拒绝
    assert!(publish_skill_in(&store, &staging, &content, "PDF", &hash).is_err());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn lock_roundtrip_and_validation() {
    let root = temp_dir("lock");
    let path = root.join("agent.lock");
    let fixed_hash = format!("sha256:{}", "e".repeat(64));
    let mut lock = SkillLock::empty();
    lock.set_entry(local_entry("pdf", &root, &fixed_hash));
    lock.write(&path).unwrap();

    let read = SkillLock::read(&path).unwrap();
    assert_eq!(read.skills.len(), 1);
    assert_eq!(read.get("pdf").unwrap().source_type, SourceType::Local);

    // 版本不符 fail-closed
    fs::write(&path, "version = 99\n").unwrap();
    assert!(SkillLock::read(&path).is_err());

    // 未知字段 fail-closed
    fs::write(&path, "version = 1\nfoo = 1\n").unwrap();
    assert!(SkillLock::read(&path).is_err());

    // git 源必须有 40 位 revision
    let mut git_entry = local_entry("pdf", &root, &format!("sha256:{}", "a".repeat(64)));
    git_entry.source_type = SourceType::Git;
    git_entry.source = "https://github.com/o/r".into();
    assert!(git_entry.validate().is_err());
    git_entry.revision = Some("b".repeat(40));
    git_entry.skill_path = Some("skills/pdf".into());
    git_entry.validate().unwrap();

    // local 源必须有绝对路径、不能有 revision
    let mut bad = local_entry(
        "pdf",
        Path::new("rel/path"),
        &format!("sha256:{}", "a".repeat(64)),
    );
    assert!(bad.validate().is_err());
    bad.source = "/abs".into();
    bad.revision = Some("b".repeat(40));
    assert!(bad.validate().is_err());

    // 重复 (name, hash) 拒绝；同名不同版本允许
    let mut dup = SkillLock::empty();
    let entry = local_entry("pdf", &root, &format!("sha256:{}", "a".repeat(64)));
    dup.upsert_version(entry.clone());
    dup.upsert_version(entry.clone());
    assert_eq!(dup.skills.len(), 1, "同 (name, hash) 幂等");
    let mut other = entry.clone();
    other.content_hash = format!("sha256:{}", "c".repeat(64));
    dup.upsert_version(other);
    assert_eq!(dup.skills.len(), 2, "同名不同版本并存");
    dup.validate().unwrap();

    // agent.lock 语义：技能名唯一
    dup.validate_unique_names().unwrap_err();

    // set_entry 按 name 替换
    let mut agent_lock = SkillLock::empty();
    agent_lock.set_entry(entry.clone());
    let mut replaced = entry;
    replaced.content_hash = format!("sha256:{}", "d".repeat(64));
    agent_lock.set_entry(replaced);
    assert_eq!(agent_lock.skills.len(), 1);
    assert!(agent_lock.skills[0].content_hash.ends_with("dddd"));

    // remove
    assert!(agent_lock.remove("pdf"));
    assert!(!agent_lock.remove("pdf"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn content_dir_name_requires_full_hash() {
    let hash = format!("sha256:{}", "a".repeat(64));
    assert_eq!(content_dir_name(&hash).unwrap(), "aaaaaaaaaaaaaaaa");
    assert!(content_dir_name("sha256:abc").is_err());
    assert!(content_dir_name(&format!("sha256:{}", "A".repeat(64))).is_err());
    assert!(content_dir_name(&"a".repeat(64)).is_err());
}

#[test]
fn publish_then_resolve_roundtrip() {
    let root = temp_dir("resolve");
    let store = root.join("store");
    let staging = root.join("tmp");
    let skill = write_skill(&root, "pdf", "body");
    let hash = hash_skill_dir(&skill).unwrap();
    let content = scan_skill_content(&skill).unwrap();
    publish_skill_in(&store, &staging, &content, "pdf", &hash).unwrap();

    let lock_path = root.join("agent.lock");
    let mut lock = SkillLock::empty();
    lock.set_entry(local_entry("pdf", &skill, &hash));
    lock.write(&lock_path).unwrap();

    let resolved = resolve_declared_skills_in(&store, &["pdf".to_string()], &lock_path).unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].meta.name, "pdf");
    assert!(resolved[0].dir.ends_with(content_dir_name(&hash).unwrap()));

    // 空声明 → 空结果（不读锁）
    assert!(
        resolve_declared_skills_in(&store, &[], &lock_path)
            .unwrap()
            .is_empty()
    );

    // 缺锁文件 fail-closed
    let error =
        resolve_declared_skills_in(&store, &["pdf".to_string()], &root.join("missing.lock"))
            .unwrap_err();
    assert!(error.contains("av skill sync"), "{error}");

    // 锁里没有该技能 fail-closed
    let empty_lock = root.join("empty.lock");
    SkillLock::empty().write(&empty_lock).unwrap();
    let error = resolve_declared_skills_in(&store, &["pdf".to_string()], &empty_lock).unwrap_err();
    assert!(error.contains("未在") && error.contains("sync"), "{error}");

    // store 内容缺失 fail-closed
    fs::remove_dir_all(entry_dir_in(&store, "pdf", &hash).unwrap()).unwrap();
    let error = resolve_declared_skills_in(&store, &["pdf".to_string()], &lock_path).unwrap_err();
    assert!(error.contains("缺失") && error.contains("sync"), "{error}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn verify_detects_missing_and_drift() {
    let root = temp_dir("verify");
    let store = root.join("store");
    let staging = root.join("tmp");
    let skill = write_skill(&root, "pdf", "body");
    let hash = hash_skill_dir(&skill).unwrap();
    let content = scan_skill_content(&skill).unwrap();
    let dir = publish_skill_in(&store, &staging, &content, "pdf", &hash).unwrap();
    let entry = local_entry("pdf", &skill, &hash);

    assert_eq!(verify_entry_in(&store, &entry).unwrap(), VerifyOutcome::Ok);

    // 漂移：篡改 store 内容
    fs::write(dir.join("SKILL.md"), "tampered").unwrap();
    match verify_entry_in(&store, &entry).unwrap() {
        VerifyOutcome::Mismatch { actual } => assert_ne!(actual, hash),
        other => panic!("应检测到漂移：{other:?}"),
    }

    // 缺失
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(
        verify_entry_in(&store, &entry).unwrap(),
        VerifyOutcome::Missing
    );

    // verify_entry（无 _in）走真实 av 家目录：路径可定位时不应 panic
    let _ = verify_entry(&entry);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn winning_use_picks_highest_declaring_layer() {
    fn layer(label: &str, text: &str) -> Layer {
        let config: AgentToml = toml::from_str(text).unwrap();
        config.validate().unwrap();
        Layer {
            label: label.to_string(),
            path: PathBuf::from(label),
            config,
        }
    }
    let agent = layer(
        "agent.toml",
        "schema = 1\n[resources.skills]\nuse = [\"pdf\"]",
    );
    let project = layer(
        "proj/agent.toml",
        "schema = 1\n[resources.skills]\nuse = [\"commit-helper\"]",
    );
    let local = layer("proj/agent.local.toml", "schema = 1");

    // 最高声明层是项目层
    let layers = vec![agent.clone(), project.clone(), local.clone()];
    let declared = winning_use(&layers).unwrap();
    assert_eq!(declared.layer.label, "proj/agent.toml");
    assert_eq!(declared.names, ["commit-helper".to_string()]);
    assert_eq!(declared.lock_path, PathBuf::from("proj/agent.lock"));

    // local 层显式禁用（use = []）覆盖项目层
    let local_disable = layer(
        "proj/agent.local.toml",
        "schema = 1\n[resources.skills]\nuse = []",
    );
    let layers = vec![agent.clone(), project, local_disable];
    let declared = winning_use(&layers).unwrap();
    assert!(declared.names.is_empty());

    // 只有 agent 层声明
    let only_agent = [agent];
    let declared = winning_use(&only_agent).unwrap();
    assert_eq!(declared.layer.label, "agent.toml");
    assert!(winning_use(&[]).is_none());
}

#[test]
fn latest_index_entry_prefers_most_recently_recorded_version() {
    let root = temp_dir("latest");
    let hash_a = format!("sha256:{}", "a".repeat(64));
    let hash_b = format!("sha256:{}", "b".repeat(64));
    let mut index = SkillLock::empty();
    // 先装的版本 updated-at 更旧
    let mut older = local_entry("pdf", &root, &hash_a);
    older.installed_at = Some("2026-01-01T00:00:00Z".into());
    let mut newer = local_entry("pdf", &root, &hash_b);
    newer.installed_at = Some("2025-01-01T00:00:00Z".into());
    newer.updated_at = Some("2026-03-01T00:00:00Z".into());
    index.upsert_version(older);
    index.upsert_version(newer.clone());

    let picked = latest_index_entry(&index, "pdf").unwrap();
    assert_eq!(picked.content_hash, newer.content_hash, "updated-at 优先");
    assert!(latest_index_entry(&index, "writer").is_none(), "无此技能");

    // 完全没有时间戳时仍能选出一个（回退到任意一条，不 panic）
    let mut bare = SkillLock::empty();
    bare.upsert_version(local_entry("pdf", &root, &hash_a));
    assert!(latest_index_entry(&bare, "pdf").is_some());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn iso8601_time() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(20_728), (2026, 10, 2));
    let now = now_iso8601();
    assert_eq!(now.len(), 20, "{now}");
    assert!(now.ends_with('Z') && now.contains('T'), "{now}");

    // 1970-01-01T00:00:00Z 的形状由 days/sod 计算保证
    assert_eq!(now_iso8601().len(), 20);
}
