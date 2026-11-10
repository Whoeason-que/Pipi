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
        let budget = pipi_context::compaction::Budget::from_window(
            model.context_window,
            def.compact_threshold_percent(),
        )
        .with_target_percent(def.compact_target_percent());
        let settings = load_settings();
        let request = CompactionRequest {
            options: run_stream_options(api_key, &model, session_id, auto_protocol),
            model,
            provider: provider_for(),
            retry: settings.retry.policy(),
            budget,
            policy: CompactionPolicy {
                fork_before_compact: settings.compaction.fork_before_compact,
                archive_original: false,
            },
            mode,
        };
        let hooks = make_session_hooks(
            event_sink,
            def.name.clone(),
            Some(make_rekey(
                self.sessions.clone(),
                self.background.clone(),
                self.browser.clone(),
                self.permission_modes.clone(),
            )),
        );
        let reservation = session.engine.reserve_run(AbortPolicy::Reset)?;
        session.active_permission_mode = None;
        let run_id = reservation.run_id();
        let prepared = reservation.prepare_compaction(request, hooks);
        let wake = session.queue.wake.clone();
        session.queue.active_run = Some(run_id);
        let runtime = self.clone();
        self.spawn_run(async move {
            let result = prepared.run().await;
            if let Err(error) = &result {
                eprintln!("pipi: manual compaction failed: {error}");
            }
            runtime.finish_queued_run(&wake, run_id, result);
        });
        Ok(())
    }
}
