//! agent.toml 契约的 schema 定义与校验。
//!
//! 原则：
//! - 未知键/未知段一律拒绝（fail-closed），字段名 kebab-case；
//! - TOML 只承载开关、指针与数值，绝不承载指令正文
//!   （AGENTS.md/SKILL.md/memory 仍是 Markdown，按需 read）；
//! - `AV_*` 是运行时保留命名空间，声明文件不得触碰。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 当前实现支持的 schema 版本。
pub const SUPPORTED_SCHEMA: u8 = 1;

/// 运行时保留命名空间前缀（嵌套检测等宿主变量）。
pub const RESERVED_PREFIX: &str = "AV_";

/// 项目层 agent.toml：只允许 `schema` / `[env]` / `[[requires]]` / `[resources]`。
///
/// Agent 身份与权限（model/permissions/mcpServers）仍由 agent.json 承载；
/// 未来 agent 层（av 家目录）的 superset 定义在后续版本扩展。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentToml {
    /// 契约版本，必须等于 [`SUPPORTED_SCHEMA`]。
    pub schema: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<EnvConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Vec<RequiresEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<Resources>,
}

/// `[env]` 段。字段全部 `Option`：缺席 = 不覆盖低层，而不是重置为默认 ——
/// 这是分层合并正确性的关键（否则 local 层 absence 会意外重置 agent 层的声明）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct EnvConfig {
    /// 继承策略：`all`（默认）| `core`（最小安全集）| `none`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherit: Option<Inherit>,
    /// 继承后删除的键（glob，如 `"AWS_*"`）；不作用于 `set`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore: Option<Vec<String>>,
    /// 覆盖/新增的静态变量。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<BTreeMap<String, String>>,
    /// 前置到 PATH 的目录（相对路径以契约文件目录为基准）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prepend: Option<Vec<String>>,
    /// 秘密引用：值永不内联，`{ env = "..." }` 透传或 `{ file = "..." }`（0600）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, SecretRef>>,
}

/// 继承策略。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Inherit {
    /// 全部继承进程环境（默认；本地优先，用户自己的机器）。
    #[default]
    All,
    /// 只继承最小安全集。
    Core,
    /// 不继承任何进程环境。
    None,
}

/// 秘密引用 —— 值只能间接取得，保证会话记录/日志永不包含明文。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub enum SecretRef {
    /// `{ env = "NAME" }`：从原始进程环境透传（绕过 inherit/ignore）。
    Env(String),
    /// `{ file = "PATH" }`：从仅属主可读的文件读取（去除尾部换行）。
    File(String),
}

/// `[[requires]]` 条目：工具链断言，只校验不安装。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RequiresEntry {
    pub command: String,
    /// 版本断言：`">=20"` / `"=20"` / `"20"`（按实际输出的首个数字段比较）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// `[resources]`：资源路径覆盖。只允许覆盖路径，绝不内联正文。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Resources {
    /// 三态：None = 按约定发现（AGENTS.override.md > AGENTS.md > CLAUDE.md…）；
    /// Some(list) = 完全替换发现，按声明顺序拼接；Some([]) = 显式禁用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Vec<String>>,
    /// 项目层字节预算（缺省 16KiB，对齐 codex 的 project_doc_max_bytes）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
    /// memory 目录覆盖（缺省 `memory/`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    /// 技能包过滤。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<SkillsFilter>,
}

/// `[resources.skills]`：sources 按层替换约定目录；only/exclude 按技能名
/// glob 过滤合并后的技能集，exclude 优先；use 声明启用全局 store 中的技能。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct SkillsFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<String>>,
    /// 声明启用全局 store（`~/.av/skills`）中的技能，按技能名引用。
    /// 三态：缺席 = 不启用；列表 = 启用这些技能；`[]` = 显式禁用全部 store 技能。
    /// 解析结果记录在同层 `agent.lock`（缺失即 fail-closed，见 `store` 模块）。
    #[serde(rename = "use", default, skip_serializing_if = "Option::is_none")]
    pub use_: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
}

impl AgentToml {
    /// fail-closed 校验：schema 版本 + env 段与 resources 段的命名空间与形状合法性。
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SUPPORTED_SCHEMA {
            return Err(format!(
                "不支持的 schema 版本 {}（当前实现支持 {}）",
                self.schema, SUPPORTED_SCHEMA
            ));
        }
        if let Some(env) = &self.env {
            env.validate()?;
        }
        if let Some(resources) = &self.resources {
            resources.validate()?;
        }
        Ok(())
    }
}

impl Resources {
    /// 资源段校验：当前覆盖 `skills.use` 的技能名形状与去重（fail-closed）。
    pub fn validate(&self) -> Result<(), String> {
        if let Some(skills) = &self.skills {
            skills.validate()?;
        }
        Ok(())
    }
}

impl SkillsFilter {
    /// 校验 `use` 中的技能名：形状合法且不重复。
    pub fn validate(&self) -> Result<(), String> {
        let Some(names) = &self.use_ else {
            return Ok(());
        };
        let mut seen = std::collections::HashSet::new();
        for name in names {
            if !crate::skills::valid_skill_name(name) {
                return Err(format!(
                    "use 中的技能名 {name:?} 形状非法（小写字母/数字/-，不以 - 开头结尾、无连续 --，≤64 字符）"
                ));
            }
            if !seen.insert(name.as_str()) {
                return Err(format!("use 中出现重复技能名：{name:?}"));
            }
        }
        Ok(())
    }
}

impl EnvConfig {
    /// 校验保留命名空间与环境变量名/值形状。
    pub fn validate(&self) -> Result<(), String> {
        if let Some(set) = &self.set {
            for key in set.keys() {
                reject_reserved(key)?;
            }
        }
        if let Some(secrets) = &self.secrets {
            for key in secrets.keys() {
                reject_reserved(key)?;
            }
        }
        if let Some(ignore) = &self.ignore {
            for pattern in ignore {
                validate_env_key(pattern)?;
                if pattern == "AV" || pattern.starts_with(RESERVED_PREFIX) {
                    return Err(format!("ignore 不能作用于保留命名空间：{pattern:?}"));
                }
            }
        }
        Ok(())
    }
}

fn reject_reserved(key: &str) -> Result<(), String> {
    validate_env_key(key)?;
    if key == "AV" || key.starts_with(RESERVED_PREFIX) {
        return Err(format!(
            "环境变量 {key:?} 属于运行时保留命名空间（{RESERVED_PREFIX}*），声明文件不得设置"
        ));
    }
    Ok(())
}

/// 环境变量名形状校验：非空、无 `=`/NUL/换行。
pub fn validate_env_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("环境变量名不能为空".into());
    }
    if key.contains('=') {
        return Err(format!("环境变量名不能包含 '='：{key:?}"));
    }
    if key.contains('\0') {
        return Err("环境变量名不能包含 NUL 字符".into());
    }
    if key.contains('\n') || key.contains('\r') {
        return Err(format!("环境变量名不能包含换行：{key:?}"));
    }
    Ok(())
}

/// 环境变量值形状校验：无 NUL。
pub fn validate_env_value(value: &str) -> Result<(), String> {
    if value.contains('\0') {
        return Err("环境变量值不能包含 NUL 字符".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
