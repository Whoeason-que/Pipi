//! 会话所有权与同步控制；异步读取只捕获消息，不延长账本寿命。
use super::*;

static NEXT_RUN_ID: AtomicUsize = AtomicUsize::new(1);

pub(super) type SharedLog = Arc<Mutex<Box<dyn SessionLog>>>;

pub(super) struct RunState {
    value: AtomicUsize,
    run_id: AtomicUsize,
}

impl RunState {
    fn new() -> Self {
        Self {
            value: AtomicUsize::new(0),
            run_id: AtomicUsize::new(NEXT_RUN_ID.load(Ordering::Acquire).saturating_sub(1)),
        }
    }

    fn begin(&self) -> Result<usize, String> {
        let mut current = self.value.load(Ordering::Acquire);
        loop {
            if current & 1 != 0 {
                return Err("Session is running, please wait for completion or stop it first".into());
            }
            let next = (((current >> 1).wrapping_add(1)) << 1) | 1;
            match self
                .value
                .compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    self.run_id.store(
                        NEXT_RUN_ID.fetch_add(1, Ordering::AcqRel),
                        Ordering::Release,
                    );
                    return Ok(next);
                }
                Err(actual) => current = actual,
            }
        }
    }

    pub(super) fn finish(&self, token: usize) {
        let _ = self
            .value
            .compare_exchange(token, token & !1, Ordering::AcqRel, Ordering::Acquire);
    }

    pub(super) fn is_running(&self) -> bool {
        self.value.load(Ordering::Acquire) & 1 != 0
    }

    pub(super) fn run_id(&self) -> usize {
        self.run_id.load(Ordering::Acquire)
    }
}

pub(super) struct RunningGuard {
    state: Arc<RunState>,
    token: usize,
}

impl RunningGuard {
    pub(super) fn new(state: Arc<RunState>) -> Result<Self, String> {
        let token = state.begin()?;
        Ok(Self { state, token })
    }

    pub(super) fn run_id(&self) -> usize {
        self.state.run_id()
    }
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.state.finish(self.token);
    }
}

#[derive(Clone)]
pub(super) struct SessionHandle {
    pub(super) messages: Arc<Mutex<Vec<Message>>>,
    pub(super) writer: SharedLog,
    pub(super) repo: Option<Arc<dyn SessionRepo>>,
    pub(super) stats: Arc<Mutex<SessionStatsTracker>>,
    pub(super) abort: AbortSignal,
    pub(super) running: Arc<RunState>,
    pub(super) model: Arc<Mutex<Option<Model>>>,
    pub(super) steering: MessageQueue,
    pub(super) control: Arc<Mutex<()>>,
}

impl SessionHandle {
    pub(super) fn session_id(&self) -> Result<String, String> {
        self.writer
            .lock()
            .map(|w| w.session_id().into())
            .map_err(|e| e.to_string())
    }
}

/// 单会话所有者不可 Clone；外部快照不会持有 writer。
pub struct AgentSession {
    state: SessionHandle,
}

impl AgentSession {
    pub fn create(
        storage: SessionStorage,
        default_model: Option<Model>,
        selected_model: Option<Model>,
        abort: AbortSignal,
    ) -> Result<Self, String> {
        let mut log = storage.log;
        if let Some(model) = &selected_model
            && Some(model) != default_model.as_ref()
        {
            log.append_model_change(model).map_err(|e| e.to_string())?;
        }
        // 提交点（会话创建）：空会话也要在列表里可见
        log.commit_point()
            .map_err(|error| format!("Cannot commit new session: {error}"))?;
        let context_max = selected_model
            .as_ref()
            .or(default_model.as_ref())
            .map(|m| m.context_window)
            .filter(|w| *w > 0);
        Ok(Self::from_parts(
            SessionStorage {
                log,
                repo: storage.repo,
            },
            Vec::new(),
            SessionStatsTracker::new(context_max),
            selected_model,
            abort,
        ))
    }

    pub fn restore(
        storage: SessionStorage,
        entries: Vec<SessionEntry>,
        default_model: Option<Model>,
        abort: AbortSignal,
    ) -> Result<Self, String> {
        let active = active_path(&entries);
        let mut messages = rebuild_messages(&active);
        let model = active_model_from_entries(&entries);
        let context_max = model
            .as_ref()
            .or(default_model.as_ref())
            .map(|m| m.context_window)
            .filter(|w| *w > 0);
        let mut log = storage.log;
        match ledger::settle_unanswered_tail(&mut *log, &messages) {
            Ok(appended) => {
                if !appended.is_empty() {
                    // 提交点（尾部修复）：超时 / 中止收尾可能把半配对的批次写进
                    // 载体（提交边界只保证整轮，不保证配对完整），载入时补上
                    // 合成的失败结果并提交 —— 之后每次打开读到的都是合法记录。
                    if let Err(error) = log.commit_point() {
                        eprintln!("pipi: 会话尾部修复未能提交: {error}");
                    }
                }
                messages.extend(appended);
            }
            Err(error) => eprintln!("pipi: 会话尾部修复失败（继续载入）: {error}"),
        }
        let mut stats = SessionStatsTracker::new(context_max);
        // 替换式压缩只改变上下文投影；累计账本仍包含被摘要替换的请求。
        for entry in active {
            match &entry.kind {
                EntryKind::Message { message } => stats.record(message),
                EntryKind::Compaction {
                    usage: Some(usage), ..
                } => stats.record_ledger(usage),
                _ => {}
            }
        }
        Ok(Self::from_parts(
            SessionStorage {
                log,
                repo: storage.repo,
            },
            messages,
            stats,
            model,
            abort,
        ))
    }

    fn from_parts(
        storage: SessionStorage,
        messages: Vec<Message>,
        stats: SessionStatsTracker,
        model: Option<Model>,
        abort: AbortSignal,
    ) -> Self {
        Self {
            state: SessionHandle {
                messages: Arc::new(Mutex::new(messages)),
                writer: Arc::new(Mutex::new(storage.log)),
                repo: storage.repo,
                stats: Arc::new(Mutex::new(stats)),
                abort,
                running: Arc::new(RunState::new()),
                model: Arc::new(Mutex::new(model)),
                steering: MessageQueue::new(),
                control: Arc::new(Mutex::new(())),
            },
        }
    }

    pub fn snapshot(&self) -> Result<SessionSnapshot, String> {
        let _control = self.state.control.lock().map_err(|e| e.to_string())?;
        let (session_id, temporary) = {
            let writer = self.state.writer.lock().map_err(|e| e.to_string())?;
            (writer.session_id().to_string(), writer.is_temporary())
        };
        Ok(SessionSnapshot {
            session_id,
            temporary,
            running: self.state.running.is_running(),
            run_id: self.state.running.run_id(),
            selected_model: self.state.model.lock().map_err(|e| e.to_string())?.clone(),
        })
    }

    pub fn messages(&self) -> impl std::future::Future<Output = Vec<Message>> + Send + 'static {
        let messages = self.state.messages.clone();
        async move { messages.lock().expect("session history poisoned").clone() }
    }

    pub fn message_history(&self) -> Result<Vec<Message>, String> {
        self.state
            .messages
            .lock()
            .map(|msgs| msgs.clone())
            .map_err(|e| e.to_string())
    }

    pub fn stats(&self) -> Result<SessionStats, String> {
        self.state
            .stats
            .lock()
            .map(|s| s.snapshot())
            .map_err(|e| e.to_string())
    }

    pub fn abort_signal(&self) -> AbortSignal {
        self.state.abort.clone()
    }

    pub fn record_env(&self, declarations: Vec<EnvDeclared>) -> Result<(), String> {
        let mut writer = self.state.writer.lock().map_err(|e| e.to_string())?;
        writer.append_env(declarations).map_err(|e| e.to_string())?;
        // 提交点（env 记账）：与创建同属轮外提交，让首轮之前的状态可见
        writer
            .commit_point()
            .map_err(|error| format!("Cannot commit environment ledger: {error}"))?;
        Ok(())
    }

    pub fn set_model(&self, model: Option<Model>, effective: Option<Model>) -> Result<(), String> {
        let _control = self.state.control.lock().map_err(|e| e.to_string())?;
        if self.state.running.is_running() {
            return Err("Session is running, please wait for completion or stop it before switching models".into());
        }
        let mut selected = self.state.model.lock().map_err(|e| e.to_string())?;
        if *selected != model {
            if let Some(target) = &effective {
                let mut writer = self.state.writer.lock().map_err(|e| e.to_string())?;
                writer
                    .append_model_change(target)
                    .map_err(|e| e.to_string())?;
                // 提交点（手动切模型）：空闲时的模型变更入史
                writer
                    .commit_point()
                    .map_err(|error| format!("Cannot commit model change: {error}"))?;
                drop(writer);
                if let Ok(mut tracker) = self.state.stats.lock() {
                    tracker.set_context_max(
                        (target.context_window > 0).then_some(target.context_window),
                    );
                }
            }
            *selected = model;
        }
        Ok(())
    }

    pub fn stop(&self) -> Result<(), String> {
        let _control = self.state.control.lock().map_err(|e| e.to_string())?;
        self.state.abort.abort();
        Ok(())
    }

    pub fn steer(&self, text: &str) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Message cannot be empty".into());
        }
        let _control = self.state.control.lock().map_err(|e| e.to_string())?;
        if !self.state.running.is_running() {
            return Err("Session is not running, please send message directly".into());
        }
        self.state.steering.push(Message::user_text(text));
        Ok(())
    }

    pub fn reserve_run(&self, policy: AbortPolicy) -> Result<RunReservation, String> {
        let _control = self.state.control.lock().map_err(|e| e.to_string())?;
        let guard = RunningGuard::new(self.state.running.clone())?;
        if matches!(policy, AbortPolicy::Reset) {
            self.state.abort.reset();
        }
        Ok(RunReservation {
            session: self.state.clone(),
            guard,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_generation_cannot_clear_new_run() {
        let state = Arc::new(RunState::new());
        let old = RunningGuard::new(state.clone()).unwrap();
        assert!(RunningGuard::new(state.clone()).is_err());
        state.finish(old.token);
        let new = RunningGuard::new(state.clone()).unwrap();
        drop(old);
        assert!(state.is_running());
        drop(new);
        assert!(!state.is_running());
    }

    #[tokio::test]
    async fn cancellation_and_panic_release_guard() {
        let state = Arc::new(RunState::new());
        let guard = RunningGuard::new(state.clone()).unwrap();
        let (started, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _guard = guard;
            let _ = started.send(());
            std::future::pending::<()>().await;
        });
        ready.await.unwrap();
        task.abort();
        assert!(task.await.is_err());
        assert!(!state.is_running());
        let guard = RunningGuard::new(state.clone()).unwrap();
        assert!(
            tokio::spawn(async move {
                let _guard = guard;
                panic!("test panic")
            })
            .await
            .is_err()
        );
        assert!(!state.is_running());
    }
}
