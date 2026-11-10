//! Agent-facing user question tool; all state is owned by the host session.
use crate::runtime::{RuntimeState, SessionKey};
use async_trait::async_trait;
use pipi_protocol::interaction::{Question, QuestionState};
use pipi_protocol::tools::{AgentTool, ToolContext, ToolOutput};
use serde_json::{Value, json};

pub struct AskQuestionTool {
    pub runtime: RuntimeState,
    pub key: SessionKey,
    pub run_id: usize,
}

#[async_trait]
impl AgentTool for AskQuestionTool {
    fn name(&self) -> &str {
        "ask_question"
    }
    fn description(&self) -> String {
        "Ask the user a short question with optional choices and free text. Set sync=true only if the answer is required before continuing; sync=false returns immediately and the answer arrives in a later turn. sync must be explicitly provided.".into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object", "additionalProperties":false, "properties":{
            "question":{"type":"string", "description":"The complete question."},
            "options":{"type":"array", "items":{"type":"string"}, "maxItems":8, "description":"Optional choices; the user can also enter free text."},
            "sync":{"type":"boolean", "description":"True waits for the answer; false continues immediately."}
        }, "required":["question", "sync"]})
    }
    fn requires_sequential(&self) -> bool {
        true
    }
    async fn execute(
        &self,
        ctx: &ToolContext,
        args: &Value,
        _: &(dyn Fn(ToolOutput) + Send + Sync),
    ) -> Result<ToolOutput, String> {
        let question: Question =
            serde_json::from_value(args.clone()).map_err(|e| format!("Invalid question: {e}"))?;
        let sync = question.sync;
        let (id, changed) = self
            .runtime
            .request_question(&self.key, self.run_id, question)?;
        if !sync {
            return Ok(ToolOutput::text(
                json!({"status":"pending","questionId":id}).to_string(),
            ));
        }
        loop {
            let notified = changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if ctx.abort.is_aborted() {
                let _ = self.runtime.finish_sync_question(&self.key, &id, true);
                return Err("Question cancelled because the run was stopped".into());
            }
            match self.runtime.question_state(&self.key, &id)? {
                QuestionState::Answered { answer } => {
                    self.runtime.finish_sync_question(&self.key, &id, false)?;
                    return Ok(ToolOutput::text(
                        json!({"status":"answered","questionId":id,"answer":answer}).to_string(),
                    ));
                }
                QuestionState::Pending => {}
                _ => return Err("Question is no longer awaiting an answer".into()),
            }
            tokio::select! { _ = &mut notified => {}, _ = ctx.abort.wait_aborted() => {} }
        }
    }
}
