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
crates/pipi-core/   Rust 核心（不依赖 Tauri）：agent_loop / tools / provider /
                    session / permissions / agents / truncate / types
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
| `cargo check --workspace` | 全量编译检查 |
| `cd src-tauri && cargo clippy` | Rust lint |
| `npx tsc --noEmit` | 前端类型检查 |

## 约定

- Rust 数据模型一律 `derive(Serialize, Deserialize)` 并 `#[serde(rename_all = "camelCase")]`，与前端类型对齐。
- 消息 / 会话的 JSON 字段名与 pi 保持一致（`role`、`toolResult`、`toolCall`、`parentId`……），移植改动不许悄悄改格式。
- 业务逻辑只进 `pipi-core`；`src-tauri` 是薄壳，不写逻辑。
- Agent 数据根目录是 `~/.pipi/agents/<name>/`，结构见 README；不要把 Agent 状态存到别处。
- 错误处理：Tauri command 返回 `Result<T, String>`，消息用用户可读的中文。
- 前端保持零 UI 框架依赖，手写样式；新增依赖需要充分理由。
- 目录骨架必须与 README「Agent 的组成」表格一致；改动时两边同步更新。
- 权限是安全边界：bash 命令检查在 `pipi-core/src/tools/bash.rs` 执行前发生，
  改权限逻辑（`permissions/`）必须带测试，且宁可拒绝不可放行 —— 无法静态
  分析的命令一律视为危险。
