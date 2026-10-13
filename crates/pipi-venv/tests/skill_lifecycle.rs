//! 技能中心的端到端（黑盒）回归：安装 → 启用 / 禁用 → 同步 / 校验 →
//! 移除的完整生命周期，只经 `pipi_venv::skills` 的公开入口与 av store 的公开 API。
//!
//! 写路径与 `av skill install/add/sync/remove` 共用同一份实现，所以这里也
//! 是「装 ≠ 启用、缺锁 / 缺内容 fail-closed」语义的回归。

use std::fs;
use std::path::{Path, PathBuf};

use pipi_test_support::HOME_LOCK;
use pipi_venv::agents;
use pipi_venv::av::store::{self, SkillLock};
use pipi_venv::skills::{
    AgentSkillStatus, SkillVerifyStatus, agent_skills, install_skill, remove_store_skill,
    set_skill_enabled, skill_usage, store_skills, sync_agent_skills, verify_store_skills,
};

/// 隔离的 HOME：`~/.pipi`（Agent 数据）与 `~/.av`（技能 store）都落在临时目录。
struct TestHome {
    root: PathBuf,
    previous_home: String,
    previous_av: Option<String>,
    previous_pipi: Option<String>,
}

impl TestHome {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pipi-skill-center-{label}-{}",
            pipi_protocol::new_id()
        ));
        fs::create_dir_all(&root).unwrap();
        let previous_home = std::env::var("HOME").unwrap();
        let previous_av = std::env::var("AV_HOME").ok();
        let previous_pipi = std::env::var("PIPI_HOME").ok();
        pipi_test_support::set_env_var("HOME", &root);
        pipi_test_support::remove_env_var("AV_HOME");
        pipi_test_support::remove_env_var("PIPI_HOME");
        Self {
            root,
            previous_home,
            previous_av,
            previous_pipi,
        }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn agent_dir(&self, name: &str) -> PathBuf {
        self.root.join(".pipi").join("agents").join(name)
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        pipi_test_support::set_env_var("HOME", &self.previous_home);
        match &self.previous_av {
            Some(value) => pipi_test_support::set_env_var("AV_HOME", value),
            None => pipi_test_support::remove_env_var("AV_HOME"),
        }
        match &self.previous_pipi {
            Some(value) => pipi_test_support::set_env_var("PIPI_HOME", value),
            None => pipi_test_support::remove_env_var("PIPI_HOME"),
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn write_skill_source(root: &Path) -> PathBuf {
    let source = root.join("source").join("pdf");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: pdf\ndescription: PDF-DESC\n---\nbody",
    )
    .unwrap();
    source
}

#[test]
fn install_enable_disable_and_purge_roundtrip() {
    let _lock = HOME_LOCK.lock().unwrap();
    let home = TestHome::new("roundtrip");
    let source = write_skill_source(home.root());

    // 安装只进 store（装 ≠ 启用）
    let report = install_skill(source.to_str().unwrap(), None, true, false).unwrap();
    assert_eq!(report.installed.len(), 1);
    assert_eq!(report.installed[0].name, "pdf");
    assert!(!report.installed[0].unchanged);

    let library = store_skills().unwrap();
    assert_eq!(library.skills.len(), 1);
    assert_eq!(library.skills[0].description.as_deref(), Some("PDF-DESC"));
    assert!(library.skills[0].versions[0].present);

    let name = format!("skill-agent-{}", pipi_protocol::new_id());
    let def = agents::create_agent(&name, "", None, None, None, None).unwrap();
    let view = agent_skills(&name).unwrap();
    assert!(view.declared.is_empty(), "安装不等于启用");
    assert_eq!(view.available.len(), 1);
    assert_eq!(view.available[0].name, "pdf");
    assert!(view.available[0].present);
    // 安装只进 store：契约是 create_agent 写的默认模板，不含任何 use 声明
    assert!(view.agent_contract_exists);
    assert!(
        !fs::read_to_string(home.agent_dir(&name).join("agent.toml"))
            .unwrap()
            .contains("use")
    );
    assert!(view.lock_error.is_none());

    // 启用：写 Agent 层 agent.toml + 同目录 agent.lock
    let view = set_skill_enabled(&name, "pdf", true).unwrap();
    assert_eq!(view.declared.len(), 1);
    assert_eq!(view.declared[0].status, AgentSkillStatus::Ready);
    assert_eq!(view.declared[0].description.as_deref(), Some("PDF-DESC"));
    assert_eq!(view.declared_by.as_deref(), Some("agent"));
    assert!(!view.declares_in_project);
    assert!(view.agent_contract_exists);
    assert_eq!(view.available.len(), 0, "已声明的技能不再出现在可用列表");
    let contract = fs::read_to_string(home.agent_dir(&name).join("agent.toml")).unwrap();
    assert!(contract.contains("[resources.skills]"), "{contract}");
    assert!(contract.contains("\"pdf\""), "{contract}");
    assert!(home.agent_dir(&name).join("agent.lock").is_file());

    // 启用后的声明能通过会话启动路径（提示词里出现技能描述）
    let prepared = agents::prepare_agent(&def).unwrap();
    let prompt = agents::build_system_prompt_with_tools(&def, &prepared, &[]).unwrap();
    assert!(prompt.contains("PDF-DESC"), "{prompt}");

    // 重复启用幂等
    let again = set_skill_enabled(&name, "pdf", true).unwrap();
    assert_eq!(again.declared.len(), 1);
    assert_eq!(again.declared[0].status, AgentSkillStatus::Ready);

    // 影响面：这个 Agent 已启用
    let usage = skill_usage("pdf").unwrap();
    assert_eq!(usage.len(), 1);
    assert!(usage[0].enabled);
    assert_eq!(usage[0].status, Some(AgentSkillStatus::Ready));

    // 仍被声明的技能拒绝从 store 移除
    let error = remove_store_skill("pdf").unwrap_err();
    assert!(error.contains(&name), "{error}");

    // 禁用：声明与锁记录一起移除，清理空表
    let view = set_skill_enabled(&name, "pdf", false).unwrap();
    assert!(view.declared.is_empty());
    assert_eq!(view.available.len(), 1);
    assert!(!home.agent_dir(&name).join("agent.lock").exists());
    let contract = fs::read_to_string(home.agent_dir(&name).join("agent.toml")).unwrap();
    assert!(!contract.contains("use"), "{contract}");
    assert!(
        contract.contains("[resources]"),
        "默认声明的素材位置仍在（只清 use）：{contract}"
    );
    assert!(
        agent_skills(&name).unwrap().lock_error.is_none(),
        "没有声明时缺锁不算错误态"
    );

    // 禁用后可移除
    assert_eq!(remove_store_skill("pdf").unwrap(), 1);
    assert!(store_skills().unwrap().skills.is_empty());
    let error = remove_store_skill("pdf").unwrap_err();
    assert!(error.contains("不在 store 索引"), "{error}");
}

#[test]
fn project_layer_declaration_wins_and_filter_is_reported() {
    let _lock = HOME_LOCK.lock().unwrap();
    let home = TestHome::new("project-layer");
    let source = write_skill_source(home.root());
    install_skill(source.to_str().unwrap(), None, true, false).unwrap();

    let name = format!("skill-agent-{}", pipi_protocol::new_id());
    // 项目层：随工作区走的 agent.toml + agent.lock
    let project = home.root().join("project");
    fs::create_dir_all(project.join(".git")).unwrap();
    fs::write(
        project.join("agent.toml"),
        "schema = 1\n\n[resources.skills]\nuse = [\"pdf\"]\n",
    )
    .unwrap();
    let index = store::read_index().unwrap();
    let entry = store::latest_index_entry(&index, "pdf").unwrap().clone();
    let mut lock = SkillLock::empty();
    lock.set_entry(entry);
    lock.write(&project.join("agent.lock")).unwrap();

    let project_path = project.to_string_lossy().into_owned();
    let def =
        agents::create_agent(&name, "", Some(project_path.as_str()), None, None, None).unwrap();
    let view = agent_skills(&name).unwrap();
    assert_eq!(view.declared_by.as_deref(), Some("project"));
    assert!(view.declares_in_project);
    assert_eq!(view.declared[0].status, AgentSkillStatus::Ready);
    assert!(
        view.declared_by_path
            .as_deref()
            .unwrap()
            .ends_with("agent.toml")
    );

    // only 过滤：解析齐备但不会注入
    fs::write(
        project.join("agent.toml"),
        "schema = 1\n\n[resources.skills]\nuse = [\"pdf\"]\nonly = [\"other-*\"]\n",
    )
    .unwrap();
    let view = agent_skills(&name).unwrap();
    assert_eq!(view.declared[0].status, AgentSkillStatus::Filtered);
    assert!(
        view.declared[0].filter.as_deref().unwrap().contains("only"),
        "{:?}",
        view.declared[0].filter
    );

    // 禁用写的是项目层文件（声明所在层），Agent 层契约保持默认模板不动
    let view = set_skill_enabled(&name, "pdf", false).unwrap();
    assert!(view.declared.is_empty());
    let project_text = fs::read_to_string(project.join("agent.toml")).unwrap();
    assert!(!project_text.contains("use"), "{project_text}");
    let agent_text = fs::read_to_string(home.agent_dir(&name).join("agent.toml")).unwrap();
    assert!(!agent_text.contains("use"), "{agent_text}");
    assert!(agent_text.contains("[resources]"), "{agent_text}");

    // 声明清空后再启用：退回 Agent 层；项目层的 only 过滤仍生效（数组按层整体替换）
    let view = set_skill_enabled(&name, "pdf", true).unwrap();
    assert_eq!(view.declared_by.as_deref(), Some("agent"));
    assert!(home.agent_dir(&name).join("agent.toml").is_file());
    assert_eq!(view.declared[0].status, AgentSkillStatus::Filtered);
    let _ = def;
}

#[test]
fn sync_restores_missing_content_resolves_lock_and_repairs_drift() {
    let _lock = HOME_LOCK.lock().unwrap();
    let home = TestHome::new("sync");
    let source = write_skill_source(home.root());
    install_skill(source.to_str().unwrap(), None, true, false).unwrap();
    let name = format!("skill-agent-{}", pipi_protocol::new_id());
    agents::create_agent(&name, "", None, None, None, None).unwrap();
    set_skill_enabled(&name, "pdf", true).unwrap();
    let entry = store::read_index().unwrap().skills[0].clone();

    // 内容缺失 → Missing，同步按锁复现（本地源，不联网）
    store::remove_version("pdf", &entry.content_hash).unwrap();
    let view = agent_skills(&name).unwrap();
    assert_eq!(view.declared[0].status, AgentSkillStatus::Missing);
    let report = sync_agent_skills(&name, false).unwrap();
    assert_eq!(report.installed, vec!["pdf"]);
    assert!(report.repaired.is_empty());
    assert_eq!(
        agent_skills(&name).unwrap().declared[0].status,
        AgentSkillStatus::Ready
    );

    // 内容漂移：默认拒绝，repair 才重装
    let dir = store::entry_dir("pdf", &entry.content_hash).unwrap();
    fs::write(dir.join("SKILL.md"), "tampered").unwrap();
    let error = sync_agent_skills(&name, false).unwrap_err();
    assert!(error.contains("漂移"), "{error}");
    let report = sync_agent_skills(&name, true).unwrap();
    assert_eq!(report.repaired, vec!["pdf"]);
    assert!(
        verify_store_skills(None)
            .unwrap()
            .iter()
            .all(|row| row.status == SkillVerifyStatus::Ok)
    );

    // 缺锁 → Unresolved + 提示；同步从 store 索引补锁
    fs::remove_file(home.agent_dir(&name).join("agent.lock")).unwrap();
    let view = agent_skills(&name).unwrap();
    assert_eq!(view.declared[0].status, AgentSkillStatus::Unresolved);
    assert!(view.lock_error.as_deref().unwrap().contains("agent.lock"));
    let report = sync_agent_skills(&name, false).unwrap();
    assert_eq!(report.resolved, vec!["pdf"]);
    assert!(home.agent_dir(&name).join("agent.lock").is_file());
    assert_eq!(
        agent_skills(&name).unwrap().declared[0].status,
        AgentSkillStatus::Ready
    );
}

#[test]
fn install_reports_git_and_name_validation_errors() {
    let _lock = HOME_LOCK.lock().unwrap();
    let _home = TestHome::new("errors");
    // 非法技能名在动网络之前拒绝
    let error = install_skill("owner/repo", Some(&["PDF".to_string()]), false, false).unwrap_err();
    assert!(error.contains("形状非法"), "{error}");
    // 选择冲突
    let error = install_skill("owner/repo", Some(&["pdf".to_string()]), true, false).unwrap_err();
    assert!(error.contains("不能同时"), "{error}");
    // 不在 store 里的技能无法启用
    let name = format!("skill-agent-{}", pipi_protocol::new_id());
    agents::create_agent(&name, "", None, None, None, None).unwrap();
    let error = set_skill_enabled(&name, "pdf", true).unwrap_err();
    assert!(error.contains("尚未安装"), "{error}");
}
