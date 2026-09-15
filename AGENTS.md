# AGENTS.md

给在此仓库工作的 AI 编码代理（以及人类协作者）的说明。

## 项目是什么

Pipi —— 一个以 **Agent（而非会话）** 为中心的本地优先桌面应用，Tauri 2.0 架构：
React + TypeScript 前端负责渲染，Rust 核心负责 Agent 循环、工具执行与文件存储。

**README.md 是设计北极星。** 提案任何新功能前，先对照其中的设计哲学：

1. Agent 是一等公民，会话只是副产品
2. 一切皆文件（不引入数据库、不引入私有格式）
3. 小核心，组合优于配置
4. 本地优先（无账号、无遥测）

## 代码结构

```
crates/av/          agent.toml 环境契约（独立工具）：schema / 发现 / 合并 /
                    env 解析 / requires；lib 供 pipi-core 复用，bin 为调试 CLI
crates/pipi-core/   Rust 核心（不依赖 Tauri）：agent_loop / tools / provider（rig 适配层）/
                    session / permissions / context / skills / stats / project_doc /
                    settings / agents / catalog（模型目录，models.dev）/ truncate / types
src-tauri/          Tauri 薄壳：commands.rs 只做 IPC 转发，不含业务逻辑
src/                React + TypeScript 前端
```

核心从 [pi](https://github.com/earendil-works/pi) 移植而来，模块映射与「有意
不移植清单」见 README「与 pi 的关系」一节。改核心逻辑前先看上游对应实现。
命令安全（`permissions/safety.rs`）与沙箱模式移植自 openai/codex，
映射见 README「与 codex 的关系」。

## 常用命令

| 命令 | 说明 |
| --- | --- |
| `npm install` | 安装前端依赖 |
| `npm run tauri dev` | 启动开发模式（前端 + 桌面壳） |
| `npm run tauri build` | 打包 |
| `cargo test -p pipi-core` | 核心单元测试（改核心必跑） |
| `cargo test -p av` | agent.toml 契约测试（改 av 必跑） |
| `cargo check --workspace` | 全量编译检查 |
| `cd src-tauri && cargo clippy` | Rust lint |
| `npx tsc --noEmit` | 前端类型检查 |

## 约定

- Rust 数据模型一律 `derive(Serialize, Deserialize)` 并 `#[serde(rename_all = "camelCase")]`，与前端类型对齐。
- 消息 / 会话的 JSON 字段名与 pi 保持一致（`role`、`toolResult`、`toolCall`、`parentId`……），移植改动不许悄悄改格式。
- 业务逻辑只进 `pipi-core`；`src-tauri` 是薄壳，不写逻辑。
- Agent 数据根目录是 `~/.pipi/agents/<name>/`，结构见 README；不要把 Agent 状态存到别处。
- 错误处理：Tauri command 返回 `Result<T, String>`，消息用用户可读的中文。
- 前端依赖走白名单，样式仍然手写；新增依赖需要充分理由。已批准的依赖：
  - react-markdown + remark-gfm + rehype-highlight（agent 输出的 Markdown 渲染，
    见 src/Markdown.tsx；不启用 rehype-raw，模型输出不可信）；
  - react-select（模型/供应商选择器的搜索与键盘导航，见 src/Select.tsx）——统一以
    `unstyled` + 自有 `.pipi-select__*` 样式使用，不让第三方样式体系渗进来。
- 模型目录（提供商与模型清单）**不再手写或生成**：运行时从 models.dev 拉取并缓存到
  `~/.pipi/cache/models.json`，实现见 `crates/pipi-core/src/catalog.rs`。上游给的是
  AI SDK 语义的 baseUrl，未经 `CURATION` 表核实不得直接当 Pipi 的 baseUrl 使用。
- 目录骨架必须与 README「Agent 的组成」表格一致；改动时两边同步更新。
- provider 协议层用 rig（`rig` crate，依赖重命名自 rig-core）：新增 provider
  能力优先看 rig 是否已支持，不要回退到手写 SSE；映射偏差记录在 provider.rs。
- 统计口径（stats.rs，移植自 hermes）：滚动窗口 N=10、命中率 = cache_read/prompt、
  数据不足时省略而非显示 0 —— 改统计先对齐这三个语义。
- 权限是安全边界：bash 命令检查在 `pipi-core/src/tools/bash.rs` 执行前发生，
  改权限逻辑（`permissions/`）必须带测试，且宁可拒绝不可放行 —— 无法静态
  分析的命令一律视为危险。
- agent.toml 契约（`crates/av`，README「agent.toml 契约（av 标准）」一节）：
  未知键/未知段一律拒绝、秘密值只许引用式（永不内联）、`AV_*` 是运行时
  保留命名空间、项目层文件不得声明身份与权限段 —— 改 schema/合并/解析
  逻辑必须带测试，宁可拒绝不可放行；工具子进程环境一律取
  `ToolContext.resolved_env`（会话启动解析一次），不许在 spawn 点读进程环境。
