//! 手动压缩的产品装配；自动与手动的执行均由单会话内核拥有。
use super::*;

impl RuntimeState {
    pub fn compact_now(
        &self,
        agent_name: &str,
        session_id: &str,
        mode: pipi_context::compaction::CompactMode,
        event_sink: EventEmitter,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        self.ensure_not_shutting_down()?;
        let key = SessionKey::new(agent_name, session_id);
        let session = sessions
            .get_mut(&key)
            .ok_or_else(|| format!("Session \"{session_id}\" is not open"))?;
        let snapshot = session.engine.snapshot()?;
        if snapshot.running {
            return Err("Session is running, please stop it before compacting".into());
        }
        let def = &session.agent;
        let ResolvedModel {
            model,
            api_key,
            auto_protocol,
        } = resolve_model(def, snapshot.selected_model.as_ref(), None)?;
        let budget = Self::compaction_budget(def, model.context_window);
        let settings = load_settings();
        let request = CompactionRequest {
            options: run_stream_options(api_key, &model, session_id, auto_protocol),
            model,
            provider: provider_for(),
            retry: settings.retry.policy(),
            budget,
            policy: Self::compaction_policy(&settings.compaction),
            mode,
        };
        let hooks = make_session_hooks(event_sink, def.name.clone(), Some(self.session_rekey()));
        let reservation = session.engine.reserve_run(AbortPolicy::Reset)?;
        session.active_permission_mode = None;
        let run_id = reservation.run_id();
        let prepared = reservation.prepare_compaction(request, hooks);
        self.start_prepared_run(session, prepared, run_id, "manual compaction failed");
        Ok(())
    }
}
