## 目标

把 Agent 侧提示词素材的**路径权威从 Rust 常量搬到 `agent.toml`**：agent 层契约必须存在、且必须显式声明 `instructions` / `memory` / `skills.sources` 三项；`[resources].memory` 从死字段变成真正的消费方。`agent.toml` 保留在 Agent 根目录（与 `agent.json` 同层，不在 `harness/` 里）。未来再迁布局 = 改契约 + 改 `create_agent` 模板，不动核心逻辑。

## 已定的三条语义

1. **三项必须显式声明**：agent 层契约是必需文件；`[resources]` 或三项任一缺席 → fail-closed，报错指明缺哪项与建议写法。`[]` 表示「这一项不要」。
2. **memory 只 agent 层可声明**：项目层（含 `agent.local.toml`）出现 `[resources].memory` = 硬错误 —— 克隆来的仓库不能把 Agent 的记忆写入位置指到仓库里。
3. **策略仍在 `agent.json`**：`systemPrompt`（both/agent/workspace/none）选「哪一侧」，契约管「每项在哪」。两者正交，现有的工作台四选一不动。

## 不变量

- 声明**必需**，路径**存在时校验**：指令文件缺失 → 报错（沿用既有 fail-closed）；memory 目录声明了但尚未创建 → 按「空记忆」处理（写工具自己建目录）；技能来源目录缺失 → 跳过（沿用现状）。
- 所有声明路径 canonicalize 后必须落在 Agent 目录内（越界、`~`/绝对路径逃出、符号链接逃逸一律拒绝，报错说明包含根）—— 与「Agent 状态只存在 `~/.pipi/agents/<name>/`」一致。
- **项目侧保持三态**（缺席 = 约定发现），不会要求别人的仓库带契约 —— 本仓库自己就没有 agent.toml，这条不能动。

## 改动

### A. 契约层（`pipi-venv/src/av/`）
1. `Layer` 增加 `kind: LayerKind`（`Agent | Project | ProjectLocal`）：`discovery::load_layer` 与 `agents/contract.rs:20` 的 agent 层构造点、`av/test_support.rs` 的测试 helper 同步（共 3 处）。
2. `merge_layers` 增层规则：非 Agent 层声明 `[resources].memory` → 硬错误（文案指向「记忆目录属 Agent 定义，项目层不得声明」）。放在 merge 里可一次覆盖 prompt / tool_context / `av check` / 技能中心全部调用点。
3. `Resources::validate` 补 memory 的形状校验（非空、不含 `..` 段；其余路径语义交给解析器）。

### B. 单一解析入口（`agents/contract.rs`）
4. 新增 `pub fn agent_material(agent_layer: Option<&Layer>, agent_dir: &Path) -> Result<AgentMaterial, String>`：
   - `AgentMaterial { instruction_files: Vec<PathBuf>, memory_dir: PathBuf, skill_sources: Vec<PathBuf> }`（全部绝对路径）；
   - 三项缺任一 → fail-closed，错误里带具体路径与示例写法；
   - 指令文件复用现有 `load_declared_context_files` 的校验（regular file / containment / 非空），只是不含预算；skills 复用 `load_declared_sources` + `load_skill_sources` 的 containment（返回目录，供读取根使用）；memory 做 canonicalize（目录不存在时返回路径本身且标为未创建）。
5. 新增共享模板常量（供 `create_agent` 与 `skills/enable.rs` 的 `NEW_CONTRACT_SKELETON` 共用）：
   ```toml
   schema = 1

   [resources]
   instructions = ["harness/AGENTS.md"]
   memory = "harness/memory"

   [resources.skills]
   sources = ["harness/skills"]
   ```
   —— 这是 `harness/` 现在**唯一**出现的地方（默认值，不是权威）。

### C. 消费方切到契约
6. `prompt.rs`：agent 侧三项全部来自 `agent_material`（不再有 `harness_dir` 拼接、agent 侧不再走候选名扫描 —— `load_agent_context_files` 退场，项目侧保留）；`memory_index` 改收 `&Path`。
7. `tool_context.rs`：memory 目录与技能读取根都来自 `agent_material`；**补上 memory 的 containment 检查**（今天 memory 根完全没有校验，符号链接逃逸会被静默信任 —— 与 skills 不对称）。
8. `files.rs` 文件编辑器：允许的可编辑路径 = 声明的指令文件 ∪ memory 目录下的 `.md`（相对 Agent 目录的路径由契约解析结果反推，不再硬编码 `harness/`）；`list_agent_md_files` 返回 `(files, memory_prefix)`；内部改为加载 def + 契约层。
9. 删掉 `paths.rs` 的 `harness_dir/skills_dir/memory_dir` 与 `AgentDefinition::{memory_dir,skills_dir}`（默认布局只剩模板一处）。
10. 前端契约：宿主命令 `list_agent_files` 结果由 `string[]` 改为 `{ files: string[], memoryPrefix: string }`；`src/ipc.ts`、`AgentFileEditor.tsx`（默认打开第一个**指令文件**、新建记忆按 `memoryPrefix` 拼名）、`dev-mock.ts`、`tests/ipc-contract.test.ts`、`tests/fixtures/ipc-wire.json` 同步。

### D. 生成与存量迁移
11. `create_agent` 写 `agent.toml`（用共享模板，走 `av::store::write_text_atomic`）；`skills/enable.rs` 的骨架改用同一模板 —— 对没有契约的 Agent 启用技能时会补出完整契约而不是 `schema = 1`。
12. 存量 9 个 Agent 由我补写 agent.toml：8 个 both 的声明 `harness/{AGENTS.md,memory,skills}`；Pipi 声明 `memory = "harness/memory"` + `instructions = []` + `sources = []`，并补建 `harness/memory/`（它的 `systemPrompt: workspace` 决定这些不注入提示词，但 memory 工具照旧有根）。
13. `av check` 不必改（它跑在工作区，只看项目层）；`av` 指到 Agent 目录时自然走同一套校验。

### E. 测试
- av：`LayerKind` 传递、项目层 `memory` 硬错误、memory 形状校验、模板可解析。
- agents：`agent_material` 的 fail-closed 矩阵（缺契约 / 缺 `[resources]` / 缺任一项 / 指令文件缺失或逃逸 / memory 越界 / 符号链接），以及自定义位置生效（把 memory 指到 `notes/`，memory 工具、读取根、提示词索引、编辑器四处一起跟着变）。
- 策略矩阵保持不变（workspace/none 下 memory 工具仍指向契约位置）。
- create_agent 与 enable 模板一致；`list_agent_files` 新形状的前端契约测试。

### F. 文档
README：`[resources]` 示例加 `memory`，契约一节写明「agent 层必需三项、项目层不得声明 memory」，目录树与「Agent 的组成」三行改述为「由 agent.toml 声明（默认 harness/…）」；AGENTS.md：把「路径权威在 `paths.rs`」改成「权威在 agent.toml，`harness/` 只是模板默认值」，并记下三项必需与项目层禁令。

## 风险（接受，且是这次的目的）

- **破坏性**：任何缺 `agent.toml` 或漏声明的 Agent 会在会话启动 fail-closed。存量由我补齐、新建由 `create_agent` 生成；手写契约会踩到，但报错可操作。
- 项目层若已声明 `memory` 会被拒 —— 本机 `quant/agent.toml` 只有 env/requires/skills，不受影响。
- `list_agent_files` 是 wire 变更，前端与夹具同步改；旧前端连新宿主会读不出文件列表（本地单机应用，接受）。