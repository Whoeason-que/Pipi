//! 手动压缩的产品装配；自动与手动的执行均由单会话内核拥有。
use super::*;

impl RuntimeState {
    pub fn compact_now(
        &self,
        agent_name: &str,
        session_id: &str,
        event_sink: EventEmitter,
    ) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let key = SessionKey::new(agent_name, session_id);
        let session = sessions
            .get(&key)
            .ok_or_else(|| format!("会话「{session_id}」没有打开"))?;
        let snapshot = session.engine.snapshot()?;
        if snapshot.running {
            return Err("会话正在运行，请先停止再压缩".into());
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
                archive_original: settings.compaction.archive_original,
            },
        };
        let hooks = make_session_hooks(
            event_sink,
            def.name.clone(),
            Some(make_rekey(self.sessions.clone(), self.background.clone())),
        );
        let prepared = session
            .engine
            .reserve_run(AbortPolicy::Reset)?
            .prepare_compaction(request, hooks);
        self.spawn_run(async move {
            if let Err(error) = prepared.run().await {
                eprintln!("pipi: 手动压缩失败: {error}");
            }
        });
        Ok(())
    }
}
