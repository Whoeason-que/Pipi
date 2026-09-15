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
//! - [`requires`]：工具链断言，只校验不安装。

pub mod discovery;
pub mod merge;
pub mod requires;
pub mod resolve;
pub mod schema;

pub use discovery::{discover, find_project_root, load_contract_file, resolve_path, Discovered, Layer};
pub use merge::{merge_layers, Merged, MergedEnv};
pub use resolve::{collect_process_env, resolve_env, ResolvedEnv};
pub use requires::{check_requires, lookup_command, probe_version, version_satisfies};
pub use schema::AgentToml;
