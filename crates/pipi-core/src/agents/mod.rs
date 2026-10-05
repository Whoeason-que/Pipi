//! Agent 定义与注册表（Pipi 核心，pi 无对应物 —— pi 以 CLI 参数 + 全局
//! 配置承载这些信息，Pipi 把它升格为一等公民）。
//!
//! 一切皆文件：一个 Agent 就是 `~/.pipi/agents/<name>/` 下的一组文件。
//! 本模块只做两件事：从文件读取 Agent、把创建请求写成文件。
//!
//! 模块划分：`types` 清单结构与校验，`paths` 目录定位与名称安全，`registry`
//! 扫描/创建/读写，`archive` 归档与删除，`files` 可编辑 Markdown 与 memory
//! 索引，`contract` agent.toml 契约层与上下文文件，`tool_context` 工具执行
//! 上下文，`prompt` 系统提示组装；单元测试在各实现文件底部。

mod archive;
mod contract;
mod files;
mod paths;
mod prompt;
mod registry;
mod tool_context;
mod types;

pub use archive::{
    archive_agent, delete_agent, delete_archived_agent, list_archived_agents, restore_agent,
};
pub use contract::collect_contract_layers;
pub use files::{
    MemoryFileMeta, list_agent_md_files, memory_index, read_agent_file, write_agent_file,
};
pub use paths::{agent_dir, agents_dir, ensure_agent_dir, expand_tilde, validate_agent_name};
pub use prompt::{build_system_prompt, build_system_prompt_with_tools};
pub use registry::{
    add_bash_allowlist_entries, create_agent, create_agent_with_instructions, ensure_default_agent,
    list_agents, list_agents_bootstrapped, load_agent, save_agent,
};
pub use tool_context::build_tool_context;
pub use types::{AgentDefinition, DEFAULT_AGENT_NAME, McpServerConfig, validate_definition};
