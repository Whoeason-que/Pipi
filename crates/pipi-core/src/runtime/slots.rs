//! 会话初始化、查询和控制的宿主入口。
use super::*;

/// 列出指定 Agent 的会话摘要。
pub fn list_sessions(agent_name: &str) -> Result<Vec<SessionSummary>, String> {
    let def = agents::load_agent(agent_name)?;
    Ok(session_repo(&def)?.list())
}

impl RuntimeState {
    /// 从会话载体读回并载入一条会话（账本重建）。不插入 map —— 调用方
    /// 在全部前置步骤成功后再插入。
    pub(super) fn load_session_object(
        def: &AgentDefinition,
        repo: Arc<dyn SessionRepo>,
        session_id: &str,
    ) -> Result<HostSession, String> {
        let (log, entries) = repo.open_with_entries(session_id)?;
        let engine = AgentSession::restore(
            SessionStorage {
                log,
                repo: Some(repo),
            },
            entries,
            def.provider.clone(),
            AbortSignal::new(),
        )?;
        Ok(HostSession {
            agent: def.clone(),
            engine,
            temporary: false,
        })
    }
    pub(super) fn create_session_object(
        def: &AgentDefinition,
        repo: Arc<dyn SessionRepo>,
        model: Option<&Model>,
    ) -> Result<(HostSession, String), String> {
        let log = repo.create()?;
        let id = log.session_id().to_string();
        let engine = AgentSession::create(
            SessionStorage {
                log,
                repo: Some(repo),
            },
            def.provider.clone(),
            model.cloned(),
            AbortSignal::new(),
        )?;
        Ok((
            HostSession {
                agent: def.clone(),
                engine,
                temporary: false,
            },
            id,
        ))
    }
    fn create_test_session_object(def: &AgentDefinition) -> (HostSession, String) {
        let log = Box::new(MemoryLog::new());
        let id = log.session_id().to_string();
        let engine = AgentSession::create(
            SessionStorage { log, repo: None },
            def.provider.clone(),
            None,
            AbortSignal::new(),
        )
        .expect("空的临时账本初始化不执行落盘");
        (
            HostSession {
                agent: def.clone(),
                engine,
                temporary: true,
            },
            id,
        )
    }

    /// 返回该 Agent 在应用运行期唯一的临时测试会话；没有则在内存中创建。
    pub fn ensure_test_session(&self, agent_name: &str) -> Result<SessionInfo, String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if let Some(session) = sessions.iter().find_map(|(key, session)| {
            (key.agent_name == agent_name && session.temporary).then_some(session)
        }) {
            return self.session_info_of(session);
        }

        // 定义读取也放在 sessions 锁内，与 save_agent_definition 的「写文件 +
        // 丢弃旧测试」串行，避免并发保存时用保存前的快照新建测试会话。
        let definition = agents::load_agent(agent_name)?;
        let (session, session_id) = Self::create_test_session_object(&definition);
        let info = self.session_info_of(&session)?;
        sessions.insert(SessionKey::new(agent_name, &session_id), session);
        Ok(info)
    }

    /// 丢弃该 Agent 的空闲临时上下文并用最新保存的 Agent 定义新建一条。
    pub fn reset_test_session(&self, agent_name: &str) -> Result<SessionInfo, String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.iter().any(|(key, session)| {
            key.agent_name == agent_name && session.temporary && self.session_occupied(session)
        }) {
            return Err(
                "临时测试仍在运行或托管后台任务未结束，请先停止/终止后再清空或保存设置".into(),
            );
        }
        sessions.retain(|key, session| !(key.agent_name == agent_name && session.temporary));
        let definition = agents::load_agent(agent_name)?;
        let (session, session_id) = Self::create_test_session_object(&definition);
        let info = self.session_info_of(&session)?;
        sessions.insert(SessionKey::new(agent_name, &session_id), session);
        Ok(info)
    }

    /// 打开（续写）一个已有会话。已经打开时是幂等的空操作 —— 同一个 Agent 的
    /// 多条会话可以同时打开，打开其中一条不影响别的。
    pub fn open_session(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let def = agents::load_agent(agent_name)?;
        let repo = session_repo(&def)?;
        if !repo.exists(session_id)? {
            return Err("会话不存在".into());
        }

        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let key = SessionKey::new(agent_name, session_id);
        if sessions.contains_key(&key) {
            return Ok(());
        }
        let session = Self::load_session_object(&def, repo.clone(), session_id)?;
        sessions.insert(key, session);
        Ok(())
    }

    /// 分叉一个会话（可指定截断至某个 entry_id，留空则分叉到当前 tip）。
    pub fn fork_session(
        &self,
        agent_name: &str,
        session_id: &str,
        up_to_entry_id: Option<&str>,
    ) -> Result<SessionInfo, String> {
        if session_id.is_empty()
            || !session_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err("非法会话 ID".into());
        }
        let def = agents::load_agent(agent_name)?;
        let repo = session_repo(&def)?;
        if !repo.exists(session_id)? {
            return Err("源会话不存在".into());
        }

        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        // 源会话正在跑时不允许分叉（分叉会复制一份半途的历史）
        if sessions
            .get(&SessionKey::new(agent_name, session_id))
            .is_some_and(|session| self.session_occupied(session))
        {
            return Err(format!(
                "会话「{session_id}」正在运行或有后台任务，请先停止/终止后再分叉"
            ));
        }

        let writer = repo.fork(session_id, up_to_entry_id)?;
        let new_session_id = writer.session_id().to_string();

        let entries = repo.entries(&new_session_id)?;
        let engine = AgentSession::restore(
            SessionStorage {
                log: writer,
                repo: Some(repo),
            },
            entries,
            def.provider.clone(),
            AbortSignal::new(),
        )?;
        let session = HostSession {
            agent: def,
            engine,
            temporary: false,
        };
        let info = self.session_info_of(&session)?;
        sessions.insert(SessionKey::new(agent_name, &new_session_id), session);
        Ok(info)
    }

    /// 某条会话（Agent + 会话 id）的信息；没有打开返回 None。
    pub fn session_info(
        &self,
        agent_name: &str,
        session_id: &str,
    ) -> Result<Option<SessionInfo>, String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|error| format!("无法读取会话槽: {error}"))?;
        match sessions.get(&SessionKey::new(agent_name, session_id)) {
            Some(session) => Ok(Some(self.session_info_of(session)?)),
            None => Ok(None),
        }
    }

    /// 所有打开中的会话。前端据此知道「哪些 Agent 正在跑」——多 Agent 并发下
    /// 运行态是集合而不是单值。
    pub fn session_infos(&self) -> Result<Vec<SessionInfo>, String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|error| format!("无法读取会话槽: {error}"))?;
        let mut infos = Vec::with_capacity(sessions.len());
        for session in sessions.values() {
            infos.push(self.session_info_of(session)?);
        }
        Ok(infos)
    }

    pub(super) fn session_info_of(&self, session: &HostSession) -> Result<SessionInfo, String> {
        let snapshot = session.engine.snapshot()?;
        let owner = BackgroundTaskOwner::new(
            session.agent.name.clone(),
            snapshot.session_id.clone(),
            snapshot.run_id,
        );
        Ok(SessionInfo {
            agent_name: session.agent.name.clone(),
            session_id: snapshot.session_id,
            temporary: snapshot.temporary,
            running: snapshot.running,
            background_tasks: self.background.active_count(&owner),
            run_id: snapshot.run_id,
            is_custom_model: snapshot.selected_model.is_some(),
            model: snapshot
                .selected_model
                .or_else(|| session.agent.provider.clone()),
        })
    }
    fn session_has_active_background_tasks(&self, session: &HostSession) -> bool {
        let Ok(snapshot) = session.engine.snapshot() else {
            return true;
        };
        let owner = BackgroundTaskOwner::new(
            session.agent.name.clone(),
            snapshot.session_id,
            snapshot.run_id,
        );
        self.background.active_count(&owner) > 0
    }

    pub(super) fn session_occupied(&self, session: &HostSession) -> bool {
        session.engine.snapshot().map(|s| s.running).unwrap_or(true)
            || self.session_has_active_background_tasks(session)
    }

    /// 让文件级会话变更可以作用于当前仍显示在前端的空闲会话。
    ///
    /// `Session` 持有追加文件的 writer；先从槽中移除并在本函数返回前释放它，
    /// 再由调用方在同一把 sessions 锁内移动/删除文件。运行中或仍有托管后台
    /// 任务时不允许脱离，避免后台继续向已归档/已删除的路径写入。
    pub(super) fn detach_idle_session(
        &self,
        sessions: &mut HashMap<SessionKey, HostSession>,
        agent_name: &str,
        session_id: &str,
        operation: &str,
    ) -> Result<(), String> {
        let key = SessionKey::new(agent_name, session_id);
        if sessions
            .get(&key)
            .is_some_and(|session| self.session_occupied(session))
        {
            return Err(format!(
                "会话「{session_id}」正在运行或有托管后台任务，请先停止/终止后再{operation}"
            ));
        }
        sessions.remove(&key);
        Ok(())
    }

    /// 为某条会话设置或切换模型配置。
    /// 传入 None 时恢复为 Agent 默认模型。
    pub fn set_session_model(
        &self,
        agent_name: &str,
        session_id: &str,
        model: Option<Model>,
    ) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let Some(session) = sessions.get(&SessionKey::new(agent_name, session_id)) else {
            return Err(format!("会话「{session_id}」没有打开"));
        };
        if session.engine.snapshot().map(|s| s.running).unwrap_or(true) {
            return Err("会话正在运行，请等待完成或先停止后再切换模型".into());
        }

        let effective_target = model.clone().or_else(|| session.agent.provider.clone());
        if let Some(target) = &effective_target {
            let _ = resolve_model(&session.agent, Some(target), None)?;
        }
        session.engine.set_model(model, effective_target)
    }

    /// 某条会话是否正在跑一轮。
    pub fn session_running(&self, agent_name: &str, session_id: &str) -> bool {
        self.sessions
            .lock()
            .map(|sessions| {
                sessions
                    .get(&SessionKey::new(agent_name, session_id))
                    .map(|session| session.engine.snapshot().map(|s| s.running).unwrap_or(true))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// 停止某条会话的当前一轮；这条会话没有打开时是空操作。只停这一条 ——
    /// 同一个 Agent 的其他会话照跑。
    pub fn stop_run(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if let Some(session) = sessions.get(&SessionKey::new(agent_name, session_id)) {
            session.engine.stop()?;
        }
        Ok(())
    }

    /// 查询某条会话拥有的托管后台任务。宿主查询与 Agent 的 query tool 共用同一
    /// owner 边界，避免通过 UI / Web command 看到别的 session 的任务。
    pub async fn query_background_tasks(
        &self,
        agent_name: &str,
        session_id: &str,
        job_id: Option<String>,
        after_seq: u64,
        wait_ms: u64,
        include_completed: bool,
        limit: usize,
    ) -> Result<Vec<pipi_protocol::BackgroundTaskSnapshot>, String> {
        let run_id = {
            let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
            let Some(session) = sessions.get(&SessionKey::new(agent_name, session_id)) else {
                return Err(format!("会话「{session_id}」没有打开"));
            };
            session.engine.snapshot()?.run_id
        };
        let owner = BackgroundTaskOwner::new(agent_name, session_id, run_id);
        self.background
            .query(
                &owner,
                crate::tools::background::BackgroundTaskQuery {
                    job_id,
                    after_seq,
                    wait_ms,
                    include_completed,
                    limit,
                },
            )
            .await
    }

    /// 由宿主终止或向指定后台任务写入 stdin；实际控制仍由 app manager 执行。
    pub async fn manage_background_task(
        &self,
        agent_name: &str,
        session_id: &str,
        job_id: &str,
        action: &str,
        data: Option<String>,
    ) -> Result<pipi_protocol::BackgroundTaskSnapshot, String> {
        let run_id = {
            let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
            let Some(session) = sessions.get(&SessionKey::new(agent_name, session_id)) else {
                return Err(format!("会话「{session_id}」没有打开"));
            };
            session.engine.snapshot()?.run_id
        };
        let command = match action {
            "terminate" => crate::tools::background::BackgroundTaskCommand::Terminate,
            "writeStdin" => crate::tools::background::BackgroundTaskCommand::WriteStdin(
                data.ok_or("writeStdin 需要 data")?,
            ),
            _ => return Err("action 必须是 terminate 或 writeStdin".into()),
        };
        let owner = BackgroundTaskOwner::new(agent_name, session_id, run_id);
        self.background.manage(&owner, job_id, command).await
    }

    /// 运行中插话（pi 的 steering）：消息注入该会话正在跑的下一轮上下文。
    /// 仅在会话正在运行时接受；消息本体由 loop 注入时经 emitter 落盘。
    /// 运行结束瞬间提交的插话由收割逻辑接住，不会丢。
    pub fn steer(&self, agent_name: &str, session_id: &str, message: &str) -> Result<(), String> {
        let message = message.trim();
        if message.is_empty() {
            return Err("空消息".into());
        }
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let Some(session) = sessions.get(&SessionKey::new(agent_name, session_id)) else {
            return Err(format!("会话「{session_id}」没有打开"));
        };
        if !session.engine.snapshot().map(|s| s.running).unwrap_or(true) {
            return Err("会话未在运行，请直接发送消息".into());
        }
        session.engine.steer(message)
    }

    /// 回传一次审批请求的用户决定。请求已过期（超时 / 中止）时返回 Err。
    pub fn resolve_approval(
        &self,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        self.approval.resolve(request_id, decision)
    }

    /// 释放某个 Agent 名下**所有空闲**的会话（不落盘，文件留在磁盘上）。
    /// 正在跑的那条保留 —— 后台运行不会因为「离开这个 Agent」被清掉。
    pub fn new_session(&self, agent_name: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        sessions.retain(|key, session| {
            key.agent_name != agent_name || session.temporary || self.session_occupied(session)
        });
        Ok(())
    }

    pub async fn session_messages(
        &self,
        agent_name: &str,
        session_id: &str,
    ) -> Result<Vec<Message>, String> {
        let messages = {
            let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
            sessions
                .get(&SessionKey::new(agent_name, session_id))
                .map(|session| session.engine.messages())
        };
        match messages {
            Some(messages) => Ok(messages.await),
            None => Ok(Vec::new()),
        }
    }

    pub fn session_stats(
        &self,
        agent_name: &str,
        session_id: &str,
    ) -> Result<pipi_agent::stats::SessionStats, String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        match sessions.get(&SessionKey::new(agent_name, session_id)) {
            Some(session) => session.engine.stats(),
            None => Ok(Default::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// 回归：桌面壳的 Tauri command 跑在 GTK 主线程上，那里没有 reactor。
    /// 核心必须把运行 spawn 到注入的运行时上，而不是依赖调用线程的上下文 ——
    /// 修复前这里会 panic（there is no reactor running），且 panic 发生在主线程、
    /// 直接中止整个进程。
    #[test]
    fn spawn_run_works_without_ambient_reactor() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let state = RuntimeState::new(runtime.handle().clone());
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = done.clone();

        // 关键：不套 runtime.block_on —— 模拟「调用线程不在运行时里」
        state.spawn_run(async move {
            flag.store(true, Ordering::SeqCst);
        });
        assert!(!done.load(Ordering::SeqCst), "任务不该在调用线程上同步执行");

        runtime.block_on(async {
            tokio::task::yield_now().await;
        });
        assert!(
            done.load(Ordering::SeqCst),
            "任务必须落在注入的运行时上执行"
        );
    }
}
