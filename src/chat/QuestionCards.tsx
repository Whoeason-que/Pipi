import { useState } from "react";
import { useI18n } from "../i18n";
import type { QuestionAnswer, QuestionRecord } from "../types";

function QuestionCard({ record, onAnswer, onError }: { record: QuestionRecord; onAnswer: (id: string, answer: QuestionAnswer) => Promise<void>; onError: (message: string) => void }) {
  const { t } = useI18n();
  const [selected, setSelected] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    if (busy || (!selected && !text.trim())) return;
    setBusy(true);
    try { await onAnswer(record.questionId, { selectedOption: selected, text }); }
    catch (error) { onError(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(false); }
  };
  return <section className="question-card" aria-label={t("chat.questionTitle")}>
    <div className="question-card-title">{t("chat.questionTitle")}{record.question.sync ? ` · ${t("chat.questionWaiting")}` : ""}</div>
    <p>{record.question.question}</p>
    {!!record.question.options?.length && <div className="question-options" role="group" aria-label={t("chat.questionChoices")}>
      {record.question.options.map(option => <button type="button" key={option} aria-pressed={selected === option} onClick={() => setSelected(selected === option ? null : option)} disabled={busy}>{option}</button>)}
    </div>}
    <textarea value={text} onChange={event => setText(event.target.value)} placeholder={t("chat.questionPlaceholder")} aria-label={t("chat.questionPlaceholder")} disabled={busy} />
    <button type="button" className="btn" disabled={busy || (!selected && !text.trim())} onClick={() => void submit()}>{t("chat.questionSubmit")}</button>
  </section>;
}

export default function QuestionCards({ questions, onAnswer, onError }: { questions: QuestionRecord[]; onAnswer: (id: string, answer: QuestionAnswer) => Promise<void>; onError: (message: string) => void }) {
  const { t } = useI18n();
  const previous = questions.filter(q => q.status.state !== "pending");
  return <>
    {questions.filter(q => q.status.state === "pending").map(q => <QuestionCard key={q.questionId} record={q} onAnswer={onAnswer} onError={onError} />)}
    {!!previous.length && <details className="question-history">
      <summary>{t("chat.questionHistory", { count: previous.length })}</summary>
      {previous.map(q => <div key={q.questionId} className="question-history-item">
        <strong>{q.question.question}</strong>
        {(q.status.state === "answered" || q.status.state === "delivered")
          ? <p>{[q.status.answer.selectedOption, q.status.answer.text].filter(Boolean).join(" · ")}</p>
          : <p>{t(q.status.state === "interrupted" ? "chat.questionInterrupted" : "chat.questionCancelled")}</p>}
      </div>)}
    </details>}
  </>;
}
