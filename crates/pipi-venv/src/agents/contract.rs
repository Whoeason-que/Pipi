use std::fs;
use std::path::{Path, PathBuf};

use super::types::AgentDefinition;

/// 新建 Agent 时的默认契约。
///
/// 三项素材位置**必须显式声明**（`[]` = 这一项不要）—— 权威在契约里，这个
/// 常量只是 Pipi 写下的默认值，也是 `harness/` 唯一出现的地方。未来换布局 =
/// 改这里 + 改存量契约，不动组装逻辑。
pub const DEFAULT_AGENT_CONTRACT: &str = "\
schema = 1

[resources]
instructions = [\"harness/AGENTS.md\"]
memory = \"harness/memory\"

[resources.skills]
sources = [\"harness/skills\"]
";

/// 默认模板里的素材目录名（新建 Agent 的骨架按它创建；模板文本与它由测试钉住）。
pub const DEFAULT_HARNESS_DIR: &str = "harness";

/// 声明的一条指令文件：真实路径 + 已读取的正文。
#[derive(Debug, Clone)]
pub struct AgentInstruction {
    /// 文件的真实路径（canonical）；UI 编辑器据此反推可编辑的相对路径。
    pub path: PathBuf,
    /// 正文（UTF-8、BOM 已去；空文件被拒绝）。
    pub content: String,
}

/// Agent 侧提示词素材：全部由 agent.toml 编排（三项必需）。
#[derive(Debug, Clone)]
pub struct AgentMaterial {
    /// 声明的指令文件（按声明顺序；已校验存在、常规文件、在 Agent 目录内）。
    pub instructions: Vec<AgentInstruction>,
    /// 记忆目录（绝对路径）。声明了但尚未创建时保留路径本身，按「空记忆」处理。
    pub memory_dir: PathBuf,
    /// 技能来源目录（绝对路径，已校验在 Agent 目录内；不存在时跳过）。
    pub skill_sources: Vec<PathBuf>,
}

/// 解析 Agent 侧提示词素材（`[resources]` 三项）。
///
/// fail-closed：agent.toml 缺席、`[resources]` 缺席、三项任一缺席、声明的路径
/// 逃出 Agent 目录，都直接报错并说明怎么补。指令文件必须已存在；记忆目录允许
/// 尚未创建（写入方会建）；技能来源目录不存在时跳过。
pub fn agent_material(
    agent_layer: Option<&crate::av::Layer>,
    agent_dir: &Path,
) -> Result<AgentMaterial, String> {
    let agent_dir =
        fs::canonicalize(agent_dir).map_err(|e| format!("无法规范化 Agent 目录：{e}"))?;
    let layer = agent_layer.ok_or_else(|| {
        format!(
            "Agent 缺少 agent.toml（{}）：提示词素材与记忆目录由它编排，补上 [resources] instructions / memory 与 [resources.skills].sources",
            agent_dir.join("agent.toml").display()
        )
    })?;
    let base = layer_path_dir(layer);
    let resources = layer.config.resources.as_ref().ok_or_else(|| {
        format!(
            "{} 缺少 [resources] 段：Agent 侧素材由它编排（instructions / memory / [resources.skills].sources 三项都要声明）",
            layer.label
        )
    })?;

    // —— instructions：指令文件列表（`[]` = 不要）——
    let instructions = resources.instructions.as_ref().ok_or_else(|| {
        format!(
            "{} 的 [resources] 缺少 instructions（指令文件列表，`[]` 表示不要）：例如 instructions = [\"harness/AGENTS.md\"]",
            layer.label
        )
    })?;
    let mut loaded_instructions = Vec::new();
    for raw in instructions {
        let resolved = crate::av::resolve_path(&base, raw);
        let canonical = fs::canonicalize(&resolved)
            .map_err(|e| format!("上下文文件不可用 {}: {e}", resolved.display()))?;
        if !canonical.is_file() {
            return Err(format!("上下文文件不是常规文件：{}", canonical.display()));
        }
        if !canonical.starts_with(&agent_dir) {
            return Err(format!(
                "上下文文件 {} 逃逸 Agent 目录 {}（fail-closed）",
                canonical.display(),
                agent_dir.display()
            ));
        }
        let content = fs::read_to_string(&canonical)
            .map_err(|e| format!("无法读取 {}: {e}", canonical.display()))?;
        let content = content
            .strip_prefix('\u{feff}')
            .unwrap_or(&content)
            .to_string();
        if content.trim().is_empty() {
            return Err(format!("上下文文件为空：{}", canonical.display()));
        }
        loaded_instructions.push(AgentInstruction {
            path: canonical,
            content,
        });
    }

    // —— memory：单个目录 ——
    let raw_memory = resources.memory.as_ref().ok_or_else(|| {
        format!(
            "{} 的 [resources] 缺少 memory（记忆目录）：例如 memory = \"harness/memory\"",
            layer.label
        )
    })?;
    let resolved_memory = crate::av::resolve_path(&base, raw_memory);
    let memory_dir = match fs::canonicalize(&resolved_memory) {
        Ok(canonical) => {
            if !canonical.is_dir() {
                return Err(format!("记忆目录不是目录：{}", canonical.display()));
            }
            if !canonical.starts_with(&agent_dir) {
                return Err(format!(
                    "[resources].memory 的 {} 逃逸 Agent 目录 {}（fail-closed）",
                    canonical.display(),
                    agent_dir.display()
                ));
            }
            canonical
        }
        // 尚未创建：按空记忆处理，但最近的已存在祖先也不能逃出 Agent 目录
        Err(_) => {
            if let Some(ancestor) = canonicalize_existing_ancestor(&resolved_memory)
                && !ancestor.starts_with(&agent_dir)
            {
                return Err(format!(
                    "[resources].memory 的 {} 逃逸 Agent 目录 {}（fail-closed）",
                    resolved_memory.display(),
                    agent_dir.display()
                ));
            }
            resolved_memory
        }
    };

    // —— skills.sources：技能来源目录列表（`[]` = 不要）——
    let sources = resources
        .skills
        .as_ref()
        .and_then(|skills| skills.sources.as_ref())
        .ok_or_else(|| {
            format!(
                "{} 的 [resources.skills] 缺少 sources（技能来源目录列表，`[]` 表示不要）：例如 sources = [\"harness/skills\"]",
                layer.label
            )
        })?;
    let mut skill_sources = Vec::new();
    for raw in sources {
        let resolved = crate::av::resolve_path(&base, raw);
        let Ok(canonical) = fs::canonicalize(&resolved) else {
            continue;
        };
        if !canonical.starts_with(&agent_dir) {
            return Err(format!(
                "[resources.skills].sources 的 {} 逃逸 Agent 目录 {}（fail-closed）",
                canonical.display(),
                agent_dir.display()
            ));
        }
        skill_sources.push(canonical);
    }

    Ok(AgentMaterial {
        instructions: loaded_instructions,
        memory_dir,
        skill_sources,
    })
}

/// 最近的已存在祖先（`canonicalize` 出来的真实路径）。
fn canonicalize_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut cursor = Some(path);
    while let Some(current) = cursor {
        if let Ok(canonical) = fs::canonicalize(current) {
            return Some(canonical);
        }
        cursor = current.parent();
    }
    None
}

/// Agent 的契约层集合（av `agent.toml`）：Agent 定义目录的 agent.toml
/// （agent 层，可能缺席）+ 项目层发现（agent.toml → agent.local.toml）。
/// env 合并、`[resources]` 三态共用同一份层集合（技能中心也用它取 `use` 声明）。
pub fn collect_contract_layers(
    def: &AgentDefinition,
    workspace: Option<&Path>,
) -> Result<(Option<crate::av::Layer>, Vec<crate::av::Layer>), String> {
    let mut agent_layer = None;
    if let Some(dir) = def.dir() {
        let path = dir.join("agent.toml");
        if path.is_file() {
            let canonical =
                fs::canonicalize(&path).map_err(|e| format!("无法读取 agent.toml：{e}"))?;
            let config = crate::av::load_contract_file(&canonical)?;
            agent_layer = Some(crate::av::Layer {
                kind: crate::av::LayerKind::Agent,
                label: "agent/agent.toml".into(),
                path: canonical,
                config,
            });
        }
    }
    let project_layers = match workspace {
        Some(workspace) => crate::av::discover(workspace)?.layers,
        None => Vec::new(),
    };
    Ok((agent_layer, project_layers))
}

pub(super) fn layer_path_dir(layer: &crate::av::Layer) -> PathBuf {
    layer
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// 声明式技能源路径解析（相对基准 = 声明它的契约文件所在目录）。
pub(super) fn resolve_declared_sources(raw: &[String], base_dir: &Path) -> Vec<PathBuf> {
    raw.iter()
        .map(|raw| crate::av::resolve_path(base_dir, raw))
        .collect()
}

/// 加载声明式上下文文件（`[resources].instructions` 的 Some(list) 分支）。
///
/// fail-closed：缺失、非常规文件、逃逸包含根、内容为空均报错；项目层预算
/// 沿用发现路径的同一语义（截断 + 预算耗尽即止）。
pub(super) fn load_declared_context_files(
    raw_paths: &[String],
    base_dir: &Path,
    containment_root: &Path,
    project_budget: Option<usize>,
) -> Result<Vec<crate::prompt::ContextFile>, String> {
    let mut files = Vec::new();
    let mut remaining = project_budget.unwrap_or(usize::MAX);
    for raw in raw_paths {
        if remaining == 0 {
            break;
        }
        let resolved = crate::av::resolve_path(base_dir, raw);
        let canonical = fs::canonicalize(&resolved)
            .map_err(|e| format!("上下文文件不可用 {}: {e}", resolved.display()))?;
        if !canonical.is_file() {
            return Err(format!("上下文文件不是常规文件：{}", canonical.display()));
        }
        if !canonical.starts_with(containment_root) {
            return Err(format!(
                "上下文文件 {} 逃逸包含根 {}（fail-closed）",
                canonical.display(),
                containment_root.display()
            ));
        }
        let content = fs::read_to_string(&canonical)
            .map_err(|e| format!("无法读取 {}: {e}", canonical.display()))?;
        let content = content
            .strip_prefix('\u{feff}')
            .unwrap_or(&content)
            .to_string();
        if content.trim().is_empty() {
            return Err(format!("上下文文件为空：{}", canonical.display()));
        }
        let content = if project_budget.is_some() {
            let truncated =
                crate::prompt::resources::truncate_to_char_boundary(&content, remaining);
            remaining = remaining.saturating_sub(truncated.len());
            truncated
        } else {
            content
        };
        files.push(crate::prompt::ContextFile {
            path: resolved.display().to_string(),
            content,
        });
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{agent_dir, create_agent};
    use pipi_test_support::HOME_LOCK;

    /// 默认模板必须合法，且声明的三项与 `DEFAULT_HARNESS_DIR` 一致
    /// —— `harness/` 只在这个模板里出现。
    #[test]
    fn default_template_declares_the_default_harness_layout() {
        let config: crate::av::AgentToml =
            toml::from_str(DEFAULT_AGENT_CONTRACT).expect("模板必须是合法 TOML");
        config.validate().expect("模板必须通过 schema 校验");
        let resources = config.resources.expect("模板必须声明 [resources]");
        let harness = DEFAULT_HARNESS_DIR;
        assert_eq!(
            resources.instructions,
            Some(vec![format!("{harness}/AGENTS.md")])
        );
        assert_eq!(resources.memory, Some(format!("{harness}/memory")));
        assert_eq!(
            resources.skills.and_then(|skills| skills.sources),
            Some(vec![format!("{harness}/skills")])
        );
    }

    /// 三项声明驱动全部消费方：缺契约/缺段/缺项/路径逃逸一律 fail-closed，
    /// 改声明即改位置（含「声明了但目录还没建」的宽容与 `[]` 的显式禁用）。
    #[test]
    fn agent_material_fails_closed_and_follows_declarations() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = pipi_test_support::temp_dir("pipi-agent-material");
        let previous_home = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);

        let name = format!("material-{}", pipi_protocol::new_id());
        let def = create_agent(&name, "", None, None, None, None).unwrap();
        let dir = agent_dir(&name).unwrap();
        let contract = dir.join("agent.toml");
        let material_now = || {
            let (agent_layer, _) = collect_contract_layers(&def, None).unwrap();
            agent_material(agent_layer.as_ref(), &dir)
        };

        // 默认模板：三项齐备
        let material = material_now().unwrap();
        assert_eq!(material.instructions.len(), 1);
        assert!(
            material.instructions[0].path.ends_with("harness/AGENTS.md"),
            "{:?}",
            material.instructions[0].path
        );
        assert!(material.memory_dir.ends_with("harness/memory"));
        assert_eq!(material.skill_sources.len(), 1);
        assert!(material.instructions[0].content.contains(&name));

        // 缺契约
        let error = agent_material(None, &dir).unwrap_err();
        assert!(error.contains("agent.toml"), "{error}");

        // 缺 [resources] 段
        fs::write(&contract, "schema = 1\n").unwrap();
        let error = material_now().unwrap_err();
        assert!(error.contains("[resources]"), "{error}");

        // 缺 instructions / memory / sources（逐个）
        for (text, key) in [
            (
                "schema = 1\n[resources]\nmemory = \"harness/memory\"\n[resources.skills]\nsources = []\n",
                "instructions",
            ),
            (
                "schema = 1\n[resources]\ninstructions = [\"harness/AGENTS.md\"]\n[resources.skills]\nsources = []\n",
                "memory",
            ),
            (
                "schema = 1\n[resources]\ninstructions = [\"harness/AGENTS.md\"]\nmemory = \"harness/memory\"\n",
                "sources",
            ),
        ] {
            fs::write(&contract, text).unwrap();
            let error = material_now().unwrap_err();
            assert!(error.contains(key), "缺 {key} 的报错应点名：{error}");
        }

        // 声明了但尚未创建的记忆目录：按空记忆处理
        fs::write(
            &contract,
            "schema = 1\n[resources]\ninstructions = [\"harness/AGENTS.md\"]\nmemory = \"notes/mem\"\n[resources.skills]\nsources = []\n",
        )
        .unwrap();
        let material = material_now().unwrap();
        assert!(
            material.memory_dir.ends_with("notes/mem"),
            "{:?}",
            material.memory_dir
        );
        assert!(!material.memory_dir.exists());
        assert!(material.skill_sources.is_empty());

        // 显式禁用两侧：没有指令、没有技能来源
        fs::write(
            &contract,
            "schema = 1\n[resources]\ninstructions = []\nmemory = \"harness/memory\"\n[resources.skills]\nsources = []\n",
        )
        .unwrap();
        let material = material_now().unwrap();
        assert!(material.instructions.is_empty());
        assert!(material.skill_sources.is_empty());

        // 声明的指令文件缺失 → fail-closed
        fs::write(
            &contract,
            "schema = 1\n[resources]\ninstructions = [\"harness/GONE.md\"]\nmemory = \"harness/memory\"\n[resources.skills]\nsources = []\n",
        )
        .unwrap();
        let error = material_now().unwrap_err();
        assert!(error.contains("上下文文件不可用"), "{error}");

        // 逃逸包含根（指令文件与记忆目录）→ fail-closed
        // 放在 `<agents>/` 下：`../outside.md` 能解析到它，但落在 Agent 目录之外
        let outside = dir.parent().unwrap().join("outside.md");
        fs::write(&outside, "outside").unwrap();
        fs::write(
            &contract,
            "schema = 1\n[resources]\ninstructions = [\"../outside.md\"]\nmemory = \"harness/memory\"\n[resources.skills]\nsources = []\n",
        )
        .unwrap();
        let error = material_now().unwrap_err();
        assert!(error.contains("逃逸"), "{error}");

        // 绝对/`~` 之类的越界路径（形状合法，但落在 Agent 目录外）
        fs::write(
            &contract,
            "schema = 1\n[resources]\ninstructions = []\nmemory = \"~\"\n[resources.skills]\nsources = []\n",
        )
        .unwrap();
        let error = material_now().unwrap_err();
        assert!(error.contains("逃逸"), "{error}");

        pipi_test_support::set_env_var("HOME", previous_home);
        let _ = fs::remove_dir_all(&home);
    }

    /// 契约里的路径形状在 schema 层就挡住明显错误（空值、含 `..`）。
    #[test]
    fn resources_memory_shape_is_validated() {
        for text in [
            "schema = 1\n[resources]\nmemory = \"\"\n",
            "schema = 1\n[resources]\nmemory = \"a/../b\"\n",
        ] {
            let config: crate::av::AgentToml = toml::from_str(text).unwrap();
            let error = config.validate().unwrap_err();
            assert!(error.contains("memory"), "{error}");
        }
    }

    /// 记忆目录是指向 Agent 目录之外的符号链接 → fail-closed（与技能来源同一条）。
    #[cfg(unix)]
    #[test]
    fn memory_dir_symlink_escape_fails_closed() {
        let _guard = HOME_LOCK.lock().unwrap();
        let home = pipi_test_support::temp_dir("pipi-agent-memory-link");
        let previous_home = std::env::var("HOME").unwrap();
        pipi_test_support::set_env_var("HOME", &home);

        let name = format!("memory-link-{}", pipi_protocol::new_id());
        let def = create_agent(&name, "", None, None, None, None).unwrap();
        let dir = agent_dir(&name).unwrap();

        // 把声明的记忆目录换成一个指向 Agent 目录之外的符号链接
        let external = pipi_test_support::temp_dir("pipi-agent-memory-external");
        fs::remove_dir_all(dir.join(DEFAULT_HARNESS_DIR).join("memory")).unwrap();
        std::os::unix::fs::symlink(&external, dir.join(DEFAULT_HARNESS_DIR).join("memory"))
            .unwrap();

        let (agent_layer, _) = collect_contract_layers(&def, None).unwrap();
        let error = agent_material(agent_layer.as_ref(), &dir).unwrap_err();
        assert!(error.contains("memory"), "{error}");
        assert!(error.contains("逃逸"), "{error}");

        pipi_test_support::set_env_var("HOME", previous_home);
        let _ = fs::remove_dir_all(&home);
        let _ = fs::remove_dir_all(&external);
    }
}
