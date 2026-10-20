# 桌面内嵌远程访问

## 目标与边界

桌面 IPC、HTTP 命令和 WebSocket 事件共享一个 `RuntimeState`。远程浏览器是这个
宿主的另一个客户端：同一会话的运行守卫、停止、steering、审批、后台任务与会话
重键都沿用原入口。关闭 HTTP 服务不取消 Agent 运行；退出宿主才结束进程。

默认不监听网络。启用后只绑定 `127.0.0.1:<port>`，以访问令牌保护 API，公开访问
继续由 Cloudflare Tunnel + Access 提供。桌面应用使用已有的 Tunnel 凭据文件启动
cloudflared 子进程，不注册系统服务，也不修改 DNS 或 Access 策略。公开地址仍是
显示/复制入口，本身不能建立连接。独立 `pipi-server` 保留给无桌面环境。

## 实施顺序

1. **统一事件和宿主所有权**：RuntimeState 持有事件中心，桌面注册同步事件接收器，
   Web 订阅同一广播；宿主启动取得数据根目录的进程锁，避免两个新宿主同时写入。
2. **可管理的 transport**：将环境变量启动拆为可注入运行时、资源与令牌的启动 API；
   返回服务句柄，支持关闭监听和现有 WebSocket，并有界等待在途 HTTP 请求。
3. **本地配置与凭据**：`remote.json` 保存端口、自动启动、公开地址及 Tunnel 凭据
   文件路径（`tunnelTokenFile`，不含凭据内容）；`remote-token`
   单独保存随机令牌。文件原子替换，Unix 凭据权限 0600。管理命令显式区分桌面与
   Web 来源，Web 不能读取/轮换令牌或启停服务；普通 settings 不包含这些数据。
4. **桌面装配**：启用 server feature、注入 Tauri 打包资源、恢复自动启动设置，
   托盘可显示窗口/退出；远程服务运行时关闭窗口隐藏到托盘。开发时远程网页读取
   前端 build 产物，桌面仍使用 Vite HMR。
5. **设置界面**：复用现有左侧设置导航，提供状态、启动/停止、端口、自动启动、
   HTTPS 公开地址及令牌查看/复制/轮换。失败状态可重试，编辑配置不会隐式启动。
6. **验证与部署迁移**：先在隔离数据根验证共享事件、跨客户端运行状态、鉴权、
   Web 管理拒绝、令牌轮换、端口冲突、停止/重启、静态资源与响应式界面；通过后
   再替换旧部署，避免在 Agent 运行时终止旧进程。

## 生命周期与错误语义

- 启停/配置/轮换由一把异步锁串行化。端口冲突清楚报错，不扫描或偷偷换端口。
- 修改端口时先绑定新端口成功，再停止旧服务；失败保留旧入口和已保存配置。
  托管 Tunnel 时须先关闭远程才能修改端口或凭据路径，并在 Cloudflare 同步服务
  路由端口；应用不推测或自动改写云端路由。
- “随应用启动远程”恢复本机 Web 服务和 cloudflared；“启动远程/关闭远程”同时
  管理两者。退出应用时关闭子进程，停止远程不会停止 Agent。关闭自动启动只影响
  下次启动，不隐式停止当前连接；保存配置也不隐式启动远程。
- `running` 只在本机服务运行且 Tunnel `/ready` 返回 200 时为真；`localRunning`、
  `tunnelRunning` 和 `tunnelConnected` 分别报告本机监听、子进程及 Cloudflare 连接。
  本机监听不再显示为公网已连接。Tunnel 连接状态不保证域名路由或 Access 登录正确。
- cloudflared 只接受凭据文件路径，Unix 文件权限不得向组或其他用户开放；应用不
  传令牌命令行、不转发原始客户端日志，并清除继承的 `TUNNEL_*` / `CF_*` 配置。
  指标端口只绑定随机回环端口；超时/断线显示连接中并由 cloudflared 自行重连。
  进程退出显示错误，由用户关闭后重新启动。启动失败会回收本机服务，避免半开状态。
- 兼容旧 `remote.json`：缺少 `tunnelTokenFile` 且有公开地址时，若旧配置目录存在
  `pipi-web/tunnel.token` 则复用其路径。显式保存的空路径不会自动补回。地址与凭据
  都留空时允许仅启动本机 Web 服务，界面会明确提示公网未连接。
- 停止/轮换断开已有 WebSocket，防止旧授权会话在令牌变更后继续收事件。
- 浏览器重连通过现有 session_infos 和水合恢复视图。广播积压时断开连接，促使
  客户端重新水合，不能静默丢失审批/终态后继续展示陈旧状态。
- 只接受同源浏览器请求，拒绝跨站 Origin；不再反射任意 CORS Origin。
- 开始监听前验证首页资源存在，缺少 build 产物会报告可操作的错误。
- 配置损坏时保持服务关闭并显示错误，不能恢复为无鉴权监听。

## 与参考项目的关系

- `reference/codex/codex-rs/app-server/src/in_process.rs`：复用既有处理器/路由，
  进程内宿主不另造执行契约；采用明确启动、关闭与积压处理。
- `reference/zcode/packages/server/src/http.ts`：transport 接收已装配服务集合，
  HTTP/WS 与静态资源共用入口；借鉴服务注入与令牌登录，权限仍以 Pipi 契约为准。

## Cloudflare 现有部署迁移

保持 Tunnel 指向 `http://127.0.0.1:1421`，保持整域名 Access 策略。确认旧服务没有
正在执行的会话/后台任务后，停止并禁用 `pipi-web.service`；移除
`pipi-tunnel.service` 对旧服务的 Wants/After 依赖，再启动桌面内嵌入口。
原部署令牌可显式迁入 `remote-token`，避免浏览器重新登录；不在启动时自动读取
其他程序的秘密文件。回退时退出桌面、停用内嵌自动启动，再恢复独立服务。

配置文件/服务单元属于主机状态，代码实现完成不代表当前正在运行的旧二进制已经
升级。实施报告必须分别说明代码检查、隔离运行验证和实际切换结果。

## 验证入口

```sh
npm run test:frontend
npm run build
cargo test -p pipi-core
cargo test -p pipi-core --features server
cargo test -p pipi-venv
cargo check --workspace
cargo xtask arch
npm run tauri:build -- --debug --no-bundle
```

`crates/pipi-core/tests/remote_access.rs` 使用真实回环 HTTP/WS 和本机模拟模型，
覆盖共享运行守卫/桌面停止 Web 运行、停服务不取消运行、Web 管理拒绝、令牌轮换
断连、配置失败保留旧端口、鉴权、同源检查、配置损坏保持关闭和凭据权限。
测试的临时 HOME 应位于非 Git 目录；宿主若在 `/tmp` 放置了 `.git` 标记，可用
`TMPDIR=/var/tmp cargo test …` 避免改变既有契约发现用例的前提。

桌面资源已用隔离 `PIPI_HOME`、自动启动配置和独立端口验证：服务返回内置 HTML/JS，
开启令牌保护，资源路径不依赖进程工作目录。设置界面已检查桌面、390px、320px
以及移动导航的 Esc 关闭。托盘需由实际桌面环境提供；创建失败时关闭窗口仍退出，
不会隐藏到无法恢复的窗口。

## 2026-10-19 本机切换记录

已将 `https://pipi.whoeasonque.dev` 切换为桌面内嵌服务：

- `pipi-web.service` 已停止并禁用。
- `pipi-desktop.service` 已启用，随图形会话启动，运行
  `~/.local/lib/pipi-desktop/pipi`；工作目录为用户主目录，不依赖源码目录。
- `pipi-tunnel.service` 保持运行，已移除对旧 Web 服务的依赖；Tunnel 目标仍为
  `http://127.0.0.1:1421`，DNS 与整域名 Access 策略保持原配置。
- `remote.json` 已设为端口 1421、`startWithApp: true` 和上述公开地址；
  `remote-token` 复用原部署令牌，Unix 权限 0600。

切换后已验证：未登录访问首页/API/WS 均进入 Access；通过 Access 后仍需 Pipi
令牌；同源登录、Agent 列表 API 和 TLS WebSocket Ping/Pong 正常；Web 读取服务
管理凭据的命令被拒绝。验证未执行 Agent 或发起模型调用。

本机备份位于
`~/.config/pipi-web/backups/embedded-20261019-170020/`，包含旧配置与回退工具。
需要恢复独立 Web 宿主时可运行（会停止内嵌宿主）：

```sh
python3 ~/.config/pipi-web/backups/embedded-20261019-170020/migration-tool.py \
  --rollback ~/.config/pipi-web/backups/embedded-20261019-170020
```

查看运行状态与日志：

```sh
systemctl --user status pipi-desktop.service pipi-tunnel.service
journalctl --user -u pipi-desktop.service -n 50 --no-pager
```

关闭桌面窗口可留在托盘；退出 Pipi 或结束图形会话会停止内嵌远程服务。
