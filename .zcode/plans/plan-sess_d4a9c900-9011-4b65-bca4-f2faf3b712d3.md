# 修复计划：协议「默认自动、可按提供商钉死」

## 目标与依据

`claude-haiku-5-5` 在 OpenCode Go 上只接受 Anthropic Messages（`/v1/messages`），models.dev 的 provider 级 npm 是 `@ai-sdk/openai-compatible`，模型条目却带 `provider.npm = @ai-sdk/anthropic`。Pipi 只读 provider 级协议 → 发到 `/chat/completions` → 400 `ModelProtocolUnsupported`（实测：glm-5.3 仅 chat、claude-haiku-5-5 仅 messages、qwen3.8-max 两者皆可，与上游标注逐条对应）。

新语义：**协议默认由系统管理（auto），用户可按提供商钉死**。三层设计：

1. **目录层**：尊重 models.dev 的**模型级**协议覆盖（同时按你选择：映射不到两种协议之一的模型从目录隐藏）；
2. **运行前解析**：每次 run 前算出有效协议并写进 `Model.api`——既有绑定（含你现在写死 openai-completions 的 agent 默认模型）零操作自动恢复，一次请求都不浪费；
3. **运行期协商（仅 auto）**：网关仍报「协议不支持」时，agent_loop 换另一种协议重试一次，并把学到的协议记进进程内 memo（此后所有请求含压缩摘要直接命中）。

---

## 0. 协议模式（新增，默认 auto）

- `crates/pipi-core/src/settings.rs:27` `ProviderConfig` 增 `apiMode: ApiMode`（`"auto" | "manual"`，`#[serde(default)]` → **旧 settings 一律 auto，无需迁移代码**）。现有 `api` 字段含义不变 =「条目声明的协议」：auto 时它是最后回退；manual 时它是钉死值。
- 解析顺序（**唯一实现点**在 pipi-provider，供宿主与适配器共用）：
  ```
  manual → entry.api（钉死：不查目录、不查 memo、不协商）
  auto   → memo(base_url, model_id)                 // 运行期实证学到，最高
         ?? 目录模型级覆盖                          // models.dev model.provider.npm
         ?? 目录 provider 级 api                    // 跟随上游/CURATION 更新
         ?? entry.api                               // 非目录端点（本地运行时、手填端点）
  ```
- 效果：预设新增与**所有既有**提供商条目天然是 auto；你在表单里显式选一个协议才变成钉死。

## 1. pipi-provider：目录带模型级协议

- `crates/pipi-provider/src/catalog/mod.rs:42` `CatalogModel` 增 `pub api: Option<Api>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`，旧缓存缺字段照样解析，无 schema 迁移）。
- `models_of`（:242）改 `models_of(provider, provider_api)`：读 `model.provider.npm` —— 映射到支持协议且 ≠ provider 级 → 写 override；**映射不到（zen 的 `gemini-*` → `@ai-sdk/google`）→ 跳过该模型**（fail-closed，与 `curation.rs:226` `protocol_of` 注释同口径，静默丢弃同 `tool_call=false`）。
- 新增 pub 入口（都放 catalog 模块，纯函数便于单测）：
  - `cached_catalog(cache_path) -> Option<ModelCatalog>`：`read_cache`（:143）的公开包装，**只读不联网**；
  - `model_api_override(&catalog, base_url, model_id, declared) -> Option<Api>`：端点归一化（去尾斜杠、忽略大小写）；同端点多条目录时优先 `api == declared` 的那条；
  - `catalog_provider_api(&catalog, base_url) -> Option<Api>`（顺序里的第三级）。
- 注释同步：`catalog/mod.rs:4-10` 模块头、`curation.rs:224-225`。
- 测试（内联 :507-953）：fixture 加「模型级 anthropic 覆盖」「不支持协议被跳过」；`wire_结构保持驼峰字段命名`（:832）扩展新字段；归一化/多条目用例。

## 2. pipi-core：run 前解析 + 模式生效

- `crates/pipi-core/src/runtime/execution.rs:36` `resolve_model`：
  - 返回 `(Model, api_key)` 扩成小结构体 `ResolvedModel { model, api_key, auto_protocol: bool }`（4 个调用点同步：:337 子 Agent、:570 主 run、compaction.rs:311、slots.rs:571；内联测试 :796 同步）；
  - provider 条目查找（:48-51）改为：先精确 `(api, baseUrl)` → 再**同 baseUrl 的 auto 条目** → 再任意同 baseUrl（同一端点可被声明成两种协议；deepseek 与 deepseek-anthropic 路径不同，不受影响）；
  - `api = auto ? 顺序解析 : entry.api`，写进返回的 `Model.api`；目录从 `pipi_venv::av::paths::catalog_cache_path()` 读（commands.rs:95-101 已有同款注入先例；读取一次/run，不联网）。
- **不重写 agent.json/settings**：只在请求时用对的协议（前端绑定语义见第 4 节）。
- 测试：`execution.rs:796` 既有风格（`HOME_LOCK` + `pipi_test_support::temp_dir` + 写最小目录缓存）断言 `claude-haiku-5-5` → `anthropic-messages`；补「auto 与 manual 两条路径」「按 baseUrl 兜底找 key」用例；settings 序列化往返（旧 json 无 `apiMode` → auto）。

## 3. 运行期协商（config 自愈）

- `crates/pipi-protocol/src/dto.rs:364` `StreamEvent::Error` 增 `protocol_mismatch: bool`（端口类型，**不过 wire**：StreamEvent 不序列化给前端，只有 agent_loop 消费）。
- `crates/pipi-provider/src/classify.rs`：新增 `protocol_mismatch(error) -> bool` —— 仅 400，优先读 rig 结构化 body code（`ModelProtocolUnsupported`），缺失退回文案 `does not support this protocol`；`StreamError`（lib.rs:57-77）带该标记，**分类仍是终态**（不设 RetryHint，不动既有重试语义）。
- `crates/pipi-provider/src/lib.rs` memo：`static LEARNED: Mutex<HashMap<(base_url, model_id), Api>>`；命中时记录；`stream_impl`（:665 一带）`let api = learned(...).unwrap_or(model.api)`，同时用于选 rig client（:679-713）与 `to_rig_messages`（:511，thinking issuer 必须一致）；暴露 `pub fn learned_protocol(...)` 供第 2 节。memo 只被 auto 路径写/读。
- `RigProvider` 去掉构造期缓存 api（:159-167）→ `provider_for()` 无参、`stream` 按 `model.api` 分派；改 5 处调用点（execution.rs:407/669、compaction.rs:117 + 3 个测试）。
- `crates/pipi-agent/src/agent_loop.rs:325`：循环内改用局部 `attempt_model`；仅当 `config.auto_protocol && protocol_mismatch && !prose_visible && !aborted && !switched` → 翻转 `attempt_model.api` 后 `continue`（**不 `failures += 1`、不发 RetryStart**，最多一次）；两次都失败时报错并注明「两种协议均已尝试」。既有不变量不动（中止不重试、已输出正文不重放、预算/退避不变）。manual 条目 `auto_protocol=false` → 行为与今天一致。
- 测试：classify 表驱动；agent_loop `ScriptedProvider`（:844）「先 mismatch 后成功」用例（断言第二次 `model.api` 已翻转、预算未增长）与「manual 不协商」用例；新集成 `crates/pipi-core/tests/protocol_negotiation.rs`（`/chat/completions` 回 400 `ModelProtocolUnsupported`、`/v1/messages` 回 SSE）断言 run 成功、JSONL 有 assistant 消息、**第二次 run 不再打 400**（memo 生效）。

## 4. 前端

- 只显示不存储：`src/catalog.ts:10` `CatalogModel` 增 `api?: ApiKind`；新增纯函数 `effectiveApiLabel(model, providerApi, apiMode)`（可 `node --test`）。
- 配置面：`src/settings/ProviderEditor.tsx:123` 协议下拉改为三选（**自动（推荐）/ Anthropic / OpenAI 兼容**）→ 写 `apiMode` + `api`；`src/settings/SettingsView.tsx:358` 徽章显示「自动」或钉死协议；预设 seed（`providerSeed`）带 `apiMode: "auto"`。
- 提示：`src/ModelPicker.tsx:71-80` hint 与 `ModelSelectModal`「已选 …」行（:219）显示「协议：Anthropic（按模型）」；若条目钉死且与目录模型级协议冲突，追加警告「此提供商已固定为 X；自动模式下会走 Y」。
- **明确不动** `ModelConfig.api` 语义（仍是条目声明的协议/提示值）→ 三处构造点（`ModelSelectModal.tsx:119`、`AgentWorkbench.tsx:135`、`CreateAgentForm.tsx:84`）与绑定反查（`AgentWorkbench.tsx:141-145`）都不用改，避免调研发现的「显示未绑定 + 保存把 provider 静默写回 null」坑。
- wire：settings 结构增可选字段 → 按 AGENTS.md 同步 `tests/fixtures/ipc-wire.json` 的 settings 样例（并跑 `npm run test:frontend` 的 ipc 契约测试）。

## 5. 文档

- `AGENTS.md`：「模型目录」条目补「模型级协议覆盖 + apiMode 语义（auto=目录/学习结果说了算；manual=钉死）」；「请求重试分两层」条目补例外——**协议协商只在 agent_loop、不算重试、不占预算、memo 记住**。
- `README.md:402-407` 模型目录段与 :164 全局设置段：协议来源与「自动/固定」说明。
- 非目标（写进文档）：不引入第三种协议；不做每模型级钉死；不主动发探测请求。

## 6. 验证

`cargo test -p pipi-provider`、`cargo test -p pipi-agent`、`cargo test -p pipi-core`（含新集成）、`npm run test:frontend`、`cargo check --workspace`、`cargo clippy --workspace --all-targets`、`npx tsc --noEmit`、`cargo xtask arch`。
手工验收：① 原样用你当前会话/默认模型（auto）发 ping → 成功，JSONL 无新 400；② 把该提供商改为「固定为 OpenAI 兼容」再发 → 复现同一条 400（证明钉死生效、不静默改写）；改回自动 → 恢复。

## 风险与边界（已核对）

- 目录缓存 24h TTL（catalog/mod.rs:35）：落地后首次刷新前覆盖信息不在缓存 → 由第 3 层兜底（代价：每进程每模型一次 400）。
- memo 只在进程内，重启后重学一次；不落盘、不改 settings。
- 压缩摘要不参与协商（其调用层不经过 agent_loop）：auto+目录已覆盖已知情况；未知网关下若本轮首请求就是压缩调用，可能先失败一次——列为已知边界，需要的话后续给压缩调用接同一协商。
- 翻转受既有不变量约束；翻转后同会话旧 thinking 块因 issuer 变化被 rig 丢弃（现有语义，不报错）。
- Anthropic 侧认证走 `x-api-key`（rig 行为）；若某网关只认 Bearer，第二次尝试也会失败，错误原样透出。
- 隐藏类模型（zen 的 `gemini-*` 等）不在目录/选择器出现，手填 ID 仍可用（运行会明确报协议错误，不静默）。
