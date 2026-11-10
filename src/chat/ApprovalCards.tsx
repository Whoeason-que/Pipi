import { useState } from "react";
import { useI18n } from "../i18n";
import type { ApprovalDecision, ApprovalRequest } from "../types";

function ApprovalCard({ approval, onAnswer, onError }: { approval: ApprovalRequest; onAnswer: (approval: ApprovalRequest, decision: ApprovalDecision) => Promise<void>; onError: (message: string) => void }) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const answer = async (decision: ApprovalDecision) => {
    if (busy) return;
    setBusy(true);
    try { await onAnswer(approval, decision); }
    catch (error) { onError(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(false); }
  };
  return <section className="approval-card" aria-label={t("chat.approvalTitle")}>
    <strong>{t("chat.approvalTitle")}</strong>
    <p>{approval.reason}</p>
    <div>{approval.toolName}</div>
    <pre>{JSON.stringify(approval.arguments, null, 2)}</pre>
    <div className="approval-actions">
      <button type="button" className="btn" disabled={busy} onClick={() => void answer("allow_once")}>{t("chat.approvalAllowOnce")}</button>
      <button type="button" className="btn" disabled={busy} onClick={() => void answer("deny")}>{t("chat.approvalDeny")}</button>
    </div>
  </section>;
}

export default function ApprovalCards({ approvals, onAnswer, onError }: { approvals: ApprovalRequest[]; onAnswer: (approval: ApprovalRequest, decision: ApprovalDecision) => Promise<void>; onError: (message: string) => void }) {
  return <>{approvals.map(approval => <ApprovalCard key={approval.approvalId} approval={approval} onAnswer={onAnswer} onError={onError} />)}</>;
}
