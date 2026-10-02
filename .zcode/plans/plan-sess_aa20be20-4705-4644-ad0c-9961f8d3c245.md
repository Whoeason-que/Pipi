# av 增强：全局 skills store（uv 式安装）+ 职责下沉

## 一、目标

1. av 从「只读契约校验」升级为「契约 + 技能包管理」：skills 可像 pylib 一样安装到系统（`~/.av/skills/`），带精确修订与内容哈希。
2. 安装源支持 skills.sh 生态（公开搜索 API + `owner/repo` 短名）与本地目录；版本可复现靠 **`agent.lock` 锁文件**（uv.lock 式，声明与锁分离）。
3. 职责下沉：skills 发现/解析域 + `~/.pipi`/`~/.av` 路径布局移入 av，pipi-core 变薄消费者。

## 二、落盘布局（新增约定）

```
~/.av/                                  # av 家目录（AV_HOME 可覆盖，便于测试/迁移）
├── skills.lock                         # store 索引（TOML，version = 1）
└── skills/
    └── <skill-name>/
        └── <hash16>/                   # 不可变内容目录，名字 = 内容 sha256 前 16 位
            └── SKILL.md                # 版本并存：不同项目锁不同修订互不覆盖
```

项目/Agent 层契约旁：
```
<项目根>/agent.toml        # 声明： [resources.skills] use = ["pdf", "commit-helper"]
<项目根>/agent.lock        # 锁：每个 name 的 source / ref / revision / content-hash / skill-path
```
`agent.lock` 与声明它的 `agent.toml` 同目录（agent 层则是 `~/.pipi/agents/<name>/agent.lock`）；`use = []` 为显式禁用。

## 三、关键语义

- **安装 ≠ 启用**：`av skill install` 只进 store；契约里 `use` 声明才生效（你选的「全局共享，需声明启用」）。
- **会话启动不联网**：pipi-core 只做解析与校验——按「最高声明 `use` 的层」找到同目录 `agent.lock`，逐名解析到 `~/.av/skills/<name>/<hash16>/`；缺锁、缺条目、缺目录一律 fail-closed 并提示 `av skill sync`。数组沿用 av 既有合并规则（高层整体替换）。
- **加载顺序**：agent 约定目录 → 项目 `.pi/skills` → 声明的 store 技能（同名先发现者赢）；store 技能目录同时进 `ToolContext.read_roots`，`only/exclude` 过滤对它们同样生效。
- **内容完整性**：内容目录名 = 内容哈希前缀；`av skill verify` 重算全量 sha256 查漂移；安装走「暂存 → 同盘 rename」原子发布。
- **供应链约束**：拒绝符号链接、拒绝逃逸路径、跳过 `.git`、限额（单技能 ≤5 MiB / ≤500 文件）；`sync` 先打印将安装的来源清单，非 TTY 需 `--yes`。

## 四、CLI 命令面（仅 av CLI + 库，不做 App UI）

```
av check / env / doctor            # 保留；check 增加「声明技能是否已锁且已装」校验
av skill search <query> [--json]   # skills.sh GET /api/search?q=&limit=20（无鉴权）
av skill install <spec>            # 只装进 store；spec = owner/repo[@ref]、完整 git URL、./本地目录、~ 路径
av skill add <spec>                # install + 往同层 agent.toml 追加 use（toml_edit 保格式）+ 写 agent.lock
av skill sync                      # 由 use + agent.lock 物化 store（新克隆仓库可复现；冲突/缺失 fail-closed）
av skill list [--json] [--declared]
av skill update [<name>...] [--dry-run]
av skill remove <name> [--purge]   # 改契约+锁；--purge 同时清 store 版本
av skill verify [--declared]
```
多技能仓库默认报错并列出可选技能，`--skill <name>` 选择、`--all` 全装。解析流程：`git ls-remote` 定 ref→commit（已是 40 位 SHA 直接用）→ 浅取暂存 → `rev-parse HEAD` 复核 → 仓库内按生态约定（`skills/`、`skills/.curated`、`.agents/skills` 等，限深）发现 → 校验 frontmatter → 复制发布。

## 五、依赖（av Cargo.toml 新增，均有明确理由）

`sha2`（内容哈希）、`toml_edit`（保格式改用户契约）、`tempfile`（暂存目录）、`ureq`（搜索 API，**放 `search` feature，`default` 开启；pipi-core 以 `default-features = false` 依赖，避免把第二个 HTTP 栈链进应用**）。git 一律 spawn 系统 git（对齐 hermes 先例，`requires` 已能断言），不引入 git2。

## 六、文件级改动

**av 新增**：`src/paths.rs`（av_home/skills_store/skills_index + pipi_home/agents_root/agent_dir/cache_dir/settings_path，支持 `AV_HOME`/`PIPI_HOME`）；`src/skills.rs`（自 pipi-core 迁移：SkillMeta/frontmatter/发现/only-exclude，测试一并迁移，去掉对 `session::new_id` 的依赖）；`src/store.rs`（哈希、索引与 agent.lock 读写、`resolve_declared_skills`、发布/删除/校验）；`src/sources.rs`（spec 解析、git 解析浅取、仓库内发现、安装管线）；`src/net.rs` + `src/cli/`（拆分 main.rs，新增 skill 子命令组）。

**av 修改**：`Cargo.toml`（上述依赖 + feature）；`schema.rs`（`SkillsFilter.use` + 校验技能名形状、去重 fail-closed）；`merge.rs`（`use` 数组整体替换 + 测试）；`lib.rs`（导出）。

**pipi-core 修改**：`src/skills.rs` 变薄 re-export（`pub use av::skills::*`，调用点零改动）；`agents.rs` 的 `build_tool_context` / `build_system_prompt_with_tools` 接入「声明技能解析 + read_roots + fail-closed 文案」；`agents_dir()`、`settings.rs`、`catalog.rs` 改调 `av::paths`；`pipi-app/src/background.rs` 经 `agents::agent_dir` 间接复用。前端零改动。

**文档**：README「agent.toml 契约（av 标准）」重写（新增 store/lock/命令/`use`，与 uv、skills.sh 对照说明）、目录树加 `~/.av`；AGENTS.md 更新 av 职责描述与新增不变量（store 内容不可变、锁是解析权威、会话启动不联网安装）。

## 七、实施顺序（每步保持 `cargo check --workspace` 绿）

1. **paths 下沉**：av::paths + 环境变量覆盖 + 改 4 处调用点 + 测试（HOME/PIPI_HOME 隔离）。
2. **skills 迁移**：pipi-core/skills.rs → av::skills + re-export + 测试迁移。
3. **store + 契约**：哈希/索引/agent.lock/`use` 字段/合并语义 + 单测（含篡改、越界、版本并存）。
4. **安装管线 + CLI**：sources + git 流程 + `install/add/sync/list/update/remove/verify` + `check` 扩展；用本地裸仓库 fixture 做集成测试（不联网）。
5. **搜索**：skills.sh 客户端 + `av skill search`（单测解析层，联网 smoke 手动跑）。
6. **pipi-core 接线**：声明技能加载/read_roots/错误提示 + 契约测试（已装→注入提示词与 read_roots；缺锁/缺目录/修订不符→fail-closed）。
7. **文档 + 全量验证**。

## 八、验证口径

`cargo test -p av`（新增：paths/store/sources/schema/merge，及本地 git 安装端到端）、`cargo test -p pipi-core`、`cargo test -p pipi-app`、`cargo check --workspace`、`cd src-tauri && cargo clippy`、`npx tsc --noEmit`（应无变化）；手动 smoke：本地起一个 git 仓库装一个技能 → `agent.toml` 声明 → `av skill sync/verify` → 起会话确认技能出现在 `<available_skills>` 且 read 可读。

## 九、明确不做（本轮）

不做归档/直链下载（`https://skills.sh/p/<pack>` 会报错并提示改用 git 源）；不做会话启动自动安装（保持显式 `sync`）；不做 App 内 UI；不支持 submodule；不与 `~/.agents/skills` 生态目录互通（`AV_HOME` 可改但布局不同，不承诺）；不把 instructions/memory 的资源解析下沉（留下轮）。