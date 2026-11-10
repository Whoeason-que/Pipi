//! User questions and answers shared by the host and its transports.
//!
//! A question is not a permission approval: an answer must never be treated as
//! authorization for a separate side effect.

use serde::{Deserialize, Serialize};

/// Arguments supplied by the agent. `sync` deliberately has no default:
/// the agent must choose whether its current tool call waits for the answer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    pub sync: bool,
}

impl Question {
    pub fn validate(&self) -> Result<(), String> {
        if self.question.trim().is_empty() || self.question.len() > 4096 {
            return Err("Question must contain 1 to 4096 bytes".into());
        }
        if self.options.len() > 8 {
            return Err("A question may have at most 8 options".into());
        }
        for (index, option) in self.options.iter().enumerate() {
            if option.trim().is_empty() || option.len() > 512 {
                return Err("Each option must contain 1 to 512 bytes".into());
            }
            if self.options[..index]
                .iter()
                .any(|previous| previous == option)
            {
                return Err("Question options must be unique".into());
            }
        }
        Ok(())
    }
}

/// The optional selected option and free text are independent; either or both
/// may be provided, but a blank submission is not an answer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionAnswer {
    pub selected_option: Option<String>,
    #[serde(default)]
    pub text: String,
}

/// Append-only question lifecycle. A delivered answer is recorded separately
/// from acceptance so recovery can retry delivery without losing the answer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum QuestionEvent {
    Requested {
        question_id: String,
        run_id: usize,
        question: Question,
    },
    Answered {
        question_id: String,
        answer: QuestionAnswer,
    },
    /// Async deliveries carry the accepted answer so this one ledger entry
    /// both acknowledges delivery and replays the user message atomically.
    Delivered {
        question_id: String,
        answer: Option<QuestionAnswer>,
        timestamp: u64,
    },
    Cancelled {
        question_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum QuestionState {
    Pending,
    Answered {
        answer: QuestionAnswer,
    },
    Delivered {
        answer: QuestionAnswer,
    },
    Cancelled,
    /// A synchronous waiter cannot be resumed after the host process exits.
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QuestionRecord {
    pub question_id: String,
    pub run_id: usize,
    pub question: Question,
    pub status: QuestionState,
}

/// Apply a lifecycle event in ledger order. Reject contradictory events rather
/// than allowing a stale or duplicate answer to reopen a completed question.
pub fn apply_question_event(
    records: &mut Vec<QuestionRecord>,
    event: &QuestionEvent,
) -> Result<(), String> {
    match event {
        QuestionEvent::Requested {
            question_id,
            run_id,
            question,
        } => {
            question.validate()?;
            if question_id.is_empty() || records.iter().any(|r| r.question_id == *question_id) {
                return Err("Question ID is empty or already exists".into());
            }
            records.push(QuestionRecord {
                question_id: question_id.clone(),
                run_id: *run_id,
                question: question.clone(),
                status: QuestionState::Pending,
            });
        }
        QuestionEvent::Answered {
            question_id,
            answer,
        } => {
            let record = records
                .iter_mut()
                .find(|r| r.question_id == *question_id)
                .ok_or("Question does not exist")?;
            if !matches!(record.status, QuestionState::Pending) {
                return Err("Question is no longer pending".into());
            }
            answer.validate(&record.question)?;
            record.status = QuestionState::Answered {
                answer: answer.clone(),
            };
        }
        QuestionEvent::Delivered {
            question_id,
            answer: delivered,
            ..
        } => {
            let record = records
                .iter_mut()
                .find(|r| r.question_id == *question_id)
                .ok_or("Question does not exist")?;
            let QuestionState::Answered { answer } = &record.status else {
                return Err("Question has no undelivered answer".into());
            };
            if (record.question.sync && delivered.is_some())
                || (!record.question.sync && delivered.as_ref() != Some(answer))
            {
                return Err("Delivered answer does not match the accepted question answer".into());
            }
            record.status = QuestionState::Delivered {
                answer: answer.clone(),
            };
        }
        QuestionEvent::Cancelled { question_id } => {
            let record = records
                .iter_mut()
                .find(|r| r.question_id == *question_id)
                .ok_or("Question does not exist")?;
            if !matches!(record.status, QuestionState::Pending)
                && !(record.question.sync
                    && matches!(record.status, QuestionState::Answered { .. }))
            {
                return Err("Question is no longer pending".into());
            }
            record.status = QuestionState::Cancelled;
        }
    }
    Ok(())
}

/// Reconstruct question state from the unabridged ledger, including entries
/// before a context compaction. No live synchronous waiter survives reopening.
pub fn restore_questions(
    entries: &[crate::session::SessionEntry],
) -> Result<Vec<QuestionRecord>, String> {
    let mut records = Vec::new();
    for entry in entries {
        if let crate::session::EntryKind::Question { event } = &entry.kind {
            apply_question_event(&mut records, event)?;
        }
    }
    for record in &mut records {
        if record.question.sync
            && matches!(
                record.status,
                QuestionState::Pending | QuestionState::Answered { .. }
            )
        {
            record.status = QuestionState::Interrupted;
        }
    }
    Ok(records)
}

impl QuestionAnswer {
    pub fn validate(&self, question: &Question) -> Result<(), String> {
        if self.text.len() > 65536 {
            return Err("Answer text exceeds 65536 bytes".into());
        }
        if let Some(option) = &self.selected_option
            && !question.options.contains(option)
        {
            return Err("Selected option is not part of this question".into());
        }
        if self.selected_option.is_none() && self.text.trim().is_empty() {
            return Err("Select an option or enter an answer".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sync_must_be_explicit_and_boolean() {
        for value in [
            json!({"question":"Which?"}),
            json!({"question":"Which?","sync":null}),
            json!({"question":"Which?","sync":"true"}),
        ] {
            assert!(serde_json::from_value::<Question>(value).is_err());
        }
        for sync in [true, false] {
            let question: Question =
                serde_json::from_value(json!({"question":"Which?","sync":sync})).unwrap();
            assert_eq!(question.sync, sync);
            question.validate().unwrap();
        }
    }

    #[test]
    fn answers_support_option_text_or_both_and_reject_invalid_submissions() {
        let q = Question {
            question: "Which?".into(),
            options: vec!["A".into(), "B".into()],
            sync: false,
        };
        for answer in [
            QuestionAnswer {
                selected_option: Some("A".into()),
                text: String::new(),
            },
            QuestionAnswer {
                selected_option: None,
                text: "Custom".into(),
            },
            QuestionAnswer {
                selected_option: Some("B".into()),
                text: "With details".into(),
            },
        ] {
            answer.validate(&q).unwrap();
        }
        assert!(
            QuestionAnswer {
                selected_option: None,
                text: "  ".into()
            }
            .validate(&q)
            .is_err()
        );
        assert!(
            QuestionAnswer {
                selected_option: Some("C".into()),
                text: "Custom".into()
            }
            .validate(&q)
            .is_err()
        );
        let no_choices = Question {
            options: vec![],
            ..q
        };
        assert!(
            QuestionAnswer {
                selected_option: Some("A".into()),
                text: String::new()
            }
            .validate(&no_choices)
            .is_err()
        );
    }

    #[test]
    fn malformed_questions_are_rejected() {
        let q = Question {
            question: " ".into(),
            options: vec![],
            sync: true,
        };
        assert!(q.validate().is_err());
        let q = Question {
            question: "Q".into(),
            options: vec!["A".into(), "A".into()],
            sync: true,
        };
        assert!(q.validate().is_err());
    }

    #[test]
    fn question_lifecycle_rejects_duplicate_and_late_answers() {
        let mut records = Vec::new();
        let request = QuestionEvent::Requested {
            question_id: "q1".into(),
            run_id: 4,
            question: Question {
                question: "Which?".into(),
                options: vec!["A".into()],
                sync: false,
            },
        };
        apply_question_event(&mut records, &request).unwrap();
        assert!(apply_question_event(&mut records, &request).is_err());
        let answer = QuestionEvent::Answered {
            question_id: "q1".into(),
            answer: QuestionAnswer {
                selected_option: Some("A".into()),
                text: "details".into(),
            },
        };
        apply_question_event(&mut records, &answer).unwrap();
        assert!(apply_question_event(&mut records, &answer).is_err());
        let delivered = QuestionEvent::Delivered {
            question_id: "q1".into(),
            answer: Some(QuestionAnswer {
                selected_option: Some("A".into()),
                text: "details".into(),
            }),
            timestamp: 1,
        };
        apply_question_event(&mut records, &delivered).unwrap();
        assert!(apply_question_event(&mut records, &delivered).is_err());
        assert!(matches!(records[0].status, QuestionState::Delivered { .. }));
    }
}
