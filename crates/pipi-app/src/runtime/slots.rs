//! 会话槽、文件生命周期与宿主查询。

use super::*;

impl RuntimeState {
    /// 用宿主运行时句柄构造。句柄必须指向多线程、IO/time 驱动齐全的运行时。
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        let background = BackgroundTaskManager::new(runtime.clone());
        Self {
            runtime,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            approval: Arc::new(ApprovalGate::new()),
            background,
        }
    }

    /// 把一轮运行交给注入的运行时执行（不依赖调用线程的 reactor 上下文）。
    pub(super) fn spawn_run<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn(task);
    }

    fn session_file_path(
        &self,
        agent_name: &str,
        session_id: &str,
    ) -> Result<std::path::PathBuf, String> {
        let def = agents::load_agent(agent_name)?;
        let dir = def
            .sessions_dir()
            .ok_or_else(|| "无法解析会话目录".to_string())?;
        // 会话目录必须是真实目录（非符号链接），否则 rename/remove 会沿链接波及外部文件
        agents::ensure_real_directory(&dir, "sessions 目录")?;
        Ok(dir.join(format!("{session_id}.jsonl")))
    }

    /// 校验会话文件：必须是真实文件（非符号链接）；不存在返回 false。
    fn ensure_real_session_file(path: &std::path::Path) -> Result<bool, String> {
        agents::ensure_real_file(path, "会话文件")
    }

    // ============ 会话归档 / 恢复 / 删除 ============
    // 归档 = 移动 `sessions/<id>.jsonl` 到 `sessions/.archive/<id>.jsonl`，
    // 与 Agent 归档同一套「文件即真相」语义。删除不可恢复，UI 层需确认。
    // 注意：占用检查、从会话槽脱离与文件操作必须在同一把 sessions 锁内完成，
    // 否则检查与移动之间可能被并发 open_session 抢跑。空闲的当前会话可以由
    // 归档/删除操作自动脱离；正在运行或仍有托管后台任务的会话必须先停止。
    // （跨进程限制：桌面壳与 Web 服务同时跑时各有 RuntimeState，进程间的
    // 并发打开不在本锁覆盖范围内 —— 产品形态是二选一，已知限制。）

    /// 归档会话：移动到 `sessions/.archive/`。
    ///
    /// 空闲会话即使仍在前端打开，也会在移动前自动从运行时会话槽脱离；实际移动
    /// 是 [`pipi_core::session::archive_session_file`]。压缩换会话后的内部归档仍然
    /// 直接走那个自由函数（那时旧 id 已不再是打开的会话）。
    pub fn archive_session(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let src = self.session_file_path(agent_name, session_id)?;
        if !Self::ensure_real_session_file(&src)? {
            return Err("会话不存在".into());
        }
        self.detach_idle_session(&mut sessions, agent_name, session_id, "归档")?;
        let Some(dir) = src.parent() else {
            return Err("无法解析会话目录".into());
        };
        pipi_core::session::archive_session_file(dir, session_id).map(|_| ())
    }

    /// 恢复归档会话：移回 `sessions/`。
    pub fn restore_session(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let _sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let live_dir = self
            .session_file_path(agent_name, session_id)?
            .parent()
            .ok_or_else(|| "无法解析会话目录".to_string())?
            .to_path_buf();
        let src = live_dir
            .join(agents::ARCHIVE_DIR)
            .join(format!("{session_id}.jsonl"));
        if !Self::ensure_real_session_file(&src)? {
            return Err("归档区没有该会话".into());
        }
        let dst = live_dir.join(format!("{session_id}.jsonl"));
        if dst.exists() {
            return Err("活跃区已存在同名会话".into());
        }
        std::fs::rename(&src, &dst).map_err(|e| format!("恢复会话失败: {e}"))
    }

    /// 彻底删除会话（`sessions/<id>.jsonl`）。不可恢复；UI 层需确认。
    pub fn delete_session(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        validate_session_id(session_id)?;
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let path = self.session_file_path(agent_name, session_id)?;
        if !Self::ensure_real_session_file(&path)? {
            return Err("会话不存在".into());
        }
        self.detach_idle_session(&mut sessions, agent_name, session_id, "删除")?;
        std::fs::remove_file(&path).map_err(|e| format!("删除会话失败: {e}"))
    }

    /// 删除已归档会话（`sessions/.archive/<id>.jsonl`）。不可恢复。
    pub fn delete_archived_session(
        &self,
        agent_name: &str,
        session_id: &str,
    ) -> Result<(), String> {
        validate_session_id(session_id)?;
        let _sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let def = agents::load_agent(agent_name)?;
        let archive_dir = def
            .sessions_dir()
            .ok_or_else(|| "无法解析会话目录".to_string())?
            .join(agents::ARCHIVE_DIR);
        if !agents::ensure_real_directory(&archive_dir, "归档目录")? {
            return Err("归档区没有该会话".into());
        }
        let path = archive_dir.join(format!("{session_id}.jsonl"));
        if !Self::ensure_real_session_file(&path)? {
            return Err("归档区没有该会话".into());
        }
        std::fs::remove_file(&path).map_err(|e| format!("删除归档会话失败: {e}"))
    }

    /// 列出已归档会话摘要。
    pub fn list_archived_sessions(&self, agent_name: &str) -> Result<Vec<SessionSummary>, String> {
        let def = agents::load_agent(agent_name)?;
        let dir = def
            .sessions_dir()
            .ok_or_else(|| "无法解析会话目录".to_string())?
            .join(agents::ARCHIVE_DIR);
        match agents::ensure_real_directory(&dir, "归档目录") {
            Ok(false) => return Ok(Vec::new()),
            Ok(true) => {}
            Err(error) => return Err(error),
        }
        Ok(list_session_summaries(&dir))
    }

    // ============ Agent 归档 / 恢复 / 删除（运行时占用检查） ============

    /// 归档 Agent：目录移入 `.archive/`。空闲会话会随操作自动释放；运行中或
    /// 仍有托管后台任务的会话必须先停止/终止。占用检查与目录移动在同一把
    /// sessions 锁内，避免检查后被并发打开抢跑。
    pub fn archive_agent(&self, name: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions
            .iter()
            .any(|(key, session)| key.agent_name == name && self.session_occupied(session))
        {
            return Err(format!(
                "Agent「{name}」仍有会话正在运行或托管后台任务未结束，请先停止/终止后再归档"
            ));
        }
        // 空闲正式会话的 writer 也在这里释放，避免归档后仍有打开的文件句柄；
        // 临时测试同样一并清掉。前端不需要先退出会话界面。
        sessions.retain(|key, _session| key.agent_name != name);
        // `open_session` 也必须先取得这把锁；不要在检查与目录移动之间释放它，
        // 否则并发打开会把一个仍指向活跃目录的 Session 插入 map，再由这里把
        // 目录移走。归档是罕见的同步文件操作，短暂持锁优先于留下悬空会话。
        agents::archive_agent(name)
    }

    /// 恢复归档 Agent（归档时已保证无会话打开，这里只做文件移动）。
    pub fn restore_agent(&self, name: &str) -> Result<(), String> {
        agents::restore_agent(name)
    }

    /// 彻底删除 Agent。空闲会话会随操作自动释放；运行中或仍有托管后台任务的
    /// 会话必须先停止/终止；不可恢复。
    pub fn delete_agent(&self, name: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions
            .iter()
            .any(|(key, session)| key.agent_name == name && self.session_occupied(session))
        {
            return Err(format!(
                "Agent「{name}」仍有会话正在运行或托管后台任务未结束，请先停止/终止后再删除"
            ));
        }
        // 与归档相同：先释放所有空闲会话槽，再做不可逆删除。
        sessions.retain(|key, _session| key.agent_name != name);
        // 与归档相同：占用检查和不可逆删除是同一个临界区，不能让
        // `open_session` 在两者之间抢跑。
        agents::delete_agent(name)
    }

    /// 保存 Agent 定义时，临时测试必须空闲；成功后丢弃旧测试上下文，确保下一轮
    /// 从完整的新定义（模型、权限、环境与工作目录）重新构造。
    pub fn save_agent_definition(&self, def: &AgentDefinition) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.iter().any(|(key, session)| {
            key.agent_name == def.name && session.temporary && self.session_occupied(session)
        }) {
            return Err("临时测试仍在运行或托管后台任务未结束，请先停止/终止后再保存设置".into());
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

    /// 写入 AGENTS.md / memory 后同样让旧测试上下文失效，避免界面声称已经测试了
    /// 尚未注入的文件内容。
    pub fn write_agent_file(
        &self,
        agent_name: &str,
        rel_path: &str,
        content: &str,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if sessions.iter().any(|(key, session)| {
            key.agent_name == agent_name && session.temporary && self.session_occupied(session)
        }) {
            return Err("临时测试仍在运行或托管后台任务未结束，请先停止/终止后再保存文件".into());
        }
        agents::write_agent_file(agent_name, rel_path, content)?;
        sessions.retain(|key, session| !(key.agent_name == agent_name && session.temporary));
        Ok(())
    }

    /// 彻底删除已归档 Agent。不可恢复。
    pub fn delete_archived_agent(&self, name: &str) -> Result<(), String> {
        agents::delete_archived_agent(name)
    }
}

/// 列出指定 Agent 的会话摘要。
pub fn list_sessions(agent_name: &str) -> Result<Vec<SessionSummary>, String> {
    let def = agents::load_agent(agent_name)?;
    let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
    Ok(list_session_summaries(&dir))
}

impl RuntimeState {
    /// 从会话文件载入一条会话（含尾部自愈与账本重建）。不插入 map —— 调用方
    /// 在全部前置步骤成功后再插入。
    pub(super) fn load_session_object(
        def: &AgentDefinition,
        path: &std::path::Path,
    ) -> Result<Session, String> {
        let entries = load_session(path).map_err(|e| e.to_string())?;
        let active = pipi_core::session::active_path(&entries);
        let mut messages = pipi_core::session::rebuild_messages(&active);
        let active_model = pipi_core::session::active_model_from_entries(&entries);
        let effective_model = active_model.as_ref().or(def.provider.as_ref());
        let context_max = effective_model
            .map(|provider| provider.context_window)
            .filter(|window| *window > 0);
        // 载入自愈：上次运行被进程中断时，尾部会留下无人回答的工具调用 ——
        // 补上合成的失败结果并落盘，否则之后每次请求都会被端点按协议拒绝。
        let mut writer = SessionWriter::open(path).map_err(|e| e.to_string())?;
        match settle_unanswered_tail(&mut writer, &messages) {
            Ok(appended) => messages.extend(appended),
            Err(error) => eprintln!("pipi: 会话尾部修复失败（继续载入）: {error}"),
        }
        let mut tracker = SessionStatsTracker::new(context_max);
        for message in &messages {
            tracker.record(message);
        }
        // 摘要压缩的用量也进账本（pi 把摘要成本计入会话总量）：只加累计值，
        // 不改写「最近一次调用」口径 —— 摘要 prompt 不是当前上下文占用。
        for entry in &active {
            if let EntryKind::Compaction {
                usage: Some(usage), ..
            } = &entry.kind
            {
                tracker.record_ledger(usage);
            }
        }
        Ok(Session {
            agent: def.clone(),
            messages: Arc::new(tokio::sync::Mutex::new(messages)),
            writer: Arc::new(Mutex::new(writer)),
            temporary: false,
            stats: Arc::new(Mutex::new(tracker)),
            abort: AbortSignal::new(),
            running: Arc::new(RunState::new()),
            model: Arc::new(Mutex::new(active_model)),
            steering: MessageQueue::new(),
        })
    }

    /// 新建一条会话文件并构造会话对象（返回新会话 id）。不插入 map。
    pub(super) fn create_session_object(
        def: &AgentDefinition,
        model: Option<&Model>,
    ) -> Result<(Session, String), String> {
        let sessions_dir = def.sessions_dir().ok_or("无法解析会话目录")?;
        let mut writer = SessionWriter::create(&sessions_dir).map_err(|e| e.to_string())?;
        let session_id = writer.session_id().to_string();
        let active_model = model.cloned();
        let effective_model = active_model.as_ref().or(def.provider.as_ref());
        let context_max = effective_model
            .map(|provider| provider.context_window)
            .filter(|window| *window > 0);
        if let Some(target) = &active_model
            && Some(target) != def.provider.as_ref()
        {
            writer
                .append_model_change(target)
                .map_err(|e| e.to_string())?;
        }
        let session = Session {
            agent: def.clone(),
            messages: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            writer: Arc::new(Mutex::new(writer)),
            temporary: false,
            stats: Arc::new(Mutex::new(SessionStatsTracker::new(context_max))),
            abort: AbortSignal::new(),
            running: Arc::new(RunState::new()),
            model: Arc::new(Mutex::new(active_model)),
            steering: MessageQueue::new(),
        };
        Ok((session, session_id))
    }

    /// 构造设置工作台的临时测试会话。不创建目录或文件，完整复用 Agent 的模型、
    /// 工具、权限、工作目录与环境契约；只有对话账本本身是易失的。
    fn create_test_session_object(def: &AgentDefinition) -> (Session, String) {
        let writer = SessionWriter::temporary();
        let session_id = writer.session_id().to_string();
        let context_max = def
            .provider
            .as_ref()
            .map(|provider| provider.context_window)
            .filter(|window| *window > 0);
        let session = Session {
            agent: def.clone(),
            messages: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            writer: Arc::new(Mutex::new(writer)),
            temporary: true,
            stats: Arc::new(Mutex::new(SessionStatsTracker::new(context_max))),
            abort: AbortSignal::new(),
            running: Arc::new(RunState::new()),
            model: Arc::new(Mutex::new(None)),
            steering: MessageQueue::new(),
        };
        (session, session_id)
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
        let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
        let path = dir.join(format!("{session_id}.jsonl"));
        if !path.is_file() {
            return Err("会话不存在".into());
        }

        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let key = SessionKey::new(agent_name, session_id);
        if sessions.contains_key(&key) {
            return Ok(());
        }
        let session = Self::load_session_object(&def, &path)?;
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
        let dir = def.sessions_dir().ok_or("无法解析会话目录")?;
        let source_path = dir.join(format!("{session_id}.jsonl"));
        if !source_path.is_file() {
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

        let writer = pipi_core::session::fork_session(&source_path, &dir, up_to_entry_id)?;
        let new_session_path = writer.path().to_path_buf();
        let new_session_id = new_session_path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "无法解析新会话 ID".to_string())?
            .to_string();

        let entries = load_session(&new_session_path).map_err(|e| e.to_string())?;
        let active = pipi_core::session::active_path(&entries);
        let mut messages = pipi_core::session::rebuild_messages(&active);
        let active_model = pipi_core::session::active_model_from_entries(&entries);
        let effective_model = active_model.clone().or_else(|| def.provider.clone());
        let context_max = effective_model
            .as_ref()
            .map(|provider| provider.context_window)
            .filter(|window| *window > 0);
        // 分叉出的会话同样载入自愈：源会话尾部若留下无主工具调用，分叉会
        // 把它原样复制过来（见 settle_unanswered_tail 的说明）。
        let mut writer = writer;
        match settle_unanswered_tail(&mut writer, &messages) {
            Ok(appended) => messages.extend(appended),
            Err(error) => eprintln!("pipi: 会话尾部修复失败（继续载入）: {error}"),
        }
        let mut tracker = SessionStatsTracker::new(context_max);
        for message in &messages {
            tracker.record(message);
        }
        // 摘要压缩的用量也进账本（分叉同样继承）
        for entry in &active {
            if let EntryKind::Compaction {
                usage: Some(usage), ..
            } = &entry.kind
            {
                tracker.record_ledger(usage);
            }
        }

        let run_state = Arc::new(RunState::new());
        // 分叉出的会话作为**新的一条**打开（源会话保持原样，不再被顶掉）
        sessions.insert(
            SessionKey::new(agent_name, &new_session_id),
            Session {
                agent: def,
                messages: Arc::new(tokio::sync::Mutex::new(messages)),
                writer: Arc::new(Mutex::new(writer)),
                temporary: false,
                stats: Arc::new(Mutex::new(tracker)),
                abort: AbortSignal::new(),
                running: run_state.clone(),
                model: Arc::new(Mutex::new(active_model.clone())),
                steering: MessageQueue::new(),
            },
        );

        Ok(SessionInfo {
            agent_name: agent_name.to_string(),
            session_id: new_session_id,
            temporary: false,
            running: false,
            background_tasks: 0,
            run_id: run_state.current_run_id(),
            model: effective_model,
            is_custom_model: active_model.is_some(),
        })
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

    fn session_info_of(&self, session: &Session) -> Result<SessionInfo, String> {
        let writer = session
            .writer
            .lock()
            .map_err(|error| format!("无法读取会话路径: {error}"))?;
        let session_id = writer.session_id().to_string();
        let custom_model = session.model.lock().ok().and_then(|m| m.clone());
        let effective_model = custom_model
            .clone()
            .or_else(|| session.agent.provider.clone());
        let is_custom_model = custom_model.is_some();
        let owner = BackgroundTaskOwner::new(
            session.agent.name.clone(),
            session_id.clone(),
            session.running.current_run_id(),
        );
        Ok(SessionInfo {
            agent_name: session.agent.name.clone(),
            session_id,
            temporary: session.temporary,
            running: session.running.is_running(),
            background_tasks: self.background.active_count(&owner),
            run_id: session.running.current_run_id(),
            model: effective_model,
            is_custom_model,
        })
    }

    fn session_has_active_background_tasks(&self, session: &Session) -> bool {
        let Ok(writer) = session.writer.lock() else {
            return true;
        };
        let owner = BackgroundTaskOwner::new(
            session.agent.name.clone(),
            writer.session_id().to_string(),
            session.running.current_run_id(),
        );
        self.background.active_count(&owner) > 0
    }

    fn session_occupied(&self, session: &Session) -> bool {
        session.running.is_running() || self.session_has_active_background_tasks(session)
    }

    /// 让文件级会话变更可以作用于当前仍显示在前端的空闲会话。
    ///
    /// `Session` 持有追加文件的 writer；先从槽中移除并在本函数返回前释放它，
    /// 再由调用方在同一把 sessions 锁内移动/删除文件。运行中或仍有托管后台
    /// 任务时不允许脱离，避免后台继续向已归档/已删除的路径写入。
    fn detach_idle_session(
        &self,
        sessions: &mut HashMap<SessionKey, Session>,
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
        if session.running.is_running() {
            return Err("会话正在运行，请等待完成或先停止后再切换模型".into());
        }

        let mut current_model_slot = session.model.lock().map_err(|e| e.to_string())?;
        let effective_target = model.clone().or_else(|| session.agent.provider.clone());

        // 校验目标模型的 API 密钥是否已配置
        if let Some(target) = &effective_target {
            let _ = resolve_model(&session.agent, Some(target), None)?;
        }

        let has_changed = *current_model_slot != model;
        if has_changed {
            if let Some(target) = &effective_target {
                let mut writer = session.writer.lock().map_err(|e| e.to_string())?;
                writer
                    .append_model_change(target)
                    .map_err(|e| e.to_string())?;
            }
            if let Some(target) = &effective_target
                && let Ok(mut tracker) = session.stats.lock()
            {
                let context_max = if target.context_window > 0 {
                    Some(target.context_window)
                } else {
                    None
                };
                tracker.set_context_max(context_max);
            }
            *current_model_slot = model;
        }
        Ok(())
    }

    /// 某条会话是否正在跑一轮。
    pub fn session_running(&self, agent_name: &str, session_id: &str) -> bool {
        self.sessions
            .lock()
            .map(|sessions| {
                sessions
                    .get(&SessionKey::new(agent_name, session_id))
                    .map(|session| session.running.is_running())
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// 停止某条会话的当前一轮；这条会话没有打开时是空操作。只停这一条 ——
    /// 同一个 Agent 的其他会话照跑。
    pub fn stop_run(&self, agent_name: &str, session_id: &str) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        if let Some(session) = sessions.get(&SessionKey::new(agent_name, session_id)) {
            session.abort.abort();
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
            session.running.current_run_id()
        };
        let owner = BackgroundTaskOwner::new(agent_name, session_id, run_id);
        self.background
            .query(
                &owner,
                pipi_core::tools::background::BackgroundTaskQuery {
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
            session.running.current_run_id()
        };
        let command = match action {
            "terminate" => pipi_core::tools::background::BackgroundTaskCommand::Terminate,
            "writeStdin" => pipi_core::tools::background::BackgroundTaskCommand::WriteStdin(
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
        if !session.running.is_running() {
            return Err("会话未在运行，请直接发送消息".into());
        }
        session.steering.push(Message::user_text(message));
        Ok(())
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
                .map(|session| session.messages.clone())
        };
        match messages {
            Some(messages) => Ok(messages.lock().await.clone()),
            None => Ok(Vec::new()),
        }
    }

    pub fn session_stats(
        &self,
        agent_name: &str,
        session_id: &str,
    ) -> Result<pipi_core::stats::SessionStats, String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        match sessions.get(&SessionKey::new(agent_name, session_id)) {
            Some(session) => Ok(session.stats.lock().map_err(|e| e.to_string())?.snapshot()),
            None => Ok(Default::default()),
        }
    }
}
