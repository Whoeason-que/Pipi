# OS 沙箱

状态：**Linux 已落地（Landlock + seccomp），macOS 已实现 Seatbelt 路径，
Windows 暂无后端**。旧的用户态命令策略保留为回退路径，不再是安全边界。

## 不变量

1. **安全边界只有一个：内核。** 命令文本（`$()`、反引号、`eval`、管道喂给
   shell、编译出来的二进制）不参与安全判定；helper 在 `exec` 前把能力集合
   交给 Landlock/Seatbelt，之后整棵进程树继承。
2. **失败即拒绝。** helper 不存在、Landlock 不可用、规则构造失败、策略 JSON
   损坏，都可能让命令不执行；只有 `danger-full-access` 明确不挂沙箱。
3. **能力是参数化的。** 三档只是预设：`read-only` / `workspace-write` /
   `danger-full-access`，外加 `networkAccess` 与 `writableRoots` 两个轴。

## 档位映射

| 档位 | 文件系统 | 网络 | 说明 |
| --- | --- | --- | --- |
| `read-only` | 全盘可读/可执行，无写入（`/dev/null` 等基础设备除外） | 默认拒绝 | 可以跑 bash；写不出去就是写不出去 |
| `workspace-write` | 全盘可读/可执行；workspace + 临时目录可写 | 默认拒绝 | 旧的 `permissions.bash` 不再参与 |
| `danger-full-access` | 不限制 | 不限制 | 显式逃生门，不启动 helper |

`SandboxPolicy` 定义在 `pipi-protocol/src/permissions.rs`；策略由
`ToolContext::sandbox_policy()` 派生。`workspace-write` 默认包含
`std::env::temp_dir()`，因为大量工具链离不开 TMPDIR；这是可用性取舍。

## 进程模型

```
宿主 / Agent
  └─ pipi-sandbox --policy <json> -- bash -c "..."
       ├─ (Linux)  Landlock ruleset → seccomp → exec bash
       └─ (macOS)  exec /usr/bin/sandbox-exec -p <profile> bash -c "..."
```

**不要在主进程用 `pre_exec` 装沙箱**：fork 之后不能分配/读文件，策略解析和
规则构造都在 helper 自己的进程里完成。helper 失败以 125 退出，绝不退化成
无沙箱执行。

当前接入的 spawn 点：

- `pipi-tools/src/bash.rs`（Agent 的 bash 工具）
- `pipi-core/src/background/service.rs`（托管后台 shell；提交时的检查不等于
  执行时的边界，必须在真正 spawn 处包装）

不应接入的 spawn 点：`pipi-session` 的会话 git、`pipi-venv` 的 av/git 安装
——它们是应用内部行为，不是模型可控输入。

## 开关

| 环境变量 | 取值 | 作用 |
| --- | --- | --- |
| `PIPI_SANDBOX` | `off` / `auto`（默认） / `require` | 关闭 / 有 helper 就用、没有回退旧策略并警告 / 缺失即拒绝 |
| `PIPI_SANDBOX_BIN` | 路径 | 显式指定 helper；指向不存在的文件时按不可用处理，不再从 PATH 兜底 |
| `PIPI_SANDBOX_NETWORK` | `1` / `true` / `on` | 保留文件系统沙箱、放行网络（临时逃生阀） |
| `PIPI_SANDBOX_WRITABLE` | `split_paths` 分隔的绝对路径 | 追加可写根（如工具链缓存目录；临时逃生阀） |

helper 查找顺序：`PIPI_SANDBOX_BIN` → 当前可执行文件同级 / 上一级 /
`../Resources` → `PATH`。开发时 `target/debug/pipi` 与
`target/debug/pipi-sandbox` 同级，直接可用。

## 发布打包

`pipi-sandbox` 是独立二进制，需要作为 sidecar 与主程序一起分发：

- Linux/Windows：放在主程序同目录；
- macOS `.app`：放 `Contents/MacOS/` 或 `Contents/Resources/`（两者都在查找范围内），
  并纳入签名/notarization；
- `cargo build --workspace` 会构建 helper；`npm run tauri:dev` / `tauri:build`
  先 `cargo build -p pipi-sandbox` 再用 `scripts/prepare-sidecar.mjs` 复制成
  `externalBin` 期望的 `<name>-<target-triple>` 名字（`tauri.conf.json` 已配置；
  当前脚本按 host triple 复制，universal macOS 构建需要另外合并两份 sidecar）。

## 已知边界（不是实现遗漏）

- **Landlock 不中介元数据操作**：`chmod` / `chown` / `utime` 不在访问权集合里。
  `workspace-write` 保护内容与目录结构，不保护权限位；要连路径都看不见请上
  namespace/容器。
- **授权范围内仍可破坏**：删自己的工作区、把工作区内容发到已放行的网络、
  在仓库里写 `.git/hooks` 供日后沙箱外执行——这些都不在沙箱职责内。后续应把
  `.git/hooks` 等提权子路径加入只读子路径，并把网络授权做成按 Agent/按次审批。
- **Windows 没有后端**：`auto` 回退旧权限层，`require` 失败。
- **老内核**：Landlock 需要 5.13+ 且 LSM 包含 `landlock`；helper 使用
  `CompatLevel::HardRequirement` + ABI V3（含 truncate），无法满足时不会
  悄悄降级。
- **网络过滤基于 seccomp `socket` 的地址族参数**：挡 `AF_INET`/`AF_INET6`，
  保留 `AF_UNIX`；继承来的已打开网络 fd 不在覆盖范围（正常 spawn 不传递）。
  保留 `AF_UNIX` 的代价是本地解析器 / 代理（systemd-resolved、nscd、Unix
  socket 代理）仍可能代发请求；需要域名级控制时应在沙箱外再叠网络代理。

## 验证

```bash
cargo test -p pipi-sandbox          # 内核黑盒：写越界 / read-only / 网络 / require
cargo test -p pipi-tools            # 门控：沙箱生效时旧策略让位
cargo xtask arch                    # 分层（pipi-sandbox 只依赖契约层）
```

手工验证：

```bash
cargo build -p pipi-sandbox
./target/debug/pipi-sandbox \
  --policy '{"mode":"workspace-write","networkAccess":false,"writableRoots":["/tmp/ws"]}' \
  -- bash -c 'echo ok > /tmp/ws/a; echo bad > /tmp/outside'
# 第二条写入应得到 Permission denied，helper 退出码非零
```

## 后续

1. macOS 签名/notarization 与三平台 CI（外部 helper 的签名与 ABI 冒烟）。
   发布打包已接 `externalBin`（`scripts/prepare-sidecar.mjs`）。
2. 把审批从「批准命令字符串」改成「授予能力」：`+writable_root` / `+network`，
   按次 / 按会话 / 写回 `agent.json`。
3. `writable_roots` 的只读子路径（`.git/hooks`、`.git/config`、`agent.json`）。
4. 删除 `permissions/{parse,safety,redirect,rm}.rs` 与 `BashMode`（先保持回退
   一个发布周期）。
5. 跨平台契约测试：Linux 内核 ABI 探测、macOS Seatbelt profile 冒烟。
