//! 会话槽、文件生命周期与宿主查询。

use super::*;

impl RuntimeState {
    /// 用宿主运行时句柄构造。句柄必须指向多线程、IO/time 驱动齐全的运行时。
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        let background =
            BackgroundTaskManager::new(runtime.clone(), Arc::new(super::RuntimeChildRunner));
        let mcp = crate::mcp::McpManager::new(runtime.clone());
        let events = crate::events::EventHub::default();
        #[cfg(feature = "server")]
        let push = {
            let push = Arc::new(crate::notifications::push::PushService::default());
            crate::notifications::push::start(&runtime, &events, push.clone());
            push
        };
        Self {
            runtime,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            shutting_down: Arc::new(AtomicBool::new(false)),
            background,
            mcp,
            events,
            #[cfg(feature = "server")]
            push,
            remote: Arc::new(std::sync::OnceLock::new()),
        }
    }

    pub fn events(&self) -> &crate::events::EventHub {
        &self.events
    }

    /// 该 Agent 声明的 MCP 服务器与当前连接状态。
    ///
    /// 管理器里只有「跑过一轮」的服务器；这里按定义补齐，让工作台在会话没跑
    /// 之前也能看到声明（此时 `connected` 为 false）。
    pub fn mcp_status(&self, agent_name: &str) -> Result<Vec<crate::mcp::McpServerStatus>, String> {
        let def = agents::load_agent(agent_name)?;
        let mut statuses = self.mcp.status(agent_name);
        for server in &def.mcp_servers {
            if statuses.iter().any(|status| status.name == server.name) {
                continue;
            }
            statuses.push(crate::mcp::McpServerStatus {
                name: server.name.clone(),
                enabled: server.enabled,
                connected: false,
                tools: 0,
                error: None,
            });
        }
        statuses.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(statuses)
    }

    /// 会话打开时的 MCP 预热：把服务器先拉起来，第一轮提示词之前通常已经连好。
    ///
    /// 环境解析失败（契约缺失 / requires 不满足）静默跳过 —— 这一轮没有 MCP，
    /// 真正的错误会在开始运行时原样报出。会话 id 不参与：服务器是 Agent 级的，
    /// 由该 Agent 的所有会话共享。
    pub(super) fn prefetch_mcp(&self, def: &AgentDefinition) {
        if self.ensure_not_shutting_down().is_err() {
            return;
        }
        if !def.mcp_servers.iter().any(|server| server.enabled) {
            return;
        }
        if let Ok(env) = agents::resolve_agent_env(def, None) {
            self.mcp.prepare(def, &env.vars);
        }
    }

    /// Agent 被归档/删除时收掉它的 MCP 服务器进程。
    pub fn shutdown_agent_mcp(&self, agent_name: &str) {
        self.mcp.shutdown_agent(agent_name);
    }

    #[cfg(feature = "server")]
    pub fn push_service(&self) -> Arc<crate::notifications::push::PushService> {
        self.push.clone()
    }

    pub fn attach_remote_access(
        &self,
        remote: Arc<dyn crate::remote::RemoteAccessControl>,
    ) -> Result<(), String> {
        self.remote
            .set(remote)
            .map_err(|_| "Remote access controller is already mounted".to_string())
    }

    pub fn remote_access(&self) -> Result<&dyn crate::remote::RemoteAccessControl, String> {
        self.remote
            .get()
            .map(Arc::as_ref)
            .ok_or_else(|| "Desktop remote access management is not enabled on current host".into())
    }

    /// 把一轮运行交给注入的运行时执行（不依赖调用线程的 reactor 上下文）。
    pub(super) fn spawn_run<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn(task);
    }

    /// 该 Agent 的会话仓库。载体由 [`session_repo`] 注入 —— 生命周期操作
    /// （归档/恢复/删除/分叉/列表）只经过 [`SessionRepo`] 接口。
    fn session_repo_for(agent_name: &str) -> Result<Arc<dyn SessionRepo>, String> {
        session_repo(&agents::load_agent(agent_name)?)
    }

    // ============ 会话归档 / 恢复 / 删除 ============
    // 归档 / 恢复 / 删除都交给 [`SessionRepo`]：JSONL 载体上就是
    // `sessions/<id>.jsonl` ↔ `sessions/.archive/<id>.jsonl` 的移动与删除，
    // 与 Agent 归档同一套「文件即真相」语义。删除不可恢复，UI 层需确认。
    // 注意：占用检查、从会话槽脱离与载体操作必须在同一把 sessions 锁内完成，
    // 否则检查与移动之间可能被并发 open_session 抢跑。空闲的当前会话可以由
    // 归档/删除操作自动脱离；正在运行或仍有托管后台任务的会话必须先停止。
    // 桌面和内嵌 Web 共用本锁。进程间由宿主启动时的 HostLease 排他保护。

    /// 归档会话。
    ///
    /// 空闲会话即使仍在前端打开，也会在移动前自动从运行时会话槽脱离。压缩换
    /// 会话后的内部归档走压缩路径里的 `repo.archive`（那时旧 id 已不是打开的会话）。
    pub fn archive_session(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let repo = Self::session_repo_for(agent_name)?;
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        self.ensure_not_shutting_down()?;
        if !repo.exists(session_id)? {
            return Err("Session does not exist".into());
        }
        self.detach_idle_session(&mut sessions, agent_name, session_id, "archiving")?;
        repo.archive(session_id)
    }

    /// 恢复归档会话。
    pub fn restore_session(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let _sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        Self::session_repo_for(agent_name)?.restore(session_id)
    }

    /// 彻底删除会话。不可恢复；UI 层需确认。
    pub fn delete_session(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let repo = Self::session_repo_for(agent_name)?;
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        self.ensure_not_shutting_down()?;
        if !repo.exists(session_id)? {
            return Err("Session does not exist".into());
        }
        self.detach_idle_session(&mut sessions, agent_name, session_id, "deleting")?;
        repo.delete(session_id)
    }

    /// 删除已归档会话。不可恢复。
    pub fn delete_archived_session(
        &self,
        agent_name: &str,
        session_id: &str,
    ) -> Result<(), String> {
        validate_session_id(session_id)?;
        let _sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        Self::session_repo_for(agent_name)?.delete_archived(session_id)
    }

    /// 列出已归档会话摘要。
    pub fn list_archived_sessions(&self, agent_name: &str) -> Result<Vec<SessionSummary>, String> {
        Self::session_repo_for(agent_name)?.list_archived()
    }

    /// 会话提交历史（一轮一个 commit；右栏 git tree 只读）。
    pub fn session_history(
        &self,
        agent_name: &str,
        session_id: &str,
    ) -> Result<Vec<pipi_protocol::session::SessionCommit>, String> {
        validate_session_id(session_id)?;
        Self::session_repo_for(agent_name)?.history(session_id)
    }

    /// 导入/归档/删除共用的占用检查与槽位脱离；调用方必须已持有 sessions 锁，
    /// 且在本函数返回后继续持有到载体操作完成（避免检查与 IO 之间被抢跑）。
    fn detach_agent_sessions_locked(
        &self,
        sessions: &mut HashMap<SessionKey, HostSession>,
        agent_name: &str,
        action: &str,
    ) -> Result<(), String> {
        self.ensure_not_shutting_down()?;
        if sessions
            .iter()
            .any(|(key, session)| key.agent_name == agent_name && self.session_occupied(session))
        {
            return Err(format!(
                "Agent \"{agent_name}\" still has running sessions, queued messages or uncompleted managed background tasks; please cancel queued messages and stop or terminate tasks before {action}"
            ));
        }
        sessions.retain(|key, _session| key.agent_name != agent_name);
        Ok(())
    }

    /// 导入前的占用检查与脱离槽位（宿主命令 `commands::import_sessions` 在锁内
    /// 同步调用，随后把读会话与 spawn git 的阻塞部分放上阻塞线程池）：运行中或
    /// 仍有托管后台任务的会话必须先停止；空闲会话在这里自动脱离槽位 —— 关掉
    /// 它们的文件句柄，否则导入改名后打开的句柄会继续往旧文件追加。
    pub fn prepare_session_import(&self, agent_name: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        self.detach_agent_sessions_locked(&mut sessions, agent_name, "importing")
    }

    // ============ Agent 归档 / 恢复 / 删除（运行时占用检查） ============

    /// 归档 Agent：目录移入 `.archive/`。空闲会话会随操作自动释放；运行中或
    /// 仍有托管后台任务的会话必须先停止/终止。占用检查与目录移动在同一把
    /// sessions 锁内，避免检查后被并发打开抢跑。
    pub fn archive_agent(&self, name: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        // 空闲正式会话的 writer 也在这里释放，避免归档后仍有打开的文件句柄；
        // 临时测试同样一并清掉。前端不需要先退出会话界面。
        self.detach_agent_sessions_locked(&mut sessions, name, "archiving")?;
        // `open_session` 也必须先取得这把锁；不要在检查与目录移动之间释放它，
        // 否则并发打开会把一个仍指向活跃目录的 Session 插入 map，再由这里把
        // 目录移走。归档是罕见的同步文件操作，短暂持锁优先于留下悬空会话。
        agents::archive_agent(name)
    }

    /// 彻底删除 Agent。空闲会话会随操作自动释放；运行中或仍有托管后台任务的
    /// 会话必须先停止/终止；不可恢复。
    pub fn delete_agent(&self, name: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        // 与归档相同：先释放所有空闲会话槽，再做不可逆删除。
        self.detach_agent_sessions_locked(&mut sessions, name, "deleting")?;
        // 与归档相同：占用检查和不可逆删除是同一个临界区，不能让
        // `open_session` 在两者之间抢跑。
        agents::delete_agent(name)
    }

    /// 保存 Agent 定义时，临时测试必须空闲；成功后丢弃旧测试上下文，确保下一轮
    /// 从完整的新定义（模型、权限、环境与工作目录）重新构造。
    pub fn save_agent_definition(&self, def: &AgentDefinition) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        self.ensure_not_shutting_down()?;
        if sessions.iter().any(|(key, session)| {
            key.agent_name == def.name && session.temporary && self.session_occupied(session)
        }) {
            return Err("Temporary test is still running, has queued messages or uncompleted managed background tasks; please cancel queued messages and stop or terminate tasks before saving settings".into());
        }
        agents::save_agent(def)?;
        // 已打开的正式会话持有 Agent 快照；只刷新压缩控制项，让用户保存后
        // 立即压缩该会话时用上新设置。正在执行的一轮仍用启动时的快照。
        for (key, session) in sessions.iter_mut() {
            if key.agent_name == def.name && !session.temporary {
                session.agent.compact_threshold_percent = def.compact_threshold_percent;
                session.agent.compact_target_percent = def.compact_target_percent;
            }
        }
        sessions.retain(|key, session| !(key.agent_name == def.name && session.temporary));
        Ok(())
    }

    /// 写入 harness/AGENTS.md / memory 后同样让旧测试上下文失效，避免界面声称已经
    /// 测试了尚未注入的文件内容。
    pub fn write_agent_file(
        &self,
        agent_name: &str,
        rel_path: &str,
        content: &str,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        self.ensure_not_shutting_down()?;
        if sessions.iter().any(|(key, session)| {
            key.agent_name == agent_name && session.temporary && self.session_occupied(session)
        }) {
            return Err("Temporary test is still running, has queued messages or uncompleted managed background tasks; please cancel queued messages and stop or terminate tasks before saving files".into());
        }
        agents::write_agent_file(agent_name, rel_path, content)?;
        sessions.retain(|key, session| !(key.agent_name == agent_name && session.temporary));
        Ok(())
    }
}
