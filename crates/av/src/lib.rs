//! av —— Agent 运行时环境契约（agent.toml）。
//!
//! 独立 crate：标准本体是 `agent.toml`，解析/发现/合并/校验全部纯函数化，
//! 供任何 agent 宿主（pipi-core、CLI 调试）消费同一份实现。
//!
//! 模块分工：
//! - [`schema`]：agent.toml 结构 + fail-closed 校验（未知键拒绝、保留命名空间）；
//! - [`discovery`]：cwd 向上发现最近 `agent.toml` + `agent.local.toml` 本地层；
//! - [`merge`]：分层合并（标量覆盖、映射按键合并、数组整体替换）；
//! - [`resolve`]：最终子进程环境解析（inherit/ignore/set/path-prepend/secrets/AV_*）；
//! - [`requires`]：工具链断言，只校验不安装；
//! - [`skills`]：SKILL.md 的发现、解析与过滤（供宿主渲染技能索引）；
//! - [`sources`]：安装源解析（skills.sh 短名 / git URL / 本地目录）与浅取；
//! - [`install`]：安装、同步与更新的管线；
//! - [`store`]：全局技能 store（`~/.av/skills`）——安装、锁文件与解析；
//! - [`paths`]：av 家目录与 Pipi 数据根的路径布局（唯一拼装点）；
//! - `net`：skills.sh 搜索客户端（`search` feature，默认开启）。

pub mod discovery;
pub mod install;
pub mod merge;
#[cfg(feature = "search")]
pub mod net;
pub mod paths;
pub mod requires;
pub mod resolve;
pub mod schema;
pub mod skills;
pub mod sources;
pub mod store;

pub use discovery::{
    Discovered, Layer, discover, find_project_root, load_contract_file, resolve_path,
};
pub use merge::{Merged, MergedEnv, merge_layers};
pub use requires::{check_requires, lookup_command, probe_version, version_satisfies};
pub use resolve::{ResolvedEnv, collect_process_env, resolve_env};
pub use schema::AgentToml;
pub use skills::{SkillMeta, filter_skills_by_name, load_skill_sources, parse_frontmatter};
