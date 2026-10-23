# File、Git 与会话文件

File、Git 在应用现有左侧图标栏中各有独立入口，共用既有侧栏/手机抽屉，不新增平行
导航列。顶部与侧栏明确显示当前 Agent 工作区，无会话也能使用。

## 文件来源

- **工作区**：从 Agent 清单解析真实工作目录，按需展开目录树，筛选已展开的文件。
  可显示隐藏文件，也可输入工作区相对路径或工作区内的绝对路径打开文件。
- **会话文件**：从已加载的 read/write/edit 工具记录提取路径。每次工具差异单独打开，
  包含失败标记；「当前文件」读取的是磁盘现状。它们都不是会话开始时的文件快照。
- **Git 改动**：读取工作目录范围内的 Git 状态，包含已暂存、未暂存和未跟踪文件。
  支持在项目根目录下通过 `pipi.toml` 声明一个或多个 Git 仓库路径（支持 monorepo 或子目录仓库）；
  未配置时默认查看工作区根目录仓库。有 HEAD 时以 HEAD 为基线；未出生仓库分别显示索引及工作树差异；未跟踪文本标为新增。
  界面基于 VS Code SCM 体验重构，支持列表/树状结构切换、多仓库过滤与分支显示、折叠分节与差异块（hunk）跳转。
  不操作暂存区，不提交或还原文件。

## 预览和引用

支持 UTF-8 文本、代码高亮、Markdown 原文/渲染切换、PNG/JPEG/GIF/WebP、PDF、
常见音视频的原生浏览器预览。媒体类型同时检查文件特征；HTML、SVG 只展示源码。
Office 文档和 PDF 页码/图形区域引用尚未实现。浏览器不支持 PDF 或某种媒体编解码器时，
使用下载入口；内嵌媒体下载始终需要先经过宿主鉴权及路径校验。

聊天消息中的 Markdown 图片也通过 `preview_workspace_file` 读取当前 Agent 工作区文件，
支持工作区相对路径、绝对路径及本机 `file://` URL。图片显示为缩略图，支持点击放大、
下载和失败重试；手机 Web 与桌面走相同接口，不直接请求宿主磁盘路径。
仍只允许 PNG/JPEG/GIF/WebP、最多 8 MiB，不自动加载外部图片；未修改消息存储格式。

点击行号选择一行，Shift + 点击选择范围。引用保存工作目录、相对路径、内容 SHA-256、
行号、选区原文和可选意见，以普通文本加入现有草稿。不会因预览或选择自动调用模型；
无需新增会话条目格式。文件在引用后可能变化，引用中的版本和摘录代表读取时的内容。

标签页、目录展开、文件名筛选和各文件滚动位置按当前浏览器来源、Agent、工作区配置
及 File/Git 分区隔离，保存在当前窗口的 sessionStorage，不含 sessionId。文件内容不写入
该缓存。读取结果只保留在内存中（最多 64 项、约 32 MiB），按读取类型、工作区、
File/Git 分区及路径隔离；再次进入先显示上次结果，后台读取成功后替换。刷新失败
保留上次结果并提示，不把其他工作区或文件的内容用作占位。刷新按钮、窗口重新
获得焦点、连接恢复及当前 Agent 运行结束都会重新读取。

引用通过明确的目标会话选择器加入该 Agent 的草稿，随后导航回该会话；目标失效时
拒绝写入，不取用其他 Agent 的活动会话。若会话打开失败，已加入的引用仍留在目标草稿。
会话右栏只持有 read/write/edit 的文件与操作记录，不读取整个工作区，也不持有工作区
浏览状态。历史消息按工具调用 id 与名称恢复参数和 details，重开会话后仍可查看操作差异。

File、Git 与会话操作记录共用 FilePreview / FileContent 渲染，读取状态与视图状态分别
由 useWorkspaceFiles、WorkspaceViewStore 拥有，不新增宿主会话事件订阅。

## 边界与限额

- 独立 `workspace` 宿主模块，不扩大 Agent 指令/记忆编辑器的允许路径。
- 路径拒绝 `..`、工作区外的绝对路径、任意层级的 `.git` 和符号链接；只读普通文件。
  Unix 使用 NOFOLLOW/NONBLOCK 打开，Linux 还校验实际打开的文件描述符归属，读取后检查文件身份。
- 每目录最多 2000 项；文本最多 256 KiB，代码行视图最多 10000 行；媒体最多 8 MiB。
  UTF-8 截断保持字符边界；截断内容不标记完整版本、不允许选区引用。
- Git stdout 最多 1 MiB，每条 Git 命令最多 8 秒；取消或超限会终止子进程。
  固定参数、字面路径，禁用 fsmonitor/hooks、clean/process filter、external diff 和 textconv。
- Markdown 不启用 raw HTML；外部图片只显示说明，本地链接重新经过宿主读取路径检查。

## 实现与验证

命令由 `pipi-core/src/commands.rs` 统一声明：`list_workspace_files`、
`preview_workspace_file`、`workspace_changes`、`workspace_diff`。Tauri 只转发，Web
走现有受保护的 dispatch；文本/媒体文件读取使用阻塞线程池，Git 进程异步执行。

- `npm run test:frontend`：引用、Markdown 路径、工具记录和三方 IPC/wire 契约。
- `cargo test -p pipi-core --features server`：路径、限额、UTF-8、格式、Git 不执行仓库命令，
  以及公开命令覆盖普通/未出生仓库、删除文件、特殊名称和子目录工作区。
- `cargo xtask arch`、`cargo check --workspace`、`npm run build`。
- `npm run dev` 后打开 `/tests/files.browser.html`，在完整应用中使用固定假数据验证独立导航、工作区切换、
  指定会话引用、历史操作记录、多标签、Markdown、图片、Git 和迟到响应；不连接真实模型。

当前机器 `/tmp/.git` 会干扰测试的项目发现；完整宿主回归使用 `TMPDIR=/var/tmp` 隔离。
